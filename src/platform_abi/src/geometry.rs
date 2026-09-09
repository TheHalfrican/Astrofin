//! Window-geometry value types + on-screen clamping.
//!
//! The clamp algorithm was byte-identical in `macos_clamp_window_geometry`
//! and `win_clamp_window_geometry`; only the OS bounds query differed
//! (`NSScreen.visibleFrame * scale` vs `SPI_GETWORKAREA`). That query stays
//! platform-side and hands the resolved [`Bounds`] in, so the shared logic
//! is testable on any host.

use std::ffi::c_int;

/// HiDPI scale factor (physical pixels per logical pixel).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Scale(pub f32);

impl Scale {
    /// Replace a non-positive (unknown) scale with 1.0.
    pub fn or_one(self) -> Self {
        if self.0 > 0.0 { self } else { Scale(1.0) }
    }
}

/// Window size in logical (DIP) pixels — the coordinate space the compositor
/// uses for the toplevel; the display scale maps it to physical pixels.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LogicalSize {
    pub w: c_int,
    pub h: c_int,
}

/// Window size in physical (backing) pixels — what mpv's `--geometry` takes and
/// what gets persisted as `windowWidth/Height`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PhysicalSize {
    pub w: c_int,
    pub h: c_int,
}

impl LogicalSize {
    pub fn to_physical(self, s: Scale) -> PhysicalSize {
        let s = s.or_one().0;
        PhysicalSize {
            w: (self.w as f32 * s).round() as c_int,
            h: (self.h as f32 * s).round() as c_int,
        }
    }
}

impl PhysicalSize {
    pub fn to_logical(self, s: Scale) -> LogicalSize {
        let s = s.or_one().0;
        LogicalSize {
            w: (self.w as f32 / s).round() as c_int,
            h: (self.h as f32 / s).round() as c_int,
        }
    }
}

/// A point in physical (backing) pixels, relative to the window's client
/// origin — what Win32 mouse messages and X11 pointer events carry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PhysicalPoint {
    pub x: c_int,
    pub y: c_int,
}

/// A point in logical (DIP) pixels — the space [`WindowExtent::logical`]
/// names, and the space CEF's view coordinates are in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LogicalPoint {
    pub x: c_int,
    pub y: c_int,
}

/// A coherent (logical, physical, scale) triple. [`WindowExtent::new`]
/// derives the logical size by division; [`WindowExtent::with_logical`]
/// preserves a producer's exact logical size, which division at fractional
/// scales cannot reproduce.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct WindowExtent {
    logical: LogicalSize,
    physical: PhysicalSize,
    scale: Scale,
}

impl WindowExtent {
    pub fn new(physical: PhysicalSize, scale: Scale) -> Self {
        Self {
            logical: physical.to_logical(scale),
            physical,
            scale,
        }
    }

    pub fn with_logical(physical: PhysicalSize, scale: Scale, logical: LogicalSize) -> Self {
        Self {
            logical,
            physical,
            scale,
        }
    }

    pub fn logical(&self) -> LogicalSize {
        self.logical
    }

    pub fn physical(&self) -> PhysicalSize {
        self.physical
    }

    pub fn scale(&self) -> Scale {
        self.scale
    }

    /// Map a pointer position into the space this extent's logical size names.
    ///
    /// Maps through this extent's own logical:physical ratio per axis, so it
    /// is the exact inverse of the size handed to CEF — including the exact
    /// logical size of [`WindowExtent::with_logical`], which division by
    /// [`WindowExtent::scale`] cannot reproduce. Floors toward negative
    /// infinity, so a point dragged past the client origin stays monotone. A
    /// degenerate physical extent maps to the identity.
    pub fn to_logical_point(&self, p: PhysicalPoint) -> LogicalPoint {
        LogicalPoint {
            x: map_axis(p.x, self.logical.w, self.physical.w),
            y: map_axis(p.y, self.logical.h, self.physical.h),
        }
    }
}

/// `v * logical / physical`, floored toward negative infinity; the identity
/// when `physical` is not positive.
fn map_axis(v: c_int, logical: c_int, physical: c_int) -> c_int {
    if physical <= 0 {
        return v;
    }
    let num = i64::from(v) * i64::from(logical);
    let den = i64::from(physical);
    let mut q = num / den;
    if num % den != 0 && (num < 0) {
        q -= 1;
    }
    q as c_int
}

/// Fully-resolved boot geometry: one typed value computed once from saved
/// config, consumed by `Platform::apply_boot_geometry` (logical) and mpv's
/// `--geometry` (physical).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BootGeometry {
    logical: LogicalSize,
    physical: PhysicalSize,
    scale: Scale,
    /// `None` ⇒ let the window center (Wayland ignores position entirely).
    position: Option<WindowPos>,
    maximized: bool,
}

