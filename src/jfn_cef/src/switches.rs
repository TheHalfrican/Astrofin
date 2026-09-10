//! Chromium command-line construction, as rules over strings.
//!
//! `ffi.rs` decides *when* these run and pushes the result at CEF; `app.rs`
//! writes the merged value onto a live `CefCommandLine`. What each switch
//! should be — which switches a display backend needs, how the
//! `disable-features` contributors merge, which log severity an integer names
//! — is here, where none of it needs a CEF runtime.

use std::os::raw::c_int;

use jfn_platform_abi::DisplayBackend;

use crate::state::PendingSwitch;

/// `app://` scheme options. Match `CEF_SCHEME_OPTION_*` from
/// `include/internal/cef_types.h` (verified against CEF 151.3.24):
/// STANDARD 1<<0, LOCAL 1<<1, DISPLAY_ISOLATED 1<<2, SECURE 1<<3,
/// CORS_ENABLED 1<<4, CSP_BYPASSING 1<<5, FETCH_ENABLED 1<<6.
pub(crate) const SCHEME_OPTION_STANDARD: i32 = 1 << 0;
pub(crate) const SCHEME_OPTION_LOCAL: i32 = 1 << 1;
pub(crate) const SCHEME_OPTION_SECURE: i32 = 1 << 3;
pub(crate) const SCHEME_OPTION_CORS_ENABLED: i32 = 1 << 4;
pub(crate) const SCHEME_OPTION_FETCH_ENABLED: i32 = 1 << 6;

/// The option mask the `app://` scheme is registered with: a standard,
/// secure, local scheme that fetch and CORS both accept, so the injected
/// bundles load like same-origin resources without bypassing CSP.
pub(crate) const APP_SCHEME_OPTIONS: i32 = SCHEME_OPTION_STANDARD
    | SCHEME_OPTION_SECURE
    | SCHEME_OPTION_LOCAL
    | SCHEME_OPTION_CORS_ENABLED
    | SCHEME_OPTION_FETCH_ENABLED;

/// Baseline Chromium features disabled in every process (Google services,
/// telemetry, spell check). Merged with every other `disable-features`
/// contributor by [`merge_features`].
pub(crate) const DISABLED_FEATURES: &[&str] = &[
    "PushMessaging",
    "BackgroundSync",
    "SafeBrowsing",
    "Translate",
    "OptimizationHints",
    "MediaRouter",
    "DialMediaRouteProvider",
    "AcceptCHFrame",
    "AutofillServerCommunication",
    "CertificateTransparencyComponentUpdater",
    "SyncNotificationServiceWhenSignedIn",
    "SpellCheck",
    "SpellCheckService",
    "PasswordManager",
    "ImmersiveReadAnything",
];

/// The features named by one comma-separated switch value, trimmed, with
/// empties dropped.
pub(crate) fn split_features(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(',')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_string)
}

/// The single value a feature-list switch must be appended with.
///
/// Chromium keeps only the last occurrence of a feature-list switch, so every
/// contributor merges into one value: `features` in order, then whatever was
/// already on the command line, first occurrence winning over later
/// duplicates.
pub(crate) fn merge_features(mut features: Vec<String>, existing: Option<&str>) -> String {
    if let Some(existing) = existing {
        features.extend(split_features(existing));
    }
    let mut seen = std::collections::HashSet::new();
    features.retain(|f| seen.insert(f.clone()));
    features.join(",")
}

/// The baseline `disable-features` set as owned strings.
pub(crate) fn baseline_disabled_features() -> Vec<String> {
    DISABLED_FEATURES.iter().map(|f| (*f).to_string()).collect()
}

/// True when this process is the browser process. CEF passes an empty
/// `--type=` there and a non-empty one ("renderer", "gpu-process", ...) in
/// every child, and passes nothing at all on some paths.
pub(crate) fn is_browser_process(process_type: Option<&str>) -> bool {
    process_type.is_none_or(str::is_empty)
}

/// What one pending switch does to the command line.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum SwitchAction<'a> {
    /// A `disable-features` contributor: merged rather than appended, or the
    /// last occurrence would drop every other contributor's features.
    MergeFeatures(&'a str),
    /// A bare flag.
    Flag(&'a str),
    /// A `--name=value` switch.
    Valued(&'a str, &'a str),
}

/// How one pending switch reaches the command line.
pub(crate) fn switch_action(sw: &PendingSwitch) -> SwitchAction<'_> {
    match (sw.name.as_str(), sw.value.as_deref()) {
        ("disable-features", Some(v)) => SwitchAction::MergeFeatures(v),
        (name, None) => SwitchAction::Flag(name),
        (name, Some(v)) => SwitchAction::Valued(name, v),
    }
}

