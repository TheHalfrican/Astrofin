//! Process entry point. [`jfn_app_main`] owns the full main loop and
//! returns the exit code.

use std::ffi::{CStr, CString, c_char, c_int};
use std::ptr;
use std::time::Duration;

use clap::Parser;
use jfn_cef::{APP_VERSION_FULL, cef_version};
use jfn_instance_ipc::jfn::{Request, Response};
use jfn_instance_ipc::{Listener, Start, Stream};
use jfn_platform_abi::{IdleInhibitLevel, Instance, LogicalSize, Platform, WindowGeometry};

use crate::cli;

// Shorthand for the installed Platform backend. `install()` happens before
// any of the call sites here run.
fn plat() -> &'static dyn Platform {
    jfn_platform_abi::get()
}

// Read once by `jfn_app_main` after CEF boot to seed the theme rotator.
static VIDEO_BG: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn video_bg_set(rgb: u32) {
    VIDEO_BG.store(rgb, std::sync::atomic::Ordering::Release);
}

fn video_bg_get() -> u32 {
    VIDEO_BG.load(std::sync::atomic::Ordering::Acquire)
}

/// mpv background applied over the user's mpv.conf color for the app's
/// lifetime before the theme rotator takes over.
const STARTUP_BG_HEX: &str = "#101010";

/// Set once the startup background override has replaced the user's color.
static STARTUP_BG_APPLIED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Shared-texture decision `CefInitialize` was given.
static SHARED_TEXTURES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether CEF was initialized with shared-texture compositing.
fn shared_textures() -> bool {
    SHARED_TEXTURES.load(std::sync::atomic::Ordering::Acquire)
}

pub(crate) const DEFAULT_LOG_FILTER: &str = "info";

struct BootArgs {
    disable_gpu_compositing: bool,
    remote_debugging_port: c_int,
}

fn cs(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

/// Normalize the audio-passthrough list: if `dts-hd` is present, drop
/// bare `dts` (the HD variant subsumes it).
fn normalize_passthrough(s: &str) -> String {
    if !s.contains("dts-hd") {
        return s.to_string();
    }
    s.split(',')
        .filter(|c| *c != "dts")
        .collect::<Vec<_>>()
        .join(",")
}

fn print_version() {
    println!("astrofin {}\n\nCEF {}\n", APP_VERSION_FULL, cef_version());
    use std::io::Write;
    let _ = std::io::stdout().flush();
    jfn_mpv::probe::jfn_mpv_print_version_info();
}

fn init_logging(log_file: Option<String>, log_level: &str) {
    let log_path = log_file.unwrap_or_else(|| {
        jfn_paths::default_log_file()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });

    let filter = if log_level.is_empty() {
        DEFAULT_LOG_FILTER.to_string()
    } else {
        log_level.to_string()
    };
    jfn_logging::jfn_log_init(&log_path, &filter);

    tracing::info!(target: "Main", "astrofin {APP_VERSION_FULL}");
    tracing::info!(target: "Main", "CEF {}", cef_version());
    if !log_path.is_empty() {
        tracing::info!(target: "Main", "Log file: {log_path}");
    }
}

fn log_mpv_versions() {
    for prop in ["mpv-version", "ffmpeg-version"] {
        let pc = cs(prop);
        let v = unsafe { jfn_mpv::api::jfn_mpv_get_property_string(pc.as_ptr()) };
        let s = if v.is_null() {
            String::new()
        } else {
            let s = unsafe { CStr::from_ptr(v) }.to_string_lossy().into_owned();
            unsafe { jfn_mpv::api::jfn_mpv_free_string(v) };
            s
        };
        tracing::info!(target: "Main", "{prop} {s}");
    }
}

/// Restores the builtin CLOSE_WIN -> quit binding that
/// `input-default-bindings=no` drops. Async: the boot path never parks on
/// mpv's core.
fn install_mpv_close_binding() {
    let kb = cs("keybind");
    let name = cs("CLOSE_WIN");
    let action = cs("quit");
    let argv = [kb.as_ptr(), name.as_ptr(), action.as_ptr()];
    unsafe { jfn_mpv::api::jfn_mpv_command_async(argv.as_ptr(), argv.len()) };
}

/// Wake any thread parked in `mpv_wait_event` whenever a host publishes a
/// window change, so the VO wait re-reads the readiness inputs mpv never
/// reports: `MpvHost::host_ready` and the host-owned extent on backends that
/// own their toplevel.
fn wake_mpv_on_window_change() {
    jfn_platform_abi::subscribe_window_changed(jfn_mpv::api::jfn_mpv_wakeup);
}

fn setup_mpv_environment() {
    let mpv_home = jfn_paths::mpv_home();
    unsafe {
        std::env::set_var("MPV_HOME", &mpv_home);
    }

    plat()
        .mpv_host()
        .prepare(jfn_config::configured_window_decorations());
}

struct StartupOptions {
    hwdec: String,
    video_mode: jfn_mpv::VideoMode,
    audio_passthrough: String,
    audio_exclusive: bool,
    audio_channels: String,
    log_level: String,
    log_file: Option<String>,
    disable_gpu_compositing: bool,
    remote_debugging_port: c_int,
}

/// The persisted video mode, normalised once onto the post-rename names.
///
/// A `settings.json` written before the rename carries no `videoModeMigrated`
/// marker; there `movies`/`anime` mean live-action/animation and `off` meant
/// "leave mpv.conf alone", whose closest new behaviour is auto (the new `off`
/// actively clears every shader). The normalised name is written straight
/// back, and every file this build saves carries the marker, so this runs at
/// most once per profile.
fn stored_video_mode() -> jfn_mpv::VideoMode {
    let raw = jfn_config::video_mode();
    if jfn_config::video_mode_migrated() {
        return jfn_mpv::VideoMode::parse(&raw).unwrap_or_default();
    }
    let mode = jfn_mpv::VideoMode::parse_legacy(&raw).unwrap_or_default();
    jfn_config::set_video_mode(mode.as_str());
    jfn_config::settings_save_async();
    mode
}

/// The `settings.json` layer of [`resolve_startup_options`], read in one place
/// so [`resolve_from_saved`] is a pure function of (saved settings, argv).
#[derive(Default)]
struct SavedOptions {
    hwdec: String,
    video_mode: jfn_mpv::VideoMode,
    audio_passthrough: String,
    audio_channels: String,
    log_level: String,
    audio_exclusive: bool,
}

fn saved_options() -> SavedOptions {
    SavedOptions {
        hwdec: jfn_config::hwdec(),
        video_mode: stored_video_mode(),
        audio_passthrough: jfn_config::audio_passthrough(),
        audio_channels: jfn_config::audio_channels(),
        log_level: jfn_config::log_level(),
        audio_exclusive: jfn_config::audio_exclusive(),
    }
}

fn resolve_startup_options(cli: &cli::Cli) -> StartupOptions {
    resolve_from_saved(saved_options(), cli)
}

/// Flag > environment variable > `settings.json` > built-in default, for every
/// startup option. The environment layer is clap's: the four `ASTROFIN_*`
/// variables are already folded into `cli` by the time this runs, which is why
/// a variable outranks a saved setting and a flag outranks both.
fn resolve_from_saved(saved: SavedOptions, cli: &cli::Cli) -> StartupOptions {
    let SavedOptions {
        hwdec: saved_hwdec,
        video_mode: saved_video_mode,
        audio_passthrough: saved_pass,
        audio_channels: saved_chans,
        log_level: saved_log_level,
        audio_exclusive: saved_audio_exclusive,
    } = saved;

    let mpv_hwdec_default = jfn_mpv::HWDEC_DEFAULT.to_string();

    let mut hwdec = if saved_hwdec.is_empty() {
        mpv_hwdec_default.clone()
    } else {
        saved_hwdec
    };
    // Empty (never chosen) resolves to the built-in default, and so does an
    // unparseable value left by a hand edit or an older/newer build.
    let mut video_mode = saved_video_mode;
    let mut audio_passthrough = saved_pass;
    let mut audio_exclusive = saved_audio_exclusive;
    let mut audio_channels = saved_chans;
    let mut log_level = saved_log_level;

    let log_file = cli.log_file.clone();
    let mut disable_gpu_compositing = false;
    let mut remote_debugging_port: c_int = 0;

    if let Some(v) = cli.hwdec.clone() {
        hwdec = v;
    }
    if let Some(v) = cli.video_mode.clone() {
        // A flag is an explicit choice for this run only: `off` here always
        // means the new "no shaders", never the legacy "leave mpv.conf alone".
        video_mode = jfn_mpv::VideoMode::parse(&v).unwrap_or(video_mode);
    }
    if let Some(v) = cli.audio_passthrough.clone() {
        audio_passthrough = v;
    }
    if let Some(v) = cli.audio_channels.clone() {
        audio_channels = v;
    }
    if let Some(v) = cli.log_level.clone() {
        log_level = v;
    }
    if cli.audio_exclusive {
        audio_exclusive = true;
    }
    if cli.disable_gpu_compositing {
        disable_gpu_compositing = true;
    }
    if let Some(p) = cli.remote_debug_port {
        remote_debugging_port = p;
    }

    if !jfn_mpv::is_valid_hwdec(&hwdec) {
        hwdec = mpv_hwdec_default;
    }

    if !audio_passthrough.is_empty() {
        audio_passthrough = normalize_passthrough(&audio_passthrough);
    }

    StartupOptions {
        hwdec,
        video_mode,
        audio_passthrough,
        audio_exclusive,
        audio_channels,
        log_level,
        log_file,
        disable_gpu_compositing,
        remote_debugging_port,
    }
}

struct MpvInitOptions<'a> {
    backend_byte: u8,
    boot_geometry: Option<&'a str>,
    boot_force_position: bool,
    boot_window_max: bool,
    embed_wid: Option<i64>,
    hwdec: &'a str,
    audio_passthrough: &'a str,
    audio_exclusive: bool,
    audio_channels: &'a str,
    mpv_log_level: &'a str,
}

