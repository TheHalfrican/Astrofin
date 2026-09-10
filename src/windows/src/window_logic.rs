//! Pure window arithmetic and message routing for the Windows backend.
//!
//! `crate::window` and `crate::platform` do the Win32 queries; the rect
//! arithmetic, the DPI conversion, the fullscreen style test and the WndProc
//! routing table live here, where they are input -> output and testable
//! without an `HWND`.

use jfn_platform_abi::geometry::Bounds;
use jfn_platform_abi::{PhysicalSize, Scale, WindowExtent, WindowPos, WindowSnapshot};
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{
    SIZE_MINIMIZED, WM_CLOSE, WM_DPICHANGED, WM_MOVE, WM_SIZE, WM_STYLECHANGED, WS_CAPTION,
    WS_THICKFRAME,
};

/// Physical pixels per logical pixel for a window DPI. A DPI of zero means
/// "unknown" — Win32 returns it for a window that has no monitor yet — and
/// maps to an unscaled 1.0.
pub(crate) fn scale_from_dpi(dpi: u32) -> f32 {
    if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 }
}

/// The size a `RECT` spans.
pub(crate) fn rect_size(rc: RECT) -> PhysicalSize {
    PhysicalSize {
        w: rc.right - rc.left,
        h: rc.bottom - rc.top,
    }
}

/// The client size of a window, or `None` when the rect is degenerate — a
/// minimized window reports a 0x0 client area, which must not overwrite the
/// last real sample.
pub(crate) fn client_size(rc: RECT) -> Option<PhysicalSize> {
    let size = rect_size(rc);
    (size.w > 0 && size.h > 0).then_some(size)
}

/// A window's position relative to the monitor's working area (which excludes
/// the taskbar) — mpv's own `--geometry +X+Y` coordinate system on Windows.
pub(crate) fn position_in_work_area(window: RECT, work: RECT) -> WindowPos {
    WindowPos {
        x: window.left - work.left,
        y: window.top - work.top,
    }
}

/// The clamp bounds a working-area rect names.
pub(crate) fn work_area_bounds(work: RECT) -> Bounds {
    let size = rect_size(work);
    Bounds {
        w: size.w,
        h: size.h,
    }
}

/// True when a window style has neither `WS_CAPTION` nor `WS_THICKFRAME`,
/// which is exactly mpv's fullscreen style.
///
/// `update_style` in `third_party/mpv/video/out/w32_common.c` keeps
/// `WS_THICKFRAME` in its borderless-windowed (NO_FRAME) set and clears it
/// only for fullscreen, and mpv owns a top-level window here (no `--wid`), so
/// the early-out for embedded windows never applies.
pub(crate) fn is_fullscreen_style(style: u32) -> bool {
    (style & WS_CAPTION.0) == 0 && (style & WS_THICKFRAME.0) == 0
}

/// One window sample. A fullscreen window is never reported as maximized:
/// `IsZoomed` still says yes for a window that was maximized before it went
/// fullscreen, and the persisted `maximized` flag is what the window is
/// restored to.
pub(crate) fn snapshot(
    extent: WindowExtent,
    position: Option<WindowPos>,
    fullscreen: bool,
    zoomed: bool,
) -> WindowSnapshot {
    WindowSnapshot {
        extent: Some(extent),
        position,
        maximized: !fullscreen && zoomed,
        fullscreen,
    }
}

/// True when this sample's extent differs from the one it replaced — the gate
/// on the one debug line per resize. A first sample always differs.
pub(crate) fn extent_changed(previous: Option<WindowSnapshot>, extent: WindowExtent) -> bool {
    previous.and_then(|p| p.extent) != Some(extent)
}

/// The whole extent of one sample: client size plus the window's DPI.
pub(crate) fn extent_from(client: PhysicalSize, dpi: u32) -> WindowExtent {
    WindowExtent::new(client, Scale(scale_from_dpi(dpi)))
}

/// What one message seen by the WndProc hook means to the backend.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum WindowMessage {
    /// The window went to the taskbar: playback stops painting.
    Minimized,
    /// A resize or a restore: resample, resize the input child, and unhide.
    Resized,
    /// A move: resample the position, nothing else.
    Moved,
    /// DPI or style changed: resample and wake the consumers.
    Rescaled,
    /// The user closed mpv's window.
    Closed,
    /// Every other message the hook sees.
    Ignored,
}

