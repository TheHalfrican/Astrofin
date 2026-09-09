//! Browser playback sink. Forwards UI-affecting events to the embedded
//! web view via the exec_js callback installed at boot. Reads only
//! from the event snapshot.

use parking_lot::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::exec_js::call as call_exec_js;
use crate::types::{PlaybackEvent, PlaybackEventKind};

#[derive(Serialize)]
struct BufferedRange {
    start: i64,
    end: i64,
}

type SetHzCb = extern "C" fn(f64);

struct Handlers {
    set_hz: Option<SetHzCb>,
}

fn slot() -> &'static Mutex<Handlers> {
    static SLOT: OnceLock<Mutex<Handlers>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(Handlers { set_hz: None }))
}

/// Install / clear the browsers.setRefreshRate handler.
pub fn jfn_playback_set_browsers_refresh_rate_handler(cb: Option<SetHzCb>) {
    slot().lock().set_hz = cb;
}

// Mirrors maximized-before-fullscreen state so the geometry-save tail in
// main can read it after coordinator shutdown without keeping coord alive.
static WAS_MAXIMIZED: AtomicBool = AtomicBool::new(false);

/// Geometry-save tail reads this at shutdown.
pub fn jfn_playback_was_maximized_before_fullscreen() -> bool {
    WAS_MAXIMIZED.load(Ordering::Relaxed)
}