fn init_mpv_handle(opts: MpvInitOptions<'_>) -> *mut jfn_mpv::sys::mpv_handle {
    let geometry_c = opts.boot_geometry.map(cs);
    let hwdec_c = cs(opts.hwdec);
    let user_agent_c = cs(&format!("Astrofin/{}", APP_VERSION_FULL));
    let passthrough_c = cs(opts.audio_passthrough);
    let channels_c = cs(opts.audio_channels);
    let mpv_log_level_c = cs(opts.mpv_log_level);
    let boot = jfn_mpv::boot::JfnMpvBoot {
        display_backend: opts.backend_byte,
        hwdec: hwdec_c.as_ptr(),
        user_agent: user_agent_c.as_ptr(),
        audio_passthrough: if opts.audio_passthrough.is_empty() {
            ptr::null()
        } else {
            passthrough_c.as_ptr()
        },
        audio_exclusive: opts.audio_exclusive,
        audio_channels: if opts.audio_channels.is_empty() {
            ptr::null()
        } else {
            channels_c.as_ptr()
        },
        geometry: geometry_c.as_ref().map_or(ptr::null(), |c| c.as_ptr()),
        wid: opts.embed_wid.unwrap_or(0),
        force_window_position: opts.boot_force_position,
        window_maximized_at_boot: opts.boot_window_max,
        mpv_log_level: mpv_log_level_c.as_ptr(),
        client_side_decorations: jfn_config::client_side_decorations(),
    };
    unsafe { jfn_mpv::boot::jfn_mpv_handle_init(&boot as *const _) }
}

/// Blocks until the window source has a usable extent. Returns false on
/// a fatal mpv event (shutdown before the VO came up).
fn wait_for_vo_window() -> bool {
    tracing::info!(target: "Main", "Waiting for mpv window...");
    let started = std::time::Instant::now();

    let mut fatal = false;

    // The platform owns the wait strategy; this pump owns all mpv event
    // handling. It drains everything mpv has queued without blocking, then,
    // when the platform's strategy grants a block budget, parks in mpv for at
    // most that long.
    plat().mpv_host().run_vo_wait(&mut |budget: Duration| {
        loop {
            match consume_boot_event(jfn_mpv::api::wait_event_owned(0.0)) {
                BootEvent::Idle => break,
                BootEvent::Fatal => {
                    fatal = true;
                    return false;
                }
                BootEvent::Consumed => {}
            }
        }
        if boot_ready() {
            return false;
        }
        if !budget.is_zero()
            && matches!(
                consume_boot_event(jfn_mpv::api::wait_event_owned(budget.as_secs_f64())),
                BootEvent::Fatal
            )
        {
            fatal = true;
            return false;
        }
        true
    });

    if fatal {
        return false;
    }
    tracing::info!(target: "Main",
        "mpv window ready in {} ms", started.elapsed().as_millis());
    true
}

/// Runs a synchronous libmpv read where the main thread must not block on
/// mpv's core lock.
///
/// mpv's macOS VO thread services window work through
/// `DispatchQueue.main.sync`, and the core thread waits on the VO while it
/// applies an option that touches the window (the startup `background-color`
/// write, for one). A main-thread `mpv_get_property` that arrives in that
/// window parks all three threads for good; whether it does is a race
/// against how fast the window came up, so it only shows on a warm start.
/// `Platform::run_blocking` runs `f` on a side thread while main keeps its
/// run loop pumping on macOS, and inline where nothing needs main. `None`
/// only if the side thread died before answering.
fn mpv_read_off_main<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    plat().run_blocking(Box::new(move || {
        let _ = tx.send(f());
    }));
    rx.recv().ok()
}

