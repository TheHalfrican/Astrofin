//! Input mapping tables and event masks for the X11 input thread.
//!
//! Split out of [`crate::input`], which keeps the xcb connection, the xkb
//! state and the three threads. Everything here is a table or a small
//! conversion: X button numbers to CEF/evdev meanings, CEF cursor shapes to
//! freedesktop cursor names, X keycodes to linux input codes, and the event
//! masks that decide what the app is told about at all.

use cursor_icon::CursorIcon;
use xcb::x;

use jfn_input::buttons;
use jfn_platform_abi::cursor::CursorShape;
use jfn_platform_abi::event_flags::{
    EVENTFLAG_LEFT_MOUSE_BUTTON, EVENTFLAG_MIDDLE_MOUSE_BUTTON, EVENTFLAG_RIGHT_MOUSE_BUTTON,
};

pub(crate) const XKB_KEY_XF86BACK: u32 = 0x1008ff26;
pub(crate) const XKB_KEY_XF86FORWARD: u32 = 0x1008ff27;

/// One notch of an X scroll button, in the 120ths-of-a-line unit CEF expects.
const WHEEL_NOTCH: i32 = 120;

/// What an X button number means. X overloads the button axis: 1-3 are real
/// buttons, 4-7 are wheel notches, 8-9 the side buttons browsers use for
/// history, and everything above is not ours.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ButtonAction {
    /// A wheel notch. Only a press counts; the matching release is silent.
    Scroll {
        dx: i32,
        dy: i32,
    },
    /// A side button. Only a press counts.
    HistoryNav {
        forward: bool,
    },
    /// A real button: its CEF modifier flag and its linux input-event code.
    Press {
        flag: u32,
        code: u32,
    },
    Ignore,
}

pub(crate) fn classify_button(button: u32) -> ButtonAction {
    match button {
        1 => ButtonAction::Press {
            flag: EVENTFLAG_LEFT_MOUSE_BUTTON,
            code: buttons::BTN_LEFT,
        },
        2 => ButtonAction::Press {
            flag: EVENTFLAG_MIDDLE_MOUSE_BUTTON,
            code: buttons::BTN_MIDDLE,
        },
        3 => ButtonAction::Press {
            flag: EVENTFLAG_RIGHT_MOUSE_BUTTON,
            code: buttons::BTN_RIGHT,
        },
        4 => ButtonAction::Scroll {
            dx: 0,
            dy: WHEEL_NOTCH,
        },
        5 => ButtonAction::Scroll {
            dx: 0,
            dy: -WHEEL_NOTCH,
        },
        6 => ButtonAction::Scroll {
            dx: WHEEL_NOTCH,
            dy: 0,
        },
        7 => ButtonAction::Scroll {
            dx: -WHEEL_NOTCH,
            dy: 0,
        },
        8 => ButtonAction::HistoryNav { forward: false },
        9 => ButtonAction::HistoryNav { forward: true },
        _ => ButtonAction::Ignore,
    }
}

/// The two keyboard keysyms that mean "go back"/"go forward", or `None` for a
/// key that should reach the page as a key instead.
pub(crate) fn history_nav_for_sym(sym: u32) -> Option<bool> {
    match sym {
        XKB_KEY_XF86BACK => Some(false),
        XKB_KEY_XF86FORWARD => Some(true),
        _ => None,
    }
}

/// X keycode to the linux input-event code the browser bridge speaks. X offsets
/// its keycodes by 8 from the kernel's.
pub(crate) fn native_key_code(keycode: u32) -> u32 {
    (keycode as i32 - 8) as u32
}

/// The modifier word CEF is sent: keyboard modifiers plus whichever mouse
/// buttons are held.
pub(crate) fn cef_modifiers(keyboard: u32, mouse_buttons: u32) -> u32 {
    keyboard | mouse_buttons
}

/// Fold a button press or release into the held-button modifier word.
pub(crate) fn apply_button_flag(held: u32, flag: u32, pressed: bool) -> u32 {
    if pressed { held | flag } else { held & !flag }
}

/// Decode a cursor shape stashed as a raw CEF cursor type. An unknown value
/// reads as the default arrow rather than leaving the cursor as it was.
pub(crate) fn cursor_shape_from_raw(raw: u32) -> CursorShape {
    CursorShape::from_cef(raw as i32).unwrap_or(CursorShape::Pointer)
}

