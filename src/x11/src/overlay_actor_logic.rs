//! Pixel-upload arithmetic for the overlay content actor.
//!
//! Split out of [`crate::overlay_actor`], which keeps the thread, the mailbox
//! and the two backends. Everything here is frame geometry: validating what CEF
//! handed us, clipping a damage rect to the frame, and turning a clipped rect
//! into the byte ranges the SHM row copy reads and writes.

use std::ops::Range;

use jfn_platform_abi::JfnRect;

/// A software frame's byte layout, once its extent has been proven usable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct SoftwareFrame {
    /// Bytes per row of the *source* buffer: BGRA, so four per pixel.
    pub(crate) stride: usize,
    /// Bytes of the source buffer the frame actually occupies. A producer may
    /// hand over a longer buffer; anything past this is not part of the frame.
    pub(crate) len: usize,
}

/// Validate an incoming software frame. `None` rejects it — a non-positive
/// extent, a byte count that does not fit a `usize`, or a buffer shorter than
/// the extent claims. The actor drops a rejected frame rather than presenting a
/// partial one.
pub(crate) fn software_frame(width: i32, height: i32, pixels_len: usize) -> Option<SoftwareFrame> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let stride = (width as usize).saturating_mul(4);
    let len = (height as usize).checked_mul(stride)?;
    if pixels_len < len {
        return None;
    }
    Some(SoftwareFrame { stride, len })
}

/// Clip a damage rect to the frame. `None` when nothing of it is left — a
/// zero/negative extent, or a rect entirely off the frame.
pub(crate) fn clip_rect(rect: &JfnRect, width: i32, height: i32) -> Option<(i32, i32, i32, i32)> {
    let mut rx = rect.x;
    let mut ry = rect.y;
    let mut rw = rect.w;
    let mut rh = rect.h;
    if rx < 0 {
        rw += rx;
        rx = 0;
    }
    if ry < 0 {
        rh += ry;
        ry = 0;
    }
    if rx + rw > width {
        rw = width - rx;
    }
    if ry + rh > height {
        rh = height - ry;
    }
    if rw <= 0 || rh <= 0 {
        return None;
    }
    Some((rx, ry, rw, rh))
}

/// Source and destination byte ranges for one row of a clipped damage rect.
/// The two strides differ: the source is CEF's buffer (which may be padded),
/// the destination the tightly-packed SHM segment.
pub(crate) fn row_ranges(
    rx: i32,
    ry: i32,
    row: i32,
    rw: i32,
    src_stride: usize,
    dst_stride: usize,
) -> (Range<usize>, Range<usize>) {
    let line = (ry + row) as usize;
    let column = (rx as usize) * 4;
    let row_bytes = (rw as usize) * 4;
    let src_off = line * src_stride + column;
    let dst_off = line * dst_stride + column;
    (src_off..src_off + row_bytes, dst_off..dst_off + row_bytes)
}

/// The other half of the double buffer.
pub(crate) fn next_buffer(idx: usize) -> usize {
    idx ^ 1
}

/// The depth `shm_put_image` is told to use. 32 when the ARGB visual has not
/// been resolved yet, which is the depth the overlays are created at anyway.
pub(crate) fn image_depth(argb_depth: Option<u8>) -> u8 {
    argb_depth.unwrap_or(32)
}

/// Whether a failed GPU surface creation should degrade this overlay to SHM.
///
/// Never for a shared (dmabuf) frame: SHM cannot present one, so degrading
/// would strand the surface with no output at all — that frame is dropped and
/// creation retried on the next one.
pub(crate) fn degrade_to_shm_on_gpu_init_failure(frame_is_shared: bool) -> bool {
    !frame_is_shared
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> JfnRect {
        JfnRect { x, y, w, h }
    }

    #[test]
    fn a_frame_is_four_bytes_per_pixel_by_its_height() {
        let f = software_frame(4, 3, 48).unwrap();
        assert_eq!(f.stride, 16);
        assert_eq!(f.len, 48);
    }

    #[test]
    fn a_longer_buffer_than_the_extent_is_accepted_and_trimmed() {
        let f = software_frame(4, 3, 4096).unwrap();
        assert_eq!(f.len, 48);
    }

    #[test]
    fn a_frame_shorter_than_its_extent_is_rejected() {
        assert_eq!(software_frame(4, 3, 47), None);
    }

    #[test]
    fn a_non_positive_extent_is_rejected() {
        assert_eq!(software_frame(0, 3, 4096), None);
        assert_eq!(software_frame(4, 0, 4096), None);
        assert_eq!(software_frame(-4, 3, 4096), None);
        assert_eq!(software_frame(4, -3, 4096), None);
    }

    #[test]
    fn an_extent_whose_byte_count_overflows_is_rejected() {
        assert_eq!(software_frame(i32::MAX, i32::MAX, usize::MAX), None);
    }

    #[test]
    fn clip_rect_clamps_negative_origin() {
        assert_eq!(clip_rect(&rect(-2, -2, 4, 4), 10, 10), Some((0, 0, 2, 2)));
    }

    #[test]
    fn clip_rect_clamps_overflow() {
        assert_eq!(clip_rect(&rect(8, 8, 10, 10), 10, 10), Some((8, 8, 2, 2)));
    }

    #[test]
    fn clip_rect_rejects_zero_and_off_screen() {
        assert_eq!(clip_rect(&rect(0, 0, 0, 5), 10, 10), None);
        assert_eq!(clip_rect(&rect(10, 0, 4, 4), 10, 10), None);
    }

    #[test]
    fn clip_rect_passes_through_in_bounds() {
        assert_eq!(clip_rect(&rect(1, 2, 3, 4), 10, 10), Some((1, 2, 3, 4)));
    }

    #[test]
    fn a_row_copy_skips_whole_lines_then_the_column_offset() {
        let (src, dst) = row_ranges(2, 3, 1, 5, 64, 40);
        // Line 4 of the source is 4 * 64 bytes in, plus 2 px of column.
        assert_eq!(src, 264..284);
        assert_eq!(dst, 168..188);
    }

    #[test]
    fn a_row_copy_at_the_origin_starts_at_byte_zero() {
        let (src, dst) = row_ranges(0, 0, 0, 4, 32, 16);
        assert_eq!(src, 0..16);
        assert_eq!(dst, 0..16);
    }

    #[test]
    fn the_double_buffer_alternates() {
        assert_eq!(next_buffer(0), 1);
        assert_eq!(next_buffer(1), 0);
    }

    #[test]
    fn an_unresolved_visual_still_puts_images_at_depth_32() {
        assert_eq!(image_depth(None), 32);
        assert_eq!(image_depth(Some(24)), 24);
    }

    #[test]
    fn a_shared_frame_never_degrades_the_surface_to_shm() {
        assert!(!degrade_to_shm_on_gpu_init_failure(true));
        assert!(degrade_to_shm_on_gpu_init_failure(false));
    }
}
