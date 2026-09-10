//! Pure decisions taken while the X11 host window is built and torn down.
//!
//! Split out of [`crate::lifecycle`], which keeps the connection, the window
//! creation and the property writes. Everything here is arithmetic or a
//! selection rule over values the caller has already read off the server.

use x11rb::protocol::xproto::{Screen, VisualClass};

use jfn_platform_abi::{BootGeometry, WindowPos};

use crate::x11_state::Atoms;

/// The default top-level extent in *logical* pixels, used when nothing was
/// persisted. Scaled by the probed display scale before it reaches the server.
const DEFAULT_LOGICAL: (f64, f64) = (1600.0, 900.0);

/// Where and how big the app top-level is created.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct BootPlacement {
    pub(crate) w: i32,
    pub(crate) h: i32,
    /// `None` leaves placement to the WM (and withholds the size hints that
    /// would otherwise override its policy).
    pub(crate) position: Option<WindowPos>,
    pub(crate) maximized: bool,
}

impl BootPlacement {
    /// Origin to create at; `(0, 0)` stands in when the WM is placing it.
    pub(crate) fn origin(&self) -> (i32, i32) {
        self.position.map_or((0, 0), |p| (p.x, p.y))
    }
}

/// Resolve the boot placement from a restored geometry, falling back to the
/// scaled default extent when there is nothing to restore. A restored extent is
/// floored at 1 in each axis: X rejects a zero-sized window.
pub(crate) fn boot_placement(boot: Option<BootGeometry>, scale: f32) -> BootPlacement {
    let (w, h) = boot.map_or_else(
        || {
            let s = f64::from(scale);
            (
                (DEFAULT_LOGICAL.0 * s) as i32,
                (DEFAULT_LOGICAL.1 * s) as i32,
            )
        },
        |b| (b.physical().w.max(1), b.physical().h.max(1)),
    );
    BootPlacement {
        w,
        h,
        position: boot.and_then(|b| b.position()),
        maximized: boot.is_some_and(|b| b.maximized()),
    }
}

/// Clamp a saved window extent to the screen. X11 constrains only the size —
/// position is the WM's business — and a zero screen extent means the probe
/// failed, so nothing is clamped against it.
pub(crate) fn clamp_to_screen(w: &mut i32, h: &mut i32, screen_w: i32, screen_h: i32) {
    if screen_w > 0 && *w > screen_w {
        *w = screen_w;
    }
    if screen_h > 0 && *h > screen_h {
        *h = screen_h;
    }
}

/// The `WM_PROTOCOLS` list for the app top-level.
///
/// `_NET_WM_SYNC_REQUEST` is advertised all-or-nothing: a WM must never be left
/// waiting on a counter we would not set, so it joins the list only when both
/// the atom and a real counter exist.
pub(crate) fn wm_protocols(atoms: &Atoms, sync_counter: u32) -> Vec<u32> {
    let mut protocols = vec![atoms.wm_delete_window];
    if sync_counter != 0 && atoms.net_wm_sync_request != 0 {
        protocols.push(atoms.net_wm_sync_request);
    }
    protocols
}

/// Find a 32-bit TrueColor visual — the one the ARGB overlays are created on.
pub(crate) fn find_argb_visual(screen: &Screen) -> Option<u32> {
    screen
        .allowed_depths
        .iter()
        .filter(|d| d.depth == 32)
        .flat_map(|d| d.visuals.iter())
        .find(|v| v.class == VisualClass::TRUE_COLOR)
        .map(|v| v.visual_id)
}

/// MIT-SHM 1.2 is the floor: fd passing (`shm_attach_fd`) arrived in it, and
/// the content path has no `shmget` fallback.
pub(crate) fn shm_version_supported(major: u16, minor: u16) -> bool {
    (major, minor) >= (1, 2)
}

/// Whether a compositing manager owns the `_NET_WM_CM_Sn` selection. `None` is
/// a failed query, which reads as present: overlays are transparent under a
/// compositor and the warning is worth withholding rather than crying wolf.
pub(crate) fn compositor_owner_present(owner: Option<u32>) -> bool {
    owner.is_none_or(|o| o != x11rb::NONE)
}

