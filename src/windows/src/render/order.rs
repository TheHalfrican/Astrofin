//! Pure parts of the presenter: the restack ordering and the two frame
//! validity tests. No DirectComposition object is touched here, so the rules
//! that decide *what* the visual tree should look like can be tested without
//! a GPU.

use std::ffi::c_int;

use jfn_gpu_paint::FrameSize;
use jfn_platform_abi::{PhysicalSize, Scale};

/// Reorder `entries` bottom-to-top to match `ordered`.
///
/// `key` names each entry's surface id. Entries `ordered` names come first,
/// in its order; every live entry it does not name keeps its relative order
/// above those it does, so a restack never drops a surface out of the tree.
pub(crate) fn restack<T>(entries: Vec<T>, key: impl Fn(&T) -> u64, ordered: &[u64]) -> Vec<T> {
    let rank = |e: &T| ordered.iter().position(|id| *id == key(e));
    let (mut named, mut unnamed): (Vec<T>, Vec<T>) =
        entries.into_iter().partition(|e| rank(e).is_some());
    named.sort_by_key(|e| rank(e).unwrap_or(usize::MAX));
    named.append(&mut unnamed);
    named
}

/// The frame size of a software paint, or `None` when there is nothing to
/// present — an empty pixel buffer or a degenerate size, either of which
/// would build a swapchain that can never be used.
pub(crate) fn software_frame_size(size: PhysicalSize, pixels: &[u8]) -> Option<FrameSize> {
    if pixels.is_empty() || size.w <= 0 || size.h <= 0 {
        return None;
    }
    Some(FrameSize {
        w: size.w,
        h: size.h,
    })
}

/// Bytes per row of a tightly packed BGRA frame.
pub(crate) fn bgra_stride(size: FrameSize) -> u32 {
    size.w as u32 * 4
}

/// The popup visual's offset inside its owning surface, in physical pixels,
/// from the logical position CEF places it at. An unknown or non-positive
/// scale is treated as 1:1.
pub(crate) fn popup_offset(x: c_int, y: c_int, scale: Option<Scale>) -> (f32, f32) {
    let scale = scale.unwrap_or(Scale(1.0)).or_one().0;
    (x as f32 * scale, y as f32 * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(live: &[u64], ordered: &[u64]) -> Vec<u64> {
        restack(live.to_vec(), |id| *id, ordered)
    }

    #[test]
    fn a_full_order_is_applied_verbatim() {
        assert_eq!(order(&[3, 1, 2], &[1, 2, 3]), vec![1, 2, 3]);
    }

    #[test]
    fn surfaces_the_order_omits_stay_on_top_in_their_own_order() {
        assert_eq!(order(&[1, 2, 3, 4], &[3, 1]), vec![3, 1, 2, 4]);
    }

    #[test]
    fn an_empty_order_leaves_every_surface_where_it_was() {
        assert_eq!(order(&[1, 2, 3], &[]), vec![1, 2, 3]);
    }

    #[test]
    fn ids_that_name_no_live_surface_are_ignored() {
        assert_eq!(order(&[1, 2], &[9, 2, 8, 1]), vec![2, 1]);
    }

    #[test]
    fn restacking_nothing_yields_nothing() {
        assert!(order(&[], &[1, 2]).is_empty());
    }

    #[test]
    fn a_software_frame_with_pixels_has_a_size() {
        let px = vec![0u8; 16];
        assert_eq!(
            software_frame_size(PhysicalSize { w: 2, h: 2 }, &px),
            Some(FrameSize { w: 2, h: 2 })
        );
    }

    #[test]
    fn an_empty_or_degenerate_software_frame_is_refused() {
        let px = vec![0u8; 16];
        assert_eq!(software_frame_size(PhysicalSize { w: 2, h: 2 }, &[]), None);
        assert_eq!(software_frame_size(PhysicalSize { w: 0, h: 2 }, &px), None);
        assert_eq!(software_frame_size(PhysicalSize { w: 2, h: 0 }, &px), None);
        assert_eq!(software_frame_size(PhysicalSize { w: -1, h: 2 }, &px), None);
    }

    #[test]
    fn a_bgra_row_is_four_bytes_per_pixel() {
        assert_eq!(bgra_stride(FrameSize { w: 1920, h: 1080 }), 7680);
        assert_eq!(bgra_stride(FrameSize { w: 0, h: 0 }), 0);
    }

    #[test]
    fn the_popup_offset_scales_with_the_window() {
        assert_eq!(popup_offset(10, 20, Some(Scale(2.0))), (20.0, 40.0));
        assert_eq!(popup_offset(10, 20, Some(Scale(1.0))), (10.0, 20.0));
    }

    #[test]
    fn an_unknown_or_bogus_scale_places_the_popup_one_to_one() {
        assert_eq!(popup_offset(10, 20, None), (10.0, 20.0));
        assert_eq!(popup_offset(10, 20, Some(Scale(0.0))), (10.0, 20.0));
        assert_eq!(popup_offset(10, 20, Some(Scale(-2.0))), (10.0, 20.0));
    }
}
