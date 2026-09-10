//! Pure NSEvent translation for `crate::input`.
//!
//! Everything here is input -> output over the plain-data halves of an
//! `NSEvent` — the modifier bitmask, the virtual key code, the button number,
//! the typed character — plus the cursor tables. No `NSEvent`, no `NSView`, no
//! message send: `crate::input` reads those values off the event and hands
//! them in, so the whole translation can be exercised without AppKit.

use std::ffi::c_int;

use jfn_input::buttons::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use jfn_platform_abi::cursor::CursorShape;
use jfn_platform_abi::event_flags::{
    EVENTFLAG_ALT_DOWN, EVENTFLAG_COMMAND_DOWN, EVENTFLAG_CONTROL_DOWN,
    EVENTFLAG_LEFT_MOUSE_BUTTON, EVENTFLAG_MIDDLE_MOUSE_BUTTON, EVENTFLAG_RIGHT_MOUSE_BUTTON,
    EVENTFLAG_SHIFT_DOWN,
};
use objc2_foundation::NSRect;

// NSEventModifierFlags (NSEvent.h).
const NSEVENT_MOD_SHIFT: u64 = 1 << 17;
const NSEVENT_MOD_CONTROL: u64 = 1 << 18;
const NSEVENT_MOD_OPTION: u64 = 1 << 19;
const NSEVENT_MOD_COMMAND: u64 = 1 << 20;
const NSEVENT_MOD_CAPSLOCK: u64 = 1 << 16;

// NSEventType values used.
const NSEVENT_TYPE_KEYDOWN: u64 = 10;
const NSEVENT_TYPE_KEYUP: u64 = 11;

// NSEvent buttonNumber for "back"/"forward" side buttons.
const NS_MOUSE_BUTTON_BACK: isize = 3;
const NS_MOUSE_BUTTON_FORWARD: isize = 4;

/// Back, in the encoding `jfn_input_dispatch_history_nav` takes.
const NAV_BACK: c_int = 0;
/// Forward — see [`NAV_BACK`].
const NAV_FORWARD: c_int = 1;

/// CEF modifier mask for an `NSEvent.modifierFlags` bitmask.
///
/// Caps lock is deliberately absent: the C++ original never forwarded it and
/// CEF derives the shifted character from `characters` anyway.
pub(crate) fn ns_to_cef_modifiers(flags: u64) -> u32 {
    let mut m = 0u32;
    if flags & NSEVENT_MOD_SHIFT != 0 {
        m |= EVENTFLAG_SHIFT_DOWN;
    }
    if flags & NSEVENT_MOD_CONTROL != 0 {
        m |= EVENTFLAG_CONTROL_DOWN;
    }
    if flags & NSEVENT_MOD_OPTION != 0 {
        m |= EVENTFLAG_ALT_DOWN;
    }
    if flags & NSEVENT_MOD_COMMAND != 0 {
        m |= EVENTFLAG_COMMAND_DOWN;
    }
    m
}

