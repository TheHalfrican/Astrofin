//! `Platform` trait + global handle held by `jfn_app_main`.
//!
//! Each backend crate (`jfn-wayland`, `jfn-x11`, `jfn-macos`, `jfn-windows`)
//! returns a concrete type implementing this trait via its
//! `make_*_platform()` factory. The binary installs the chosen backend into
//! the [`OnceLock`] below via [`install`] / [`get`].
//!
//! `JfnRect` stays `#[repr(C)]` because CEF's `OnAcceleratedPaint` accel-paint
//! info hands it across the C ABI surface; the popup request and other
//! payloads are plain Rust.

#![allow(non_snake_case)]

use parking_lot::{Condvar, Mutex};
use std::ffi::{c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub mod cef_host;
pub mod file_dialog;
pub mod geometry;
pub mod instance;
pub mod media_sink;
pub mod menu;
pub mod mpv_host;
pub mod osr_popup;
#[cfg_attr(unix, path = "process_unix.rs")]
#[cfg_attr(not(unix), path = "process_other.rs")]
mod process;
#[cfg(unix)]
mod shutdown_logic;
#[cfg_attr(unix, path = "signal_unix.rs")]
#[cfg_attr(not(unix), path = "signal_other.rs")]
mod signal;
pub mod window_source;

pub use cef_host::CefHost;
pub use file_dialog::{FileDialogFilter, FileDialogKind, FileDialogRequest};
pub use geometry::{
    BootGeometry, LogicalPoint, LogicalSize, PhysicalPoint, PhysicalSize, Scale, SurfaceSize,
    WindowExtent, WindowGeometry, WindowPos,
};
pub use instance::{Instance, InstanceId};
pub use jfn_gpu_paint::DamageRect as JfnRect;
pub use media_sink::MediaSink;
pub use menu::{
    Generation, MENU_DISMISSED, MenuClose, MenuDelivery, MenuHost, MenuItem, MenuKind, MenuMetrics,
    MenuPaint, MenuPlacement, MenuRequest, MenuScript, MenuSelection, PopupSurface, menu_delivery,
    menu_has_selectable, menu_initial_row, menu_scripts,
};
pub use mpv_host::{DefaultMpvHost, MpvHost, VO_WAIT_TICK};
pub use osr_popup::{NoOsrPopup, OsrPopupSurface};
pub use window_source::{
    WindowSnapshot, WindowSource, notify_window_changed, subscribe_window_changed,
};

/// Preserves the process's SIGINT/SIGTERM dispositions across a scope.
///
/// `chrome/browser/chrome_browser_main_posix.cc` installs SIGINT/SIGTERM
/// handlers during `CefInitialize`, and that path is NOT gated by
/// `disable_signal_handlers`. Snapshot the caller's handlers on
/// construction and restore them on drop, confining Chromium's installs to
/// the guarded window. No-op off unix.
pub use signal::SignalGuard;

// =====================================================================
// Main-thread park (non-macOS default for run_main_loop/wake_main_loop)
// =====================================================================
//
// Non-macOS backends have no native run loop to block the process main
// thread on. The default `Platform::run_main_loop` parks here until the
// shutdown manager calls `wake_main_loop`, at which point main runs the
// teardown tail. A latching `bool` + `Condvar` is enough — it's a single
// dedicated blocking wait (not a `poll()` multiplexer), so no fd is needed
// and there's no `playback`-crate dependency. macOS overrides both methods
// (`[NSApp run]` / stop-NSApp) and never touches this.

struct MainPark {
    woken: Mutex<bool>,
    cv: Condvar,
}

static MAIN_PARK: MainPark = MainPark {
    woken: Mutex::new(false),
    cv: Condvar::new(),
};

/// Block until [`main_park_signal`] is called. Returns immediately if the
/// signal already fired (latched), so a wake racing ahead of the wait is
/// not lost.
pub fn main_park_wait() {
    let mut woken = MAIN_PARK.woken.lock();
    while !*woken {
        MAIN_PARK.cv.wait(&mut woken);
    }
}

/// Release [`main_park_wait`]. Idempotent and safe from any thread.
pub fn main_park_signal() {
    *MAIN_PARK.woken.lock() = true;
    MAIN_PARK.cv.notify_all();
}

/// Fixed `cef_cursor_type_t` shapes. `CT_CUSTOM` (a bitmap) and the `CT_DND_*`
/// cursors are excluded — listing them here would map a non-fixed cursor to a
/// fixed shape.
pub mod cursor {
    use cef::sys::cef_cursor_type_t as ct;

    macro_rules! cursor_shape {
        ($($variant:ident = $ct:ident),* $(,)?) => {
            #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
            #[repr(i32)]
            pub enum CursorShape {
                $($variant = ct::$ct as i32,)*
            }

            impl CursorShape {
                pub fn from_cef(raw: i32) -> Option<Self> {
                    $(if raw == ct::$ct as i32 { return Some(Self::$variant); })*
                    None
                }

                pub const fn as_raw(self) -> i32 {
                    self as i32
                }
            }
        };
    }

    cursor_shape! {
        Pointer = CT_POINTER,
        Cross = CT_CROSS,
        Hand = CT_HAND,
        IBeam = CT_IBEAM,
        Wait = CT_WAIT,
        Help = CT_HELP,
        EastResize = CT_EASTRESIZE,
        NorthResize = CT_NORTHRESIZE,
        NorthEastResize = CT_NORTHEASTRESIZE,
        NorthWestResize = CT_NORTHWESTRESIZE,
        SouthResize = CT_SOUTHRESIZE,
        SouthEastResize = CT_SOUTHEASTRESIZE,
        SouthWestResize = CT_SOUTHWESTRESIZE,
        WestResize = CT_WESTRESIZE,
        NorthSouthResize = CT_NORTHSOUTHRESIZE,
        EastWestResize = CT_EASTWESTRESIZE,
        NorthEastSouthWestResize = CT_NORTHEASTSOUTHWESTRESIZE,
        NorthWestSouthEastResize = CT_NORTHWESTSOUTHEASTRESIZE,
        ColumnResize = CT_COLUMNRESIZE,
        RowResize = CT_ROWRESIZE,
        MiddlePanning = CT_MIDDLEPANNING,
        EastPanning = CT_EASTPANNING,
        NorthPanning = CT_NORTHPANNING,
        NorthEastPanning = CT_NORTHEASTPANNING,
        NorthWestPanning = CT_NORTHWESTPANNING,
        SouthPanning = CT_SOUTHPANNING,
        SouthEastPanning = CT_SOUTHEASTPANNING,
        SouthWestPanning = CT_SOUTHWESTPANNING,
        WestPanning = CT_WESTPANNING,
        Move = CT_MOVE,
        VerticalText = CT_VERTICALTEXT,
        Cell = CT_CELL,
        ContextMenu = CT_CONTEXTMENU,
        Alias = CT_ALIAS,
        Progress = CT_PROGRESS,
        NoDrop = CT_NODROP,
        Copy = CT_COPY,
        None = CT_NONE,
        NotAllowed = CT_NOTALLOWED,
        ZoomIn = CT_ZOOMIN,
        ZoomOut = CT_ZOOMOUT,
        Grab = CT_GRAB,
        Grabbing = CT_GRABBING,
        MiddlePanningVertical = CT_MIDDLE_PANNING_VERTICAL,
        MiddlePanningHorizontal = CT_MIDDLE_PANNING_HORIZONTAL,
    }
}

/// Canonical `cef_event_flags_t` modifier bits — the single source of truth
/// for the CEF `EVENTFLAG_*` masks that flow through key/mouse dispatch.
/// Derived from the generated CEF bindings (a newtype with associated
/// constants) so backends import these instead of hand-copying bit shifts
/// that can silently drift. Typed `u32` to match the dispatch ABI.
pub mod event_flags {
    use cef::sys::cef_event_flags_t as ef;

    macro_rules! flag_consts {
        ($($name:ident),* $(,)?) => {
            $(pub const $name: u32 = ef::$name.0 as u32;)*
        };
    }

    flag_consts! {
        EVENTFLAG_CAPS_LOCK_ON, EVENTFLAG_SHIFT_DOWN, EVENTFLAG_CONTROL_DOWN,
        EVENTFLAG_ALT_DOWN, EVENTFLAG_LEFT_MOUSE_BUTTON, EVENTFLAG_MIDDLE_MOUSE_BUTTON,
        EVENTFLAG_RIGHT_MOUSE_BUTTON, EVENTFLAG_COMMAND_DOWN, EVENTFLAG_NUM_LOCK_ON,
        EVENTFLAG_IS_KEY_PAD, EVENTFLAG_IS_LEFT, EVENTFLAG_IS_RIGHT, EVENTFLAG_ALTGR_DOWN,
        EVENTFLAG_IS_REPEAT, EVENTFLAG_PRECISION_SCROLLING_DELTA, EVENTFLAG_SCROLL_BY_PAGE,
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DisplayBackend {
    Wayland,
    X11,
    Windows,
    MacOS,
}

impl DisplayBackend {
    /// The modifier that means "application action" in keyboard shortcuts.
    pub fn action_modifier_flag(self) -> u32 {
        match self {
            DisplayBackend::MacOS => event_flags::EVENTFLAG_COMMAND_DOWN,
            _ => event_flags::EVENTFLAG_CONTROL_DOWN,
        }
    }

    /// Whether CEF's browser-process `MainArgs` carries the full argv for
    /// Chromium to parse. When false the caller hands CEF a clean
    /// `[argv[0]]` and pushes switches explicitly instead.
    pub fn cef_full_browser_argv(self) -> bool {
        matches!(self, DisplayBackend::Windows)
    }
}

/// Filesystem locations CEF needs written into `Settings` before
/// `CefInitialize`, resolved per platform. Each `None` field is left at
/// CEF's own default rather than cleared.
#[derive(Default)]
pub struct CefPaths {
    pub browser_subprocess_path: Option<PathBuf>,
    pub framework_dir_path: Option<PathBuf>,
    pub resources_dir_path: Option<PathBuf>,
    pub locales_dir_path: Option<PathBuf>,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum WindowDecorations {
    /// Client-side: the app draws its own titlebar in-page.
    Csd,
    Server,
    ServerThemed,
}

impl WindowDecorations {
    /// Wire/persistence contract: settings.json, the JS↔Rust IPC, and the web
    /// settings UI all speak these literals.
    pub fn as_str(self) -> &'static str {
        match self {
            WindowDecorations::Csd => "csd",
            WindowDecorations::Server => "server",
            WindowDecorations::ServerThemed => "serverThemed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "csd" => Some(WindowDecorations::Csd),
            "server" => Some(WindowDecorations::Server),
            "serverThemed" => Some(WindowDecorations::ServerThemed),
            _ => None,
        }
    }
}

/// The decoration mode in effect right now — the app-level projection of
/// whatever authority the platform has (on Wayland, the compositor's
/// xdg-decoration verdict). Two-valued and total: protocol lifecycle states
/// (no such protocol, verdict pending) never cross this boundary.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum EffectiveDecorations {
    /// The app draws its own titlebar.
    ClientSide,
    /// The OS/compositor owns decorations; the app draws none.
    ServerSide,
}

/// The set of decoration modes a backend can actually honor. CSD is a member
/// of every set — the app can always draw its own titlebar — so only the
/// server-side modes vary, and resolution against a set is total: anything
/// the set lacks falls back to CSD, which [`Self::contains`] always accepts.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct DecorationOptions {
    server: bool,
    server_themed: bool,
}

impl DecorationOptions {
    pub fn csd_only() -> Self {
        Self {
            server: false,
            server_themed: false,
        }
    }

    pub fn with_server(themed: bool) -> Self {
        Self {
            server: true,
            server_themed: themed,
        }
    }

    pub fn all() -> Self {
        Self::with_server(true)
    }

    pub fn contains(self, mode: WindowDecorations) -> bool {
        match mode {
            WindowDecorations::Csd => true,
            WindowDecorations::Server => self.server,
            WindowDecorations::ServerThemed => self.server_themed,
        }
    }

    /// Whether there is anything to choose — a CSD-only set leaves the user
    /// no decision, so the settings entry is hidden.
    pub fn has_choice(self) -> bool {
        self.server
    }

    pub fn iter(self) -> impl Iterator<Item = WindowDecorations> {
        [
            Some(WindowDecorations::Csd),
            self.server.then_some(WindowDecorations::Server),
            self.server_themed
                .then_some(WindowDecorations::ServerThemed),
        ]
        .into_iter()
        .flatten()
    }
}

/// One CEF paint callback's payload, decoded.
///
/// CEF has exactly two output paths — `OnAcceleratedPaint` and `OnPaint` —
/// selected once per browser by `shared_texture_enabled`.
pub enum PaintFrame<'a> {
    /// A texture the app owns. By value, because a backend that presents off
    /// the callback thread (X11 and Wayland both do) has to keep it.
    Accelerated(jfn_gpu_paint::SharedTexture),
    /// CPU pixels in BGRA, tightly packed, with the regions that changed.
    Software {
        size: PhysicalSize,
        pixels: &'a [u8],
        dirty: &'a [JfnRect],
    },
}

