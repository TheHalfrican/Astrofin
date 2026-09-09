//! xkb keysym → Windows VK code.

use xkbcommon::xkb::keysyms as ks;

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
}
