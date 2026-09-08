//! On-demand mpv playback statistics for jellyfin-web's "Playback Info"
//! overlay.
//!
//! The overlay is open for seconds at a time and wants a dozen properties
//! nothing else in the app reads, several of which mpv re-emits every
//! frame (`avsync`, `estimated-vf-fps`). Observing them for the whole
//! process would put that churn on the ingest thread permanently, so the
//! set is observed only while the panel asks for it: the web layer calls
//! `jmpNative.playerStatsActive(true)` when it starts polling `getStats()`
//! and `false` when it stops (see `src/web/mpv-stats.js`).
//!
//! Flow matches the rest of the app — mpv is authoritative and state moves
//! outward. Property changes arrive on the mpv ingest thread, are coalesced
//! into [`StatsData`], and are pushed to JS as `window._nativeUpdateStats`
//! at most once per [`PUSH_INTERVAL`]. Nothing here reads mpv
//! synchronously, so none of it can deadlock the event thread.
//!
//! Unlike every other JS push this one does not travel through the
//! coordinator: the numbers are telemetry, not playback state. Putting them
//! in `PlaybackSnapshot` would add a dozen `String`s to a struct that is
//! cloned for every event (position updates included), and a new
//! `PlaybackEventKind` would fan out to the MPRIS/SMTC sinks that have no
//! use for it.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

use jfn_mpv::PropertyValue;

/// Minimum wall-clock gap between two `_nativeUpdateStats` pushes.
const PUSH_INTERVAL: Duration = Duration::from_millis(1000);

/// How long after activation the first push may go out. mpv delivers the
/// initial value of every freshly observed property within a few
/// milliseconds; waiting a moment means the first snapshot JS sees is a
/// full one rather than whichever property happened to arrive first.
const FIRST_PUSH_DELAY: Duration = Duration::from_millis(250);

/// `reply_userdata` shared by every stats observation. One id for the whole
/// set means a single `mpv_unobserve_property` call retires all of them;
/// the digest dispatches on the property name instead.
pub const STATS_OBSERVE_ID: u64 = 100;

/// The observed set. `display-fps` is deliberately absent — it is observed
/// for the lifetime of the process by
/// [`crate::ingest_driver::jfn_playback_observe_mpv_properties`], and is
/// folded into the pushed snapshot from that cache.
const OBSERVED: &[(&std::ffi::CStr, jfn_mpv::sys::mpv_format)] = {
    use jfn_mpv::sys::mpv_format;
    const STR: mpv_format = mpv_format::MPV_FORMAT_STRING;
    const MPV_FORMAT_DOUBLE: mpv_format = mpv_format::MPV_FORMAT_DOUBLE;
    const MPV_FORMAT_INT64: mpv_format = mpv_format::MPV_FORMAT_INT64;
    const MPV_FORMAT_NODE: mpv_format = mpv_format::MPV_FORMAT_NODE;
    &[
        (c"video-codec", STR),
        (c"hwdec-current", STR),
        // One node instead of four scalars: w/h/pixelformat/colormatrix all
        // arrive together and only change on a video reconfig.
        (c"video-params", MPV_FORMAT_NODE),
        (c"container-fps", MPV_FORMAT_DOUBLE),
        (c"estimated-vf-fps", MPV_FORMAT_DOUBLE),
        (c"frame-drop-count", MPV_FORMAT_INT64),
        (c"decoder-frame-drop-count", MPV_FORMAT_INT64),
        (c"video-bitrate", MPV_FORMAT_DOUBLE),
        (c"audio-codec-name", STR),
        (c"audio-params", MPV_FORMAT_NODE),
        (c"audio-bitrate", MPV_FORMAT_DOUBLE),
        (c"demuxer-cache-duration", MPV_FORMAT_DOUBLE),
        (c"cache-buffering-state", MPV_FORMAT_INT64),
        (c"current-vo", STR),
        (c"current-ao", STR),
        (c"avsync", MPV_FORMAT_DOUBLE),
        (c"mpv-version", STR),
    ]
};

/// One coalesced statistics snapshot. Every field is optional: mpv reports
/// `PropertyValue::None` for anything unavailable (no audio track, a VO
/// that does not count dropped frames), and an absent field is omitted
/// from the JSON so the JS builder can skip the row entirely.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hwdec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pixel_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_matrix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_fps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_fps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_frames_vo: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_frames_decoder: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_bitrate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_sample_rate: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_channels: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_bitrate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ao: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpv_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_buffering: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_fps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avsync: Option<f64>,
}

