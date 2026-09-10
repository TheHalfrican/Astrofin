//! `app://` scheme handler.
//!
//! Embedded resources are included at compile time from `src/web/*`. Two
//! URLs need dynamic generation:
//! - `app://resources/theme.css` — `:root{--bg-color:#RRGGBB}` from the
//!   compile-time background color constant.
//! - `app://resources/about.js` — a `var _aboutData = {...};` prefix
//!   prepended to the static about.js body.

use cef::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Serialize;

use crate::version::CefVersion;

// ---- embedded resources ----------------------------------------------------

struct Embedded {
    bytes: &'static [u8],
    mime: &'static str,
}

macro_rules! embedded {
    ($name:literal, $mime:literal) => {
        (
            $name,
            Embedded {
                bytes: include_bytes!(concat!("../../web/", $name)),
                mime: $mime,
            },
        )
    };
}

// URL key is the path after the `app://` scheme (no leading slash).
static RESOURCES: &[(&str, Embedded)] = &[
    embedded!("ab-loop.js", "application/javascript"),
    embedded!("about.html", "text/html"),
    embedded!("about.js", "application/javascript"),
    embedded!("astrofin-fonts.css", "text/css"),
    embedded!("astrofin-tokens.css", "text/css"),
    embedded!("client-settings.js", "application/javascript"),
    embedded!("connectivityHelper.js", "application/javascript"),
    embedded!("input-plugin.js", "application/javascript"),
    embedded!("logo-mark.svg", "image/svg+xml"),
    embedded!("mpv-audio-player.js", "application/javascript"),
    embedded!("mpv-player-base.js", "application/javascript"),
    embedded!("mpv-video-player.js", "application/javascript"),
    embedded!("native-shim.js", "application/javascript"),
    embedded!("overlay.css", "text/css"),
    embedded!("overlay.html", "text/html"),
    embedded!("overlay.js", "application/javascript"),
    embedded!("overlay.lang.js", "application/javascript"),
    embedded!("playback-source.js", "application/javascript"),
    embedded!("video-mode-resolver.js", "application/javascript"),
];

fn lookup(url_path: &str) -> Option<&'static Embedded> {
    // URL key has the "resources/" prefix; strip it to match RESOURCES.
    // Exact name match against a fixed table — no filesystem lookup, so a
    // `..` or an absolute path in the URL simply misses.
    let name = url_path.strip_prefix("resources/")?;
    RESOURCES.iter().find(|(n, _)| *n == name).map(|(_, r)| r)
}

/// `app://resources/about.js?v=2#x` -> `resources/about.js`.
///
/// The URL comes from the page (any frame can request an `app://` URL, and
/// the scheme is registered CORS- and fetch-enabled), so this only ever
/// produces a lookup key for the table above; it never touches the disk.
fn url_to_resource_path(url: &str) -> &str {
    // Strip the scheme prefix and the query/fragment.
    let after_scheme = match url.find("://") {
        // `find` returns a char boundary and "://" is ASCII, so the slice
        // index is always on a boundary.
        Some(p) => &url[p + 3..],
        None => url,
    };
    after_scheme.split(['?', '#']).next().unwrap_or("")
}

// Background color from src/color.h:40 — kBgColor{0x101010}.
const BG_COLOR_HEX: &str = "#101010";

fn theme_css() -> Vec<u8> {
    format!(":root{{--bg-color:{BG_COLOR_HEX}}}").into_bytes()
}

/// GPL-2 section 2(a) asks a modified version to say so; for a GUI the
/// customary place is the About box rather than a startup banner.
const UPSTREAM_CREDIT: &str = "Jellium Desktop (GPL-2.0)";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AboutData<'a> {
    app: &'a str,
    cef: &'a CefVersion,
    based_on: &'a str,
    config_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    log_file: Option<String>,
}