/// Idle-inhibit level.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum IdleInhibitLevel {
    None,
    System,
    Display,
}

/// Backend-allocated per-surface handle: an opaque, backend-defined id.
///
/// Callers hold it as a value and pass it back verbatim; no caller may
/// dereference it. Its representation is one machine word so it round-trips
/// losslessly through the CEF C++ layer's `void*` slot. Pointer-backed
/// backends (Wayland/Windows/macOS) bridge through [`SurfaceHandle::from_ptr`]
/// / [`SurfaceHandle::as_ptr`]; the X11 backend stores a generational id via
/// [`SurfaceHandle::from_id`] / [`SurfaceHandle::id`] and never treats it as a
/// pointer.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[repr(transparent)]
pub struct SurfaceHandle(*mut c_void);

// A word-sized opaque id; the wrapped pointer is never dereferenced by the ABI
// itself. Backends carry their own `unsafe impl Send` on the state they map it to.
unsafe impl Send for SurfaceHandle {}
unsafe impl Sync for SurfaceHandle {}

impl SurfaceHandle {
    /// The absent handle (allocation failed / no surface).
    pub const NONE: Self = Self(std::ptr::null_mut());

    #[must_use]
    pub fn is_none(self) -> bool {
        self.0.is_null()
    }

    /// Bridge for pointer-backed backends: wrap a backend surface pointer.
    #[must_use]
    pub fn from_ptr(p: *mut c_void) -> Self {
        Self(p)
    }

