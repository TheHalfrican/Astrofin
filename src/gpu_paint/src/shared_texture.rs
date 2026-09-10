//! A CEF accelerated-paint texture, selected statically per platform.
//!
//! CEF reclaims the resources backing a frame when the paint callback returns.
//! The Linux constructor therefore receives already-duplicated plane fds;
//! Windows and macOS consume their borrowed handles inline during the callback.

/// Integer texture extent used by the paint crate without depending on the
/// platform ABI's geometry types.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FrameSize {
    pub w: i32,
    pub h: i32,
}

#[cfg(target_os = "linux")]
use std::os::fd::OwnedFd;

#[cfg(target_os = "linux")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DmabufFormat {
    Bgra8,
    Rgba8,
}

/// One plane of a dmabuf. Owns its fd, closed on drop; an importer that needs
/// to hand the fd to a driver consumes a dup of it.
#[cfg(target_os = "linux")]
pub struct DmabufPlane {
    pub fd: OwnedFd,
    pub offset: u64,
    pub stride: u32,
}

#[cfg(target_os = "linux")]
pub struct SharedTexture {
    coded: FrameSize,
    visible_rect: FrameSize,
    format: DmabufFormat,
    modifier: u64,
    planes: Vec<DmabufPlane>,
}

#[cfg(target_os = "linux")]
impl SharedTexture {
    /// `planes` must already own their fds — see the module docs.
    pub fn new(
        coded: FrameSize,
        visible_rect: FrameSize,
        format: DmabufFormat,
        modifier: u64,
        planes: Vec<DmabufPlane>,
    ) -> Self {
        Self {
            coded,
            visible_rect,
            format,
            modifier,
            planes,
        }
    }

    pub fn coded(&self) -> FrameSize {
        self.coded
    }

    pub fn format(&self) -> DmabufFormat {
        self.format
    }

    pub fn modifier(&self) -> u64 {
        self.modifier
    }

    pub fn planes(&self) -> &[DmabufPlane] {
        &self.planes
    }

    pub fn visible_rect(&self) -> FrameSize {
        self.visible_rect
    }

    pub fn visible(&self) -> FrameSize {
        visible_or_coded(self.visible_rect, self.coded)
    }
}

#[cfg(windows)]
pub struct SharedTexture {
    handle: *mut std::ffi::c_void,
    coded: FrameSize,
    visible_rect: FrameSize,
}

#[cfg(windows)]
impl SharedTexture {
    pub fn new(handle: *mut std::ffi::c_void, coded: FrameSize, visible_rect: FrameSize) -> Self {
        Self {
            handle,
            coded,
            visible_rect,
        }
    }

    pub fn handle(&self) -> *mut std::ffi::c_void {
        self.handle
    }

    pub fn coded(&self) -> FrameSize {
        self.coded
    }

    pub fn visible_rect(&self) -> FrameSize {
        self.visible_rect
    }

    pub fn visible(&self) -> FrameSize {
        visible_or_coded(self.visible_rect, self.coded)
    }
}

#[cfg(target_os = "macos")]
pub struct SharedTexture {
    io_surface: *mut std::ffi::c_void,
    coded: FrameSize,
    visible_rect: FrameSize,
}

#[cfg(target_os = "macos")]
impl SharedTexture {
    pub fn new(
        io_surface: *mut std::ffi::c_void,
        coded: FrameSize,
        visible_rect: FrameSize,
    ) -> Self {
        Self {
            io_surface,
            coded,
            visible_rect,
        }
    }

    pub fn io_surface(&self) -> *mut std::ffi::c_void {
        self.io_surface
    }

    pub fn coded(&self) -> FrameSize {
        self.coded
    }

    pub fn visible_rect(&self) -> FrameSize {
        self.visible_rect
    }

    pub fn visible(&self) -> FrameSize {
        visible_or_coded(self.visible_rect, self.coded)
    }
}

fn visible_or_coded(visible_rect: FrameSize, coded: FrameSize) -> FrameSize {
    if visible_rect.w > 0 && visible_rect.h > 0 {
        visible_rect
    } else {
        coded
    }
}

