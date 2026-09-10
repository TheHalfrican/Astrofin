//! Double- and triple-click detection for the platforms whose native event
//! stream carries no click count.
//!
//! Chromium synthesises the DOM `dblclick` event from the `click_count` field
//! of a `CefMouseEvent`, so a host that always sends `1` can never produce one.
//! Windows (no `CS_DBLCLKS`), X11 and Wayland are all in that position; this is
//! the arithmetic that recovers the count from the press stream alone. It is
//! pure — the caller supplies the clock — so every rule below is testable.

use std::os::raw::c_int;

/// Chromium's own multi-click thresholds, so a double click here means the
/// same thing it means in a native browser window:
///
/// * a press counts as a repeat only if it lands **less than
///   [`DOUBLE_CLICK_MS`] after** the previous press,
/// * **no more than [`SLOP_PX`]** away from it in *each* axis, and
/// * with the **same button** — any other button starts over.
///
/// The count saturates at [`MAX_CLICKS`]; CEF has no meaning for a fourth.
const DOUBLE_CLICK_MS: u64 = 500;
/// Per-axis movement a repeat press may drift — see [`DOUBLE_CLICK_MS`].
const SLOP_PX: i32 = 4;
/// Highest click count CEF is given — see [`DOUBLE_CLICK_MS`].
pub(crate) const MAX_CLICKS: c_int = 3;

/// The press a following press is compared against.
#[derive(Debug, Clone, Copy)]
struct Press {
    button: u32,
    x: i32,
    y: i32,
    at_ms: u64,
    count: c_int,
}

/// Running click count for one pointer.
#[derive(Debug, Default)]
pub(crate) struct ClickCounter {
    last: Option<Press>,
}

impl ClickCounter {
    /// An empty counter, whose [`last`](Self::last) is a single click.
    pub(crate) const fn new() -> Self {
        Self { last: None }
    }

    /// Records a press at `now_ms` and returns the click count CEF should see.
    pub(crate) fn press(&mut self, button: u32, x: i32, y: i32, now_ms: u64) -> c_int {
        let slop = SLOP_PX.unsigned_abs();
        let count = match self.last {
            Some(p)
                if p.button == button
                    && now_ms.saturating_sub(p.at_ms) < DOUBLE_CLICK_MS
                    && x.abs_diff(p.x) <= slop
                    && y.abs_diff(p.y) <= slop =>
            {
                (p.count + 1).min(MAX_CLICKS)
            }
            _ => 1,
        };
        self.last = Some(Press {
            button,
            x,
            y,
            at_ms: now_ms,
            count,
        });
        count
    }

    /// The count of the most recent press, for the release that belongs to it:
    /// CEF wants the same `click_count` on the up event as on the down event.
    pub(crate) fn last(&self) -> c_int {
        self.last.map_or(1, |p| p.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: u32 = crate::buttons::BTN_LEFT;
    const RIGHT: u32 = crate::buttons::BTN_RIGHT;

    #[test]
    fn a_second_press_within_the_window_and_the_slop_is_a_double_click() {
        let mut c = ClickCounter::new();
        assert_eq!(c.press(LEFT, 10, 20, 1_000), 1);
        assert_eq!(c.press(LEFT, 10, 20, 1_100), 2);
    }

    #[test]
    fn a_third_press_is_a_triple_click_and_a_fourth_stays_capped() {
        let mut c = ClickCounter::new();
        assert_eq!(c.press(LEFT, 0, 0, 0), 1);
        assert_eq!(c.press(LEFT, 0, 0, 100), 2);
        assert_eq!(c.press(LEFT, 0, 0, 200), 3);
        assert_eq!(c.press(LEFT, 0, 0, 300), MAX_CLICKS, "capped at a triple");
        assert_eq!(c.press(LEFT, 0, 0, 400), MAX_CLICKS);
    }

    #[test]
    fn a_press_after_the_double_click_window_starts_a_new_count() {
        let mut c = ClickCounter::new();
        assert_eq!(c.press(LEFT, 0, 0, 0), 1);
        assert_eq!(c.press(LEFT, 0, 0, DOUBLE_CLICK_MS - 1), 2, "just inside");
        // A full window after that second press: too late to be a triple.
        assert_eq!(c.press(LEFT, 0, 0, DOUBLE_CLICK_MS * 2 - 1), 1);
    }

    #[test]
    fn a_press_beyond_the_slop_starts_a_new_count() {
        let mut c = ClickCounter::new();
        assert_eq!(c.press(LEFT, 100, 100, 0), 1);
        assert_eq!(
            c.press(LEFT, 100 + SLOP_PX, 100, 10),
            2,
            "slop is inclusive"
        );
        assert_eq!(
            c.press(LEFT, 100, 100 + SLOP_PX + 1, 20),
            1,
            "one px too far"
        );
    }

    #[test]
    fn a_different_button_starts_a_new_count() {
        let mut c = ClickCounter::new();
        assert_eq!(c.press(LEFT, 0, 0, 0), 1);
        assert_eq!(c.press(RIGHT, 0, 0, 10), 1);
        assert_eq!(c.press(LEFT, 0, 0, 20), 1);
    }

    #[test]
    fn last_reports_the_count_of_the_most_recent_press() {
        let mut c = ClickCounter::new();
        assert_eq!(c.last(), 1, "a fresh counter is a single click");
        c.press(LEFT, 0, 0, 0);
        assert_eq!(c.last(), 1);
        c.press(LEFT, 0, 0, 10);
        assert_eq!(c.last(), 2, "the release carries the press's own count");
        c.press(RIGHT, 0, 0, 20);
        assert_eq!(c.last(), 1);
    }
}
