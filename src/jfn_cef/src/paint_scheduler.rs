use cef::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, Ordering};

use cef::{
    CefString, Frame, ImplFrame, ImplTask, Task, ThreadId, WrapTask, post_delayed_task, post_task,
    wrap_task,
};

use crate::client::{Inner, now_ns};

const BOOST_MULTIPLIER: i32 = 2;
const INVALIDATE_TICK_LIMIT: i32 = 1000;
const SKIP_PAINTS_AFTER_RESIZE: i32 = 1;

// After each window resize, keep producing compositor frames until
// `CefLayer::noteStableSize` calls `window.__cefStopRaf`.
const JS_PAINT_NUDGE: &str = r#"
(function () {
    console.debug('CEF paint nudge installed');
    var running = false;
    var stop = false;
    function tick() {
        if (stop) {
            stop = false;
            running = false;
            return;
        }
        requestAnimationFrame(tick);
    }
    window.addEventListener('resize', function () {
        stop = false;
        if (!running) {
            running = true;
            requestAnimationFrame(tick);
        }
    });
    window.__cefStopRaf = function () { stop = true; };
})();
"#;

struct PaintState {
    saved_frame_rate: AtomicI32,
    resize_gen: AtomicU64,
    invalidate_running: AtomicBool,
    invalidate_stop: AtomicBool,
    invalidate_tick_count: AtomicI32,
    last_paint_gen: AtomicU64,
    paints_since_resize: AtomicI32,
    pump_paint_count: AtomicI32,
    last_skip_reset_ns: AtomicI64,
}

impl PaintState {
    fn new() -> Self {
        Self {
            saved_frame_rate: AtomicI32::new(0),
            resize_gen: AtomicU64::new(0),
            invalidate_running: AtomicBool::new(false),
            invalidate_stop: AtomicBool::new(false),
            invalidate_tick_count: AtomicI32::new(0),
            last_paint_gen: AtomicU64::new(0),
            paints_since_resize: AtomicI32::new(SKIP_PAINTS_AFTER_RESIZE),
            pump_paint_count: AtomicI32::new(0),
            last_skip_reset_ns: AtomicI64::new(0),
        }
    }

    fn begin_resize(&self) {
        self.resize_gen.fetch_add(1, Ordering::AcqRel);
    }

    fn stop_invalidate_loop(&self) {
        self.invalidate_stop.store(true, Ordering::Release);
    }

    fn start_invalidate_loop(&self) -> bool {
        self.invalidate_stop.store(false, Ordering::Release);
        if self
            .invalidate_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        self.invalidate_tick_count.store(0, Ordering::Release);
        true
    }

    fn update_boost_saved_frame_rate(&self, target: i32) -> bool {
        if self.saved_frame_rate.load(Ordering::Acquire) == 0 {
            return false;
        }
        self.saved_frame_rate.store(target, Ordering::Release);
        true
    }
}

#[derive(Debug)]
pub(crate) struct PaintMode {
    shared_textures: bool,
}

impl PaintMode {
    pub(crate) fn new(shared_textures: bool) -> Self {
        Self { shared_textures }
    }

    pub(crate) fn shared_textures(&self) -> bool {
        self.shared_textures
    }

    pub(crate) fn make_scheduler(&self) -> PaintScheduler {
        PaintScheduler::new(self.shared_textures)
    }
}

#[derive(Clone)]
pub(crate) struct PaintScheduler {
    mode: Arc<dyn PaintSchedulerMode>,
}

trait PaintSchedulerMode: Send + Sync {
    fn before_resize(&self) {}
    fn after_resize(&self, _scheduler: PaintScheduler, _inner: &Arc<Inner>) {}
    fn before_close(&self) {}
    fn refresh_rate_changed(&self, _target: i32) -> bool {
        false
    }
    fn should_present_paint(&self, _inner: &Inner) -> bool {
        true
    }
    fn kick_task(&self, _scheduler: PaintScheduler, _inner: &Arc<Inner>) {}
    fn tick_task(&self, _scheduler: PaintScheduler, _inner: &Arc<Inner>) {}
}

