//! Hotkey decision logic.
//!
//! Pure classifier: the input dispatcher hands us a key-down's
//! `windows_key_code` + CEF modifier mask and we return the action to
//! perform. Bindings live here so platform translators stay generic.
//!
//! Fullscreen is only meaningful when the video player is the active
//! content. Music playback ignores fullscreen hotkeys; a paused video
//! still counts as "active" because the user may want to toggle
//! fullscreen while paused.

use crate::ffi::coord_slot;
use crate::types::{MediaType, PlaybackPhase, PlayerPresence};

#[repr(u8)]
pub enum HotkeyAction {
    None = 0,
    Shutdown = 1,
    ToggleFullscreen = 2,
}

// Stable Windows VK codes (also what CefKeyEvent.windows_key_code carries).
const VK_F: i32 = 0x46;
const VK_F4: i32 = 0x73;
const VK_F11: i32 = 0x7A;

// Mirror of CEF's EVENTFLAG_ALT_DOWN (include/internal/cef_types.h).
const EVENTFLAG_ALT_DOWN: u32 = 1 << 3;

fn video_player_active() -> bool {
    let guard = coord_slot().lock();
    let Some(c) = guard.as_ref() else {
        return false;
    };
    let s = c.snapshot();
    s.media_type == MediaType::Video
        && s.presence == PlayerPresence::Present
        && s.phase != PlaybackPhase::Stopped
}

/// Classify a key-down event. Caller invokes only for `KeyAction::Down`.
/// Returns the [`HotkeyAction`] the dispatcher must perform; `None` means
/// forward the event to the browser as normal.
pub fn jfn_hotkey_classify_keydown(windows_key_code: i32, modifiers: u32) -> u8 {
    if windows_key_code == VK_F4 && (modifiers & EVENTFLAG_ALT_DOWN) != 0 {
        return HotkeyAction::Shutdown as u8;
    }
    if windows_key_code == VK_F || windows_key_code == VK_F11 {
        if !video_player_active() {
            return HotkeyAction::None as u8;
        }
        return HotkeyAction::ToggleFullscreen as u8;
    }
    HotkeyAction::None as u8
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::test_support;

    const NO_MODIFIERS: u32 = 0;
    const VK_A: i32 = 0x41;
    const VK_ESCAPE: i32 = 0x1B;

    /// Runs `body` with a coordinator whose snapshot says a video is
    /// playing, then takes it back out again.
    fn with_video_player(body: impl FnOnce()) {
        let coord = test_support::video_playing_coordinator();
        *coord_slot().lock() = Some(coord);
        body();
        let taken = coord_slot().lock().take();
        if let Some(mut c) = taken {
            c.stop();
        }
    }

    #[test]
    fn alt_f4_asks_for_shutdown_whatever_is_playing() {
        let _g = test_support::lock();
        assert_eq!(
            jfn_hotkey_classify_keydown(VK_F4, EVENTFLAG_ALT_DOWN),
            HotkeyAction::Shutdown as u8
        );
        with_video_player(|| {
            assert_eq!(
                jfn_hotkey_classify_keydown(VK_F4, EVENTFLAG_ALT_DOWN),
                HotkeyAction::Shutdown as u8
            );
        });
    }

    #[test]
    fn f4_without_alt_is_forwarded_to_the_browser() {
        let _g = test_support::lock();
        assert_eq!(
            jfn_hotkey_classify_keydown(VK_F4, NO_MODIFIERS),
            HotkeyAction::None as u8
        );
    }

    #[test]
    fn f_and_f11_toggle_fullscreen_while_a_video_player_is_active() {
        let _g = test_support::lock();
        with_video_player(|| {
            for key in [VK_F, VK_F11] {
                assert_eq!(
                    jfn_hotkey_classify_keydown(key, NO_MODIFIERS),
                    HotkeyAction::ToggleFullscreen as u8
                );
            }
        });
    }

    #[test]
    fn fullscreen_keys_are_forwarded_when_nothing_is_playing() {
        let _g = test_support::lock();
        // No coordinator at all: the classifier must not assume one.
        let saved = coord_slot().lock().take();
        for key in [VK_F, VK_F11] {
            assert_eq!(
                jfn_hotkey_classify_keydown(key, NO_MODIFIERS),
                HotkeyAction::None as u8
            );
        }
        *coord_slot().lock() = saved;
    }

    #[test]
    fn an_unbound_key_is_always_forwarded() {
        let _g = test_support::lock();
        with_video_player(|| {
            for key in [VK_A, VK_ESCAPE] {
                assert_eq!(
                    jfn_hotkey_classify_keydown(key, NO_MODIFIERS),
                    HotkeyAction::None as u8
                );
                assert_eq!(
                    jfn_hotkey_classify_keydown(key, EVENTFLAG_ALT_DOWN),
                    HotkeyAction::None as u8
                );
            }
        });
    }

    #[test]
    fn a_modifier_does_not_stop_the_fullscreen_keys() {
        let _g = test_support::lock();
        with_video_player(|| {
            assert_eq!(
                jfn_hotkey_classify_keydown(VK_F11, EVENTFLAG_ALT_DOWN),
                HotkeyAction::ToggleFullscreen as u8
            );
        });
    }
}
