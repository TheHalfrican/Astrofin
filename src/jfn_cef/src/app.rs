//! `cef::App` implementation. Owns both browser-process and render-process
//! handlers — CEF re-execs the same binary for child processes, so the same
//! App is constructed in every process and CEF dispatches based on the
//! `--type=` switch.

use cef::*;
use parking_lot::Mutex;
use std::collections::HashMap;

use crate::cef_string::userfree_to_string;
use crate::embedded_css;
use crate::embedded_js;
use crate::injection::ExtraInfo;
use crate::paint_scheduler::PaintScheduler;
use crate::state;
use crate::switches::{
    self, APP_SCHEME_OPTIONS, SwitchAction, baseline_disabled_features, is_browser_process,
};
use crate::v8_handler::NativeHandlerBuilder;

// V8 property attribute. Equivalent to V8_PROPERTY_ATTRIBUTE_READONLY.
fn readonly_attr() -> V8Propertyattribute {
    V8Propertyattribute::from(sys::cef_v8_propertyattribute_t::V8_PROPERTY_ATTRIBUTE_READONLY)
}

// A feature-list switch is last-occurrence-wins in Chromium, so it must be
// appended exactly once with every contributor's features merged in.
fn append_merged_features(cl: &mut CommandLine, name: &str, features: Vec<String>) {
    let key = CefString::from(name);
    let existing =
        (cl.has_switch(Some(&key)) == 1).then(|| userfree_to_string(&cl.switch_value(Some(&key))));
    let merged = switches::merge_features(features, existing.as_deref());
    cl.append_switch_with_value(Some(&key), Some(&CefString::from(merged.as_str())));
}

// ----- App ------------------------------------------------------------------

// Shared profile map. CEF may call `App::render_process_handler()` more than
// once per process; each call must hand back a handler that shares the same
// browser-id → injection-profile map. Holding the inner state on JfnApp via
// Arc lets us clone-cheap on every render_process_handler() invocation while
// preserving the map across calls.
type ProfileMap = std::sync::Arc<Mutex<HashMap<i32, ExtraInfo>>>;

#[derive(Clone)]
pub struct JfnApp {
    profiles: ProfileMap,
}

impl JfnApp {
    pub fn new() -> Self {
        Self {
            profiles: std::sync::Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

wrap_app! {
    pub struct JfnAppBuilder {
        inner: JfnApp,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefStringUtf16>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(cl) = command_line else { return };

            // Disable all Google services.
            for sw in [
                "disable-background-networking",
                "disable-client-side-phishing-detection",
                "disable-default-apps",
                "disable-extensions",
                "disable-component-update",
                "disable-sync",
                "disable-translate",
                "disable-domain-reliability",
                "disable-breakpad",
                "disable-notifications",
                "disable-spell-checking",
                "no-pings",
                "bwsi",
            ] {
                cl.append_switch(Some(&CefString::from(sw)));
            }

            for (name, value) in [
                ("google-api-key", ""),
                ("google-default-client-id", ""),
                ("google-default-client-secret", ""),
            ] {
                cl.append_switch_with_value(
                    Some(&CefString::from(name)),
                    Some(&CefString::from(value)),
                );
            }

            // Chromium keeps only the last `disable-features` occurrence, so
            // every contributor merges into one value appended exactly once:
            // whatever is already on the command line (Chromium forwards the
            // browser's set to subprocesses), the baseline below, and any
            // pending switch.
            let mut disable_features: Vec<String> = baseline_disabled_features();

            // Browser-process-only switches from CefRuntime::Set*().
            let process_type = process_type.map(|s| s.to_string());
            if is_browser_process(process_type.as_deref()) {
                for sw in state::snapshot_switches() {
                    match switches::switch_action(&sw) {
                        SwitchAction::MergeFeatures(v) => {
                            disable_features.extend(switches::split_features(v));
                        }
                        SwitchAction::Flag(name) => {
                            cl.append_switch(Some(&CefString::from(name)));
                        }
                        SwitchAction::Valued(name, v) => cl.append_switch_with_value(
                            Some(&CefString::from(name)),
                            Some(&CefString::from(v)),
                        ),
                    }
                }
            }

            append_merged_features(cl, "disable-features", disable_features);
        }

        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            let Some(reg) = registrar else { return };
            let name = CefString::from("app");
            reg.add_custom_scheme(Some(&name), APP_SCHEME_OPTIONS);
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(BphBuilder::new(JfnBph))
        }

        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(RphBuilder::new(JfnRph { profiles: self.inner.profiles.clone() }))
        }
    }
}

// ----- BrowserProcessHandler ------------------------------------------------