fn publish_device_profile(mpv_raw: *mut jfn_mpv::sys::mpv_handle) {
    // Raw pointers are not `Send`; the handle outlives every boot step, so
    // the side thread may read through it.
    let addr = mpv_raw as usize;
    let caps = mpv_read_off_main(move || unsafe {
        jfn_mpv::capabilities::query_raw(addr as *mut jfn_mpv::sys::mpv_handle)
    })
    .unwrap_or_default();
    let decoders: Vec<jfn_jellyfin::Codec> = caps
        .decoders
        .into_iter()
        .map(|c| jfn_jellyfin::Codec {
            name: c.name,
            kind: match c.kind {
                jfn_mpv::capabilities::MediaKind::Video => jfn_jellyfin::MediaKind::Video,
                jfn_mpv::capabilities::MediaKind::Audio => jfn_jellyfin::MediaKind::Audio,
                jfn_mpv::capabilities::MediaKind::Subtitle => jfn_jellyfin::MediaKind::Subtitle,
            },
        })
        .collect();
    let force = jfn_config::force_transcoding();
    let profile = jfn_jellyfin::build_device_profile(
        &decoders,
        &caps.demuxers,
        "Astrofin",
        APP_VERSION_FULL,
        force,
    );
    tracing::info!(target: "Main", "Device profile: {profile}");
    unsafe {
        jfn_cef::injection::jfn_cef_set_device_profile_json(
            profile.as_ptr() as *const _,
            profile.len(),
        );
    }
}

/// `CefInitialize` with the flags this boot resolved, recording the
/// shared-texture decision for [`shared_textures`]. A call after a successful
/// one returns true without re-entering CEF.
fn ensure_cef_initialized(ba: &BootArgs) -> bool {
    if CEF_INITED.load(std::sync::atomic::Ordering::Acquire) {
        return true;
    }
    let use_shared_textures = plat().shared_texture_supported() && !ba.disable_gpu_compositing;
    SHARED_TEXTURES.store(use_shared_textures, std::sync::atomic::Ordering::Release);
    jfn_cef::ffi::jfn_cef_set_log_severity(cef_severity_for_cef_filter());
    jfn_cef::ffi::jfn_cef_set_remote_debugging_port(ba.remote_debugging_port);
    jfn_cef::ffi::jfn_cef_set_disable_gpu_compositing(!use_shared_textures);
    jfn_cef::ffi::jfn_cef_set_platform_switches(plat().display());
    tracing::info!(target: "Main", "[FLOW] calling CefInitialize...");
    let started = std::time::Instant::now();
    if !jfn_cef::ffi::jfn_cef_initialize() {
        tracing::error!(target: "Main", "CefInitialize failed");
        return false;
    }
    CEF_INITED.store(true, std::sync::atomic::Ordering::Release);
    tracing::info!(target: "Main",
        "[FLOW] CefInitialize returned ok in {} ms", started.elapsed().as_millis());
    true
}

fn start_playback_coordination(instance: &Instance) -> bool {
    jfn_playback::ffi::jfn_playback_init();
    COORD_INITED.store(true, std::sync::atomic::Ordering::Release);

    jfn_playback::idle_inhibit_sink::jfn_playback_set_idle_inhibit_handler(Some(h_idle_inhibit));
    jfn_playback::theme_color_sink::jfn_playback_set_theme_video_mode_handler(Some(
        h_theme_video_mode,
    ));
    jfn_playback::exec_js::jfn_playback_set_web_exec_js_handler(Some(h_web_exec_js));
    jfn_playback::browser_sink::jfn_playback_set_browsers_refresh_rate_handler(Some(
        h_browsers_set_refresh_rate,
    ));

    plat().media_session().start(instance);

    jfn_playback::ingest_driver::jfn_playback_set_scale_provider(|| {
        let s = plat().get_scale();
        if s > 0.0 { s } else { 1.0 }
    });
    jfn_playback::ingest_driver::jfn_playback_set_fullscreen_handler(|fs| {
        plat().set_fullscreen(fs)
    });
    jfn_playback::ingest_driver::jfn_playback_set_shutdown_handler(|| {
        tracing::info!(target: "Main", "MPV_EVENT_SHUTDOWN received");
        jfn_playback::jfn_shutdown_initiate();
    });

    tracing::info!(target: "Main", "[FLOW] starting Rust-owned mpv event thread");
    if !jfn_playback::ingest_driver::jfn_playback_start_mpv_event_thread() {
        tracing::error!(target: "Main", "failed to start mpv event thread");
        return false;
    }

    true
}

fn shutdown_runtime(manager_thread: std::thread::JoinHandle<()>) {
    // Persist before the joins below: they can block on a VO-teardown
    // roundtrip, and a hang there must not cost the window geometry.
    crate::window_geometry::controller().persist();
    jfn_config::settings_save();

    // Join before any teardown so no posted task outlives the layer free below.
    let _ = manager_thread.join();

    // Sever host↔mpv links that could deadlock the teardown below once
    // CEF threads start dying.
    plat().mpv_host().detach();

    jfn_color::theme::jfn_theme_color_shutdown();
    plat().media_session().stop();

    jfn_playback::ingest_driver::jfn_playback_stop_mpv_event_thread();

    jfn_config::settings_shutdown_save_worker();

    jfn_cef::browsers::jfn_browsers_shutdown();
    jfn_cef::ffi::jfn_cef_shutdown();
    CEF_INITED.store(false, std::sync::atomic::Ordering::Release);

    plat().set_idle_inhibit(IdleInhibitLevel::None);

    plat().cleanup();
    PLATFORM_INITED.store(false, std::sync::atomic::Ordering::Release);

    jfn_playback::ffi::jfn_playback_shutdown();
    COORD_INITED.store(false, std::sync::atomic::Ordering::Release);
}

/// Boot-time mpv size reconcile (saved scale vs live display scale);
/// seeds the display-hz cache and returns it for browser init.
fn boot_mpv_reconcile(mpv_raw: *mut jfn_mpv::sys::mpv_handle) -> f64 {
    // Two more sync reads after CEF init; same main-thread hazard as the
    // device profile, see mpv_read_off_main.
    let addr = mpv_raw as usize;
    let display_hidpi_scale = mpv_read_off_main(move || {
        let mut scale: f64 = 0.0;
        unsafe {
            let name = cs("display-hidpi-scale");
            jfn_mpv::sys::mpv_get_property(
                addr as *mut jfn_mpv::sys::mpv_handle,
                name.as_ptr(),
                jfn_mpv::sys::mpv_format::MPV_FORMAT_DOUBLE,
                &mut scale as *mut f64 as *mut std::ffi::c_void,
            );
        }
        jfn_playback::ingest_driver::jfn_playback_seed_display_hz_sync();
        scale
    })
    .unwrap_or(0.0);
    let hz = jfn_playback::ingest_driver::jfn_playback_display_hz();
    let saved = jfn_config::window_geometry();
    let snap = crate::window_geometry::controller().source().snapshot();
    tracing::info!(target: "Main",
        "[FLOW] display-hidpi-scale={display_hidpi_scale} fullscreen={} display-hz={hz}",
        snap.fullscreen
    );

    // Saved intent, not an observation: the OS may still be applying the
    // maximize, and a set_geometry landing mid-flight leaves mpv's stored
    // window size disagreeing with the visible window.
    let locked = saved.maximized || snap.fullscreen || snap.maximized;
    if let Some(physical) = plat().reconcile_mpv_size(
        display_hidpi_scale,
        saved.scale,
        LogicalSize {
            w: saved.logical_width,
            h: saved.logical_height,
        },
        locked,
    ) {
        let clamped = plat().clamp_window_geometry(WindowGeometry {
            w: physical.w,
            h: physical.h,
            position: None,
        });
        let (new_pw, new_ph) = (clamped.w, clamped.h);
        let geom_str = format!("{new_pw}x{new_ph}");
        tracing::info!(target: "Main",
            "[FLOW] scale {:.3} -> {:.3}, resize to {}", saved.scale, display_hidpi_scale, geom_str);
        let g_c = cs(&geom_str);
        unsafe { jfn_mpv::api::jfn_mpv_set_geometry(g_c.as_ptr()) };
    }

    hz
}