    /// Bridge for pointer-backed backends: recover the backend surface pointer.
    #[must_use]
    pub fn as_ptr(self) -> *mut c_void {
        self.0
    }

    /// Bridge for id-backed backends (X11): pack a generational id. The value
    /// is never dereferenced.
    #[must_use]
    pub fn from_id(id: u64) -> Self {
        Self(id as *mut c_void)
    }

    /// Bridge for id-backed backends (X11): recover the generational id.
    #[must_use]
    pub fn id(self) -> u64 {
        self.0 as u64
    }
}

/// Process-wide platform handle. Optional methods have no-op defaults so
/// backends only override what they care about.
///
/// All methods take `&self` — backends keep their own interior mutability
/// (`Mutex`, `AtomicBool`, etc) where they need it.
pub trait Platform: Send + Sync {
    fn display(&self) -> DisplayBackend;

    fn default_window_decorations(&self) -> WindowDecorations;

    /// Decoration modes this backend can honor. The default keeps every mode
    /// valid; backends with an authoritative availability source (Wayland
    /// derives it from the compositor's advertised protocols) narrow the set.
    fn window_decoration_options(&self) -> DecorationOptions {
        DecorationOptions::all()
    }

    fn resolve_window_decorations(
        &self,
        configured: Option<WindowDecorations>,
    ) -> WindowDecorations {
        let wanted = configured.unwrap_or_else(|| self.default_window_decorations());
        if self.window_decoration_options().contains(wanted) {
            wanted
        } else {
            WindowDecorations::Csd
        }
    }

    fn early_init(&self) {}
    /// `mpv` is the opaque libmpv `mpv_handle` — a raw C handle, stays raw.
    fn init(&self, mpv: *mut c_void) -> bool {
        let _ = mpv;
        true
    }
    fn cleanup(&self) {}
    fn post_window_cleanup(&self) {}

    // Per-surface
    fn alloc_surface(&self) -> SurfaceHandle {
        SurfaceHandle::NONE
    }
    fn free_surface(&self, _s: SurfaceHandle) {}
    fn surface_present(&self, _s: SurfaceHandle, _frame: PaintFrame<'_>) -> bool {
        false
    }
    fn surface_resize(&self, _s: SurfaceHandle, _size: SurfaceSize) {}
    fn surface_set_visible(&self, _s: SurfaceHandle, _visible: bool) {}
    fn restack(&self, _ordered: &[SurfaceHandle]) {}

    /// How this backend delivers `kind`; `Host` names the backend's own menu
    /// host.
    fn menu_delivery(&self, kind: MenuKind) -> MenuDelivery;

    fn osr_popup_surface(&self) -> &dyn OsrPopupSurface {
        &NoOsrPopup
    }

    /// How this platform hosts mpv's lifecycle (env prep, VO wait,
    /// teardown detach). Default: mpv needs nothing from the platform.
    fn mpv_host(&self) -> &dyn MpvHost {
        &DefaultMpvHost
    }

    /// `Some` when the platform drives CEF's message loop itself
    /// (external pump); `None` runs CEF's multi-threaded message loop.
    fn cef_host(&self) -> Option<&dyn CefHost> {
        None
    }

    /// OS media-session integration. Non-optional — every platform has a
    /// sink.
    fn media_session(&self) -> &dyn MediaSink;

    fn cef_paths(&self) -> CefPaths;

    // Fullscreen
    fn set_fullscreen(&self, _v: bool) {}
    fn toggle_fullscreen(&self) {}

    // Window controls for client-side decorations. Default no-ops cover
    // backends without CSD (X11 WMs / macOS / Windows draw their own).
    fn window_minimize(&self) {}
    fn window_toggle_maximize(&self) {}
    /// Begin an interactive, compositor-driven window move. Must be called in
    /// response to a pointer button press on the titlebar drag region.
    fn window_start_move(&self) {}
    /// Begin an interactive, compositor-driven resize from the given edge.
    /// `edge` uses xdg_toplevel resize-edge values (1=top, 2=bottom, 4=left,
    /// 8=right, corners are the ORs, e.g. 5=top-left).
    fn window_start_resize(&self, _edge: c_int) {}