#[derive(Clone)]
struct JfnBph;

wrap_browser_process_handler! {
    struct BphBuilder { inner: JfnBph, }

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            jfn_logging::log(jfn_logging::CATEGORY_CEF, jfn_logging::LEVEL_INFO, "CEF context initialized");
            crate::resource::register();
            // Optional C-side callback (kept for any future C++ context-init
            // hooks; currently unused now that scheme registration is in Rust).
            if let Some(cb) = state::with_config(|c| c.on_context_initialized) {
                cb();
            }
        }

        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            if let Some(host) = jfn_platform_abi::try_get().and_then(|p| p.cef_host()) {
                host.pump_schedule(delay_ms);
            }
        }
    }
}

// ----- RenderProcessHandler -------------------------------------------------
//
// Renderer-local map of browser identifier → injection profile passed through
// extra_info at CreateBrowser time. Populated in on_browser_created, consumed
// in on_context_created, erased in on_browser_destroyed.

#[derive(Default, Clone)]
struct JfnRph {
    profiles: ProfileMap,
}

wrap_render_process_handler! {
    struct RphBuilder { inner: JfnRph, }

    impl RenderProcessHandler {
        fn on_browser_created(
            &self,
            browser: Option<&mut Browser>,
            extra_info: Option<&mut DictionaryValue>,
        ) {
            let (Some(browser), Some(extra)) = (browser, extra_info) else { return };
            let id = browser.identifier();
            self.inner
                .profiles
                .lock()
                .insert(id, ExtraInfo::from_dictionary(extra.clone()));
        }

        fn on_browser_destroyed(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            let id = browser.identifier();
            self.inner.profiles.lock().remove(&id);
        }

        fn on_context_created(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            let (Some(browser), Some(frame), Some(ctx)) = (browser, frame, context) else {
                return;
            };
            // Top-frame only. jmpNative + player shim must not pollute iframes.
            if frame.is_main() != 1 {
                return;
            }

            let profile = {
                let id = browser.identifier();
                self.inner.profiles.lock().get(&id).cloned()
            };
            let Some(profile) = profile else { return };

            inject_jmp_native(browser, &profile, ctx);
            PaintScheduler::on_context_created(profile.shared_textures_enabled(), frame);
            run_user_scripts(&profile, frame);
        }

        fn on_process_message_received(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> ::std::os::raw::c_int {
            let (Some(frame), Some(msg)) = (frame, message) else { return 0 };
            let name = userfree_to_string(&msg.name());
            let args = msg.argument_list();

            match name.as_str() {
                "savedServerUrl" => {
                    let Some(args) = args else { return 1 };
                    let url = userfree_to_string(&args.string(0));
                    call_js_global_string(frame, "_onSavedServerUrl", &[Arg::Str(&url)]);
                    1
                }
                "serverConnectivityResult" => {
                    let Some(args) = args else { return 1 };
                    let url = userfree_to_string(&args.string(0));
                    let ok = args.bool(1) != 0;
                    let detail = userfree_to_string(&args.string(2));
                    call_js_global_string(
                        frame,
                        "_onServerConnectivityResult",
                        &[Arg::Str(&url), Arg::Bool(ok), Arg::Str(&detail)],
                    );
                    1
                }
                "getPopupOptions" => {
                    let popup = collect_popup_options(frame);
                    let Some(mut reply) = process_message_create(Some(&CefString::from("popupOptions")))
                    else { return 1 };
                    if let Some(reply_args) = reply.argument_list() {
                        if let Some(mut list) = list_value_create() {
                            for (i, opt) in popup.options.iter().enumerate() {
                                list.set_string(i, Some(&CefString::from(opt.as_str())));
                            }
                            reply_args.set_list(0, Some(&mut list));
                        }
                        reply_args.set_int(1, popup.selected);
                        if let Some(mut sel_list) = list_value_create() {
                            for (i, idx) in popup.selectable.iter().enumerate() {
                                sel_list.set_int(i, *idx);
                            }
                            reply_args.set_list(2, Some(&mut sel_list));
                        }
                        if let Some((ax, ay)) = popup.anchor {
                            reply_args.set_int(3, ax);
                            reply_args.set_int(4, ay);
                            reply_args.set_int(5, 1);
                        } else {
                            reply_args.set_int(5, 0);
                        }
                    }
                    frame.send_process_message(
                        ProcessId::from(sys::cef_process_id_t::PID_BROWSER),
                        Some(&mut reply),
                    );
                    1
                }
                _ => 0,
            }
        }
    }
}

enum Arg<'a> {
    Str(&'a str),
    Bool(bool),
}

fn call_js_global_string(frame: &Frame, fn_name: &str, args: &[Arg<'_>]) {
    let Some(ctx) = frame.v8_context() else {
        return;
    };
    if ctx.enter() != 1 {
        return;
    }
    let _drop = ContextExit(&ctx);
    let Some(global) = ctx.global() else { return };
    let fn_key = CefString::from(fn_name);
    let Some(fn_val) = global.value_bykey(Some(&fn_key)) else {
        return;
    };
    if fn_val.is_function() != 1 {
        return;
    }
    let v8_args: Vec<Option<V8Value>> = args
        .iter()
        .map(|a| match a {
            Arg::Str(s) => v8_value_create_string(Some(&CefString::from(*s))),
            Arg::Bool(b) => v8_value_create_bool(if *b { 1 } else { 0 }),
        })
        .collect();
    fn_val.execute_function(None, Some(&v8_args));
}

struct ContextExit<'a>(&'a V8Context);

impl Drop for ContextExit<'_> {
    fn drop(&mut self) {
        self.0.exit();
    }
}