fn init_main_browser(
    hz: f64,
    use_shared_textures: bool,
) -> (std::thread::JoinHandle<()>, *mut jfn_cef::JfnCefLayer) {
    // Must run before main browser create: the pre-loaded page fires its
    // initial theme-color IPC at DOMContentLoaded.
    let titlebar_themed = jfn_config::titlebar_theme_color();
    unsafe {
        jfn_color::theme::jfn_theme_color_init(
            if titlebar_themed {
                Some(h_theme_set_titlebar)
            } else {
                None
            },
            Some(h_theme_set_mpv_bg),
        );
    }
    jfn_color::theme::jfn_theme_color_set_video_bg(video_bg_get());

    jfn_cef::browsers::jfn_browsers_init(hz, use_shared_textures);
    let manager_thread = crate::manager::jfn_manager_start();
    jfn_playback::jfn_shutdown_set_handler(Some(h_shutdown_wake_manager));

    let web_kind = cs("web");
    let main_layer = unsafe { jfn_cef::browsers::jfn_browsers_create(web_kind.as_ptr()) };
    jfn_cef::business_web::jfn_web_init(main_layer);

    let server_url = startup_server_url(jfn_config::server_url());
    tracing::info!(target: "Main", "[FLOW] CreateBrowser(main) url={server_url}");
    unsafe {
        jfn_cef::client::jfn_cef_layer_create(
            main_layer,
            server_url.as_ptr() as *const _,
            server_url.len(),
        );
    }
    tracing::info!(target: "Main", "[FLOW] CreateBrowser(main) call returned");

    tracing::info!(target: "Main", "[FLOW] jfn_overlay_init(main_layer)");
    jfn_cef::business_overlay::jfn_overlay_init(main_layer);
    tracing::info!(target: "Main", "[FLOW] jfn_overlay_init returned");

    (manager_thread, main_layer)
}

pub fn jfn_app_main() -> c_int {
    crate::platform_install::install_early();

    let rc = jfn_cef::ffi::jfn_cef_start();
    if rc >= 0 {
        return rc;
    }

    // Path overrides must be applied before settings load and CEF
    // root_cache_path construction below.
    let cli = cli::Cli::parse();
    if cli.version {
        print_version();
        return 0;
    }
    // The overrides are also exported as ASTROFIN_CONFIG_DIR/ASTROFIN_CACHE_DIR
    // so the CEF helper processes, which `jfn_cef_start` returned from above
    // before argv was parsed and which load settings.json on their own for
    // the injected `jmpInfo`, inherit the same directories. Single-threaded
    // here: nothing has been spawned yet.
    if let Some(path) = &cli.config_dir {
        jfn_paths::set_config_dir_override(path.into());
        unsafe { std::env::set_var(jfn_paths::ENV_CONFIG_DIR, path) };
    }
    if let Some(path) = &cli.cache_dir {
        jfn_paths::set_cache_dir_override(path.into());
        unsafe { std::env::set_var(jfn_paths::ENV_CACHE_DIR, path) };
    }

    // Only the browser process reaches this point (helper processes returned
    // above), so no two processes race the import. It has to run before the
    // first `config_dir()`/`cache_dir()` call, which create what they return,
    // and before `settings_init` and `Instance::for_config_dir` so the
    // imported settings.json and instance.json are the ones that load.
    // Logging is not up yet; the report is emitted right after init_logging.
    let migration = jfn_paths::migrate_legacy();

    // Repair for profiles imported before the import learned to rewrite paths:
    // absolute `glsl-shaders=` entries in mpv.conf still naming the legacy
    // jellium-desktop folder. Idempotent, and a no-op on every other install.
    let conf_repair = jfn_paths::repair_mpv_conf();

    let settings_path = jfn_paths::config_dir().join("settings.json");
    jfn_config::settings_init(&settings_path);
    jfn_config::settings_load();

    let opts = resolve_startup_options(&cli);

    init_logging(opts.log_file.clone(), &opts.log_level);

    for line in migration.info_lines() {
        tracing::info!(target: "Main", "{line}");
    }
    for line in &conf_repair {
        tracing::info!(target: "Main", "{line}");
    }
    for line in migration.warnings() {
        tracing::warn!(target: "Main", "{line}");
    }

    crate::platform_install::install_from_cli(&cli);

    let _ = crate::window_geometry::controller();

    plat().install_shutdown_handler(jfn_playback::jfn_shutdown_initiate);

    let instance = match Instance::for_config_dir(&jfn_paths::config_dir()) {
        Ok(instance) => instance,
        Err(e) => {
            tracing::error!(target: "Main", "establishing instance identity: {e}");
            return 1;
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!(target: "Main", "tokio runtime: {e}");
            return 1;
        }
    };
    // The accept loop lives on `runtime`'s workers while `run_app` blocks the
    // main thread on the native loop, so `runtime` must outlive `run_app`.
    match runtime.block_on(Listener::try_start(
        &instance,
        jfn_instance_ipc::jfn::handle,
    )) {
        Start::Started(_listener) => run_app(&instance, opts),
        Start::AlreadyRunning => runtime.block_on(notify_running(&instance)),
        Start::Failed(e) => {
            tracing::error!(target: "Main", "could not start instance IPC: {e}");
            1
        }
    }
}

async fn notify_running(instance: &Instance) -> c_int {
    let acked = async {
        let mut stream = Stream::connect(instance).await?;
        stream.send(&Request::Ping).await?;
        stream.recv::<Response>().await
    }
    .await;
    match acked {
        Ok(Some(_)) => tracing::info!(target: "Main", "Signaled existing instance, exiting"),
        Ok(None) => tracing::warn!(target: "Main", "existing instance closed without ack"),
        Err(e) => tracing::warn!(target: "Main", "could not signal existing instance: {e}"),
    }
    0
}

