//! A-B repeat loop: mpv's `ab-loop-a` / `ab-loop-b`, mirrored to the OSD.
//!
//! mpv owns the loop, and mpv stamps the points. The web layer asks for one
//! of three *actions* — set A, set B, clear — never for a time; this module
//! turns the action into `no-osd ab-loop` (which mpv answers with its own
//! `get_current_time()`) or into a pair of `"no"` writes, and the *only*
//! thing that ever changes the state JS draws is mpv reporting the
//! properties back.
//!
//! The points have to come from mpv rather than from the UI's sampled
//! position because mpv disarms the loop at write time when the core's pts is
//! already past the `b` being written (`player/playloop.c`,
//! `update_ab_loop_clip`, run on every `ab-loop-a` / `ab-loop-b` set). The
//! position the OSD has lags the core by up to half a second, so a `b` taken
//! from it is always in the past and the loop never armed until something
//! seeked back before it.
//!
//! Nothing here polices the playhead: once both points are set mpv seeks to
//! `a` when playback passes `b`, and a manual seek past `b` deliberately does
//! not loop (`DOCS/man/options.rst`, `--ab-loop-a`). With either point unset
//! — mpv spells that `"no"` — looping is off.
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
pub type Points = (Option<f64>, Option<f64>);

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

/// The three things the web layer can ask for. Deliberately not "here are
/// the two times": see the module note on `update_ab_loop_clip`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    SetA,
    SetB,
    Clear,
}

impl Action {
    /// The wire spelling, as `playerAbLoop`'s single argument.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "set-a" => Some(Self::SetA),
            "set-b" => Some(Self::SetB),
            "clear" => Some(Self::Clear),
            _ => None,
        }
    }
}

/// What an [`Action`] does to mpv, given the pair mpv last reported.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    /// `no-osd ab-loop` — mpv stamps the next point from its own pts.
    Cycle,
    /// Write `"no"` to both ends.
    ClearBoth,
    /// The UI asked for a step mpv is not on. Nothing is written; the next
    /// observation push resyncs the UI.
    Ignore,
}

/// mpv's `ab-loop` command walks A → B → clear off *mpv's* state, so an
/// action is only run when the observed pair says mpv is on that step.
/// A stale UI (a click that raced a load, or the intermediate pair a clear
/// passes through) therefore cannot turn "set A" into "set B".
#[must_use]
pub fn command_for(action: Action, observed: Points) -> Command {
    match (action, observed) {
        (Action::SetA, (None, None)) | (Action::SetB, (Some(_), None)) => Command::Cycle,
        (Action::Clear, _) => Command::ClearBoth,
        _ => Command::Ignore,
    }
}

/// Run one A-B loop action against mpv. Unknown spellings are logged and
/// dropped rather than guessed at.
pub fn jfn_playback_ab_loop_action(action: &str) {
    let Some(act) = Action::parse(action) else {
        tracing::warn!(target: "mpv", "ab-loop: unknown action {action:?}");
        return;
    };
    let observed = {
        let g = inner().lock();
        (g.a, g.b)
    };
    match command_for(act, observed) {
        Command::Cycle => jfn_mpv::api::jfn_mpv_ab_loop_cycle(),
        Command::ClearBoth => jfn_mpv::api::jfn_mpv_clear_ab_loop(),
        Command::Ignore => tracing::debug!(
            target: "mpv",
            "ab-loop: ignoring {action:?}; mpv is at a={} b={}",
            fmt_point(observed.0),
            fmt_point(observed.1)
        ),
    }
}

