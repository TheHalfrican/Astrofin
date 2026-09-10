//! macOS input — NSEvent translation + AstrofinInputView NSView subclass.
//!
//! The NSView is created by `macos_init` via the
//! `jfn_input_macos_create_view` extern "C" thunk;
//! `jfn_input_macos_set_cursor` is wired into the Platform vtable.
//!
//! Event dispatch goes through the `jfn_input_dispatch_*` extern "C"
//! entry points implemented in `src/input/src/lib.rs` (active-browser
//! lookup, hotkey classification, CEF forwarding).

use parking_lot::Mutex;
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{AnyThread, DefinedClass, define_class, extern_class, msg_send};
use objc2_app_kit::NSCursor;
use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize};

// =====================================================================
// NSView shim — `define_class!(super(NSView))` needs an `AnyThread`
// superclass; objc2-app-kit's NSView is main-thread-only.
// =====================================================================

extern_class!(
    #[unsafe(super(NSObject))]
    #[name = "NSView"]
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct NSView;
);

use jfn_input::buttons::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use jfn_input::scroll::ScrollAccum;
use jfn_platform_abi::cursor::CursorShape;

// The NSEvent translation tables live in `input_logic`; this module only
// reads the values off the event and dispatches.
use crate::input_logic::{
    CursorPlan, NsCursor, button_event_flag, cursor_plan, event_type_carries_characters,
    history_nav_for_button, modifier_key_pressed, mouse_buttons_after, ns_cursor_for,
    ns_keycode_to_vkey, ns_to_cef_modifiers, point_is_in_titlebar, should_forward_char,
};

// NSTrackingArea options (NSTrackingArea.h).
const NS_TRACKING_MOUSE_MOVED: u64 = 0x02;
const NS_TRACKING_MOUSE_ENTERED_AND_EXITED: u64 = 0x01;
const NS_TRACKING_ACTIVE_IN_KEY_WINDOW: u64 = 0x20;
const NS_TRACKING_IN_VISIBLE_RECT: u64 = 0x200;

// =====================================================================
// Cross-crate entry points
// =====================================================================

use jfn_input::{
    jfn_input_dispatch_char_sys, jfn_input_dispatch_history_nav, jfn_input_dispatch_key_full,
    jfn_input_dispatch_keyboard_focus, jfn_input_dispatch_mouse_button,
    jfn_input_dispatch_mouse_move, jfn_input_dispatch_scroll_precise,
};

use crate::dispatch::post_to_main;

// =====================================================================
// Cursor state
// =====================================================================

static G_CURSOR_HIDDEN: AtomicBool = AtomicBool::new(false);
static G_MOUSE_INSIDE: AtomicBool = AtomicBool::new(false);
/// Pending cursor type from CEF. Updated from any thread; applied on main.
static G_PENDING_CURSOR: AtomicI32 = AtomicI32::new(CursorShape::Pointer.as_raw());
/// Mouse-button bits to OR into modifier masks (so CEF sees the buttons
/// held during drags). Touched only from the main thread.
static G_MOUSE_BUTTON_MODIFIERS: AtomicU32 = AtomicU32::new(0);

/// The stock `NSCursor` for one of the cursors `ns_cursor_for` names.
/// AppKit exposes no non-deprecated directional resize cursors.
#[allow(deprecated)]
fn ns_cursor(cursor: NsCursor) -> Retained<NSCursor> {
    match cursor {
        NsCursor::Arrow => NSCursor::arrowCursor(),
        NsCursor::Crosshair => NSCursor::crosshairCursor(),
        NsCursor::PointingHand => NSCursor::pointingHandCursor(),
        NsCursor::IBeam => NSCursor::IBeamCursor(),
        NsCursor::IBeamVertical => NSCursor::IBeamCursorForVerticalLayout(),
        NsCursor::ResizeRight => NSCursor::resizeRightCursor(),
        NsCursor::ResizeLeft => NSCursor::resizeLeftCursor(),
        NsCursor::ResizeUp => NSCursor::resizeUpCursor(),
        NsCursor::ResizeDown => NSCursor::resizeDownCursor(),
        NsCursor::ResizeUpDown => NSCursor::resizeUpDownCursor(),
        NsCursor::ResizeLeftRight => NSCursor::resizeLeftRightCursor(),
        NsCursor::OpenHand => NSCursor::openHandCursor(),
        NsCursor::ClosedHand => NSCursor::closedHandCursor(),
        NsCursor::OperationNotAllowed => NSCursor::operationNotAllowedCursor(),
        NsCursor::DragCopy => NSCursor::dragCopyCursor(),
        NsCursor::DragLink => NSCursor::dragLinkCursor(),
        NsCursor::ContextualMenu => NSCursor::contextualMenuCursor(),
    }
}

