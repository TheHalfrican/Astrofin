//! Pure decisions the geometry thread makes, lifted out of [`crate::geometry`].
//!
//! The geometry thread reads parent truth off the server, decides what changed
//! and what to reassert, and writes structure. Everything in the middle — the
//! work-level lattice, the z-order merge, the EWMH state fold, the resize-sync
//! handshake bookkeeping — is arithmetic over values, so it lives here where it
//! runs without a server. The per-overlay placement state machine is a module
//! of its own: [`crate::overlay_fsm`].

use jfn_platform_abi::{PhysicalSize, Scale, WindowExtent, WindowPos, WindowSnapshot};

use crate::overlay_fsm::Geom;
use crate::x11_state::ParentSnapshot;

/// How much work one pass of the loop owes, as a lattice: a pass does the most
/// demanding thing any event in it asked for, and the order is exactly
/// "includes everything below".
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Pending {
    Idle,
    Reconcile,
    Restack,
    Refocus,
}

impl Pending {
    /// Fold another request into this one. Never downgrades: an event that
    /// wants only a reconcile cannot cancel a restack another event asked for.
    pub(crate) fn merge(self, other: Self) -> Self {
        self.max(other)
    }
}

/// What a `PropertyNotify` is about. The root's `RESOURCE_MANAGER` is the
/// Xft.dpi channel, so a change there re-probes the display scale; anything
/// else on the root is not ours.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PropertyTarget {
    Parent,
    ResourceManager,
    Other,
}

pub(crate) fn classify_property(
    window: u32,
    atom: u32,
    parent: u32,
    root: u32,
    resource_manager: u32,
) -> PropertyTarget {
    if window == parent {
        PropertyTarget::Parent
    } else if window == root && atom == resource_manager {
        PropertyTarget::ResourceManager
    } else {
        PropertyTarget::Other
    }
}

/// The bottom-to-top overlay order after a restack request.
///
/// Ids we no longer own are dropped, the caller's order is otherwise kept, and
/// any overlay the caller omitted is appended on top — defensively, so a
/// surface can never fall out of the stacking pass entirely.
pub(crate) fn merged_order<T: Copy + PartialEq>(
    requested: impl IntoIterator<Item = T>,
    current: &[T],
    is_owned: impl Fn(&T) -> bool,
) -> Vec<T> {
    let mut order: Vec<T> = requested.into_iter().filter(|id| is_owned(id)).collect();
    for id in current {
        if !order.contains(id) {
            order.push(*id);
        }
    }
    order
}

/// Fold a `_NET_WM_STATE` atom list into `(fullscreen, maximized)`. Maximized
/// means both axes: a half-maximized window is not the maximized state the app
/// persists.
pub(crate) fn fold_wm_state(
    values: impl IntoIterator<Item = u32>,
    fullscreen: u32,
    maximized_vert: u32,
    maximized_horz: u32,
) -> (bool, bool) {
    let (mut fs, mut mv, mut mh) = (false, false, false);
    for atom in values {
        fs |= atom == fullscreen;
        mv |= atom == maximized_vert;
        mh |= atom == maximized_horz;
    }
    (fs, mv && mh)
}

/// Fullscreen inferred from geometry alone, for a WM that fills the screen
/// without setting the state atom. `>=`, not `==`: a WM may overscan slightly.
pub(crate) fn geometric_fullscreen(geom: Geom, root_w: i32, root_h: i32) -> bool {
    geom.2 >= root_w && geom.3 >= root_h
}

/// The video host is a child filling the client area, so it is sized in local
/// coordinates and floored at one pixel in each axis — X rejects a zero extent.
pub(crate) fn host_fill_size(parent_w: i32, parent_h: i32) -> (i32, i32) {
    (parent_w.max(1), parent_h.max(1))
}

/// Whether the published parent snapshot would change. Only a change notifies
/// the rest of the app; the snapshot itself is republished every pass.
pub(crate) fn parent_changed(
    previous: (Geom, bool, bool),
    geom: Geom,
    fullscreen: bool,
    maximized: bool,
) -> bool {
    previous != (geom, fullscreen, maximized)
}

/// Whether a re-probed display scale differs from the published one.
pub(crate) fn scale_changed(published: f32, probed: f32) -> bool {
    (published - probed).abs() > f32::EPSILON
}

/// Whether a `ClientMessage` is the WM asking the app to close.
pub(crate) fn is_wm_delete(
    type_: u32,
    data0: u32,
    wm_protocols: u32,
    wm_delete_window: u32,
) -> bool {
    type_ == wm_protocols && data0 == wm_delete_window
}

/// The `(hi, lo)` counter value a `_NET_WM_SYNC_REQUEST` client message asks us
/// to set, or `None` when the message is something else or the handshake was
/// never established. Data layout: `[protocol, timestamp, lo, hi, _]`.
pub(crate) fn sync_request_value(
    type_: u32,
    data: [u32; 5],
    wm_protocols: u32,
    net_wm_sync_request: u32,
    sync_counter: u32,
) -> Option<(i32, u32)> {
    if sync_counter == 0 || net_wm_sync_request == 0 {
        return None;
    }
    if type_ != wm_protocols || data[0] != net_wm_sync_request {
        return None;
    }
    Some((data[3] as i32, data[2]))
}

