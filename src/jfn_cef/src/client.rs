//! CefLayer state.
//!
//! Holds the small bits of CefLayer state plus the resize-debounce and the
//! per-layer CEF browser ops dispatch that schedules `WasResized`,
//! `NotifyScreenInfoChanged`, `Invalidate`, `SetWindowlessFrameRate`,
//! `SendExternalBeginFrame`, and `ExecuteJavaScript` calls on TID_UI.
//!
//! Lifetime model: the FFI handle is `Box<JfnCefLayer>` (raw pointer owned
//! by the caller). Internal state lives in an `Arc<Inner>` so posted CEF
//! tasks can keep a clone alive past `jfn_cef_layer_free`. CefLayer
//! destructor clears `cef_ops` first, so any in-flight task that does
//! eventually run sees `None` and exits.

use cef::{Browser, RunContextMenuCallback};
use crossbeam_utils::atomic::AtomicCell;
use parking_lot::{Condvar, Mutex};
use std::os::raw::{c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicPtr, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crate::ipc::BrowserMessage;
use crate::platform_ops;
use crate::sink_routing::Handle;

use crate::paint_scheduler::{PaintMode, PaintScheduler};

mod accel;
mod browser_ops;
mod callbacks;
mod events;
mod ffi;
mod lifecycle;
mod paint;
mod popup;
mod resize;
mod tasks;
pub(crate) use ffi::*;
pub use ffi::{jfn_cef_layer_create, jfn_cef_layer_wait_for_load};
pub(crate) use tasks::{
    jfn_cef_post_close_and_collect, jfn_cef_post_csd_state_all, jfn_cef_post_set_hidden_all,
};

// A word-sized handle must never fall back to AtomicCell's global-lock
// path: it is read on every paint.
const _: () = assert!(AtomicCell::<platform_ops::SurfaceHandle>::is_lock_free());

const STATE_NORMAL: i32 = 0;
const STATE_PENDING_RESET: i32 = 1;
const STATE_RECREATING: i32 = 2;

#[repr(C)]
pub struct JfnCefLayer {
    pub(crate) inner: Arc<Inner>,
}

// Process-wide defaults set once at startup by Browsers ctor; consumed by
// Inner::do_create_browser when building WindowInfo + BrowserSettings.
static DEFAULT_FRAME_RATE: AtomicI32 = AtomicI32::new(60);
static PAINT_MODE: OnceLock<PaintMode> = OnceLock::new();

pub(crate) struct Inner {
    // identity / state queries (slice 1)
    name: Mutex<String>,
    closed: AtomicBool,
    loaded: AtomicBool,
    close_mtx: Mutex<()>,
    close_cv: Condvar,
    load_mtx: Mutex<()>,
    load_cv: Condvar,

    // Stored cef::Browser captured at LifeSpanHandler::on_after_created.
    // All CEF host/frame ops on TID_UI route through this; dropped on
    // OnBeforeClose.
    browser: Mutex<Option<Browser>>,
    // Pending RunContextMenuCallback — held while a context menu is open.
    pending_menu_callback: Mutex<Option<RunContextMenuCallback>>,
    // Injection-profile kind ("web" / "overlay" / "about") — looked up at
    // browser-create time to build the extra_info DictionaryValue.
    injection_kind: Mutex<String>,
    // Opaque per-layer surface handle (PlatformSurface*); passed back to the
    // C++ platform vtable for surface_resize / present / popup.
    surface: AtomicCell<platform_ops::SurfaceHandle>,

    // logical/physical dims (slice 3)
    width: AtomicI32,
    height: AtomicI32,
    physical_w: AtomicI32,
    physical_h: AtomicI32,

    paint_scheduler: PaintScheduler,

    // frame rate (slice 3): configured and last applied
    pub(crate) frame_rate: AtomicI32,
    current_frame_rate: AtomicI32,

    // resize-debounce (slice 3)
    resize_scheduled: AtomicBool,
    last_was_resized_ns: AtomicI64,

    // popup state (slice 4). Owned 1:1 with the platform surface; each
    // CefLayer owns its popup on the platform side. Two-phase reveal: rect
    // arrives via OnPopupSize, options via the "popupOptions" renderer IPC;
    // try_show_popup fires when popup_visible + size_received + options_received.
    popup: Mutex<PopupState>,
    dropdown: jfn_platform_abi::MenuDelivery,

    // lifecycle / reset state machine (slice 5)
    state: AtomicI32,
    pending_url: Mutex<String>,
    has_browser: AtomicBool,
    pending_internal_reset: AtomicBool,

    // app-level callback slots, stored as boxed closures.
    message_handler: Mutex<Option<Box<MessageFn>>>,
    created_callback: Mutex<Option<Box<CreatedFn>>>,
    before_close_callback: Mutex<Option<Box<BeforeCloseFn>>>,
    context_menu_builder: Mutex<Option<Box<ContextBuilderFn>>>,
    context_menu_dispatcher: Mutex<Option<Box<ContextDispatcherFn>>>,

    // Back-pointer to the owning `Box<JfnCefLayer>` raw handle. Set once
    // after `Box::into_raw` in `jfn_cef_layer_new`; in
    // `handle_on_before_close` we `swap` to null and act on the prior value.
    // `AtomicPtr` (not `OnceLock`) because the null sentinel after swap is
    // load-bearing — it guarantees the auto-remove + log fires exactly
    // once even if `OnBeforeClose` were ever re-entered, and prevents a
    // double-free of the registry entry.
    layer_ptr: AtomicPtr<JfnCefLayer>,

    cursor_handle: OnceLock<Handle>,
}

// Typed closure signatures stored in each callback slot. `*mut c_void` args
// stay raw because callers may want to receive cef-rs handles or C++
// CefRefPtr objects depending on which side installed the handler.
pub(crate) type MessageFn = dyn Fn(BrowserMessage) -> bool + Send + Sync;
pub type CreatedFn = dyn Fn(*mut c_void) + Send + Sync;
pub type BeforeCloseFn = dyn Fn() + Send + Sync;
pub type ContextBuilderFn = dyn Fn(*mut c_void) + Send + Sync;
pub type ContextDispatcherFn = dyn Fn(c_int) -> bool + Send + Sync;

#[derive(Default)]
struct PopupState {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    visible: bool,
    options: Vec<String>,
    selected_idx: i32,
    // Option indices an arrow key can land on (disabled/optgroup-disabled
    // excluded). Used to drive CEF's own popup to the chosen row.
    selectable: Vec<i32>,
    // Bottom-left corner of the <select> element in view coordinates.
    anchor: Option<(i32, i32)>,
    size_received: bool,
    options_received: bool,
}

// SAFETY: `Inner` is not auto-Send/Sync only because of the CEF ref-counted
// handles it stores (`Browser`, `RunContextMenuCallback`); those live behind
// `Inner`'s own mutexes and CEF ref-counts them atomically.
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

impl Inner {
    fn new() -> Arc<Self> {
        let paint_scheduler = PAINT_MODE
            .get_or_init(|| PaintMode::new(false))
            .make_scheduler();
        Arc::new(Self {
            name: Mutex::new(String::new()),
            closed: AtomicBool::new(false),
            loaded: AtomicBool::new(false),
            close_mtx: Mutex::new(()),
            close_cv: Condvar::new(),
            load_mtx: Mutex::new(()),
            load_cv: Condvar::new(),
            browser: Mutex::new(None),
            pending_menu_callback: Mutex::new(None),
            injection_kind: Mutex::new(String::new()),
            surface: AtomicCell::new(platform_ops::SurfaceHandle::NONE),
            width: AtomicI32::new(0),
            height: AtomicI32::new(0),
            physical_w: AtomicI32::new(0),
            physical_h: AtomicI32::new(0),
            paint_scheduler,
            frame_rate: AtomicI32::new(0),
            current_frame_rate: AtomicI32::new(0),
            resize_scheduled: AtomicBool::new(false),
            last_was_resized_ns: AtomicI64::new(0),
            popup: Mutex::new(PopupState {
                selected_idx: -1,
                ..PopupState::default()
            }),
            dropdown: jfn_platform_abi::menu_delivery(jfn_platform_abi::MenuKind::Dropdown),
            state: AtomicI32::new(STATE_NORMAL),
            pending_url: Mutex::new(String::new()),
            has_browser: AtomicBool::new(false),
            pending_internal_reset: AtomicBool::new(false),
            message_handler: Mutex::new(None),
            created_callback: Mutex::new(None),
            before_close_callback: Mutex::new(None),
            context_menu_builder: Mutex::new(None),
            context_menu_dispatcher: Mutex::new(None),
            layer_ptr: AtomicPtr::new(std::ptr::null_mut()),
            cursor_handle: OnceLock::new(),
        })
    }

    fn name_str(&self) -> String {
        self.name.lock().clone()
    }

    pub(crate) fn set_layer_ptr(&self, p: *mut JfnCefLayer) {
        self.layer_ptr.store(p, Ordering::Release);
    }

    /// Current raw layer ptr, or null after `handle_on_before_close` swap.
    /// Callbacks fired by `Inner` (created/before-close/etc.) read this to
    /// route to ptr-keyed APIs (e.g. `jfn_browsers_set_active`) without
    /// capturing the raw ptr into the closure.
    pub(crate) fn layer_ptr(&self) -> *mut JfnCefLayer {
        self.layer_ptr.load(Ordering::Acquire)
    }

    pub(crate) fn set_cursor_handle(&self, handle: Handle) {
        let _ = self.cursor_handle.set(handle);
    }

    pub(crate) fn cursor_handle(&self) -> Option<Handle> {
        self.cursor_handle.get().copied()
    }

    fn surface_handle(&self) -> platform_ops::SurfaceHandle {
        self.surface.load()
    }

    /// A detached `Inner` with no CEF browser attached, for the unit tests.
    /// Every browser op on `Inner` early-returns while `browser` is `None`,
    /// so such an instance exercises the pure state without a live CEF.
    #[cfg(test)]
    pub(crate) fn new_detached() -> Arc<Self> {
        crate::test_support::install_platform();
        Self::new()
    }
}

pub(crate) fn now_ns() -> i64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    Instant::now()
        .duration_since(*ORIGIN.get_or_init(Instant::now))
        .as_nanos() as i64
}