impl PaintScheduler {
    fn new(shared_textures: bool) -> Self {
        let mode: Arc<dyn PaintSchedulerMode> = if shared_textures {
            Arc::new(ActivePaintScheduler {
                state: PaintState::new(),
            })
        } else {
            Arc::new(PassivePaintScheduler)
        };
        Self { mode }
    }

    pub(crate) fn on_context_created(shared_textures: bool, frame: &Frame) {
        if !shared_textures {
            return;
        }
        let code = CefString::from(JS_PAINT_NUDGE);
        let url_uf = frame.url();
        let url = CefString::from(&url_uf);
        frame.execute_java_script(Some(&code), Some(&url), 0);
    }

    pub(crate) fn during_resize<R>(&self, inner: &Arc<Inner>, resize: impl FnOnce() -> R) -> R {
        self.mode.before_resize();
        let result = resize();
        self.mode.after_resize(self.clone(), inner);
        result
    }

    pub(crate) fn before_close(&self) {
        self.mode.before_close();
    }

    pub(crate) fn refresh_rate_changed(&self, target: i32) -> bool {
        self.mode.refresh_rate_changed(target)
    }

    pub(crate) fn should_present_paint(&self, inner: &Inner) -> bool {
        self.mode.should_present_paint(inner)
    }

    fn kick_task(&self, inner: &Arc<Inner>) {
        self.mode.kick_task(self.clone(), inner);
    }

    fn tick_task(&self, inner: &Arc<Inner>) {
        self.mode.tick_task(self.clone(), inner);
    }
}

struct PassivePaintScheduler;

impl PaintSchedulerMode for PassivePaintScheduler {}

struct ActivePaintScheduler {
    state: PaintState,
}

impl PaintSchedulerMode for ActivePaintScheduler {
    fn before_resize(&self) {
        self.state.begin_resize();
    }

    fn after_resize(&self, scheduler: PaintScheduler, inner: &Arc<Inner>) {
        inner.invalidate_view();
        start_invalidate_loop(scheduler, &self.state, inner);
    }

    fn before_close(&self) {
        self.state.stop_invalidate_loop();
    }

    fn refresh_rate_changed(&self, target: i32) -> bool {
        self.state.update_boost_saved_frame_rate(target)
    }

    fn should_present_paint(&self, inner: &Inner) -> bool {
        active_should_present_paint(&self.state, inner)
    }

    fn kick_task(&self, scheduler: PaintScheduler, inner: &Arc<Inner>) {
        active_kick_apply(scheduler, &self.state, inner);
    }

    fn tick_task(&self, scheduler: PaintScheduler, inner: &Arc<Inner>) {
        active_invalidate_tick(scheduler, &self.state, inner);
    }
}

fn start_invalidate_loop(scheduler: PaintScheduler, state: &PaintState, inner: &Arc<Inner>) {
    if !state.start_invalidate_loop() {
        return;
    }
    let next = Arc::clone(inner);
    let mut task = KickTask::new(scheduler, next);
    let _ = post_task(ThreadId::UI, Some(&mut task));
}

fn active_kick_apply(scheduler: PaintScheduler, state: &PaintState, inner: &Arc<Inner>) {
    // Boost CEF compositor rate while the loop is live — JS rAF ties to
    // compositor rate, so this speeds up convergence to post-resize dims.
    let fps = inner.frame_rate.load(Ordering::Acquire);
    if inner.browser_alive() && fps > 0 && state.saved_frame_rate.load(Ordering::Acquire) == 0 {
        state.saved_frame_rate.store(fps, Ordering::Release);
        inner.set_frame_rate(boosted_frame_rate(fps));
    }
    active_invalidate_tick(scheduler, state, inner);
}

