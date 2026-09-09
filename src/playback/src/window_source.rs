//! The mpv-backed [`WindowSource`]: on backends where mpv owns the OS
//! window (macOS / Windows / X11), live geometry comes from the ingest
//! extent cell that mpv's property observations feed.

use jfn_platform_abi::{WindowSnapshot, WindowSource};

pub struct MpvWindowSource;

pub static MPV_WINDOW_SOURCE: MpvWindowSource = MpvWindowSource;

impl WindowSource for MpvWindowSource {
    fn snapshot(&self) -> WindowSnapshot {
        WindowSnapshot {
            extent: crate::ingest_driver::jfn_playback_window_extent(),
            position: jfn_platform_abi::get().query_window_position(),
            maximized: crate::ingest_driver::jfn_playback_window_maximized(),
            fullscreen: crate::ingest_driver::jfn_playback_fullscreen(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::ingest::observe_id;
    use crate::test_support;
    use jfn_mpv::{Event, PropertyValue};

    fn prop(id: u64, value: PropertyValue) -> Event {
        Event::PropertyChange {
            id,
            name: String::new(),
            value,
        }
    }

    #[test]
    fn the_snapshot_mirrors_what_mpv_last_reported() {
        let _g = test_support::lock();
        for (fullscreen, maximized) in [(true, false), (false, true)] {
            crate::ingest_driver::jfn_playback_ingest_mpv_event_owned(
                &prop(observe_id::WINDOW_MAX, PropertyValue::Flag(maximized)),
                1.0,
                None,
            );
            crate::ingest_driver::jfn_playback_ingest_mpv_event_owned(
                &prop(observe_id::FULLSCREEN, PropertyValue::Flag(fullscreen)),
                1.0,
                None,
            );
            let snap = MPV_WINDOW_SOURCE.snapshot();
            assert_eq!(snap.fullscreen, fullscreen);
            assert_eq!(snap.maximized, maximized);
            assert_eq!(
                snap.extent.map(|e| e.physical()),
                crate::ingest_driver::jfn_playback_window_extent().map(|e| e.physical())
            );
        }
    }

    #[test]
    fn the_position_comes_from_the_platform_not_from_mpv() {
        let _g = test_support::lock();
        // The stub platform reports no position; mpv's ingest state has no
        // say in it either way.
        assert!(MPV_WINDOW_SOURCE.snapshot().position.is_none());
    }
}
