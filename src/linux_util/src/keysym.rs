//! The X11/Wayland input translation tables: xkb keysym -> Windows VK code,
//! xkb modifier state -> CEF event flags, and what a raw key press does.
//!
//! Everything here is input -> output. The two backends' input modules
//! (`crate::input`, `crate::xkb`) do the xkb queries and the dispatch; the
//! tables themselves need neither a keymap nor a display.

use std::ffi::c_int;

use jfn_platform_abi::event_flags::{
    EVENTFLAG_ALT_DOWN, EVENTFLAG_CONTROL_DOWN, EVENTFLAG_SHIFT_DOWN,
};
use xkbcommon::xkb::keysyms as ks;

/// `XKB_KEY_XF86Back` — the browser Back key.
pub const XF86_BACK: u32 = 0x1008FF26;
/// `XKB_KEY_XF86Forward` — the browser Forward key.
pub const XF86_FORWARD: u32 = 0x1008FF27;

/// The CEF event-flag bits for an xkb modifier state, given which of the
/// three tracked modifiers are effectively active.
pub fn cef_mods(shift: bool, ctrl: bool, alt: bool) -> u32 {
    let mut m = 0u32;
    if shift {
        m |= EVENTFLAG_SHIFT_DOWN;
    }
    if ctrl {
        m |= EVENTFLAG_CONTROL_DOWN;
    }
    if alt {
        m |= EVENTFLAG_ALT_DOWN;
    }
    m
}

/// What a raw key event from a Linux backend does.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RawKey {
    /// A browser Back/Forward key on its press half: 1 forward, 0 back.
    HistoryNav(c_int),
    /// A browser Back/Forward key on its release half: swallowed, nothing
    /// dispatched.
    Swallow,
    /// Hand the key to CEF. `native` is the X11 keycode CEF expects in
    /// `native_key_code` — the evdev keycode plus 8, on Wayland as on X11.
    Dispatch { vkey: i32, native: i32 },
}

/// Route one raw key event.
pub fn raw_key_action(keysym: u32, native_code: u32, pressed: c_int) -> RawKey {
    if keysym == XF86_BACK || keysym == XF86_FORWARD {
        if pressed == 0 {
            return RawKey::Swallow;
        }
        return RawKey::HistoryNav((keysym == XF86_FORWARD) as c_int);
    }
    RawKey::Dispatch {
        vkey: keysym_to_vkey(keysym),
        native: native_code as i32 + 8,
    }
}