fn active_invalidate_tick(scheduler: PaintScheduler, state: &PaintState, inner: &Arc<Inner>) {
    if state.invalidate_tick_count.fetch_add(1, Ordering::AcqRel) + 1 > INVALIDATE_TICK_LIMIT {
        state.invalidate_stop.store(true, Ordering::Release);
    }
    if state.invalidate_stop.load(Ordering::Acquire) {
        let saved = state.saved_frame_rate.swap(0, Ordering::AcqRel);
        if inner.browser_alive() && saved > 0 {
            inner.set_frame_rate(saved);
        }
        state.invalidate_running.store(false, Ordering::Release);
        return;
    }
    if inner.browser_alive() {
        inner.invalidate_view();
        let external_bf = jfn_platform_abi::try_get()
            .and_then(|p| p.cef_host())
            .is_some_and(|h| h.external_begin_frame());
        if external_bf {
            inner.send_external_begin_frame();
        }
    }
    let fps = inner.frame_rate.load(Ordering::Acquire);
    if fps <= 0 {
        state.invalidate_running.store(false, Ordering::Release);
        return;
    }
    let delay_ms = tick_delay_ms(fps);
    let next = Arc::clone(inner);
    let mut task = TickTask::new(scheduler, next);
    let _ = post_delayed_task(ThreadId::UI, Some(&mut task), delay_ms);
}

/// Ticks are posted at 4x the display refresh so the compositor is nudged
/// more often than the boosted output rate (2x) — that keeps frame
/// production ahead of the present cadence during a resize. The result is
/// rounded to whole milliseconds and never drops to a zero delay, which
/// would spin TID_UI.
fn tick_delay_ms(fps: i32) -> i64 {
    let tick_hz = fps.saturating_mul(4);
    let delay_ms = ((1000.0 / f64::from(tick_hz)) + 0.5) as i64;
    delay_ms.max(1)
}

/// Nanoseconds per display frame, falling back to 60 Hz when the display
/// rate is unknown or nonsensical (mpv reports 0.0 before the first VO).
fn frame_period_ns(hz: f64) -> i64 {
    if hz > 0.0 {
        (1e9 / hz) as i64
    } else {
        16_666_667
    }
}

/// Whether the skip-counter reset is due again. A continuous drag bumps the
/// resize generation many times per second; resetting on every bump would
/// keep wiping the counter before any paint cleared the skip threshold, so
/// the reset is clamped to at most one per display frame.
fn skip_reset_is_due(now_ns_val: i64, last_reset_ns: i64, period_ns: i64) -> bool {
    now_ns_val - last_reset_ns >= period_ns
}

/// How many paints to pump after a resize before telling the host loop and
/// the page's rAF loop to stop. Zero disables the stop signal entirely,
/// which is what an unknown frame rate must produce.
fn pump_target(fps: i32) -> i32 {
    if fps > 0 { 1 + fps } else { 0 }
}

/// The first [`SKIP_PAINTS_AFTER_RESIZE`] paints after a resize carry the
/// pre-resize dimensions, so they are dropped rather than presented.
fn should_present(paints_since_resize: i32) -> bool {
    paints_since_resize > SKIP_PAINTS_AFTER_RESIZE
}

/// The compositor rate to run at while the invalidate loop is live. JS rAF
/// ties to the compositor rate, so boosting speeds up convergence to the
/// post-resize dimensions.
fn boosted_frame_rate(fps: i32) -> i32 {
    fps.saturating_mul(BOOST_MULTIPLIER)
}

fn active_should_present_paint(state: &PaintState, inner: &Inner) -> bool {
    let cur_gen = state.resize_gen.load(Ordering::Acquire);
    let last_gen = state.last_paint_gen.load(Ordering::Acquire);
    if cur_gen != last_gen {
        state.last_paint_gen.store(cur_gen, Ordering::Release);
        // Rate-clamp the skip-counter reset. Continuous drag bumps gen
        // many times per second; resetting on every bump would keep
        // wiping the counter before any paint clears the skip threshold.
        let now_ns_val = now_ns();
        let hz = jfn_playback::ingest_driver::jfn_playback_display_hz();
        let period_ns = frame_period_ns(hz);
        if skip_reset_is_due(
            now_ns_val,
            state.last_skip_reset_ns.load(Ordering::Acquire),
            period_ns,
        ) {
            state
                .last_skip_reset_ns
                .store(now_ns_val, Ordering::Release);
            let fps = inner.frame_rate.load(Ordering::Acquire);
            state
                .pump_paint_count
                .store(pump_target(fps), Ordering::Release);
            state.paints_since_resize.store(0, Ordering::Release);
        }
    }
    let count = state.paints_since_resize.fetch_add(1, Ordering::AcqRel) + 1;
    let pump = state.pump_paint_count.load(Ordering::Acquire);
    let present = should_present(count);
    if pump > 0 && count == pump {
        // Pumped enough frames — signal stop to host Invalidate loop and
        // renderer's rAF loop. Counter remains past pump so subsequent
        // paints don't re-fire.
        state.invalidate_stop.store(true, Ordering::Release);
        inner.exec_js("window.__cefStopRaf && window.__cefStopRaf();");
    }
    present
}

