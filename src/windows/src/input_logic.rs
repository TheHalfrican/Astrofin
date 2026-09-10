//! Pure decoding for the Win32 input messages `crate::input` pumps.
//!
//! Everything here is input -> output over `WPARAM`/`LPARAM` payloads and
//! virtual-key codes: no `HWND`, no `GetKeyState`, no dispatch. The live key
//! state a couple of the tables need is passed in as lookup closures, so the
//! whole modifier table — including the keypad and left/right side flags —
//! can be exercised against a fake keyboard.

use std::ffi::c_int;

use jfn_input::buttons::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use jfn_platform_abi::cursor::CursorShape;
use jfn_platform_abi::event_flags::{
    EVENTFLAG_ALT_DOWN, EVENTFLAG_CAPS_LOCK_ON, EVENTFLAG_CONTROL_DOWN, EVENTFLAG_IS_KEY_PAD,
    EVENTFLAG_IS_LEFT, EVENTFLAG_IS_RIGHT, EVENTFLAG_LEFT_MOUSE_BUTTON,
    EVENTFLAG_MIDDLE_MOUSE_BUTTON, EVENTFLAG_NUM_LOCK_ON, EVENTFLAG_RIGHT_MOUSE_BUTTON,
    EVENTFLAG_SHIFT_DOWN,
};
use windows::Win32::System::SystemServices::{
    APPCOMMAND_BROWSER_BACKWARD, APPCOMMAND_BROWSER_FORWARD, MK_CONTROL, MK_LBUTTON, MK_MBUTTON,
    MK_RBUTTON, MK_SHIFT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_ADD, VK_BROWSER_BACK, VK_BROWSER_FORWARD, VK_CAPITAL, VK_CLEAR, VK_CONTROL, VK_DECIMAL,
    VK_DELETE, VK_DIVIDE, VK_DOWN, VK_END, VK_F4, VK_HOME, VK_INSERT, VK_LCONTROL, VK_LEFT,
    VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_MULTIPLY, VK_NEXT, VK_NUMLOCK, VK_NUMPAD0,
    VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4, VK_NUMPAD5, VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8,
    VK_NUMPAD9, VK_PRIOR, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
    VK_SUBTRACT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IDC_APPSTARTING, IDC_ARROW, IDC_CROSS, IDC_HAND, IDC_HELP, IDC_IBEAM, IDC_NO, IDC_SIZEALL,
    IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, IDC_WAIT, KF_EXTENDED, WM_KEYDOWN,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
    XBUTTON2,
};
use windows::core::PCWSTR;

/// Back, in the encoding `jfn_input_dispatch_history_nav` takes.
pub(crate) const NAV_BACK: c_int = 0;
/// Forward — see [`NAV_BACK`].
pub(crate) const NAV_FORWARD: c_int = 1;

/// Live keyboard state, as `GetKeyState` reports it.
///
/// `down` is the high bit (the key is held); `toggled` is the low bit (the
/// lock is lit). Closures rather than a snapshot map because the real caller
/// queries Win32 lazily, one virtual key at a time.
pub(crate) struct Keyboard<'a> {
    pub(crate) down: &'a dyn Fn(u16) -> bool,
    pub(crate) toggled: &'a dyn Fn(u16) -> bool,
}

/// The low 16 bits of a message parameter.
pub(crate) fn loword(v: u32) -> u16 {
    (v & 0xFFFF) as u16
}

/// The high 16 bits of a message parameter, signed.
pub(crate) fn hiword_i16(v: u32) -> i16 {
    ((v >> 16) & 0xFFFF) as i16
}

/// `GET_X_LPARAM` — the signed low word of a mouse message's `LPARAM`.
pub(crate) fn x_lparam(lp: isize) -> i32 {
    i32::from(lp as i16)
}

/// `GET_Y_LPARAM` — the signed high word of a mouse message's `LPARAM`.
pub(crate) fn y_lparam(lp: isize) -> i32 {
    i32::from((lp >> 16) as i16)
}

/// `GET_XBUTTON_WPARAM` — which extra mouse button a `WM_XBUTTON*` names.
pub(crate) fn xbutton_wparam(wp: usize) -> u16 {
    hiword_i16(wp as u32) as u16
}