    // Transition
    fn begin_transition(&self) {}
    fn end_transition(&self) {}
    fn in_transition(&self) -> bool {
        false
    }
    fn set_expected_size(&self, _w: c_int, _h: c_int) {}

    fn get_scale(&self) -> f32 {
        1.0
    }
    fn get_display_scale(&self, _x: c_int, _y: c_int) -> f32 {
        1.0
    }

    /// Scale used to convert physical window pixels to CEF logical size.
    /// Default trusts mpv's `display-hidpi-scale` when known; Wayland
    /// overrides to always use the compositor scale (mpv doesn't own the
    /// surface there, so its value isn't authoritative).
    fn effective_scale(&self, mpv_display_hidpi_scale: f64) -> f32 {
        if mpv_display_hidpi_scale > 0.0 {
            mpv_display_hidpi_scale as f32
        } else {
            self.get_scale()
        }
    }

    /// Seed the window owner with the restored boot geometry. Backends that
    /// own their toplevel (Wayland) size it here; mpv-backed backends rely on
    /// mpv's `--geometry` instead and keep the no-op default.
    fn apply_boot_geometry(&self, _g: &BootGeometry) {}

    /// Current window position, or `None` if it can't be determined.
    fn query_window_position(&self) -> Option<WindowPos> {
        None
    }

    /// Live window-geometry authority for this backend: the compositor-backed
    /// source where the backend owns its toplevel (Wayland), the mpv-backed
    /// source everywhere else.
    fn window_source(&self) -> &dyn WindowSource;

    /// The mpv `--geometry` string for boot, or `None` when the backend owns
    /// its toplevel and sizes it itself. The default sizes via mpv; toplevel-
    /// owning backends (Wayland) override to `None`.
    fn boot_mpv_geometry(&self, g: &BootGeometry) -> Option<String> {
        Some(g.mpv_geometry_string())
    }

    /// Physical size mpv should be resized to when the boot display scale
    /// differs from the saved scale, or `None` to leave sizing untouched. The
    /// default performs the mpv reconcile; `locked` (booting fullscreen or
    /// maximized) and toplevel-owning backends yield `None`.
    fn reconcile_mpv_size(
        &self,
        display_hidpi_scale: f64,
        saved_scale: f32,
        saved_logical: LogicalSize,
        locked: bool,
    ) -> Option<PhysicalSize> {
        if locked
            || display_hidpi_scale <= 0.0
            || saved_scale <= 0.0
            || (display_hidpi_scale - f64::from(saved_scale)).abs() < 0.01
        {
            return None;
        }
        Some(saved_logical.to_physical(Scale(display_hidpi_scale as f32)))
    }

    /// Clamp saved geometry to stay on-screen. Backends that don't constrain
    /// geometry return `g` unchanged (the default).
    fn clamp_window_geometry(&self, g: WindowGeometry) -> WindowGeometry {
        g
    }

    fn pump(&self) {}
    /// Block the process main thread until [`wake_main_loop`] is called.
    /// Default parks on the process-wide [`main_park_wait`]; macOS overrides
    /// with `[NSApp run]`.
    fn run_main_loop(&self) {
        main_park_wait();
    }
    /// Release [`run_main_loop`] so main can run the teardown tail. Safe from
    /// any thread. Default signals [`main_park_signal`]; macOS overrides to
    /// stop the NSApp loop.
    fn wake_main_loop(&self) {
        main_park_signal();
    }

    fn set_cursor(&self, _shape: cursor::CursorShape) {}
    fn set_idle_inhibit(&self, _level: IdleInhibitLevel) {}
    fn set_theme_color(&self, _rgb: u32) {}

    /// Whether the window-decorations setting (client-side vs server-side
    /// titlebar) applies on this platform. Gates the settings UI entry; the
    /// entry's option list comes from [`Platform::window_decoration_options`].
    fn window_decorations_supported(&self) -> bool {
        false
    }
    /// The decoration mode currently in effect. Defaults to ServerSide — on
    /// macOS/Windows/X11 the OS/WM draws the titlebar, so the app never
    /// does. Changes are announced via [`notify_decorations_changed`].
    fn effective_decorations(&self) -> EffectiveDecorations {
        EffectiveDecorations::ServerSide
    }

    fn shared_texture_supported(&self) -> bool {
        true
    }

    /// True where `CefInitialize` depends on neither platform init nor a run
    /// loop the boot wait owns, so CEF's process bring-up may run while mpv's
    /// core thread creates the VO. False on Wayland and X11 (platform init
    /// resolves shared-texture support) and on macOS (external pump).
    fn cef_init_precedes_mpv_window(&self) -> bool {
        false
    }
    /// Set during init by Wayland backend (dmabuf probe) when GPU lacks the
    /// shared-texture path.
    fn set_shared_texture_unsupported(&self) {}

    /// Whether [`Platform::clipboard_read_text`] will actually invoke the
    /// backend clipboard. Wayland clears this in `wl_init` when no data
    /// device manager is present; the menu Paste path uses it to decide
    /// between native OS read vs CEF `frame.Paste()`.
    fn clipboard_text_supported(&self) -> bool {
        true
    }

    /// Read the clipboard's plain text, handing it to `on_done`.
    ///
    /// `on_done` runs exactly once, but *when* is the backend's business and no
    /// caller may assume either way: macOS (`NSPasteboard`), Windows
    /// (`GetClipboardData`), X11 (no backend read at all) and this default all
    /// invoke it inline on the calling thread, because those reads are
    /// synchronous and cheap; Wayland alone defers it to its clipboard worker,
    /// which owns the `wl_data_offer` pipe. Hence the borrowed `&str` — a
    /// callback that wants to keep the text owns a copy of it — and hence the
    /// plain name: it used to be `clipboard_read_text_async`, promising an
    /// asynchrony three of the four backends never had.
    fn clipboard_read_text(&self, on_done: Box<dyn FnOnce(&str) + Send>) {
        // No backend support — invoke with empty text, inline.
        on_done("");
    }
    /// Disable subsequent clipboard reads (set by Wayland when no data
    /// device manager is available).
    fn clear_clipboard_handler(&self) {}

    fn open_external_url(&self, _url: &str) {}