fn apply_cursor_state() {
    let pending = CursorShape::from_cef(G_PENDING_CURSOR.load(Ordering::SeqCst))
        .unwrap_or(CursorShape::Pointer);
    let inside = G_MOUSE_INSIDE.load(Ordering::SeqCst);
    let CursorPlan { hide, unhide, set } =
        cursor_plan(pending, inside, G_CURSOR_HIDDEN.load(Ordering::SeqCst));
    if hide {
        NSCursor::hide();
        G_CURSOR_HIDDEN.store(true, Ordering::SeqCst);
    }
    if unhide {
        NSCursor::unhide();
        G_CURSOR_HIDDEN.store(false, Ordering::SeqCst);
    }
    if let Some(shape) = set {
        ns_cursor(ns_cursor_for(shape)).set();
    }
}

/// Platform::set_cursor — safe to call from any thread.
pub fn jfn_input_macos_set_cursor(t: c_int) {
    G_PENDING_CURSOR.store(t, Ordering::SeqCst);
    post_to_main(apply_cursor_state);
}

// =====================================================================
// Scroll accumulator — coalesces wheel/trackpad deltas onto a single
// flush per runloop cycle. All fields touched from main thread only.
// =====================================================================

static SCROLL: Mutex<ScrollAccum> = Mutex::new(ScrollAccum::new());

fn flush_scroll_accumulator() {
    let flush = SCROLL.lock().flush();
    if let Some(f) = flush {
        jfn_input_dispatch_scroll_precise(
            f.x,
            f.y,
            f.dx,
            f.dy,
            f.mods,
            if f.precise { 1 } else { 0 },
        );
    }
}

// =====================================================================
// AstrofinInputView — transparent NSView capturing input for CEF.
// =====================================================================

