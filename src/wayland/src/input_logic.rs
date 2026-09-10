//! Pure input mapping and arithmetic behind [`crate::input`].
//!
//! Everything here is input -> output over primitives: the keysym/button
//! tables, the CEF modifier mask, the per-frame scroll accumulator, and the
//! key-repeat timings. No seat, no proxy, no state — [`crate::input`] keeps
//! the protocol handlers and calls these for every decision they make.

use std::time::Duration;

use jfn_input::buttons::{
    BTN_BACK, BTN_EXTRA, BTN_FORWARD, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, BTN_SIDE,
};
use jfn_platform_abi::event_flags::{
    EVENTFLAG_ALT_DOWN, EVENTFLAG_CONTROL_DOWN, EVENTFLAG_LEFT_MOUSE_BUTTON,
    EVENTFLAG_MIDDLE_MOUSE_BUTTON, EVENTFLAG_RIGHT_MOUSE_BUTTON, EVENTFLAG_SHIFT_DOWN,
};

const XK_MENU: u32 = 0xff67;
const XK_F10: u32 = 0xffc7;

/// Both bindings X11 apps use for "open the context menu": the dedicated Menu
/// key, and Shift+F10.
pub(crate) fn is_context_menu_key(sym: u32, mods: u32) -> bool {
    sym == XK_MENU || (sym == XK_F10 && mods & EVENTFLAG_SHIFT_DOWN != 0)
}

/// The CEF modifier mask for one `wl_keyboard.modifiers` state. Only the three
/// modifiers CEF's key dispatch reads are carried; latched/locked bits (caps,
/// num) are deliberately not mapped.
pub(crate) fn cef_modifier_flags(shift: bool, ctrl: bool, alt: bool) -> u32 {
    let mut m = 0u32;
    if shift {
        m |= EVENTFLAG_SHIFT_DOWN;
    }
    if ctrl {
        m |= EVENTFLAG_CONTROL_DOWN;
    }
    if alt {
        m |= EVENTFLAG_ALT_DOWN;
    }
    m
}

/// The CEF button-held flag for an evdev button code, or `None` for a button
/// CEF has no modifier bit for (the side/history buttons).
pub(crate) fn mouse_button_flag(button: u32) -> Option<u32> {
    match button {
        BTN_LEFT => Some(EVENTFLAG_LEFT_MOUSE_BUTTON),
        BTN_RIGHT => Some(EVENTFLAG_RIGHT_MOUSE_BUTTON),
        BTN_MIDDLE => Some(EVENTFLAG_MIDDLE_MOUSE_BUTTON),
        _ => None,
    }
}

/// `Some(true)` for a forward-navigation button, `Some(false)` for a back one,
/// `None` for a button that is not history navigation at all. Mice disagree on
/// which pair they report, so both are accepted.
pub(crate) fn history_nav_forward(button: u32) -> Option<bool> {
    match button {
        BTN_EXTRA | BTN_FORWARD => Some(true),
        BTN_SIDE | BTN_BACK => Some(false),
        _ => None,
    }
}

/// One wheel step in the units CEF's scroll dispatch expects.
const SCROLL_STEP: f64 = 12.0;

/// One pointer frame's accumulated scroll, reduced to the integer deltas CEF
/// receives plus the sub-step remainder carried into the next frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollSteps {
    pub(crate) dx: i32,
    pub(crate) dy: i32,
    pub(crate) carry_x: f64,
    pub(crate) carry_y: f64,
}

/// Reduce one pointer frame's axis accumulation to CEF deltas.
///
/// A frame that carried any `value120` is authoritative and consumes the
/// continuous accumulation outright (both are the same wheel motion reported
/// twice). Otherwise the continuous deltas are scaled to steps and the
/// sub-step remainder is carried, because zeroing it rounds slow trackpad
/// scrolling away to nothing. Wayland's axis sign is the opposite of CEF's,
/// hence the negation on every path.
pub(crate) fn scroll_steps(
    have_v120: bool,
    v120_x: i32,
    v120_y: i32,
    accum_x: f64,
    accum_y: f64,
) -> ScrollSteps {
    if have_v120 {
        return ScrollSteps {
            dx: -v120_x,
            dy: -v120_y,
            carry_x: 0.0,
            carry_y: 0.0,
        };
    }
    if accum_x != 0.0 || accum_y != 0.0 {
        let scaled_x = -accum_x * SCROLL_STEP;
        let scaled_y = -accum_y * SCROLL_STEP;
        let dx = scaled_x as i32;
        let dy = scaled_y as i32;
        return ScrollSteps {
            dx,
            dy,
            carry_x: -(scaled_x - f64::from(dx)) / SCROLL_STEP,
            carry_y: -(scaled_y - f64::from(dy)) / SCROLL_STEP,
        };
    }
    ScrollSteps {
        dx: 0,
        dy: 0,
        carry_x: 0.0,
        carry_y: 0.0,
    }
}