/// Windows VK code for CefKeyEvent.windows_key_code. `0` for a key with no
/// Windows equivalent, which is what CEF treats as "no virtual key".
pub(crate) fn ns_keycode_to_vkey(kc: u16) -> i32 {
    match kc {
        // Letters (VK_A = 0x41 .. VK_Z = 0x5A)
        0x00 => b'A' as i32,
        0x0B => b'B' as i32,
        0x08 => b'C' as i32,
        0x02 => b'D' as i32,
        0x0E => b'E' as i32,
        0x03 => b'F' as i32,
        0x05 => b'G' as i32,
        0x04 => b'H' as i32,
        0x22 => b'I' as i32,
        0x26 => b'J' as i32,
        0x28 => b'K' as i32,
        0x25 => b'L' as i32,
        0x2E => b'M' as i32,
        0x2D => b'N' as i32,
        0x1F => b'O' as i32,
        0x23 => b'P' as i32,
        0x0C => b'Q' as i32,
        0x0F => b'R' as i32,
        0x01 => b'S' as i32,
        0x11 => b'T' as i32,
        0x20 => b'U' as i32,
        0x09 => b'V' as i32,
        0x0D => b'W' as i32,
        0x07 => b'X' as i32,
        0x10 => b'Y' as i32,
        0x06 => b'Z' as i32,
        // Digits (VK_0 = 0x30 .. VK_9 = 0x39)
        0x1D => b'0' as i32,
        0x12 => b'1' as i32,
        0x13 => b'2' as i32,
        0x14 => b'3' as i32,
        0x15 => b'4' as i32,
        0x17 => b'5' as i32,
        0x16 => b'6' as i32,
        0x1A => b'7' as i32,
        0x1C => b'8' as i32,
        0x19 => b'9' as i32,
        // Function keys (VK_F1 = 0x70 .. VK_F12 = 0x7B)
        0x7A => 0x70,
        0x78 => 0x71,
        0x63 => 0x72,
        0x76 => 0x73,
        0x60 => 0x74,
        0x61 => 0x75,
        0x62 => 0x76,
        0x64 => 0x77,
        0x65 => 0x78,
        0x6D => 0x79,
        0x67 => 0x7A,
        0x6F => 0x7B,
        // Navigation
        0x7B => 0x25,
        0x7E => 0x26,
        0x7C => 0x27,
        0x7D => 0x28,
        0x73 => 0x24,
        0x77 => 0x23,
        0x74 => 0x21,
        0x79 => 0x22,
        // Editing
        0x30 => 0x09,
        0x24 => 0x0D,
        0x35 => 0x1B,
        0x33 => 0x08,
        0x75 => 0x2E,
        0x31 => 0x20,
        0x72 => 0x2D,
        // Modifiers
        0x38 | 0x3C => 0x10,
        0x3B | 0x3E => 0x11,
        0x3A | 0x3D => 0x12,
        0x36 | 0x37 => 0x5B,
        0x39 => 0x14,
        // OEM punctuation
        0x29 => 0xBA,
        0x18 => 0xBB,
        0x2B => 0xBC,
        0x1B => 0xBD,
        0x2F => 0xBE,
        0x2C => 0xBF,
        0x32 => 0xC0,
        0x21 => 0xDB,
        0x2A => 0xDC,
        0x1E => 0xDD,
        0x27 => 0xDE,
        _ => 0,
    }
}

/// Whether an `NSEventTypeFlagsChanged` event is a press rather than a
/// release: its key code names a modifier whose bit is still set in the
/// post-event flags. A key code that is not a modifier reads as a release,
/// which is what the C++ original reported.
pub(crate) fn modifier_key_pressed(kc: u16, raw_flags: u64) -> bool {
    let flag: u64 = match kc {
        56 | 60 => NSEVENT_MOD_SHIFT,
        59 | 62 => NSEVENT_MOD_CONTROL,
        58 | 61 => NSEVENT_MOD_OPTION,
        54 | 55 => NSEVENT_MOD_COMMAND,
        57 => NSEVENT_MOD_CAPSLOCK,
        _ => 0,
    };
    flag != 0 && (raw_flags & flag) != 0
}

/// Whether `-characters` is meaningful for this event type. Only key up/down
/// carry text; `NSEventTypeFlagsChanged` raises on `-characters`.
pub(crate) fn event_type_carries_characters(etype: u64) -> bool {
    etype == NSEVENT_TYPE_KEYDOWN || etype == NSEVENT_TYPE_KEYUP
}

/// Whether a typed UTF-16 unit earns a paired CEF CHAR event.
///
/// Return plus the printable range, minus DEL and minus AppKit's private-use
/// function-key block (`0xF700..=0xF7FF`, where the arrows, F-keys and Home /
/// End live) — those arrive as key events and would otherwise be inserted as
/// text. `0` means the event carried no character at all.
pub(crate) fn should_forward_char(ch: u16) -> bool {
    ch == 0x0d || (ch >= 0x20 && ch != 0x7f && !(0xF700..=0xF7FF).contains(&ch))
}