#[derive(Default)]
struct ViewIvars {
    tracking_area: Mutex<Option<Retained<AnyObject>>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "AstrofinInputView"]
    #[ivars = ViewIvars]
    struct InputView;

    impl InputView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> Bool { Bool::YES }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> Bool { Bool::YES }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> Bool { Bool::NO }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point_in_super: NSPoint) -> *mut AnyObject {
            // Makes the title-bar overlay strip click-through so AppKit's frame view receives the
            // clicks and natively handles window-drag and double-click-to-zoom.
            unsafe {
                let window: *mut AnyObject = msg_send![self, window];
                if !window.is_null() {
                    let content_layout: NSRect = msg_send![window, contentLayoutRect];
                    if point_is_in_titlebar(content_layout, point_in_super.y) {
                        return std::ptr::null_mut();
                    }
                }
                msg_send![super(self), hitTest: point_in_super]
            }
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            unsafe {
                let _: () = msg_send![super(self), updateTrackingAreas];
                let mut slot = self.ivars().tracking_area.lock();
                if let Some(old) = slot.take() {
                    let _: () = msg_send![self, removeTrackingArea: &*old];
                }
                let bounds: NSRect = msg_send![self, bounds];
                let cls = objc2::class!(NSTrackingArea);
                let area: *mut AnyObject = msg_send![cls, alloc];
                let opts: u64 = NS_TRACKING_MOUSE_MOVED
                    | NS_TRACKING_MOUSE_ENTERED_AND_EXITED
                    | NS_TRACKING_ACTIVE_IN_KEY_WINDOW
                    | NS_TRACKING_IN_VISIBLE_RECT;
                let area: *mut AnyObject = msg_send![
                    area,
                    initWithRect: bounds,
                    options: opts,
                    owner: self,
                    userInfo: std::ptr::null_mut::<AnyObject>(),
                ];
                if !area.is_null() {
                    let _: () = msg_send![self, addTrackingArea: area];
                    *slot = Retained::from_raw(area);
                }
            }
        }

        // ---- Mouse buttons ----
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &AnyObject) {
            dispatch_mouse_button(self, event, BTN_LEFT, true);
        }
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &AnyObject) {
            dispatch_mouse_button(self, event, BTN_LEFT, false);
        }
        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &AnyObject) {
            dispatch_mouse_button(self, event, BTN_RIGHT, true);
        }
        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &AnyObject) {
            dispatch_mouse_button(self, event, BTN_RIGHT, false);
        }
        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &AnyObject) {
            let n: isize = unsafe { msg_send![event, buttonNumber] };
            if let Some(direction) = history_nav_for_button(n) {
                jfn_input_dispatch_history_nav(direction);
                return;
            }
            dispatch_mouse_button(self, event, BTN_MIDDLE, true);
        }
        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &AnyObject) {
            let n: isize = unsafe { msg_send![event, buttonNumber] };
            if history_nav_for_button(n).is_some() {
                return;
            }
            dispatch_mouse_button(self, event, BTN_MIDDLE, false);
        }

        // ---- Mouse move ----
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &AnyObject) { dispatch_mouse_move(self, event, false); }
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &AnyObject) { dispatch_mouse_move(self, event, false); }
        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &AnyObject) { dispatch_mouse_move(self, event, false); }
        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &AnyObject) { dispatch_mouse_move(self, event, false); }
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &AnyObject) {
            G_MOUSE_INSIDE.store(true, Ordering::SeqCst);
            apply_cursor_state();
            dispatch_mouse_move(self, event, false);
        }
        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, event: &AnyObject) {
            G_MOUSE_INSIDE.store(false, Ordering::SeqCst);
            apply_cursor_state();
            dispatch_mouse_move(self, event, true);
        }

        // ---- Scroll ----
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &AnyObject) {
            let loc = mouse_loc_in_view(self, event);
            let precise: Bool = unsafe { msg_send![event, hasPreciseScrollingDeltas] };
            let precise = precise.as_bool();
            let (delta_x, delta_y) = unsafe {
                if precise {
                    let dx: f64 = msg_send![event, scrollingDeltaX];
                    let dy: f64 = msg_send![event, scrollingDeltaY];
                    (dx as f32, dy as f32)
                } else {
                    let dx: f64 = msg_send![event, deltaX];
                    let dy: f64 = msg_send![event, deltaY];
                    (dx as f32, dy as f32)
                }
            };
            let mods_raw: u64 = unsafe { msg_send![event, modifierFlags] };
            let sched = SCROLL.lock().accumulate(
                loc.x as i32,
                loc.y as i32,
                ns_to_cef_modifiers(mods_raw),
                precise,
                delta_x,
                delta_y,
            );
            if sched {
                post_to_main(flush_scroll_accumulator);
            }
        }

        // ---- Keyboard ----
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &AnyObject) {
            let (vkey, mods, kc, ch, ch_nomod) = key_event_fields(event);
            jfn_input_dispatch_key_full(1, vkey, kc as i32, mods, ch, ch_nomod, 0);
            // Forward typed characters for text input — paired CHAR event only
            // for printable chars + Return.
            if should_forward_char(ch) {
                jfn_input_dispatch_char_sys(ch as u32, mods, kc as u32, 0);
            }
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &AnyObject) {
            let (vkey, mods, kc, ch, ch_nomod) = key_event_fields(event);
            jfn_input_dispatch_key_full(0, vkey, kc as i32, mods, ch, ch_nomod, 0);
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &AnyObject) {
            let kc: u16 = unsafe { msg_send![event, keyCode] };
            let raw_flags: u64 = unsafe { msg_send![event, modifierFlags] };
            // The key code names the modifier, the post-event flags say
            // whether it is still down. character/unmodified_character left
            // at 0 (correct for the NSEventTypeFlagsChanged path).
            let pressed = modifier_key_pressed(kc, raw_flags);
            let vkey = ns_keycode_to_vkey(kc);
            let mods = ns_to_cef_modifiers(raw_flags);
            jfn_input_dispatch_key_full(if pressed { 1 } else { 0 }, vkey, kc as i32, mods, 0, 0, 0);
        }

        // ---- Focus ----
        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> Bool {
            jfn_input_dispatch_keyboard_focus(1);
            unsafe { msg_send![super(self), becomeFirstResponder] }
        }
        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> Bool {
            jfn_input_dispatch_keyboard_focus(0);
            unsafe { msg_send![super(self), resignFirstResponder] }
        }

        // ---- Edit menu actions ----
        // Without an Edit menu in the responder chain, AppKit never sends
        // these. Forward each to the active CEF browser's focused frame.
        #[unsafe(method(undo:))]
        fn undo_action(&self, _sender: *mut AnyObject) {
            if let Some(b) = jfn_platform_abi::browser_bridge() {
                b.undo();
            }
        }
        #[unsafe(method(redo:))]
        fn redo_action(&self, _sender: *mut AnyObject) {
            if let Some(b) = jfn_platform_abi::browser_bridge() {
                b.redo();
            }
        }
        #[unsafe(method(cut:))]
        fn cut_action(&self, _sender: *mut AnyObject) {
            if let Some(b) = jfn_platform_abi::browser_bridge() {
                b.cut();
            }
        }
        #[unsafe(method(copy:))]
        fn copy_action(&self, _sender: *mut AnyObject) {
            if let Some(b) = jfn_platform_abi::browser_bridge() {
                b.copy();
            }
        }
        #[unsafe(method(paste:))]
        fn paste_action(&self, _sender: *mut AnyObject) {
            if let Some(b) = jfn_platform_abi::browser_bridge() {
                b.paste();
            }
        }
        #[unsafe(method(selectAll:))]
        fn select_all_action(&self, _sender: *mut AnyObject) {
            if let Some(b) = jfn_platform_abi::browser_bridge() {
                b.select_all();
            }
        }
    }
);

