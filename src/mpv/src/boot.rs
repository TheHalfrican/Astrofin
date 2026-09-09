//! End-to-end mpv handle bring-up: create, apply defaults + per-arg
//! options, initialize, and set log level — exposed as a single
//! `jfn_mpv_handle_init` entry point.
//!
//! Rust owns the lifetime: a process-global slot retains the
//! [`Handle`], and `jfn_mpv_handle_terminate` drops it, calling
//! `mpv_terminate_destroy` via [`Handle::Drop`].

use parking_lot::Mutex;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;
use std::sync::{Arc, OnceLock};

use crate::handle::Handle;
use crate::sys;

/// Display backend in use for this process.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DisplayBackend {
    Wayland = 0,
    X11 = 1,
    Other = 2,
}

impl DisplayBackend {
    fn from_raw(v: u8) -> Self {
        match v {
            0 => Self::Wayland,
            1 => Self::X11,
            _ => Self::Other,
        }
    }
}

/// Boot-time configuration handed to `jfn_mpv_handle_init`. Every
/// option applied between `mpv_create` and `mpv_initialize` lives here.
/// All string fields are NUL-terminated UTF-8 or
/// null; non-null pointers must remain valid for the duration of the
/// init call only (Rust copies what it needs).
#[repr(C)]
pub struct JfnMpvBoot {
    pub display_backend: u8,
    /// Hardware-decoding mode, e.g. `"auto"`, `"no"`, `"vaapi"`.
    pub hwdec: *const c_char,
    pub user_agent: *const c_char,
    /// Optional `--audio-spdif` codecs (e.g. `"ac3,dts-hd,eac3,truehd"`).
    pub audio_passthrough: *const c_char,
    pub audio_exclusive: bool,
    pub audio_channels: *const c_char,
    /// Optional `<W>x<H>[+x+y]` geometry string from saved settings.
    pub geometry: *const c_char,
    /// Native window ID mpv embeds into (`wid` option); 0 = mpv owns its
    /// own window.
    pub wid: i64,
    pub force_window_position: bool,
    pub window_maximized_at_boot: bool,
    /// libmpv log-message subscription level (`"no"`, `"error"`,
    /// `"warn"`, `"info"`, `"v"`, `"debug"`, `"trace"`).
    pub mpv_log_level: *const c_char,
    /// When set on Wayland, suppress mpv's server-side decoration request so
    /// the app's own client-side decorations don't stack under a compositor
    /// titlebar (e.g. KDE). No effect on X11 (WM draws decorations).
    pub client_side_decorations: bool,
}

/// Owns the Handle for the rest of the process. `mpv_terminate_destroy`
/// fires when the slot is taken via [`jfn_mpv_handle_terminate`].
fn handle_slot() -> &'static Mutex<Option<Arc<Handle>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<Handle>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