// ---------------------------------------------------------------------------
// Pub Rust API: in-process callers install closures directly. Pass `None`
// to clear; the previously installed closure is dropped.
// ---------------------------------------------------------------------------
impl JfnCefLayer {
    pub(crate) fn set_message_handler_rust(&self, f: Option<Box<MessageFn>>) {
        *self.inner.message_handler.lock() = f;
    }
    pub fn set_created_callback_rust(&self, f: Option<Box<CreatedFn>>) {
        *self.inner.created_callback.lock() = f;
    }
    pub fn set_before_close_callback_rust(&self, f: Option<Box<BeforeCloseFn>>) {
        *self.inner.before_close_callback.lock() = f;
    }
    pub fn set_context_menu_builder_rust(&self, f: Option<Box<ContextBuilderFn>>) {
        *self.inner.context_menu_builder.lock() = f;
    }
    pub fn set_context_menu_dispatcher_rust(&self, f: Option<Box<ContextDispatcherFn>>) {
        *self.inner.context_menu_dispatcher.lock() = f;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn layer() -> JfnCefLayer {
        JfnCefLayer {
            inner: Inner::new_detached(),
        }
    }

    #[test]
    fn a_fresh_inner_has_no_layer_pointer() {
        let inner = Inner::new_detached();
        assert!(inner.layer_ptr().is_null());
    }

    #[test]
    fn set_layer_ptr_is_read_back_by_layer_ptr() {
        let inner = Inner::new_detached();
        let ptr = 0x1234_usize as *mut JfnCefLayer;
        inner.set_layer_ptr(ptr);
        assert_eq!(inner.layer_ptr(), ptr);
        // The null sentinel after `handle_on_before_close`'s swap is
        // load-bearing, so storing null must be observable.
        inner.set_layer_ptr(std::ptr::null_mut());
        assert!(inner.layer_ptr().is_null());
    }

    #[test]
    fn cursor_handle_is_none_until_set() {
        let inner = Inner::new_detached();
        assert!(inner.cursor_handle().is_none());
    }

    struct DropSink;

    impl crate::sink_routing::Sink<()> for DropSink {
        fn emit(&mut self, _value: ()) {}
    }

    #[test]
    fn set_cursor_handle_keeps_the_first_handle_it_is_given() {
        let inner = Inner::new_detached();
        let mut router = crate::sink_routing::Router::<(), DropSink>::new(DropSink);
        let first = router.add_level();
        let second = router.add_level();
        inner.set_cursor_handle(first);
        // A layer is registered with the cursor router exactly once; a second
        // set must not silently re-key an already-routed layer.
        inner.set_cursor_handle(second);
        assert_eq!(inner.cursor_handle(), Some(first));
    }

    #[test]
    fn now_ns_is_monotonic_and_starts_near_zero() {
        let first = now_ns();
        let second = now_ns();
        assert!(first >= 0, "first reading was {first}");
        assert!(second >= first, "{second} went backwards from {first}");
        // The origin is captured on the first call, so a reading taken
        // immediately after cannot be a wall-clock epoch.
        assert!(second < 60_000_000_000, "second reading was {second}");
    }

    #[test]
    fn set_message_handler_rust_installs_and_clears_the_slot() {
        let l = layer();
        assert!(l.inner.message_handler.lock().is_none());
        l.set_message_handler_rust(Some(Box::new(|_msg| true)));
        assert!(l.inner.message_handler.lock().is_some());
        l.set_message_handler_rust(None);
        assert!(l.inner.message_handler.lock().is_none());
    }

    #[test]
    fn set_created_callback_rust_installs_the_closure_that_is_invoked() {
        let l = layer();
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&hits);
        l.set_created_callback_rust(Some(Box::new(move |_raw| {
            seen.fetch_add(1, Ordering::Release);
        })));
        {
            let g = l.inner.created_callback.lock();
            let f = g.as_ref().expect("callback installed");
            f(std::ptr::null_mut());
        }
        assert_eq!(hits.load(Ordering::Acquire), 1);
        l.set_created_callback_rust(None);
        assert!(l.inner.created_callback.lock().is_none());
    }

