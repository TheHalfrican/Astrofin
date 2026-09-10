//! Runtime version of the libcef loaded into this process.

use std::ffi::CStr;
use std::fmt;
use std::os::raw::c_int;
use std::sync::LazyLock;

use serde::{Serialize, Serializer};

// Entries (from CEF's cef_version.h): 0-2 CEF major/minor/patch,
// 3 commit number, 4-7 Chromium major/minor/build/patch.
unsafe extern "C" {
    fn cef_version_info(entry: c_int) -> c_int;
}

pub struct ShortHash([u8; 7]);

impl ShortHash {
    fn new(full: &str) -> Option<Self> {
        let bytes = full.as_bytes().get(..7)?;
        if !bytes.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let mut hash = [0u8; 7];
        hash.copy_from_slice(bytes);
        Some(Self(hash))
    }
}

impl fmt::Display for ShortHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(str::from_utf8(&self.0).unwrap_or_default())
    }
}

pub struct CefVersionInfo {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub commit: ShortHash,
    pub chromium: [u32; 4],
}

pub enum CefVersion {
    Known(CefVersionInfo),
    Unknown,
}

impl fmt::Display for CefVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(v) => write!(
                f,
                "{}.{}.{}+g{}+chromium-{}.{}.{}.{}",
                v.major,
                v.minor,
                v.patch,
                v.commit,
                v.chromium[0],
                v.chromium[1],
                v.chromium[2],
                v.chromium[3],
            ),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

/// `Known` serializes as the [`Display`](fmt::Display) form; `Unknown` as null.
impl Serialize for CefVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Known(_) => serializer.collect_str(self),
            Self::Unknown => serializer.serialize_none(),
        }
    }
}

fn commit_hash() -> Option<ShortHash> {
    // cef_api_hash's first call also configures the libcef API version;
    // it must get the same value the cef crate passes.
    let ptr = unsafe { cef::sys::cef_api_hash(cef::sys::CEF_API_VERSION_LAST, 2) };
    if ptr.is_null() {
        return None;
    }
    let full = unsafe { CStr::from_ptr(ptr) }.to_str().ok()?;
    ShortHash::new(full)
}

fn probe() -> CefVersion {
    let Some(commit) = commit_hash() else {
        return CefVersion::Unknown;
    };
    let v = |entry| unsafe { cef_version_info(entry) }.unsigned_abs();
    CefVersion::Known(CefVersionInfo {
        major: v(0),
        minor: v(1),
        patch: v(2),
        commit,
        chromium: [v(4), v(5), v(6), v(7)],
    })
}

static CEF_VERSION: LazyLock<CefVersion> = LazyLock::new(probe);

pub fn cef_version() -> &'static CefVersion {
    &CEF_VERSION
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn info() -> CefVersionInfo {
        CefVersionInfo {
            major: 151,
            minor: 3,
            patch: 24,
            commit: ShortHash::new("0123abcdef4567").expect("valid hash"),
            chromium: [139, 0, 7258, 128],
        }
    }

    #[test]
    fn short_hash_keeps_the_first_seven_hex_digits() {
        let h = ShortHash::new("0123abcdef4567").expect("valid hash");
        assert_eq!(h.to_string(), "0123abc");
    }

    #[test]
    fn short_hash_rejects_a_too_short_or_non_hex_string() {
        assert!(ShortHash::new("").is_none());
        assert!(ShortHash::new("abc123").is_none(), "six digits is too few");
        assert!(ShortHash::new("0123abz4567").is_none(), "z is not hex");
        assert!(ShortHash::new("0123 bc4567").is_none(), "space is not hex");
    }

    #[test]
    fn short_hash_accepts_exactly_seven_digits() {
        let h = ShortHash::new("deadbee").expect("valid hash");
        assert_eq!(h.to_string(), "deadbee");
    }

    #[test]
    fn a_known_version_prints_the_cef_and_chromium_halves() {
        assert_eq!(
            CefVersion::Known(info()).to_string(),
            "151.3.24+g0123abc+chromium-139.0.7258.128"
        );
    }

    #[test]
    fn an_unknown_version_prints_the_word_unknown() {
        assert_eq!(CefVersion::Unknown.to_string(), "unknown");
    }

    #[test]
    fn a_known_version_serialises_as_its_display_string() {
        let json = serde_json::to_string(&CefVersion::Known(info())).expect("serialises");
        assert_eq!(json, "\"151.3.24+g0123abc+chromium-139.0.7258.128\"");
    }

    #[test]
    fn an_unknown_version_serialises_as_null() {
        let json = serde_json::to_string(&CefVersion::Unknown).expect("serialises");
        assert_eq!(json, "null");
    }

    #[test]
    fn cef_version_probes_the_linked_libcef_once() {
        crate::test_support::ensure_cef_loaded();

        let first = cef_version();
        let second = cef_version();
        assert!(
            std::ptr::eq(first, second),
            "the probe result is cached for the process"
        );
        // The linked libcef is a real one, so the probe must resolve — and
        // its rendering must be the same on every read.
        assert_eq!(first.to_string(), second.to_string());
        assert!(!first.to_string().is_empty());
    }
}