impl BootGeometry {
    /// The one constructor: `physical` and `position` are both taken from a
    /// single already-clamped [`WindowGeometry`], so they cannot disagree with
    /// each other or be set independently of the clamp. `scale` is the factor
    /// that produced `clamped` from `logical`.
    pub fn from_clamped(
        logical: LogicalSize,
        scale: Scale,
        clamped: WindowGeometry,
        maximized: bool,
    ) -> Self {
        Self {
            logical,
            physical: PhysicalSize {
                w: clamped.w,
                h: clamped.h,
            },
            scale,
            position: clamped.position,
            maximized,
        }
    }

    pub fn logical(&self) -> LogicalSize {
        self.logical
    }

    pub fn physical(&self) -> PhysicalSize {
        self.physical
    }

    pub fn scale(&self) -> Scale {
        self.scale
    }

    pub fn position(&self) -> Option<WindowPos> {
        self.position
    }

    pub fn maximized(&self) -> bool {
        self.maximized
    }

    /// mpv `--geometry`: `"<W>x<H>"` or `"<W>x<H>+<X>+<Y>"`, physical pixels.
    pub fn mpv_geometry_string(&self) -> String {
        let mut s = format!("{}x{}", self.physical.w, self.physical.h);
        if let Some(p) = self.position {
            s.push_str(&format!("+{}+{}", p.x, p.y));
        }
        s
    }

    pub fn force_position(&self) -> bool {
        self.position.is_some()
    }
}

/// Working-area dimensions — excludes the menu bar / dock / taskbar — in the
/// same pixel space (backing pixels) as the geometry being clamped.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub w: c_int,
    pub h: c_int,
}

/// Saved window geometry: size plus an optional top-left position. `None`
/// position asks [`clamp_to_bounds`] to center the window (mpv's own centering
/// misbehaves when only the width/height are overridden).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WindowGeometry {
    pub w: c_int,
    pub h: c_int,
    pub position: Option<WindowPos>,
}

impl WindowGeometry {
    /// Build from raw coordinates where a negative `x` or `y` means "unset".
    /// The single home for that OS/config-facing sentinel convention.
    pub fn from_raw(w: c_int, h: c_int, x: c_int, y: c_int) -> Self {
        Self {
            w,
            h,
            position: (x >= 0 && y >= 0).then_some(WindowPos { x, y }),
        }
    }

    /// Raw coordinates for OS APIs that take a sentinel; `(-1, -1)` when unset.
    pub fn raw_position(&self) -> (c_int, c_int) {
        self.position.map_or((-1, -1), |p| (p.x, p.y))
    }
}

/// A window's top-left position, in the coordinate space the backend
/// reports (backing pixels relative to the working area). Returned by
/// `Platform::query_window_position`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WindowPos {
    pub x: c_int,
    pub y: c_int,
}

/// A surface resize request: logical (DIP) and physical (pixel) dimensions.
/// Carried as one struct through `Platform::surface_resize` so adding a
/// field later doesn't change the method's arity (and so doesn't churn
/// every backend + call site).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SurfaceSize {
    pub logical_w: c_int,
    pub logical_h: c_int,
    pub physical_w: c_int,
    pub physical_h: c_int,
}