fn run_app(instance: &Instance, opts: StartupOptions) -> c_int {
    // Boot geometry resolves before the host prepare so its display probes
    // hit the real server, not the mpv proxy the prepare may install.
    let boot = crate::window_geometry::controller().boot();
    plat().apply_boot_geometry(&boot);

    setup_mpv_environment();

    // Hosts that own their toplevel create it here, before mpv init, so its
    // window ID can be handed to mpv as `wid`.
    plat().mpv_host().ensure_host_window();

    let mpv_log_level = mpv_log_level_from_filter();

    // mpv's --geometry takes physical pixels (see m_geometry_apply in
    // third_party/mpv/options/m_option.c). Window boot options only apply
    // when mpv owns the window; toplevel-owning backends size and
    // position/maximize the host window themselves.
    let backend_byte: u8 = plat().display() as u8;
    let boot_mpv_geometry = plat().boot_mpv_geometry(&boot);
    let mpv_owns_window = boot_mpv_geometry.is_some();
    let mpv_started = std::time::Instant::now();
    let raw = init_mpv_handle(MpvInitOptions {
        backend_byte,
        boot_geometry: boot_mpv_geometry.as_deref(),
        boot_force_position: mpv_owns_window && boot.force_position(),
        boot_window_max: mpv_owns_window && boot.maximized(),
        embed_wid: plat().mpv_host().embed_wid(),
        hwdec: &opts.hwdec,
        audio_passthrough: &opts.audio_passthrough,
        audio_exclusive: opts.audio_exclusive,
        audio_channels: &opts.audio_channels,
        mpv_log_level,
    });
    if raw.is_null() {
        tracing::error!(target: "Main", "mpv handle init failed");
        return 1;
    }
    tracing::info!(target: "Main",
        "[FLOW] mpv handle initialized in {} ms", mpv_started.elapsed().as_millis());

    if !jfn_playback::ingest_driver::jfn_playback_observe_mpv_properties(backend_byte) {
        tracing::error!(target: "Main", "observe_mpv_properties failed");
        return 1;
    }

    // force-window=yes keeps VO creation on mpv's core thread. The user's
    // mpv.conf color is only known after mpv_initialize parsed the config, so
    // the capture is async: the reply lands in the boot pump, which writes the
    // override and gates boot readiness on it.
    jfn_mpv::api::jfn_mpv_request_background_color();

    // Upscaling preset. Queued after the background-color read and before any
    // file is loaded; mpv holds the shader chain as plain options, so it
    // applies to the first frame of the first video.
    jfn_mpv::video_mode::boot(opts.video_mode);

    // input-default-bindings=no drops the builtin CLOSE_WIN -> quit binding;
    // the WM close button needs it back.
    install_mpv_close_binding();

    wake_mpv_on_window_change();

    let boot_args = BootArgs {
        disable_gpu_compositing: opts.disable_gpu_compositing,
        remote_debugging_port: opts.remote_debugging_port,
    };

    // CEF's process bring-up needs nothing mpv owns; where the platform
    // allows it, it runs while the core thread builds the VO and its GPU
    // context instead of after.
    if plat().cef_init_precedes_mpv_window() && !ensure_cef_initialized(&boot_args) {
        return 1;
    }

    if !wait_for_vo_window() {
        return 0;
    }

    // Two sync reads; same main-thread hazard as the device profile.
    let _ = mpv_read_off_main(log_mpv_versions);

    let rc = unsafe { run_with_cef(&boot_args, instance) };
    if rc != 0 {
        return rc;
    }

    // macOS must run TerminateDestroy off the main thread (mpv's VO uninit
    // does DispatchQueue.main.sync); run_blocking keeps main pumping.
    plat().run_blocking(Box::new(jfn_mpv::boot::jfn_mpv_handle_terminate));

    plat().post_window_cleanup();

    0
}

// =====================================================================
// mpv boot helpers + VO wait loop
// =====================================================================

const LOG_MPV: u8 = 1;
const LEVEL_TRACE: u8 = 0;
const LEVEL_DEBUG: u8 = 1;
const LEVEL_INFO: u8 = 2;
const LEVEL_WARN: u8 = 3;
const LEVEL_ERROR: u8 = 4;

fn mpv_log_level_from_filter() -> &'static str {
    let e = jfn_logging::log_enabled;
    if e(LOG_MPV, LEVEL_TRACE) {
        "debug"
    } else if e(LOG_MPV, LEVEL_DEBUG) {
        "v"
    } else if e(LOG_MPV, LEVEL_INFO) {
        "info"
    } else if e(LOG_MPV, LEVEL_WARN) {
        "warn"
    } else if e(LOG_MPV, LEVEL_ERROR) {
        "error"
    } else {
        "no"
    }
}

/// What one drained libmpv event means for the boot wait.
enum BootEvent {
    /// The queue was empty, or the parked wait timed out.
    Idle,
    /// mpv is going away before its window came up.
    Fatal,
    /// Folded into boot state.
    Consumed,
}

/// Log messages reach tracing, the background-color reply applies the startup
/// override, every other event reaches the ingest layer.
fn consume_boot_event(event: jfn_mpv::api::WaitEvent) -> BootEvent {
    match event {
        jfn_mpv::api::WaitEvent::None => BootEvent::Idle,
        jfn_mpv::api::WaitEvent::LogMessage(m) => {
            jfn_mpv::forward_log_to_tracing(&m);
            BootEvent::Consumed
        }
        jfn_mpv::api::WaitEvent::Event(jfn_mpv::Event::Shutdown | jfn_mpv::Event::EndFile(_)) => {
            BootEvent::Fatal
        }
        jfn_mpv::api::WaitEvent::Event(jfn_mpv::Event::GetPropertyReply {
            reply: jfn_mpv::api::BACKGROUND_COLOR_REPLY,
            ref value,
            ..
        }) => {
            apply_startup_background(value);
            BootEvent::Consumed
        }
        // The video-mode baseline reads land during the VO wait; the ingest
        // thread that normally consumes them does not exist yet.
        jfn_mpv::api::WaitEvent::Event(jfn_mpv::Event::GetPropertyReply {
            reply,
            ref value,
            ..
        }) if jfn_mpv::video_mode::consume_reply(reply, value) => BootEvent::Consumed,
        jfn_mpv::api::WaitEvent::Event(event) => {
            let scale_raw = plat().get_scale();
            let scale = if scale_raw > 0.0 { scale_raw } else { 1.0 };
            jfn_playback::ingest_driver::jfn_playback_ingest_mpv_event_owned(
                &event,
                scale,
                plat().mpv_host().logical_content_size(),
            );
            BootEvent::Consumed
        }
    }
}

/// Stores the user's color for the theme rotator, then writes
/// [`STARTUP_BG_HEX`] in its place. Latches [`STARTUP_BG_APPLIED`] even when
/// the reply carried no value.
fn apply_startup_background(value: &jfn_mpv::PropertyValue) {
    if let Some(user_bg) = jfn_mpv::api::background_color_from_reply(value) {
        video_bg_set(user_bg);
        tracing::info!(target: "Main", "video bg captured: #{user_bg:06x}");
    }
    let startup_bg = cs(STARTUP_BG_HEX);
    unsafe { jfn_mpv::api::jfn_mpv_set_background_color_hex(startup_bg.as_ptr()) };
    STARTUP_BG_APPLIED.store(true, std::sync::atomic::Ordering::Release);
}

/// Ready once the window authority reports an extent, the host's own startup
/// gate is open, and the startup background override has landed. The window's
/// mode is never a boot precondition: where mpv owns the toplevel the
/// `window-maximized` report is an echo of the boot option, and where a host
/// owns the toplevel the WM/compositor may decline the maximize outright.
fn boot_ready() -> bool {
    crate::window_geometry::controller()
        .source()
        .snapshot()
        .extent
        .is_some()
        && plat().mpv_host().host_ready()
        && STARTUP_BG_APPLIED.load(std::sync::atomic::Ordering::Acquire)
}

// =====================================================================
// run_with_cef body — Rust port
// =====================================================================