/// CEF cursor type to the freedesktop cursor name X loads by theme. Shapes with
/// no freedesktop equivalent fall back to the default arrow.
pub(crate) fn cef_cursor_to_icon(shape: CursorShape) -> CursorIcon {
    use CursorShape::*;
    match shape {
        Cross => CursorIcon::Crosshair,
        Hand => CursorIcon::Pointer,
        IBeam => CursorIcon::Text,
        Wait => CursorIcon::Wait,
        Help => CursorIcon::Help,
        EastResize => CursorIcon::EResize,
        NorthResize => CursorIcon::NResize,
        NorthEastResize => CursorIcon::NeResize,
        NorthWestResize => CursorIcon::NwResize,
        SouthResize => CursorIcon::SResize,
        SouthEastResize => CursorIcon::SeResize,
        SouthWestResize => CursorIcon::SwResize,
        WestResize => CursorIcon::WResize,
        NorthSouthResize => CursorIcon::NsResize,
        EastWestResize => CursorIcon::EwResize,
        NorthEastSouthWestResize => CursorIcon::NeswResize,
        NorthWestSouthEastResize => CursorIcon::NwseResize,
        ColumnResize => CursorIcon::ColResize,
        RowResize => CursorIcon::RowResize,
        MiddlePanning | MiddlePanningVertical | MiddlePanningHorizontal => CursorIcon::AllScroll,
        Move => CursorIcon::Move,
        VerticalText => CursorIcon::VerticalText,
        Cell => CursorIcon::Cell,
        ContextMenu => CursorIcon::ContextMenu,
        Alias => CursorIcon::Alias,
        Progress => CursorIcon::Progress,
        NoDrop => CursorIcon::NoDrop,
        Copy => CursorIcon::Copy,
        NotAllowed => CursorIcon::NotAllowed,
        ZoomIn => CursorIcon::ZoomIn,
        ZoomOut => CursorIcon::ZoomOut,
        Grab => CursorIcon::Grab,
        Grabbing => CursorIcon::Grabbing,
        _ => CursorIcon::Default,
    }
}

/// What the input thread selects on the app top-level.
///
/// No `STRUCTURE_NOTIFY`: window structure (geometry and map state) is watched
/// on a separate connection by the geometry thread, and event masks are
/// per-client, so selecting it here would fight that.
pub(crate) fn toplevel_event_mask() -> x::EventMask {
    x::EventMask::KEY_PRESS
        | x::EventMask::KEY_RELEASE
        | x::EventMask::BUTTON_PRESS
        | x::EventMask::BUTTON_RELEASE
        | x::EventMask::POINTER_MOTION
        | x::EventMask::ENTER_WINDOW
        | x::EventMask::LEAVE_WINDOW
}

/// What is selected on a WM-managed overlay. Buttons are deliberately absent:
/// only one client may select `ButtonPress` on a window and the WM may already
/// hold it, so they come through the passive grab below instead.
pub(crate) fn overlay_event_mask() -> x::EventMask {
    x::EventMask::POINTER_MOTION | x::EventMask::ENTER_WINDOW | x::EventMask::LEAVE_WINDOW
}