    /// Open a filesystem path in the OS file manager.
    fn open_path(&self, _path: &Path) {}

    /// Open a native file chooser for `req`.
    ///
    /// `true` means the backend took the request and will invoke
    /// `req.on_done` exactly once (with `None` for cancel or failure).
    /// `false` means "no native chooser here": the request is dropped
    /// without `on_done` ever running, and the caller must resolve its own
    /// side. The default returns `false`.
    ///
    /// Windows (Common Item Dialog) and macOS (NSOpenPanel / NSSavePanel as a
    /// sheet) implement this. Linux keeps the default on purpose — CEF's own
    /// chooser cannot run for a windowless browser, so the caller cancels the
    /// dialog instead of crashing; a native chooser there is follow-up work.
    fn open_file_dialog(&self, req: FileDialogRequest) -> bool {
        let _ = req;
        false
    }

    /// Run `f` to completion without deadlocking work that needs the
    /// main thread (e.g. mpv's VO uninit doing `DispatchQueue.main.sync`).
    /// Default runs `f` inline; macOS runs it on a side thread while main
    /// pumps its run loop.
    fn run_blocking(&self, f: Box<dyn FnOnce() + Send>) {
        f();
    }

    /// `on_shutdown` must be async-signal-safe.
    fn install_shutdown_handler(&self, on_shutdown: fn()) {
        process::install_shutdown(on_shutdown);
    }
}

// =====================================================================
// Process-wide handle
// =====================================================================

// `OnceLock<Box<dyn Platform>>` doesn't give us a stable `'static` reference
// shape that's ergonomic for the existing `unsafe extern "C"` thunks below;
// store a raw fat pointer instead. Set exactly once during boot.
static PLATFORM: OnceLock<&'static dyn Platform> = OnceLock::new();

/// Install the platform backend. Must be called exactly once during boot,
/// before any other code dispatches through [`get`]. Panics if called
/// twice — there is no "swap backend at runtime" path.
#[allow(clippy::expect_used)] // boot invariant: install exactly once
pub fn install(p: Box<dyn Platform>) {
    let leaked: &'static dyn Platform = Box::leak(p);
    PLATFORM
        .set(leaked)
        .map_err(|_| ())
        .expect("install() called twice");
}

/// Returns the installed platform backend. Panics if [`install`] hasn't
/// been called yet — every call site is post-boot.
#[allow(clippy::expect_used)] // every call site is post-boot
pub fn get() -> &'static dyn Platform {
    *PLATFORM
        .get()
        .expect("jfn_platform_abi::get() called before install()")
}

/// Like [`get`] but returns `None` before install. Used by jfn_cef's
/// `OnConsoleMessage` and similar paths that may fire during early CEF
/// helper-process boot when no platform is installed.
pub fn try_get() -> Option<&'static dyn Platform> {
    PLATFORM.get().copied()
}

// =====================================================================
// Browser bridge
// =====================================================================
//
// Lets crates that can't depend on jfn_cef (input, macos) forward events
// to whichever CEF layer is currently active. jfn_cef installs the impl
// at boot; the trait methods resolve the active layer internally so
// callers never see a JfnCefLayer pointer.

pub trait BrowserBridge: Send + Sync {
    #[allow(clippy::too_many_arguments)] // mirrors CEF's KeyEvent layout 1:1
    fn send_key_event(
        &self,
        type_: c_int,
        modifiers: u32,
        windows_key_code: c_int,
        native_key_code: c_int,
        is_system_key: bool,
        character: u16,
        unmodified_character: u16,
    );
    fn send_mouse_click(
        &self,
        x: c_int,
        y: c_int,
        modifiers: u32,
        button: c_int,
        mouse_up: bool,
        click_count: c_int,
    );
    fn send_mouse_move(&self, x: i32, y: i32, modifiers: u32, leave: bool);
    fn send_mouse_wheel(&self, x: c_int, y: c_int, modifiers: u32, delta_x: c_int, delta_y: c_int);
    fn set_focus(&self, focus: bool);
    fn navigate_history(&self, forward: bool);
    fn undo(&self);
    fn redo(&self);
    fn cut(&self);
    fn copy(&self);
    fn paste(&self);
    fn select_all(&self);
    /// True if a layer is currently active. Cheap check used by callers
    /// that want to early-out before building an event payload.
    fn has_active(&self) -> bool;
}

static BROWSER_BRIDGE: OnceLock<&'static dyn BrowserBridge> = OnceLock::new();

#[allow(clippy::expect_used)] // boot invariant: install exactly once
pub fn install_browser_bridge(b: Box<dyn BrowserBridge>) {
    let leaked: &'static dyn BrowserBridge = Box::leak(b);
    BROWSER_BRIDGE
        .set(leaked)
        .map_err(|_| ())
        .expect("install_browser_bridge called twice");
}

pub fn browser_bridge() -> Option<&'static dyn BrowserBridge> {
    BROWSER_BRIDGE.get().copied()
}

static DECORATIONS_LISTENER: OnceLock<fn()> = OnceLock::new();

/// Register the callback fired when [`Platform::effective_decorations`]
/// changes. Single listener, installed once alongside the browser bridge.
pub fn set_decorations_listener(f: fn()) {
    let _ = DECORATIONS_LISTENER.set(f);
}