/// The history direction an `-otherMouse*` button number names, or `None` for
/// the buttons that are middle-clicks rather than side buttons.
pub(crate) fn history_nav_for_button(button_number: isize) -> Option<c_int> {
    match button_number {
        NS_MOUSE_BUTTON_BACK => Some(NAV_BACK),
        NS_MOUSE_BUTTON_FORWARD => Some(NAV_FORWARD),
        _ => None,
    }
}

/// The CEF held-button flag for a platform button code; `0` for a button CEF
/// has no modifier bit for.
pub(crate) fn button_event_flag(button_code: u32) -> u32 {
    match button_code {
        BTN_LEFT => EVENTFLAG_LEFT_MOUSE_BUTTON,
        BTN_RIGHT => EVENTFLAG_RIGHT_MOUSE_BUTTON,
        BTN_MIDDLE => EVENTFLAG_MIDDLE_MOUSE_BUTTON,
        _ => 0,
    }
}

/// The held-button mask after a press or release of `flag`. CEF wants the
/// buttons held during a drag ORed into every event's modifiers.
pub(crate) fn mouse_buttons_after(prev: u32, flag: u32, pressed: bool) -> u32 {
    if pressed { prev | flag } else { prev & !flag }
}

/// Whether a point in window coordinates lands in the title-bar strip above
/// the content layout rect, which the input view makes click-through so
/// AppKit's own frame view handles window drags and double-click-to-zoom.
///
/// The boundary itself belongs to the content: a point exactly on the top
/// edge is not in the title bar.
pub(crate) fn point_is_in_titlebar(content_layout: NSRect, point_y: f64) -> bool {
    point_y > content_layout.origin.y + content_layout.size.height
}

/// One of AppKit's stock cursors, named after the `NSCursor` factory that
/// returns it. `crate::input` does the actual message send; the table that
/// picks the cursor is here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NsCursor {
    Arrow,
    Crosshair,
    PointingHand,
    IBeam,
    IBeamVertical,
    ResizeRight,
    ResizeLeft,
    ResizeUp,
    ResizeDown,
    ResizeUpDown,
    ResizeLeftRight,
    OpenHand,
    ClosedHand,
    OperationNotAllowed,
    DragCopy,
    DragLink,
    ContextualMenu,
}

/// The stock cursor standing in for a CEF cursor shape.
///
/// AppKit exposes no non-deprecated directional resize cursors and no
/// panning, zoom or cell cursors at all, so several shapes collapse onto one
/// stock cursor and everything unmapped falls back to the arrow.
pub(crate) fn ns_cursor_for(shape: CursorShape) -> NsCursor {
    use CursorShape::*;
    match shape {
        Cross => NsCursor::Crosshair,
        Hand => NsCursor::PointingHand,
        IBeam => NsCursor::IBeam,
        VerticalText => NsCursor::IBeamVertical,
        EastResize => NsCursor::ResizeRight,
        WestResize => NsCursor::ResizeLeft,
        NorthResize => NsCursor::ResizeUp,
        SouthResize => NsCursor::ResizeDown,
        NorthSouthResize | RowResize => NsCursor::ResizeUpDown,
        EastWestResize | ColumnResize => NsCursor::ResizeLeftRight,
        Move | Grab => NsCursor::OpenHand,
        Grabbing => NsCursor::ClosedHand,
        NoDrop | NotAllowed => NsCursor::OperationNotAllowed,
        Copy => NsCursor::DragCopy,
        Alias => NsCursor::DragLink,
        ContextMenu => NsCursor::ContextualMenu,
        _ => NsCursor::Arrow,
    }
}