/// What the overlay's passive button grab delivers.
pub(crate) fn overlay_grab_event_mask() -> x::EventMask {
    x::EventMask::BUTTON_PRESS | x::EventMask::BUTTON_RELEASE | x::EventMask::POINTER_MOTION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_real_buttons_carry_a_flag_and_an_evdev_code() {
        assert_eq!(
            classify_button(1),
            ButtonAction::Press {
                flag: EVENTFLAG_LEFT_MOUSE_BUTTON,
                code: buttons::BTN_LEFT,
            }
        );
        assert_eq!(
            classify_button(2),
            ButtonAction::Press {
                flag: EVENTFLAG_MIDDLE_MOUSE_BUTTON,
                code: buttons::BTN_MIDDLE,
            }
        );
        assert_eq!(
            classify_button(3),
            ButtonAction::Press {
                flag: EVENTFLAG_RIGHT_MOUSE_BUTTON,
                code: buttons::BTN_RIGHT,
            }
        );
    }

    #[test]
    fn wheel_buttons_scroll_one_notch_in_four_directions() {
        assert_eq!(classify_button(4), ButtonAction::Scroll { dx: 0, dy: 120 });
        assert_eq!(classify_button(5), ButtonAction::Scroll { dx: 0, dy: -120 });
        assert_eq!(classify_button(6), ButtonAction::Scroll { dx: 120, dy: 0 });
        assert_eq!(classify_button(7), ButtonAction::Scroll { dx: -120, dy: 0 });
    }

    #[test]
    fn the_side_buttons_are_back_then_forward() {
        assert_eq!(
            classify_button(8),
            ButtonAction::HistoryNav { forward: false }
        );
        assert_eq!(
            classify_button(9),
            ButtonAction::HistoryNav { forward: true }
        );
    }

    #[test]
    fn an_unknown_button_is_dropped() {
        assert_eq!(classify_button(0), ButtonAction::Ignore);
        assert_eq!(classify_button(10), ButtonAction::Ignore);
        assert_eq!(classify_button(u32::MAX), ButtonAction::Ignore);
    }

    #[test]
    fn only_the_two_xf86_keys_navigate_history() {
        assert_eq!(history_nav_for_sym(XKB_KEY_XF86BACK), Some(false));
        assert_eq!(history_nav_for_sym(XKB_KEY_XF86FORWARD), Some(true));
        assert_eq!(history_nav_for_sym(0x0061), None);
        assert_eq!(history_nav_for_sym(0), None);
    }

    #[test]
    fn an_x_keycode_is_eight_above_the_linux_input_code() {
        // KEY_A is 30 in the kernel, keycode 38 in X.
        assert_eq!(native_key_code(38), 30);
        assert_eq!(native_key_code(8), 0);
    }

    #[test]
    fn held_buttons_join_the_keyboard_modifiers() {
        assert_eq!(cef_modifiers(0, 0), 0);
        assert_eq!(
            cef_modifiers(0b10, EVENTFLAG_LEFT_MOUSE_BUTTON),
            0b10 | EVENTFLAG_LEFT_MOUSE_BUTTON
        );
    }

    #[test]
    fn a_release_clears_only_its_own_button() {
        let held = apply_button_flag(0, EVENTFLAG_LEFT_MOUSE_BUTTON, true);
        let held = apply_button_flag(held, EVENTFLAG_RIGHT_MOUSE_BUTTON, true);
        assert_eq!(
            held,
            EVENTFLAG_LEFT_MOUSE_BUTTON | EVENTFLAG_RIGHT_MOUSE_BUTTON
        );
        let held = apply_button_flag(held, EVENTFLAG_LEFT_MOUSE_BUTTON, false);
        assert_eq!(held, EVENTFLAG_RIGHT_MOUSE_BUTTON);
    }

    #[test]
    fn a_stashed_cursor_shape_round_trips() {
        let shape = CursorShape::Grabbing;
        assert_eq!(cursor_shape_from_raw(shape.as_raw() as u32), shape);
    }

    #[test]
    fn an_unknown_cursor_value_reads_as_the_arrow() {
        assert_eq!(cursor_shape_from_raw(u32::MAX), CursorShape::Pointer);
    }

    #[test]
    fn cursor_shapes_map_to_their_freedesktop_names() {
        assert_eq!(cef_cursor_to_icon(CursorShape::Hand), CursorIcon::Pointer);
        assert_eq!(cef_cursor_to_icon(CursorShape::IBeam), CursorIcon::Text);
        assert_eq!(
            cef_cursor_to_icon(CursorShape::NorthEastSouthWestResize),
            CursorIcon::NeswResize
        );
    }

    #[test]
    fn every_panning_cursor_becomes_all_scroll() {
        assert_eq!(
            cef_cursor_to_icon(CursorShape::MiddlePanning),
            CursorIcon::AllScroll
        );
        assert_eq!(
            cef_cursor_to_icon(CursorShape::MiddlePanningVertical),
            CursorIcon::AllScroll
        );
        assert_eq!(
            cef_cursor_to_icon(CursorShape::MiddlePanningHorizontal),
            CursorIcon::AllScroll
        );
    }

    #[test]
    fn a_shape_with_no_freedesktop_name_falls_back_to_the_arrow() {
        assert_eq!(
            cef_cursor_to_icon(CursorShape::Pointer),
            CursorIcon::Default
        );
        assert_eq!(
            cef_cursor_to_icon(CursorShape::EastPanning),
            CursorIcon::Default
        );
        // `None` is drawn as a 1x1 transparent pixmap, not loaded by name.
        assert_eq!(cef_cursor_to_icon(CursorShape::None), CursorIcon::Default);
    }

    #[test]
    fn the_toplevel_mask_never_selects_structure_notify() {
        let mask = toplevel_event_mask();
        assert!(!mask.contains(x::EventMask::STRUCTURE_NOTIFY));
        assert!(mask.contains(x::EventMask::KEY_PRESS));
        assert!(mask.contains(x::EventMask::BUTTON_PRESS));
        assert!(mask.contains(x::EventMask::LEAVE_WINDOW));
    }

    #[test]
    fn an_overlay_selects_pointer_events_but_grabs_its_buttons() {
        let selected = overlay_event_mask();
        assert!(!selected.contains(x::EventMask::BUTTON_PRESS));
        assert!(selected.contains(x::EventMask::POINTER_MOTION));

        let grabbed = overlay_grab_event_mask();
        assert!(grabbed.contains(x::EventMask::BUTTON_PRESS));
        assert!(grabbed.contains(x::EventMask::BUTTON_RELEASE));
        assert!(grabbed.contains(x::EventMask::POINTER_MOTION));
    }
}
