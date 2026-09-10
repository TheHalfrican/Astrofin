use std::sync::Arc;
use std::sync::atomic::Ordering;

use jfn_playback::ingest_driver::jfn_playback_display_hz;

use crate::client_logic::{
    ResizeAction, frame_period_ns, frame_rate_usable, refresh_target, resize_action,
};

use super::{Inner, now_ns, platform_ops, tasks};

impl Inner {
    pub(crate) fn set_frame_rate(&self, hz: i32) {
        if !frame_rate_usable(hz) || !self.browser_alive() {
            return;
        }
        self.cef_set_windowless_frame_rate(hz);
        self.current_frame_rate.store(hz, Ordering::Release);
    }

    pub(super) fn apply_pending_resize(self: &Arc<Self>) {
        self.resize_scheduled.store(false, Ordering::Release);
        if !self.browser_alive() {
            return;
        }
        let now = now_ns();
        self.last_was_resized_ns.store(now, Ordering::Release);
        self.paint_scheduler.during_resize(self, || {
            self.notify_screen_info_changed();
            self.cef_was_resized();
        });
    }

    pub(super) fn resize(self: &Arc<Self>, w: i32, h: i32, pw: i32, ph: i32) {
        self.width.store(w, Ordering::Release);
        self.height.store(h, Ordering::Release);
        self.physical_w.store(pw, Ordering::Release);
        self.physical_h.store(ph, Ordering::Release);

        // Wayland viewport must update on every configure (not debounced) or
        // src/dst go stale.
        let surface = self.surface_handle();
        if !surface.is_none()
            && let Some(p) = platform_ops::ops()
        {
            p.surface_resize(
                surface,
                platform_ops::SurfaceSize {
                    logical_w: w,
                    logical_h: h,
                    physical_w: pw,
                    physical_h: ph,
                },
            );
        }

        if !self.browser_alive() {
            return;
        }

        let now = now_ns();
        let period_ns = frame_period_ns(jfn_playback_display_hz());
        let last = self.last_was_resized_ns.load(Ordering::Acquire);
        let action = resize_action(now, last, period_ns);
        self.paint_scheduler.during_resize(self, || match action {
            ResizeAction::Now => {
                self.last_was_resized_ns.store(now, Ordering::Release);
                self.notify_screen_info_changed();
                self.cef_was_resized();
            }
            ResizeAction::After(delay_ms) => {
                if self
                    .resize_scheduled
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    tasks::post_apply_resize(Arc::clone(self), delay_ms);
                }
            }
        });
    }

    pub(super) fn set_refresh_rate(self: &Arc<Self>, hz: f64) {
        let Some(target) = refresh_target(hz) else {
            return;
        };
        tasks::post_set_refresh(Arc::clone(self), target);
    }

    pub(super) fn apply_set_refresh(&self, target: i32) {
        self.frame_rate.store(target, Ordering::Release);
        if !self.paint_scheduler.refresh_rate_changed(target) {
            self.set_frame_rate(target);
        }
    }
}
