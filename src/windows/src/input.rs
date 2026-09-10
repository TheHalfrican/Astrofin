//! Windows input — Win32 child window owning all keyboard/mouse for CEF.
//!
//! Runs on a dedicated thread (spawned by `platform.rs::win_init`);
//! registers an `AstrofinCefInput` window class, creates a child of mpv's
//! HWND covering the client area, and translates `WM_*` messages into
//! the platform-agnostic `jfn_input_dispatch_*` entry points exposed by
//! `src/input/src/lib.rs`.

#![allow(non_snake_case)]

use parking_lot::Mutex;
use std::ffi::c_int;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::{
    GetAwarenessFromDpiAwarenessContext, GetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_MENU};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetWindowThreadProcessId, HCURSOR, HICON, HMENU, HTCLIENT, LoadCursorW, MSG, PostMessageW,
    PostThreadMessageW, RegisterClassExW, SET_WINDOW_POS_FLAGS, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOZORDER, SetCursor, SetWindowPos, TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APPCOMMAND, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SETFOCUS, WM_SYSCHAR, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSEXW, WS_CHILD, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

const WM_MOUSELEAVE: u32 = 0x02A3;

use jfn_platform_abi::cursor::CursorShape;
use jfn_platform_abi::{LogicalPoint, PhysicalPoint};

use jfn_input::{
    jfn_input_dispatch_char_sys, jfn_input_dispatch_history_nav, jfn_input_dispatch_key_full,
    jfn_input_dispatch_keyboard_focus, jfn_input_dispatch_mouse_button,
    jfn_input_dispatch_mouse_move, jfn_input_dispatch_scroll,
};
use jfn_playback::shutdown::jfn_shutdown_initiate;

use crate::input_logic::{self, KeyAction, Keyboard};
use crate::menu::{WM_JFN_MENU_END, WM_JFN_MENU_TRACK};

struct State {
    input_hwnd_raw: usize,
    thread_id: u32,
    cursor_type: i32,
}

static STATE: Mutex<State> = Mutex::new(State {
    input_hwnd_raw: 0,
    thread_id: 0,
    cursor_type: CursorShape::Pointer.as_raw(),
});

/// The input child window, or `None` before the input thread creates it and
/// after it tears it down.
pub(crate) fn input_hwnd() -> Option<HWND> {
    let raw = STATE.lock().input_hwnd_raw;
    (raw != 0).then_some(HWND(raw as *mut _))
}

#[inline]
fn is_key_down(vk: u16) -> bool {
    let s = unsafe { GetKeyState(vk as i32) };
    (s as u16 & 0x8000) != 0
}

/// True when the lock light for `vk` is lit.
#[inline]
fn is_key_toggled(vk: u16) -> bool {
    (unsafe { GetKeyState(vk as i32) } & 1) != 0
}