/// What the cursor state machine has to do this time round: at most one of
/// `hide` / `unhide`, then the shape to install if any.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct CursorPlan {
    /// Hide the (currently visible) cursor.
    pub hide: bool,
    /// Show the (currently hidden) cursor.
    pub unhide: bool,
    /// Shape to set once the cursor is visible.
    pub set: Option<CursorShape>,
}

/// The plan for a (requested shape, pointer inside the view, cursor currently
/// hidden) triple.
///
/// CEF asks for [`CursorShape::None`] to hide the cursor — during playback,
/// mostly — but hiding is only correct while the pointer is over our view;
/// anywhere else AppKit's cursor belongs to whatever is under it. Both
/// `NSCursor::hide` and `unhide` stack, so each is only issued when it
/// actually changes the state.
pub(crate) fn cursor_plan(pending: CursorShape, inside: bool, hidden: bool) -> CursorPlan {
    let wants_hidden = pending == CursorShape::None && inside;
    CursorPlan {
        hide: wants_hidden && !hidden,
        unhide: !wants_hidden && hidden,
        set: (!wants_hidden && inside && pending != CursorShape::None).then_some(pending),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(y: f64, height: f64) -> NSRect {
        NSRect {
            origin: objc2_foundation::NSPoint { x: 0.0, y },
            size: objc2_foundation::NSSize {
                width: 100.0,
                height,
            },
        }
    }

    #[test]
    fn every_modifier_bit_maps_onto_its_cef_flag() {
        assert_eq!(ns_to_cef_modifiers(1 << 17), EVENTFLAG_SHIFT_DOWN);
        assert_eq!(ns_to_cef_modifiers(1 << 18), EVENTFLAG_CONTROL_DOWN);
        assert_eq!(ns_to_cef_modifiers(1 << 19), EVENTFLAG_ALT_DOWN);
        assert_eq!(ns_to_cef_modifiers(1 << 20), EVENTFLAG_COMMAND_DOWN);
    }

    #[test]
    fn held_modifiers_combine_and_unknown_bits_are_dropped() {
        let flags = (1 << 17) | (1 << 20) | (1 << 16) | (1 << 23);
        assert_eq!(
            ns_to_cef_modifiers(flags),
            EVENTFLAG_SHIFT_DOWN | EVENTFLAG_COMMAND_DOWN
        );
        // Caps lock alone carries no CEF flag.
        assert_eq!(ns_to_cef_modifiers(1 << 16), 0);
        assert_eq!(ns_to_cef_modifiers(0), 0);
    }

    #[test]
    fn letters_digits_and_function_keys_reach_their_windows_codes() {
        assert_eq!(ns_keycode_to_vkey(0x00), 0x41); // A
        assert_eq!(ns_keycode_to_vkey(0x06), 0x5A); // Z
        assert_eq!(ns_keycode_to_vkey(0x1D), 0x30); // 0
        assert_eq!(ns_keycode_to_vkey(0x19), 0x39); // 9
        assert_eq!(ns_keycode_to_vkey(0x7A), 0x70); // F1
        assert_eq!(ns_keycode_to_vkey(0x6F), 0x7B); // F12
    }

    #[test]
    fn navigation_and_editing_keys_reach_their_windows_codes() {
        assert_eq!(ns_keycode_to_vkey(0x7B), 0x25); // Left
        assert_eq!(ns_keycode_to_vkey(0x7D), 0x28); // Down
        assert_eq!(ns_keycode_to_vkey(0x24), 0x0D); // Return
        assert_eq!(ns_keycode_to_vkey(0x35), 0x1B); // Escape
        assert_eq!(ns_keycode_to_vkey(0x31), 0x20); // Space
    }

    #[test]
    fn both_sides_of_a_modifier_share_one_windows_code() {
        assert_eq!(ns_keycode_to_vkey(0x38), ns_keycode_to_vkey(0x3C)); // shift
        assert_eq!(ns_keycode_to_vkey(0x3B), ns_keycode_to_vkey(0x3E)); // control
        assert_eq!(ns_keycode_to_vkey(0x36), ns_keycode_to_vkey(0x37)); // command
    }

    #[test]
    fn a_key_without_a_windows_equivalent_maps_to_zero() {
        // 0x0A is ISO section, 0x3F is fn, 0x6E is the menu key: none of them
        // are in the table.
        assert_eq!(ns_keycode_to_vkey(0x0A), 0);
        assert_eq!(ns_keycode_to_vkey(0x3F), 0);
        assert_eq!(ns_keycode_to_vkey(0xFFFF), 0);
    }

    #[test]
    fn a_modifier_is_pressed_only_while_its_own_bit_is_set() {
        assert!(modifier_key_pressed(56, 1 << 17));
        assert!(modifier_key_pressed(60, 1 << 17));
        assert!(!modifier_key_pressed(56, 0));
        // Shift released while command is still held.
        assert!(!modifier_key_pressed(56, 1 << 20));
        assert!(modifier_key_pressed(57, 1 << 16));
    }

    #[test]
    fn a_non_modifier_key_code_never_reads_as_pressed() {
        assert!(!modifier_key_pressed(0x00, u64::MAX));
    }

    #[test]
    fn only_key_up_and_key_down_carry_characters() {
        assert!(event_type_carries_characters(10));
        assert!(event_type_carries_characters(11));
        // NSEventTypeFlagsChanged.
        assert!(!event_type_carries_characters(12));
        assert!(!event_type_carries_characters(0));
    }

    #[test]
    fn printable_characters_and_return_are_forwarded() {
        assert!(should_forward_char(0x0d));
        assert!(should_forward_char(0x20));
        assert!(should_forward_char(u16::from(b'a')));
        assert!(should_forward_char(0x00e9)); // é
    }

    #[test]
    fn control_codes_delete_and_function_keys_are_not_forwarded() {
        assert!(!should_forward_char(0)); // no character at all
        assert!(!should_forward_char(0x08)); // backspace
        assert!(!should_forward_char(0x1f));
        assert!(!should_forward_char(0x7f)); // delete
        assert!(!should_forward_char(0xF700)); // up arrow
        assert!(!should_forward_char(0xF7FF));
        // One past the private-use block is text again.
        assert!(should_forward_char(0xF800));
    }

    #[test]
    fn the_side_buttons_navigate_history_and_nothing_else_does() {
        assert_eq!(history_nav_for_button(3), Some(NAV_BACK));
        assert_eq!(history_nav_for_button(4), Some(NAV_FORWARD));
        assert_eq!(history_nav_for_button(2), None);
        assert_eq!(history_nav_for_button(0), None);
        assert_eq!(history_nav_for_button(-1), None);
    }

    #[test]
    fn each_mouse_button_carries_its_own_cef_flag() {
        assert_eq!(button_event_flag(BTN_LEFT), EVENTFLAG_LEFT_MOUSE_BUTTON);
        assert_eq!(button_event_flag(BTN_RIGHT), EVENTFLAG_RIGHT_MOUSE_BUTTON);
        assert_eq!(button_event_flag(BTN_MIDDLE), EVENTFLAG_MIDDLE_MOUSE_BUTTON);
        assert_eq!(button_event_flag(0), 0);
    }

    #[test]
    fn a_press_adds_its_bit_and_a_release_clears_only_that_bit() {
        let left = EVENTFLAG_LEFT_MOUSE_BUTTON;
        let right = EVENTFLAG_RIGHT_MOUSE_BUTTON;
        assert_eq!(mouse_buttons_after(0, left, true), left);
        assert_eq!(mouse_buttons_after(left, right, true), left | right);
        assert_eq!(mouse_buttons_after(left | right, left, false), right);
        // Releasing a button that was never down changes nothing.
        assert_eq!(mouse_buttons_after(right, left, false), right);
        // A button with no flag leaves the mask alone either way.
        assert_eq!(mouse_buttons_after(left, 0, true), left);
        assert_eq!(mouse_buttons_after(left, 0, false), left);
    }

    #[test]
    fn only_points_above_the_content_rect_are_titlebar() {
        let content = rect(0.0, 700.0);
        assert!(point_is_in_titlebar(content, 700.5));
        assert!(!point_is_in_titlebar(content, 699.5));
        // The top edge of the content belongs to the content.
        assert!(!point_is_in_titlebar(content, 700.0));
        // An offset content rect moves the boundary with it.
        assert!(point_is_in_titlebar(rect(10.0, 700.0), 710.5));
        assert!(!point_is_in_titlebar(rect(10.0, 700.0), 705.0));
    }

    #[test]
    fn resize_shapes_share_the_stock_cursors_appkit_offers() {
        assert_eq!(
            ns_cursor_for(CursorShape::EastResize),
            NsCursor::ResizeRight
        );
        assert_eq!(
            ns_cursor_for(CursorShape::NorthSouthResize),
            NsCursor::ResizeUpDown
        );
        assert_eq!(
            ns_cursor_for(CursorShape::RowResize),
            NsCursor::ResizeUpDown
        );
        assert_eq!(
            ns_cursor_for(CursorShape::ColumnResize),
            NsCursor::ResizeLeftRight
        );
        assert_eq!(ns_cursor_for(CursorShape::Grab), NsCursor::OpenHand);
        assert_eq!(ns_cursor_for(CursorShape::Grabbing), NsCursor::ClosedHand);
        assert_eq!(ns_cursor_for(CursorShape::Cross), NsCursor::Crosshair);
        assert_eq!(ns_cursor_for(CursorShape::Hand), NsCursor::PointingHand);
    }

    #[test]
    fn an_unmapped_shape_falls_back_to_the_arrow() {
        // AppKit has no zoom, cell or panning cursor.
        assert_eq!(ns_cursor_for(CursorShape::ZoomIn), NsCursor::Arrow);
        assert_eq!(ns_cursor_for(CursorShape::Cell), NsCursor::Arrow);
        assert_eq!(ns_cursor_for(CursorShape::MiddlePanning), NsCursor::Arrow);
        assert_eq!(ns_cursor_for(CursorShape::Pointer), NsCursor::Arrow);
        // `None` is handled by hiding, not by a cursor.
        assert_eq!(ns_cursor_for(CursorShape::None), NsCursor::Arrow);
    }

    #[test]
    fn a_hide_request_inside_the_view_hides_once() {
        let plan = cursor_plan(CursorShape::None, true, false);
        assert!(plan.hide);
        assert!(!plan.unhide);
        assert_eq!(plan.set, None);
        // Already hidden: nothing to do, and no second `NSCursor::hide`.
        let plan = cursor_plan(CursorShape::None, true, true);
        assert_eq!(
            plan,
            CursorPlan {
                hide: false,
                unhide: false,
                set: None
            }
        );
    }

    #[test]
    fn leaving_the_view_while_hidden_unhides_without_setting_a_shape() {
        let plan = cursor_plan(CursorShape::None, false, true);
        assert!(plan.unhide);
        assert!(!plan.hide);
        assert_eq!(plan.set, None);
    }

    #[test]
    fn a_shape_is_set_only_while_the_pointer_is_inside() {
        let inside = cursor_plan(CursorShape::Hand, true, false);
        assert_eq!(inside.set, Some(CursorShape::Hand));
        assert!(!inside.hide && !inside.unhide);
        let outside = cursor_plan(CursorShape::Hand, false, false);
        assert_eq!(outside.set, None);
    }

    #[test]
    fn a_shape_arriving_while_hidden_unhides_first() {
        let plan = cursor_plan(CursorShape::IBeam, true, true);
        assert!(plan.unhide);
        assert!(!plan.hide);
        assert_eq!(plan.set, Some(CursorShape::IBeam));
    }
}