/// Route one message from mpv's window. `wparam` is only read for `WM_SIZE`,
/// where it separates a minimize from every other size change.
pub(crate) fn classify(message: u32, wparam: usize) -> WindowMessage {
    match message {
        WM_SIZE if wparam == SIZE_MINIMIZED as usize => WindowMessage::Minimized,
        WM_SIZE => WindowMessage::Resized,
        WM_MOVE => WindowMessage::Moved,
        WM_DPICHANGED | WM_STYLECHANGED => WindowMessage::Rescaled,
        WM_CLOSE => WindowMessage::Closed,
        _ => WindowMessage::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        SIZE_MAXIMIZED, SIZE_RESTORED, WM_PAINT, WS_OVERLAPPEDWINDOW,
    };

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn ninety_six_dpi_is_unscaled() {
        assert_eq!(scale_from_dpi(96), 1.0);
        assert_eq!(scale_from_dpi(192), 2.0);
        assert_eq!(scale_from_dpi(144), 1.5);
    }

    #[test]
    fn an_unknown_dpi_falls_back_to_one() {
        assert_eq!(scale_from_dpi(0), 1.0);
    }

    #[test]
    fn rect_size_is_the_span_not_the_corners() {
        assert_eq!(
            rect_size(rect(10, 20, 110, 220)),
            PhysicalSize { w: 100, h: 200 }
        );
    }

    #[test]
    fn a_client_rect_with_area_is_a_size() {
        assert_eq!(
            client_size(rect(0, 0, 1280, 720)),
            Some(PhysicalSize { w: 1280, h: 720 })
        );
    }

    #[test]
    fn a_minimized_window_has_no_client_size() {
        assert_eq!(client_size(rect(0, 0, 0, 0)), None);
        assert_eq!(client_size(rect(0, 0, 1280, 0)), None);
        assert_eq!(client_size(rect(0, 0, 0, 720)), None);
    }

    #[test]
    fn position_is_measured_from_the_work_area_origin() {
        // A work area that starts below a top-docked taskbar.
        let work = rect(0, 40, 1920, 1080);
        let window = rect(100, 140, 900, 640);
        assert_eq!(
            position_in_work_area(window, work),
            WindowPos { x: 100, y: 100 }
        );
    }

    #[test]
    fn a_window_above_the_work_area_gets_a_negative_position() {
        let work = rect(0, 40, 1920, 1080);
        assert_eq!(
            position_in_work_area(rect(-10, 0, 100, 100), work),
            WindowPos { x: -10, y: -40 }
        );
    }

    #[test]
    fn clamp_bounds_are_the_work_areas_own_span() {
        assert_eq!(
            work_area_bounds(rect(0, 40, 1920, 1080)),
            Bounds { w: 1920, h: 1040 }
        );
    }

    #[test]
    fn a_style_without_caption_or_frame_is_fullscreen() {
        assert!(is_fullscreen_style(0));
        assert!(!is_fullscreen_style(WS_OVERLAPPEDWINDOW.0));
        assert!(!is_fullscreen_style(WS_CAPTION.0));
        assert!(!is_fullscreen_style(WS_THICKFRAME.0));
    }

    #[test]
    fn an_extent_carries_the_dpi_scale() {
        let extent = extent_from(PhysicalSize { w: 2560, h: 1440 }, 192);
        assert_eq!(extent.physical(), PhysicalSize { w: 2560, h: 1440 });
        assert_eq!(extent.scale(), Scale(2.0));
        assert_eq!(extent.logical().w, 1280);
    }

    #[test]
    fn a_fullscreen_window_is_never_reported_maximized() {
        let extent = extent_from(PhysicalSize { w: 1920, h: 1080 }, 96);
        let snap = snapshot(extent, None, true, true);
        assert!(snap.fullscreen);
        assert!(!snap.maximized);
    }

    #[test]
    fn a_zoomed_window_is_maximized_and_keeps_its_position() {
        let extent = extent_from(PhysicalSize { w: 1920, h: 1080 }, 96);
        let pos = WindowPos { x: 4, y: 8 };
        let snap = snapshot(extent, Some(pos), false, true);
        assert!(snap.maximized);
        assert_eq!(snap.position, Some(pos));
        assert_eq!(snap.extent, Some(extent));
    }

    #[test]
    fn the_first_sample_always_counts_as_a_change() {
        let extent = extent_from(PhysicalSize { w: 800, h: 600 }, 96);
        assert!(extent_changed(None, extent));
    }

    #[test]
    fn resampling_the_same_extent_is_not_a_change() {
        let extent = extent_from(PhysicalSize { w: 800, h: 600 }, 96);
        let same = snapshot(extent, None, false, false);
        assert!(!extent_changed(Some(same), extent));

        let bigger = extent_from(PhysicalSize { w: 801, h: 600 }, 96);
        assert!(extent_changed(Some(same), bigger));
    }

    #[test]
    fn a_minimize_is_told_apart_from_every_other_size() {
        assert_eq!(
            classify(WM_SIZE, SIZE_MINIMIZED as usize),
            WindowMessage::Minimized
        );
        assert_eq!(
            classify(WM_SIZE, SIZE_RESTORED as usize),
            WindowMessage::Resized
        );
        assert_eq!(
            classify(WM_SIZE, SIZE_MAXIMIZED as usize),
            WindowMessage::Resized
        );
    }

    #[test]
    fn moves_scale_changes_and_closes_each_route_to_their_own_action() {
        assert_eq!(classify(WM_MOVE, 0), WindowMessage::Moved);
        assert_eq!(classify(WM_DPICHANGED, 0), WindowMessage::Rescaled);
        assert_eq!(classify(WM_STYLECHANGED, 0), WindowMessage::Rescaled);
        assert_eq!(classify(WM_CLOSE, 0), WindowMessage::Closed);
    }

    #[test]
    fn every_other_message_is_ignored() {
        assert_eq!(classify(WM_PAINT, 0), WindowMessage::Ignored);
        assert_eq!(classify(0, 0), WindowMessage::Ignored);
    }
}
