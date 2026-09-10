//! X11 input thread.
//!
//! The mapping tables it applies — button meanings, cursor names, keycode
//! offsets, event masks — live in [`crate::input_logic`].

use std::ffi::c_int;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use calloop::EventLoop;
use calloop::channel::{Channel, Event as ChannelEvent, Sender, channel};

use x11rb::connection::Connection as X11rbConnection;
use x11rb::cursor::Handle as X11rbCursorHandle;
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux as X11rbChangeWindowAttributesAux,
    ConnectionExt as X11rbXprotoConnection,
};
use x11rb::resource_manager::new_from_default;
use x11rb::rust_connection::RustConnection;
use xcb::{Xid, XidNew, x};
use xkbcommon::xkb::{self, x11 as xkb_x11};

use jfn_input::{
    jfn_input_dispatch_char, jfn_input_dispatch_history_nav, jfn_input_dispatch_mouse_button,
    jfn_input_dispatch_mouse_move, jfn_input_dispatch_scroll,
};
use jfn_linux_util::input::jfn_input_dispatch_key_raw;
use jfn_playback::shutdown::jfn_shutdown_register_waker;
use jfn_wake_event::{Drain, WakeEvent, WakeSource};

use crate::conn_source::XcbSource;
use crate::input_logic::{
    ButtonAction, apply_button_flag, cef_cursor_to_icon, cef_modifiers as combine_modifiers,
    classify_button, cursor_shape_from_raw, history_nav_for_sym, native_key_code,
    overlay_event_mask, overlay_grab_event_mask, toplevel_event_mask,
};

use jfn_linux_util::xkb::to_cef_mods;
use jfn_platform_abi::cursor::CursorShape;

#[derive(Clone)]
pub(crate) struct CursorChannel {
    tx: Sender<CursorShape>,
    latest: Arc<AtomicU32>,
}

impl CursorChannel {
    fn new() -> (CursorChannel, Channel<CursorShape>) {
        let (tx, rx) = channel();
        let ch = CursorChannel {
            tx,
            latest: Arc::new(AtomicU32::new(CursorShape::Pointer.as_raw() as u32)),
        };
        (ch, rx)
    }

    fn set(&self, shape: CursorShape) {
        self.latest.store(shape.as_raw() as u32, Ordering::Release);
        let _ = self.tx.send(shape);
    }

    fn resend_latest(&self) {
        let shape = cursor_shape_from_raw(self.latest.load(Ordering::Acquire));
        let _ = self.tx.send(shape);
    }
}

pub struct Handle {
    join: Option<std::thread::JoinHandle<()>>,
    cursor_join: Option<std::thread::JoinHandle<()>>,
    input_join: Option<std::thread::JoinHandle<()>>,
    cursor: Option<CursorChannel>,
    dispatch: Option<Sender<QueuedInputEvent>>,
}

impl Handle {
    pub fn join(&mut self) {
        if let Some(ev) = x11_shutdown_waker() {
            ev.signal();
        }
        // The input thread is the producer for both channels, so it must be
        // gone before either sender drops — otherwise a queued event is lost.
        if let Some(j) = self.join.take()
            && let Err(e) = j.join()
        {
            eprintln!("[x11] input thread panicked: {e:?}");
        }
        self.cursor = None;
        self.dispatch = None;
        if let Some(j) = self.cursor_join.take()
            && let Err(e) = j.join()
        {
            eprintln!("[x11] cursor thread panicked: {e:?}");
        }
        if let Some(j) = self.input_join.take()
            && let Err(e) = j.join()
        {
            eprintln!("[x11] input dispatch thread panicked: {e:?}");
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.join();
    }
}

enum QueuedInputEvent {
    KeyRaw {
        sym: u32,
        native: u32,
        modifiers: u32,
        pressed: c_int,
    },
    Char {
        cp: u32,
        modifiers: u32,
        native: u32,
    },
    HistoryNav {
        forward: c_int,
    },
    MouseButton {
        code: u32,
        pressed: c_int,
        x: i32,
        y: i32,
        modifiers: u32,
    },
    MouseMove {
        x: i32,
        y: i32,
        modifiers: u32,
        leave: c_int,
    },
    Scroll {
        x: i32,
        y: i32,
        dx: i32,
        dy: i32,
        modifiers: u32,
    },
}

struct State {
    conn: Arc<xcb::Connection>,
    window: u32,
    root: u32,
    net_active_window: u32,
    xkb_ctx: xkb::Context,
    xkb_kmap: Option<xkb::Keymap>,
    xkb_st: Option<xkb::State>,
    xkb_device_id: i32,
    xkb_base_event: u8,
    modifiers: u32,

