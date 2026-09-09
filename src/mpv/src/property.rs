//! Format trait for typed property/option access.

use crate::sys;

/// Maps a Rust type to its libmpv format tag. Used by `Handle::get_property`,
/// `Handle::set_property_async`, `Handle::set_option`, and
/// `Handle::observe_property`.
pub trait Format: Sized {
    const MPV_FORMAT: sys::mpv_format;
}

impl Format for i64 {
    const MPV_FORMAT: sys::mpv_format = sys::mpv_format::MPV_FORMAT_INT64;
}

impl Format for f64 {
    const MPV_FORMAT: sys::mpv_format = sys::mpv_format::MPV_FORMAT_DOUBLE;
}

/// libmpv's flag format. Stored as `i32` (0 or 1) on the wire; the safe API
/// exposes `bool`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flag(pub bool);

impl From<bool> for Flag {
    fn from(b: bool) -> Self {
        Self(b)
    }
}

impl From<Flag> for bool {
    fn from(f: Flag) -> Self {
        f.0
    }
}

impl Format for Flag {
    const MPV_FORMAT: sys::mpv_format = sys::mpv_format::MPV_FORMAT_FLAG;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_wraps_a_bool_without_changing_it() {
        assert_eq!(Flag::from(true), Flag(true));
        assert_eq!(Flag::from(false), Flag(false));
    }

    #[test]
    fn unwrapping_a_flag_gives_the_bool_back() {
        assert!(bool::from(Flag(true)));
        assert!(!bool::from(Flag(false)));
        // Round trip in both directions.
        for b in [true, false] {
            assert_eq!(bool::from(Flag::from(b)), b);
        }
    }

    /// The format tag is what `mpv_get_property` is told to decode into, so a
    /// wrong one here silently misreads every property of that type.
    #[test]
    fn each_rust_type_carries_its_libmpv_format_tag() {
        assert_eq!(
            <i64 as Format>::MPV_FORMAT,
            sys::mpv_format::MPV_FORMAT_INT64
        );
        assert_eq!(
            <f64 as Format>::MPV_FORMAT,
            sys::mpv_format::MPV_FORMAT_DOUBLE
        );
        assert_eq!(
            <Flag as Format>::MPV_FORMAT,
            sys::mpv_format::MPV_FORMAT_FLAG
        );
    }
}