const LOG_CEF: u8 = 2;
// cef_log_severity_t ABI: 1 VERBOSE, 2 INFO, 3 WARNING, 4 ERROR.
// Must match `jfn_cef::ffi::log_severity_from_int` / `client/events.rs`.
const LOG_SEVERITY_VERBOSE: c_int = 1;
const LOG_SEVERITY_INFO: c_int = 2;
const LOG_SEVERITY_WARNING: c_int = 3;
const LOG_SEVERITY_ERROR: c_int = 4;

fn cef_severity_for_cef_filter() -> c_int {
    // Map LOG_CEF level to CEF severity:
    //   Trace/Debug -> VERBOSE, Info -> INFO, Warn -> WARNING, Error -> ERROR.
    let e = jfn_logging::log_enabled;
    if e(LOG_CEF, LEVEL_TRACE) || e(LOG_CEF, LEVEL_DEBUG) {
        LOG_SEVERITY_VERBOSE
    } else if e(LOG_CEF, LEVEL_INFO) {
        LOG_SEVERITY_INFO
    } else if e(LOG_CEF, LEVEL_WARN) {
        LOG_SEVERITY_WARNING
    } else {
        LOG_SEVERITY_ERROR
    }
}

// Handler thunks installed via jfn_playback_set_*_handler. They capture
// nothing (Rust function items are 'static) and forward to the platform
// backend / jfn-cef.

extern "C" fn h_idle_inhibit(level: u32) {
    let lvl = match level {
        1 => IdleInhibitLevel::System,
        2 => IdleInhibitLevel::Display,
        _ => IdleInhibitLevel::None,
    };
    plat().set_idle_inhibit(lvl);
}
extern "C" fn h_theme_video_mode(active: bool) {
    jfn_color::theme::jfn_theme_color_set_video_mode(active);
}
extern "C" fn h_web_exec_js(js: *const c_char) {
    if !js.is_null() {
        unsafe { jfn_cef::business_web::jfn_web_exec_js(js) };
    }
}
extern "C" fn h_browsers_set_refresh_rate(hz: f64) {
    tracing::info!(target: "Main", "Display refresh rate changed: {hz} Hz");
    jfn_cef::browsers::jfn_browsers_set_refresh_rate(hz);
}
extern "C" fn h_theme_set_titlebar(rgb: u32) {
    plat().set_theme_color(rgb);
}
extern "C" fn h_theme_set_mpv_bg(hex: *const c_char) {
    unsafe { jfn_mpv::api::jfn_mpv_set_background_color_hex(hex) };
}

fn h_shutdown_wake_manager() {
    // Runs inline on whichever thread called jfn_shutdown_initiate (signal
    // handler, CEF dispatch, input thread, …). Signal-only by contract: just
    // wake the manager, which orchestrates the close/drain off-thread. Never
    // close a browser or wake the main loop here — that would reenter CEF or
    // race the drain.
    crate::manager::jfn_manager_notify_shutdown();
}

/// Owns the run_with_cef body — invoked once by `jfn_app_main`.
unsafe fn run_with_cef(ba: &BootArgs, instance: &Instance) -> c_int {
    // 2. Platform init (PlatformScope). Cleanup happens in shutdown_runtime.
    let mpv_raw = jfn_mpv::boot::jfn_mpv_handle_get();
    let platform_ok = plat().init(mpv_raw as *mut std::ffi::c_void);
    if !platform_ok {
        tracing::error!(target: "Main", "Platform init failed");
        return 1;
    }
    tracing::info!(target: "Main", "Platform init ok");
    PLATFORM_INITED.store(true, std::sync::atomic::Ordering::Release);

    // 3. Apply titlebar theme color before CefInitialize so the window doesn't
    //    sit with the system default palette during init.
    if jfn_config::titlebar_theme_color() {
        plat().set_theme_color(0x101010);
    }

    // 4. Build device profile. Its demuxer read is a sync mpv_get_property,
    //    which must not run on the main thread here: see mpv_read_off_main.
    publish_device_profile(mpv_raw);

    // 5. CEF init flags + initialise.
    if !ensure_cef_initialized(ba) {
        return 1;
    }

    let hz = boot_mpv_reconcile(mpv_raw);

    let (manager_thread, main_layer) = init_main_browser(hz, shared_textures());

    if !start_playback_coordination(instance) {
        return 1;
    }

    // 14. Wait for the main browser to finish loading. Skipped when the
    //     platform pumps CEF itself (external pump on the main thread):
    //     blocking main here would starve the pump and never load.
    if plat().cef_host().is_none() {
        unsafe { jfn_cef::client::jfn_cef_layer_wait_for_load(main_layer) };
    }
    tracing::info!(target: "Main", "Main browser loaded");

    tracing::info!(target: "Main", "[FLOW] Running — about to enter run_main_loop");

    // 15. Park the main thread until the manager has closed + drained every
    //     browser, at which point it calls plat().wake_main_loop() to release
    //     us. Unified across platforms: macOS parks in [NSApp run] (whose
    //     pump runs the posted close + OnBeforeClose while the manager waits);
    //     other platforms park on the Condvar main-park. Exit is driven by the
    //     shutdown signal (routed through the manager), never by transient
    //     browser-close state when the overlay resets the main layer.
    plat().run_main_loop();
    tracing::info!(target: "Main", "[FLOW] run_main_loop returned — browsers drained, running teardown");

    shutdown_runtime(manager_thread);

    0
}

static PLATFORM_INITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static CEF_INITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static COORD_INITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The URL the main web layer starts on. `settings.json` is hand-editable and
/// the legacy-profile import copies it verbatim, so the saved server URL is
/// gated the same way the connect overlay gates a typed one: only http(s)
/// may load into the layer that carries the native bridge. Anything else
/// starts on `about:blank`, which shows the connect overlay.
fn startup_server_url(saved: String) -> String {
    if saved.is_empty() || jfn_jellyfin::is_http_url(&saved) {
        saved
    } else {
        tracing::warn!(
            target: "Main",
            "ignoring saved server URL with a non-http(s) scheme: {saved:?}"
        );
        String::new()
    }
}

#[cfg(test)]
mod startup_server_url_tests {
    use super::startup_server_url;

    #[test]
    fn startup_server_url_keeps_http_and_https() {
        assert_eq!(
            startup_server_url("https://jf.example.com/".into()),
            "https://jf.example.com/"
        );
        assert_eq!(
            startup_server_url("http://10.0.0.5:8096".into()),
            "http://10.0.0.5:8096"
        );
    }

    #[test]
    fn startup_server_url_keeps_empty() {
        assert_eq!(startup_server_url(String::new()), "");
    }

    #[test]
    fn startup_server_url_drops_other_schemes() {
        for bad in [
            "file:///C:/x.html",
            "app://resources/about.html",
            "chrome://gpu",
            "javascript:1",
            "//host",
        ] {
            assert_eq!(startup_server_url(bad.into()), "", "{bad}");
        }
    }
}