/// Run `f` against the live keyboard, which is what the pure modifier tables
/// in [`crate::input_logic`] read their key state through.
fn with_keyboard<R>(f: impl FnOnce(&Keyboard<'_>) -> R) -> R {
    let down = |vk: u16| is_key_down(vk);
    let toggled = |vk: u16| is_key_toggled(vk);
    f(&Keyboard {
        down: &down,
        toggled: &toggled,
    })
}

fn mouse_modifiers(wp: WPARAM) -> u32 {
    with_keyboard(|kb| input_logic::mouse_modifier_flags(wp.0, kb))
}

fn keyboard_modifiers(wp: WPARAM, lp: LPARAM) -> u32 {
    with_keyboard(|kb| input_logic::keyboard_modifier_flags(wp.0 as u16, lp.0, kb))
}

/// The pointer position in the space CEF's view was sized in, mapped through
/// the extent `crate::window` last sampled. The identity before the first
/// sample exists.
fn view_point(x: i32, y: i32) -> LogicalPoint {
    let physical = PhysicalPoint { x, y };
    crate::window::client_extent().map_or(LogicalPoint { x, y }, |e| e.to_logical_point(physical))
}

/// One debug line per press.
fn log_press(msg: u32, physical: PhysicalPoint, logical: LogicalPoint) {
    let Some(extent) = crate::window::client_extent() else {
        return;
    };
    let logical_size = extent.logical();
    let physical_size = extent.physical();
    let scale = extent.scale();
    tracing::debug!(
        target: "platform",
        "press msg=0x{msg:04x} physical=({},{}) logical=({},{}) \
         extent=logical {}x{} physical {}x{} scale {}",
        physical.x, physical.y, logical.x, logical.y,
        logical_size.w, logical_size.h, physical_size.w, physical_size.h, scale.0,
    );
}

unsafe extern "system" fn input_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SETCURSOR if u32::from(input_logic::loword(lp.0 as u32)) == HTCLIENT => {
            let shape =
                CursorShape::from_cef(STATE.lock().cursor_type).unwrap_or(CursorShape::Pointer);
            if shape == CursorShape::None {
                unsafe { SetCursor(None) };
            } else {
                let cur = unsafe { LoadCursorW(None, input_logic::cursor_to_win(shape)).ok() };
                unsafe { SetCursor(cur) };
            }
            return LRESULT(1);
        }

        WM_MOUSEMOVE => {
            let p = view_point(input_logic::x_lparam(lp.0), input_logic::y_lparam(lp.0));
            jfn_input_dispatch_mouse_move(p.x, p.y, mouse_modifiers(wp), 0);
            return LRESULT(0);
        }

        WM_MOUSELEAVE => {
            jfn_input_dispatch_mouse_move(-1, -1, mouse_modifiers(wp), 1);
            return LRESULT(0);
        }

        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP
        | WM_MBUTTONUP => {
            let down = input_logic::is_button_down(msg);
            if down {
                let _ = unsafe { SetFocus(Some(hwnd)) };
            }
            let physical = PhysicalPoint {
                x: input_logic::x_lparam(lp.0),
                y: input_logic::y_lparam(lp.0),
            };
            let p = view_point(physical.x, physical.y);
            if down {
                log_press(msg, physical, p);
            }
            jfn_input_dispatch_mouse_button(
                input_logic::msg_to_button_code(msg),
                if down { 1 } else { 0 },
                p.x,
                p.y,
                mouse_modifiers(wp),
            );
            return LRESULT(0);
        }

        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let btn = input_logic::xbutton_wparam(wp.0);
            if let Some(fwd) = input_logic::xbutton_nav(msg, btn) {
                jfn_input_dispatch_history_nav(fwd);
            }
            return LRESULT(1);
        }

        WM_APPCOMMAND => {
            let cmd = u32::from(input_logic::appcommand_lparam(lp.0));
            if let Some(fwd) = input_logic::appcommand_nav(cmd) {
                jfn_input_dispatch_history_nav(fwd);
                return LRESULT(1);
            }
        }

        WM_MOUSEWHEEL => {
            let mut pt = POINT {
                x: input_logic::x_lparam(lp.0),
                y: input_logic::y_lparam(lp.0),
            };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let delta = i32::from(input_logic::hiword_i16(wp.0 as u32));
            let p = view_point(pt.x, pt.y);
            jfn_input_dispatch_scroll(p.x, p.y, 0, delta, mouse_modifiers(wp));
            return LRESULT(0);
        }

        WM_MOUSEHWHEEL => {
            let mut pt = POINT {
                x: input_logic::x_lparam(lp.0),
                y: input_logic::y_lparam(lp.0),
            };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let delta = i32::from(input_logic::hiword_i16(wp.0 as u32));
            let p = view_point(pt.x, pt.y);
            jfn_input_dispatch_scroll(p.x, p.y, delta, 0, mouse_modifiers(wp));
            return LRESULT(0);
        }

        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            let vk = wp.0 as u16;
            match input_logic::key_action(msg, vk, is_key_down(VK_MENU.0)) {
                KeyAction::Shutdown => jfn_shutdown_initiate(),
                KeyAction::HistoryNav(Some(fwd)) => jfn_input_dispatch_history_nav(fwd),
                KeyAction::HistoryNav(None) => {}
                KeyAction::Dispatch { pressed, is_sys } => jfn_input_dispatch_key_full(
                    if pressed { 1 } else { 0 },
                    vk as i32,
                    lp.0 as i32,
                    keyboard_modifiers(wp, lp),
                    0,
                    0,
                    if is_sys { 1 } else { 0 },
                ),
            }
            return LRESULT(0);
        }

        WM_CHAR | WM_SYSCHAR => {
            jfn_input_dispatch_char_sys(
                wp.0 as u32,
                keyboard_modifiers(wp, lp),
                lp.0 as u32,
                if msg == WM_SYSCHAR { 1 } else { 0 },
            );
            return LRESULT(0);
        }

        WM_JFN_MENU_TRACK | WM_JFN_MENU_END => {
            crate::menu::on_input_message(hwnd, msg);
            return LRESULT(0);
        }

        WM_SETFOCUS => {
            jfn_input_dispatch_keyboard_focus(1);
            return LRESULT(0);
        }
        WM_KILLFOCUS => {
            jfn_input_dispatch_keyboard_focus(0);
            return LRESULT(0);
        }

        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