pub fn notify_decorations_changed() {
    if let Some(f) = DECORATIONS_LISTENER.get() {
        f();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Once};
    use std::time::Duration;

    use super::*;

    // -----------------------------------------------------------------
    // A backend that implements only the mandatory methods, so every
    // other assertion below is about the trait's own defaults.
    // -----------------------------------------------------------------

    pub(crate) struct FakeMediaSink;

    impl MediaSink for FakeMediaSink {
        fn start(&self, _instance: &Instance) {}
        fn stop(&self) {}
    }

    pub(crate) struct FakeWindowSource;

    impl WindowSource for FakeWindowSource {
        fn snapshot(&self) -> WindowSnapshot {
            WindowSnapshot {
                extent: None,
                position: None,
                maximized: false,
                fullscreen: false,
            }
        }
    }

    #[derive(Default)]
    pub(crate) struct FakePlatform {
        options: Option<DecorationOptions>,
        default_decorations: Option<WindowDecorations>,
    }

    impl FakePlatform {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        fn with(options: DecorationOptions, default_decorations: WindowDecorations) -> Self {
            Self {
                options: Some(options),
                default_decorations: Some(default_decorations),
            }
        }
    }

    impl Platform for FakePlatform {
        fn display(&self) -> DisplayBackend {
            DisplayBackend::Windows
        }

        fn default_window_decorations(&self) -> WindowDecorations {
            self.default_decorations
                .unwrap_or(WindowDecorations::Server)
        }

        fn window_decoration_options(&self) -> DecorationOptions {
            self.options.unwrap_or_else(DecorationOptions::all)
        }

        fn menu_delivery(&self, kind: MenuKind) -> MenuDelivery {
            match kind {
                MenuKind::Dropdown => MenuDelivery::Page,
                MenuKind::ContextMenu => MenuDelivery::Composited,
            }
        }

        fn media_session(&self) -> &dyn MediaSink {
            &FakeMediaSink
        }

        fn cef_paths(&self) -> CefPaths {
            CefPaths::default()
        }

        fn window_source(&self) -> &dyn WindowSource {
            &FakeWindowSource
        }
    }

    /// The platform handle installs exactly once per process, so the whole
    /// test binary shares one install and no test depends on running first.
    pub(crate) fn installed_platform() -> &'static dyn Platform {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| install(Box::new(FakePlatform::new())));
        get()
    }

    // -----------------------------------------------------------------
    // Global handles
    // -----------------------------------------------------------------

    #[test]
    fn an_installed_backend_is_visible_through_get_and_try_get() {
        let installed = installed_platform();
        assert_eq!(installed.display(), DisplayBackend::Windows);
        let seen = try_get().expect("try_get after install");
        assert_eq!(seen.display(), DisplayBackend::Windows);
        assert!(std::ptr::addr_eq(
            std::ptr::from_ref(installed),
            std::ptr::from_ref(seen),
        ));
    }

    #[test]
    #[should_panic(expected = "install() called twice")]
    fn installing_a_second_backend_panics() {
        installed_platform();
        install(Box::new(FakePlatform::new()));
    }

    struct SilentBridge;

    impl BrowserBridge for SilentBridge {
        fn send_key_event(
            &self,
            _type_: c_int,
            _modifiers: u32,
            _windows_key_code: c_int,
            _native_key_code: c_int,
            _is_system_key: bool,
            _character: u16,
            _unmodified_character: u16,
        ) {
        }
        fn send_mouse_click(
            &self,
            _x: c_int,
            _y: c_int,
            _modifiers: u32,
            _button: c_int,
            _mouse_up: bool,
            _click_count: c_int,
        ) {
        }
        fn send_mouse_move(&self, _x: i32, _y: i32, _modifiers: u32, _leave: bool) {}
        fn send_mouse_wheel(
            &self,
            _x: c_int,
            _y: c_int,
            _modifiers: u32,
            _delta_x: c_int,
            _delta_y: c_int,
        ) {
        }
        fn set_focus(&self, _focus: bool) {}
        fn navigate_history(&self, _forward: bool) {}
        fn undo(&self) {}
        fn redo(&self) {}
        fn cut(&self) {}
        fn copy(&self) {}
        fn paste(&self) {}
        fn select_all(&self) {}
        fn has_active(&self) -> bool {
            true
        }
    }

    fn installed_bridge() -> &'static dyn BrowserBridge {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| install_browser_bridge(Box::new(SilentBridge)));
        browser_bridge().expect("bridge after install")
    }

    #[test]
    fn an_installed_browser_bridge_is_visible_through_browser_bridge() {
        assert!(installed_bridge().has_active());
    }

    #[test]
    #[should_panic(expected = "install_browser_bridge called twice")]
    fn installing_a_second_browser_bridge_panics() {
        installed_bridge();
        install_browser_bridge(Box::new(SilentBridge));
    }

    static LISTENER_HITS: AtomicUsize = AtomicUsize::new(0);
    static OTHER_LISTENER_HITS: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn the_first_registered_decorations_listener_is_the_one_notified() {
        // Single test for the whole listener slot: it is a process-wide
        // OnceLock, so a second test could not observe a fresh one.
        set_decorations_listener(|| {
            LISTENER_HITS.fetch_add(1, Ordering::Relaxed);
        });
        notify_decorations_changed();
        assert_eq!(LISTENER_HITS.load(Ordering::Relaxed), 1);

        // A later registration is dropped rather than replacing the first.
        set_decorations_listener(|| {
            OTHER_LISTENER_HITS.fetch_add(1, Ordering::Relaxed);
        });
        notify_decorations_changed();
        assert_eq!(LISTENER_HITS.load(Ordering::Relaxed), 2);
        assert_eq!(OTHER_LISTENER_HITS.load(Ordering::Relaxed), 0);
    }

    // -----------------------------------------------------------------
    // Main-thread park
    // -----------------------------------------------------------------

    #[test]
    fn the_main_park_releases_every_waiter_once_signaled() {
        let waiter = std::thread::spawn(main_park_wait);
        main_park_signal();
        waiter.join().expect("parked thread");
        // Latched: a wait entered after the signal returns immediately, and
        // the platform default routes through the same latch.
        main_park_wait();
        installed_platform().run_main_loop();
    }

    #[test]
    fn waking_the_main_loop_is_idempotent() {
        let p = FakePlatform::new();
        p.wake_main_loop();
        p.wake_main_loop();
        main_park_signal();
        p.run_main_loop();
    }

    // -----------------------------------------------------------------
    // Value types
    // -----------------------------------------------------------------

    #[test]
    fn a_fixed_cursor_shape_round_trips_through_its_cef_value() {
        let shape = cursor::CursorShape::Hand;
        assert_eq!(cursor::CursorShape::from_cef(shape.as_raw()), Some(shape));
        for shape in [
            cursor::CursorShape::Pointer,
            cursor::CursorShape::IBeam,
            cursor::CursorShape::NotAllowed,
            cursor::CursorShape::Grabbing,
        ] {
            assert_eq!(cursor::CursorShape::from_cef(shape.as_raw()), Some(shape));
        }
    }

    #[test]
    fn a_cursor_value_outside_the_fixed_shapes_is_rejected() {
        assert_eq!(cursor::CursorShape::from_cef(-1), None);
        assert_eq!(cursor::CursorShape::from_cef(i32::MAX), None);
    }

    #[test]
    fn macos_uses_command_as_its_action_modifier_and_the_rest_use_control() {
        assert_eq!(
            DisplayBackend::MacOS.action_modifier_flag(),
            event_flags::EVENTFLAG_COMMAND_DOWN
        );
        for backend in [
            DisplayBackend::Windows,
            DisplayBackend::Wayland,
            DisplayBackend::X11,
        ] {
            assert_eq!(
                backend.action_modifier_flag(),
                event_flags::EVENTFLAG_CONTROL_DOWN
            );
        }
    }

    #[test]
    fn only_windows_hands_cef_the_full_browser_argv() {
        assert!(DisplayBackend::Windows.cef_full_browser_argv());
        for backend in [
            DisplayBackend::MacOS,
            DisplayBackend::Wayland,
            DisplayBackend::X11,
        ] {
            assert!(!backend.cef_full_browser_argv());
        }
    }

    #[test]
    fn every_decoration_mode_round_trips_through_its_wire_string() {
        for mode in [
            WindowDecorations::Csd,
            WindowDecorations::Server,
            WindowDecorations::ServerThemed,
        ] {
            assert_eq!(WindowDecorations::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(WindowDecorations::Csd.as_str(), "csd");
        assert_eq!(WindowDecorations::Server.as_str(), "server");
        assert_eq!(WindowDecorations::ServerThemed.as_str(), "serverThemed");
    }

    #[test]
    fn an_unknown_decoration_string_does_not_parse() {
        for raw in ["", "CSD", "serverthemed", "none"] {
            assert_eq!(WindowDecorations::parse(raw), None);
        }
    }

    #[test]
    fn csd_is_a_member_of_every_decoration_option_set() {
        for options in [
            DecorationOptions::csd_only(),
            DecorationOptions::with_server(false),
            DecorationOptions::with_server(true),
            DecorationOptions::all(),
        ] {
            assert!(options.contains(WindowDecorations::Csd));
        }
    }

    #[test]
    fn a_decoration_set_contains_only_the_server_modes_it_advertises() {
        let csd = DecorationOptions::csd_only();
        assert!(!csd.contains(WindowDecorations::Server));
        assert!(!csd.contains(WindowDecorations::ServerThemed));
        assert!(!csd.has_choice());
        assert_eq!(csd.iter().collect::<Vec<_>>(), vec![WindowDecorations::Csd]);

        let plain = DecorationOptions::with_server(false);
        assert!(plain.contains(WindowDecorations::Server));
        assert!(!plain.contains(WindowDecorations::ServerThemed));
        assert!(plain.has_choice());
        assert_eq!(
            plain.iter().collect::<Vec<_>>(),
            vec![WindowDecorations::Csd, WindowDecorations::Server]
        );

        let all = DecorationOptions::all();
        assert!(all.contains(WindowDecorations::ServerThemed));
        assert!(all.has_choice());
        assert_eq!(
            all.iter().collect::<Vec<_>>(),
            vec![
                WindowDecorations::Csd,
                WindowDecorations::Server,
                WindowDecorations::ServerThemed
            ]
        );
    }

    #[test]
    fn a_surface_handle_carries_a_pointer_or_an_id_unchanged() {
        assert!(SurfaceHandle::NONE.is_none());
        assert!(SurfaceHandle::from_ptr(std::ptr::null_mut()).is_none());
        assert!(SurfaceHandle::from_id(0).is_none());

        let mut backing = 7u32;
        let ptr: *mut c_void = std::ptr::from_mut(&mut backing).cast();
        let handle = SurfaceHandle::from_ptr(ptr);
        assert!(!handle.is_none());
        assert_eq!(handle.as_ptr(), ptr);

        let id = SurfaceHandle::from_id(42);
        assert!(!id.is_none());
        assert_eq!(id.id(), 42);
    }

    // -----------------------------------------------------------------
    // Trait defaults
    // -----------------------------------------------------------------

    #[test]
    fn a_configured_decoration_mode_wins_when_the_backend_offers_it() {
        let p = FakePlatform::with(DecorationOptions::all(), WindowDecorations::Server);
        assert_eq!(
            p.resolve_window_decorations(Some(WindowDecorations::ServerThemed)),
            WindowDecorations::ServerThemed
        );
    }

    #[test]
    fn an_unconfigured_decoration_mode_falls_back_to_the_backend_default() {
        let p = FakePlatform::with(DecorationOptions::all(), WindowDecorations::Server);
        assert_eq!(
            p.resolve_window_decorations(None),
            WindowDecorations::Server
        );
    }

    #[test]
    fn a_decoration_mode_the_backend_cannot_honor_falls_back_to_csd() {
        let p = FakePlatform::with(DecorationOptions::csd_only(), WindowDecorations::Server);
        // Both the configured mode and the backend's own default are outside
        // the option set, so every path lands on CSD.
        assert_eq!(
            p.resolve_window_decorations(Some(WindowDecorations::ServerThemed)),
            WindowDecorations::Csd
        );
        assert_eq!(p.resolve_window_decorations(None), WindowDecorations::Csd);
    }

    #[test]
    fn the_effective_scale_trusts_mpv_when_it_reports_one() {
        let p = FakePlatform::new();
        assert!((p.effective_scale(1.5) - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn an_unknown_mpv_scale_falls_back_to_the_platform_scale() {
        let p = FakePlatform::new();
        for unknown in [0.0, -2.0] {
            assert!((p.effective_scale(unknown) - p.get_scale()).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn mpv_is_resized_only_when_the_boot_scale_differs_from_the_saved_one() {
        let p = FakePlatform::new();
        let saved = LogicalSize { w: 1280, h: 720 };
        assert_eq!(
            p.reconcile_mpv_size(2.0, 1.0, saved, false),
            Some(PhysicalSize { w: 2560, h: 1440 })
        );
    }

    #[test]
    fn a_locked_or_unknown_or_unchanged_scale_leaves_mpv_sizing_alone() {
        let p = FakePlatform::new();
        let saved = LogicalSize { w: 1280, h: 720 };
        assert_eq!(p.reconcile_mpv_size(2.0, 1.0, saved, true), None);
        assert_eq!(p.reconcile_mpv_size(0.0, 1.0, saved, false), None);
        assert_eq!(p.reconcile_mpv_size(2.0, 0.0, saved, false), None);
        assert_eq!(p.reconcile_mpv_size(1.0, 1.0, saved, false), None);
        // Under the 0.01 tolerance the difference is noise, not a rescale.
        assert_eq!(p.reconcile_mpv_size(1.005, 1.0, saved, false), None);
    }

    #[test]
    fn the_default_backend_boots_mpv_with_the_saved_geometry_string() {
        let p = FakePlatform::new();
        let boot = BootGeometry::from_clamped(
            LogicalSize { w: 1280, h: 720 },
            Scale(1.0),
            WindowGeometry::from_raw(1280, 720, 20, 30),
            false,
        );
        assert_eq!(
            p.boot_mpv_geometry(&boot).as_deref(),
            Some("1280x720+20+30")
        );
        // ... and applies it without touching the window itself.
        p.apply_boot_geometry(&boot);
        assert_eq!(p.query_window_position(), None);
    }

    #[test]
    fn the_default_backend_does_not_clamp_geometry() {
        let p = FakePlatform::new();
        let g = WindowGeometry::from_raw(4000, 3000, -1, -1);
        assert_eq!(p.clamp_window_geometry(g), g);
    }

    #[test]
    fn a_backend_without_a_native_chooser_declines_the_file_dialog() {
        let p = FakePlatform::new();
        let called = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&called);
        let taken = p.open_file_dialog(FileDialogRequest {
            kind: FileDialogKind::OpenFile,
            title: None,
            default_path: None,
            filters: Vec::new(),
            on_done: Box::new(move |_| flag.store(true, Ordering::Relaxed)),
        });
        assert!(!taken);
        assert!(
            !called.load(Ordering::Relaxed),
            "a declined request must not resolve"
        );
    }

    #[test]
    fn a_backend_without_a_clipboard_reads_empty_text_synchronously() {
        let p = FakePlatform::new();
        assert!(p.clipboard_text_supported());
        let seen = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&seen);
        p.clipboard_read_text(Box::new(move |text| {
            *sink.lock() = Some(text.to_string());
        }));
        assert_eq!(seen.lock().as_deref(), Some(""));
        p.clear_clipboard_handler();
    }

    #[test]
    fn the_default_run_blocking_runs_the_closure_inline() {
        let p = FakePlatform::new();
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        p.run_blocking(Box::new(move || flag.store(true, Ordering::Relaxed)));
        assert!(done.load(Ordering::Relaxed));
    }

    #[test]
    fn a_backend_that_overrides_nothing_owns_no_surfaces() {
        let p = FakePlatform::new();
        let s = p.alloc_surface();
        assert!(s.is_none());
        assert!(!p.surface_present(
            s,
            PaintFrame::Software {
                size: PhysicalSize { w: 1, h: 1 },
                pixels: &[0, 0, 0, 0],
                dirty: &[],
            }
        ));
        // Every remaining surface hook is a no-op.
        p.surface_resize(
            s,
            SurfaceSize {
                logical_w: 1,
                logical_h: 1,
                physical_w: 1,
                physical_h: 1,
            },
        );
        p.surface_set_visible(s, true);
        p.restack(&[s]);
        p.free_surface(s);
    }

    #[test]
    fn the_trait_defaults_describe_a_windowed_backend_with_no_extras() {
        let p = FakePlatform::new();
        assert!(p.init(std::ptr::null_mut()));
        assert!(!p.in_transition());
        assert!((p.get_scale() - 1.0).abs() < f32::EPSILON);
        assert!((p.get_display_scale(100, 100) - 1.0).abs() < f32::EPSILON);
        assert!(!p.window_decorations_supported());
        assert_eq!(p.effective_decorations(), EffectiveDecorations::ServerSide);
        assert!(p.shared_texture_supported());
        assert!(!p.cef_init_precedes_mpv_window());
        assert!(p.cef_host().is_none());
        // The no-op hooks must stay callable on a bare backend.
        p.early_init();
        p.begin_transition();
        p.end_transition();
        p.set_expected_size(800, 600);
        p.set_fullscreen(true);
        p.toggle_fullscreen();
        p.window_minimize();
        p.window_toggle_maximize();
        p.window_start_move();
        p.window_start_resize(1);
        p.set_cursor(cursor::CursorShape::Hand);
        p.set_idle_inhibit(IdleInhibitLevel::Display);
        p.set_theme_color(0x00_FF_00);
        p.set_shared_texture_unsupported();
        p.open_external_url("https://example.invalid/");
        p.open_path(Path::new("."));
        p.pump();
        p.cleanup();
        p.post_window_cleanup();
    }

    #[test]
    fn the_default_mpv_host_needs_nothing_from_the_platform() {
        let p = FakePlatform::new();
        let host = p.mpv_host();
        assert!(host.host_ready());
        assert_eq!(host.embed_wid(), None);
        assert_eq!(host.logical_content_size(), None);
        host.prepare(Some(WindowDecorations::Csd));
        host.ensure_host_window();
        host.detach();
    }

    #[test]
    fn the_default_vo_wait_pumps_with_the_tick_budget_until_the_pump_is_done() {
        let p = FakePlatform::new();
        let mut budgets: Vec<Duration> = Vec::new();
        p.mpv_host().run_vo_wait(&mut |budget| {
            budgets.push(budget);
            budgets.len() < 3
        });
        assert_eq!(budgets, vec![VO_WAIT_TICK; 3]);
    }

    #[test]
    fn a_backend_without_an_osr_popup_surface_ignores_every_popup_call() {
        let p = FakePlatform::new();
        let popup = p.osr_popup_surface();
        let s = SurfaceHandle::NONE;
        popup.show(s, 0, 0, 10, 10);
        popup.present(
            s,
            PaintFrame::Software {
                size: PhysicalSize { w: 1, h: 1 },
                pixels: &[0, 0, 0, 0],
                dirty: &[],
            },
            10,
            10,
        );
        popup.hide(s);
    }

    #[test]
    fn the_default_cef_paths_leave_every_location_at_cefs_own_default() {
        let paths = FakePlatform::new().cef_paths();
        assert!(paths.browser_subprocess_path.is_none());
        assert!(paths.framework_dir_path.is_none());
        assert!(paths.resources_dir_path.is_none());
        assert!(paths.locales_dir_path.is_none());
    }
}