    ptr_x: i32,
    ptr_y: i32,
    mouse_button_modifiers: u32,

    cursor: CursorChannel,
    dispatch: Sender<QueuedInputEvent>,
}

unsafe impl Send for State {}

fn setup_xkb(conn: &xcb::Connection, st: &mut State) -> bool {
    let mut major = 0u16;
    let mut minor = 0u16;
    let mut base_event = 0u8;
    let mut base_error = 0u8;
    if !xkb_x11::setup_xkb_extension(
        conn,
        xkb_x11::MIN_MAJOR_XKB_VERSION,
        xkb_x11::MIN_MINOR_XKB_VERSION,
        xkb_x11::SetupXkbExtensionFlags::NoFlags,
        &mut major,
        &mut minor,
        &mut base_event,
        &mut base_error,
    ) {
        return false;
    }
    st.xkb_base_event = base_event;

    let device_id = xkb_x11::get_core_keyboard_device_id(conn);
    if device_id < 0 {
        return false;
    }
    st.xkb_device_id = device_id;

    let kmap =
        xkb_x11::keymap_new_from_device(&st.xkb_ctx, conn, device_id, xkb::KEYMAP_COMPILE_NO_FLAGS);
    if kmap.get_raw_ptr().is_null() {
        return false;
    }
    let state = xkb_x11::state_new_from_device(&kmap, conn, device_id);
    if state.get_raw_ptr().is_null() {
        return false;
    }
    st.xkb_kmap = Some(kmap);
    st.xkb_st = Some(state);

    let required_map = xcb::xkb::MapPart::KEY_TYPES
        | xcb::xkb::MapPart::KEY_SYMS
        | xcb::xkb::MapPart::MODIFIER_MAP
        | xcb::xkb::MapPart::EXPLICIT_COMPONENTS
        | xcb::xkb::MapPart::KEY_ACTIONS
        | xcb::xkb::MapPart::VIRTUAL_MODS
        | xcb::xkb::MapPart::VIRTUAL_MOD_MAP;
    let required_events = xcb::xkb::EventType::STATE_NOTIFY
        | xcb::xkb::EventType::MAP_NOTIFY
        | xcb::xkb::EventType::NEW_KEYBOARD_NOTIFY;

    conn.send_request(&xcb::xkb::SelectEvents {
        device_spec: device_id as xcb::xkb::DeviceSpec,
        affect_which: required_events,
        clear: xcb::xkb::EventType::empty(),
        select_all: required_events,
        affect_map: required_map,
        map: required_map,
        details: &[],
    });
    true
}

fn update_keymap(conn: &xcb::Connection, st: &mut State) {
    let kmap = xkb_x11::keymap_new_from_device(
        &st.xkb_ctx,
        conn,
        st.xkb_device_id,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    );
    if kmap.get_raw_ptr().is_null() {
        return;
    }
    let new_state = xkb_x11::state_new_from_device(&kmap, conn, st.xkb_device_id);
    if new_state.get_raw_ptr().is_null() {
        return;
    }
    st.xkb_kmap = Some(kmap);
    st.xkb_st = Some(new_state);
}

fn cef_modifiers(st: &State) -> u32 {
    combine_modifiers(st.modifiers, st.mouse_button_modifiers)
}

fn to_logical(physical: i32) -> i32 {
    crate::scale_logic::logical_from_physical(physical, crate::x11_state::parent_snapshot().scale)
}

fn handle_key(st: &mut State, detail: u8, pressed: bool) {
    let Some(xst) = st.xkb_st.as_mut() else {
        return;
    };
    let kc_raw = detail as u32;
    let kc = xkb::Keycode::new(kc_raw);
    let sym: u32 = xst.key_get_one_sym(kc).raw();

    if let Some(forward) = history_nav_for_sym(sym) {
        if pressed {
            let _ = st.dispatch.send(QueuedInputEvent::HistoryNav {
                forward: c_int::from(forward),
            });
        }
        xst.update_key(
            kc,
            if pressed {
                xkb::KeyDirection::Down
            } else {
                xkb::KeyDirection::Up
            },
        );
        return;
    }

    let native = native_key_code(kc_raw);
    let _ = st.dispatch.send(QueuedInputEvent::KeyRaw {
        sym,
        native,
        modifiers: st.modifiers,
        pressed: pressed as c_int,
    });

    if pressed {
        let cp = xst.key_get_utf32(kc);
        if cp > 0 {
            let _ = st.dispatch.send(QueuedInputEvent::Char {
                cp,
                modifiers: st.modifiers,
                native,
            });
        }
    }

    xst.update_key(
        kc,
        if pressed {
            xkb::KeyDirection::Down
        } else {
            xkb::KeyDirection::Up
        },
    );
    st.modifiers = to_cef_mods(xst);
}

fn handle_button(st: &mut State, detail: u8, event_x: i16, event_y: i16, pressed: bool) {
    let x = to_logical(event_x as i32);
    let y = to_logical(event_y as i32);

    match classify_button(detail as u32) {
        ButtonAction::Scroll { dx, dy } => {
            if !pressed {
                return;
            }
            let _ = st.dispatch.send(QueuedInputEvent::Scroll {
                x,
                y,
                dx,
                dy,
                modifiers: cef_modifiers(st),
            });
        }
        ButtonAction::HistoryNav { forward } => {
            if pressed {
                let _ = st.dispatch.send(QueuedInputEvent::HistoryNav {
                    forward: c_int::from(forward),
                });
            }
        }
        // `code` is a linux/input-event-codes.h button code: what the browser
        // bridge speaks.
        ButtonAction::Press { flag, code } => {
            st.mouse_button_modifiers = apply_button_flag(st.mouse_button_modifiers, flag, pressed);
            if pressed {
                activate_parent(st);
            }
            let _ = st.dispatch.send(QueuedInputEvent::MouseButton {
                code,
                pressed: pressed as c_int,
                x,
                y,
                modifiers: cef_modifiers(st),
            });
        }
        ButtonAction::Ignore => {}
    }
}

fn activate_parent(st: &State) {
    if st.root == 0 || st.net_active_window == 0 {
        return;
    }
    let ev = x::ClientMessageEvent::new(
        x::Window::new(st.window),
        x::Atom::new(st.net_active_window),
        x::ClientMessageData::Data32([2, 0, 0, 0, 0]),
    );
    st.conn.send_request(&x::SendEvent {
        propagate: false,
        destination: x::SendEventDest::Window(x::Window::new(st.root)),
        event_mask: x::EventMask::SUBSTRUCTURE_NOTIFY | x::EventMask::SUBSTRUCTURE_REDIRECT,
        event: &ev,
    });
    let _ = st.conn.flush();
}

fn handle_motion(st: &mut State, ev: &xcb::x::MotionNotifyEvent) {
    st.ptr_x = to_logical(ev.event_x() as i32);
    st.ptr_y = to_logical(ev.event_y() as i32);
    let _ = st.dispatch.send(QueuedInputEvent::MouseMove {
        x: st.ptr_x,
        y: st.ptr_y,
        modifiers: cef_modifiers(st),
        leave: 0,
    });
}

fn handle_enter(st: &mut State, ev: &xcb::x::EnterNotifyEvent) {
    st.ptr_x = to_logical(ev.event_x() as i32);
    st.ptr_y = to_logical(ev.event_y() as i32);
    st.cursor.resend_latest();
    let _ = st.dispatch.send(QueuedInputEvent::MouseMove {
        x: st.ptr_x,
        y: st.ptr_y,
        modifiers: cef_modifiers(st),
        leave: 0,
    });
}

fn handle_leave(st: &State, _ev: &xcb::x::LeaveNotifyEvent) {
    let _ = st.dispatch.send(QueuedInputEvent::MouseMove {
        x: st.ptr_x,
        y: st.ptr_y,
        modifiers: cef_modifiers(st),
        leave: 1,
    });
}

fn handle_xkb_state_notify(st: &mut State, ev: &xcb::xkb::StateNotifyEvent) {
    if let Some(xst) = st.xkb_st.as_mut() {
        xst.update_mask(
            ev.base_mods().bits(),
            ev.latched_mods().bits(),
            ev.locked_mods().bits(),
            ev.base_group() as u32,
            ev.latched_group() as u32,
            ev.locked_group() as u32,
        );
        st.modifiers = to_cef_mods(xst);
    }
}

struct CursorState {
    conn: Arc<RustConnection>,
    window: u32,
    // Never freed: `load_cursor` caches by name and hands back the same id, so
    // freeing leaves a dangling id the next lookup would re-hand out.
    cache: std::collections::HashMap<CursorShape, u32>,
    cursor_handle: Option<X11rbCursorHandle>,
}

unsafe impl Send for CursorState {}

fn live_overlay_windows() -> Vec<u32> {
    crate::x11_state::overlay_windows().as_ref().clone()
}

fn apply_cursor(st: &mut CursorState, shape: CursorShape) {
    let conn = &st.conn;
    // Pointer sits over the grabbed overlay windows, so the cursor must be set on
    // them, not the mpv window beneath.
    let windows = live_overlay_windows();
    if windows.is_empty() {
        return;
    }

    let cursor_id = match st.cache.get(&shape) {
        Some(&id) => id,
        None => {
            let id = if shape == CursorShape::None {
                let Ok(pix) = conn.generate_id() else {
                    return;
                };
                let _ = conn.create_pixmap(1, pix, st.window, 1, 1);
                let Ok(blank) = conn.generate_id() else {
                    let _ = conn.free_pixmap(pix);
                    return;
                };
                let _ = conn.create_cursor(blank, pix, pix, 0, 0, 0, 0, 0, 0, 0, 0);
                let _ = conn.free_pixmap(pix);
                blank
            } else {
                let Some(cursor_handle) = st.cursor_handle.as_ref() else {
                    return;
                };
                let name = cef_cursor_to_icon(shape).name();
                let Ok(id) = cursor_handle.load_cursor(&**conn, name) else {
                    return;
                };
                if id == 0 {
                    return;
                }
                id
            };
            st.cache.insert(shape, id);
            id
        }
    };

    for w in &windows {
        let aux = X11rbChangeWindowAttributesAux::new().cursor(cursor_id);
        let _ = conn.change_window_attributes(*w, &aux);
    }
    let _ = conn.flush();
}

/// Per-process X11 shutdown waker. Allocated on first use and registered
/// with the shutdown fan-out so the input loop can wait on its fd alongside
/// xcb. The geometry loop waits on the same fd.
pub(crate) fn x11_shutdown_waker() -> Option<&'static WakeEvent> {
    use std::sync::OnceLock;
    static EV: OnceLock<Option<&'static WakeEvent>> = OnceLock::new();
    *EV.get_or_init(|| {
        let leaked: &'static WakeEvent = Box::leak(Box::new(WakeEvent::new()?));
        jfn_shutdown_register_waker(leaked);
        Some(leaked)
    })
}

