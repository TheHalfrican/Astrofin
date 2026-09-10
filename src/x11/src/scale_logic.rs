//! Pure scale arithmetic behind [`crate::scale`] and every X11 caller of the
//! published scale.
//!
//! Nothing here touches a connection or a snapshot: the DPI readings arrive as
//! plain numbers, so the quantization ladder that has to match mpv's historical
//! behaviour (`third_party/mpv/video/out/x11_common.c`) is pinned by tests
//! rather than by a live server.

const BASE_DPI: f64 = 96.0;

/// Quantize a DPI reading to mpv's half-step scale ladder. `None` for an
/// unscaled (1.0), absurd (>= 10.0) or non-finite reading, which the probe
/// treats as "no opinion" and falls through.
pub(crate) fn quantize_dpi(dpi: f64) -> Option<f32> {
    let s = quantize_dpi_steps(dpi)?;
    Some(s as f32 / 2.0)
}

/// The same ladder in half-steps: 3 = 1.5x, 4 = 2.0x, ... Kept separate so the
/// two-axis screen probe can compare steps without going through floats twice.
pub(crate) fn quantize_dpi_steps(dpi: f64) -> Option<i32> {
    if !dpi.is_finite() {
        return None;
    }
    let s = (2.0 * dpi / BASE_DPI).clamp(0.0, 20.0).round_ties_even() as i32;
    if s > 2 && s < 20 { Some(s) } else { None }
}

/// Scale from a screen's pixel and millimetre extent, which X reports per axis.
/// `None` unless both axes are measurable AND land on the same half-step — a
/// screen whose axes disagree is reporting a physical size we do not trust.
pub(crate) fn screen_dpi_scale(w_px: u16, h_px: u16, w_mm: u16, h_mm: u16) -> Option<f32> {
    if w_mm == 0 || h_mm == 0 {
        return None;
    }
    let dpi_x = f64::from(w_px) * 25.4 / f64::from(w_mm);
    let dpi_y = f64::from(h_px) * 25.4 / f64::from(h_mm);
    if !dpi_x.is_finite() || !dpi_y.is_finite() {
        return None;
    }
    let sx = quantize_dpi_steps(dpi_x)?;
    let sy = quantize_dpi_steps(dpi_y)?;
    if sx == sy {
        Some(sx as f32 / 2.0)
    } else {
        None
    }
}

/// A published scale is only usable when positive; a zero (never published) or
/// negative one reads as unscaled. Every consumer of
/// [`crate::x11_state::ParentSnapshot::scale`] goes through this.
pub(crate) fn positive_scale_or_one(scale: f32) -> f32 {
    if scale > 0.0 { scale } else { 1.0 }
}

/// Physical event coordinate to the logical one CEF is laid out in.
pub(crate) fn logical_from_physical(physical: i32, scale: f32) -> i32 {
    let s = f64::from(positive_scale_or_one(scale));
    (f64::from(physical) / s) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xft_dpi_uses_mpv_half_step_quantization() {
        assert_eq!(quantize_dpi(144.0), Some(1.5));
        assert_eq!(quantize_dpi(168.0), Some(2.0));
        assert_eq!(quantize_dpi(192.0), Some(2.0));
        assert_eq!(quantize_dpi(288.0), Some(3.0));
    }

    #[test]
    fn unscaled_or_invalid_dpi_is_ignored() {
        assert_eq!(quantize_dpi(96.0), None);
        assert_eq!(quantize_dpi(120.0), None);
        assert_eq!(quantize_dpi(0.0), None);
        assert_eq!(quantize_dpi(f64::NAN), None);
    }

    #[test]
    fn a_step_at_either_end_of_the_ladder_is_rejected() {
        // 2 half-steps is 1.0x (nothing to do) and 20 is the clamp ceiling.
        assert_eq!(quantize_dpi_steps(96.0), None);
        assert_eq!(quantize_dpi_steps(960.0), None);
        assert_eq!(quantize_dpi_steps(f64::INFINITY), None);
        assert_eq!(quantize_dpi_steps(144.0), Some(3));
    }

    #[test]
    fn screen_scale_needs_both_axes_on_the_same_step() {
        // 3840x2160 on a 508x286 mm panel: both axes read ~192 dpi.
        assert_eq!(screen_dpi_scale(3840, 2160, 508, 286), Some(2.0));
    }

    #[test]
    fn screen_scale_rejects_axes_that_disagree() {
        assert_eq!(screen_dpi_scale(3840, 2160, 508, 572), None);
    }

    #[test]
    fn screen_without_a_physical_size_has_no_scale() {
        assert_eq!(screen_dpi_scale(3840, 2160, 0, 286), None);
        assert_eq!(screen_dpi_scale(3840, 2160, 508, 0), None);
    }

    #[test]
    fn a_zero_or_negative_scale_falls_back_to_one() {
        assert_eq!(positive_scale_or_one(0.0), 1.0);
        assert_eq!(positive_scale_or_one(-2.0), 1.0);
        assert_eq!(positive_scale_or_one(1.5), 1.5);
    }

    #[test]
    fn logical_coordinates_divide_by_the_scale() {
        assert_eq!(logical_from_physical(300, 1.5), 200);
        assert_eq!(logical_from_physical(-300, 1.5), -200);
        assert_eq!(logical_from_physical(0, 2.0), 0);
    }

    #[test]
    fn logical_coordinates_pass_through_at_an_unusable_scale() {
        assert_eq!(logical_from_physical(300, 0.0), 300);
        assert_eq!(logical_from_physical(300, -1.0), 300);
    }
}