struct PopupOptions {
    options: Vec<String>,
    selected: i32,
    selectable: Vec<i32>,
    anchor: Option<(i32, i32)>,
}

fn collect_popup_options(frame: &Frame) -> PopupOptions {
    let mut options = Vec::new();
    let mut selected = -1;
    let mut selectable = Vec::new();
    let mut anchor = None;
    let Some(ctx) = frame.v8_context() else {
        return PopupOptions {
            options,
            selected,
            selectable,
            anchor,
        };
    };
    if ctx.enter() != 1 {
        return PopupOptions {
            options,
            selected,
            selectable,
            anchor,
        };
    }
    let _drop = ContextExit(&ctx);
    let Some(global) = ctx.global() else {
        return PopupOptions {
            options,
            selected,
            selectable,
            anchor,
        };
    };
    let doc_key = CefString::from("document");
    let Some(doc) = global.value_bykey(Some(&doc_key)) else {
        return PopupOptions {
            options,
            selected,
            selectable,
            anchor,
        };
    };
    let active_key = CefString::from("activeElement");
    let Some(mut el) = doc.value_bykey(Some(&active_key)) else {
        return PopupOptions {
            options,
            selected,
            selectable,
            anchor,
        };
    };
    if el.is_object() != 1 {
        return PopupOptions {
            options,
            selected,
            selectable,
            anchor,
        };
    }
    let tag_key = CefString::from("tagName");
    let Some(tag) = el.value_bykey(Some(&tag_key)) else {
        return PopupOptions {
            options,
            selected,
            selectable,
            anchor,
        };
    };
    if tag.is_string() != 1 {
        return PopupOptions {
            options,
            selected,
            selectable,
            anchor,
        };
    }
    if userfree_to_string(&tag.string_value()) != "SELECT" {
        return PopupOptions {
            options,
            selected,
            selectable,
            anchor,
        };
    }
    let rect_key = CefString::from("getBoundingClientRect");
    if let Some(get_rect) = el.value_bykey(Some(&rect_key))
        && get_rect.is_function() == 1
        && let Some(rect) = get_rect.execute_function(Some(&mut el), Some(&[]))
        && rect.is_object() == 1
    {
        let read = |name: &str| -> Option<f64> {
            let key = CefString::from(name);
            rect.value_bykey(Some(&key))
                .filter(|v| v.is_double() == 1 || v.is_int() == 1)
                .map(|v| v.double_value())
        };
        if let (Some(left), Some(bottom)) = (read("left"), read("bottom")) {
            anchor = Some((left.round() as i32, bottom.round() as i32));
        }
    }
    let opts_key = CefString::from("options");
    let Some(opts) = el.value_bykey(Some(&opts_key)) else {
        return PopupOptions {
            options,
            selected,
            selectable,
            anchor,
        };
    };
    let len_key = CefString::from("length");
    let Some(len_val) = opts.value_bykey(Some(&len_key)) else {
        return PopupOptions {
            options,
            selected,
            selectable,
            anchor,
        };
    };
    if opts.is_object() != 1 || len_val.is_int() != 1 {
        return PopupOptions {
            options,
            selected,
            selectable,
            anchor,
        };
    }
    let len = len_val.int_value();
    let text_key = CefString::from("text");
    let disabled_key = CefString::from("disabled");
    let parent_key = CefString::from("parentNode");
    let tagname_key = CefString::from("tagName");
    let read_disabled = |o: &V8Value| -> bool {
        o.value_bykey(Some(&disabled_key))
            .is_some_and(|d| d.is_bool() == 1 && d.bool_value() != 0)
    };
    for i in 0..len {
        let Some(opt) = opts.value_byindex(i) else {
            options.push(String::new());
            continue;
        };
        let mut s = String::new();
        let mut disabled = false;
        if opt.is_object() == 1 {
            if let Some(t) = opt.value_bykey(Some(&text_key))
                && t.is_string() == 1
            {
                s = userfree_to_string(&t.string_value());
            }
            disabled = read_disabled(&opt);
            if !disabled
                && let Some(parent) = opt.value_bykey(Some(&parent_key))
                && parent.is_object() == 1
                && let Some(ptag) = parent.value_bykey(Some(&tagname_key))
                && ptag.is_string() == 1
                && userfree_to_string(&ptag.string_value()) == "OPTGROUP"
            {
                disabled = read_disabled(&parent);
            }
        }
        if !disabled {
            selectable.push(i);
        }
        options.push(s);
    }
    let sel_key = CefString::from("selectedIndex");
    if let Some(sel) = el.value_bykey(Some(&sel_key))
        && sel.is_int() == 1
    {
        selected = sel.int_value();
    }
    PopupOptions {
        options,
        selected,
        selectable,
        anchor,
    }
}