/// The Chromium switches a display backend needs.
///
/// The one mapping between Astrofin's CLI namespace and Chromium's: the
/// switch names are written here in code, never passed through from a user
/// flag.
pub(crate) fn platform_switches(backend: DisplayBackend) -> Vec<PendingSwitch> {
    match backend {
        DisplayBackend::Wayland => vec![
            PendingSwitch::with_value("ozone-platform", "wayland"),
            // OSR honors GetScreenInfo device_scale_factor only without the
            // fractional-scale protocol.
            PendingSwitch::with_value("disable-features", "WaylandFractionalScaleV1"),
        ],
        DisplayBackend::X11 => vec![PendingSwitch::with_value("ozone-platform", "x11")],
        DisplayBackend::MacOS => vec![
            PendingSwitch::flag("single-process"),
            PendingSwitch::flag("use-mock-keychain"),
            PendingSwitch::with_value("password-store", "basic"),
        ],
        DisplayBackend::Windows => Vec::new(),
    }
}

/// The switch that turns GPU compositing off, when it is being turned off.
pub(crate) fn gpu_compositing_switch(disable: bool) -> Option<PendingSwitch> {
    disable.then(|| PendingSwitch::flag("disable-gpu-compositing"))
}

/// Which message loop CEF runs. A platform that hands us a `CefHost` pumps
/// CEF from its own run loop; everything else lets CEF own a thread.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum MessageLoop {
    /// `external_message_pump = 1`.
    ExternalPump,
    /// `multi_threaded_message_loop = 1`.
    MultiThreaded,
}

/// See [`MessageLoop`].
pub(crate) fn message_loop(has_cef_host: bool) -> MessageLoop {
    if has_cef_host {
        MessageLoop::ExternalPump
    } else {
        MessageLoop::MultiThreaded
    }
}

