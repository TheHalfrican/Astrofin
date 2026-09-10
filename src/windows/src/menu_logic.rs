//! Pure parts of the Win32 popup menu: the wide-string conversion, the item
//! flags, the anchor arithmetic and how a tracked menu's return value becomes
//! a [`MenuSelection`](jfn_platform_abi::MenuSelection) result.
//!
//! `crate::menu` keeps the `HMENU`, the posted messages and
//! `TrackPopupMenuEx`, none of which can run without the input thread.

use std::ffi::{OsStr, c_int};
use std::os::windows::ffi::OsStrExt;

use jfn_platform_abi::{MENU_DISMISSED, MenuItem};
use windows::Win32::UI::WindowsAndMessaging::{
    MENU_ITEM_FLAGS, MF_GRAYED, MF_STRING, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TPM_TOPALIGN,
};

/// `TrackPopupMenuEx` flags: return the command rather than posting it, tell
/// nobody about it, place the menu below-right of the anchor, and let the
/// right button pick too.
pub(crate) const TRACK_FLAGS: u32 =
    TPM_RETURNCMD.0 | TPM_NONOTIFY.0 | TPM_LEFTALIGN.0 | TPM_TOPALIGN.0 | TPM_RIGHTBUTTON.0;

/// A NUL-terminated UTF-16 copy of `s`, for the `PCWSTR` Win32 wants.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// What one [`MenuItem`] becomes in the `HMENU`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Append {
    /// A separator line.
    Separator,
    /// A command, with the flags it is appended under.
    Command(MENU_ITEM_FLAGS),
    /// Nothing: an item with no usable command id would be unpickable, and
    /// `TrackPopupMenuEx` reports "id 0" for a dismissal.
    Skip,
}

/// How an item joins the menu. Separators win over the id check, matching the
/// order the builder tests them in.
pub(crate) fn append_kind(item: &MenuItem) -> Append {
    if item.separator {
        return Append::Separator;
    }
    if item.id <= 0 {
        return Append::Skip;
    }
    Append::Command(if item.enabled {
        MF_STRING
    } else {
        MENU_ITEM_FLAGS(MF_STRING.0 | MF_GRAYED.0)
    })
}

/// The anchor in physical pixels, from the logical (view) coordinates the
/// request carries.
pub(crate) fn anchor(x: c_int, y: c_int, scale: f32) -> (i32, i32) {
    (
        (x as f32 * scale).round() as i32,
        (y as f32 * scale).round() as i32,
    )
}

/// The id to resolve the request with. A cancelled menu and a menu closed
/// without a pick both dismiss; anything else is the picked command.
pub(crate) fn selection(cancelled: bool, picked: i32) -> i32 {
    if cancelled || picked == 0 {
        MENU_DISMISSED
    } else {
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i32, label: &str, enabled: bool, separator: bool) -> MenuItem {
        MenuItem {
            id,
            label: label.to_string(),
            enabled,
            separator,
        }
    }

    #[test]
    fn wide_is_nul_terminated_utf16() {
        assert_eq!(wide("Ok"), vec![0x4F, 0x6B, 0]);
        assert_eq!(wide(""), vec![0]);
    }

    #[test]
    fn wide_encodes_astral_characters_as_a_surrogate_pair() {
        // U+1F3AC CLAPPER BOARD.
        assert_eq!(wide("\u{1F3AC}"), vec![0xD83C, 0xDFAC, 0]);
    }

    #[test]
    fn an_enabled_command_is_a_plain_string_item() {
        assert_eq!(
            append_kind(&item(7, "Play", true, false)),
            Append::Command(MF_STRING)
        );
    }

    #[test]
    fn a_disabled_command_is_greyed() {
        assert_eq!(
            append_kind(&item(7, "Play", false, false)),
            Append::Command(MENU_ITEM_FLAGS(MF_STRING.0 | MF_GRAYED.0))
        );
    }

    #[test]
    fn a_separator_is_appended_whatever_its_id_says() {
        assert_eq!(append_kind(&item(0, "", true, true)), Append::Separator);
        assert_eq!(append_kind(&item(3, "x", true, true)), Append::Separator);
    }

    #[test]
    fn an_item_with_no_usable_id_is_skipped() {
        assert_eq!(append_kind(&item(0, "Play", true, false)), Append::Skip);
        assert_eq!(append_kind(&item(-1, "Play", true, false)), Append::Skip);
    }

    #[test]
    fn the_anchor_scales_from_logical_to_physical() {
        assert_eq!(anchor(100, 50, 1.0), (100, 50));
        assert_eq!(anchor(100, 50, 2.0), (200, 100));
        // 1.5x rounds to nearest, not toward zero.
        assert_eq!(anchor(101, 51, 1.5), (152, 77));
    }

    #[test]
    fn a_negative_anchor_stays_negative() {
        assert_eq!(anchor(-10, -20, 2.0), (-20, -40));
    }

    #[test]
    fn a_pick_resolves_with_its_command_id() {
        assert_eq!(selection(false, 42), 42);
    }

    #[test]
    fn a_cancelled_or_empty_close_dismisses() {
        assert_eq!(selection(true, 42), MENU_DISMISSED);
        assert_eq!(selection(false, 0), MENU_DISMISSED);
        assert_eq!(selection(true, 0), MENU_DISMISSED);
    }

    #[test]
    fn track_flags_return_the_command_and_notify_nobody() {
        assert_eq!(TRACK_FLAGS & TPM_RETURNCMD.0, TPM_RETURNCMD.0);
        assert_eq!(TRACK_FLAGS & TPM_NONOTIFY.0, TPM_NONOTIFY.0);
        assert_eq!(TRACK_FLAGS & TPM_RIGHTBUTTON.0, TPM_RIGHTBUTTON.0);
    }
}