    #[test]
    fn set_before_close_callback_rust_installs_the_closure_that_is_invoked() {
        let l = layer();
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&hits);
        l.set_before_close_callback_rust(Some(Box::new(move || {
            seen.fetch_add(1, Ordering::Release);
        })));
        {
            let g = l.inner.before_close_callback.lock();
            let f = g.as_ref().expect("callback installed");
            f();
        }
        assert_eq!(hits.load(Ordering::Acquire), 1);
        l.set_before_close_callback_rust(None);
        assert!(l.inner.before_close_callback.lock().is_none());
    }

    #[test]
    fn set_context_menu_builder_rust_replaces_a_previously_installed_builder() {
        let l = layer();
        let hits = Arc::new(AtomicUsize::new(0));
        let first = Arc::clone(&hits);
        l.set_context_menu_builder_rust(Some(Box::new(move |_raw| {
            first.fetch_add(1, Ordering::Release);
        })));
        let second = Arc::clone(&hits);
        l.set_context_menu_builder_rust(Some(Box::new(move |_raw| {
            second.fetch_add(10, Ordering::Release);
        })));
        {
            let g = l.inner.context_menu_builder.lock();
            let f = g.as_ref().expect("builder installed");
            f(std::ptr::null_mut());
        }
        assert_eq!(hits.load(Ordering::Acquire), 10, "the first builder ran");
        l.set_context_menu_builder_rust(None);
        assert!(l.inner.context_menu_builder.lock().is_none());
    }

    #[test]
    fn set_context_menu_dispatcher_rust_installs_a_closure_whose_answer_is_returned() {
        let l = layer();
        l.set_context_menu_dispatcher_rust(Some(Box::new(|cmd| cmd == 7)));
        {
            let g = l.inner.context_menu_dispatcher.lock();
            let f = g.as_ref().expect("dispatcher installed");
            assert!(f(7));
            assert!(!f(8));
        }
        l.set_context_menu_dispatcher_rust(None);
        assert!(l.inner.context_menu_dispatcher.lock().is_none());
    }

    #[test]
    fn a_detached_inner_reports_no_live_browser_and_ignores_browser_ops() {
        let inner = Inner::new_detached();
        assert!(!inner.browser_alive());
        // Every op below must be a no-op rather than a null dereference.
        inner.invalidate_view();
        inner.send_external_begin_frame();
        inner.cef_was_hidden(true);
        inner.close_browser_force();
        inner.exec_js("window.__nothing();");
        assert!(!inner.browser_alive());
    }
}
