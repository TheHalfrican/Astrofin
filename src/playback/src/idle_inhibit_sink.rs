//! Idle-inhibit sink. Watches phase + media_type transitions and drives
//! the platform idle-inhibit level via the registered callback wired to
//! `g_platform.set_idle_inhibit`.

use parking_lot::Mutex;
use std::sync::OnceLock;

use crate::types::{MediaType, PlaybackEvent, PlaybackEventKind, PlaybackPhase, PlaybackSnapshot};

// IdleInhibitLevel: None, System, Display.
const LEVEL_NONE: u32 = 0;
const LEVEL_SYSTEM: u32 = 1;
const LEVEL_DISPLAY: u32 = 2;

type SetCb = extern "C" fn(u32);

fn cb_slot() -> &'static Mutex<Option<SetCb>> {
    static SLOT: OnceLock<Mutex<Option<SetCb>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Install the platform idle-inhibit setter. `cb == None` disables the sink.
pub fn jfn_playback_set_idle_inhibit_handler(cb: Option<SetCb>) {
    *cb_slot().lock() = cb;
}

fn apply(snap: &PlaybackSnapshot) {
    let Some(cb) = *cb_slot().lock() else {
        return;
    };
    let level = if snap.phase != PlaybackPhase::Playing {
        LEVEL_NONE
    } else if snap.media_type == MediaType::Audio {
        LEVEL_SYSTEM
    } else {
        LEVEL_DISPLAY
    };
    cb(level);
}

pub(crate) fn deliver(ev: &PlaybackEvent) {
    match ev.kind {
        PlaybackEventKind::Started
        | PlaybackEventKind::Paused
        | PlaybackEventKind::Finished
        | PlaybackEventKind::Canceled
        | PlaybackEventKind::Error
        | PlaybackEventKind::MediaTypeChanged => apply(&ev.snapshot),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::test_support;
    use crate::types::PlaybackEvent;

    static CALLS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

    extern "C" fn record(level: u32) {
        CALLS.lock().push(level);
    }

    fn arm() {
        CALLS.lock().clear();
        jfn_playback_set_idle_inhibit_handler(Some(record));
    }

    fn calls() -> Vec<u32> {
        CALLS.lock().clone()
    }

    fn ev(kind: PlaybackEventKind, phase: PlaybackPhase, media: MediaType) -> PlaybackEvent {
        let mut e = PlaybackEvent::new(kind);
        e.snapshot.phase = phase;
        e.snapshot.media_type = media;
        e
    }

    #[test]
    fn playing_video_inhibits_the_display() {
        let _g = test_support::lock();
        arm();
        deliver(&ev(
            PlaybackEventKind::Started,
            PlaybackPhase::Playing,
            MediaType::Video,
        ));
        assert_eq!(calls(), vec![LEVEL_DISPLAY]);
        jfn_playback_set_idle_inhibit_handler(None);
    }

    #[test]
    fn playing_audio_inhibits_only_the_system() {
        let _g = test_support::lock();
        arm();
        deliver(&ev(
            PlaybackEventKind::Started,
            PlaybackPhase::Playing,
            MediaType::Audio,
        ));
        assert_eq!(calls(), vec![LEVEL_SYSTEM]);
        jfn_playback_set_idle_inhibit_handler(None);
    }

    #[test]
    fn anything_but_playing_releases_the_inhibit() {
        let _g = test_support::lock();
        arm();
        for phase in [
            PlaybackPhase::Paused,
            PlaybackPhase::Stopped,
            PlaybackPhase::Starting,
        ] {
            deliver(&ev(PlaybackEventKind::Paused, phase, MediaType::Video));
        }
        assert_eq!(calls(), vec![LEVEL_NONE, LEVEL_NONE, LEVEL_NONE]);
        jfn_playback_set_idle_inhibit_handler(None);
    }

    #[test]
    fn an_unknown_media_type_is_treated_as_video() {
        let _g = test_support::lock();
        arm();
        deliver(&ev(
            PlaybackEventKind::MediaTypeChanged,
            PlaybackPhase::Playing,
            MediaType::Unknown,
        ));
        assert_eq!(calls(), vec![LEVEL_DISPLAY]);
        jfn_playback_set_idle_inhibit_handler(None);
    }

    #[test]
    fn events_outside_the_watched_set_never_change_the_level() {
        let _g = test_support::lock();
        arm();
        for kind in [
            PlaybackEventKind::PositionChanged,
            PlaybackEventKind::SeekingChanged,
            PlaybackEventKind::RateChanged,
            PlaybackEventKind::TrackLoaded,
            PlaybackEventKind::BufferingChanged,
        ] {
            deliver(&ev(kind, PlaybackPhase::Playing, MediaType::Video));
        }
        assert!(calls().is_empty());
        jfn_playback_set_idle_inhibit_handler(None);
    }

    #[test]
    fn clearing_the_handler_disables_the_sink() {
        let _g = test_support::lock();
        arm();
        jfn_playback_set_idle_inhibit_handler(None);
        deliver(&ev(
            PlaybackEventKind::Started,
            PlaybackPhase::Playing,
            MediaType::Video,
        ));
        assert!(calls().is_empty());
    }
}
