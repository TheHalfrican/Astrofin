//! libmpv error codes wrapped in `Result`.

use crate::sys;
use std::ffi::CStr;
use std::fmt;

/// libmpv error. `code` is the negative integer libmpv returns; the string
/// payload is the static `mpv_error_string` lookup at construction time.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.message())]
pub struct Error {
    pub code: i32,
}

impl Error {
    pub fn new(code: i32) -> Self {
        Self { code }
    }

    /// Human-readable error string from libmpv. Never null — libmpv falls
    /// back to "unknown error" for out-of-range codes.
    pub fn message(&self) -> &'static str {
        // SAFETY: mpv_error_string returns a pointer to a static string.
        let ptr = unsafe { sys::mpv_error_string(self.code) };
        if ptr.is_null() {
            return "unknown mpv error";
        }
        unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .unwrap_or("invalid utf-8")
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mpv::Error({}: {})", self.code, self.message())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Wrap a libmpv return code into `Result<()>`. libmpv contract: `>= 0` on
/// success, negative on failure.
pub(crate) fn check(code: i32) -> Result<()> {
    if code >= 0 {
        Ok(())
    } else {
        Err(Error::new(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_keeps_the_code_libmpv_returned() {
        assert_eq!(Error::new(-4).code, -4);
        assert_eq!(Error::new(0).code, 0);
        assert_eq!(Error::new(i32::MIN).code, i32::MIN);
    }

    #[test]
    fn message_is_libmpvs_own_text_for_a_known_code() {
        let success = Error::new(sys::mpv_error::MPV_ERROR_SUCCESS.0).message();
        let invalid = Error::new(sys::mpv_error::MPV_ERROR_INVALID_PARAMETER.0).message();
        assert!(!success.is_empty());
        assert!(!invalid.is_empty());
        assert_ne!(
            success, invalid,
            "distinct codes must not share one description"
        );
    }

    /// libmpv answers every out-of-range code with the same fallback string,
    /// so `message` never has to invent one of its own.
    #[test]
    fn message_falls_back_to_one_text_for_out_of_range_codes() {
        let a = Error::new(-9999).message();
        let b = Error::new(-31337).message();
        assert_eq!(a, b);
        assert_ne!(a, Error::new(sys::mpv_error::MPV_ERROR_SUCCESS.0).message());
    }

    #[test]
    fn display_is_the_message_alone() {
        let e = Error::new(sys::mpv_error::MPV_ERROR_PROPERTY_NOT_FOUND.0);
        assert_eq!(e.to_string(), e.message());
    }

    #[test]
    fn debug_shows_the_code_beside_the_message() {
        let e = Error::new(sys::mpv_error::MPV_ERROR_INVALID_PARAMETER.0);
        let text = format!("{e:?}");
        assert!(text.starts_with("mpv::Error(-4: "), "{text}");
        assert!(text.ends_with(')'), "{text}");
        assert!(text.contains(e.message()), "{text}");
    }

    /// libmpv's contract: `>= 0` is success, negative carries the code.
    #[test]
    fn check_treats_zero_and_positive_as_success() {
        assert_eq!(check(0), Ok(()));
        assert_eq!(check(1), Ok(()));
        assert_eq!(check(i32::MAX), Ok(()));
    }

    #[test]
    fn check_wraps_a_negative_code_into_the_error() {
        assert_eq!(check(-1), Err(Error::new(-1)));
        assert_eq!(check(-4).unwrap_err().code, -4);
        assert_eq!(check(i32::MIN).unwrap_err().code, i32::MIN);
    }
}
