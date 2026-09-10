//! MIT-SHM segment bookkeeping: the buffer record and the size/reuse
//! arithmetic around it.
//!
//! Split out of [`crate::shm`], which keeps only the parts that need a live
//! server (`memfd_create`, `mmap`, attach/detach). A [`ShmBuffer`] is a plain
//! record — segment id, mapping, dimensions — so its whole state machine
//! (empty → mapped → empty) is exercised with an anonymous mapping and no X
//! connection at all.

use memmap2::MmapMut;
use x11rb::protocol::shm;

/// Owns one MIT-SHM segment plus its mapping. Two per surface so the renderer
/// can double-buffer.
pub struct ShmBuffer {
    seg: shm::Seg,
    map: Option<MmapMut>,
    w: i32,
    h: i32,
}

impl ShmBuffer {
    pub fn empty() -> Self {
        Self {
            seg: 0,
            map: None,
            w: 0,
            h: 0,
        }
    }

    /// The segment registered with the server, or 0 while unmapped.
    pub fn seg(&self) -> shm::Seg {
        self.seg
    }

    pub fn is_mapped(&self) -> bool {
        self.map.is_some()
    }

    pub fn dims(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    /// The live mapping, or an empty slice while unmapped.
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        self.map.as_mut().map_or(&mut [], |m| &mut m[..])
    }

    /// Replaces the mapping; the caller detaches the previous segment first.
    pub fn set(&mut self, seg: shm::Seg, map: MmapMut, w: i32, h: i32) {
        self.seg = seg;
        self.map = Some(map);
        self.w = w;
        self.h = h;
    }

    /// Unmaps and returns the buffer to its empty state.
    pub fn clear(&mut self) {
        *self = Self::empty();
    }
}

impl Default for ShmBuffer {
    fn default() -> Self {
        Self::empty()
    }
}

/// Bytes one `w` x `h` BGRA segment needs. Callers only ever reach here with a
/// positive extent (the actor rejects a non-positive frame before it presents),
/// which is what keeps the cast well-defined.
pub(crate) fn segment_size(w: i32, h: i32) -> usize {
    (w as usize) * (h as usize) * 4
}

/// Whether an existing buffer can be presented into as-is. A mapped buffer at
/// exactly the requested extent is reused; anything else is detached and
/// re-attached, because the server's mapping is sized once at attach time.
pub(crate) fn can_reuse(mapped: bool, dims: (i32, i32), w: i32, h: i32) -> bool {
    mapped && dims == (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use memmap2::MmapOptions;

    fn anon(len: usize) -> MmapMut {
        MmapOptions::new().len(len).map_anon().unwrap()
    }

    #[test]
    fn an_empty_buffer_has_no_segment_and_no_pixels() {
        let mut buf = ShmBuffer::empty();
        assert!(!buf.is_mapped());
        assert_eq!(buf.seg(), 0);
        assert_eq!(buf.dims(), (0, 0));
        assert!(buf.pixels_mut().is_empty());
    }

    #[test]
    fn the_default_buffer_is_the_empty_one() {
        let mut buf = ShmBuffer::default();
        assert!(!buf.is_mapped());
        assert!(buf.pixels_mut().is_empty());
    }

    #[test]
    fn setting_a_mapping_publishes_the_segment_and_extent() {
        let mut buf = ShmBuffer::empty();
        buf.set(9, anon(segment_size(4, 2)), 4, 2);
        assert!(buf.is_mapped());
        assert_eq!(buf.seg(), 9);
        assert_eq!(buf.dims(), (4, 2));
        assert_eq!(buf.pixels_mut().len(), 32);
    }

    #[test]
    fn written_pixels_survive_until_the_buffer_is_cleared() {
        let mut buf = ShmBuffer::empty();
        buf.set(1, anon(16), 2, 2);
        buf.pixels_mut()[0] = 0xAB;
        assert_eq!(buf.pixels_mut()[0], 0xAB);
        buf.clear();
        assert!(!buf.is_mapped());
        assert_eq!(buf.seg(), 0);
        assert_eq!(buf.dims(), (0, 0));
    }

    #[test]
    fn a_remap_replaces_the_segment_and_the_extent() {
        let mut buf = ShmBuffer::empty();
        buf.set(1, anon(segment_size(2, 2)), 2, 2);
        buf.set(2, anon(segment_size(4, 4)), 4, 4);
        assert_eq!(buf.seg(), 2);
        assert_eq!(buf.dims(), (4, 4));
        assert_eq!(buf.pixels_mut().len(), 64);
    }

    #[test]
    fn the_mapping_is_writable_end_to_end() {
        let mut buf = ShmBuffer::empty();
        buf.set(1, anon(segment_size(4, 2)), 4, 2);
        let pixels = buf.pixels_mut();
        let last = pixels.len() - 1;
        pixels[0] = 1;
        pixels[last] = 2;
        assert_eq!(buf.pixels_mut()[0], 1);
        assert_eq!(buf.pixels_mut()[last], 2);
    }

    #[test]
    fn a_cleared_buffer_hands_back_no_pixels() {
        let mut buf = ShmBuffer::empty();
        buf.set(1, anon(16), 2, 2);
        buf.clear();
        assert!(buf.pixels_mut().is_empty());
    }

    #[test]
    fn a_single_row_segment_is_the_row_stride() {
        assert_eq!(segment_size(800, 1), 3200);
        assert_eq!(segment_size(1, 600), 2400);
    }

    #[test]
    fn a_segment_is_four_bytes_per_pixel() {
        assert_eq!(segment_size(1, 1), 4);
        assert_eq!(segment_size(1920, 1080), 1920 * 1080 * 4);
        assert_eq!(segment_size(0, 1080), 0);
    }

    #[test]
    fn only_a_mapped_buffer_at_the_same_extent_is_reused() {
        assert!(can_reuse(true, (800, 600), 800, 600));
        assert!(!can_reuse(false, (800, 600), 800, 600));
        assert!(!can_reuse(true, (800, 600), 801, 600));
        assert!(!can_reuse(true, (800, 600), 800, 601));
    }
}
