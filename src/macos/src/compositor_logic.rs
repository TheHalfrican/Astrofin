//! Pure sizing arithmetic for the `CAMetalLayer` compositor.
//!
//! The layer, the painter and the surface registry live in
//! `crate::compositor`; the point-to-backing-pixel conversions and the
//! software-frame checks it does on the way in are here, where they can be
//! exercised without a GPU.

use std::ffi::c_int;

use jfn_gpu_paint::FrameSize;
use jfn_platform_abi::PhysicalSize;
use objc2_foundation::NSRect;

/// A view's frame in backing pixels — the drawable size a `CAMetalLayer` at
/// `scale` needs to cover it. Truncates, matching the cast it replaces.
pub(crate) fn backing_size(frame: NSRect, scale: f64) -> FrameSize {
    FrameSize {
        w: (frame.size.width * scale) as c_int,
        h: (frame.size.height * scale) as c_int,
    }
}

/// The layer's `contentsScale` derived from one resize's physical and logical
/// widths, or `None` when the pair cannot name a ratio (either is zero or
/// negative, which happens while a surface is still unmapped). The caller then
/// falls back to the window's own backing scale.
pub(crate) fn contents_scale(physical_w: c_int, logical_w: c_int) -> Option<f64> {
    (physical_w > 0 && logical_w > 0).then(|| f64::from(physical_w) / f64::from(logical_w))
}

/// The row stride of a CEF `OnPaint` buffer, which is tightly packed BGRA, or
/// `None` when the frame cannot be presented at all: an empty buffer, or a
/// size with a non-positive dimension.
pub(crate) fn software_frame_stride(pixels: &[u8], size: PhysicalSize) -> Option<u32> {
    if pixels.is_empty() || size.w <= 0 || size.h <= 0 {
        return None;
    }
    Some(size.w as u32 * 4)
}

/// The paint crate's extent for a platform-ABI size. Both are `c_int` pairs;
/// the two crates just do not share a type.
pub(crate) fn frame_size(size: PhysicalSize) -> FrameSize {
    FrameSize {
        w: size.w,
        h: size.h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::{NSPoint, NSSize};

    fn rect(width: f64, height: f64) -> NSRect {
        NSRect {
            origin: NSPoint { x: 0.0, y: 0.0 },
            size: NSSize { width, height },
        }
    }

    #[test]
    fn a_retina_frame_doubles_into_backing_pixels() {
        assert_eq!(
            backing_size(rect(1280.0, 720.0), 2.0),
            FrameSize { w: 2560, h: 1440 }
        );
        assert_eq!(
            backing_size(rect(1280.0, 720.0), 1.0),
            FrameSize { w: 1280, h: 720 }
        );
    }

    #[test]
    fn a_fractional_backing_size_truncates() {
        assert_eq!(
            backing_size(rect(100.5, 100.5), 1.5),
            FrameSize { w: 150, h: 150 }
        );
        assert_eq!(backing_size(rect(0.0, 0.0), 2.0), FrameSize { w: 0, h: 0 });
    }

    #[test]
    fn the_contents_scale_is_the_physical_to_logical_ratio() {
        assert_eq!(contents_scale(2560, 1280), Some(2.0));
        assert_eq!(contents_scale(1280, 1280), Some(1.0));
        assert_eq!(contents_scale(2000, 1000), Some(2.0));
    }

    #[test]
    fn an_unusable_width_pair_names_no_scale() {
        assert_eq!(contents_scale(0, 1280), None);
        assert_eq!(contents_scale(2560, 0), None);
        assert_eq!(contents_scale(-1, -1), None);
    }

    #[test]
    fn a_packed_bgra_row_is_four_bytes_a_pixel() {
        let pixels = vec![0u8; 8 * 4 * 4];
        assert_eq!(
            software_frame_stride(&pixels, PhysicalSize { w: 8, h: 4 }),
            Some(32)
        );
    }

    #[test]
    fn an_empty_or_degenerate_software_frame_is_refused() {
        let pixels = vec![0u8; 16];
        assert_eq!(
            software_frame_stride(&[], PhysicalSize { w: 8, h: 4 }),
            None
        );
        assert_eq!(
            software_frame_stride(&pixels, PhysicalSize { w: 0, h: 4 }),
            None
        );
        assert_eq!(
            software_frame_stride(&pixels, PhysicalSize { w: 8, h: 0 }),
            None
        );
        assert_eq!(
            software_frame_stride(&pixels, PhysicalSize { w: -8, h: 4 }),
            None
        );
    }

    #[test]
    fn a_physical_size_crosses_into_the_paint_crates_extent() {
        assert_eq!(
            frame_size(PhysicalSize { w: 1920, h: 1080 }),
            FrameSize { w: 1920, h: 1080 }
        );
    }
}
