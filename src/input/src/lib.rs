//! Input dispatch. Translates platform key/pointer events into CEF
//! events and forwards them to the active browser via
//! [`jfn_platform_abi::browser_bridge`].

use jfn_platform_abi::event_flags::EVENTFLAG_PRECISION_SCROLLING_DELTA;
use jfn_platform_abi::{BrowserBridge, browser_bridge};
use jfn_playback::hotkey::jfn_hotkey_classify_keydown;
use jfn_playback::shutdown::jfn_shutdown_initiate;
use std::os::raw::c_int;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Instant;

pub mod buttons;
mod click_count;
pub mod scroll;

use click_count::{ClickCounter, MAX_CLICKS};

const KEYEVENT_RAWKEYDOWN: c_int = 0;
const KEYEVENT_KEYUP: c_int = 2;
const KEYEVENT_CHAR: c_int = 3;
const MBT_LEFT: c_int = 0;
const MBT_MIDDLE: c_int = 1;
const MBT_RIGHT: c_int = 2;

fn cef_button(button_code: u32) -> Option<c_int> {
    match button_code {
        buttons::BTN_LEFT => Some(MBT_LEFT),
        buttons::BTN_RIGHT => Some(MBT_RIGHT),
        buttons::BTN_MIDDLE => Some(MBT_MIDDLE),
        _ => None,
    }
}

fn with_bridge<F: FnOnce(&dyn BrowserBridge)>(f: F) {
    if let Some(b) = browser_bridge() {
        f(b);
    }
}

pub fn jfn_input_dispatch_mouse_move(x: i32, y: i32, mods: u32, leave: c_int) {
    with_bridge(|b| b.send_mouse_move(x, y, mods, leave != 0));
}

/// Process-wide click counter for the hosts that have no native count.
static CLICK_COUNTER: Mutex<ClickCounter> = Mutex::new(ClickCounter::new());

/// Monotonic base for the counter's clock; `Instant::now()` at first use.
static PROCESS_START: LazyLock<Instant> = LazyLock::new(Instant::now);