/// `GET_APPCOMMAND_LPARAM` — the command id of a `WM_APPCOMMAND`, with the
/// device and key-state bits masked off.
pub(crate) fn appcommand_lparam(lp: isize) -> u16 {
    (hiword_i16(lp as u32) as u16) & 0x7FFF
}

/// True when a keyboard message's `LPARAM` carries `KF_EXTENDED` — the bit
/// that separates the navigation cluster from the numeric keypad.
pub(crate) fn is_extended_key(lp: isize) -> bool {
    ((lp >> 16) as u32 & KF_EXTENDED) != 0
}

/// CEF event flags for a mouse message: the modifier and button bits Windows
/// packs into `WPARAM`, plus Alt, which `WPARAM` never carries.
pub(crate) fn mouse_modifier_flags(wp: usize, keyboard: &Keyboard<'_>) -> u32 {
    let mut m = 0u32;
    let w = wp as u32;
    if w & MK_CONTROL.0 != 0 {
        m |= EVENTFLAG_CONTROL_DOWN;
    }
    if w & MK_SHIFT.0 != 0 {
        m |= EVENTFLAG_SHIFT_DOWN;
    }
    if (keyboard.down)(VK_MENU.0) {
        m |= EVENTFLAG_ALT_DOWN;
    }
    if w & MK_LBUTTON.0 != 0 {
        m |= EVENTFLAG_LEFT_MOUSE_BUTTON;
    }
    if w & MK_RBUTTON.0 != 0 {
        m |= EVENTFLAG_RIGHT_MOUSE_BUTTON;
    }
    if w & MK_MBUTTON.0 != 0 {
        m |= EVENTFLAG_MIDDLE_MOUSE_BUTTON;
    }
    m
}

/// CEF event flags for a keyboard message: the held modifiers, the two lock
/// lights, `EVENTFLAG_IS_KEY_PAD` for keys that came from the numeric keypad,
/// and the left/right side of a bare Shift/Ctrl/Alt/Win.
///
/// `vk` is the virtual key from `WPARAM`; `lp` is the raw `LPARAM`, of which
/// only the `KF_EXTENDED` bit is read.
pub(crate) fn keyboard_modifier_flags(vk: u16, lp: isize, keyboard: &Keyboard<'_>) -> u32 {
    let mut m = 0u32;
    if (keyboard.down)(VK_SHIFT.0) {
        m |= EVENTFLAG_SHIFT_DOWN;
    }
    if (keyboard.down)(VK_CONTROL.0) {
        m |= EVENTFLAG_CONTROL_DOWN;
    }
    if (keyboard.down)(VK_MENU.0) {
        m |= EVENTFLAG_ALT_DOWN;
    }
    if (keyboard.toggled)(VK_NUMLOCK.0) {
        m |= EVENTFLAG_NUM_LOCK_ON;
    }
    if (keyboard.toggled)(VK_CAPITAL.0) {
        m |= EVENTFLAG_CAPS_LOCK_ON;
    }

    let extended = is_extended_key(lp);
    match vk {
        v if v == VK_RETURN.0 && extended => {
            m |= EVENTFLAG_IS_KEY_PAD;
        }
        v if !extended
            && (v == VK_INSERT.0
                || v == VK_DELETE.0
                || v == VK_HOME.0
                || v == VK_END.0
                || v == VK_PRIOR.0
                || v == VK_NEXT.0
                || v == VK_UP.0
                || v == VK_DOWN.0
                || v == VK_LEFT.0
                || v == VK_RIGHT.0) =>
        {
            m |= EVENTFLAG_IS_KEY_PAD;
        }
        v if v == VK_NUMLOCK.0
            || v == VK_NUMPAD0.0
            || v == VK_NUMPAD1.0
            || v == VK_NUMPAD2.0
            || v == VK_NUMPAD3.0
            || v == VK_NUMPAD4.0
            || v == VK_NUMPAD5.0
            || v == VK_NUMPAD6.0
            || v == VK_NUMPAD7.0
            || v == VK_NUMPAD8.0
            || v == VK_NUMPAD9.0
            || v == VK_DIVIDE.0
            || v == VK_MULTIPLY.0
            || v == VK_SUBTRACT.0
            || v == VK_ADD.0
            || v == VK_DECIMAL.0
            || v == VK_CLEAR.0 =>
        {
            m |= EVENTFLAG_IS_KEY_PAD;
        }
        v if v == VK_SHIFT.0 => {
            if (keyboard.down)(VK_LSHIFT.0) {
                m |= EVENTFLAG_IS_LEFT;
            } else if (keyboard.down)(VK_RSHIFT.0) {
                m |= EVENTFLAG_IS_RIGHT;
            }
        }
        v if v == VK_CONTROL.0 => {
            if (keyboard.down)(VK_LCONTROL.0) {
                m |= EVENTFLAG_IS_LEFT;
            } else if (keyboard.down)(VK_RCONTROL.0) {
                m |= EVENTFLAG_IS_RIGHT;
            }
        }
        v if v == VK_MENU.0 => {
            if (keyboard.down)(VK_LMENU.0) {
                m |= EVENTFLAG_IS_LEFT;
            } else if (keyboard.down)(VK_RMENU.0) {
                m |= EVENTFLAG_IS_RIGHT;
            }
        }
        v if v == VK_LWIN.0 => m |= EVENTFLAG_IS_LEFT,
        v if v == VK_RWIN.0 => m |= EVENTFLAG_IS_RIGHT,
        _ => {}
    }
    m
}