impl StatsData {
    /// Fold one property change in. Returns true when the value changed,
    /// which is what arms the next push.
    fn apply(&mut self, name: &str, value: &PropertyValue) -> bool {
        match name {
            "video-codec" => set(&mut self.video_codec, as_string(value)),
            "hwdec-current" => set(&mut self.hwdec, as_string(value)),
            "video-params" => {
                let mut changed = set(&mut self.width, node_int(value, "w"));
                changed |= set(&mut self.height, node_int(value, "h"));
                changed |= set(&mut self.pixel_format, node_string(value, "pixelformat"));
                changed |= set(&mut self.color_matrix, node_string(value, "colormatrix"));
                changed
            }
            "container-fps" => set(&mut self.container_fps, as_double(value)),
            "estimated-vf-fps" => set(&mut self.estimated_fps, as_double(value)),
            "frame-drop-count" => set(&mut self.dropped_frames_vo, as_int(value)),
            "decoder-frame-drop-count" => set(&mut self.dropped_frames_decoder, as_int(value)),
            "video-bitrate" => set(&mut self.video_bitrate, as_double(value)),
            "current-vo" => set(&mut self.vo, as_string(value)),
            "audio-codec-name" => set(&mut self.audio_codec, as_string(value)),
            "audio-params" => {
                let mut changed = set(&mut self.audio_format, node_string(value, "format"));
                changed |= set(&mut self.audio_sample_rate, node_int(value, "samplerate"));
                changed |= set(&mut self.audio_channels, node_int(value, "channel-count"));
                changed
            }
            "audio-bitrate" => set(&mut self.audio_bitrate, as_double(value)),
            "current-ao" => set(&mut self.ao, as_string(value)),
            "mpv-version" => set(&mut self.mpv_version, as_string(value)),
            "demuxer-cache-duration" => set(&mut self.cache_duration, as_double(value)),
            "cache-buffering-state" => set(&mut self.cache_buffering, as_int(value)),
            "avsync" => set(&mut self.avsync, as_double(value)),
            _ => false,
        }
    }
}

struct Inner {
    active: bool,
    data: StatsData,
    /// A change has landed that the last push did not carry.
    dirty: bool,
    last_push: Option<Instant>,
}

fn inner() -> &'static Mutex<Inner> {
    static SLOT: std::sync::OnceLock<Mutex<Inner>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| {
        Mutex::new(Inner {
            active: false,
            data: StatsData::default(),
            dirty: false,
            last_push: None,
        })
    })
}

/// Start or stop the stats observations. Returns true when this call
/// changed the state, so the caller logs one line per real transition.
///
/// Safe to call from any thread except the mpv event thread — the libmpv
/// calls here are queue operations, not synchronous property reads, but
/// keeping them off that thread costs nothing and keeps the rule simple.
pub fn jfn_playback_set_stats_active(active: bool) -> bool {
    {
        let mut g = inner().lock();
        if g.active == active {
            return false;
        }
        g.active = active;
        g.data = StatsData::default();
        g.dirty = false;
        // Backdate so the first push lands FIRST_PUSH_DELAY after
        // activation while the once-per-second floor still holds.
        g.last_push = active
            .then(|| Instant::now().checked_sub(PUSH_INTERVAL - FIRST_PUSH_DELAY))
            .flatten();
    }
    // libmpv calls outside our lock: the ingest thread takes that lock and
    // must never wait behind an FFI call.
    if active {
        observe();
    } else {
        unobserve();
        crate::exec_js::call("window._nativeUpdateStats && window._nativeUpdateStats(null)");
    }
    true
}

/// True while the stats properties are observed.
#[must_use]
pub fn jfn_playback_stats_active() -> bool {
    inner().lock().active
}

fn observe() {
    let Some(raw) = jfn_mpv::boot::current_raw_handle() else {
        return;
    };
    for &(name, fmt) in OBSERVED {
        unsafe { jfn_mpv::sys::mpv_observe_property(raw, STATS_OBSERVE_ID, name.as_ptr(), fmt) };
    }
}

fn unobserve() {
    let Some(raw) = jfn_mpv::boot::current_raw_handle() else {
        return;
    };
    // One id for the whole set, so one call retires all of them.
    unsafe { jfn_mpv::sys::mpv_unobserve_property(raw, STATS_OBSERVE_ID) };
}

/// Fold one observed stats property in and push the coalesced snapshot to
/// JS when one is due. Called from the mpv ingest thread.
pub(crate) fn on_property(name: &str, value: &PropertyValue) {
    let json = {
        let mut g = inner().lock();
        if !g.active {
            return;
        }
        if g.data.apply(name, value) {
            g.dirty = true;
        }
        if !g.dirty {
            return;
        }
        let now = Instant::now();
        if g.last_push.is_some_and(|last| now - last < PUSH_INTERVAL) {
            return;
        }
        g.last_push = Some(now);
        g.dirty = false;
        let mut data = g.data.clone();
        drop(g);
        // Refresh rate is already observed process-wide; take it from that
        // cache rather than observing the property a second time.
        let hz = crate::ingest_driver::jfn_playback_display_hz();
        if hz > 0.0 {
            data.display_fps = Some(hz);
        }
        jfn_js_json::to_js_json(&data)
    };
    if let Some(json) = json {
        crate::exec_js::call(&format!("window._nativeUpdateStats({json})"));
    }
}

