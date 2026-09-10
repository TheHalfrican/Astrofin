//! How much room is left where a directory lives.
//!
//! Behind a trait with one method, because the only caller — the legacy-profile
//! import in [`crate::migrate`] — makes a decision from the answer, and that
//! decision has to be testable without a full disk.

use std::path::Path;

/// Free space on the filesystem holding a path, in bytes.
///
/// `None` means "could not tell", never "nothing free": every caller treats an
/// unknown as permission to go ahead, because a filesystem whose free space we
/// cannot read (a network share, a container mount) is still usually writable.
pub(crate) trait FreeSpace {
    fn free_bytes(&self, path: &Path) -> Option<u64>;
}

/// The real filesystem: `statvfs` on unix, `GetDiskFreeSpaceExW` on Windows.
pub(crate) struct Disk;

/// Widen a libc count whose width is not the same on every unix: `f_frsize`
/// and `f_bavail` are 32-bit on macOS and 64-bit on Linux, so neither
/// `u64::from` nor `u64::try_from` is free of a `useless_conversion` warning
/// on both.
#[cfg(unix)]
fn widen(value: impl TryInto<u64>) -> Option<u64> {
    value.try_into().ok()
}

#[cfg(unix)]
impl FreeSpace for Disk {
    fn free_bytes(&self, path: &Path) -> Option<u64> {
        let stat = nix::sys::statvfs::statvfs(path).ok()?;
        // `f_bavail` — blocks available to an unprivileged process — not
        // `f_bfree`, which counts the root reserve we cannot spend.
        widen(stat.fragment_size())?.checked_mul(widen(stat.blocks_available())?)
    }
}

#[cfg(windows)]
impl FreeSpace for Disk {
    fn free_bytes(&self, path: &Path) -> Option<u64> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return None;
        }
        wide.push(0);
        let mut available: u64 = 0;
        // "Available to the calling user", which is what a quota makes
        // smaller than the volume's own free space.
        // SAFETY: `wide` is NUL-terminated and outlives the call; the other
        // two out-parameters are optional and passed as null.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(available)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{Disk, FreeSpace};
    use std::path::Path;

    #[test]
    fn the_disk_reports_some_free_space_for_a_directory_we_just_created() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let free = Disk.free_bytes(tmp.path()).expect("free space");
        // A machine with a truly full temp directory would fail this, and so
        // would the import it guards — which is the point of the check.
        assert!(free > 0, "no free space reported for a writable temp dir");
    }

    #[test]
    fn the_disk_cannot_report_free_space_for_a_path_that_does_not_exist() {
        let missing = Path::new("/definitely/not/a/mount/point/astrofin-test");
        assert_eq!(Disk.free_bytes(missing), None);
    }
}