/// The `cef_log_severity_t` an integer from `jfn_cef_set_log_severity` or a
/// CEF console callback names.
///
/// These are ABI values, not Chromium's older signed log levels (-1..2). Keep
/// this table in sync with `client/events.rs` and the constants in
/// `jfn_rust::app`: 0 DEFAULT, 1 VERBOSE, 2 INFO, 3 WARNING, 4 ERROR,
/// 5 FATAL. Anything else is DEFAULT.
pub(crate) fn log_severity_raw(v: c_int) -> cef::sys::cef_log_severity_t {
    use cef::sys::cef_log_severity_t as S;
    match v {
        1 => S::LOGSEVERITY_VERBOSE,
        2 => S::LOGSEVERITY_INFO,
        3 => S::LOGSEVERITY_WARNING,
        4 => S::LOGSEVERITY_ERROR,
        5 => S::LOGSEVERITY_FATAL,
        _ => S::LOGSEVERITY_DEFAULT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(switches: &[PendingSwitch]) -> Vec<(&str, Option<&str>)> {
        switches
            .iter()
            .map(|s| (s.name.as_str(), s.value.as_deref()))
            .collect()
    }

    #[test]
    fn features_split_on_commas_and_lose_their_padding() {
        let got: Vec<String> = split_features(" A , B,C ").collect();
        assert_eq!(got, vec!["A", "B", "C"]);
    }

    #[test]
    fn empty_entries_are_dropped_from_a_feature_list() {
        assert_eq!(split_features("").count(), 0);
        assert_eq!(split_features(",,").count(), 0);
        let got: Vec<String> = split_features("A,,B").collect();
        assert_eq!(got, vec!["A", "B"]);
    }

    #[test]
    fn merging_appends_what_was_already_on_the_command_line() {
        let merged = merge_features(vec!["A".into(), "B".into()], Some("C,D"));
        assert_eq!(merged, "A,B,C,D");
    }

    #[test]
    fn merging_keeps_the_first_occurrence_of_a_duplicate() {
        let merged = merge_features(vec!["A".into(), "B".into(), "A".into()], Some("B,E"));
        assert_eq!(merged, "A,B,E");
    }

    #[test]
    fn merging_with_nothing_on_the_line_is_the_contributors_own_list() {
        assert_eq!(merge_features(vec!["A".into()], None), "A");
        assert_eq!(merge_features(Vec::new(), None), "");
    }

    #[test]
    fn the_baseline_disables_the_google_service_features() {
        let baseline = baseline_disabled_features();
        assert!(baseline.contains(&"SafeBrowsing".to_string()));
        assert!(baseline.contains(&"MediaRouter".to_string()));
        assert_eq!(baseline.len(), DISABLED_FEATURES.len());
    }

    #[test]
    fn an_empty_or_absent_process_type_is_the_browser_process() {
        assert!(is_browser_process(None));
        assert!(is_browser_process(Some("")));
    }

    #[test]
    fn a_named_process_type_is_a_child_process() {
        assert!(!is_browser_process(Some("renderer")));
        assert!(!is_browser_process(Some("gpu-process")));
    }

    #[test]
    fn a_disable_features_switch_merges_rather_than_appends() {
        let sw = PendingSwitch::with_value("disable-features", "X,Y");
        assert_eq!(switch_action(&sw), SwitchAction::MergeFeatures("X,Y"));
    }

    #[test]
    fn a_valueless_switch_is_appended_as_a_flag() {
        let sw = PendingSwitch::flag("single-process");
        assert_eq!(switch_action(&sw), SwitchAction::Flag("single-process"));
    }

    #[test]
    fn every_other_valued_switch_is_appended_verbatim() {
        let sw = PendingSwitch::with_value("ozone-platform", "wayland");
        assert_eq!(
            switch_action(&sw),
            SwitchAction::Valued("ozone-platform", "wayland")
        );
    }

    #[test]
    fn wayland_picks_its_ozone_platform_and_drops_fractional_scale() {
        assert_eq!(
            names(&platform_switches(DisplayBackend::Wayland)),
            vec![
                ("ozone-platform", Some("wayland")),
                ("disable-features", Some("WaylandFractionalScaleV1")),
            ]
        );
    }

    #[test]
    fn x11_only_picks_its_ozone_platform() {
        assert_eq!(
            names(&platform_switches(DisplayBackend::X11)),
            vec![("ozone-platform", Some("x11"))]
        );
    }

    #[test]
    fn macos_runs_single_process_with_a_mock_keychain() {
        assert_eq!(
            names(&platform_switches(DisplayBackend::MacOS)),
            vec![
                ("single-process", None),
                ("use-mock-keychain", None),
                ("password-store", Some("basic")),
            ]
        );
    }

    #[test]
    fn windows_needs_no_platform_switches() {
        assert!(platform_switches(DisplayBackend::Windows).is_empty());
    }

    #[test]
    fn gpu_compositing_is_only_switched_when_it_is_being_disabled() {
        let sw = gpu_compositing_switch(true);
        assert_eq!(
            sw.as_ref().map(|s| (s.name.as_str(), s.value.as_deref())),
            Some(("disable-gpu-compositing", None))
        );
        assert!(gpu_compositing_switch(false).is_none());
    }

    #[test]
    fn a_platform_with_a_cef_host_pumps_cef_itself() {
        assert_eq!(message_loop(true), MessageLoop::ExternalPump);
        assert_eq!(message_loop(false), MessageLoop::MultiThreaded);
    }

    #[test]
    fn the_log_severity_table_follows_the_cef_abi_values() {
        use cef::sys::cef_log_severity_t as S;
        assert_eq!(log_severity_raw(1), S::LOGSEVERITY_VERBOSE);
        assert_eq!(log_severity_raw(2), S::LOGSEVERITY_INFO);
        assert_eq!(log_severity_raw(3), S::LOGSEVERITY_WARNING);
        assert_eq!(log_severity_raw(4), S::LOGSEVERITY_ERROR);
        assert_eq!(log_severity_raw(5), S::LOGSEVERITY_FATAL);
    }

    #[test]
    fn an_unknown_log_severity_is_the_default_one() {
        use cef::sys::cef_log_severity_t as S;
        assert_eq!(log_severity_raw(0), S::LOGSEVERITY_DEFAULT);
        assert_eq!(log_severity_raw(-1), S::LOGSEVERITY_DEFAULT);
        assert_eq!(log_severity_raw(99), S::LOGSEVERITY_DEFAULT);
    }

    #[test]
    fn the_app_scheme_is_standard_secure_local_and_fetchable() {
        assert_eq!(APP_SCHEME_OPTIONS & SCHEME_OPTION_STANDARD, 1);
        assert_eq!(APP_SCHEME_OPTIONS & SCHEME_OPTION_SECURE, 1 << 3);
        assert_eq!(APP_SCHEME_OPTIONS & SCHEME_OPTION_LOCAL, 1 << 1);
        assert_eq!(APP_SCHEME_OPTIONS & SCHEME_OPTION_CORS_ENABLED, 1 << 4);
        assert_eq!(APP_SCHEME_OPTIONS & SCHEME_OPTION_FETCH_ENABLED, 1 << 6);
        // DISPLAY_ISOLATED and CSP_BYPASSING stay off.
        assert_eq!(APP_SCHEME_OPTIONS & (1 << 2), 0);
        assert_eq!(APP_SCHEME_OPTIONS & (1 << 5), 0);
    }
}