/// The `IDC_*` system cursor that stands in for a CEF cursor shape.
pub(crate) fn cursor_to_win(shape: CursorShape) -> PCWSTR {
    use CursorShape::*;
    match shape {
        Cross => IDC_CROSS,
        Hand | Grab | Grabbing => IDC_HAND,
        IBeam => IDC_IBEAM,
        Wait => IDC_WAIT,
        Help => IDC_HELP,
        EastResize | WestResize | EastWestResize | ColumnResize => IDC_SIZEWE,
        NorthResize | SouthResize | NorthSouthResize | RowResize => IDC_SIZENS,
        NorthEastResize | SouthWestResize | NorthEastSouthWestResize => IDC_SIZENESW,
        NorthWestResize | SouthEastResize | NorthWestSouthEastResize => IDC_SIZENWSE,
        Move | MiddlePanning | MiddlePanningVertical | MiddlePanningHorizontal => IDC_SIZEALL,
        Progress => IDC_APPSTARTING,
        NoDrop | NotAllowed => IDC_NO,
        _ => IDC_ARROW,
    }
}

/// The platform-agnostic button code a `WM_?BUTTON*` message names. Anything
/// else falls back to the left button; the window procedure only ever reaches
/// this with the three tracked buttons.
pub(crate) fn msg_to_button_code(msg: u32) -> u32 {
    match msg {
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK => BTN_LEFT,
        WM_RBUTTONDOWN | WM_RBUTTONUP | WM_RBUTTONDBLCLK => BTN_RIGHT,
        WM_MBUTTONDOWN | WM_MBUTTONUP | WM_MBUTTONDBLCLK => BTN_MIDDLE,
        _ => BTN_LEFT,
    }
}

/// True for the press half of a mouse-button message pair.
pub(crate) fn is_button_down(msg: u32) -> bool {
    matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN)
}

/// What a `WM_KEYDOWN`/`WM_KEYUP`/`WM_SYSKEY*` message should do.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum KeyAction {
    /// Alt+F4 — start the app's own shutdown instead of letting
    /// `DefWindowProc` close mpv's window out from under it.
    Shutdown,
    /// A browser back/forward key. `Some(dir)` on the press, `None` on the
    /// release: the message is swallowed either way, but only the press
    /// navigates.
    HistoryNav(Option<c_int>),
    /// Hand the key to CEF.
    Dispatch { pressed: bool, is_sys: bool },
}

/// Route one keyboard message. `alt_down` is the live Alt state, which the
/// Alt+F4 check needs and `WPARAM` does not carry.
pub(crate) fn key_action(msg: u32, vk: u16, alt_down: bool) -> KeyAction {
    let pressed = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    if vk == VK_F4.0 && msg == WM_SYSKEYDOWN && alt_down {
        return KeyAction::Shutdown;
    }
    if vk == VK_BROWSER_BACK.0 || vk == VK_BROWSER_FORWARD.0 {
        if !pressed {
            return KeyAction::HistoryNav(None);
        }
        let dir = if vk == VK_BROWSER_FORWARD.0 {
            NAV_FORWARD
        } else {
            NAV_BACK
        };
        return KeyAction::HistoryNav(Some(dir));
    }
    KeyAction::Dispatch {
        pressed,
        is_sys: msg == WM_SYSKEYDOWN || msg == WM_SYSKEYUP,
    }
}