pub(crate) fn deliver(ev: &PlaybackEvent) {
    let snap = &ev.snapshot;
    match ev.kind {
        PlaybackEventKind::Started => call_exec_js("window._nativeEmit('playing')"),
        PlaybackEventKind::Paused => call_exec_js("window._nativeEmit('paused')"),
        PlaybackEventKind::Finished => call_exec_js("window._nativeEmit('finished')"),
        PlaybackEventKind::Canceled => call_exec_js("window._nativeEmit('canceled')"),
        PlaybackEventKind::Error => {
            let msg = if ev.error_message.is_empty() {
                "Playback error"
            } else {
                ev.error_message.as_str()
            };
            let text = jfn_js_json::to_js_json(msg).unwrap_or_else(|| "\"\"".to_string());
            call_exec_js(&format!("window._nativeEmit('error',{text})"));
        }
        PlaybackEventKind::SeekingChanged => {
            if ev.flag {
                call_exec_js("window._nativeEmit('seeking')");
            }
        }
        PlaybackEventKind::TrackLoaded => {
            // Variant switch (same Jellyfin Id): JS's playerLoad path doesn't
            // fire its own pause UI, so drive the pause indicator from here.
            // Cleared on first-frame Started via the Started → 'playing' emit.
            if snap.variant_switch_pending {
                call_exec_js("window._nativeEmit('paused')");
            }
        }
        PlaybackEventKind::PositionChanged => {
            let ms = (snap.position_us / 1000) as i32;
            call_exec_js(&format!("window._nativeUpdatePosition({})", ms));
        }
        PlaybackEventKind::DurationChanged => {
            let ms = (snap.duration_us / 1000) as i32;
            call_exec_js(&format!("window._nativeUpdateDuration({})", ms));
        }
        PlaybackEventKind::RateChanged => {
            call_exec_js(&format!("window._nativeSetRate({})", snap.rate));
        }
        PlaybackEventKind::FullscreenChanged => {
            // Mirror was-maximized so the geometry-save tail in main can
            // read it after coord shutdown without keeping coord alive.
            WAS_MAXIMIZED.store(snap.maximized_before_fullscreen, Ordering::Relaxed);
            call_exec_js(&format!(
                "window._nativeFullscreenChanged({})",
                if snap.fullscreen { "true" } else { "false" }
            ));
        }
        PlaybackEventKind::DisplayHzChanged => {
            if let Some(cb) = slot().lock().set_hz {
                cb(snap.display_hz);
            }
        }
        PlaybackEventKind::BufferedRangesChanged => {
            let ranges: Vec<BufferedRange> = snap
                .buffered
                .iter()
                .map(|r| BufferedRange {
                    start: r.start_ticks,
                    end: r.end_ticks,
                })
                .collect();
            let json = jfn_js_json::to_js_json(&ranges).unwrap_or_else(|| "[]".to_string());
            call_exec_js(&format!("window._nativeUpdateBufferedRanges({json})"));
        }
        PlaybackEventKind::BufferingChanged
        | PlaybackEventKind::MediaTypeChanged
        | PlaybackEventKind::MetadataChanged
        | PlaybackEventKind::ArtworkChanged
        | PlaybackEventKind::QueueCapsChanged
        | PlaybackEventKind::Seeked => {
            // Not surfaced via this sink. JS already owns metadata.
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::test_support;
    use crate::types::{MediaType, PlaybackBufferedRange, PlaybackPhase};

    static HZ: Mutex<Vec<f64>> = Mutex::new(Vec::new());

    extern "C" fn record_hz(hz: f64) {
        HZ.lock().push(hz);
    }

    fn ev(kind: PlaybackEventKind) -> PlaybackEvent {
        PlaybackEvent::new(kind)
    }

    #[test]
    fn the_four_terminal_states_emit_their_own_names() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        deliver(&ev(PlaybackEventKind::Started));
        deliver(&ev(PlaybackEventKind::Paused));
        deliver(&ev(PlaybackEventKind::Finished));
        deliver(&ev(PlaybackEventKind::Canceled));
        assert_eq!(
            js.take(),
            vec![
                "window._nativeEmit('playing')".to_string(),
                "window._nativeEmit('paused')".to_string(),
                "window._nativeEmit('finished')".to_string(),
                "window._nativeEmit('canceled')".to_string(),
            ]
        );
    }

    #[test]
    fn an_error_message_is_escaped_into_the_emit() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        let mut e = ev(PlaybackEventKind::Error);
        e.error_message = "he said \"no\"\nand </script>".to_string();
        deliver(&e);
        assert_eq!(
            js.only(),
            r#"window._nativeEmit('error',"he said \"no\"\nand </script>")"#
        );
    }

    #[test]
    fn an_empty_error_message_falls_back_to_a_generic_one() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        deliver(&ev(PlaybackEventKind::Error));
        assert_eq!(js.only(), r#"window._nativeEmit('error',"Playback error")"#);
    }

    #[test]
    fn only_the_seeking_edge_is_emitted() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        let mut on = ev(PlaybackEventKind::SeekingChanged);
        on.flag = true;
        deliver(&on);
        assert_eq!(js.only(), "window._nativeEmit('seeking')");
        // Seek completion is reported by mpv's position, not by this event.
        deliver(&ev(PlaybackEventKind::SeekingChanged));
        assert!(js.take().is_empty());
    }

    #[test]
    fn a_variant_switch_load_drives_the_pause_indicator() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        let mut e = ev(PlaybackEventKind::TrackLoaded);
        e.snapshot.variant_switch_pending = true;
        deliver(&e);
        assert_eq!(js.only(), "window._nativeEmit('paused')");
        // A plain load is the JS layer's own business.
        deliver(&ev(PlaybackEventKind::TrackLoaded));
        assert!(js.take().is_empty());
    }

    #[test]
    fn position_and_duration_are_pushed_in_whole_milliseconds() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        let mut pos = ev(PlaybackEventKind::PositionChanged);
        pos.snapshot.position_us = 1_500_999;
        deliver(&pos);
        assert_eq!(js.only(), "window._nativeUpdatePosition(1500)");
        let mut dur = ev(PlaybackEventKind::DurationChanged);
        dur.snapshot.duration_us = 3_600_000_000;
        deliver(&dur);
        assert_eq!(js.only(), "window._nativeUpdateDuration(3600000)");
    }

    #[test]
    fn a_rate_change_pushes_mpvs_own_number() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        let mut e = ev(PlaybackEventKind::RateChanged);
        e.snapshot.rate = 1.5;
        deliver(&e);
        assert_eq!(js.only(), "window._nativeSetRate(1.5)");
        e.snapshot.rate = 2.0;
        deliver(&e);
        assert_eq!(js.only(), "window._nativeSetRate(2)");
    }

    #[test]
    fn a_fullscreen_change_pushes_a_js_boolean_and_mirrors_maximized() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        let mut e = ev(PlaybackEventKind::FullscreenChanged);
        e.snapshot.fullscreen = true;
        e.snapshot.maximized_before_fullscreen = true;
        deliver(&e);
        assert_eq!(js.only(), "window._nativeFullscreenChanged(true)");
        assert!(jfn_playback_was_maximized_before_fullscreen());

        e.snapshot.fullscreen = false;
        e.snapshot.maximized_before_fullscreen = false;
        deliver(&e);
        assert_eq!(js.only(), "window._nativeFullscreenChanged(false)");
        assert!(!jfn_playback_was_maximized_before_fullscreen());
    }

    #[test]
    fn buffered_ranges_are_pushed_as_a_json_array_of_ticks() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        let mut e = ev(PlaybackEventKind::BufferedRangesChanged);
        e.snapshot.buffered = vec![
            PlaybackBufferedRange {
                start_ticks: 0,
                end_ticks: 25_000_000,
            },
            PlaybackBufferedRange {
                start_ticks: 30_000_000,
                end_ticks: 45_000_000,
            },
        ];
        deliver(&e);
        assert_eq!(
            js.only(),
            r#"window._nativeUpdateBufferedRanges([{"start":0,"end":25000000},{"start":30000000,"end":45000000}])"#
        );
    }

    #[test]
    fn an_empty_buffered_set_still_pushes_an_empty_array() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        deliver(&ev(PlaybackEventKind::BufferedRangesChanged));
        assert_eq!(js.only(), "window._nativeUpdateBufferedRanges([])");
    }

    #[test]
    fn a_display_hz_change_goes_to_the_refresh_rate_handler_not_to_js() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        HZ.lock().clear();
        jfn_playback_set_browsers_refresh_rate_handler(Some(record_hz));
        let mut e = ev(PlaybackEventKind::DisplayHzChanged);
        e.snapshot.display_hz = 59.94;
        deliver(&e);
        assert_eq!(HZ.lock().clone(), vec![59.94]);
        assert!(js.take().is_empty());
        jfn_playback_set_browsers_refresh_rate_handler(None);
    }

    #[test]
    fn clearing_the_refresh_rate_handler_drops_the_hz_push() {
        let _g = test_support::lock();
        let _js = test_support::JsRecorder::install();
        jfn_playback_set_browsers_refresh_rate_handler(Some(record_hz));
        jfn_playback_set_browsers_refresh_rate_handler(None);
        HZ.lock().clear();
        let mut e = ev(PlaybackEventKind::DisplayHzChanged);
        e.snapshot.display_hz = 120.0;
        deliver(&e);
        assert!(HZ.lock().is_empty());
    }

    #[test]
    fn the_events_js_already_owns_reach_the_page_through_no_other_route() {
        let _g = test_support::lock();
        let js = test_support::JsRecorder::install();
        for kind in [
            PlaybackEventKind::BufferingChanged,
            PlaybackEventKind::MediaTypeChanged,
            PlaybackEventKind::MetadataChanged,
            PlaybackEventKind::ArtworkChanged,
            PlaybackEventKind::QueueCapsChanged,
            PlaybackEventKind::Seeked,
        ] {
            let mut e = ev(kind);
            e.snapshot.media_type = MediaType::Video;
            e.snapshot.phase = PlaybackPhase::Playing;
            deliver(&e);
        }
        assert!(js.take().is_empty());
    }
}
