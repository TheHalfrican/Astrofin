//! JSON serialization for values embedded directly in JavaScript source.

use serde::Serialize;

/// Serializes `value` as JSON safe to paste into JS source: the U+2028 and
/// U+2029 code points, which plain JSON leaves raw and a JS string literal
/// reads as line terminators, are escaped by the formatter below.
/// `None` when the value's `Serialize` impl fails.
pub fn to_js_json<T: Serialize + ?Sized>(value: &T) -> Option<String> {
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, JsSourceFormatter);
    value.serialize(&mut ser).ok()?;
    String::from_utf8(out).ok()
}

struct JsSourceFormatter;

impl serde_json::ser::Formatter for JsSourceFormatter {
    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        let mut rest = fragment;
        while let Some(at) = rest.find(['\u{2028}', '\u{2029}']) {
            writer.write_all(&rest.as_bytes()[..at])?;
            let (sep, tail) = rest[at..].split_at('\u{2028}'.len_utf8());
            writer.write_all(if sep == "\u{2028}" {
                b"\\u2028"
            } else {
                b"\\u2029"
            })?;
            rest = tail;
        }
        writer.write_all(rest.as_bytes())
    }

    /// Pre-serialized JSON (`serde_json::value::RawValue`, behind serde_json's
    /// `raw_value` feature, which nothing in this workspace enables today)
    /// bypasses `write_string_fragment` entirely — the default impl copies the
    /// fragment through verbatim, which would put a raw U+2028 back into the
    /// output. JSON has no use for either code point outside a string literal,
    /// so escaping the fragment is safe as well as necessary, and this closes
    /// the hole before a call site opens it.
    fn write_raw_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        self.write_string_fragment(writer, fragment)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn line_separators_inside_strings_are_escaped() {
        assert_eq!(
            to_js_json("a\u{2028}b\u{2029}c").as_deref(),
            Some("\"a\\u2028b\\u2029c\"")
        );
    }

    #[test]
    fn line_separators_in_object_keys_are_escaped() {
        let mut map = std::collections::BTreeMap::new();
        map.insert("k\u{2028}", 1);
        assert_eq!(to_js_json(&map).as_deref(), Some("{\"k\\u2028\":1}"));
    }

    #[test]
    fn output_matches_serde_json_when_no_line_separators() {
        let value = serde_json::json!({"a": [1, 2, "x\"y\n"], "b": null});
        assert_eq!(
            to_js_json(&value),
            serde_json::to_string(&value).ok(),
            "plain values must serialize identically"
        );
    }

    #[test]
    fn line_separators_survive_a_round_trip_as_json() {
        let out = to_js_json("a\u{2028}b").expect("serializes");
        // The escape is a JSON escape too, so the browser parses it back to
        // the original code point.
        let back: String = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(back, "a\u{2028}b");
    }

    #[test]
    fn every_js_string_hazard_is_escaped() {
        // What a hostile server-supplied string can carry into
        // `exec_js`/`var x = <json>;`: the quote and the backslash that would
        // close the literal, the control characters that would break the
        // line, and the two separators plain JSON leaves raw.
        let hostile = "\"\\\n\r\t\u{0}\u{1f}\u{2028}\u{2029}";
        let out = to_js_json(hostile).expect("serializes");
        for raw in ['\n', '\r', '\t', '\u{0}', '\u{1f}', '\u{2028}', '\u{2029}'] {
            assert!(!out.contains(raw), "{raw:?} left raw in {out}");
        }
        assert!(out.starts_with('"') && out.ends_with('"'));
        assert!(
            !out[1..out.len() - 1].replace("\\\"", "").contains('"'),
            "an unescaped quote would close the literal early: {out}"
        );
        let back: String = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(back, hostile);
    }

    #[test]
    fn a_script_end_tag_is_left_alone() {
        // `to_js_json` targets JS source, not HTML: `</script>` is only a
        // hazard inside an inline `<script>` block, and no call site embeds
        // its output in one.
        assert_eq!(to_js_json("</script>").as_deref(), Some("\"</script>\""));
    }

    #[test]
    fn nested_structures_are_escaped_at_every_depth() {
        let value = serde_json::json!({
            "a": ["x\u{2029}y", {"k\u{2028}": "v\u{2028}"}],
        });
        let out = to_js_json(&value).expect("serializes");
        assert!(!out.contains('\u{2028}'), "{out}");
        assert!(!out.contains('\u{2029}'), "{out}");
        assert_eq!(out.matches("\\u2028").count(), 2, "{out}");
        assert_eq!(out.matches("\\u2029").count(), 1, "{out}");
    }

    #[test]
    fn a_serializer_failure_is_none_rather_than_a_panic() {
        // A map with a non-string key cannot be JSON; the caller gets None
        // and falls back to its own literal.
        let mut map = std::collections::BTreeMap::new();
        map.insert(vec![1u8, 2], "v");
        assert_eq!(to_js_json(&map), None);
    }

    #[test]
    fn a_large_input_is_escaped_throughout() {
        let big = "a\u{2028}".repeat(50_000);
        let out = to_js_json(&big).expect("serializes");
        assert_eq!(out.matches("\\u2028").count(), 50_000);
        assert!(!out.contains('\u{2028}'));
    }
}