unsafe fn cstr_opt(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// Set a string option, tolerating options absent from the linked libmpv build
/// (e.g. wayland-app-id on the windows build). Warns and continues on
/// MPV_ERROR_OPTION_NOT_FOUND; propagates other errors after logging the name.
fn set_option_or_skip(handle: &Handle, name: &str, value: &str) -> crate::error::Result<()> {
    match handle.set_option_string(name, value) {
        Ok(()) => Ok(()),
        Err(e) if e.code == sys::mpv_error::MPV_ERROR_OPTION_NOT_FOUND.0 => {
            tracing::warn!(target: "mpv", "option {} not supported by this libmpv build; skipping", name);
            Ok(())
        }
        Err(e) => {
            tracing::error!(target: "mpv", "set_option_string({}={}) failed: {:?}", name, value, e);
            Err(e)
        }
    }
}

/// Flag variant of [`set_option_or_skip`].
fn set_option_flag_or_skip(handle: &Handle, name: &str, value: bool) -> crate::error::Result<()> {
    match handle.set_option_flag(name, value) {
        Ok(()) => Ok(()),
        Err(e) if e.code == sys::mpv_error::MPV_ERROR_OPTION_NOT_FOUND.0 => {
            tracing::warn!(target: "mpv", "option {} not supported by this libmpv build; skipping", name);
            Ok(())
        }
        Err(e) => {
            tracing::error!(target: "mpv", "set_option_flag({}={}) failed: {:?}", name, value, e);
            Err(e)
        }
    }
}

/// One option written between `mpv_create` and `mpv_initialize`, as a value.
///
/// The tables below are the whole of the boot policy; keeping them as data
/// rather than as a run of `set(..)?` calls is what makes that policy
/// readable back out (and testable) without a live mpv core.
#[derive(Clone, Debug, PartialEq)]
enum Opt {
    Str(&'static str, String),
    Flag(&'static str, bool),
}

impl Opt {
    fn str(name: &'static str, value: impl Into<String>) -> Self {
        Self::Str(name, value.into())
    }
}

/// Write `opts` in order, stopping at the first error that is not
/// "this libmpv build has no such option".
fn apply_options(handle: &Handle, opts: &[Opt]) -> crate::error::Result<()> {
    for opt in opts {
        match opt {
            Opt::Str(name, value) => set_option_or_skip(handle, name, value)?,
            Opt::Flag(name, value) => set_option_flag_or_skip(handle, name, *value)?,
        }
    }
    Ok(())
}

/// The options every run applies, whatever the caller asked for.
fn default_options(display: DisplayBackend, client_side_decorations: bool) -> Vec<Opt> {
    let mut opts = vec![
        // OSD/OSC off — CEF overlay handles all UI.
        Opt::str("osd-level", "0"),
        Opt::str("osc", "no"),
        Opt::str("display-tags", ""),
        // Track selection is owned by Jellyfin. Disable mpv's heuristic
        // so unspecified tracks stay disabled instead of being auto-picked
        // by language / default-flag / codec scoring.
        Opt::str("track-auto-selection", "no"),
        // Input: we own all devices and route through CEF.
        Opt::str("input-default-bindings", "no"),
        Opt::str("input-vo-keyboard", "no"),
        Opt::str("input-cursor", "no"),
        Opt::str("cursor-autohide", "no"),
    ];

    if display == DisplayBackend::Other {
        opts.push(Opt::str("input-vo-cursor", "no"));
        opts.push(Opt::str("input-keyboard", "no"));
    }

    // Disable mpv's clipboard so it keeps a single wl_display connection.
    if display == DisplayBackend::Wayland {
        opts.push(Opt::str("clipboard-backends", ""));
    }

    // Window behavior.
    opts.push(Opt::str("stop-screensaver", "no"));
    opts.push(Opt::str("keepaspect-window", "no"));
    opts.push(Opt::str("auto-window-resize", "no"));
    // Suppress the server-side decoration request on Wayland when the app
    // draws its own client-side decorations; otherwise a compositor titlebar
    // (e.g. KDE) would stack on top of ours.
    let suppress_ssd = display == DisplayBackend::Wayland && client_side_decorations;
    opts.push(Opt::str("border", if suppress_ssd { "no" } else { "yes" }));
    opts.push(Opt::str("title", "Astrofin"));
    opts.push(Opt::str(
        "wayland-app-id",
        "io.github.thehalfrican.Astrofin",
    ));

    // Keep window open when idle. `force-window=yes` (not "immediate")
    // avoids a macOS deadlock: "immediate" calls handle_force_window
    // inside `mpv_initialize`, which triggers `DispatchQueue.main.sync`
    // while the main thread is blocked in init.
    opts.push(Opt::str("force-window", "yes"));
    opts.push(Opt::str("idle", "yes"));

    opts
}

/// [`JfnMpvBoot`] with its C strings decoded, so the option table it maps to
/// is a pure function of owned Rust values.
#[derive(Clone, Debug, Default, PartialEq)]
struct BootSettings {
    hwdec: Option<String>,
    user_agent: Option<String>,
    audio_passthrough: Option<String>,
    audio_exclusive: bool,
    audio_channels: Option<String>,
    geometry: Option<String>,
    wid: i64,
    force_window_position: bool,
    window_maximized_at_boot: bool,
}

impl BootSettings {
    /// # Safety
    /// Every non-null string field of `boot` must be NUL-terminated and valid
    /// for the duration of the call.
    unsafe fn from_boot(boot: &JfnMpvBoot) -> Self {
        Self {
            hwdec: unsafe { cstr_opt(boot.hwdec) },
            user_agent: unsafe { cstr_opt(boot.user_agent) },
            audio_passthrough: unsafe { cstr_opt(boot.audio_passthrough) },
            audio_exclusive: boot.audio_exclusive,
            audio_channels: unsafe { cstr_opt(boot.audio_channels) },
            geometry: unsafe { cstr_opt(boot.geometry) },
            wid: boot.wid,
            force_window_position: boot.force_window_position,
            window_maximized_at_boot: boot.window_maximized_at_boot,
        }
    }
}

/// The options the caller's boot struct asks for, in the order they are
/// written.
fn boot_options(s: &BootSettings) -> Vec<Opt> {
    let mut opts = vec![
        // libmpv defaults config=no (opposite of the mpv CLI); enable it so
        // users' $MPV_HOME/mpv.conf is loaded.
        Opt::str("config", "yes"),
        // We only feed mpv direct media URLs from the Jellyfin server; the
        // youtube-dl/yt-dlp hook would just add startup latency.
        Opt::str("ytdl", "no"),
    ];

    if let Some(ua) = &s.user_agent {
        opts.push(Opt::str("user-agent", ua.clone()));
    }
    if let Some(hwdec) = &s.hwdec {
        opts.push(Opt::str("hwdec", hwdec.clone()));
    }
    if let Some(geom) = &s.geometry {
        opts.push(Opt::str("geometry", geom.clone()));
    }
    if s.wid != 0 {
        opts.push(Opt::str("wid", s.wid.to_string()));
    }
    if s.force_window_position {
        opts.push(Opt::str("force-window-position", "yes"));
    }
    if s.window_maximized_at_boot {
        opts.push(Opt::str("window-maximized", "yes"));
    }
    if let Some(spdif) = s.audio_passthrough.as_deref().filter(|v| !v.is_empty()) {
        opts.push(Opt::str("audio-spdif", spdif));
    }
    if s.audio_exclusive {
        opts.push(Opt::Flag("audio-exclusive", true));
    }
    if let Some(ch) = s.audio_channels.as_deref().filter(|v| !v.is_empty()) {
        opts.push(Opt::str("audio-channels", ch));
    }
    opts
}

fn apply_defaults(
    handle: &Handle,
    display: DisplayBackend,
    client_side_decorations: bool,
) -> crate::error::Result<()> {
    apply_options(handle, &default_options(display, client_side_decorations))
}

/// # Safety
/// See [`BootSettings::from_boot`].
unsafe fn apply_boot_options(handle: &Handle, boot: &JfnMpvBoot) -> crate::error::Result<()> {
    apply_options(
        handle,
        &boot_options(&unsafe { BootSettings::from_boot(boot) }),
    )
}

/// Create + configure + initialize the libmpv handle. On success, the
/// raw `mpv_handle*` is returned for callers to borrow. On failure,
/// returns null and any partially-initialized handle is destroyed
/// before returning.
///
/// # Safety
/// `boot` must point to a valid `JfnMpvBoot` whose string fields are
/// either null or NUL-terminated UTF-8 valid for the call.
pub unsafe fn jfn_mpv_handle_init(boot: *const JfnMpvBoot) -> *mut sys::mpv_handle {
    if boot.is_null() {
        return ptr::null_mut();
    }
    let boot = unsafe { &*boot };
    let display = DisplayBackend::from_raw(boot.display_backend);

    let handle = match Handle::create() {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(target: "mpv", "mpv_create failed: {:?}", e);
            return ptr::null_mut();
        }
    };

    if let Err(e) = apply_defaults(&handle, display, boot.client_side_decorations) {
        tracing::error!(target: "mpv", "apply_defaults failed: {:?}", e);
        return ptr::null_mut();
    }
    if let Err(e) = unsafe { apply_boot_options(&handle, boot) } {
        tracing::error!(target: "mpv", "apply_boot_options failed: {:?}", e);
        return ptr::null_mut();
    }

    // Wakeup callback exists only to unstick mpv_wait_event during
    // shutdown.
    handle.set_wakeup_callback(|| {});

    if let Err(e) = handle.initialize() {
        tracing::error!(target: "mpv", "mpv_initialize failed: {:?}", e);
        return ptr::null_mut();
    }

    // mpv log subscription. Token is the same one
    // `mpv_request_log_messages` accepts directly.
    if let Some(level) = unsafe { cstr_opt(boot.mpv_log_level) }
        && !level.is_empty()
    {
        unsafe {
            use std::ffi::CString;
            if let Ok(c) = CString::new(level) {
                sys::mpv_request_log_messages(handle.raw(), c.as_ptr());
            }
        }
    }

    let raw = handle.raw();
    *handle_slot().lock() = Some(Arc::new(handle));
    raw
}

/// Tear down the handle owned by [`jfn_mpv_handle_init`].
/// Idempotent — repeated calls are no-ops.
///
/// On macOS the caller must invoke this off the main thread (mpv's VO
/// uninit does `DispatchQueue.main.sync`).
pub fn jfn_mpv_handle_terminate() {
    let taken = handle_slot().lock().take();
    let Some(handle) = taken else {
        return;
    };
    if Arc::into_inner(handle).is_none() {
        tracing::warn!(
            target: "mpv",
            "mpv handle still referenced at terminate; mpv_terminate_destroy deferred"
        );
    }
}

/// Borrow the live raw `mpv_handle*`. Returns null before
/// [`jfn_mpv_handle_init`] succeeds and after
/// [`jfn_mpv_handle_terminate`].
pub fn jfn_mpv_handle_get() -> *mut sys::mpv_handle {
    current_raw_handle().unwrap_or(ptr::null_mut())
}

/// Returns the live mpv handle. `None` until [`jfn_mpv_handle_init`]
/// has succeeded.
pub fn current_raw_handle() -> Option<*mut sys::mpv_handle> {
    handle_slot().lock().as_ref().map(|h| h.raw())
}

/// Clone the process-global [`Handle`]. `None` before
/// [`jfn_mpv_handle_init`] succeeds and after
/// [`jfn_mpv_handle_terminate`].
pub fn current_handle() -> Option<Arc<Handle>> {
    handle_slot().lock().as_ref().map(Arc::clone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn value_of<'a>(opts: &'a [Opt], name: &str) -> Option<&'a Opt> {
        opts.iter().find(|o| match o {
            Opt::Str(n, _) | Opt::Flag(n, _) => *n == name,
        })
    }

    fn string_of(opts: &[Opt], name: &str) -> Option<String> {
        match value_of(opts, name) {
            Some(Opt::Str(_, v)) => Some(v.clone()),
            _ => None,
        }
    }

    fn names(opts: &[Opt]) -> Vec<&'static str> {
        opts.iter()
            .map(|o| match o {
                Opt::Str(n, _) | Opt::Flag(n, _) => *n,
            })
            .collect()
    }

    /// A `JfnMpvBoot` with every pointer null, i.e. "nothing configured".
    fn empty_boot() -> JfnMpvBoot {
        JfnMpvBoot {
            display_backend: DisplayBackend::Other as u8,
            hwdec: ptr::null(),
            user_agent: ptr::null(),
            audio_passthrough: ptr::null(),
            audio_exclusive: false,
            audio_channels: ptr::null(),
            geometry: ptr::null(),
            wid: 0,
            force_window_position: false,
            window_maximized_at_boot: false,
            mpv_log_level: ptr::null(),
            client_side_decorations: false,
        }
    }

    // ---- the process-global slot ------------------------------------------

    #[test]
    fn handle_init_refuses_a_null_boot_struct_instead_of_dereferencing_it() {
        assert!(unsafe { jfn_mpv_handle_init(ptr::null()) }.is_null());
    }

    #[test]
    fn handle_get_is_null_until_init_has_succeeded() {
        assert!(jfn_mpv_handle_get().is_null());
    }

    #[test]
    fn current_raw_handle_is_none_until_init_has_succeeded() {
        assert!(current_raw_handle().is_none());
    }

    #[test]
    fn current_handle_is_none_until_init_has_succeeded() {
        assert!(current_handle().is_none());
    }

    /// Shutdown runs it, and so does a failed start-up; neither must depend
    /// on a handle ever having existed.
    #[test]
    fn terminate_is_a_no_op_when_no_handle_was_ever_published() {
        jfn_mpv_handle_terminate();
        jfn_mpv_handle_terminate();
        assert!(current_handle().is_none());
    }

    // ---- the wire value for the display backend ---------------------------

    #[test]
    fn the_display_backend_byte_maps_to_the_three_backends() {
        assert!(DisplayBackend::from_raw(0) == DisplayBackend::Wayland);
        assert!(DisplayBackend::from_raw(1) == DisplayBackend::X11);
        assert!(DisplayBackend::from_raw(2) == DisplayBackend::Other);
        // Anything a future caller invents is "not a windowing system we
        // drive", which is the conservative branch.
        assert!(DisplayBackend::from_raw(3) == DisplayBackend::Other);
        assert!(DisplayBackend::from_raw(255) == DisplayBackend::Other);
    }

    // ---- default options --------------------------------------------------

    /// The overlay draws every pixel of UI, and CEF owns every input device,
    /// so mpv's own OSD, key bindings and cursor handling all have to go.
    #[test]
    fn the_defaults_hand_ui_and_input_to_the_overlay() {
        let opts = default_options(DisplayBackend::X11, false);
        for name in [
            "osd-level",
            "osc",
            "input-default-bindings",
            "input-vo-keyboard",
            "input-cursor",
            "cursor-autohide",
        ] {
            assert!(value_of(&opts, name).is_some(), "{name} missing");
        }
        assert_eq!(string_of(&opts, "osd-level").as_deref(), Some("0"));
        assert_eq!(string_of(&opts, "osc").as_deref(), Some("no"));
    }

    /// Jellyfin picks the tracks; mpv's language/default-flag scoring would
    /// silently override it.
    #[test]
    fn the_defaults_disable_mpvs_own_track_selection() {
        let opts = default_options(DisplayBackend::Wayland, false);
        assert_eq!(
            string_of(&opts, "track-auto-selection").as_deref(),
            Some("no")
        );
    }

    /// `force-window=yes` and not `immediate`: the latter deadlocks macOS
    /// inside `mpv_initialize`.
    #[test]
    fn the_defaults_keep_an_idle_window_open_without_the_macos_deadlock() {
        let opts = default_options(DisplayBackend::Other, false);
        assert_eq!(string_of(&opts, "force-window").as_deref(), Some("yes"));
        assert_eq!(string_of(&opts, "idle").as_deref(), Some("yes"));
        assert_eq!(string_of(&opts, "title").as_deref(), Some("Astrofin"));
        assert_eq!(
            string_of(&opts, "wayland-app-id").as_deref(),
            Some("io.github.thehalfrican.Astrofin")
        );
    }

    /// One `wl_display` connection per process: mpv's clipboard backend would
    /// open a second one.
    #[test]
    fn only_wayland_gets_the_clipboard_disabled() {
        assert_eq!(
            string_of(
                &default_options(DisplayBackend::Wayland, false),
                "clipboard-backends"
            )
            .as_deref(),
            Some("")
        );
        for display in [DisplayBackend::X11, DisplayBackend::Other] {
            assert!(value_of(&default_options(display, false), "clipboard-backends").is_none());
        }
    }

    /// Client-side decorations only clash with a compositor titlebar, which
    /// is a Wayland-only thing; on X11 the WM draws the border either way.
    #[test]
    fn client_side_decorations_suppress_the_border_on_wayland_only() {
        assert_eq!(
            string_of(&default_options(DisplayBackend::Wayland, true), "border").as_deref(),
            Some("no")
        );
        assert_eq!(
            string_of(&default_options(DisplayBackend::Wayland, false), "border").as_deref(),
            Some("yes")
        );
        for display in [DisplayBackend::X11, DisplayBackend::Other] {
            assert_eq!(
                string_of(&default_options(display, true), "border").as_deref(),
                Some("yes"),
                "{}",
                display as u8
            );
        }
    }

    /// With no windowing system of ours to route through, mpv must not read
    /// the keyboard or the cursor itself either.
    #[test]
    fn a_backend_we_do_not_drive_also_loses_vo_cursor_and_keyboard_input() {
        let other = default_options(DisplayBackend::Other, false);
        assert_eq!(string_of(&other, "input-vo-cursor").as_deref(), Some("no"));
        assert_eq!(string_of(&other, "input-keyboard").as_deref(), Some("no"));
        for display in [DisplayBackend::Wayland, DisplayBackend::X11] {
            let opts = default_options(display, false);
            assert!(value_of(&opts, "input-vo-cursor").is_none());
            assert!(value_of(&opts, "input-keyboard").is_none());
        }
    }

    // ---- boot options -----------------------------------------------------

    /// libmpv defaults `config=no`, the opposite of the mpv CLI, so the
    /// user's own mpv.conf would never be read without this.
    #[test]
    fn boot_options_always_load_the_users_config_and_skip_ytdl() {
        let opts = boot_options(&BootSettings::default());
        assert_eq!(names(&opts), ["config", "ytdl"]);
        assert_eq!(string_of(&opts, "config").as_deref(), Some("yes"));
        assert_eq!(string_of(&opts, "ytdl").as_deref(), Some("no"));
    }

    #[test]
    fn boot_options_carry_every_field_the_caller_set() {
        let settings = BootSettings {
            hwdec: Some("d3d11va".into()),
            user_agent: Some("Astrofin/1".into()),
            audio_passthrough: Some("ac3,dts-hd".into()),
            audio_exclusive: true,
            audio_channels: Some("7.1".into()),
            geometry: Some("1280x720+10+20".into()),
            wid: 4242,
            force_window_position: true,
            window_maximized_at_boot: true,
        };
        let opts = boot_options(&settings);
        assert_eq!(string_of(&opts, "hwdec").as_deref(), Some("d3d11va"));
        assert_eq!(
            string_of(&opts, "user-agent").as_deref(),
            Some("Astrofin/1")
        );
        assert_eq!(
            string_of(&opts, "audio-spdif").as_deref(),
            Some("ac3,dts-hd")
        );
        assert_eq!(string_of(&opts, "audio-channels").as_deref(), Some("7.1"));
        assert_eq!(
            string_of(&opts, "geometry").as_deref(),
            Some("1280x720+10+20")
        );
        assert_eq!(string_of(&opts, "wid").as_deref(), Some("4242"));
        assert_eq!(
            string_of(&opts, "force-window-position").as_deref(),
            Some("yes")
        );
        assert_eq!(string_of(&opts, "window-maximized").as_deref(), Some("yes"));
        // Exclusive audio is the one flag-typed option.
        assert_eq!(
            value_of(&opts, "audio-exclusive"),
            Some(&Opt::Flag("audio-exclusive", true))
        );
    }

    /// An empty passthrough or channel string means "not configured"; writing
    /// it would turn spdif on with no codecs, or force a layout of "".
    #[test]
    fn an_empty_audio_string_is_not_written_at_all() {
        let settings = BootSettings {
            audio_passthrough: Some(String::new()),
            audio_channels: Some(String::new()),
            ..BootSettings::default()
        };
        let opts = boot_options(&settings);
        assert!(value_of(&opts, "audio-spdif").is_none());
        assert!(value_of(&opts, "audio-channels").is_none());
    }

    /// `wid` 0 is the sentinel for "mpv owns its own window"; writing it
    /// would ask mpv to embed into window handle zero.
    #[test]
    fn a_zero_window_id_is_left_to_mpv() {
        assert!(value_of(&boot_options(&BootSettings::default()), "wid").is_none());
        let embedded = BootSettings {
            wid: -1,
            ..BootSettings::default()
        };
        assert_eq!(
            string_of(&boot_options(&embedded), "wid").as_deref(),
            Some("-1")
        );
    }

    /// The false flags are absent rather than written as "no": mpv's own
    /// default already is off, and this keeps the boot log short.
    #[test]
    fn flags_left_off_produce_no_option_at_all() {
        let opts = boot_options(&BootSettings::default());
        for name in [
            "force-window-position",
            "window-maximized",
            "audio-exclusive",
        ] {
            assert!(value_of(&opts, name).is_none(), "{name}");
        }
    }

    /// An empty string is a value the caller chose (`--hwdec=`); only a null
    /// pointer means "unset". The two must not collapse.
    #[test]
    fn decoding_a_boot_struct_keeps_null_apart_from_empty() {
        let empty = CString::new("").expect("cstring");
        let hwdec = CString::new("auto").expect("cstring");
        let mut boot = empty_boot();
        boot.hwdec = hwdec.as_ptr();
        boot.user_agent = empty.as_ptr();

        let decoded = unsafe { BootSettings::from_boot(&boot) };
        assert_eq!(decoded.hwdec.as_deref(), Some("auto"));
        assert_eq!(decoded.user_agent.as_deref(), Some(""));
        assert_eq!(decoded.geometry, None);

        let opts = boot_options(&decoded);
        assert_eq!(string_of(&opts, "hwdec").as_deref(), Some("auto"));
        assert_eq!(string_of(&opts, "user-agent").as_deref(), Some(""));
        assert!(value_of(&opts, "geometry").is_none());
    }

    #[test]
    fn decoding_an_all_null_boot_struct_configures_nothing() {
        let decoded = unsafe { BootSettings::from_boot(&empty_boot()) };
        assert_eq!(decoded, BootSettings::default());
    }
}