#[cfg(test)]
mod resolve_startup_options_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::cli::{ENV_BACKED, ENV_LOCK, ENV_LOG_FILE, ENV_LOG_LEVEL};
    use jfn_mpv::VideoMode;

    /// Holds the crate-wide env lock and restores every `ASTROFIN_*` variable
    /// clap reads, so a variable set here — or one that happens to be set in
    /// the developer's shell — can never leak into another test.
    struct EnvScope {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvScope {
        fn new() -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let saved = ENV_BACKED
                .iter()
                .map(|&key| {
                    let previous = std::env::var(key).ok();
                    unsafe { std::env::remove_var(key) };
                    (key, previous)
                })
                .collect();
            EnvScope { _lock: lock, saved }
        }

        fn set(&self, key: &str, value: &str) {
            unsafe { std::env::set_var(key, value) };
        }
    }

    impl Drop for EnvScope {
        fn drop(&mut self) {
            for (key, previous) in &self.saved {
                match previous {
                    Some(value) => unsafe { std::env::set_var(key, value) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    fn parse_cli(args: &[&str]) -> cli::Cli {
        cli::Cli::try_parse_from(args.iter().copied()).expect("argv parses")
    }

    /// A profile that has never chosen anything: every setting at its default.
    fn unset() -> SavedOptions {
        SavedOptions::default()
    }

    fn resolve(saved: SavedOptions, args: &[&str]) -> StartupOptions {
        resolve_from_saved(saved, &parse_cli(args))
    }

    // ---- hwdec -------------------------------------------------------------

    #[test]
    fn hwdec_falls_back_to_the_mpv_default_when_nothing_is_configured() {
        let _env = EnvScope::new();
        assert_eq!(
            resolve(unset(), &["astrofin"]).hwdec,
            jfn_mpv::HWDEC_DEFAULT
        );
    }

    #[test]
    fn hwdec_comes_from_settings_when_no_flag_is_given() {
        let _env = EnvScope::new();
        let saved = SavedOptions {
            hwdec: "auto".into(),
            ..unset()
        };
        assert_eq!(resolve(saved, &["astrofin"]).hwdec, "auto");
    }

    #[test]
    fn the_hwdec_flag_wins_over_the_settings_value() {
        let _env = EnvScope::new();
        let saved = SavedOptions {
            hwdec: "auto".into(),
            ..unset()
        };
        assert_eq!(resolve(saved, &["astrofin", "--hwdec", "no"]).hwdec, "no");
    }

    #[test]
    fn an_unknown_hwdec_is_replaced_by_the_mpv_default() {
        let _env = EnvScope::new();
        // From a hand-edited settings.json …
        let saved = SavedOptions {
            hwdec: "not-a-decoder".into(),
            ..unset()
        };
        assert_eq!(resolve(saved, &["astrofin"]).hwdec, jfn_mpv::HWDEC_DEFAULT);
        // … and from the command line, which clap does not validate either.
        assert_eq!(
            resolve(unset(), &["astrofin", "--hwdec=not-a-decoder"]).hwdec,
            jfn_mpv::HWDEC_DEFAULT
        );
    }

    // ---- video mode --------------------------------------------------------

    #[test]
    fn the_video_mode_defaults_to_auto() {
        let _env = EnvScope::new();
        assert_eq!(resolve(unset(), &["astrofin"]).video_mode, VideoMode::Auto);
    }

    #[test]
    fn the_video_mode_comes_from_settings_when_no_flag_is_given() {
        let _env = EnvScope::new();
        let saved = SavedOptions {
            video_mode: VideoMode::Animation,
            ..unset()
        };
        assert_eq!(
            resolve(saved, &["astrofin"]).video_mode,
            VideoMode::Animation
        );
    }

    #[test]
    fn the_video_mode_flag_wins_over_the_settings_value() {
        let _env = EnvScope::new();
        let saved = SavedOptions {
            video_mode: VideoMode::Animation,
            ..unset()
        };
        assert_eq!(
            resolve(saved, &["astrofin", "--video-mode", "off"]).video_mode,
            VideoMode::Off
        );
    }

    #[test]
    fn an_unparseable_video_mode_flag_keeps_the_stored_mode() {
        let _env = EnvScope::new();
        for flag in ["--video-mode=nonsense", "--video-mode="] {
            let saved = SavedOptions {
                video_mode: VideoMode::Animation,
                ..unset()
            };
            assert_eq!(
                resolve(saved, &["astrofin", flag]).video_mode,
                VideoMode::Animation,
                "{flag}"
            );
        }
    }

    #[test]
    fn the_video_mode_flag_still_accepts_the_pre_rename_spellings() {
        let _env = EnvScope::new();
        for (flag, expected) in [
            ("movies", VideoMode::LiveAction),
            ("anime", VideoMode::Animation),
        ] {
            assert_eq!(
                resolve(unset(), &["astrofin", "--video-mode", flag]).video_mode,
                expected,
                "{flag}"
            );
        }
    }

    // ---- audio passthrough -------------------------------------------------

    #[test]
    fn audio_passthrough_is_empty_when_nothing_is_configured() {
        let _env = EnvScope::new();
        assert_eq!(resolve(unset(), &["astrofin"]).audio_passthrough, "");
    }

    #[test]
    fn audio_passthrough_comes_from_settings_when_no_flag_is_given() {
        let _env = EnvScope::new();
        let saved = SavedOptions {
            audio_passthrough: "ac3,eac3".into(),
            ..unset()
        };
        assert_eq!(resolve(saved, &["astrofin"]).audio_passthrough, "ac3,eac3");
    }

    #[test]
    fn the_audio_passthrough_flag_wins_over_the_settings_value() {
        let _env = EnvScope::new();
        let saved = SavedOptions {
            audio_passthrough: "ac3,eac3".into(),
            ..unset()
        };
        assert_eq!(
            resolve(saved, &["astrofin", "--audio-passthrough", "truehd"]).audio_passthrough,
            "truehd"
        );
    }

    #[test]
    fn dts_hd_drops_bare_dts_whichever_layer_the_list_came_from() {
        let _env = EnvScope::new();
        let saved = SavedOptions {
            audio_passthrough: "ac3,dts,dts-hd".into(),
            ..unset()
        };
        assert_eq!(
            resolve(saved, &["astrofin"]).audio_passthrough,
            "ac3,dts-hd"
        );
        assert_eq!(
            resolve(unset(), &["astrofin", "--audio-passthrough=dts,dts-hd"]).audio_passthrough,
            "dts-hd"
        );
    }

    #[test]
    fn normalize_passthrough_leaves_a_list_without_dts_hd_alone() {
        assert_eq!(normalize_passthrough("ac3,dts,eac3"), "ac3,dts,eac3");
        assert_eq!(normalize_passthrough(""), "");
    }

    #[test]
    fn normalize_passthrough_drops_only_the_bare_dts_entry() {
        assert_eq!(normalize_passthrough("dts-hd"), "dts-hd");
        assert_eq!(normalize_passthrough("dts,dts-hd"), "dts-hd");
        assert_eq!(
            normalize_passthrough("ac3,dts,dts-hd,truehd"),
            "ac3,dts-hd,truehd"
        );
    }

    // ---- audio channels ----------------------------------------------------

    #[test]
    fn audio_channels_are_empty_when_nothing_is_configured() {
        let _env = EnvScope::new();
        assert_eq!(resolve(unset(), &["astrofin"]).audio_channels, "");
    }

    #[test]
    fn audio_channels_come_from_settings_when_no_flag_is_given() {
        let _env = EnvScope::new();
        let saved = SavedOptions {
            audio_channels: "5.1".into(),
            ..unset()
        };
        assert_eq!(resolve(saved, &["astrofin"]).audio_channels, "5.1");
    }

    #[test]
    fn the_audio_channels_flag_wins_over_the_settings_value() {
        let _env = EnvScope::new();
        let saved = SavedOptions {
            audio_channels: "5.1".into(),
            ..unset()
        };
        assert_eq!(
            resolve(saved, &["astrofin", "--audio-channels", "stereo"]).audio_channels,
            "stereo"
        );
    }

    // ---- exclusive audio ---------------------------------------------------

    #[test]
    fn exclusive_audio_is_off_unless_something_asks_for_it() {
        let _env = EnvScope::new();
        assert!(!resolve(unset(), &["astrofin"]).audio_exclusive);
    }

    #[test]
    fn exclusive_audio_can_be_turned_on_by_settings_or_by_the_flag() {
        let _env = EnvScope::new();
        let saved = SavedOptions {
            audio_exclusive: true,
            ..unset()
        };
        assert!(resolve(saved, &["astrofin"]).audio_exclusive);
        assert!(resolve(unset(), &["astrofin", "--audio-exclusive"]).audio_exclusive);
    }

    #[test]
    fn no_flag_can_turn_exclusive_audio_back_off() {
        let _env = EnvScope::new();
        // The flag is a set-only switch, so a stored `true` survives every argv.
        let saved = SavedOptions {
            audio_exclusive: true,
            ..unset()
        };
        assert!(resolve(saved, &["astrofin", "--hwdec=auto"]).audio_exclusive);
    }

    // ---- log level (flag > variable > settings) ----------------------------

    #[test]
    fn an_unset_log_level_is_left_empty_for_the_logging_default() {
        let _env = EnvScope::new();
        assert_eq!(resolve(unset(), &["astrofin"]).log_level, "");
        // init_logging is what turns the empty string into a filter.
        assert_eq!(DEFAULT_LOG_FILTER, "info");
    }

    #[test]
    fn the_log_level_comes_from_settings_when_no_flag_or_variable_is_set() {
        let _env = EnvScope::new();
        let saved = SavedOptions {
            log_level: "warn".into(),
            ..unset()
        };
        assert_eq!(resolve(saved, &["astrofin"]).log_level, "warn");
    }

    #[test]
    fn the_log_level_variable_wins_over_the_settings_value() {
        let env = EnvScope::new();
        env.set(ENV_LOG_LEVEL, "debug");
        let saved = SavedOptions {
            log_level: "warn".into(),
            ..unset()
        };
        assert_eq!(resolve(saved, &["astrofin"]).log_level, "debug");
    }

    #[test]
    fn the_log_level_flag_wins_over_the_variable_and_the_settings_value() {
        let env = EnvScope::new();
        env.set(ENV_LOG_LEVEL, "debug");
        let saved = SavedOptions {
            log_level: "warn".into(),
            ..unset()
        };
        assert_eq!(
            resolve(saved, &["astrofin", "--log-level", "trace"]).log_level,
            "trace"
        );
    }

    #[test]
    fn an_empty_log_level_variable_still_overrides_the_settings_value() {
        let env = EnvScope::new();
        env.set(ENV_LOG_LEVEL, "");
        let saved = SavedOptions {
            log_level: "warn".into(),
            ..unset()
        };
        // Empty is not "absent": it reaches init_logging, which resolves it to
        // DEFAULT_LOG_FILTER rather than to the saved setting.
        assert_eq!(resolve(saved, &["astrofin"]).log_level, "");
    }

    // ---- log file (flag > variable, no settings key) -----------------------

    #[test]
    fn the_log_file_is_unset_when_neither_flag_nor_variable_is_given() {
        let _env = EnvScope::new();
        assert_eq!(resolve(unset(), &["astrofin"]).log_file, None);
    }

    #[test]
    fn the_log_file_variable_is_used_and_the_flag_wins_over_it() {
        let env = EnvScope::new();
        env.set(ENV_LOG_FILE, "from-env.log");
        assert_eq!(
            resolve(unset(), &["astrofin"]).log_file.as_deref(),
            Some("from-env.log")
        );
        assert_eq!(
            resolve(unset(), &["astrofin", "--log-file", "from-flag.log"])
                .log_file
                .as_deref(),
            Some("from-flag.log")
        );
    }

    #[test]
    fn an_explicitly_empty_log_file_stays_empty_rather_than_unset() {
        let _env = EnvScope::new();
        // `--log-file=` is how file logging is turned off; it must not read as
        // "no flag given" and fall back to the default log path.
        assert_eq!(
            resolve(unset(), &["astrofin", "--log-file="])
                .log_file
                .as_deref(),
            Some("")
        );
    }

    // ---- CEF flags ---------------------------------------------------------

    #[test]
    fn gpu_compositing_is_only_disabled_by_the_flag() {
        let _env = EnvScope::new();
        // No settings key is consulted here: the flag is the whole input.
        assert!(!resolve(unset(), &["astrofin"]).disable_gpu_compositing);
        assert!(
            resolve(unset(), &["astrofin", "--disable-gpu-compositing"]).disable_gpu_compositing
        );
    }

    #[test]
    fn the_remote_debugging_port_is_zero_unless_the_flag_gives_one() {
        let _env = EnvScope::new();
        assert_eq!(resolve(unset(), &["astrofin"]).remote_debugging_port, 0);
        assert_eq!(
            resolve(unset(), &["astrofin", "--remote-debug-port", "9222"]).remote_debugging_port,
            9222
        );
    }

    // ---- everything at once ------------------------------------------------

    #[test]
    fn a_full_command_line_overrides_every_stored_setting() {
        let env = EnvScope::new();
        env.set(ENV_LOG_LEVEL, "from-env");
        env.set(ENV_LOG_FILE, "from-env.log");
        let saved = SavedOptions {
            hwdec: "auto".into(),
            video_mode: VideoMode::Animation,
            audio_passthrough: "ac3".into(),
            audio_channels: "5.1".into(),
            log_level: "warn".into(),
            audio_exclusive: false,
        };
        let opts = resolve(
            saved,
            &[
                "astrofin",
                "--hwdec=no",
                "--video-mode=live-action",
                "--audio-passthrough=truehd",
                "--audio-channels=stereo",
                "--audio-exclusive",
                "--log-level=trace",
                "--log-file=run.log",
                "--disable-gpu-compositing",
                "--remote-debug-port=9222",
            ],
        );
        assert_eq!(opts.hwdec, "no");
        assert_eq!(opts.video_mode, VideoMode::LiveAction);
        assert_eq!(opts.audio_passthrough, "truehd");
        assert_eq!(opts.audio_channels, "stereo");
        assert!(opts.audio_exclusive);
        assert_eq!(opts.log_level, "trace");
        assert_eq!(opts.log_file.as_deref(), Some("run.log"));
        assert!(opts.disable_gpu_compositing);
        assert_eq!(opts.remote_debugging_port, 9222);
    }
}