fn input_thread_body(mut st: State) {
    if !setup_xkb(&st.conn.clone(), &mut st) {
        eprintln!("[x11] xkb setup failed; key input disabled");
    }

    // Selected on the same xcb connection this thread polls; event masks are
    // per-client.
    let mask = toplevel_event_mask();
    st.conn.send_request(&x::ChangeWindowAttributes {
        window: x::Window::new(st.window),
        value_list: &[x::Cw::EventMask(mask)],
    });
    let _ = st.conn.flush();

    let mut event_loop: EventLoop<'_, State> = match EventLoop::try_new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("[x11] failed to create input event loop: {e}");
            return;
        }
    };
    let signal = event_loop.get_signal();
    let handle = event_loop.handle();

    if let Err(e) =
        handle.insert_source(XcbSource::new(st.conn.clone()), |ev, (), st: &mut State| {
            handle_event(st, ev);
        })
    {
        eprintln!("[x11] failed to register xcb event source: {e}");
        return;
    }

    if let Some(ev) = x11_shutdown_waker() {
        // `Drain::Never`: the geometry loop waits on the same eventfd, and
        // level-triggered-undrained is what lets both threads see one signal.
        let res = handle.insert_source(WakeSource::new(ev.fd(), Drain::Never), move |(), (), _| {
            signal.stop();
        });
        if let Err(e) = res {
            eprintln!("[x11] failed to register shutdown source: {e}");
            return;
        }
    }

    if let Err(e) = event_loop.run(None, &mut st, |_| {}) {
        eprintln!("[x11] input event loop exited: {e}");
    }
}

