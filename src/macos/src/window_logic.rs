//! Pure helpers for the AppKit window glue in `crate::lib` and `crate::init`.
//!
//! Screen and window rects arrive as plain `NSRect` values (AppKit points,
//! origin bottom-left) and leave as the platform ABI's backing-pixel types;
//! the colour, style-mask and path helpers are likewise input -> output. Every
//! `NSWindow` / `NSScreen` message send stays at the call site.

use std::ffi::{CString, c_int};
use std::path::{Path, PathBuf};

use jfn_platform_abi::WindowPos;
use jfn_platform_abi::geometry::Bounds;
use objc2_foundation::NSRect;

/// `NSWindowStyleMaskFullSizeContentView`.
const NS_WINDOW_STYLE_MASK_FULL_SIZE_CONTENT_VIEW: u64 = 1 << 15;

/// The `kIOPMAssertionTypePrevent*` name an [`jfn_platform_abi::IdleInhibitLevel`]
/// asks for, or `None` for "no assertion at all".
///
/// The constants are `CFSTR()` macros with no linker symbol, so the caller
/// builds the `CFString` from this name. Levels are `None=0`, `System=1`,
/// `Display=2`; anything else releases without asserting, which is what an
/// unknown level did before.
pub(crate) fn idle_assertion_type(level: c_int) -> Option<&'static str> {
    match level {
        2 => Some("PreventUserIdleDisplaySleep"),
        1 => Some("PreventUserIdleSystemSleep"),
        _ => None,
    }
}

/// The window's top-left corner in backing pixels, relative to the screen's
/// visible frame (menu bar and dock excluded), Y measured downwards.
///
/// AppKit measures Y upwards from the bottom of the screen and mpv's
/// `--geometry +X+Y` measures it downwards from the top of the work area, so
/// the Y axis is flipped against the visible frame's top edge. Lossless
/// round-trip with `--geometry`.
pub(crate) fn window_position_in_backing(frame: NSRect, visible: NSRect, scale: f64) -> WindowPos {
    let lx = frame.origin.x - visible.origin.x;
    let ly = (visible.origin.y + visible.size.height) - (frame.origin.y + frame.size.height);
    WindowPos {
        x: (lx * scale) as c_int,
        y: (ly * scale) as c_int,
    }
}

/// A screen's visible frame as clamp bounds in backing pixels — the space the
/// saved geometry is stored in.
pub(crate) fn visible_bounds_in_backing(visible: NSRect, scale: f64) -> Bounds {
    Bounds {
        w: (visible.size.width * scale) as c_int,
        h: (visible.size.height * scale) as c_int,
    }
}

/// A content view's size in whole logical points, truncated the way the C++
/// original truncated it. Zero or negative means "no usable size"; the caller
/// decides what to do with that.
pub(crate) fn content_size_in_points(bounds: NSRect) -> (c_int, c_int) {
    (bounds.size.width as c_int, bounds.size.height as c_int)
}

/// `0xRRGGBB` as the three 0..=1 components `colorWithSRGBRed:green:blue:`
/// takes. Anything above the low 24 bits is ignored.
pub(crate) fn srgb_components(rgb: u32) -> (f64, f64, f64) {
    (
        f64::from((rgb >> 16) & 0xff) / 255.0,
        f64::from((rgb >> 8) & 0xff) / 255.0,
        f64::from(rgb & 0xff) / 255.0,
    )
}

/// The window's style mask with `NSWindowStyleMaskFullSizeContentView` added,
/// so the content view extends under the transparent title bar.
pub(crate) fn with_full_size_content_view(mask: u64) -> u64 {
    mask | NS_WINDOW_STYLE_MASK_FULL_SIZE_CONTENT_VIEW
}

/// Where the CEF framework sits relative to the running executable.
///
/// In a bundle the executable is `Astrofin.app/Contents/MacOS/astrofin` and
/// the framework is `Astrofin.app/Contents/Frameworks/...`, i.e. two levels up
/// and back down. An executable with fewer than two ancestors (a bare relative
/// path) is used as the base itself, which yields a path that does not exist —
/// CEF then reports a missing framework rather than this silently guessing.
pub(crate) fn cef_framework_dir(exe: &Path) -> PathBuf {
    let app_contents = exe.parent().and_then(|p| p.parent()).unwrap_or(exe);
    app_contents
        .join("Frameworks")
        .join("Chromium Embedded Framework.framework")
}