fn now_ms() -> u64 {
    u64::try_from(PROCESS_START.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Forgets the last press, so one test's clicks cannot become another's
/// double click. Called from the test setup that takes the serial lock.
#[cfg(test)]
pub(crate) fn reset_click_counter() {
    *CLICK_COUNTER.lock().unwrap_or_else(PoisonError::into_inner) = ClickCounter::new();
}

/// Mouse button dispatch for hosts whose events carry no click count —
/// Windows, X11 and Wayland. The count is recovered from the press stream by
/// [`click_count::ClickCounter`], so `dblclick` reaches the page. macOS has a
/// real count on the `NSEvent` and uses
/// [`jfn_input_dispatch_mouse_button_counted`] instead.
pub fn jfn_input_dispatch_mouse_button(
    button_code: u32,
    pressed: c_int,
    x: i32,
    y: i32,
    mods: u32,
) {
    let Some(btn) = cef_button(button_code) else {
        return;
    };
    let count = {
        let mut counter = CLICK_COUNTER.lock().unwrap_or_else(PoisonError::into_inner);
        if pressed == 0 {
            // A release repeats the count of the press it closes.
            counter.last()
        } else {
            counter.press(button_code, x, y, now_ms())
        }
    };
    with_bridge(|b| b.send_mouse_click(x, y, mods, btn, pressed == 0, count));
}

/// Mouse button dispatch with a platform-supplied click count, for hosts that
/// track one themselves (macOS `NSEvent.clickCount`). The count is clamped to
/// the range CEF understands; a synthetic event can report 0, which is one
/// click here. This path deliberately bypasses the shared counter, so a
/// platform never counts a click twice.
pub fn jfn_input_dispatch_mouse_button_counted(
    button_code: u32,
    pressed: c_int,
    x: i32,
    y: i32,
    mods: u32,
    click_count: c_int,
) {
    let Some(btn) = cef_button(button_code) else {
        return;
    };
    let count = click_count.clamp(1, MAX_CLICKS);
    with_bridge(|b| b.send_mouse_click(x, y, mods, btn, pressed == 0, count));
}

pub fn jfn_input_dispatch_scroll(x: i32, y: i32, dx: i32, dy: i32, mods: u32) {
    with_bridge(|b| b.send_mouse_wheel(x, y, mods, dx, dy));
}

/// Variant that lets the caller flag a precision (trackpad) delta.
pub fn jfn_input_dispatch_scroll_precise(
    x: i32,
    y: i32,
    dx: i32,
    dy: i32,
    mods: u32,
    precise: c_int,
) {
    let mods = if precise != 0 {
        mods | EVENTFLAG_PRECISION_SCROLLING_DELTA
    } else {
        mods
    };
    with_bridge(|b| b.send_mouse_wheel(x, y, mods, dx, dy));
}

pub fn jfn_input_dispatch_history_nav(forward: c_int) {
    with_bridge(|b| b.navigate_history(forward != 0));
}

pub fn jfn_input_dispatch_keyboard_focus(gained: c_int) {
    with_bridge(|b| b.set_focus(gained != 0));
}

/// Char event with explicit is_system_key (for WM_SYSCHAR on Windows). The
/// 3-arg `jfn_input_dispatch_char` below is the wayland/x11 path which never
/// generates system chars.
pub fn jfn_input_dispatch_char_sys(
    codepoint: u32,
    mods: u32,
    native_code: u32,
    is_system_key: c_int,
) {
    if codepoint == 0 || codepoint >= 0x10_FFFF {
        return;
    }
    let cp16 = codepoint as u16;
    with_bridge(|b| {
        b.send_key_event(
            KEYEVENT_CHAR,
            mods,
            codepoint as c_int,
            native_code as c_int,
            is_system_key != 0,
            cp16,
            cp16,
        );
    });
}

pub fn jfn_input_dispatch_char(codepoint: u32, mods: u32, native_code: u32) {
    jfn_input_dispatch_char_sys(codepoint, mods, native_code, 0);
}

/// Flat key dispatch used by macOS and Windows input shims. Linux paths use
/// `jfn_linux_util::input::jfn_input_dispatch_key_raw`, which routes through
/// the xkb keysym → VK mapping first.
pub fn jfn_input_dispatch_key_full(
    pressed: c_int,
    windows_key_code: i32,
    native_key_code: i32,
    modifiers: u32,
    character: u16,
    unmodified_character: u16,
    is_system_key: c_int,
) {
    if pressed != 0 {
        match jfn_hotkey_classify_keydown(windows_key_code, modifiers) {
            1 => {
                jfn_shutdown_initiate();
                return;
            }
            2 => {
                if let Some(p) = jfn_platform_abi::try_get() {
                    p.toggle_fullscreen();
                }
                return;
            }
            _ => {}
        }
    }
    let type_ = if pressed != 0 {
        KEYEVENT_RAWKEYDOWN
    } else {
        KEYEVENT_KEYUP
    };
    with_bridge(|b| {
        b.send_key_event(
            type_,
            modifiers,
            windows_key_code,
            native_key_code,
            is_system_key != 0,
            character,
            unmodified_character,
        );
    });
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard, Once, PoisonError};

    use jfn_platform_abi::event_flags::{EVENTFLAG_ALT_DOWN, EVENTFLAG_SHIFT_DOWN};
    use jfn_playback::shutdown::jfn_shutting_down;

    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        Key {
            type_: c_int,
            mods: u32,
            windows_key_code: c_int,
            native_key_code: c_int,
            is_system_key: bool,
            character: u16,
            unmodified_character: u16,
        },
        Click {
            x: c_int,
            y: c_int,
            mods: u32,
            button: c_int,
            mouse_up: bool,
            click_count: c_int,
        },
        Move {
            x: i32,
            y: i32,
            mods: u32,
            leave: bool,
        },
        Wheel {
            x: c_int,
            y: c_int,
            mods: u32,
            dx: c_int,
            dy: c_int,
        },
        Focus(bool),
        History(bool),
    }

    static CALLS: Mutex<Vec<Call>> = Mutex::new(Vec::new());
    static SERIAL: Mutex<()> = Mutex::new(());

    struct Recorder;

    impl Recorder {
        fn push(call: Call) {
            CALLS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(call);
        }
    }

    impl BrowserBridge for Recorder {
        fn send_key_event(
            &self,
            type_: c_int,
            modifiers: u32,
            windows_key_code: c_int,
            native_key_code: c_int,
            is_system_key: bool,
            character: u16,
            unmodified_character: u16,
        ) {
            Self::push(Call::Key {
                type_,
                mods: modifiers,
                windows_key_code,
                native_key_code,
                is_system_key,
                character,
                unmodified_character,
            });
        }

        fn send_mouse_click(
            &self,
            x: c_int,
            y: c_int,
            modifiers: u32,
            button: c_int,
            mouse_up: bool,
            click_count: c_int,
        ) {
            Self::push(Call::Click {
                x,
                y,
                mods: modifiers,
                button,
                mouse_up,
                click_count,
            });
        }

        fn send_mouse_move(&self, x: i32, y: i32, modifiers: u32, leave: bool) {
            Self::push(Call::Move {
                x,
                y,
                mods: modifiers,
                leave,
            });
        }

        fn send_mouse_wheel(
            &self,
            x: c_int,
            y: c_int,
            modifiers: u32,
            delta_x: c_int,
            delta_y: c_int,
        ) {
            Self::push(Call::Wheel {
                x,
                y,
                mods: modifiers,
                dx: delta_x,
                dy: delta_y,
            });
        }

        fn set_focus(&self, focus: bool) {
            Self::push(Call::Focus(focus));
        }

        fn navigate_history(&self, forward: bool) {
            Self::push(Call::History(forward));
        }

        fn undo(&self) {}
        fn redo(&self) {}
        fn cut(&self) {}
        fn copy(&self) {}
        fn paste(&self) {}
        fn select_all(&self) {}

        fn has_active(&self) -> bool {
            true
        }
    }

    /// The browser bridge is a process-wide install-once handle: install the
    /// recorder once for the whole test binary and serialize the tests that
    /// read its log, so no test depends on running first.
    fn recording() -> MutexGuard<'static, ()> {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| jfn_platform_abi::install_browser_bridge(Box::new(Recorder)));
        let guard = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        CALLS.lock().unwrap_or_else(PoisonError::into_inner).clear();
        reset_click_counter();
        guard
    }

    fn calls() -> Vec<Call> {
        std::mem::take(&mut *CALLS.lock().unwrap_or_else(PoisonError::into_inner))
    }

    #[test]
    fn a_mouse_move_forwards_its_position_and_leave_flag() {
        let _serial = recording();
        jfn_input_dispatch_mouse_move(3, 4, EVENTFLAG_SHIFT_DOWN, 0);
        jfn_input_dispatch_mouse_move(5, 6, 0, 1);
        assert_eq!(
            calls(),
            vec![
                Call::Move {
                    x: 3,
                    y: 4,
                    mods: EVENTFLAG_SHIFT_DOWN,
                    leave: false,
                },
                Call::Move {
                    x: 5,
                    y: 6,
                    mods: 0,
                    leave: true,
                },
            ]
        );
    }

    #[test]
    fn each_known_button_maps_to_its_cef_button_id() {
        let _serial = recording();
        for (code, cef) in [
            (buttons::BTN_LEFT, MBT_LEFT),
            (buttons::BTN_MIDDLE, MBT_MIDDLE),
            (buttons::BTN_RIGHT, MBT_RIGHT),
        ] {
            jfn_input_dispatch_mouse_button(code, 1, 10, 20, 0);
            assert_eq!(
                calls(),
                vec![Call::Click {
                    x: 10,
                    y: 20,
                    mods: 0,
                    button: cef,
                    mouse_up: false,
                    click_count: 1,
                }]
            );
        }
    }

    #[test]
    fn a_released_button_is_reported_as_a_mouse_up() {
        let _serial = recording();
        jfn_input_dispatch_mouse_button(buttons::BTN_LEFT, 0, 1, 2, 0);
        assert_eq!(
            calls(),
            vec![Call::Click {
                x: 1,
                y: 2,
                mods: 0,
                button: MBT_LEFT,
                mouse_up: true,
                click_count: 1,
            }]
        );
    }

    #[test]
    fn two_quick_presses_at_one_point_are_dispatched_as_a_double_click() {
        let _serial = recording();
        for _ in 0..2 {
            jfn_input_dispatch_mouse_button(buttons::BTN_LEFT, 1, 7, 8, 0);
            jfn_input_dispatch_mouse_button(buttons::BTN_LEFT, 0, 7, 8, 0);
        }
        let counts: Vec<(bool, c_int)> = calls()
            .iter()
            .filter_map(|c| match *c {
                Call::Click {
                    mouse_up,
                    click_count,
                    ..
                } => Some((mouse_up, click_count)),
                _ => None,
            })
            .collect();
        assert_eq!(
            counts,
            vec![(false, 1), (true, 1), (false, 2), (true, 2)],
            "the release carries the count of the press it closes"
        );
    }

    #[test]
    fn the_counted_dispatch_forwards_its_count_and_clamps_it_to_the_cef_range() {
        let _serial = recording();
        for (given, expected) in [(2, 2), (0, 1), (-3, 1), (7, 3)] {
            jfn_input_dispatch_mouse_button_counted(buttons::BTN_LEFT, 1, 0, 0, 0, given);
            assert_eq!(
                calls(),
                vec![Call::Click {
                    x: 0,
                    y: 0,
                    mods: 0,
                    button: MBT_LEFT,
                    mouse_up: false,
                    click_count: expected,
                }],
                "a native count of {given}"
            );
        }
    }

    #[test]
    fn a_button_code_cef_has_no_id_for_is_dropped() {
        let _serial = recording();
        for code in [
            buttons::BTN_SIDE,
            buttons::BTN_EXTRA,
            buttons::BTN_FORWARD,
            buttons::BTN_BACK,
            0,
        ] {
            jfn_input_dispatch_mouse_button(code, 1, 0, 0, 0);
        }
        assert_eq!(calls(), vec![]);
    }

    #[test]
    fn a_scroll_forwards_its_deltas_unchanged() {
        let _serial = recording();
        jfn_input_dispatch_scroll(1, 2, -30, 120, EVENTFLAG_SHIFT_DOWN);
        assert_eq!(
            calls(),
            vec![Call::Wheel {
                x: 1,
                y: 2,
                mods: EVENTFLAG_SHIFT_DOWN,
                dx: -30,
                dy: 120,
            }]
        );
    }

    #[test]
    fn a_precise_scroll_adds_the_precision_flag_to_the_modifiers() {
        let _serial = recording();
        jfn_input_dispatch_scroll_precise(0, 0, 1, 2, EVENTFLAG_SHIFT_DOWN, 1);
        jfn_input_dispatch_scroll_precise(0, 0, 1, 2, EVENTFLAG_SHIFT_DOWN, 0);
        assert_eq!(
            calls(),
            vec![
                Call::Wheel {
                    x: 0,
                    y: 0,
                    mods: EVENTFLAG_SHIFT_DOWN | EVENTFLAG_PRECISION_SCROLLING_DELTA,
                    dx: 1,
                    dy: 2,
                },
                Call::Wheel {
                    x: 0,
                    y: 0,
                    mods: EVENTFLAG_SHIFT_DOWN,
                    dx: 1,
                    dy: 2,
                },
            ]
        );
    }

    #[test]
    fn history_nav_forwards_the_direction() {
        let _serial = recording();
        jfn_input_dispatch_history_nav(0);
        jfn_input_dispatch_history_nav(1);
        assert_eq!(calls(), vec![Call::History(false), Call::History(true)]);
    }

    #[test]
    fn keyboard_focus_forwards_gained_and_lost() {
        let _serial = recording();
        jfn_input_dispatch_keyboard_focus(1);
        jfn_input_dispatch_keyboard_focus(0);
        assert_eq!(calls(), vec![Call::Focus(true), Call::Focus(false)]);
    }

    #[test]
    fn a_char_event_carries_the_codepoint_as_both_characters() {
        let _serial = recording();
        jfn_input_dispatch_char(0x41, EVENTFLAG_SHIFT_DOWN, 0x1E);
        assert_eq!(
            calls(),
            vec![Call::Key {
                type_: KEYEVENT_CHAR,
                mods: EVENTFLAG_SHIFT_DOWN,
                windows_key_code: 0x41,
                native_key_code: 0x1E,
                is_system_key: false,
                character: 0x41,
                unmodified_character: 0x41,
            }]
        );
    }

    #[test]
    fn char_dispatch_drops_the_null_and_out_of_range_codepoints() {
        let _serial = recording();
        for cp in [0, 0x10_FFFF, 0x11_0000, u32::MAX] {
            jfn_input_dispatch_char(cp, 0, 0);
        }
        assert_eq!(calls(), vec![]);
        // One below the rejected boundary still dispatches.
        jfn_input_dispatch_char(0x10_FFFE, 0, 0);
        assert_eq!(calls().len(), 1);
    }

    #[test]
    fn a_system_char_is_flagged_for_the_browser() {
        let _serial = recording();
        jfn_input_dispatch_char_sys(0x66, EVENTFLAG_ALT_DOWN, 0x21, 1);
        assert_eq!(
            calls(),
            vec![Call::Key {
                type_: KEYEVENT_CHAR,
                mods: EVENTFLAG_ALT_DOWN,
                windows_key_code: 0x66,
                native_key_code: 0x21,
                is_system_key: true,
                character: 0x66,
                unmodified_character: 0x66,
            }]
        );
    }

    #[test]
    fn a_key_down_and_a_key_up_use_distinct_cef_event_types() {
        let _serial = recording();
        jfn_input_dispatch_key_full(1, 0x41, 0x1E, 0, 97, 97, 0);
        jfn_input_dispatch_key_full(0, 0x41, 0x1E, 0, 97, 97, 0);
        assert_eq!(
            calls(),
            vec![
                Call::Key {
                    type_: KEYEVENT_RAWKEYDOWN,
                    mods: 0,
                    windows_key_code: 0x41,
                    native_key_code: 0x1E,
                    is_system_key: false,
                    character: 97,
                    unmodified_character: 97,
                },
                Call::Key {
                    type_: KEYEVENT_KEYUP,
                    mods: 0,
                    windows_key_code: 0x41,
                    native_key_code: 0x1E,
                    is_system_key: false,
                    character: 97,
                    unmodified_character: 97,
                },
            ]
        );
    }

    #[test]
    fn a_fullscreen_hotkey_with_no_active_video_player_reaches_the_browser() {
        let _serial = recording();
        // VK_F11 with no playback coordinator: not a hotkey here, so it is
        // forwarded like any other key.
        jfn_input_dispatch_key_full(1, 0x7A, 0, 0, 0, 0, 0);
        assert_eq!(calls().len(), 1);
    }

    #[test]
    fn alt_f4_initiates_shutdown_instead_of_reaching_the_browser() {
        let _serial = recording();
        jfn_input_dispatch_key_full(1, 0x73, 0, EVENTFLAG_ALT_DOWN, 0, 0, 0);
        assert_eq!(calls(), vec![], "the shutdown hotkey is swallowed");
        assert!(jfn_shutting_down());
        // The key-up half is not a hotkey and still reaches the browser.
        jfn_input_dispatch_key_full(0, 0x73, 0, EVENTFLAG_ALT_DOWN, 0, 0, 0);
        assert_eq!(calls().len(), 1);
    }
}
