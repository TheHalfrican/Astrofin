//! Token redaction for log output. Detects known query-param / JSON / header
//! patterns that precede a Jellyfin access token and overwrites the token
//! value with 'x' characters in place, preserving URL/JSON shape.
//!
//! Matching is ASCII-case-insensitive: `api_key=`, `API_KEY=` and `ApiKey=`
//! are the same rule, because the shape a token is logged in depends on
//! whichever component (jellyfin-web, CEF, mpv, our own code) wrote the line.
//! Every needle in [`RULES`] is therefore written lowercase and compared
//! against a lowercased copy of the buffer; the copy has the same byte
//! offsets, so the spans it yields index the original buffer directly.
//!
//! Known limits, deliberate:
//!   - Redaction is per emitted record. A token split across two records —
//!     e.g. a CEF stderr line longer than the 4 KiB capture chunk — is not
//!     reassembled, so each half passes through unredacted.
//!   - Only the shapes below are recognised. A brand-new credential shape in
//!     a log line is not redacted until it is added here.

use memchr::memmem;

struct PatternRule {
    /// Lowercase ASCII. Compared against a lowercased copy of the buffer.
    needle: &'static [u8],
    /// Bytes that end the secret that follows `needle`.
    terminators: &'static [u8],
}

const URL_TERMINATORS: &[u8] = b"&\"' \t\r\n;<>";
const QUOTE_TERMINATORS: &[u8] = b"\"";

/// Bytes that end a URL authority, for the userinfo scan.
const AUTHORITY_TERMINATORS: &[u8] = b"/?#&\"' \t\r\n;<>";

const RULES: &[PatternRule] = &[
    // Query parameters: `?api_key=...`, `&ApiKey=...`.
    PatternRule {
        needle: b"api_key=",
        terminators: URL_TERMINATORS,
    },
    PatternRule {
        needle: b"apikey=",
        terminators: URL_TERMINATORS,
    },
    // Any `*Token=` query parameter or form field: X-Emby-Token,
    // X-MediaBrowser-Token, AccessToken, DeviceToken, bare Token.
    PatternRule {
        needle: b"token=",
        terminators: URL_TERMINATORS,
    },
    // The same, percent-encoded, as it appears inside a proxied URL.
    PatternRule {
        needle: b"token%3d",
        terminators: URL_TERMINATORS,
    },
    // Header forms, with and without the conventional space:
    // `X-Emby-Token: abc`, `X-MediaBrowser-Token:abc`.
    PatternRule {
        needle: b"token: ",
        terminators: URL_TERMINATORS,
    },
    PatternRule {
        needle: b"token:",
        terminators: URL_TERMINATORS,
    },
    // `Authorization: MediaBrowser Client="...", Token="abc"`. Only the token
    // is blanked; the client/device fields stay readable for debugging.
    PatternRule {
        needle: b"token=\"",
        terminators: QUOTE_TERMINATORS,
    },
    PatternRule {
        needle: b"bearer ",
        terminators: URL_TERMINATORS,
    },
    // JSON bodies: {"AccessToken":"abc"}, {"Token": "abc"}.
    PatternRule {
        needle: b"token\":\"",
        terminators: QUOTE_TERMINATORS,
    },
    PatternRule {
        needle: b"token\": \"",
        terminators: QUOTE_TERMINATORS,
    },
    // Passwords travel the same paths as tokens (AuthenticateByName body,
    // query strings on older endpoints).
    PatternRule {
        needle: b"password=",
        terminators: URL_TERMINATORS,
    },
    PatternRule {
        needle: b"password\":\"",
        terminators: QUOTE_TERMINATORS,
    },
    PatternRule {
        needle: b"password\": \"",
        terminators: QUOTE_TERMINATORS,
    },
    // Jellyfin's short field name, e.g. {"Username":"x","Pw":"secret"}.
    PatternRule {
        needle: b"pw\":\"",
        terminators: QUOTE_TERMINATORS,
    },
    PatternRule {
        needle: b"pw\": \"",
        terminators: QUOTE_TERMINATORS,
    },
];

fn find_token_end(buf: &[u8], from: usize, terminators: &[u8]) -> usize {
    buf[from..]
        .iter()
        .position(|c| terminators.contains(c))
        .map(|p| p + from)
        .unwrap_or(buf.len())
}