fn cursor_thread_body(screen_num: i32, window: u32, requests: Channel<CursorShape>) {
    let Some(conn) = crate::x11_state::x11rb_conn() else {
        return;
    };
    let cursor_handle = new_from_default(&*conn).ok().and_then(|db| {
        X11rbCursorHandle::new(&*conn, screen_num as usize, &db)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
    });
    if cursor_handle.is_none() {
        eprintln!("[x11] x11rb cursor handle creation failed");
    }

    let mut st = CursorState {
        conn,
        window,
        cache: std::collections::HashMap::new(),
        cursor_handle,
    };

    let mut event_loop: EventLoop<'_, CursorState> = match EventLoop::try_new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("[x11] failed to create cursor event loop: {e}");
            return;
        }
    };
    let signal = event_loop.get_signal();
    let res = event_loop
        .handle()
        .insert_source(requests, move |ev, (), st: &mut CursorState| match ev {
            ChannelEvent::Msg(shape) => apply_cursor(st, shape),
            ChannelEvent::Closed => signal.stop(),
        });
    if let Err(e) = res {
        eprintln!("[x11] failed to register cursor request source: {e}");
        return;
    }

    if let Err(e) = event_loop.run(None, &mut st, |_| {}) {
        eprintln!("[x11] cursor event loop exited: {e}");
    }
}

