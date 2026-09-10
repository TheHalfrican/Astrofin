//! The crate's single conversion from a CEF userfree UTF-16 string to a Rust
//! `String`.

use cef::{CefStringUserfreeUtf16, sys};

/// The Rust string for a CEF string's UTF-16 units.
///
/// `None` — no `cef_string_t`, a null buffer, or a zero length — and an empty
/// buffer are both the empty string; unpaired surrogates are replaced rather
/// than rejected, because a page title is not worth dropping over one.
pub(crate) fn utf16_to_string(units: Option<&[u16]>) -> String {
    units.map(String::from_utf16_lossy).unwrap_or_default()
}

/// Empty string for a null or zero-length CEF string.
pub(crate) fn userfree_to_string(s: &CefStringUserfreeUtf16) -> String {
    let raw: Option<&sys::_cef_string_utf16_t> = s.into();
    // SAFETY: a non-null `str_` with a non-zero `length` is CEF's own live
    // buffer for the duration of this borrow.
    let units = raw.and_then(|r| {
        (!r.str_.is_null() && r.length != 0)
            .then(|| unsafe { std::slice::from_raw_parts(r.str_, r.length) })
    });
    utf16_to_string(units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_units_become_the_string_they_spell() {
        let units: Vec<u16> = "Astrofin".encode_utf16().collect();
        assert_eq!(utf16_to_string(Some(&units)), "Astrofin");
    }

    #[test]
    fn an_absent_or_empty_buffer_is_the_empty_string() {
        assert_eq!(utf16_to_string(None), "");
        assert_eq!(utf16_to_string(Some(&[])), "");
    }

    #[test]
    fn a_surrogate_pair_survives_the_round_trip() {
        let units: Vec<u16> = "\u{1F3AC} Clapper".encode_utf16().collect();
        assert_eq!(utf16_to_string(Some(&units)), "\u{1F3AC} Clapper");
    }

    #[test]
    fn an_unpaired_surrogate_is_replaced_rather_than_dropping_the_string() {
        // A lone high surrogate between two ASCII characters.
        assert_eq!(utf16_to_string(Some(&[0x41, 0xD800, 0x42])), "A\u{FFFD}B");
    }

    #[test]
    fn an_embedded_nul_is_kept_as_a_character() {
        // CEF strings are counted, not NUL-terminated.
        assert_eq!(utf16_to_string(Some(&[0x41, 0x00, 0x42])), "A\0B");
    }
}