/// Push a `(start, end)` span for every non-empty secret this rule finds.
/// Every occurrence is reported, not just the first: a line that carries an
/// empty `api_key=` before a populated one must still be censored.
fn rule_spans(lower: &[u8], rule: &PatternRule, out: &mut Vec<(usize, usize)>) {
    for pos in memmem::find_iter(lower, rule.needle) {
        let start = pos + rule.needle.len();
        let end = find_token_end(lower, start, rule.terminators);
        if end > start {
            out.push((start, end));
        }
    }
}

/// Push a span for the password half of any `scheme://user:password@host`
/// authority. The user half is left readable; only the credential goes.
fn userinfo_spans(lower: &[u8], out: &mut Vec<(usize, usize)>) {
    for pos in memmem::find_iter(lower, b"://") {
        let start = pos + 3;
        let end = find_token_end(lower, start, AUTHORITY_TERMINATORS);
        let authority = &lower[start..end];
        let Some(at) = authority.iter().rposition(|&c| c == b'@') else {
            continue;
        };
        let Some(colon) = authority[..at].iter().position(|&c| c == b':') else {
            continue;
        };
        if at > colon + 1 {
            out.push((start + colon + 1, start + at));
        }
    }
}

/// Every span of `buf` that holds a secret, in no particular order. Spans may
/// overlap; all of them get blanked.
fn secret_spans(buf: &[u8]) -> Vec<(usize, usize)> {
    let lower = buf.to_ascii_lowercase();
    let mut spans = Vec::new();
    for rule in RULES {
        rule_spans(&lower, rule, &mut spans);
    }
    userinfo_spans(&lower, &mut spans);
    spans
}

pub fn contains_secret(buf: &[u8]) -> bool {
    !secret_spans(buf).is_empty()
}

pub fn censor(buf: &mut [u8]) {
    for (start, end) in secret_spans(buf) {
        for b in &mut buf[start..end] {
            *b = b'x';
        }
    }
}

/// Cap on the escaped length of one page-supplied string. A page can hand the
/// browser process a megabyte-long "URL"; the log line records what it was,
/// not the whole of it.
const PAGE_STRING_LIMIT: usize = 512;

