//! Rust `&str` -> libmpv `CString`, with libmpv's own error for the one
//! string it cannot take.
//!
//! Every `Handle` entry point that names a property, an option or a value has
//! to make this conversion, and every one of them turns the same failure into
//! the same libmpv error code. It is the whole of the pure logic in
//! `handle.rs`, so it lives here where it is testable without an mpv core.

use std::ffi::CString;

use crate::error::{Error, Result};
use crate::sys;

/// A NUL-terminated copy of `s`.
///
/// A string with an interior NUL is not representable in C and is refused as
/// `MPV_ERROR_INVALID_PARAMETER` — the same answer libmpv gives for an
/// argument it cannot parse, so callers have one error to handle rather than
/// two.
pub(crate) fn c_string(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error::new(sys::mpv_error::MPV_ERROR_INVALID_PARAMETER.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_becomes_a_nul_terminated_copy() {
        let c = c_string("time-pos").expect("no interior NUL");
        assert_eq!(c.as_bytes(), b"time-pos");
        assert_eq!(c.as_bytes_with_nul(), b"time-pos\0");
    }

    #[test]
    fn an_empty_string_is_representable() {
        let c = c_string("").expect("no interior NUL");
        assert_eq!(c.as_bytes(), b"");
    }

    #[test]
    fn utf8_survives_the_conversion() {
        let c = c_string("/media/Fügen.mkv").expect("no interior NUL");
        assert_eq!(c.as_bytes(), "/media/Fügen.mkv".as_bytes());
    }

    #[test]
    fn an_interior_nul_is_refused_as_an_invalid_parameter() {
        let err = c_string("time\0pos").expect_err("interior NUL");
        assert_eq!(
            err,
            Error::new(sys::mpv_error::MPV_ERROR_INVALID_PARAMETER.0)
        );
    }
}
