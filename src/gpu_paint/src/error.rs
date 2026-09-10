use crate::FrameSize;
use thiserror::Error;

/// This surface can no longer present; the caller should abandon it.
///
/// Its *existence* is the whole signal — there is no severity to interrogate,
/// which is why it is opaque. Anything recoverable is handled internally and
/// comes back as [`crate::Presented::Skipped`]: a stale, occluded or timed-out
/// swapchain, and a shared-texture import that failed (a shared frame has no
/// CPU pixels, so there is nothing to fall back to and the frame is dropped).
///
/// The detail below exists for the log line, and reaches callers only through
/// `Display`.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct SurfaceLost(Kind);

#[derive(Debug, Error)]
pub(crate) enum Kind {
    #[error("no usable adapter available")]
    NoAdapter,
    #[error("device request failed: {0}")]
    DeviceRequest(#[from] wgpu::RequestDeviceError),
    #[error("surface creation failed: {0}")]
    SurfaceCreate(#[from] wgpu::CreateSurfaceError),
    #[error("adapter does not support requested surface")]
    SurfaceUnsupported,
    #[error("swapchain acquire failed: {0}")]
    Acquire(&'static str),
    #[error("invalid frame dimensions: {}x{}", .0.w, .0.h)]
    BadDimensions(FrameSize),
    #[error(
        "frame buffer does not cover {}x{} at stride {stride}: {len} bytes",
        .size.w, .size.h
    )]
    BadPixelBuffer {
        size: FrameSize,
        stride: u32,
        len: usize,
    },
}

impl<E: Into<Kind>> From<E> for SurfaceLost {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_becomes_a_surface_lost_that_displays_its_detail() {
        let lost: SurfaceLost = Kind::NoAdapter.into();
        assert_eq!(lost.to_string(), "no usable adapter available");
    }

    #[test]
    fn the_dimension_error_names_the_frame_it_refused() {
        let lost: SurfaceLost = Kind::BadDimensions(FrameSize { w: 0, h: 720 }).into();
        assert_eq!(lost.to_string(), "invalid frame dimensions: 0x720");
    }

    #[test]
    fn the_buffer_error_names_the_size_stride_and_length() {
        let lost: SurfaceLost = Kind::BadPixelBuffer {
            size: FrameSize { w: 4, h: 4 },
            stride: 16,
            len: 12,
        }
        .into();
        assert_eq!(
            lost.to_string(),
            "frame buffer does not cover 4x4 at stride 16: 12 bytes"
        );
    }

    #[test]
    fn an_acquire_failure_carries_the_reason_through() {
        let lost: SurfaceLost = Kind::Acquire("Outdated").into();
        assert_eq!(lost.to_string(), "swapchain acquire failed: Outdated");
    }
}