/// The `_NET_WM_SYNC_REQUEST` handshake's client-side bookkeeping.
///
/// The WM latches a counter value on us before a resize and waits for us to set
/// it once our configures have landed. Latching disarms: the value is only
/// committed after the `ConfigureNotify` that armed it, so we never tell the WM
/// we are done with a resize we have not seen yet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ResizeSync {
    /// The counter id, or 0 when the protocol was not advertised.
    counter: u32,
    pending: Option<(i32, u32)>,
    armed: bool,
}

impl ResizeSync {
    pub(crate) fn new(counter: u32) -> Self {
        Self {
            counter,
            pending: None,
            armed: false,
        }
    }

    /// Record the value the WM asked for. Ignored entirely when the protocol
    /// was not advertised.
    pub(crate) fn latch(&mut self, hi: i32, lo: u32) {
        if self.counter == 0 {
            return;
        }
        self.pending = Some((hi, lo));
        self.armed = false;
    }

    /// The parent's `ConfigureNotify` for the latched resize has arrived.
    pub(crate) fn arm(&mut self) {
        self.armed = true;
    }

    /// The counter write owed to the WM, consuming it. `None` while nothing is
    /// latched or the configure has not arrived yet.
    pub(crate) fn take_commit(&mut self) -> Option<(u32, i32, u32)> {
        if !self.armed {
            return None;
        }
        let (hi, lo) = self.pending.take()?;
        Some((self.counter, hi, lo))
    }
}

/// The ABI window snapshot derived from the parent geometry the geometry thread
/// published. A zero extent is "not known yet", never a real size.
pub(crate) fn window_snapshot(m: &ParentSnapshot) -> WindowSnapshot {
    let extent = (m.width > 0 && m.height > 0).then(|| {
        WindowExtent::new(
            PhysicalSize {
                w: m.width,
                h: m.height,
            },
            Scale(m.scale),
        )
    });
    WindowSnapshot {
        extent,
        position: Some(WindowPos {
            x: m.origin_x,
            y: m.origin_y,
        }),
        maximized: m.maximized,
        fullscreen: m.fullscreen,
    }
}

