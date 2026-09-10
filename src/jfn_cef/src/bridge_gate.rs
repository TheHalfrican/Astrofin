//! Which documents get `window.jmpNative`.
//!
//! The bridge used to be bound per *browser*: whatever the main layer had
//! navigated to got the whole player IPC surface, because the profile was
//! chosen at `CreateBrowser` time and never consulted again. Since 2026-09-10
//! it is bound per *origin*: a top frame is handed the bridge only when its
//! origin is the saved server's, or when it is one of the app's own internal
//! pages (`app://resources/...` — the connect overlay, About, and the
//! never-navigated `about:blank` placeholder the main layer is created at).
//!
//! Everything here is pure except [`refused_origin_is_new`], which owns the
//! once-per-origin warn dedup.

use parking_lot::Mutex;
use std::collections::HashSet;

/// The custom scheme `resource.rs` serves the app's own pages from. Registered
/// in `app::on_register_custom_schemes`.
pub(crate) const APP_SCHEME: &str = "app://";

/// The blank document CEF creates a layer at before anything navigates it
/// (`client::browser_ops` creates the main layer here on purpose).
const BLANK_URLS: [&str; 2] = ["about:blank", "about:blank#blocked"];

/// True for one of the app's own documents: an `app://` page or the blank
/// placeholder. These are shipped inside the binary, so their contents are
/// exactly as trusted as the Rust that serves them.
pub(crate) fn is_internal_layer_url(url: &str) -> bool {
    if url.is_empty() {
        // OnContextCreated for a frame with no URL yet: the layer's own
        // bootstrap context, never a server document.
        return true;
    }
    // `get`, not a slice: a URL whose seventh byte falls inside a multi-byte
    // character would panic on `url[..6]`.
    if url
        .get(..APP_SCHEME.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(APP_SCHEME))
    {
        return true;
    }
    BLANK_URLS.iter().any(|b| url.eq_ignore_ascii_case(b))
}

/// Whether a top frame at `frame_url` may be given `window.jmpNative`, with
/// `saved_server_url` as the currently saved server.
///
/// An internal page always may. Everything else must match the saved server's
/// origin exactly — scheme, host (case-insensitively) and port, with the
/// scheme's default port filled in. An empty saved server (nothing connected
/// yet) means no http(s) document qualifies.
pub(crate) fn bridge_allowed(frame_url: &str, saved_server_url: &str) -> bool {
    if is_internal_layer_url(frame_url) {
        return true;
    }
    jfn_jellyfin::same_origin(frame_url, saved_server_url)
}

/// A short, log-safe label for the origin a refusal happened at: the origin
/// when the URL parses as http(s), otherwise the escaped URL itself.
pub(crate) fn refusal_label(frame_url: &str) -> String {
    jfn_jellyfin::parse_origin(frame_url).map_or_else(
        || jfn_logging::escape_page_string(frame_url),
        |o| o.to_url(),
    )
}

/// True the first time `origin` is refused in this process, so a page that
/// reloads itself in a loop cannot flood the log.
pub(crate) fn refused_origin_is_new(origin: &str) -> bool {
    static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let mut guard = WARNED.lock();
    let set = guard.get_or_insert_with(HashSet::new);
    // A hostile page can navigate to unlimited distinct origins; the set is
    // capped so it cannot be turned into a memory leak.
    if set.len() >= 256 {
        return false;
    }
    set.insert(origin.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    // The two shapes the owner requires to keep working verbatim.
    const IP_PORT: &str = "http://192.168.1.10:8096";
    const TAILNET: &str = "http://thehalfrican-truenas.tail1cdca8.ts.net:8096";

    #[test]
    fn the_apps_own_pages_always_get_the_bridge() {
        for internal in [
            "app://resources/overlay.html",
            "APP://resources/about.html",
            "app://resources/about.js",
            "about:blank",
            "",
        ] {
            assert!(is_internal_layer_url(internal), "{internal:?}");
            assert!(bridge_allowed(internal, ""), "{internal:?}");
        }
    }

    #[test]
    fn a_document_on_the_saved_servers_origin_gets_the_bridge() {
        // An IP:port server, which is the common LAN deployment.
        assert!(bridge_allowed(
            "http://192.168.1.10:8096/web/index.html#!/home.html",
            IP_PORT
        ));
        // A tailnet MagicDNS name on the same port, case-insensitively.
        assert!(bridge_allowed(
            "http://THEHALFRICAN-TRUENAS.tail1cdca8.ts.net:8096/web/",
            TAILNET
        ));
        // And the saved URL keeping a `/web` path does not narrow the origin.
        assert!(bridge_allowed(
            "http://192.168.1.10:8096/Videos/1/stream",
            "http://192.168.1.10:8096/web/index.html"
        ));
    }

    #[test]
    fn a_document_on_any_other_origin_gets_no_bridge() {
        for hostile in [
            // A different host entirely — what a redirect or an ad iframe
            // navigating the top frame would land on.
            "http://evil.example.com/",
            // The right host on the wrong port or scheme.
            "http://192.168.1.10:8920/",
            "https://192.168.1.10:8096/",
            // A subdomain of the saved host is a different origin.
            "http://a.192.168.1.10:8096/",
            // Non-http schemes never qualify (they are not internal either).
            "file:///etc/passwd",
            "data:text/html,<script>1</script>",
            "javascript:alert(1)",
        ] {
            assert!(!bridge_allowed(hostile, IP_PORT), "{hostile:?}");
        }
        assert!(!bridge_allowed("http://evil.example.com/", TAILNET));
    }

    #[test]
    fn no_saved_server_means_no_http_document_gets_the_bridge() {
        assert!(!bridge_allowed(IP_PORT, ""));
        assert!(!bridge_allowed("http://any.host/", ""));
        // The overlay still runs, which is how a server gets saved at all.
        assert!(bridge_allowed("app://resources/overlay.html", ""));
    }

    #[test]
    fn changing_the_saved_server_moves_the_bridge_to_the_new_origin() {
        // `saveServerUrl` at runtime: the old origin loses the bridge and the
        // new one gains it, with no browser re-creation in between.
        assert!(bridge_allowed("http://192.168.1.10:8096/web/", IP_PORT));
        assert!(!bridge_allowed("http://192.168.1.10:8096/web/", TAILNET));
        assert!(bridge_allowed(
            "http://thehalfrican-truenas.tail1cdca8.ts.net:8096/web/",
            TAILNET
        ));
    }

    #[test]
    fn refusal_label_is_the_origin_for_a_url_and_escaped_otherwise() {
        assert_eq!(
            refusal_label("http://evil.example.com:8096/x?y#z"),
            "http://evil.example.com:8096"
        );
        assert_eq!(refusal_label("http://host/"), "http://host");
        let label = refusal_label("data:text/html,<b>\n\u{1b}[2J");
        assert!(
            !label.contains('\n') && !label.contains('\u{1b}'),
            "{label}"
        );
    }

    #[test]
    fn is_internal_layer_url_never_panics_on_a_multibyte_prefix() {
        // The `app://` test must not slice a URL whose sixth byte is inside a
        // character. Nothing here may be internal either.
        for odd in ["日本語://x", "ap\u{2028}p://x", "à", "app:/", "app:"] {
            assert!(!is_internal_layer_url(odd), "{odd:?}");
        }
    }

    #[test]
    fn an_origin_is_only_warned_about_once() {
        let origin = format!("http://once.{}.example", std::process::id());
        assert!(refused_origin_is_new(&origin));
        assert!(!refused_origin_is_new(&origin));
        // A different origin is still reported.
        assert!(refused_origin_is_new(&format!("{origin}:8096")));
    }
}