/// A menu title or key equivalent as a C string. A string carrying an interior
/// NUL cannot cross into `-stringWithUTF8String:` at all, so it becomes empty
/// rather than truncated at the NUL.
pub(crate) fn menu_c_string(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::{NSPoint, NSSize};

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
        NSRect {
            origin: NSPoint { x, y },
            size: NSSize { width, height },
        }
    }

    #[test]
    fn each_inhibit_level_names_its_assertion() {
        assert_eq!(idle_assertion_type(2), Some("PreventUserIdleDisplaySleep"));
        assert_eq!(idle_assertion_type(1), Some("PreventUserIdleSystemSleep"));
    }

    #[test]
    fn no_inhibit_and_unknown_levels_assert_nothing() {
        assert_eq!(idle_assertion_type(0), None);
        assert_eq!(idle_assertion_type(3), None);
        assert_eq!(idle_assertion_type(-1), None);
    }

    #[test]
    fn a_window_at_the_top_left_of_the_work_area_is_the_origin() {
        // 1440x900 visible frame whose origin sits above the dock.
        let visible = rect(0.0, 50.0, 1440.0, 850.0);
        let frame = rect(0.0, 500.0, 400.0, 400.0);
        assert_eq!(
            window_position_in_backing(frame, visible, 1.0),
            WindowPos { x: 0, y: 0 }
        );
    }

    #[test]
    fn the_y_axis_is_flipped_against_the_visible_frame_and_scaled() {
        let visible = rect(0.0, 50.0, 1440.0, 850.0);
        // 100 points right, and its top edge 200 points below the work area's.
        let frame = rect(100.0, 300.0, 400.0, 400.0);
        assert_eq!(
            window_position_in_backing(frame, visible, 1.0),
            WindowPos { x: 100, y: 200 }
        );
        // Retina: the same window in backing pixels.
        assert_eq!(
            window_position_in_backing(frame, visible, 2.0),
            WindowPos { x: 200, y: 400 }
        );
    }

    #[test]
    fn a_secondary_screen_is_measured_from_its_own_origin() {
        let visible = rect(1440.0, 0.0, 1920.0, 1080.0);
        let frame = rect(1540.0, 780.0, 400.0, 300.0);
        assert_eq!(
            window_position_in_backing(frame, visible, 1.0),
            WindowPos { x: 100, y: 0 }
        );
    }

    #[test]
    fn visible_bounds_scale_to_backing_pixels() {
        let visible = rect(0.0, 50.0, 1440.0, 850.0);
        assert_eq!(
            visible_bounds_in_backing(visible, 2.0),
            Bounds { w: 2880, h: 1700 }
        );
        assert_eq!(
            visible_bounds_in_backing(visible, 1.0),
            Bounds { w: 1440, h: 850 }
        );
        // A fractional scale truncates rather than rounds, as the cast did.
        assert_eq!(
            visible_bounds_in_backing(rect(0.0, 0.0, 1000.0, 1000.0), 1.5001),
            Bounds { w: 1500, h: 1500 }
        );
    }

    #[test]
    fn a_content_size_truncates_to_whole_points() {
        assert_eq!(
            content_size_in_points(rect(0.0, 0.0, 800.0, 600.0)),
            (800, 600)
        );
        assert_eq!(
            content_size_in_points(rect(0.0, 0.0, 1439.9, 899.5)),
            (1439, 899)
        );
        assert_eq!(content_size_in_points(rect(0.0, 0.0, 0.0, 0.0)), (0, 0));
    }

    #[test]
    fn a_theme_colour_splits_into_srgb_components() {
        assert_eq!(srgb_components(0x000000), (0.0, 0.0, 0.0));
        assert_eq!(srgb_components(0xffffff), (1.0, 1.0, 1.0));
        let (r, g, b) = srgb_components(0xff8000);
        assert_eq!(r, 1.0);
        assert_eq!(g, 128.0 / 255.0);
        assert_eq!(b, 0.0);
    }

    #[test]
    fn bits_above_the_colour_are_ignored() {
        assert_eq!(srgb_components(0xff_101010), srgb_components(0x101010));
    }

    #[test]
    fn the_full_size_content_bit_is_added_once() {
        assert_eq!(with_full_size_content_view(0), 1 << 15);
        // Every other style bit survives.
        assert_eq!(with_full_size_content_view(0b1111), 0b1111 | (1 << 15));
        // Already set: idempotent.
        assert_eq!(with_full_size_content_view(1 << 15), 1 << 15);
    }

    #[test]
    fn the_framework_sits_beside_the_executables_grandparent() {
        assert_eq!(
            cef_framework_dir(Path::new("/A/Astrofin.app/Contents/MacOS/astrofin")),
            PathBuf::from(
                "/A/Astrofin.app/Contents/Frameworks/Chromium Embedded Framework.framework"
            )
        );
    }

    #[test]
    fn an_executable_without_two_ancestors_is_its_own_base() {
        assert_eq!(
            cef_framework_dir(Path::new("astrofin")),
            PathBuf::from("astrofin/Frameworks/Chromium Embedded Framework.framework")
        );
    }

    #[test]
    fn a_menu_title_crosses_as_utf8_bytes() {
        assert_eq!(
            menu_c_string("About Astrofin").as_bytes(),
            b"About Astrofin"
        );
        assert_eq!(menu_c_string("").as_bytes(), b"");
    }

    #[test]
    fn a_title_with_an_interior_nul_becomes_empty() {
        assert_eq!(menu_c_string("Qu\0it").as_bytes(), b"");
    }
}
