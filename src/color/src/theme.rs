//! Window-scoped theme color tracker.
//!
//! Owns the current theme-color (`<meta name="theme-color">` updates),
//! buffers it until the loading overlay dismisses, and switches to the
//! mpv background color while video is playing so resize letterbox gaps
//! match mpv exactly.
//!
//! The two sink callbacks are installed once at process start:
//!   * `on_set_theme_color(rgb)` — optional; only set when the user has
//!     `titlebarThemeColor` enabled, drives the platform titlebar tint.
//!   * `on_set_bg_hex(c_str)` — required; passes `#RRGGBB` to mpv so its
//!     background matches the chrome during resize.

use parking_lot::Mutex;
use std::ffi::c_char;

const DEFAULT_BG_RGB: u32 = 0x101010; // kBgColor

struct ThemeColor {
    on_set_theme_color: Option<unsafe extern "C" fn(u32)>,
    on_set_bg_hex: unsafe extern "C" fn(*const c_char),
    video_bg_rgb: u32,
    current_rgb: u32,
    unlocked: bool,
    video_active: bool,
    last_applied: Option<u32>,
}

impl ThemeColor {
    fn resolved(&self) -> u32 {
        if self.video_active {
            self.video_bg_rgb
        } else {
            self.current_rgb
        }
    }

    fn apply(&mut self) {
        let rgb = self.resolved();
        if self.last_applied == Some(rgb) {
            return;
        }
        self.last_applied = Some(rgb);
        if let Some(f) = self.on_set_theme_color {
            unsafe { f(rgb) };
        }
        let mut hex = [0u8; 8];
        format_hex_rgb(rgb, &mut hex);
        unsafe { (self.on_set_bg_hex)(hex.as_ptr() as *const c_char) };
    }
}

fn format_hex_rgb(rgb: u32, out: &mut [u8; 8]) {
    out[0] = b'#';
    for i in 0..6 {
        let nibble = ((rgb >> (20 - i * 4)) & 0xF) as u8;
        out[1 + i] = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + (nibble - 10)
        };
    }
    out[7] = 0;
}

static INSTANCE: Mutex<Option<ThemeColor>> = Mutex::new(None);

/// Initialise the process-wide theme color singleton. Calling a second time
/// replaces the previous state — the new sink callbacks take effect on the
/// next `apply()`.
///
/// # Safety
/// Callbacks must be valid for the lifetime of the process.
pub unsafe fn jfn_theme_color_init(
    on_set_theme_color: Option<unsafe extern "C" fn(u32)>,
    on_set_bg_hex: Option<unsafe extern "C" fn(*const c_char)>,
) {
    let Some(on_set_bg_hex) = on_set_bg_hex else {
        return;
    };
    let mut tc = ThemeColor {
        on_set_theme_color,
        on_set_bg_hex,
        video_bg_rgb: DEFAULT_BG_RGB,
        current_rgb: DEFAULT_BG_RGB,
        unlocked: false,
        video_active: false,
        last_applied: None,
    };
    tc.apply();
    *INSTANCE.lock() = Some(tc);
}

pub fn jfn_theme_color_set_video_bg(rgb: u32) {
    let mut g = INSTANCE.lock();
    if let Some(t) = g.as_mut() {
        t.video_bg_rgb = rgb;
        if t.video_active && t.unlocked {
            t.apply();
        }
    }
}

pub fn jfn_theme_color_on_color(rgb: u32) {
    let mut g = INSTANCE.lock();
    if let Some(t) = g.as_mut() {
        t.current_rgb = rgb;
        if t.unlocked {
            t.apply();
        }
    }
}

pub fn jfn_theme_color_on_overlay_dismissed() {
    let mut g = INSTANCE.lock();
    if let Some(t) = g.as_mut() {
        t.unlocked = true;
        t.apply();
    }
}