/// Drop both loop points. mpv keeps `ab-loop-a` / `ab-loop-b` across files,
/// so a loop set on one episode would otherwise apply to the next; the CEF
/// layer clears on every load and stop.
pub fn jfn_playback_clear_ab_loop() {
    jfn_mpv::api::jfn_mpv_clear_ab_loop();
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
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::test_support;

    /// Put the process-wide pair back where mpv starts it.
    fn reset() {
        let mut g = inner().lock();
        g.a = None;
        g.b = None;
        g.pushed = (None, None);
    }

    fn observed() -> Points {
        let g = inner().lock();
        (g.a, g.b)
    }

    #[test]
    fn the_wire_spellings_are_the_three_actions() {
        assert_eq!(Action::parse("set-a"), Some(Action::SetA));
        assert_eq!(Action::parse("set-b"), Some(Action::SetB));
        assert_eq!(Action::parse("clear"), Some(Action::Clear));
        assert_eq!(Action::parse("setA"), None);
        assert_eq!(Action::parse(""), None);
    }

    #[test]
    fn setting_a_only_cycles_mpv_when_mpv_has_no_points() {
        assert_eq!(command_for(Action::SetA, (None, None)), Command::Cycle);
        // mpv is past that step: its `ab-loop` would stamp B, not A.
        assert_eq!(
            command_for(Action::SetA, (Some(12.0), None)),
            Command::Ignore
        );
        assert_eq!(
            command_for(Action::SetA, (Some(12.0), Some(20.0))),
            Command::Ignore
        );
        // The pair a clear passes through on its way to (no, no).
        assert_eq!(
            command_for(Action::SetA, (None, Some(20.0))),
            Command::Ignore
        );
    }

    #[test]
    fn setting_b_only_cycles_mpv_when_a_is_set_and_b_is_not() {
        assert_eq!(
            command_for(Action::SetB, (Some(12.0), None)),
            Command::Cycle
        );
        assert_eq!(command_for(Action::SetB, (None, None)), Command::Ignore);
        assert_eq!(
            command_for(Action::SetB, (Some(12.0), Some(20.0))),
            Command::Ignore
        );
        assert_eq!(
            command_for(Action::SetB, (None, Some(20.0))),
            Command::Ignore
        );
    }

    #[test]
    fn clearing_writes_both_ends_whatever_mpv_is_showing() {
        for observed in [
            (None, None),
            (Some(12.0), None),
            (None, Some(20.0)),
            (Some(12.0), Some(20.0)),
        ] {
            assert_eq!(command_for(Action::Clear, observed), Command::ClearBoth);
        }
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
    fn an_observed_pair_round_trips_through_the_push() {
        let (a, b) = (
            point_from_value(&PropertyValue::Node(Node::Double(83.0))),
            point_from_value(&PropertyValue::Node(Node::Double(105.5))),
        );
        assert_eq!(
            push_js(a, b),
            "window._nativeAbLoop && window._nativeAbLoop(83, 105.5)"
        );
    }

    #[test]
    fn an_unset_point_is_rendered_as_no() {
        assert_eq!(fmt_point(None), "no");
        assert_eq!(fmt_point(Some(83.25)), "83.250");
    }

    // ---- the process-wide pair ------------------------------------------

    #[test]
    fn an_observed_point_is_pushed_to_the_osd() {
        let _g = test_support::lock();
        reset();
        let js = test_support::JsRecorder::install();
        on_property("ab-loop-a", &PropertyValue::Node(Node::Double(12.5)));
        assert_eq!(
            js.only(),
            "window._nativeAbLoop && window._nativeAbLoop(12.5, null)"
        );
        on_property("ab-loop-b", &PropertyValue::Node(Node::Double(20.0)));
        assert_eq!(
            js.only(),
            "window._nativeAbLoop && window._nativeAbLoop(12.5, 20)"
        );
        assert_eq!(observed(), (Some(12.5), Some(20.0)));
        reset();
    }

    #[test]
    fn a_pair_that_did_not_move_is_not_pushed_again() {
        let _g = test_support::lock();
        reset();
        let js = test_support::JsRecorder::install();
        // mpv's boot value: both ends already unset, so nothing to redraw.
        on_property("ab-loop-a", &PropertyValue::Node(Node::String("no".into())));
        assert!(js.take().is_empty());
        on_property("ab-loop-a", &PropertyValue::Node(Node::Double(12.5)));
        assert!(!js.take().is_empty());
        on_property("ab-loop-a", &PropertyValue::Node(Node::Double(12.5)));
        assert!(js.take().is_empty());
        reset();
    }

    #[test]
    fn an_unrelated_property_never_touches_the_pair() {
        let _g = test_support::lock();
        reset();
        let js = test_support::JsRecorder::install();
        on_property("ab-loop-c", &PropertyValue::Node(Node::Double(3.0)));
        assert!(js.take().is_empty());
        assert_eq!(observed(), (None, None));
    }

    #[test]
    fn clearing_a_point_pushes_null_for_that_end() {
        let _g = test_support::lock();
        reset();
        let js = test_support::JsRecorder::install();
        on_property("ab-loop-a", &PropertyValue::Node(Node::Double(12.5)));
        js.take();
        on_property("ab-loop-a", &PropertyValue::Node(Node::String("no".into())));
        assert_eq!(
            js.only(),
            "window._nativeAbLoop && window._nativeAbLoop(null, null)"
        );
        reset();
    }

    #[test]
    fn an_unknown_action_is_dropped() {
        let _g = test_support::lock();
        reset();
        let js = test_support::JsRecorder::install();
        jfn_playback_ab_loop_action("set-c");
        jfn_playback_ab_loop_action("");
        assert_eq!(observed(), (None, None));
        assert!(js.take().is_empty());
    }

    #[test]
    fn an_action_never_writes_the_points_itself() {
        // mpv stamps both points and reports them back; nothing here may
        // pre-empt that, or the OSD would draw a loop mpv never armed.
        let _g = test_support::lock();
        reset();
        let js = test_support::JsRecorder::install();
        jfn_playback_ab_loop_action("set-a");
        jfn_playback_ab_loop_action("set-b");
        jfn_playback_ab_loop_action("clear");
        assert_eq!(observed(), (None, None));
        assert!(js.take().is_empty());
    }

    #[test]
    fn clearing_the_loop_leaves_the_observed_pair_to_mpv() {
        let _g = test_support::lock();
        reset();
        on_property("ab-loop-a", &PropertyValue::Node(Node::Double(12.5)));
        let js = test_support::JsRecorder::install();
        jfn_playback_clear_ab_loop();
        // The write went to mpv; the pair only moves when mpv reports back.
        assert_eq!(observed(), (Some(12.5), None));
        assert!(js.take().is_empty());
        reset();
    }
}