fn about_js_payload() -> Vec<u8> {
    let log_path = jfn_logging::active_path();
    let data = AboutData {
        app: crate::APP_VERSION_FULL,
        cef: crate::cef_version(),
        based_on: UPSTREAM_CREDIT,
        config_dir: abs_path(&jfn_paths::config_dir().to_string_lossy()),
        log_file: (!log_path.is_empty()).then(|| abs_path(&log_path)),
    };
    let json = jfn_js_json::to_js_json(&data).unwrap_or_else(|| "{}".to_string());
    let prefix = format!("var _aboutData = {json};\n");

    let static_body = RESOURCES
        .iter()
        .find(|(n, _)| *n == "about.js")
        .map(|(_, r)| r.bytes)
        .unwrap_or(&[]);

    let mut out = Vec::with_capacity(prefix.len() + static_body.len());
    out.extend_from_slice(prefix.as_bytes());
    out.extend_from_slice(static_body);
    out
}

// Absolute-but-not-resolved: prepend CWD if relative, leave symlinks/../.
// alone. Fall back to input on error.
fn abs_path(p: &str) -> String {
    let pb = std::path::Path::new(p);
    if pb.is_absolute() {
        return p.to_string();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(pb).to_string_lossy().into_owned(),
        Err(_) => p.to_string(),
    }
}

// ---- SchemeHandlerFactory --------------------------------------------------

#[derive(Clone)]
pub(crate) struct JfnSchemeFactory;

wrap_scheme_handler_factory! {
    pub(crate) struct JfnSchemeFactoryBuilder { inner: JfnSchemeFactory, }

    impl SchemeHandlerFactory {
        fn create(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let request = request?;
            let url_uf = request.url();
            let url = crate::cef_string::userfree_to_string(&url_uf);
            let url_path = url_to_resource_path(&url);

            let (bytes, mime): (Vec<u8>, &'static str) = if url_path == "resources/theme.css" {
                (theme_css(), "text/css")
            } else if url_path == "resources/about.js" {
                (about_js_payload(), "application/javascript")
            } else if let Some(r) = lookup(url_path) {
                (r.bytes.to_vec(), r.mime)
            } else {
                jfn_logging::log(
                    jfn_logging::CATEGORY_RESOURCE,
                    jfn_logging::LEVEL_WARN,
                    &format!("EmbeddedScheme not found: {url_path}"),
                );
                return None;
            };

            Some(
                JfnResourceHandlerBuilder::new(JfnResourceHandler {
                    bytes: Arc::new(bytes),
                    mime,
                    offset: Arc::new(AtomicUsize::new(0)),
                }),
            )
        }
    }
}

// ---- ResourceHandler -------------------------------------------------------

#[derive(Clone)]
pub(crate) struct JfnResourceHandler {
    bytes: Arc<Vec<u8>>,
    mime: &'static str,
    offset: Arc<AtomicUsize>,
}

wrap_resource_handler! {
    pub(crate) struct JfnResourceHandlerBuilder { inner: JfnResourceHandler, }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut ::std::os::raw::c_int>,
            _callback: Option<&mut Callback>,
        ) -> ::std::os::raw::c_int {
            if let Some(h) = handle_request { *h = 1; }
            1
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            let len = self.inner.bytes.len() as i64;
            if let Some(rsp) = response {
                rsp.set_status(200);
                rsp.set_status_text(Some(&CefString::from("OK")));
                rsp.set_mime_type(Some(&CefString::from(self.inner.mime)));
            }
            if let Some(rl) = response_length { *rl = len; }
        }

        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: ::std::os::raw::c_int,
            bytes_read: Option<&mut ::std::os::raw::c_int>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> ::std::os::raw::c_int {
            let offset = self.inner.offset.load(Ordering::Relaxed);
            let n = read_span(self.inner.bytes.len(), offset, bytes_to_read);
            // Nothing left, or a degenerate request: report completion rather
            // than copying. A non-positive `bytes_to_read` used to widen into
            // a huge `usize` and copy the whole remainder into whatever the
            // caller had sized for it.
            if n == 0 || data_out.is_null() {
                if let Some(br) = bytes_read { *br = 0; }
                return 0;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.inner.bytes.as_ptr().add(offset),
                    data_out,
                    n,
                );
            }
            self.inner.offset.store(offset + n, Ordering::Relaxed);
            // `n <= bytes_to_read`, which is a positive c_int here.
            if let Some(br) = bytes_read { *br = n as i32; }
            1
        }
    }
}