/// A texture with no real backing buffer, for tests that need a value rather
/// than an importable frame. Nothing in this module dereferences the handle.
#[cfg(test)]
pub(crate) fn test_texture() -> SharedTexture {
    let size = FrameSize { w: 1, h: 1 };
    #[cfg(target_os = "linux")]
    {
        SharedTexture::new(size, size, DmabufFormat::Bgra8, 0, Vec::new())
    }
    #[cfg(windows)]
    {
        SharedTexture::new(std::ptr::null_mut(), size, size)
    }
    #[cfg(target_os = "macos")]
    {
        SharedTexture::new(std::ptr::null_mut(), size, size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODED: FrameSize = FrameSize { w: 1920, h: 1088 };
    const VISIBLE: FrameSize = FrameSize { w: 1920, h: 1080 };

    #[test]
    fn a_visible_rect_with_area_wins_over_the_coded_size() {
        assert_eq!(visible_or_coded(VISIBLE, CODED), VISIBLE);
    }

    #[test]
    fn an_empty_visible_rect_falls_back_to_the_coded_size() {
        assert_eq!(visible_or_coded(FrameSize { w: 0, h: 0 }, CODED), CODED);
        assert_eq!(visible_or_coded(FrameSize { w: 1920, h: 0 }, CODED), CODED);
        assert_eq!(visible_or_coded(FrameSize { w: 0, h: 1080 }, CODED), CODED);
    }

    #[test]
    fn a_negative_visible_rect_falls_back_to_the_coded_size() {
        assert_eq!(visible_or_coded(FrameSize { w: -1, h: -1 }, CODED), CODED);
    }

    #[cfg(target_os = "linux")]
    mod linux {
        use super::*;

        fn texture(visible: FrameSize) -> SharedTexture {
            SharedTexture::new(CODED, visible, DmabufFormat::Rgba8, 7, Vec::new())
        }

        #[test]
        fn a_dmabuf_texture_reports_the_coded_size_it_was_built_with() {
            assert_eq!(texture(VISIBLE).coded(), CODED);
        }

        #[test]
        fn a_dmabuf_texture_reports_its_format() {
            assert_eq!(texture(VISIBLE).format(), DmabufFormat::Rgba8);
        }

        #[test]
        fn a_dmabuf_texture_reports_its_modifier() {
            assert_eq!(texture(VISIBLE).modifier(), 7);
        }

        #[test]
        fn a_texture_built_without_planes_has_none() {
            assert!(texture(VISIBLE).planes().is_empty());
        }

        #[test]
        fn a_dmabuf_texture_reports_the_visible_rect_verbatim() {
            assert_eq!(texture(VISIBLE).visible_rect(), VISIBLE);
            assert_eq!(texture(FrameSize { w: 0, h: 0 }).visible_rect().w, 0);
        }

        #[test]
        fn a_dmabuf_textures_visible_size_falls_back_to_coded() {
            assert_eq!(texture(VISIBLE).visible(), VISIBLE);
            assert_eq!(texture(FrameSize { w: 0, h: 0 }).visible(), CODED);
        }
    }

    #[cfg(windows)]
    mod windows {
        use super::*;

        fn texture(visible: FrameSize) -> SharedTexture {
            SharedTexture::new(std::ptr::dangling_mut(), CODED, visible)
        }

        #[test]
        fn a_shared_handle_texture_hands_its_handle_back() {
            assert_eq!(texture(VISIBLE).handle(), std::ptr::dangling_mut());
        }

        #[test]
        fn a_shared_handle_texture_reports_the_coded_size() {
            assert_eq!(texture(VISIBLE).coded(), CODED);
        }

        #[test]
        fn a_shared_handle_texture_reports_the_visible_rect_verbatim() {
            assert_eq!(texture(VISIBLE).visible_rect(), VISIBLE);
            assert_eq!(texture(FrameSize { w: 0, h: 0 }).visible_rect().w, 0);
        }

        #[test]
        fn a_shared_handle_textures_visible_size_falls_back_to_coded() {
            assert_eq!(texture(VISIBLE).visible(), VISIBLE);
            assert_eq!(texture(FrameSize { w: 0, h: 0 }).visible(), CODED);
        }
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use super::*;

        fn texture(visible: FrameSize) -> SharedTexture {
            SharedTexture::new(std::ptr::dangling_mut(), CODED, visible)
        }

        #[test]
        fn an_io_surface_texture_hands_its_surface_back() {
            assert_eq!(texture(VISIBLE).io_surface(), std::ptr::dangling_mut());
        }

        #[test]
        fn an_io_surface_texture_reports_the_coded_size() {
            assert_eq!(texture(VISIBLE).coded(), CODED);
        }

        #[test]
        fn an_io_surface_texture_reports_the_visible_rect_verbatim() {
            assert_eq!(texture(VISIBLE).visible_rect(), VISIBLE);
            assert_eq!(texture(FrameSize { w: 0, h: 0 }).visible_rect().w, 0);
        }

        #[test]
        fn an_io_surface_textures_visible_size_falls_back_to_coded() {
            assert_eq!(texture(VISIBLE).visible(), VISIBLE);
            assert_eq!(texture(FrameSize { w: 0, h: 0 }).visible(), CODED);
        }
    }
}
