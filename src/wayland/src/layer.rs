use std::ffi::c_void;
use std::ptr::NonNull;

use thiserror::Error;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Proxy};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;

use jfn_gpu_paint::WindowTarget;

use crate::wl_state::FrameBuffer;

/// Success outcome of a present/enqueue. A `Skipped` is a deliberate no-op, not
/// a failure, so it must never be mapped to an `Err`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Present {
    Committed,
    Skipped,
}

#[derive(Debug, Error)]
pub(crate) enum PresentError {
    #[error("invalid frame dimensions: {0}x{1}")]
    BadDimensions(i32, i32),
    #[error("pixel buffer too small: have {have}, need {need}")]
    ShortBuffer { have: usize, need: usize },
    #[error("gpu paint failed: {0}")]
    Gpu(#[from] jfn_gpu_paint::SurfaceLost),
    #[error("shm buffer allocation failed")]
    ShmAlloc,
    #[error("dmabuf buffer creation failed")]
    DmabufCreate,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) struct ViewportState {
    pub(crate) lw: i32,
    pub(crate) lh: i32,
    pub(crate) pw: i32,
    pub(crate) ph: i32,
}

pub(crate) struct LayerSurface {
    conn: Connection,
    surface: WlSurface,
    viewport: Option<WpViewport>,
}

impl LayerSurface {
    pub(crate) fn new(conn: Connection, surface: WlSurface, viewport: Option<WpViewport>) -> Self {
        Self {
            conn,
            surface,
            viewport,
        }
    }

    pub(crate) fn window_target(&self) -> Option<WindowTarget> {
        let display = NonNull::new(self.conn.backend().display_ptr().cast::<c_void>())?;
        let surface = NonNull::new(self.surface.id().as_ptr().cast::<c_void>())?;
        Some(WindowTarget::Wayland { display, surface })
    }

    pub(crate) fn attach_none(&self) {
        self.surface.attach(None, 0, 0);
    }

    pub(crate) fn set_viewport(&self, src_w: i32, src_h: i32, dst_w: i32, dst_h: i32) {
        let Some(viewport) = self.viewport.as_ref() else {
            return;
        };
        let (send_source, send_destination) = viewport_update(src_w, src_h, dst_w, dst_h);
        if send_source {
            viewport.set_source(0.0, 0.0, src_w as f64, src_h as f64);
        }
        if send_destination {
            viewport.set_destination(dst_w, dst_h);
        }
    }

    pub(crate) fn present(&self, frame: FrameCommit<'_>) {
        self.set_viewport(frame.src_w, frame.src_h, frame.dst_w, frame.dst_h);
        frame.buf.attach_to(&self.surface);
        self.surface.damage_buffer(0, 0, frame.buf_w, frame.buf_h);
        self.surface.commit();
    }

    pub(crate) fn commit(&self) {
        self.surface.commit();
    }

    pub(crate) fn flush(&self) {
        let _ = self.conn.flush();
    }
}

pub(crate) struct FrameCommit<'a> {
    buf: FrameBuffer<'a>,
    buf_w: i32,
    buf_h: i32,
    src_w: i32,
    src_h: i32,
    dst_w: i32,
    dst_h: i32,
}

/// Clamp a viewport source rect to the buffer it reads from: a `wp_viewport`
/// source larger than the attached buffer is a fatal protocol error that kills
/// the client, so a producer that overstates its visible rect is corrected
/// here rather than trusted.
pub(crate) fn clamp_source(src_w: i32, src_h: i32, buf_w: i32, buf_h: i32) -> (i32, i32) {
    (src_w.min(buf_w), src_h.min(buf_h))
}

/// Which halves of a `wp_viewport` update may go on the wire. The protocol
/// rejects a non-positive source or destination, and omitting either leaves
/// whatever the compositor already latched in force — which is how a frame
/// that only rescales its destination avoids disturbing the source.
pub(crate) fn viewport_update(src_w: i32, src_h: i32, dst_w: i32, dst_h: i32) -> (bool, bool) {
    (src_w > 0 && src_h > 0, dst_w > 0 && dst_h > 0)
}

impl<'a> FrameCommit<'a> {
    /// Clamps `src_*` to the buffer dimensions via [`clamp_source`].
    pub(crate) fn new(
        buf: FrameBuffer<'a>,
        buf_w: i32,
        buf_h: i32,
        src_w: i32,
        src_h: i32,
        dst_w: i32,
        dst_h: i32,
    ) -> Self {
        let (src_w, src_h) = clamp_source(src_w, src_h, buf_w, buf_h);
        Self {
            buf,
            buf_w,
            buf_h,
            src_w,
            src_h,
            dst_w,
            dst_h,
        }
    }
}

pub(crate) struct SurfaceRef {
    surface: WlSurface,
    viewport: Option<WpViewport>,
}

impl SurfaceRef {
    pub(crate) fn new(surface: WlSurface, viewport: Option<WpViewport>) -> Self {
        Self { surface, viewport }
    }

    pub(crate) fn as_arg(&self) -> &WlSurface {
        &self.surface
    }

    pub(crate) fn destroy(self) {
        if let Some(viewport) = self.viewport {
            viewport.destroy();
        }
        self.surface.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::{clamp_source, viewport_update};

    #[test]
    fn a_source_inside_the_buffer_is_left_alone() {
        assert_eq!(clamp_source(100, 50, 200, 200), (100, 50));
        assert_eq!(clamp_source(200, 200, 200, 200), (200, 200));
    }

    #[test]
    fn a_source_larger_than_the_buffer_is_clamped_per_axis() {
        // Overstating one axis must not shrink the other.
        assert_eq!(clamp_source(300, 50, 200, 200), (200, 50));
        assert_eq!(clamp_source(50, 300, 200, 200), (50, 200));
        assert_eq!(clamp_source(i32::MAX, i32::MAX, 64, 32), (64, 32));
    }

    #[test]
    fn a_positive_source_and_destination_both_go_on_the_wire() {
        assert_eq!(viewport_update(64, 32, 1280, 720), (true, true));
    }

    #[test]
    fn a_non_positive_source_leaves_the_latched_source_in_force() {
        // The reapply-viewport path deliberately passes a zero source so only
        // the destination is rescaled.
        assert_eq!(viewport_update(0, 0, 1280, 720), (false, true));
        assert_eq!(viewport_update(64, 0, 1280, 720), (false, true));
        assert_eq!(viewport_update(-1, 32, 1280, 720), (false, true));
    }

    #[test]
    fn a_non_positive_destination_is_never_sent() {
        assert_eq!(viewport_update(64, 32, 0, 0), (true, false));
        assert_eq!(viewport_update(64, 32, 1280, -1), (true, false));
        assert_eq!(viewport_update(0, 0, 0, 0), (false, false));
    }
}
