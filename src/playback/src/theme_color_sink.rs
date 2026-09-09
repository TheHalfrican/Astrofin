//! Theme-color sink. Resets ThemeColor video mode on terminal playback
//! events. Active-true setVideoMode fires from the web_browser path
//! on metadata arrival; that's not mpv-derived and stays out of the
//! playback event stream.

use parking_lot::Mutex;
use std::sync::OnceLock;

use crate::types::{PlaybackEvent, PlaybackEventKind};

type SetCb = extern "C" fn(bool);

fn cb_slot() -> &'static Mutex<Option<SetCb>> {
    static SLOT: OnceLock<Mutex<Option<SetCb>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Install the ThemeColor::setVideoMode setter. `cb == None` disables.
pub fn jfn_playback_set_theme_video_mode_handler(cb: Option<SetCb>) {
    *cb_slot().lock() = cb;
}

pub(crate) fn deliver(ev: &PlaybackEvent) {
    match ev.kind {
        PlaybackEventKind::Finished | PlaybackEventKind::Canceled | PlaybackEventKind::Error => {
            if let Some(cb) = *cb_slot().lock() {
                cb(false);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::test_support;
    use crate::types::{PlaybackEvent, PlaybackEventKind};

    static CALLS: Mutex<Vec<bool>> = Mutex::new(Vec::new());

    extern "C" fn record(active: bool) {
        CALLS.lock().push(active);
    }

    fn arm() -> Vec<bool> {
        CALLS.lock().clear();
        jfn_playback_set_theme_video_mode_handler(Some(record));
        Vec::new()
    }

    fn calls() -> Vec<bool> {
        CALLS.lock().clone()
    }

    fn ev(kind: PlaybackEventKind) -> PlaybackEvent {
        PlaybackEvent::new(kind)
    }

    #[test]
    fn a_terminal_event_leaves_video_mode() {
        let _g = test_support::lock();
        let _ = arm();
        deliver(&ev(PlaybackEventKind::Finished));
        deliver(&ev(PlaybackEventKind::Canceled));
        deliver(&ev(PlaybackEventKind::Error));
        assert_eq!(calls(), vec![false, false, false]);
        jfn_playback_set_theme_video_mode_handler(None);
    }

    #[test]
    fn entering_playback_is_not_this_sinks_business() {
        // Active-true comes from the web layer on metadata arrival; it is not
        // mpv-derived and must not be synthesised here.
        let _g = test_support::lock();
        let _ = arm();
        for kind in [
            PlaybackEventKind::Started,
            PlaybackEventKind::Paused,
            PlaybackEventKind::TrackLoaded,
            PlaybackEventKind::PositionChanged,
            PlaybackEventKind::MediaTypeChanged,
        ] {
            deliver(&ev(kind));
        }
        assert!(calls().is_empty());
        jfn_playback_set_theme_video_mode_handler(None);
    }

    #[test]
    fn clearing_the_handler_disables_the_sink() {
        let _g = test_support::lock();
        let _ = arm();
        jfn_playback_set_theme_video_mode_handler(None);
        deliver(&ev(PlaybackEventKind::Finished));
        assert!(calls().is_empty());
    }
}
