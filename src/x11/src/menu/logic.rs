//! Popup placement and keysym lookup for the X11 context menu.
//!
//! Split out of [`super`], which keeps the popup thread, the override-redirect
//! window and the pointer/keyboard grab. Both pieces here are arithmetic over
//! numbers the caller has already read off the server.

use jfn_platform_abi::MenuPlacement;

use crate::scale_logic::positive_scale_or_one;

/// Everything placement needs to know about the world: where the app window is,
/// how big the screen is, and what a logical pixel is worth.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct PopupBounds {
    pub(crate) parent_x: i32,
    pub(crate) parent_y: i32,
    pub(crate) root_w: i32,
    pub(crate) root_h: i32,
    /// Physical pixels per logical pixel, already floored at 1.0.
    scale: f32,
}

impl PopupBounds {
    pub(crate) fn new(parent_x: i32, parent_y: i32, root_w: i32, root_h: i32, scale: f32) -> Self {
        Self {
            parent_x,
            parent_y,
            root_w,
            root_h,
            scale: positive_scale_or_one(scale),
        }
    }

    pub(crate) fn scale(&self) -> f32 {
        self.scale
    }
}

/// The popup's absolute origin: the anchor, scaled and offset by the app
/// window, then flipped or clamped to stay on the screen.
///
/// A popup that would run off the bottom prefers to open *upwards* from the
/// anchor — that is what a context menu near the taskbar has to do — and only
/// clamps to the screen edge when there is no room above either. Horizontally
/// there is nothing to flip to, so it just clamps.
pub(crate) fn place_popup(bounds: &PopupBounds, place: MenuPlacement) -> (i32, i32) {
    let (w, h) = (place.pw.max(1), place.ph.max(1));
    let scale = bounds.scale();
    let mut x = bounds.parent_x + (place.x as f32 * scale).round() as i32;
    let mut y = bounds.parent_y + (place.y as f32 * scale).round() as i32;
    if x + w > bounds.root_w {
        x = (bounds.root_w - w).max(0);
    }
    if y + h > bounds.root_h {
        let above = y - h;
        y = if above >= 0 {
            above
        } else {
            (bounds.root_h - h).max(0)
        };
    }
    (x.max(0), y.max(0))
}

/// The unshifted keysym for a keycode, from a `GetKeyboardMapping` reply.
///
/// The reply is a flat table of `per` syms per keycode starting at
/// `min_keycode`; 0 is "no symbol", which the menu treats as a key it does not
/// handle. Out-of-range keycodes and an empty table both read as 0.
pub(crate) fn keysym_for(min_keycode: u8, per: u8, syms: &[u32], keycode: u8) -> u32 {
    if per == 0 || keycode < min_keycode {
        return 0;
    }
    let idx = usize::from(keycode - min_keycode) * usize::from(per);
    syms.get(idx).copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(parent_x: i32, parent_y: i32, scale: f32) -> PopupBounds {
        PopupBounds::new(parent_x, parent_y, root_w(), root_h(), scale)
    }

    const fn root_w() -> i32 {
        1920
    }

    const fn root_h() -> i32 {
        1080
    }

    fn placement(x: i32, y: i32, pw: i32, ph: i32) -> MenuPlacement {
        MenuPlacement {
            x,
            y,
            lw: pw,
            lh: ph,
            pw,
            ph,
        }
    }

    #[test]
    fn an_anchor_is_scaled_then_offset_by_the_app_window() {
        let b = bounds(100, 50, 2.0);
        assert_eq!(place_popup(&b, placement(10, 20, 200, 300)), (120, 90));
    }

    #[test]
    fn an_unusable_scale_places_the_anchor_unscaled() {
        let b = bounds(0, 0, 0.0);
        assert_eq!(b.scale(), 1.0);
        assert_eq!(place_popup(&b, placement(10, 20, 200, 300)), (10, 20));
    }

    #[test]
    fn a_popup_running_off_the_right_edge_is_clamped() {
        let b = bounds(0, 0, 1.0);
        assert_eq!(place_popup(&b, placement(1900, 10, 200, 100)).0, 1720);
    }

    #[test]
    fn a_popup_running_off_the_bottom_opens_upwards() {
        let b = bounds(0, 0, 1.0);
        // 1000 + 300 > 1080, and 1000 - 300 fits: open above the anchor.
        assert_eq!(place_popup(&b, placement(10, 1000, 200, 300)).1, 700);
    }

    #[test]
    fn a_popup_too_tall_to_flip_is_clamped_to_the_bottom_edge() {
        let b = bounds(0, 0, 1.0);
        // 200 - 900 is off-screen above, so clamp to 1080 - 900.
        assert_eq!(place_popup(&b, placement(10, 200, 200, 900)).1, 180);
    }

    #[test]
    fn a_popup_taller_than_the_screen_still_lands_on_it() {
        let b = bounds(0, 0, 1.0);
        assert_eq!(place_popup(&b, placement(10, 200, 200, 4000)), (10, 0));
    }

    #[test]
    fn a_negative_anchor_is_pulled_back_onto_the_screen() {
        let b = bounds(-500, -400, 1.0);
        assert_eq!(place_popup(&b, placement(10, 20, 200, 100)), (0, 0));
    }

    #[test]
    fn a_zero_sized_popup_is_treated_as_one_pixel() {
        let b = bounds(0, 0, 1.0);
        // Placement must not divide by or clamp against a zero extent.
        assert_eq!(place_popup(&b, placement(1920, 1080, 0, 0)), (1919, 1079));
    }

    #[test]
    fn a_keycode_indexes_the_flat_mapping_table() {
        let syms = [0x61, 0x41, 0x62, 0x42, 0x63, 0x43];
        assert_eq!(keysym_for(8, 2, &syms, 8), 0x61);
        assert_eq!(keysym_for(8, 2, &syms, 9), 0x62);
        assert_eq!(keysym_for(8, 2, &syms, 10), 0x63);
    }

    #[test]
    fn a_keycode_below_the_minimum_has_no_symbol() {
        assert_eq!(keysym_for(8, 2, &[0x61, 0x41], 7), 0);
        assert_eq!(keysym_for(8, 2, &[0x61, 0x41], 0), 0);
    }

    #[test]
    fn a_keycode_past_the_table_has_no_symbol() {
        assert_eq!(keysym_for(8, 2, &[0x61, 0x41], 9), 0);
    }

    #[test]
    fn an_empty_mapping_reply_has_no_symbols() {
        assert_eq!(keysym_for(8, 0, &[], 8), 0);
        assert_eq!(keysym_for(8, 0, &[0x61], 8), 0);
    }
}