pub fn jfn_theme_color_set_video_mode(active: bool) {
    let mut g = INSTANCE.lock();
    if let Some(t) = g.as_mut() {
        if t.video_active == active {
            return;
        }
        t.video_active = active;
        if t.unlocked {
            t.apply();
        }
    }
}

pub fn jfn_theme_color_shutdown() {
    *INSTANCE.lock() = None;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    // The tracker is a process-wide singleton, so every test that touches it
    // runs under this lock and tears the singleton down again. Recorded calls
    // are cleared on entry rather than on exit so a panicking test cannot
    // leave a stale expectation behind.
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static THEME_CALLS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    static BG_CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    unsafe extern "C" fn record_theme(rgb: u32) {
        THEME_CALLS.lock().push(rgb);
    }

    unsafe extern "C" fn record_bg(s: *const c_char) {
        let text = unsafe { std::ffi::CStr::from_ptr(s) }
            .to_string_lossy()
            .into_owned();
        BG_CALLS.lock().push(text);
    }

    /// Serialises the test and starts it from a known-empty recorder.
    struct Fixture(#[allow(dead_code)] parking_lot::MutexGuard<'static, ()>);

    impl Fixture {
        fn new() -> Self {
            let g = TEST_LOCK.lock();
            jfn_theme_color_shutdown();
            THEME_CALLS.lock().clear();
            BG_CALLS.lock().clear();
            Self(g)
        }

        fn init(&self, with_theme_sink: bool) {
            unsafe {
                jfn_theme_color_init(
                    with_theme_sink.then_some(record_theme as unsafe extern "C" fn(u32)),
                    Some(record_bg as unsafe extern "C" fn(*const c_char)),
                );
            }
        }

        fn theme_calls(&self) -> Vec<u32> {
            THEME_CALLS.lock().clone()
        }

        fn bg_calls(&self) -> Vec<String> {
            BG_CALLS.lock().clone()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            jfn_theme_color_shutdown();
        }
    }

    #[test]
    fn hex_format() {
        let mut buf = [0u8; 8];
        format_hex_rgb(0x101010, &mut buf);
        assert_eq!(&buf[..7], b"#101010");
        format_hex_rgb(0xabcdef, &mut buf);
        assert_eq!(&buf[..7], b"#abcdef");
        format_hex_rgb(0x000000, &mut buf);
        assert_eq!(&buf[..7], b"#000000");
    }

    #[test]
    fn hex_format_nul_terminates_the_buffer() {
        let mut buf = [0xFFu8; 8];
        format_hex_rgb(0xFF_FFFF, &mut buf);
        assert_eq!(&buf, b"#ffffff\0");
    }

    #[test]
    fn init_pushes_the_default_background_to_both_sinks() {
        let f = Fixture::new();
        f.init(true);
        assert_eq!(f.theme_calls(), vec![DEFAULT_BG_RGB]);
        assert_eq!(f.bg_calls(), vec!["#101010".to_string()]);
    }

    #[test]
    fn init_without_a_bg_sink_installs_nothing() {
        let f = Fixture::new();
        unsafe {
            jfn_theme_color_init(Some(record_theme as unsafe extern "C" fn(u32)), None);
        }
        assert!(f.theme_calls().is_empty());
        assert!(f.bg_calls().is_empty());
        // No singleton was installed, so later setters stay silent too.
        jfn_theme_color_on_overlay_dismissed();
        jfn_theme_color_on_color(0xFF_0000);
        assert!(f.bg_calls().is_empty());
    }

    #[test]
    fn a_theme_color_is_buffered_until_the_overlay_dismisses() {
        let f = Fixture::new();
        f.init(true);
        jfn_theme_color_on_color(0x00_2244);
        // Still locked: only the boot default has been applied.
        assert_eq!(f.bg_calls(), vec!["#101010".to_string()]);
        jfn_theme_color_on_overlay_dismissed();
        assert_eq!(
            f.bg_calls(),
            vec!["#101010".to_string(), "#002244".to_string()]
        );
        assert_eq!(f.theme_calls(), vec![DEFAULT_BG_RGB, 0x00_2244]);
    }

    #[test]
    fn a_repeated_color_is_applied_once() {
        let f = Fixture::new();
        f.init(true);
        jfn_theme_color_on_overlay_dismissed();
        jfn_theme_color_on_color(0x00_2244);
        jfn_theme_color_on_color(0x00_2244);
        assert_eq!(
            f.bg_calls(),
            vec!["#101010".to_string(), "#002244".to_string()]
        );
    }

    #[test]
    fn video_mode_swaps_in_the_mpv_background_and_back() {
        let f = Fixture::new();
        f.init(true);
        jfn_theme_color_on_overlay_dismissed();
        jfn_theme_color_on_color(0x00_2244);
        jfn_theme_color_set_video_mode(true);
        assert_eq!(f.bg_calls().last().map(String::as_str), Some("#101010"));
        // The same value again is a no-op: the tracker dedupes on the flag.
        let before = f.bg_calls().len();
        jfn_theme_color_set_video_mode(true);
        assert_eq!(f.bg_calls().len(), before);
        jfn_theme_color_set_video_mode(false);
        assert_eq!(f.bg_calls().last().map(String::as_str), Some("#002244"));
    }

    #[test]
    fn video_bg_repaints_only_while_video_is_active() {
        let f = Fixture::new();
        f.init(true);
        jfn_theme_color_on_overlay_dismissed();
        let before = f.bg_calls().len();
        // Not in video mode: the new background is stored, not applied.
        jfn_theme_color_set_video_bg(0x12_3456);
        assert_eq!(f.bg_calls().len(), before);
        jfn_theme_color_set_video_mode(true);
        assert_eq!(f.bg_calls().last().map(String::as_str), Some("#123456"));
        // Now it is live, so a further change repaints immediately.
        jfn_theme_color_set_video_bg(0x65_4321);
        assert_eq!(f.bg_calls().last().map(String::as_str), Some("#654321"));
    }

    #[test]
    fn a_locked_tracker_never_applies_a_video_mode_change() {
        let f = Fixture::new();
        f.init(true);
        // No overlay dismissal: only the boot default has gone out.
        jfn_theme_color_set_video_bg(0x12_3456);
        jfn_theme_color_set_video_mode(true);
        assert_eq!(f.bg_calls(), vec!["#101010".to_string()]);
    }

    #[test]
    fn the_optional_theme_sink_may_be_absent() {
        let f = Fixture::new();
        f.init(false);
        jfn_theme_color_on_overlay_dismissed();
        jfn_theme_color_on_color(0x00_2244);
        assert!(f.theme_calls().is_empty());
        assert_eq!(
            f.bg_calls(),
            vec!["#101010".to_string(), "#002244".to_string()]
        );
    }

    #[test]
    fn shutdown_makes_every_setter_a_no_op() {
        let f = Fixture::new();
        f.init(true);
        jfn_theme_color_on_overlay_dismissed();
        jfn_theme_color_shutdown();
        let after_shutdown = f.bg_calls().len();
        jfn_theme_color_on_color(0xFF_0000);
        jfn_theme_color_set_video_bg(0x00_FF00);
        jfn_theme_color_set_video_mode(true);
        jfn_theme_color_on_overlay_dismissed();
        assert_eq!(f.bg_calls().len(), after_shutdown);
    }

    #[test]
    fn a_second_init_replaces_the_previous_state() {
        let f = Fixture::new();
        f.init(true);
        jfn_theme_color_on_overlay_dismissed();
        jfn_theme_color_on_color(0x00_2244);
        f.init(true);
        // Re-initialised: the boot default is applied again and the tracker is
        // locked again, so the previously reported color no longer applies.
        assert_eq!(f.bg_calls().last().map(String::as_str), Some("#101010"));
        jfn_theme_color_on_color(0x00_2244);
        assert_eq!(f.bg_calls().last().map(String::as_str), Some("#101010"));
    }
}