fn dispatch_input_event(ev: QueuedInputEvent) {
    match ev {
        QueuedInputEvent::KeyRaw {
            sym,
            native,
            modifiers,
            pressed,
        } => jfn_input_dispatch_key_raw(sym, native, modifiers, pressed),
        QueuedInputEvent::Char {
            cp,
            modifiers,
            native,
        } => jfn_input_dispatch_char(cp, modifiers, native),
        QueuedInputEvent::HistoryNav { forward } => jfn_input_dispatch_history_nav(forward),
        QueuedInputEvent::MouseButton {
            code,
            pressed,
            x,
            y,
            modifiers,
        } => jfn_input_dispatch_mouse_button(code, pressed, x, y, modifiers),
        QueuedInputEvent::MouseMove {
            x,
            y,
            modifiers,
            leave,
        } => jfn_input_dispatch_mouse_move(x, y, modifiers, leave),
        QueuedInputEvent::Scroll {
            x,
            y,
            dx,
            dy,
            modifiers,
        } => jfn_input_dispatch_scroll(x, y, dx, dy, modifiers),
    }
}

fn input_dispatch_thread_body(events: Channel<QueuedInputEvent>) {
    let mut event_loop: EventLoop<'_, ()> = match EventLoop::try_new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("[x11] failed to create input dispatch event loop: {e}");
            return;
        }
    };
    let signal = event_loop.get_signal();
    let res = event_loop
        .handle()
        .insert_source(events, move |ev, (), ()| match ev {
            ChannelEvent::Msg(ev) => dispatch_input_event(ev),
            ChannelEvent::Closed => signal.stop(),
        });
    if let Err(e) = res {
        eprintln!("[x11] failed to register input dispatch source: {e}");
        return;
    }

    if let Err(e) = event_loop.run(None, &mut (), |()| {}) {
        eprintln!("[x11] input dispatch event loop exited: {e}");
    }
}