/// The history-navigation direction a `WM_XBUTTON*` pair carries, or `None`
/// on the release half, which is swallowed without navigating.
pub(crate) fn xbutton_nav(msg: u32, button: u16) -> Option<c_int> {
    if msg != WM_XBUTTONDOWN {
        return None;
    }
    Some(if button == XBUTTON2 {
        NAV_FORWARD
    } else {
        NAV_BACK
    })
}

/// The history-navigation direction a `WM_APPCOMMAND` carries, or `None` for
/// every command this window does not handle.
pub(crate) fn appcommand_nav(cmd: u32) -> Option<c_int> {
    if cmd == APPCOMMAND_BROWSER_BACKWARD.0 {
        return Some(NAV_BACK);
    }
    if cmd == APPCOMMAND_BROWSER_FORWARD.0 {
        return Some(NAV_FORWARD);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{WM_KEYUP, WM_XBUTTONUP};

    /// Modifier flags for `vk` on a keyboard where exactly `down` is held and
    /// exactly `toggled` is lit.
    fn flags(vk: u16, lp: isize, down: &[u16], toggled: &[u16]) -> u32 {
        let d = |k: u16| down.contains(&k);
        let t = |k: u16| toggled.contains(&k);
        keyboard_modifier_flags(
            vk,
            lp,
            &Keyboard {
                down: &d,
                toggled: &t,
            },
        )
    }

    fn mouse_flags(wp: usize, down: &[u16]) -> u32 {
        let d = |k: u16| down.contains(&k);
        let t = |_: u16| false;
        mouse_modifier_flags(
            wp,
            &Keyboard {
                down: &d,
                toggled: &t,
            },
        )
    }

    #[test]
    fn loword_keeps_the_low_sixteen_bits() {
        assert_eq!(loword(0xDEAD_BEEF), 0xBEEF);
        assert_eq!(loword(0), 0);
        assert_eq!(loword(0xFFFF), 0xFFFF);
    }

    #[test]
    fn hiword_is_signed() {
        assert_eq!(hiword_i16(0x0001_0000), 1);
        assert_eq!(hiword_i16(0xFFFF_0000), -1);
        assert_eq!(hiword_i16(0x7FFF_1234), 0x7FFF);
    }

    #[test]
    fn mouse_coordinates_are_signed_words() {
        assert_eq!(x_lparam(0x0002_0001), 1);
        assert_eq!(y_lparam(0x0002_0001), 2);
        // A pointer dragged above and left of the client origin.
        assert_eq!(x_lparam(-1), -1);
        assert_eq!(y_lparam(-1), -1);
    }

    #[test]
    fn xbutton_is_the_high_word_of_wparam() {
        assert_eq!(xbutton_wparam(0x0002_0000), 2);
        assert_eq!(xbutton_wparam(0x0001_FFFF), 1);
    }

    #[test]
    fn appcommand_masks_off_the_device_bits() {
        // Command 11 with the device/key-state bits set above bit 14.
        let lp = (0x800B_i32 << 16) as isize;
        assert_eq!(appcommand_lparam(lp), 11);
    }

    #[test]
    fn extended_bit_comes_from_the_lparam_high_word() {
        assert!(!is_extended_key(0));
        assert!(is_extended_key((KF_EXTENDED as isize) << 16));
    }

    #[test]
    fn mouse_flags_map_the_wparam_bits() {
        let m = mouse_flags((MK_CONTROL.0 | MK_SHIFT.0 | MK_LBUTTON.0) as usize, &[]);
        assert_eq!(
            m,
            EVENTFLAG_CONTROL_DOWN | EVENTFLAG_SHIFT_DOWN | EVENTFLAG_LEFT_MOUSE_BUTTON
        );
    }

    #[test]
    fn mouse_flags_take_alt_from_the_live_key_state() {
        assert_eq!(mouse_flags(0, &[]), 0);
        assert_eq!(mouse_flags(0, &[VK_MENU.0]), EVENTFLAG_ALT_DOWN);
    }

    #[test]
    fn mouse_flags_report_every_held_button() {
        let m = mouse_flags((MK_RBUTTON.0 | MK_MBUTTON.0) as usize, &[]);
        assert_eq!(
            m,
            EVENTFLAG_RIGHT_MOUSE_BUTTON | EVENTFLAG_MIDDLE_MOUSE_BUTTON
        );
    }

    #[test]
    fn keyboard_flags_report_held_modifiers_and_lit_locks() {
        let m = flags(
            u16::from(b'A'),
            0,
            &[VK_SHIFT.0, VK_CONTROL.0, VK_MENU.0],
            &[VK_NUMLOCK.0, VK_CAPITAL.0],
        );
        assert_eq!(
            m,
            EVENTFLAG_SHIFT_DOWN
                | EVENTFLAG_CONTROL_DOWN
                | EVENTFLAG_ALT_DOWN
                | EVENTFLAG_NUM_LOCK_ON
                | EVENTFLAG_CAPS_LOCK_ON
        );
    }

    #[test]
    fn extended_return_is_the_keypad_enter() {
        let extended = (KF_EXTENDED as isize) << 16;
        assert_eq!(flags(VK_RETURN.0, extended, &[], &[]), EVENTFLAG_IS_KEY_PAD);
        assert_eq!(flags(VK_RETURN.0, 0, &[], &[]), 0);
    }

    #[test]
    fn unextended_navigation_keys_are_the_keypad_ones() {
        let extended = (KF_EXTENDED as isize) << 16;
        for vk in [VK_HOME.0, VK_END.0, VK_UP.0, VK_DELETE.0, VK_PRIOR.0] {
            assert_eq!(flags(vk, 0, &[], &[]), EVENTFLAG_IS_KEY_PAD, "vk {vk:#x}");
            assert_eq!(flags(vk, extended, &[], &[]), 0, "vk {vk:#x} extended");
        }
    }

    #[test]
    fn numpad_keys_are_the_keypad_whatever_the_extended_bit_says() {
        let extended = (KF_EXTENDED as isize) << 16;
        for vk in [
            VK_NUMPAD0.0,
            VK_NUMPAD9.0,
            VK_ADD.0,
            VK_DECIMAL.0,
            VK_CLEAR.0,
        ] {
            assert_eq!(flags(vk, 0, &[], &[]), EVENTFLAG_IS_KEY_PAD, "vk {vk:#x}");
            assert_eq!(
                flags(vk, extended, &[], &[]),
                EVENTFLAG_IS_KEY_PAD,
                "vk {vk:#x} extended"
            );
        }
    }

    #[test]
    fn bare_modifiers_report_which_side_is_held() {
        assert_eq!(
            flags(VK_SHIFT.0, 0, &[VK_SHIFT.0, VK_LSHIFT.0], &[]),
            EVENTFLAG_SHIFT_DOWN | EVENTFLAG_IS_LEFT
        );
        assert_eq!(
            flags(VK_CONTROL.0, 0, &[VK_CONTROL.0, VK_RCONTROL.0], &[]),
            EVENTFLAG_CONTROL_DOWN | EVENTFLAG_IS_RIGHT
        );
        assert_eq!(
            flags(VK_MENU.0, 0, &[VK_MENU.0, VK_LMENU.0], &[]),
            EVENTFLAG_ALT_DOWN | EVENTFLAG_IS_LEFT
        );
    }

    #[test]
    fn a_modifier_with_neither_side_held_gets_no_side_flag() {
        assert_eq!(
            flags(VK_SHIFT.0, 0, &[VK_SHIFT.0], &[]),
            EVENTFLAG_SHIFT_DOWN
        );
    }

    #[test]
    fn the_windows_keys_carry_their_own_side() {
        assert_eq!(flags(VK_LWIN.0, 0, &[], &[]), EVENTFLAG_IS_LEFT);
        assert_eq!(flags(VK_RWIN.0, 0, &[], &[]), EVENTFLAG_IS_RIGHT);
    }

    #[test]
    fn cursor_shapes_map_onto_the_system_cursors() {
        let idc = |shape| cursor_to_win(shape).0 as usize;
        assert_eq!(idc(CursorShape::Pointer), IDC_ARROW.0 as usize);
        assert_eq!(idc(CursorShape::Hand), IDC_HAND.0 as usize);
        assert_eq!(idc(CursorShape::Grabbing), IDC_HAND.0 as usize);
        assert_eq!(idc(CursorShape::IBeam), IDC_IBEAM.0 as usize);
        assert_eq!(idc(CursorShape::ColumnResize), IDC_SIZEWE.0 as usize);
        assert_eq!(idc(CursorShape::RowResize), IDC_SIZENS.0 as usize);
        assert_eq!(idc(CursorShape::NorthEastResize), IDC_SIZENESW.0 as usize);
        assert_eq!(idc(CursorShape::SouthEastResize), IDC_SIZENWSE.0 as usize);
        assert_eq!(idc(CursorShape::Move), IDC_SIZEALL.0 as usize);
        assert_eq!(idc(CursorShape::Progress), IDC_APPSTARTING.0 as usize);
        assert_eq!(idc(CursorShape::NotAllowed), IDC_NO.0 as usize);
        assert_eq!(idc(CursorShape::Cross), IDC_CROSS.0 as usize);
        assert_eq!(idc(CursorShape::Wait), IDC_WAIT.0 as usize);
        assert_eq!(idc(CursorShape::Help), IDC_HELP.0 as usize);
    }

    #[test]
    fn every_button_message_maps_to_its_button() {
        assert_eq!(msg_to_button_code(WM_LBUTTONDBLCLK), BTN_LEFT);
        assert_eq!(msg_to_button_code(WM_RBUTTONUP), BTN_RIGHT);
        assert_eq!(msg_to_button_code(WM_MBUTTONDOWN), BTN_MIDDLE);
        // Anything unrecognised falls back to the left button.
        assert_eq!(msg_to_button_code(WM_KEYDOWN), BTN_LEFT);
    }

    #[test]
    fn only_the_press_half_is_a_button_down() {
        assert!(is_button_down(WM_LBUTTONDOWN));
        assert!(is_button_down(WM_RBUTTONDOWN));
        assert!(is_button_down(WM_MBUTTONDOWN));
        assert!(!is_button_down(WM_LBUTTONUP));
        assert!(!is_button_down(WM_LBUTTONDBLCLK));
    }

    #[test]
    fn alt_f4_starts_the_apps_own_shutdown() {
        assert_eq!(
            key_action(WM_SYSKEYDOWN, VK_F4.0, true),
            KeyAction::Shutdown
        );
    }

    #[test]
    fn f4_without_alt_or_without_syskey_is_an_ordinary_key() {
        assert_eq!(
            key_action(WM_SYSKEYDOWN, VK_F4.0, false),
            KeyAction::Dispatch {
                pressed: true,
                is_sys: true
            }
        );
        assert_eq!(
            key_action(WM_KEYDOWN, VK_F4.0, true),
            KeyAction::Dispatch {
                pressed: true,
                is_sys: false
            }
        );
    }

    #[test]
    fn browser_keys_navigate_on_the_press_only() {
        assert_eq!(
            key_action(WM_KEYDOWN, VK_BROWSER_FORWARD.0, false),
            KeyAction::HistoryNav(Some(NAV_FORWARD))
        );
        assert_eq!(
            key_action(WM_KEYDOWN, VK_BROWSER_BACK.0, false),
            KeyAction::HistoryNav(Some(NAV_BACK))
        );
        assert_eq!(
            key_action(WM_KEYUP, VK_BROWSER_BACK.0, false),
            KeyAction::HistoryNav(None)
        );
    }

    #[test]
    fn a_released_system_key_is_dispatched_as_one() {
        assert_eq!(
            key_action(WM_SYSKEYUP, u16::from(b'A'), false),
            KeyAction::Dispatch {
                pressed: false,
                is_sys: true
            }
        );
        assert_eq!(
            key_action(WM_KEYUP, u16::from(b'A'), false),
            KeyAction::Dispatch {
                pressed: false,
                is_sys: false
            }
        );
    }

    #[test]
    fn the_second_extra_mouse_button_goes_forward() {
        assert_eq!(xbutton_nav(WM_XBUTTONDOWN, XBUTTON2), Some(NAV_FORWARD));
        assert_eq!(xbutton_nav(WM_XBUTTONDOWN, 1), Some(NAV_BACK));
        assert_eq!(xbutton_nav(WM_XBUTTONUP, XBUTTON2), None);
    }

    #[test]
    fn only_the_two_browser_appcommands_navigate() {
        assert_eq!(
            appcommand_nav(APPCOMMAND_BROWSER_BACKWARD.0),
            Some(NAV_BACK)
        );
        assert_eq!(
            appcommand_nav(APPCOMMAND_BROWSER_FORWARD.0),
            Some(NAV_FORWARD)
        );
        assert_eq!(appcommand_nav(0), None);
    }
}