pub fn keysym_to_vkey(sym: u32) -> i32 {
    if (ks::KEY_a..=ks::KEY_z).contains(&sym) {
        return (b'A' as u32 + (sym - ks::KEY_a)) as i32;
    }
    if (ks::KEY_A..=ks::KEY_Z).contains(&sym) {
        return sym as i32;
    }
    if (ks::KEY_0..=ks::KEY_9).contains(&sym) {
        return sym as i32;
    }
    if (ks::KEY_F1..=ks::KEY_F12).contains(&sym) {
        return 0x70 + (sym - ks::KEY_F1) as i32;
    }

    match sym {
        ks::KEY_Return => 0x0D,
        ks::KEY_Escape => 0x1B,
        ks::KEY_Tab | ks::KEY_ISO_Left_Tab => 0x09,
        ks::KEY_BackSpace => 0x08,
        ks::KEY_space => 0x20,
        ks::KEY_Left => 0x25,
        ks::KEY_Up => 0x26,
        ks::KEY_Right => 0x27,
        ks::KEY_Down => 0x28,
        ks::KEY_Home => 0x24,
        ks::KEY_End => 0x23,
        ks::KEY_Page_Up => 0x21,
        ks::KEY_Page_Down => 0x22,
        ks::KEY_Delete => 0x2E,
        ks::KEY_Insert => 0x2D,
        // OEM punctuation. Required so Chromium can derive event.key (e.g.
        // '>' from Shift+Period) for DOM keydown handlers; without a VK
        // here, jellyfin-web shortcuts like '<' / '>' never match.
        ks::KEY_semicolon | ks::KEY_colon => 0xBA,
        ks::KEY_equal | ks::KEY_plus => 0xBB,
        ks::KEY_comma | ks::KEY_less => 0xBC,
        ks::KEY_minus | ks::KEY_underscore => 0xBD,
        ks::KEY_period | ks::KEY_greater => 0xBE,
        ks::KEY_slash | ks::KEY_question => 0xBF,
        ks::KEY_grave | ks::KEY_asciitilde => 0xC0,
        ks::KEY_bracketleft | ks::KEY_braceleft => 0xDB,
        ks::KEY_backslash | ks::KEY_bar => 0xDC,
        ks::KEY_bracketright | ks::KEY_braceright => 0xDD,
        ks::KEY_apostrophe | ks::KEY_quotedbl => 0xDE,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    // Literal keysyms rather than the `ks::` constants: the point of the table
    // is the X11 → Win32 mapping, and hard numbers would catch a keysym import
    // that silently changed meaning.

    #[test]
    fn a_lowercase_letter_maps_to_its_uppercase_virtual_key() {
        assert_eq!(keysym_to_vkey(0x0061), 0x41, "a");
        assert_eq!(keysym_to_vkey(0x007a), 0x5a, "z");
    }

    #[test]
    fn an_uppercase_letter_is_already_its_own_virtual_key() {
        assert_eq!(keysym_to_vkey(0x0041), 0x41, "A");
        assert_eq!(keysym_to_vkey(0x005a), 0x5a, "Z");
    }

    #[test]
    fn a_digit_is_its_own_virtual_key() {
        assert_eq!(keysym_to_vkey(0x0030), 0x30, "0");
        assert_eq!(keysym_to_vkey(0x0039), 0x39, "9");
    }

    #[test]
    fn the_function_keys_land_on_the_vk_f1_block() {
        assert_eq!(keysym_to_vkey(0xffbe), 0x70, "F1");
        assert_eq!(keysym_to_vkey(0xffc9), 0x7b, "F12");
        // F13 is outside the mapped run.
        assert_eq!(keysym_to_vkey(0xffca), 0, "F13");
    }

    #[test]
    fn the_editing_and_navigation_keys_have_their_win32_codes() {
        for (sym, vkey, name) in [
            (0xff0du32, 0x0Di32, "Return"),
            (0xff1b, 0x1B, "Escape"),
            (0xff09, 0x09, "Tab"),
            (0xfe20, 0x09, "ISO_Left_Tab"),
            (0xff08, 0x08, "BackSpace"),
            (0x0020, 0x20, "space"),
            (0xff51, 0x25, "Left"),
            (0xff52, 0x26, "Up"),
            (0xff53, 0x27, "Right"),
            (0xff54, 0x28, "Down"),
            (0xff50, 0x24, "Home"),
            (0xff57, 0x23, "End"),
            (0xff55, 0x21, "Page_Up"),
            (0xff56, 0x22, "Page_Down"),
            (0xffff, 0x2E, "Delete"),
            (0xff63, 0x2D, "Insert"),
        ] {
            assert_eq!(keysym_to_vkey(sym), vkey, "{name}");
        }
    }

    #[test]
    fn both_halves_of_an_oem_punctuation_key_share_one_virtual_key() {
        // Chromium derives `event.key` (e.g. '>' from Shift+Period) from the
        // VK, so the shifted keysym must map to the same OEM code.
        for (unshifted, shifted, vkey, name) in [
            (0x003bu32, 0x003au32, 0xBAi32, "semicolon/colon"),
            (0x003d, 0x002b, 0xBB, "equal/plus"),
            (0x002c, 0x003c, 0xBC, "comma/less"),
            (0x002d, 0x005f, 0xBD, "minus/underscore"),
            (0x002e, 0x003e, 0xBE, "period/greater"),
            (0x002f, 0x003f, 0xBF, "slash/question"),
            (0x0060, 0x007e, 0xC0, "grave/asciitilde"),
            (0x005b, 0x007b, 0xDB, "bracketleft/braceleft"),
            (0x005c, 0x007c, 0xDC, "backslash/bar"),
            (0x005d, 0x007d, 0xDD, "bracketright/braceright"),
            (0x0027, 0x0022, 0xDE, "apostrophe/quotedbl"),
        ] {
            assert_eq!(keysym_to_vkey(unshifted), vkey, "{name} unshifted");
            assert_eq!(keysym_to_vkey(shifted), vkey, "{name} shifted");
        }
    }

    #[test]
    fn an_unmapped_keysym_yields_no_virtual_key() {
        // 0 is the "no VK" sentinel the input layer checks for.
        for sym in [0x0000, 0x0021, 0xff67, 0x01000041] {
            assert_eq!(keysym_to_vkey(sym), 0, "{sym:#x}");
        }
    }

    #[test]
    fn the_letter_ranges_agree_with_the_xkb_constants() {
        // Guards the literals above against an xkbcommon rename.
        assert_eq!(keysym_to_vkey(ks::KEY_a), 0x41);
        assert_eq!(keysym_to_vkey(ks::KEY_Z), 0x5a);
        assert_eq!(keysym_to_vkey(ks::KEY_F1), 0x70);
        assert_eq!(keysym_to_vkey(ks::KEY_Escape), 0x1B);
    }
    #[test]
    fn no_active_modifier_is_no_flags() {
        assert_eq!(cef_mods(false, false, false), 0);
    }

    #[test]
    fn each_modifier_contributes_its_own_flag() {
        assert_eq!(cef_mods(true, false, false), EVENTFLAG_SHIFT_DOWN);
        assert_eq!(cef_mods(false, true, false), EVENTFLAG_CONTROL_DOWN);
        assert_eq!(cef_mods(false, false, true), EVENTFLAG_ALT_DOWN);
        assert_eq!(
            cef_mods(true, true, true),
            EVENTFLAG_SHIFT_DOWN | EVENTFLAG_CONTROL_DOWN | EVENTFLAG_ALT_DOWN
        );
    }

    #[test]
    fn the_browser_keys_navigate_on_their_press_half() {
        assert_eq!(raw_key_action(XF86_BACK, 158, 1), RawKey::HistoryNav(0));
        assert_eq!(raw_key_action(XF86_FORWARD, 159, 1), RawKey::HistoryNav(1));
    }

    #[test]
    fn the_browser_keys_are_swallowed_on_release() {
        assert_eq!(raw_key_action(XF86_BACK, 158, 0), RawKey::Swallow);
        assert_eq!(raw_key_action(XF86_FORWARD, 159, 0), RawKey::Swallow);
    }

    #[test]
    fn an_ordinary_key_is_dispatched_with_its_vk_and_x11_keycode() {
        // evdev keycode 30 ("A") is X11 keycode 38.
        assert_eq!(
            raw_key_action(ks::KEY_a, 30, 1),
            RawKey::Dispatch {
                vkey: keysym_to_vkey(ks::KEY_a),
                native: 38,
            }
        );
    }

    #[test]
    fn a_released_ordinary_key_is_still_dispatched() {
        assert_eq!(
            raw_key_action(ks::KEY_Escape, 1, 0),
            RawKey::Dispatch {
                vkey: 0x1B,
                native: 9,
            }
        );
    }
}