fn handle_event(st: &mut State, ev: xcb::Event) {
    use xcb::Event;
    match ev {
        Event::X(x::Event::KeyPress(e)) => handle_key(st, e.detail(), true),
        Event::X(x::Event::KeyRelease(e)) => handle_key(st, e.detail(), false),
        Event::X(x::Event::ButtonPress(e)) => {
            handle_button(st, e.detail(), e.event_x(), e.event_y(), true)
        }
        Event::X(x::Event::ButtonRelease(e)) => {
            handle_button(st, e.detail(), e.event_x(), e.event_y(), false)
        }
        Event::X(x::Event::MotionNotify(e)) => handle_motion(st, &e),
        Event::X(x::Event::EnterNotify(e)) => handle_enter(st, &e),
        Event::X(x::Event::LeaveNotify(e)) => handle_leave(st, &e),
        Event::Xkb(xkb_ev) => {
            use xcb::xkb;
            match xkb_ev {
                xkb::Event::StateNotify(e) => handle_xkb_state_notify(st, &e),
                xkb::Event::MapNotify(_) | xkb::Event::NewKeyboardNotify(_) => {
                    let conn = st.conn.clone();
                    update_keymap(&conn, st);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

pub fn start(screen_num: i32, parent: u32) -> Option<Handle> {
    let Some(conn) = crate::x11_state::xcb_conn() else {
        eprintln!("[x11] xcb input connection unavailable");
        return None;
    };
    let (cursor, cursor_requests) = CursorChannel::new();
    let (dispatch, dispatch_events) = channel();
    let (root, net_active_window) = crate::x11_state::host()
        .map(|h| (h.root, h.atoms.net_active_window))
        .unwrap_or((0, 0));
    let st = State {
        conn: conn.clone(),
        window: parent,
        root,
        net_active_window,
        xkb_ctx: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
        xkb_kmap: None,
        xkb_st: None,
        xkb_device_id: -1,
        xkb_base_event: 0,
        modifiers: 0,
        ptr_x: 0,
        ptr_y: 0,
        mouse_button_modifiers: 0,
        cursor: cursor.clone(),
        dispatch: dispatch.clone(),
    };

    let input_join = match std::thread::Builder::new()
        .name("jfn-x11-input-dispatch".into())
        .spawn(move || input_dispatch_thread_body(dispatch_events))
    {
        Ok(j) => j,
        Err(e) => {
            eprintln!("[x11] failed to spawn input dispatch thread: {e}");
            return None;
        }
    };

    let cursor_join = match std::thread::Builder::new()
        .name("jfn-x11-cursor".into())
        .spawn(move || cursor_thread_body(screen_num, parent, cursor_requests))
    {
        Ok(j) => j,
        Err(e) => {
            eprintln!("[x11] failed to spawn cursor thread: {e}");
            return None;
        }
    };

    let join = match std::thread::Builder::new()
        .name("jfn-x11-input".into())
        .spawn(move || input_thread_body(st))
    {
        Ok(j) => j,
        Err(e) => {
            eprintln!("[x11] failed to spawn input thread: {e}");
            return None;
        }
    };

    Some(Handle {
        join: Some(join),
        cursor_join: Some(cursor_join),
        input_join: Some(input_join),
        cursor: Some(cursor),
        dispatch: Some(dispatch),
    })
}

/// Capture pointer input directly on a WM-managed overlay.
///
/// Buttons go through a *passive grab* (`GrabButton`), not event selection,
/// because only one client may select `ButtonPress` on a window and the WM may
/// already hold it — a grab is independent of selection and cannot conflict.
/// Must use the same xcb connection the input thread polls.
pub fn grab_overlay_input(window: u32) {
    let Some(conn) = crate::x11_state::xcb_conn() else {
        return;
    };
    let w = x::Window::new(window);
    let attr_cookie = conn.send_request_checked(&x::ChangeWindowAttributes {
        window: w,
        value_list: &[x::Cw::EventMask(overlay_event_mask())],
    });
    let grab_cookie = conn.send_request_checked(&x::GrabButton {
        owner_events: true,
        grab_window: w,
        event_mask: overlay_grab_event_mask(),
        pointer_mode: x::GrabMode::Async,
        keyboard_mode: x::GrabMode::Async,
        confine_to: x::Window::none(),
        cursor: x::Cursor::none(),
        button: x::ButtonIndex::Any,
        modifiers: x::ModMask::ANY,
    });
    if let Err(e) = conn.check_request(attr_cookie) {
        tracing::error!(target: "x11::input", "grab_overlay_input: select events on 0x{window:x} failed: {e:?}");
    }
    if let Err(e) = conn.check_request(grab_cookie) {
        tracing::error!(target: "x11::input", "grab_overlay_input: GrabButton on 0x{window:x} failed: {e:?}");
    }
}

pub fn set_cursor(handle: &Handle, shape: CursorShape) {
    if let Some(cursor) = handle.cursor.as_ref() {
        cursor.set(shape);
    }
}