/// Clamp `g` so the window stays fully within `bounds`: shrink oversized
/// dimensions, center any unset (negative) axis, pull a past-the-edge window
/// back in-bounds, then floor at the origin. Byte-for-byte the former
/// per-platform clamp.
pub fn clamp_to_bounds(g: &mut WindowGeometry, bounds: Bounds) {
    let vw = bounds.w;
    let vh = bounds.h;
    if g.w > vw {
        g.w = vw;
    }
    if g.h > vh {
        g.h = vh;
    }
    // Center an unset position; otherwise start from the requested one.
    let (mut x, mut y) = match g.position {
        Some(p) => (p.x, p.y),
        None => ((vw - g.w) / 2, (vh - g.h) / 2),
    };
    if x + g.w > vw {
        x = vw - g.w;
    }
    if y + g.h > vh {
        y = vh - g.h;
    }
    if x < 0 {
        x = 0;
    }
    if y < 0 {
        y = 0;
    }
    g.position = Some(WindowPos { x, y });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_physical_round_trip() {
        for (logical, scale, physical) in [
            (
                LogicalSize { w: 1280, h: 720 },
                1.0,
                PhysicalSize { w: 1280, h: 720 },
            ),
            (
                LogicalSize { w: 1280, h: 720 },
                1.25,
                PhysicalSize { w: 1600, h: 900 },
            ),
            (
                LogicalSize { w: 1600, h: 900 },
                1.5,
                PhysicalSize { w: 2400, h: 1350 },
            ),
            (
                LogicalSize { w: 1280, h: 720 },
                2.0,
                PhysicalSize { w: 2560, h: 1440 },
            ),
        ] {
            assert_eq!(logical.to_physical(Scale(scale)), physical);
            assert_eq!(physical.to_logical(Scale(scale)), logical);
        }
    }

    #[test]
    fn scale_or_one_guards_nonpositive() {
        assert_eq!(Scale(0.0).or_one(), Scale(1.0));
        assert_eq!(Scale(-2.0).or_one(), Scale(1.0));
        assert_eq!(Scale(1.5).or_one(), Scale(1.5));
        assert_eq!(
            LogicalSize { w: 800, h: 600 }.to_physical(Scale(0.0)),
            PhysicalSize { w: 800, h: 600 }
        );
    }

    #[test]
    fn mpv_geometry_string_with_and_without_position() {
        let logical = LogicalSize { w: 1280, h: 720 };
        let base = BootGeometry::from_clamped(
            logical,
            Scale(1.25),
            WindowGeometry::from_raw(1600, 900, -1, -1),
            false,
        );
        assert_eq!(base.mpv_geometry_string(), "1600x900");
        assert!(!base.force_position());

        let positioned = BootGeometry::from_clamped(
            logical,
            Scale(1.25),
            WindowGeometry::from_raw(1600, 900, 100, 50),
            false,
        );
        assert_eq!(positioned.mpv_geometry_string(), "1600x900+100+50");
        assert!(positioned.force_position());
    }

    const SCREEN: Bounds = Bounds { w: 1920, h: 1080 };

    fn pos(g: &WindowGeometry) -> (c_int, c_int) {
        g.raw_position()
    }

    #[test]
    fn fits_unchanged() {
        let mut g = WindowGeometry::from_raw(800, 600, 100, 50);
        clamp_to_bounds(&mut g, SCREEN);
        assert_eq!(g, WindowGeometry::from_raw(800, 600, 100, 50));
    }

    #[test]
    fn oversized_shrinks_to_bounds() {
        let mut g = WindowGeometry::from_raw(3000, 2000, 0, 0);
        clamp_to_bounds(&mut g, SCREEN);
        assert_eq!(g.w, 1920);
        assert_eq!(g.h, 1080);
    }

    #[test]
    fn unset_axes_center() {
        let mut g = WindowGeometry::from_raw(800, 600, -1, -1);
        clamp_to_bounds(&mut g, SCREEN);
        assert_eq!(pos(&g), ((1920 - 800) / 2, (1080 - 600) / 2));
    }

    #[test]
    fn past_edge_pulls_back() {
        let mut g = WindowGeometry::from_raw(800, 600, 1500, 900);
        clamp_to_bounds(&mut g, SCREEN);
        assert_eq!(pos(&g), (1920 - 800, 1080 - 600));
    }

    #[test]
    fn oversized_then_floored_at_origin() {
        // Oversized window: shrink to bounds, center (negative → 0 after
        // edge-adjust + floor).
        let mut g = WindowGeometry::from_raw(3000, 2000, -1, -1);
        clamp_to_bounds(&mut g, SCREEN);
        assert_eq!(g, WindowGeometry::from_raw(1920, 1080, 0, 0));
    }

    #[test]
    fn an_extent_derives_its_logical_size_from_the_scale() {
        let e = WindowExtent::new(PhysicalSize { w: 2560, h: 1440 }, Scale(2.0));
        assert_eq!(e.physical(), PhysicalSize { w: 2560, h: 1440 });
        assert_eq!(e.logical(), LogicalSize { w: 1280, h: 720 });
        assert_eq!(e.scale(), Scale(2.0));
    }

    #[test]
    fn with_logical_keeps_a_logical_size_division_cannot_reproduce() {
        // 1601/1.25 rounds to 1281, so the producer's own 1280 only survives
        // because it is carried explicitly.
        let e = WindowExtent::with_logical(
            PhysicalSize { w: 1601, h: 900 },
            Scale(1.25),
            LogicalSize { w: 1280, h: 720 },
        );
        assert_eq!(e.logical(), LogicalSize { w: 1280, h: 720 });
        assert_ne!(
            e.logical(),
            WindowExtent::new(PhysicalSize { w: 1601, h: 900 }, Scale(1.25)).logical()
        );
    }

    #[test]
    fn a_pointer_maps_through_the_extents_own_ratio() {
        let e = WindowExtent::new(PhysicalSize { w: 2560, h: 1440 }, Scale(2.0));
        assert_eq!(
            e.to_logical_point(PhysicalPoint { x: 1000, y: 400 }),
            LogicalPoint { x: 500, y: 200 }
        );
        assert_eq!(
            e.to_logical_point(PhysicalPoint { x: 0, y: 0 }),
            LogicalPoint { x: 0, y: 0 }
        );
    }

    #[test]
    fn a_point_past_the_client_origin_floors_toward_negative_infinity() {
        let e = WindowExtent::new(PhysicalSize { w: 2560, h: 1440 }, Scale(2.0));
        // -1 physical is -0.5 logical: flooring keeps the mapping monotone
        // instead of snapping a dragged pointer back to 0.
        assert_eq!(
            e.to_logical_point(PhysicalPoint { x: -1, y: -3 }),
            LogicalPoint { x: -1, y: -2 }
        );
    }

    #[test]
    fn a_degenerate_extent_maps_points_unchanged() {
        let e = WindowExtent::with_logical(
            PhysicalSize { w: 0, h: -4 },
            Scale(1.0),
            LogicalSize { w: 800, h: 600 },
        );
        assert_eq!(
            e.to_logical_point(PhysicalPoint { x: 17, y: -9 }),
            LogicalPoint { x: 17, y: -9 }
        );
    }

    #[test]
    fn boot_geometry_reports_the_clamped_values_it_was_built_from() {
        let g = BootGeometry::from_clamped(
            LogicalSize { w: 1280, h: 720 },
            Scale(1.25),
            WindowGeometry::from_raw(1600, 900, 40, 50),
            true,
        );
        assert_eq!(g.logical(), LogicalSize { w: 1280, h: 720 });
        assert_eq!(g.physical(), PhysicalSize { w: 1600, h: 900 });
        assert_eq!(g.scale(), Scale(1.25));
        assert_eq!(g.position(), Some(WindowPos { x: 40, y: 50 }));
        assert!(g.maximized());
    }

    #[test]
    fn an_unset_boot_position_is_dropped_rather_than_defaulted() {
        let g = BootGeometry::from_clamped(
            LogicalSize { w: 800, h: 600 },
            Scale(1.0),
            WindowGeometry::from_raw(800, 600, -1, 5),
            false,
        );
        assert_eq!(g.position(), None);
        assert!(!g.maximized());
        assert_eq!(g.mpv_geometry_string(), "800x600");
    }

    #[test]
    fn a_partly_negative_raw_position_counts_as_unset() {
        assert_eq!(WindowGeometry::from_raw(800, 600, -1, 5).position, None);
        assert_eq!(WindowGeometry::from_raw(800, 600, 5, -1).position, None);
        assert_eq!(
            WindowGeometry::from_raw(800, 600, 0, 0).position,
            Some(WindowPos { x: 0, y: 0 })
        );
        assert_eq!(
            WindowGeometry::from_raw(800, 600, -1, -1).raw_position(),
            (-1, -1)
        );
    }

    #[test]
    fn a_scale_that_is_not_a_number_counts_as_unknown() {
        assert_eq!(Scale(f32::NAN).or_one(), Scale(1.0));
        assert_eq!(
            LogicalSize { w: 800, h: 600 }.to_physical(Scale(f32::NAN)),
            PhysicalSize { w: 800, h: 600 }
        );
    }

    #[test]
    fn converting_a_size_rounds_to_the_nearest_whole_pixel() {
        // 801 * 1.5 is 1201.5 — a half pixel rounds away from zero, not down.
        assert_eq!(
            LogicalSize { w: 801, h: 601 }.to_physical(Scale(1.5)),
            PhysicalSize { w: 1202, h: 902 }
        );
        assert_eq!(
            PhysicalSize { w: 1202, h: 902 }.to_logical(Scale(1.5)),
            LogicalSize { w: 801, h: 601 }
        );
    }

    #[test]
    fn a_window_exactly_filling_the_bounds_is_left_alone() {
        let mut g = WindowGeometry::from_raw(1920, 1080, 0, 0);
        clamp_to_bounds(&mut g, SCREEN);
        assert_eq!(g, WindowGeometry::from_raw(1920, 1080, 0, 0));
    }

    #[test]
    fn empty_bounds_collapse_the_window_onto_the_origin() {
        let mut g = WindowGeometry::from_raw(800, 600, 40, 50);
        clamp_to_bounds(&mut g, Bounds { w: 0, h: 0 });
        assert_eq!(g, WindowGeometry::from_raw(0, 0, 0, 0));
    }

    #[test]
    fn a_boot_position_at_the_origin_is_still_a_position() {
        let g = BootGeometry::from_clamped(
            LogicalSize { w: 800, h: 600 },
            Scale(1.0),
            WindowGeometry::from_raw(800, 600, 0, 0),
            false,
        );
        assert!(g.force_position());
        assert_eq!(g.mpv_geometry_string(), "800x600+0+0");
    }
}