/// Fold one `wl_pointer.axis`/`axis_stop` into the running accumulation. A
/// stop event ends the gesture, so it discards whatever was accumulated rather
/// than adding to it.
pub(crate) fn axis_accumulate(current: f64, stop: bool, absolute: f64) -> f64 {
    if stop { 0.0 } else { current + absolute }
}

/// Initial delay and repeat period for a compositor's `repeat_info`, or `None`
/// when it reports no repeat at all.
///
/// `rate` is keys per second and `delay` milliseconds. Both are clamped to at
/// least 1ms: a zero delay would fire the first repeat in the same breath as
/// the press, and a rate faster than 1000/s would round the period to 0ms and
/// spin the timer.
pub(crate) fn repeat_timings(rate: i32, delay: i32) -> Option<(Duration, Duration)> {
    if rate <= 0 {
        return None;
    }
    let period = Duration::from_millis(u64::from((1000u32 / rate as u32).max(1)));
    let delay = Duration::from_millis(delay.max(1) as u64);
    Some((delay, period))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_key_opens_the_context_menu_with_any_modifiers() {
        assert!(is_context_menu_key(XK_MENU, 0));
        assert!(is_context_menu_key(XK_MENU, EVENTFLAG_CONTROL_DOWN));
    }

    #[test]
    fn f10_opens_the_context_menu_only_with_shift() {
        assert!(is_context_menu_key(XK_F10, EVENTFLAG_SHIFT_DOWN));
        assert!(is_context_menu_key(
            XK_F10,
            EVENTFLAG_SHIFT_DOWN | EVENTFLAG_ALT_DOWN
        ));
        assert!(!is_context_menu_key(XK_F10, 0));
        assert!(!is_context_menu_key(XK_F10, EVENTFLAG_CONTROL_DOWN));
    }

    #[test]
    fn an_ordinary_keysym_is_not_the_context_menu_key() {
        assert!(!is_context_menu_key(b'a'.into(), EVENTFLAG_SHIFT_DOWN));
        assert!(!is_context_menu_key(0, 0));
    }

    #[test]
    fn no_modifiers_is_an_empty_mask() {
        assert_eq!(cef_modifier_flags(false, false, false), 0);
    }

    #[test]
    fn each_modifier_contributes_its_own_bit() {
        assert_eq!(cef_modifier_flags(true, false, false), EVENTFLAG_SHIFT_DOWN);
        assert_eq!(
            cef_modifier_flags(false, true, false),
            EVENTFLAG_CONTROL_DOWN
        );
        assert_eq!(cef_modifier_flags(false, false, true), EVENTFLAG_ALT_DOWN);
        assert_eq!(
            cef_modifier_flags(true, true, true),
            EVENTFLAG_SHIFT_DOWN | EVENTFLAG_CONTROL_DOWN | EVENTFLAG_ALT_DOWN
        );
    }

    #[test]
    fn only_the_three_main_buttons_have_a_held_flag() {
        assert_eq!(
            mouse_button_flag(BTN_LEFT),
            Some(EVENTFLAG_LEFT_MOUSE_BUTTON)
        );
        assert_eq!(
            mouse_button_flag(BTN_RIGHT),
            Some(EVENTFLAG_RIGHT_MOUSE_BUTTON)
        );
        assert_eq!(
            mouse_button_flag(BTN_MIDDLE),
            Some(EVENTFLAG_MIDDLE_MOUSE_BUTTON)
        );
        for button in [BTN_SIDE, BTN_EXTRA, BTN_BACK, BTN_FORWARD, 0, u32::MAX] {
            assert_eq!(mouse_button_flag(button), None, "button {button:#x}");
        }
    }

    #[test]
    fn both_forward_button_codes_navigate_forward() {
        assert_eq!(history_nav_forward(BTN_EXTRA), Some(true));
        assert_eq!(history_nav_forward(BTN_FORWARD), Some(true));
    }

    #[test]
    fn both_back_button_codes_navigate_back() {
        assert_eq!(history_nav_forward(BTN_SIDE), Some(false));
        assert_eq!(history_nav_forward(BTN_BACK), Some(false));
    }

    #[test]
    fn the_main_buttons_are_not_history_navigation() {
        for button in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, 0, u32::MAX] {
            assert_eq!(history_nav_forward(button), None, "button {button:#x}");
        }
    }

    #[test]
    fn a_discrete_wheel_frame_inverts_the_wayland_sign_and_carries_nothing() {
        let s = scroll_steps(true, 0, 120, 0.0, 0.0);
        assert_eq!(
            s,
            ScrollSteps {
                dx: 0,
                dy: -120,
                carry_x: 0.0,
                carry_y: 0.0,
            }
        );
    }

    #[test]
    fn a_discrete_frame_discards_the_continuous_accumulation() {
        // Both describe the same wheel motion; counting each would double it.
        let s = scroll_steps(true, 0, 120, 3.5, 9.0);
        assert_eq!(s.dy, -120);
        assert_eq!(s.carry_x, 0.0);
        assert_eq!(s.carry_y, 0.0);
    }

    #[test]
    fn a_whole_step_of_continuous_scroll_leaves_no_remainder() {
        let s = scroll_steps(false, 0, 0, 0.0, 1.0);
        assert_eq!(s.dy, -12);
        assert_eq!(s.carry_y, 0.0);
    }

    #[test]
    fn a_sub_step_of_continuous_scroll_is_carried_not_rounded_away() {
        // 0.5 * 12 = 6 whole steps, no remainder.
        let s = scroll_steps(false, 0, 0, 0.0, 0.5);
        assert_eq!(s.dy, -6);
        assert_eq!(s.carry_y, 0.0);
        // 0.01 * 12 = 0.12: no whole step yet, so all of it must survive.
        let s = scroll_steps(false, 0, 0, 0.0, 0.01);
        assert_eq!(s.dy, 0);
        assert!((s.carry_y - 0.01).abs() < 1e-12, "carry {}", s.carry_y);
    }

    #[test]
    fn repeated_sub_step_frames_eventually_produce_a_step() {
        let mut accum = 0.0f64;
        let mut total = 0i32;
        for _ in 0..100 {
            accum = axis_accumulate(accum, false, 0.01);
            let s = scroll_steps(false, 0, 0, 0.0, accum);
            total += s.dy;
            accum = s.carry_y;
        }
        assert!(total < 0, "slow scrolling never produced a step");
    }

    #[test]
    fn an_idle_frame_produces_nothing() {
        let s = scroll_steps(false, 0, 0, 0.0, 0.0);
        assert_eq!(
            s,
            ScrollSteps {
                dx: 0,
                dy: 0,
                carry_x: 0.0,
                carry_y: 0.0,
            }
        );
    }

    #[test]
    fn an_axis_stop_discards_the_gesture_so_far() {
        assert_eq!(axis_accumulate(7.5, true, 1.0), 0.0);
    }

    #[test]
    fn an_axis_event_adds_to_the_running_total() {
        assert_eq!(axis_accumulate(1.5, false, 2.0), 3.5);
        assert_eq!(axis_accumulate(0.0, false, -2.0), -2.0);
    }

    #[test]
    fn a_disabled_repeat_rate_has_no_timings() {
        assert_eq!(repeat_timings(0, 500), None);
        assert_eq!(repeat_timings(-1, 500), None);
    }

    #[test]
    fn typical_repeat_info_maps_to_delay_and_period() {
        assert_eq!(
            repeat_timings(25, 600),
            Some((Duration::from_millis(600), Duration::from_millis(40)))
        );
    }

    #[test]
    fn a_zero_delay_never_reaches_zero_milliseconds() {
        let (delay, _) = repeat_timings(25, 0).unwrap();
        assert_eq!(delay, Duration::from_millis(1));
        let (delay, _) = repeat_timings(25, -5).unwrap();
        assert_eq!(delay, Duration::from_millis(1));
    }

    #[test]
    fn a_rate_faster_than_the_timer_resolution_never_reaches_zero_period() {
        let (_, period) = repeat_timings(2000, 300).unwrap();
        assert_eq!(period, Duration::from_millis(1));
        let (_, period) = repeat_timings(i32::MAX, 300).unwrap();
        assert_eq!(period, Duration::from_millis(1));
    }
}