fn inject_jmp_native(browser: &mut Browser, profile: &ExtraInfo, context: &mut V8Context) {
    let Some(global) = context.global() else {
        return;
    };
    let Some(mut jmp_native) = v8_value_create_object(None, None) else {
        return;
    };
    let browser_id = browser.identifier();
    for function in profile.functions() {
        let cef_name = CefString::from(function.name());
        let _ = browser_id;
        let mut handler = NativeHandlerBuilder::new(crate::v8_handler::NativeHandler);
        let Some(mut fn_val) = v8_value_create_function(Some(&cef_name), Some(&mut handler)) else {
            continue;
        };
        jmp_native.set_value_bykey(Some(&cef_name), Some(&mut fn_val), readonly_attr());
    }
    let key = CefString::from("jmpNative");
    global.set_value_bykey(Some(&key), Some(&mut jmp_native), readonly_attr());
}

/// Builds the JS preamble that installs the profile's stylesheets as a single
/// `<style id="af-theme">`. Runs at `OnContextCreated`, i.e. before the page
/// has been parsed and long before its own stylesheets load, so the element is
/// parked in `<head>` (or, if there is no root yet, on the first mutation that
/// creates one).
///
/// From DOMContentLoaded on, `astrofin-theme.js` owns its position and moves it
/// to the end of `<body>`: jellyfin-web appends a chunk stylesheet per lazily
/// loaded route *and* serves `themes/<name>/theme.css` from a `<div>` inside
/// `<body>`, which no position in `<head>` can outrank at equal specificity.
///
/// `None` when the profile declares no styles.
fn styles_preamble(profile: &ExtraInfo) -> Option<String> {
    let styles = profile.styles();
    if styles.is_empty() {
        return None;
    }

    let mut css = String::new();
    for style in styles {
        if let Some(src) = embedded_css::get(style.file_name()) {
            if !css.is_empty() {
                css.push('\n');
            }
            css.push_str(src);
        }
    }
    if css.is_empty() {
        return None;
    }

    // The sheet is embedded as a JS string literal, so it must be JSON-escaped
    // (including U+2028/U+2029, which `to_js_json` handles and plain JSON does
    // not). Bail rather than emit a broken literal.
    let css_json = jfn_js_json::to_js_json(css.as_str())?;

    Some(format!(
        r#"(function () {{
    'use strict';
    // This preamble shares one execute_java_script call with every shim that
    // follows it, so it must never throw: an uncaught exception here would
    // abort native-shim.js and the mpv players with it.
    try {{
        var css = {css_json};
        var ID = 'af-theme';
        function install() {{
            // OnContextCreated fires before parsing starts, so there may be
            // no <html> yet. Report failure and let the caller retry.
            var host = document.head || document.documentElement;
            if (!host) {{
                return null;
            }}
            var el = document.getElementById(ID);
            if (!el) {{
                el = document.createElement('style');
                el.id = ID;
                el.setAttribute('data-astrofin', 'theme');
            }}
            if (el.textContent !== css) {{
                el.textContent = css;
            }}
            // Park it once. astrofin-theme.js owns its position from then on:
            // jf-web loads its own theme sheet from a <div> inside <body>, so
            // no position in <head> wins the cascade after the app has booted.
            if (!el.parentNode || el.parentNode === document.documentElement) {{
                host.appendChild(el);
            }}
            return el;
        }}
        window.__afInstallTheme = install;
        if (!install() && typeof MutationObserver === 'function') {{
            // Wait for the root element to appear, then install once.
            var mo = new MutationObserver(function () {{
                if (install()) {{
                    mo.disconnect();
                }}
            }});
            mo.observe(document, {{ childList: true, subtree: true }});
        }}
        if (document.readyState === 'loading') {{
            document.addEventListener('DOMContentLoaded', function () {{
                install();
            }}, {{ once: true }});
        }}
    }} catch (e) {{
        if (window.console && console.warn) {{
            console.warn('[Astrofin theme] stylesheet install failed', e);
        }}
    }}
}})();
"#
    ))
}