/// How many bytes one `ResourceHandler::read` call may copy: the smaller of
/// what is left and what the caller asked for, and 0 for a spent handler or a
/// non-positive request size.
fn read_span(total: usize, offset: usize, bytes_to_read: std::os::raw::c_int) -> usize {
    if offset >= total || bytes_to_read <= 0 {
        return 0;
    }
    (total - offset).min(bytes_to_read as usize)
}

// ---- registration ----------------------------------------------------------

pub(crate) fn register() {
    let scheme = CefString::from("app");
    let domain = CefString::from("");
    register_scheme_handler_factory(
        Some(&scheme),
        Some(&domain),
        Some(&mut JfnSchemeFactoryBuilder::new(JfnSchemeFactory)),
    );
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    // --- url_to_resource_path ----------------------------------------------

    #[test]
    fn url_to_resource_path_strips_the_scheme_and_the_query() {
        assert_eq!(
            url_to_resource_path("app://resources/about.js"),
            "resources/about.js"
        );
        assert_eq!(
            url_to_resource_path("app://resources/overlay.css?v=2"),
            "resources/overlay.css"
        );
        assert_eq!(
            url_to_resource_path("app://resources/about.html#top"),
            "resources/about.html"
        );
        assert_eq!(url_to_resource_path("app://"), "");
        assert_eq!(url_to_resource_path(""), "");
    }

    #[test]
    fn url_to_resource_path_handles_urls_without_a_scheme() {
        assert_eq!(
            url_to_resource_path("resources/about.js"),
            "resources/about.js"
        );
        assert_eq!(url_to_resource_path("?a"), "");
    }

    #[test]
    fn url_to_resource_path_splits_on_the_first_scheme_separator_only() {
        assert_eq!(
            url_to_resource_path("app://resources/a://b.js"),
            "resources/a://b.js"
        );
    }

    #[test]
    fn url_to_resource_path_never_panics_on_multibyte_input() {
        // The scheme split is a byte index; a multibyte path must not split
        // a character.
        for url in [
            "app://\u{1F600}/x.js",
            "app://resources/\u{2028}.js",
            "\u{1F600}://a",
            "://",
            "a://",
        ] {
            let _ = url_to_resource_path(url);
        }
        assert_eq!(url_to_resource_path("app://\u{1F600}"), "\u{1F600}");
    }

    // --- lookup -------------------------------------------------------------

    #[test]
    fn lookup_resolves_every_embedded_resource() {
        for (name, _) in RESOURCES {
            let path = format!("resources/{name}");
            assert!(lookup(&path).is_some(), "{name} must resolve");
        }
    }

    #[test]
    fn lookup_returns_the_declared_mime_type() {
        assert_eq!(lookup("resources/about.html").unwrap().mime, "text/html");
        assert_eq!(
            lookup("resources/overlay.js").unwrap().mime,
            "application/javascript"
        );
        assert_eq!(
            lookup("resources/logo-mark.svg").unwrap().mime,
            "image/svg+xml"
        );
    }

    #[test]
    fn lookup_rejects_anything_outside_the_table() {
        for path in [
            "",
            "resources/",
            "about.js",
            "resources/About.js",
            "resources/../about.js",
            "resources/../../src/config/settings.json",
            "resources//about.js",
            "resources/about.js/",
            "resources/resources/about.js",
            "/resources/about.js",
            "resources/about.js\0",
            "C:/Windows/win.ini",
            "resources/C:/Windows/win.ini",
            "resources/theme.css",
        ] {
            assert!(lookup(path).is_none(), "{path:?} must not resolve");
        }
    }

    // --- generated payloads -------------------------------------------------

    #[test]
    fn theme_css_declares_the_background_color() {
        assert_eq!(
            String::from_utf8(theme_css()).unwrap(),
            ":root{--bg-color:#101010}"
        );
    }

    #[test]
    fn about_js_payload_prefixes_the_static_body_with_parsable_json() {
        // The payload carries the runtime CEF version, so libcef must be live.
        crate::test_support::ensure_cef_loaded();

        let out = about_js_payload();
        let text = String::from_utf8(out).unwrap();
        let first = text.lines().next().unwrap();
        let json = first
            .strip_prefix("var _aboutData = ")
            .and_then(|s| s.strip_suffix(';'))
            .expect("payload starts with the data prefix");
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        assert!(parsed.get("app").is_some());
        assert!(parsed.get("configDir").is_some());
        assert!(text.len() > first.len(), "static body is appended");
    }

    #[test]
    fn about_data_escapes_a_hostile_config_dir_for_js() {
        // `about.js` is served as an external script, so the JS-string
        // hazards are the quote, the backslash and the two line separators
        // JSON leaves raw.
        let data = AboutData {
            app: "0.0.0",
            cef: &CefVersion::Unknown,
            based_on: UPSTREAM_CREDIT,
            config_dir: "C:\\a\"b\u{2028}c\u{2029}d</script>".to_string(),
            log_file: None,
        };
        let json = jfn_js_json::to_js_json(&data).unwrap();
        assert!(json.contains("\\u2028"), "{json}");
        assert!(json.contains("\\u2029"), "{json}");
        assert!(json.contains("\\\\a\\\"b"), "{json}");
        assert!(!json.contains('\u{2028}'));
        // Round-trips as JSON, which is what the browser will do with it.
        let back: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.get("configDir").and_then(|v| v.as_str()),
            Some("C:\\a\"b\u{2028}c\u{2029}d</script>")
        );
    }

    #[test]
    fn about_data_omits_an_absent_log_file() {
        let data = AboutData {
            app: "0.0.0",
            cef: &CefVersion::Unknown,
            based_on: UPSTREAM_CREDIT,
            config_dir: "/tmp/x".to_string(),
            log_file: None,
        };
        let json = jfn_js_json::to_js_json(&data).unwrap();
        assert!(!json.contains("logFile"), "{json}");
    }

    // --- abs_path -----------------------------------------------------------

    #[test]
    fn abs_path_keeps_an_absolute_path() {
        let p = if cfg!(windows) { "C:\\x\\y" } else { "/x/y" };
        assert_eq!(abs_path(p), p);
    }

    #[test]
    fn abs_path_prepends_the_working_directory_to_a_relative_one() {
        let got = abs_path("rel.log");
        assert!(std::path::Path::new(&got).is_absolute(), "{got}");
        assert!(got.ends_with("rel.log"), "{got}");
    }

    #[test]
    fn abs_path_leaves_an_empty_path_alone() {
        // `Path::new("")` is relative, so this joins onto the cwd rather
        // than panicking.
        let _ = abs_path("");
    }

    // --- read_span ----------------------------------------------------------

    #[test]
    fn read_span_returns_the_smaller_of_remaining_and_requested() {
        assert_eq!(read_span(100, 0, 10), 10);
        assert_eq!(read_span(100, 95, 10), 5);
        assert_eq!(read_span(100, 0, i32::MAX), 100);
        assert_eq!(read_span(100, 99, 1), 1);
    }

    #[test]
    fn read_span_is_zero_once_the_handler_is_spent() {
        assert_eq!(read_span(100, 100, 10), 0);
        assert_eq!(read_span(100, 1_000, 10), 0);
        assert_eq!(read_span(0, 0, 10), 0);
    }

    #[test]
    fn read_span_rejects_a_non_positive_request() {
        // `-1 as usize` is usize::MAX, which used to make `min` pick the
        // whole remaining buffer and copy it into the caller's.
        assert_eq!(read_span(100, 0, 0), 0);
        assert_eq!(read_span(100, 0, -1), 0);
        assert_eq!(read_span(100, 0, i32::MIN), 0);
    }
}