fn mouse_loc_in_view(view: &InputView, event: &AnyObject) -> NSPoint {
    unsafe {
        let p: NSPoint = msg_send![event, locationInWindow];
        msg_send![view, convertPoint: p, fromView: std::ptr::null_mut::<AnyObject>()]
    }
}

fn dispatch_mouse_button(view: &InputView, event: &AnyObject, button_code: u32, pressed: bool) {
    let flag = button_event_flag(button_code);
    let prev = G_MOUSE_BUTTON_MODIFIERS.load(Ordering::SeqCst);
    let next = mouse_buttons_after(prev, flag, pressed);
    G_MOUSE_BUTTON_MODIFIERS.store(next, Ordering::SeqCst);

    let loc = mouse_loc_in_view(view, event);
    let raw_flags: u64 = unsafe { msg_send![event, modifierFlags] };
    let _click: isize = unsafe { msg_send![event, clickCount] };
    let mods = ns_to_cef_modifiers(raw_flags) | next;
    jfn_input_dispatch_mouse_button(
        button_code,
        if pressed { 1 } else { 0 },
        loc.x as i32,
        loc.y as i32,
        mods,
    );
}

fn dispatch_mouse_move(view: &InputView, event: &AnyObject, leave: bool) {
    let loc = mouse_loc_in_view(view, event);
    let raw_flags: u64 = unsafe { msg_send![event, modifierFlags] };
    let mods = ns_to_cef_modifiers(raw_flags) | G_MOUSE_BUTTON_MODIFIERS.load(Ordering::SeqCst);
    jfn_input_dispatch_mouse_move(loc.x as i32, loc.y as i32, mods, if leave { 1 } else { 0 });
}

/// Returns (windows_key_code, modifiers, native_keycode, character, unmodified_character).
fn key_event_fields(event: &AnyObject) -> (i32, u32, u16, u16, u16) {
    let kc: u16 = unsafe { msg_send![event, keyCode] };
    let raw_flags: u64 = unsafe { msg_send![event, modifierFlags] };
    let etype: u64 = unsafe { msg_send![event, type] };
    let (mut ch, mut ch_nomod) = (0u16, 0u16);
    if event_type_carries_characters(etype) {
        unsafe {
            let chars: *mut AnyObject = msg_send![event, characters];
            if !chars.is_null() {
                let len: usize = msg_send![chars, length];
                if len > 0 {
                    let c: u16 = msg_send![chars, characterAtIndex: 0usize];
                    ch = c;
                }
            }
            let chars_nm: *mut AnyObject = msg_send![event, charactersIgnoringModifiers];
            if !chars_nm.is_null() {
                let len: usize = msg_send![chars_nm, length];
                if len > 0 {
                    let c: u16 = msg_send![chars_nm, characterAtIndex: 0usize];
                    ch_nomod = c;
                }
            }
        }
    }
    (
        ns_keycode_to_vkey(kc),
        ns_to_cef_modifiers(raw_flags),
        kc,
        ch,
        ch_nomod,
    )
}

// =====================================================================
// Public entry — create the input NSView. Called from C++ macos_init
// after locating mpv's NSWindow.
// =====================================================================

/// Returns a +1-retained NSView pointer (transfers ownership to the
/// caller). The caller adds it to the window's content view subtree.
pub fn jfn_input_macos_create_view() -> *mut c_void {
    let zero_rect = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: NSSize {
            width: 0.0,
            height: 0.0,
        },
    };
    let view = InputView::alloc().set_ivars(ViewIvars::default());
    let view: Retained<InputView> = unsafe { msg_send![super(view), initWithFrame: zero_rect] };
    // Retain across the FFI boundary; caller owns the +1.
    Retained::into_raw(view) as *mut c_void
}