/// The snapshot reported before the host window exists: nothing is known.
pub(crate) fn empty_window_snapshot() -> WindowSnapshot {
    WindowSnapshot {
        extent: None,
        position: None,
        maximized: false,
        fullscreen: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: Geom = (100, 50, 800, 600);

    fn snapshot(width: i32, height: i32) -> ParentSnapshot {
        ParentSnapshot {
            origin_x: 10,
            origin_y: 20,
            width,
            height,
            fullscreen: true,
            maximized: false,
            scale: 1.5,
        }
    }

    #[test]
    fn a_pass_does_the_most_demanding_thing_asked_of_it() {
        assert_eq!(Pending::Idle.merge(Pending::Restack), Pending::Restack);
        assert_eq!(Pending::Restack.merge(Pending::Reconcile), Pending::Restack);
        assert_eq!(Pending::Restack.merge(Pending::Refocus), Pending::Refocus);
        assert_eq!(Pending::Idle.merge(Pending::Idle), Pending::Idle);
    }

    #[test]
    fn the_work_lattice_is_ordered_by_how_much_it_includes() {
        assert!(Pending::Idle < Pending::Reconcile);
        assert!(Pending::Reconcile < Pending::Restack);
        assert!(Pending::Restack < Pending::Refocus);
    }

    #[test]
    fn a_resource_manager_change_on_the_root_is_the_scale_channel() {
        assert_eq!(
            classify_property(9, 5, 9, 1, 5),
            PropertyTarget::Parent,
            "the parent wins even when the atom matches"
        );
        assert_eq!(
            classify_property(1, 5, 9, 1, 5),
            PropertyTarget::ResourceManager
        );
        assert_eq!(classify_property(1, 6, 9, 1, 5), PropertyTarget::Other);
        assert_eq!(classify_property(7, 5, 9, 1, 5), PropertyTarget::Other);
    }

    #[test]
    fn a_restack_keeps_the_requested_order_of_the_ids_we_own() {
        let order = merged_order([3u32, 1, 2], &[1, 2, 3], |id| [1, 2, 3].contains(id));
        assert_eq!(order, vec![3, 1, 2]);
    }

    #[test]
    fn a_restack_drops_ids_we_no_longer_own() {
        let order = merged_order([9u32, 1], &[1], |id| *id == 1);
        assert_eq!(order, vec![1]);
    }

    #[test]
    fn an_omitted_overlay_is_appended_on_top() {
        let order = merged_order([2u32], &[1, 2, 3], |id| [1, 2, 3].contains(id));
        assert_eq!(order, vec![2, 1, 3]);
    }

    #[test]
    fn maximized_needs_both_axes() {
        assert_eq!(fold_wm_state([20u32, 21], 10, 20, 21), (false, true));
        assert_eq!(fold_wm_state([20u32], 10, 20, 21), (false, false));
        assert_eq!(fold_wm_state([10u32], 10, 20, 21), (true, false));
        assert_eq!(fold_wm_state([10u32, 20, 21], 10, 20, 21), (true, true));
    }

    #[test]
    fn an_empty_wm_state_is_neither_fullscreen_nor_maximized() {
        assert_eq!(
            fold_wm_state(std::iter::empty(), 10, 20, 21),
            (false, false)
        );
    }

    #[test]
    fn a_window_at_least_as_big_as_the_root_reads_as_fullscreen() {
        assert!(geometric_fullscreen((0, 0, 1920, 1080), 1920, 1080));
        assert!(geometric_fullscreen((0, 0, 1921, 1081), 1920, 1080));
        assert!(!geometric_fullscreen((0, 0, 1920, 1053), 1920, 1080));
        assert!(!geometric_fullscreen(WIN, 1920, 1080));
    }

    #[test]
    fn the_video_host_is_never_sized_to_zero() {
        assert_eq!(host_fill_size(800, 600), (800, 600));
        assert_eq!(host_fill_size(0, 0), (1, 1));
        assert_eq!(host_fill_size(-4, 600), (1, 600));
    }

    #[test]
    fn any_field_of_the_parent_snapshot_counts_as_a_change() {
        let base = (WIN, false, false);
        assert!(!parent_changed(base, WIN, false, false));
        assert!(parent_changed(base, (101, 50, 800, 600), false, false));
        assert!(parent_changed(base, WIN, true, false));
        assert!(parent_changed(base, WIN, false, true));
    }

    #[test]
    fn an_identical_scale_is_not_a_change() {
        assert!(!scale_changed(1.5, 1.5));
        assert!(scale_changed(1.0, 1.5));
        assert!(scale_changed(2.0, 1.0));
    }

    #[test]
    fn only_the_delete_protocol_message_closes_the_app() {
        assert!(is_wm_delete(3, 7, 3, 7));
        assert!(!is_wm_delete(4, 7, 3, 7));
        assert!(!is_wm_delete(3, 8, 3, 7));
    }

    #[test]
    fn a_sync_request_carries_the_counter_value_hi_then_lo() {
        assert_eq!(
            sync_request_value(3, [9, 0, 0xAA, 5, 0], 3, 9, 42),
            Some((5, 0xAA))
        );
    }

    #[test]
    fn a_sync_request_is_ignored_without_a_counter_or_the_atom() {
        assert_eq!(sync_request_value(3, [9, 0, 1, 2, 0], 3, 9, 0), None);
        assert_eq!(sync_request_value(3, [9, 0, 1, 2, 0], 3, 0, 42), None);
    }

    #[test]
    fn another_protocol_message_is_not_a_sync_request() {
        assert_eq!(sync_request_value(3, [8, 0, 1, 2, 0], 3, 9, 42), None);
        assert_eq!(sync_request_value(4, [9, 0, 1, 2, 0], 3, 9, 42), None);
    }

    #[test]
    fn a_latched_value_commits_only_after_the_configure_arrives() {
        let mut sync = ResizeSync::new(42);
        sync.latch(1, 2);
        assert_eq!(sync.take_commit(), None);
        sync.arm();
        assert_eq!(sync.take_commit(), Some((42, 1, 2)));
    }

    #[test]
    fn a_committed_value_is_not_written_twice() {
        let mut sync = ResizeSync::new(42);
        sync.latch(1, 2);
        sync.arm();
        assert!(sync.take_commit().is_some());
        assert_eq!(sync.take_commit(), None);
    }

    #[test]
    fn a_new_latch_disarms_the_previous_configure() {
        let mut sync = ResizeSync::new(42);
        sync.latch(1, 2);
        sync.arm();
        sync.latch(3, 4);
        assert_eq!(sync.take_commit(), None);
        sync.arm();
        assert_eq!(sync.take_commit(), Some((42, 3, 4)));
    }

    #[test]
    fn without_an_advertised_counter_nothing_is_ever_latched() {
        let mut sync = ResizeSync::new(0);
        sync.latch(1, 2);
        sync.arm();
        assert_eq!(sync.take_commit(), None);
    }

    #[test]
    fn a_published_snapshot_becomes_the_abi_window_state() {
        let snap = window_snapshot(&snapshot(800, 600));
        assert_eq!(
            snap.position,
            Some(WindowPos { x: 10, y: 20 }),
            "position is always reported once the host window exists"
        );
        assert!(snap.fullscreen);
        assert!(!snap.maximized);
        let extent = snap.extent.unwrap();
        assert_eq!(extent.physical().w, 800);
        assert_eq!(extent.physical().h, 600);
    }

    #[test]
    fn a_zero_extent_snapshot_reports_no_extent() {
        assert!(window_snapshot(&snapshot(0, 600)).extent.is_none());
        assert!(window_snapshot(&snapshot(800, 0)).extent.is_none());
    }

    #[test]
    fn before_the_host_window_exists_nothing_is_known() {
        let snap = empty_window_snapshot();
        assert!(snap.extent.is_none());
        assert!(snap.position.is_none());
        assert!(!snap.maximized);
        assert!(!snap.fullscreen);
    }
}