/// The compositing-manager selection atom for one screen.
pub(crate) fn cm_atom_name(screen_num: i32) -> String {
    format!("_NET_WM_CM_S{screen_num}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use jfn_platform_abi::{LogicalSize, Scale, WindowGeometry};
    use x11rb::protocol::xproto::{Depth, Visualtype};

    fn geometry(w: i32, h: i32, position: Option<WindowPos>, maximized: bool) -> BootGeometry {
        BootGeometry::from_clamped(
            LogicalSize { w, h },
            Scale(1.0),
            WindowGeometry { w, h, position },
            maximized,
        )
    }

    fn visual(id: u32, class: VisualClass) -> Visualtype {
        Visualtype {
            visual_id: id,
            class,
            bits_per_rgb_value: 8,
            colormap_entries: 256,
            red_mask: 0x00ff_0000,
            green_mask: 0x0000_ff00,
            blue_mask: 0x0000_00ff,
        }
    }

    fn screen_with(depths: Vec<Depth>) -> Screen {
        Screen {
            root: 1,
            default_colormap: 0,
            white_pixel: 0xff_ffff,
            black_pixel: 0,
            current_input_masks: 0u16.into(),
            width_in_pixels: 1920,
            height_in_pixels: 1080,
            width_in_millimeters: 508,
            height_in_millimeters: 286,
            min_installed_maps: 1,
            max_installed_maps: 1,
            root_visual: 0x21,
            backing_stores: x11rb::protocol::xproto::BackingStore::NOT_USEFUL,
            save_unders: false,
            root_depth: 24,
            allowed_depths: depths,
        }
    }

    fn atoms(delete: u32, sync_request: u32) -> Atoms {
        Atoms {
            net_wm_window_type: 0,
            net_wm_window_type_normal: 0,
            net_wm_state: 0,
            net_wm_state_skip_taskbar: 0,
            net_wm_state_skip_pager: 0,
            net_wm_state_fullscreen: 0,
            net_wm_state_maximized_vert: 0,
            net_wm_state_maximized_horz: 0,
            wm_protocols: 0,
            wm_delete_window: delete,
            net_wm_sync_request: sync_request,
            net_wm_sync_request_counter: 0,
            cardinal: 0,
            motif_wm_hints: 0,
            net_active_window: 0,
        }
    }

    #[test]
    fn a_restored_geometry_is_used_verbatim() {
        let p = boot_placement(
            Some(geometry(1280, 720, Some(WindowPos { x: 40, y: 50 }), true)),
            2.0,
        );
        assert_eq!((p.w, p.h), (1280, 720));
        assert_eq!(p.origin(), (40, 50));
        assert!(p.maximized);
    }

    #[test]
    fn a_zero_sized_restore_is_floored_at_one_pixel() {
        let p = boot_placement(Some(geometry(0, 0, None, false)), 1.0);
        assert_eq!((p.w, p.h), (1, 1));
    }

    #[test]
    fn nothing_saved_boots_at_the_scaled_default_extent() {
        assert_eq!(
            (boot_placement(None, 1.0).w, boot_placement(None, 1.0).h),
            (1600, 900)
        );
        let p = boot_placement(None, 1.5);
        assert_eq!((p.w, p.h), (2400, 1350));
        assert_eq!(p.origin(), (0, 0));
        assert!(!p.maximized);
        assert!(p.position.is_none());
    }

    #[test]
    fn an_extent_larger_than_the_screen_is_clamped_per_axis() {
        let (mut w, mut h) = (3000, 900);
        clamp_to_screen(&mut w, &mut h, 1920, 1080);
        assert_eq!((w, h), (1920, 900));
    }

    #[test]
    fn an_unknown_screen_extent_clamps_nothing() {
        let (mut w, mut h) = (3000, 2000);
        clamp_to_screen(&mut w, &mut h, 0, 0);
        assert_eq!((w, h), (3000, 2000));
    }

    #[test]
    fn sync_request_is_advertised_only_with_a_real_counter() {
        assert_eq!(wm_protocols(&atoms(7, 9), 42), vec![7, 9]);
        assert_eq!(wm_protocols(&atoms(7, 9), 0), vec![7]);
        assert_eq!(wm_protocols(&atoms(7, 0), 42), vec![7]);
    }

    #[test]
    fn the_argb_visual_is_the_true_colour_one_at_depth_32() {
        let screen = screen_with(vec![
            Depth {
                depth: 24,
                visuals: vec![visual(0x21, VisualClass::TRUE_COLOR)],
            },
            Depth {
                depth: 32,
                visuals: vec![
                    visual(0x40, VisualClass::DIRECT_COLOR),
                    visual(0x41, VisualClass::TRUE_COLOR),
                ],
            },
        ]);
        assert_eq!(find_argb_visual(&screen), Some(0x41));
    }

    #[test]
    fn a_screen_without_a_32_bit_true_colour_visual_has_no_argb_visual() {
        let screen = screen_with(vec![Depth {
            depth: 24,
            visuals: vec![visual(0x21, VisualClass::TRUE_COLOR)],
        }]);
        assert_eq!(find_argb_visual(&screen), None);
        assert_eq!(find_argb_visual(&screen_with(Vec::new())), None);
    }

    #[test]
    fn mit_shm_below_one_point_two_is_rejected() {
        assert!(shm_version_supported(1, 2));
        assert!(shm_version_supported(1, 3));
        assert!(shm_version_supported(2, 0));
        assert!(!shm_version_supported(1, 1));
        assert!(!shm_version_supported(0, 9));
    }

    #[test]
    fn an_unowned_compositor_selection_means_no_compositor() {
        assert!(!compositor_owner_present(Some(x11rb::NONE)));
        assert!(compositor_owner_present(Some(12)));
        // A failed query never raises the warning.
        assert!(compositor_owner_present(None));
    }

    #[test]
    fn the_compositor_selection_is_named_per_screen() {
        assert_eq!(cm_atom_name(0), "_NET_WM_CM_S0");
        assert_eq!(cm_atom_name(3), "_NET_WM_CM_S3");
    }
}