const CLASS_NAME: PCWSTR = w!("AstrofinCefInput");

pub(crate) fn jfn_input_windows_run_input_thread(mpv_hwnd: *mut std::ffi::c_void) {
    let mpv = HWND(mpv_hwnd);
    let tid = unsafe { GetCurrentThreadId() };

    {
        let mut s = STATE.lock();
        s.thread_id = tid;
    }

    let hinst = unsafe { GetModuleHandleW(None).unwrap_or_default() };

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: Default::default(),
        lpfnWndProc: Some(input_wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst.into(),
        hIcon: HICON::default(),
        hCursor: HCURSOR::default(),
        hbrBackground: Default::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: CLASS_NAME,
        hIconSm: HICON::default(),
    };
    unsafe { RegisterClassExW(&wc) };

    let mut rc = RECT::default();
    let _ = unsafe { GetClientRect(mpv, &mut rc) };

    let input_hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!(""),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
            0,
            0,
            rc.right - rc.left,
            rc.bottom - rc.top,
            Some(mpv),
            Some(HMENU(std::ptr::null_mut())),
            Some(hinst.into()),
            None,
        )
    };

    let input_hwnd = match input_hwnd {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("CreateWindowExW(AstrofinCefInput) failed: {e:?}");
            STATE.lock().thread_id = 0;
            return;
        }
    };
    STATE.lock().input_hwnd_raw = input_hwnd.0 as usize;

    {
        let mut ir = RECT::default();
        let _ = unsafe { GetClientRect(input_hwnd, &mut ir) };
        let thread_awareness =
            unsafe { GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext()) }.0;
        tracing::debug!(
            target: "platform",
            "input window: size={}x{} parent_client={}x{} thread_awareness={}",
            ir.right - ir.left, ir.bottom - ir.top,
            rc.right - rc.left, rc.bottom - rc.top,
            thread_awareness,
        );
    }

    let mpv_tid = unsafe { GetWindowThreadProcessId(mpv, None) };
    let _ = unsafe { AttachThreadInput(tid, mpv_tid, true) };
    let _ = unsafe { SetFocus(Some(input_hwnd)) };

    let mut m = MSG::default();
    while unsafe { GetMessageW(&mut m, None, 0, 0).0 } > 0 {
        unsafe {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }

    if STATE.lock().input_hwnd_raw != 0 {
        let _ = unsafe { DestroyWindow(input_hwnd) };
        STATE.lock().input_hwnd_raw = 0;
    }
    let _ = unsafe { UnregisterClassW(CLASS_NAME, Some(hinst.into())) };
    STATE.lock().thread_id = 0;
}

pub(crate) fn jfn_input_windows_stop_input_thread() {
    let tid = STATE.lock().thread_id;
    if tid != 0 {
        let _ = unsafe { PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
}

pub(crate) fn jfn_input_windows_resize_to_parent(pw: c_int, ph: c_int) {
    let hwnd_raw = STATE.lock().input_hwnd_raw;
    if hwnd_raw == 0 {
        return;
    }
    let hwnd = HWND(hwnd_raw as *mut _);
    let flags: SET_WINDOW_POS_FLAGS =
        SET_WINDOW_POS_FLAGS(SWP_NOZORDER.0 | SWP_NOMOVE.0 | SWP_NOACTIVATE.0);
    let _ = unsafe { SetWindowPos(hwnd, None, 0, 0, pw, ph, flags) };
}

/// Platform::set_cursor — invoked from the CEF UI thread. Stores the
/// pending cursor type and posts a synthetic WM_SETCURSOR so the input
/// thread applies it via SetCursor (which is thread-affine).
pub(crate) fn jfn_input_windows_set_cursor(t: c_int) {
    let hwnd_raw = {
        let mut s = STATE.lock();
        s.cursor_type = t;
        s.input_hwnd_raw
    };
    if hwnd_raw == 0 {
        return;
    }
    let hwnd = HWND(hwnd_raw as *mut _);
    let _ = unsafe { PostMessageW(Some(hwnd), WM_SETCURSOR, WPARAM(hwnd_raw), LPARAM(1)) };
}
