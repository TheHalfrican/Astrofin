//! A-B repeat loop: mpv's `ab-loop-a` / `ab-loop-b`, mirrored to the OSD.
//!
//! mpv owns the loop. The web layer asks for two points, this module writes
//! them as async property sets, and the *only* thing that ever changes the
//! state JS draws is mpv reporting the properties back. Nothing here polices
//! the playhead: once both points are set mpv seeks to `a` when playback
//! passes `b`, and a manual seek past `b` deliberately does not loop
//! (`DOCS/man/options.rst`, `--ab-loop-a`). With either point unset — mpv
//! spells that `"no"` — looping is off.
//!
//! The two observations get their own `reply_userdata`, like
//! [`crate::stats`]: they are a UI concern with no bearing on the playback
//! state machine, and routing them through `PlaybackSnapshot` would put two
//! more fields on a struct cloned for every position tick.

use parking_lot::Mutex;
use std::sync::OnceLock;

use jfn_mpv::{Node, PropertyValue};

/// `reply_userdata` shared by both A-B loop observations. One id for the
/// pair; the ingest thread dispatches on the property name.
pub const AB_LOOP_OBSERVE_ID: u64 = 101;

/// Both loop points, in seconds. `None` is mpv's `"no"` — that end is unset.
type Points = (Option<f64>, Option<f64>);

struct Inner {
    a: Option<f64>,
    b: Option<f64>,
    /// The pair the last push carried. Seeded with the pair mpv starts in so
    /// the initial `no`/`no` observations do not push a redundant clear.
    pushed: Points,
}

fn inner() -> &'static Mutex<Inner> {
    static SLOT: OnceLock<Mutex<Inner>> = OnceLock::new();
    SLOT.get_or_init(|| {
        Mutex::new(Inner {
            a: None,
            b: None,
            pushed: (None, None),
        })
    })
}

// ---------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------

/// Set both loop points from the web layer's milliseconds, where a negative
/// value means "unset". Both ends are always written, so a half-set pair can
/// never survive a call.
pub fn jfn_playback_set_ab_loop_ms(a_ms: i64, b_ms: i64) {
    let (a, b) = points_from_ms(a_ms, b_ms);
    jfn_mpv::api::jfn_mpv_set_ab_loop(a, b);
}

/// Drop both loop points. mpv keeps `ab-loop-a` / `ab-loop-b` across files,
/// so a loop set on one episode would otherwise apply to the next; the web
/// layer clears on every load and stop.
pub fn jfn_playback_clear_ab_loop() {
    jfn_mpv::api::jfn_mpv_set_ab_loop(None, None);
}

/// Milliseconds from JS to mpv's seconds. Anything negative is the "unset"
/// sentinel the web layer sends for a point it does not have.
#[must_use]
pub fn points_from_ms(a_ms: i64, b_ms: i64) -> Points {
    (point_from_ms(a_ms), point_from_ms(b_ms))
}

fn point_from_ms(ms: i64) -> Option<f64> {
    if ms < 0 {
        None
    } else {
        Some(ms as f64 / 1000.0)
    }
}

// ---------------------------------------------------------------------
// Observing
// ---------------------------------------------------------------------

/// Fold one observed loop point in and push the pair to JS when it moved.
/// Called from the mpv ingest thread; nothing here reads mpv.
pub(crate) fn on_property(name: &str, value: &PropertyValue) {
    let points = {
        let mut g = inner().lock();
        match name {
            "ab-loop-a" => g.a = point_from_value(value),
            "ab-loop-b" => g.b = point_from_value(value),
            _ => return,
        }
        let points = (g.a, g.b);
        if g.pushed == points {
            return;
        }
        g.pushed = points;
        points
    };
    tracing::info!(
        target: "mpv",
        "ab-loop: a={} b={}",
        fmt_point(points.0),
        fmt_point(points.1)
    );
    crate::exec_js::call(&push_js(points.0, points.1));
}

/// mpv reports an unset point as the string `"no"`; a set one is a time.
/// Observed as `MPV_FORMAT_NODE`, so both arrive under [`PropertyValue::Node`]
/// — the scalar arms are there for a caller that observed a narrower format.
fn point_from_value(v: &PropertyValue) -> Option<f64> {
    match v {
        PropertyValue::Double(d) | PropertyValue::Node(Node::Double(d)) if d.is_finite() => {
            Some(*d)
        }
        PropertyValue::Int(i) | PropertyValue::Node(Node::Int(i)) => Some(*i as f64),
        _ => None,
    }
}

/// `null` for an unset point, otherwise the time in seconds as a JS number.
/// Only finite values reach this — see [`point_from_value`].
fn js_number(v: Option<f64>) -> String {
    v.map_or_else(|| "null".to_string(), |s| format!("{s}"))
}

/// Guarded on the JS side: the push can land before `ab-loop.js` has run.
fn push_js(a: Option<f64>, b: Option<f64>) -> String {
    format!(
        "window._nativeAbLoop && window._nativeAbLoop({}, {})",
        js_number(a),
        js_number(b)
    )
}

fn fmt_point(v: Option<f64>) -> String {
    v.map_or_else(|| "no".to_string(), |s| format!("{s:.3}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_milliseconds_are_the_unset_sentinel() {
        assert_eq!(points_from_ms(-1, -1), (None, None));
        assert_eq!(points_from_ms(1500, -1), (Some(1.5), None));
        assert_eq!(points_from_ms(-1, 1500), (None, Some(1.5)));
    }

    #[test]
    fn milliseconds_become_seconds() {
        assert_eq!(points_from_ms(0, 3250), (Some(0.0), Some(3.25)));
        assert_eq!(points_from_ms(83_000, 105_500), (Some(83.0), Some(105.5)));
    }

    #[test]
    fn mpvs_no_is_an_unset_point() {
        assert_eq!(point_from_value(&PropertyValue::String("no".into())), None);
        assert_eq!(
            point_from_value(&PropertyValue::Node(Node::String("no".into()))),
            None
        );
        assert_eq!(point_from_value(&PropertyValue::None), None);
    }

    #[test]
    fn a_time_comes_back_as_seconds() {
        assert_eq!(
            point_from_value(&PropertyValue::Node(Node::Double(12.5))),
            Some(12.5)
        );
        assert_eq!(point_from_value(&PropertyValue::Double(12.5)), Some(12.5));
        assert_eq!(
            point_from_value(&PropertyValue::Node(Node::Int(7))),
            Some(7.0)
        );
        // A non-finite time would serialize as `inf`, which is not JS.
        assert_eq!(point_from_value(&PropertyValue::Double(f64::NAN)), None);
        assert_eq!(
            point_from_value(&PropertyValue::Double(f64::INFINITY)),
            None
        );
    }

    #[test]
    fn the_push_carries_seconds_or_null() {
        assert_eq!(
            push_js(Some(1.5), Some(3.25)),
            "window._nativeAbLoop && window._nativeAbLoop(1.5, 3.25)"
        );
        assert_eq!(
            push_js(Some(83.0), None),
            "window._nativeAbLoop && window._nativeAbLoop(83, null)"
        );
        assert_eq!(
            push_js(None, None),
            "window._nativeAbLoop && window._nativeAbLoop(null, null)"
        );
    }

    #[test]
    fn milliseconds_round_trip_through_the_push() {
        let (a, b) = points_from_ms(83_000, 105_500);
        assert_eq!(
            push_js(a, b),
            "window._nativeAbLoop && window._nativeAbLoop(83, 105.5)"
        );
    }
}