wrap_task! {
    struct KickTask {
        scheduler: PaintScheduler,
        inner: Arc<Inner>,
    }
    impl Task {
        fn execute(&self) {
            self.scheduler.kick_task(&self.inner);
        }
    }
}

wrap_task! {
    struct TickTask {
        scheduler: PaintScheduler,
        inner: Arc<Inner>,
    }
    impl Task {
        fn execute(&self) {
            self.scheduler.tick_task(&self.inner);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn detached() -> Arc<Inner> {
        Inner::new_detached()
    }

    fn active() -> ActivePaintScheduler {
        ActivePaintScheduler {
            state: PaintState::new(),
        }
    }

    // --- pure pacing arithmetic -------------------------------------------

    #[test]
    fn tick_delay_is_a_quarter_of_the_frame_interval() {
        // 60 Hz -> 240 ticks/s -> 4.16ms -> 4ms after the +0.5 rounding.
        assert_eq!(tick_delay_ms(60), 4);
        assert_eq!(tick_delay_ms(30), 8);
        assert_eq!(tick_delay_ms(120), 2);
    }

    #[test]
    fn tick_delay_never_reaches_zero() {
        // A very high refresh rate must not turn the delayed task into a
        // busy loop on TID_UI.
        assert_eq!(tick_delay_ms(1000), 1);
        assert_eq!(tick_delay_ms(i32::MAX), 1);
    }

    #[test]
    fn frame_period_falls_back_to_sixty_hertz_for_an_unknown_rate() {
        assert_eq!(frame_period_ns(0.0), 16_666_667);
        assert_eq!(frame_period_ns(-60.0), 16_666_667);
        assert_eq!(frame_period_ns(f64::NAN), 16_666_667);
        assert_eq!(frame_period_ns(60.0), 16_666_666);
        assert_eq!(frame_period_ns(120.0), 8_333_333);
    }

    #[test]
    fn pump_target_is_disabled_when_the_frame_rate_is_unknown() {
        assert_eq!(pump_target(0), 0);
        assert_eq!(pump_target(-1), 0);
        assert_eq!(pump_target(60), 61);
    }

    #[test]
    fn the_first_paint_after_a_resize_is_dropped() {
        assert!(!should_present(0));
        assert!(!should_present(SKIP_PAINTS_AFTER_RESIZE));
        assert!(should_present(SKIP_PAINTS_AFTER_RESIZE + 1));
    }

    #[test]
    fn boosted_frame_rate_doubles_and_saturates() {
        assert_eq!(boosted_frame_rate(60), 120);
        assert_eq!(boosted_frame_rate(i32::MAX), i32::MAX);
    }

    // --- PaintState --------------------------------------------------------

    #[test]
    fn a_fresh_paint_state_starts_with_the_skip_budget_already_spent() {
        let s = PaintState::new();
        assert_eq!(
            s.paints_since_resize.load(Ordering::Acquire),
            SKIP_PAINTS_AFTER_RESIZE
        );
        assert_eq!(s.resize_gen.load(Ordering::Acquire), 0);
        assert!(!s.invalidate_running.load(Ordering::Acquire));
    }

    #[test]
    fn begin_resize_bumps_the_generation_once_per_call() {
        let s = PaintState::new();
        s.begin_resize();
        s.begin_resize();
        assert_eq!(s.resize_gen.load(Ordering::Acquire), 2);
    }

    #[test]
    fn start_invalidate_loop_admits_one_runner_at_a_time() {
        let s = PaintState::new();
        assert!(s.start_invalidate_loop(), "the first caller owns the loop");
        assert!(!s.start_invalidate_loop(), "a second caller is refused");
        assert_eq!(s.invalidate_tick_count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn start_invalidate_loop_clears_a_pending_stop() {
        let s = PaintState::new();
        s.stop_invalidate_loop();
        assert!(s.invalidate_stop.load(Ordering::Acquire));
        assert!(s.start_invalidate_loop());
        assert!(!s.invalidate_stop.load(Ordering::Acquire));
    }

    #[test]
    fn stop_invalidate_loop_only_raises_the_flag() {
        let s = PaintState::new();
        assert!(s.start_invalidate_loop());
        s.stop_invalidate_loop();
        assert!(s.invalidate_stop.load(Ordering::Acquire));
        // The runner clears "running" itself on its next tick; stopping does
        // not release the slot on its own.
        assert!(s.invalidate_running.load(Ordering::Acquire));
    }

    #[test]
    fn update_boost_saved_frame_rate_is_ignored_while_no_boost_is_held() {
        let s = PaintState::new();
        assert!(!s.update_boost_saved_frame_rate(120));
        assert_eq!(s.saved_frame_rate.load(Ordering::Acquire), 0);
    }

    #[test]
    fn update_boost_saved_frame_rate_retargets_a_held_boost() {
        let s = PaintState::new();
        s.saved_frame_rate.store(60, Ordering::Release);
        assert!(s.update_boost_saved_frame_rate(144));
        assert_eq!(s.saved_frame_rate.load(Ordering::Acquire), 144);
    }

    // --- PaintMode / PaintScheduler ---------------------------------------

    #[test]
    fn paint_mode_reports_the_shared_texture_flag_it_was_built_with() {
        assert!(PaintMode::new(true).shared_textures());
        assert!(!PaintMode::new(false).shared_textures());
    }

    #[test]
    fn make_scheduler_is_passive_without_shared_textures() {
        let inner = detached();
        // The passive scheduler presents every paint and never claims a
        // refresh-rate change.
        let passive = PaintMode::new(false).make_scheduler();
        assert!(passive.should_present_paint(&inner));
        assert!(!passive.refresh_rate_changed(120));

        // The active scheduler is a different mode: it holds paint state and
        // can claim a refresh-rate change once a boost is in flight.
        let act = PaintMode::new(true).make_scheduler();
        assert!(act.should_present_paint(&inner));
    }

    #[test]
    fn during_resize_returns_the_closure_result_and_runs_it_once() {
        let inner = detached();
        let scheduler = PaintScheduler::new(false);
        let mut runs = 0;
        let out = scheduler.during_resize(&inner, || {
            runs += 1;
            "resized"
        });
        assert_eq!(out, "resized");
        assert_eq!(runs, 1);
    }

    #[test]
    fn before_close_on_a_passive_scheduler_is_a_no_op() {
        // Nothing to stop; the call must simply not panic.
        PaintScheduler::new(false).before_close();
    }

    #[test]
    fn refresh_rate_changed_is_never_claimed_by_the_passive_scheduler() {
        let scheduler = PaintScheduler::new(false);
        assert!(!scheduler.refresh_rate_changed(60));
        assert!(!scheduler.refresh_rate_changed(0));
    }

    // --- ActivePaintScheduler mode methods --------------------------------

    #[test]
    fn active_before_resize_bumps_the_resize_generation() {
        let a = active();
        a.before_resize();
        assert_eq!(a.state.resize_gen.load(Ordering::Acquire), 1);
    }

    #[test]
    fn active_after_resize_does_not_start_a_second_invalidate_loop() {
        let a = active();
        let inner = detached();
        // Claim the loop first, so after_resize finds it running and posts no
        // second kick task.
        assert!(a.state.start_invalidate_loop());
        a.after_resize(PaintScheduler::new(true), &inner);
        assert!(a.state.invalidate_running.load(Ordering::Acquire));
        assert_eq!(a.state.invalidate_tick_count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn active_before_close_asks_the_invalidate_loop_to_stop() {
        let a = active();
        assert!(a.state.start_invalidate_loop());
        a.before_close();
        assert!(a.state.invalidate_stop.load(Ordering::Acquire));
    }

    #[test]
    fn active_refresh_rate_changed_only_retargets_a_held_boost() {
        let a = active();
        assert!(!a.refresh_rate_changed(120));
        a.state.saved_frame_rate.store(60, Ordering::Release);
        assert!(a.refresh_rate_changed(120));
        assert_eq!(a.state.saved_frame_rate.load(Ordering::Acquire), 120);
    }

    #[test]
    fn skip_reset_is_due_only_once_per_display_frame() {
        let period = frame_period_ns(60.0);
        assert!(skip_reset_is_due(period, 0, period), "a full frame elapsed");
        assert!(skip_reset_is_due(period * 3, 0, period));
        assert!(
            !skip_reset_is_due(period - 1, 0, period),
            "a drag bumping the generation mid-frame must not reset again"
        );
        assert!(!skip_reset_is_due(0, 0, period));
    }

    #[test]
    fn active_should_present_paint_drops_the_first_paint_of_a_new_resize() {
        let a = active();
        let inner = detached();
        // The reset is rate-clamped to one display frame; stamp the last one
        // a second back so this resize definitely clears the clamp.
        a.state
            .last_skip_reset_ns
            .store(now_ns() - 1_000_000_000, Ordering::Release);
        a.state.begin_resize();
        // The first paint of the new generation resets the skip counter, so it
        // is itself skipped; the next one presents.
        assert!(!a.should_present_paint(&inner));
        assert!(a.should_present_paint(&inner));
        assert!(a.should_present_paint(&inner));
    }

    #[test]
    fn active_should_present_paint_keeps_presenting_through_a_mid_frame_resize() {
        let a = active();
        let inner = detached();
        // A reset stamped later than any reading this test can take keeps the
        // clamp shut, so the drag does not wipe the already-spent skip budget.
        a.state
            .last_skip_reset_ns
            .store(now_ns() + 1_000_000_000, Ordering::Release);
        a.state.begin_resize();
        assert!(a.should_present_paint(&inner));
    }

    #[test]
    fn active_should_present_paint_presents_while_the_generation_is_unchanged() {
        let a = active();
        let inner = detached();
        // No resize since construction: the skip budget is already spent.
        assert!(a.should_present_paint(&inner));
    }

    #[test]
    fn active_tick_task_releases_the_loop_slot_once_stopped() {
        let a = active();
        let inner = detached();
        assert!(a.state.start_invalidate_loop());
        a.state.saved_frame_rate.store(60, Ordering::Release);
        a.state.stop_invalidate_loop();
        a.tick_task(PaintScheduler::new(true), &inner);
        assert!(!a.state.invalidate_running.load(Ordering::Acquire));
        assert_eq!(
            a.state.saved_frame_rate.load(Ordering::Acquire),
            0,
            "the boost is handed back on the stopping tick"
        );
    }

    #[test]
    fn active_tick_task_stops_itself_at_the_tick_limit() {
        let a = active();
        let inner = detached();
        assert!(a.state.start_invalidate_loop());
        a.state
            .invalidate_tick_count
            .store(INVALIDATE_TICK_LIMIT, Ordering::Release);
        a.tick_task(PaintScheduler::new(true), &inner);
        assert!(a.state.invalidate_stop.load(Ordering::Acquire));
        assert!(!a.state.invalidate_running.load(Ordering::Acquire));
    }

    #[test]
    fn active_kick_task_takes_no_boost_without_a_live_browser() {
        let a = active();
        let inner = detached();
        inner.frame_rate.store(60, Ordering::Release);
        // The kick decides on the boost and then falls straight into a tick.
        // Pre-stopping the loop makes that tick unwind instead of posting a
        // delayed task into a CEF this process never initialised.
        a.state.stop_invalidate_loop();
        a.kick_task(PaintScheduler::new(true), &inner);
        assert_eq!(
            a.state.saved_frame_rate.load(Ordering::Acquire),
            0,
            "nothing to boost while the browser is gone"
        );
    }
}