fn run_user_scripts(profile: &ExtraInfo, frame: &Frame) {
    let scripts = profile.scripts();
    let preamble = styles_preamble(profile);
    if scripts.is_empty() && preamble.is_none() {
        return;
    }

    // Renderer is a separate process; load settings here for placeholder
    // substitution.
    ensure_renderer_settings_loaded();

    let mut code = String::new();
    for (i, script) in scripts.iter().enumerate() {
        if i > 0 {
            code.push('\n');
        }
        if let Some(src) = embedded_js::get(script.file_name()) {
            code.push_str(src);
        }
    }

    fn replace_first(code: &mut String, ph: &str, value: &str) {
        if let Some(pos) = code.find(ph) {
            code.replace_range(pos..pos + ph.len(), value);
        }
    }
    replace_first(
        &mut code,
        "__SERVER_URL__",
        &js_string_literal(&jfn_config::server_url()),
    );
    replace_first(
        &mut code,
        "__SETTINGS_JSON__",
        &js_string_literal(&jfn_config::cli_json(jfn_mpv::hwdec_options())),
    );
    replace_first(&mut code, "__APP_VERSION__", crate::APP_VERSION);
    let decoration_options = profile
        .window_decoration_options()
        .iter()
        .map(|wd| format!("'{}'", wd.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    replace_first(
        &mut code,
        "__WINDOW_DECORATION_OPTIONS__",
        &format!("[{decoration_options}]"),
    );

    if let Some(dp) = profile.device_profile_json() {
        replace_first(&mut code, "__DEVICE_PROFILE_JSON__", dp);
    }

    let window_decorations = profile
        .window_decorations()
        .map_or_else(|| "null".to_string(), |wd| format!("'{wd}'"));
    replace_first(&mut code, "__WINDOW_DECORATIONS__", &window_decorations);

    // Prepended only after placeholder substitution, so stylesheet bytes can
    // never be mistaken for a `__PLACEHOLDER__` and consume a replacement.
    if let Some(preamble) = preamble {
        code.insert_str(0, &preamble);
    }

    let url_uf = frame.url();
    let url = CefString::from(&url_uf);
    let code_cef = CefString::from(code.as_str());
    frame.execute_java_script(Some(&code_cef), Some(&url), 0);
}

fn ensure_renderer_settings_loaded() {
    use std::sync::OnceLock;
    static INITED: OnceLock<()> = OnceLock::new();
    INITED.get_or_init(|| {
        let path = jfn_paths::config_dir().join("settings.json");
        jfn_config::settings_init(&path);
        let _ = jfn_config::settings_load();
    });
}

/// The saved server URL as a JS string literal, ready to replace the bare
/// `__SERVER_URL__` placeholder in `native-shim.js`. The value comes from
/// `settings.json` (hand-editable) and from `saveServerUrl`, so it must go
/// through the JS-source escaper: a quote or a U+2028 inside a `'...'`
/// literal would otherwise break out of the string and run in the renderer
/// that has the native bridge.
fn js_string_literal(url: &str) -> String {
    jfn_js_json::to_js_json(url).unwrap_or_else(|| "\"\"".to_string())
}

#[cfg(test)]
mod tests {
    use super::js_string_literal;

    #[test]
    fn js_string_literal_is_a_quoted_json_string() {
        assert_eq!(
            js_string_literal("https://jf.example.com/"),
            "\"https://jf.example.com/\""
        );
    }

    #[test]
    fn js_string_literal_cannot_break_out_of_the_literal() {
        let hostile = "http://x/';alert(1);//\u{2028}\"</script>";
        let lit = js_string_literal(hostile);
        assert!(lit.starts_with('"') && lit.ends_with('"'));
        let body = &lit[1..lit.len() - 1];
        assert!(!body.contains('\u{2028}'));
        assert!(!body.contains(['\n', '\r']));
        assert!(body.contains("\\\""), "inner quote must be escaped: {body}");
    }

    #[test]
    fn js_string_literal_empty_is_an_empty_string_literal() {
        assert_eq!(js_string_literal(""), "\"\"");
    }
}