/// Render a string that came from a web page so it cannot forge log output.
///
/// Every C0 control character (and DEL) is escaped — `\n` as `\\n`, `\r` as
/// `\\r`, `\t` as `\\t`, everything else as `\\xNN` — so one page string can
/// never become two log records, move the cursor with an ANSI escape, or fake
/// a level and category prefix. A backslash is doubled so the escaping is
/// unambiguous. The result is truncated to [`PAGE_STRING_LIMIT`] escaped
/// characters with a `…` marker.
///
/// This is the companion of [`censor`]: that one removes secrets from a
/// finished record, this one is applied to each untrusted fragment *before*
/// it is formatted into one.
#[must_use]
pub fn escape_page_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut truncated = false;
    for c in s.chars() {
        if out.chars().count() >= PAGE_STRING_LIMIT {
            truncated = true;
            break;
        }
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // C0 and DEL. U+2028/U+2029 are line breaks to a JS parser but not
            // to a log reader, so they are left alone here.
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    if truncated {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn censor_str(s: &str) -> String {
        let mut bytes = s.as_bytes().to_vec();
        censor(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[test]
    fn url_token() {
        assert_eq!(
            censor_str("/path?api_key=abc123&x=1"),
            "/path?api_key=xxxxxx&x=1"
        );
        assert!(contains_secret(b"/path?api_key=abc"));
    }

    #[test]
    fn json_token() {
        assert_eq!(
            censor_str("\"AccessToken\":\"abc\""),
            "\"AccessToken\":\"xxx\""
        );
    }

    #[test]
    fn empty_token() {
        assert_eq!(censor_str("api_key=&x=1"), "api_key=&x=1");
        assert!(!contains_secret(b"api_key=&x=1"));
    }

    #[test]
    fn header_encoded() {
        assert_eq!(
            censor_str("X-MediaBrowser-Token%3Dabcdef HTTP"),
            "X-MediaBrowser-Token%3Dxxxxxx HTTP"
        );
    }

    #[test]
    fn repeated_tokens_on_one_line() {
        assert_eq!(
            censor_str("a?api_key=aa&b?api_key=bb"),
            "a?api_key=xx&b?api_key=xx"
        );
    }

    #[test]
    fn token_at_end_of_buffer() {
        assert_eq!(censor_str("?api_key=abc"), "?api_key=xxx");
    }

    #[test]
    fn no_pattern() {
        assert_eq!(censor_str("plain message"), "plain message");
        assert!(!contains_secret(b"plain message"));
    }

    // ---- security audit, phase 1 ----

    #[test]
    fn an_empty_first_occurrence_does_not_hide_a_later_secret() {
        // Regression: `contains_secret` only inspected the *first* match of
        // each needle, so a line that opened with an empty `api_key=` was
        // declared clean and the real token behind it reached the log.
        let line = "GET /a?api_key=&b=1 -> /c?api_key=REALSECRET";
        assert!(contains_secret(line.as_bytes()));
        let censored = censor_str(line);
        assert!(!censored.contains("REALSECRET"), "{censored}");
        assert_eq!(censored, "GET /a?api_key=&b=1 -> /c?api_key=xxxxxxxxxx");
    }

    #[test]
    fn matching_is_case_insensitive() {
        for line in [
            "?API_KEY=SECRETVAL",
            "?ApiKey=SECRETVAL",
            "?apikey=SECRETVAL",
            "?AccessToken=SECRETVAL",
            "?accesstoken=SECRETVAL",
            "X-MEDIABROWSER-TOKEN%3DSECRETVAL",
        ] {
            let censored = censor_str(line);
            assert!(
                !censored.contains("SECRETVAL"),
                "not redacted: {line} -> {censored}"
            );
            assert!(contains_secret(line.as_bytes()), "not detected: {line}");
        }
    }

    #[test]
    fn emby_token_header_is_redacted_in_both_spellings() {
        assert_eq!(
            censor_str("X-Emby-Token: abc123\n"),
            "X-Emby-Token: xxxxxx\n"
        );
        assert_eq!(censor_str("x-emby-token:abc123"), "x-emby-token:xxxxxx");
        assert!(contains_secret(b"X-Emby-Token: abc123"));
    }

    #[test]
    fn media_browser_authorization_header_keeps_everything_but_the_token() {
        let line = concat!(
            r#"Authorization: MediaBrowser Client="Astrofin", Device="pc", "#,
            r#"DeviceId="d1", Version="0.5.0", Token="8f14e45fceea167a""#
        );
        let censored = censor_str(line);
        assert!(!censored.contains("8f14e45fceea167a"), "{censored}");
        assert!(censored.contains(r#"Client="Astrofin""#), "{censored}");
        assert!(censored.contains(r#"DeviceId="d1""#), "{censored}");
        assert!(
            censored.contains(r#"Token="xxxxxxxxxxxxxxxx""#),
            "{censored}"
        );
    }

    #[test]
    fn bearer_credentials_are_redacted() {
        assert_eq!(
            censor_str("Authorization: Bearer eyJhbGciOi.J9 rest"),
            "Authorization: Bearer xxxxxxxxxxxxx rest"
        );
    }

    #[test]
    fn json_access_token_survives_pretty_printing() {
        assert_eq!(
            censor_str(r#"{"AccessToken": "abc", "User": {}}"#),
            r#"{"AccessToken": "xxx", "User": {}}"#
        );
        assert_eq!(censor_str(r#"{"Token":"abc"}"#), r#"{"Token":"xxx"}"#);
    }

    #[test]
    fn passwords_are_redacted_in_query_and_json_form() {
        assert_eq!(censor_str("?password=hunter2&x"), "?password=xxxxxxx&x");
        assert_eq!(
            censor_str(r#"{"Username":"noah","Pw":"hunter2"}"#),
            r#"{"Username":"noah","Pw":"xxxxxxx"}"#
        );
        assert_eq!(
            censor_str(r#"{"CurrentPassword": "hunter2"}"#),
            r#"{"CurrentPassword": "xxxxxxx"}"#
        );
    }

    #[test]
    fn url_userinfo_password_is_redacted_but_the_host_is_kept() {
        assert_eq!(
            censor_str("connecting to http://noah:hunter2@jelly.example:8096/web"),
            "connecting to http://noah:xxxxxxx@jelly.example:8096/web"
        );
        assert!(contains_secret(b"https://u:p@host/"));
        // A port is not a password.
        assert_eq!(
            censor_str("http://jelly.example:8096/web"),
            "http://jelly.example:8096/web"
        );
        assert!(!contains_secret(b"http://jelly.example:8096/web"));
        // Userinfo with no password is left alone.
        assert_eq!(censor_str("http://noah@host/"), "http://noah@host/");
        // An `@` in the path or query is not userinfo.
        assert_eq!(
            censor_str("http://host/Users/a:b@c"),
            "http://host/Users/a:b@c"
        );
    }

    #[test]
    fn several_distinct_shapes_on_one_line_are_all_redacted() {
        let line = concat!(
            "http://u:PWSECRET@host/Items?api_key=KEYSECRET ",
            r#"hdr X-Emby-Token: HDRSECRET body {"AccessToken":"JSONSECRET"}"#
        );
        let censored = censor_str(line);
        for secret in ["PWSECRET", "KEYSECRET", "HDRSECRET", "JSONSECRET"] {
            assert!(!censored.contains(secret), "{secret} leaked: {censored}");
        }
    }

    #[test]
    fn censoring_preserves_length_and_utf8() {
        let line = "?api_key=ключ&x=1";
        let censored = censor_str(line);
        assert_eq!(censored.len(), line.len());
        assert!(!censored.contains("ключ"));
        assert!(censored.ends_with("&x=1"));
        // The replacement is still valid UTF-8 (no lossy replacement char).
        assert!(!censored.contains('\u{fffd}'));
    }

    #[test]
    fn a_secret_that_runs_to_the_end_of_the_buffer_is_redacted() {
        assert_eq!(censor_str("X-Emby-Token: abc"), "X-Emby-Token: xxx");
        assert_eq!(
            censor_str(r#"{"AccessToken":"abc"#),
            r#"{"AccessToken":"xxx"#
        );
        assert_eq!(censor_str("http://u:p@"), "http://u:x@");
    }

    #[test]
    fn non_utf8_and_empty_buffers_are_handled() {
        let mut empty: Vec<u8> = Vec::new();
        censor(&mut empty);
        assert!(empty.is_empty());
        assert!(!contains_secret(&[]));

        let mut binary = b"\xff\xfeapi_key=\xff\xfe secret".to_vec();
        censor(&mut binary);
        assert_eq!(&binary, b"\xff\xfeapi_key=xx secret");
    }

    #[test]
    fn a_token_split_across_two_records_is_a_known_gap() {
        // Documented limit, pinned so a future reassembly change is visible:
        // redaction sees one record at a time.
        let mut first = b"GET /Items?api_k".to_vec();
        let mut second = b"ey=REALSECRET&x=1".to_vec();
        censor(&mut first);
        censor(&mut second);
        assert_eq!(&first, b"GET /Items?api_k");
        assert_eq!(&second, b"ey=REALSECRET&x=1");
    }

    #[test]
    fn escape_page_string_keeps_a_hostile_string_on_one_line() {
        let hostile = "http://host/\nERROR   [Main] wiped the disk\u{1b}[2J\r\n";
        let escaped = escape_page_string(hostile);
        assert!(!escaped.contains('\n'), "{escaped}");
        assert!(!escaped.contains('\r'), "{escaped}");
        assert!(!escaped.contains('\u{1b}'), "{escaped}");
        assert_eq!(
            escaped,
            "http://host/\\nERROR   [Main] wiped the disk\\x1b[2J\\r\\n"
        );
    }

    #[test]
    fn escape_page_string_leaves_an_ordinary_url_untouched() {
        // The shapes the connect screen actually logs must read exactly as
        // they were typed.
        for plain in [
            "http://192.168.1.10:8096",
            "http://thehalfrican-truenas.tail1cdca8.ts.net:8096/web/index.html",
            "https://jf.example.com:8920/Videos/1/stream?x=1",
            "",
        ] {
            assert_eq!(escape_page_string(plain), plain);
        }
        // Non-ASCII text is not mangled either.
        assert_eq!(escape_page_string("naïve — 日本語"), "naïve — 日本語");
    }

    #[test]
    fn escape_page_string_doubles_a_backslash_and_escapes_nul_and_del() {
        assert_eq!(escape_page_string("a\\nb"), "a\\\\nb");
        assert_eq!(escape_page_string("a\u{0}b\u{7f}"), "a\\x00b\\x7f");
        assert_eq!(escape_page_string("\t"), "\\t");
    }

    #[test]
    fn escape_page_string_truncates_an_unbounded_page_string() {
        let long = "a".repeat(PAGE_STRING_LIMIT * 4);
        let escaped = escape_page_string(&long);
        assert_eq!(escaped.chars().count(), PAGE_STRING_LIMIT + 1);
        assert!(escaped.ends_with('…'));
    }
}