// ---------------------------------------------------------------------
// Value helpers
// ---------------------------------------------------------------------

/// Assign and report whether anything changed.
fn set<T: PartialEq>(slot: &mut Option<T>, value: Option<T>) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

/// mpv reports an unavailable string property as `None`; an empty string
/// carries no more information, so both become `None`.
fn as_string(v: &PropertyValue) -> Option<String> {
    match v {
        PropertyValue::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn as_int(v: &PropertyValue) -> Option<i64> {
    match v {
        PropertyValue::Int(i) => Some(*i),
        _ => None,
    }
}

fn as_double(v: &PropertyValue) -> Option<f64> {
    match v {
        PropertyValue::Double(d) if d.is_finite() => Some(*d),
        _ => None,
    }
}

fn node_int(v: &PropertyValue, key: &str) -> Option<i64> {
    match v {
        PropertyValue::Node(n) => n.get(key).and_then(jfn_mpv::Node::as_int),
        _ => None,
    }
}

fn node_string(v: &PropertyValue, key: &str) -> Option<String> {
    match v {
        PropertyValue::Node(n) => n
            .get(key)
            .and_then(jfn_mpv::Node::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jfn_mpv::Node;

    #[test]
    fn scalar_properties_land_in_their_fields() {
        let mut d = StatsData::default();
        assert!(d.apply("video-codec", &PropertyValue::String("h264".into())));
        assert!(d.apply("frame-drop-count", &PropertyValue::Int(3)));
        assert!(d.apply("avsync", &PropertyValue::Double(-0.002)));
        assert_eq!(d.video_codec.as_deref(), Some("h264"));
        assert_eq!(d.dropped_frames_vo, Some(3));
        assert_eq!(d.avsync, Some(-0.002));
    }

    #[test]
    fn an_unchanged_value_reports_no_change() {
        let mut d = StatsData::default();
        assert!(d.apply("current-vo", &PropertyValue::String("gpu-next".into())));
        assert!(!d.apply("current-vo", &PropertyValue::String("gpu-next".into())));
    }

    #[test]
    fn an_unavailable_property_clears_its_field() {
        let mut d = StatsData::default();
        d.apply("audio-bitrate", &PropertyValue::Double(384_000.0));
        assert!(d.apply("audio-bitrate", &PropertyValue::None));
        assert_eq!(d.audio_bitrate, None);
        d.apply("video-codec", &PropertyValue::String("h264".into()));
        assert!(d.apply("video-codec", &PropertyValue::String(String::new())));
        assert_eq!(d.video_codec, None);
    }

    #[test]
    fn video_params_node_fans_out_to_four_fields() {
        let n = Node::Map(vec![
            ("w".into(), Node::Int(1920)),
            ("h".into(), Node::Int(1080)),
            ("pixelformat".into(), Node::String("yuv420p".into())),
            ("colormatrix".into(), Node::String("bt.709".into())),
        ]);
        let mut d = StatsData::default();
        assert!(d.apply("video-params", &PropertyValue::Node(n.clone())));
        assert_eq!((d.width, d.height), (Some(1920), Some(1080)));
        assert_eq!(d.pixel_format.as_deref(), Some("yuv420p"));
        assert_eq!(d.color_matrix.as_deref(), Some("bt.709"));
        // Same node again: nothing moved, so nothing to push.
        assert!(!d.apply("video-params", &PropertyValue::Node(n)));
    }

    #[test]
    fn audio_params_node_fans_out() {
        let n = Node::Map(vec![
            ("format".into(), Node::String("floatp".into())),
            ("samplerate".into(), Node::Int(48_000)),
            ("channel-count".into(), Node::Int(6)),
        ]);
        let mut d = StatsData::default();
        assert!(d.apply("audio-params", &PropertyValue::Node(n)));
        assert_eq!(d.audio_format.as_deref(), Some("floatp"));
        assert_eq!(d.audio_sample_rate, Some(48_000));
        assert_eq!(d.audio_channels, Some(6));
    }

    #[test]
    fn absent_fields_are_omitted_from_the_json() {
        let mut d = StatsData::default();
        d.apply("video-codec", &PropertyValue::String("hevc".into()));
        let json = jfn_js_json::to_js_json(&d).unwrap_or_default();
        assert_eq!(json, r#"{"videoCodec":"hevc"}"#);
    }

    #[test]
    fn an_unknown_property_name_is_ignored() {
        let mut d = StatsData::default();
        assert!(!d.apply("nonesuch", &PropertyValue::Int(1)));
        assert_eq!(d, StatsData::default());
    }
}
