//! Wayland-backend `Platform::init` / `Platform::cleanup` body.
//!
//! Drives the per-process Wayland subsystems in order: read mpv's
//! wayland-display and -surface handles, prime the cached fullscreen,
//! wire input, bring up the core state, install mpv's close-cb
//! trampoline, init EGL, probe dmabuf support, attach the KDE palette
//! manager, start the input thread, and bring up the clipboard reader.

use std::ffi::c_void;

use jfn_linux_util::egl;

// =====================================================================
// FFI declarations consumed during init/cleanup.
// =====================================================================

use jfn_linux_util::dmabuf_probe::jfn_wl_dmabuf_probe;

// =====================================================================
// Helpers
// =====================================================================

fn paint_name(mode: crate::paint_override::WlPaintOverride) -> &'static str {
    use crate::paint_override::WlPaintOverride as M;
    match mode {
        M::Dmabuf => "dmabuf",
        M::Gpu => "gpu",
        M::Shm => "shm",
    }
}

/// What a requested paint path resolves to before any GPU device is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PaintPlan {
    /// The path to try. Still provisional: `Gpu` degrades to `Shm` when no
    /// usable device turns up.
    resolved: crate::paint_override::WlPaintOverride,
    /// Whether a `jfn_gpu_paint::Surfaces` has to be brought up.
    want_gpu_paint: bool,
    /// Whether CEF must be told it cannot hand us shared textures.
    shared_texture_unsupported: bool,
    /// The reason, for the one log line this decision emits.
    note: &'static str,
}

/// Resolve a requested paint path against what the display can actually do.
///
/// `dmabuf_available` is only meaningful for the dmabuf entry — probing it
/// costs an EGL display, so the caller must not run the probe for the other
/// two and passes `false` instead.
fn plan_paint(entry: crate::paint_override::WlPaintOverride, dmabuf_available: bool) -> PaintPlan {
    use crate::paint_override::WlPaintOverride as Req;
    match entry {
        Req::Shm => PaintPlan {
            resolved: Req::Shm,
            want_gpu_paint: false,
            shared_texture_unsupported: true,
            note: "using wl_shm",
        },
        Req::Gpu => PaintPlan {
            resolved: Req::Gpu,
            want_gpu_paint: true,
            shared_texture_unsupported: true,
            note: "Vulkan WSI pixel-upload",
        },
        Req::Dmabuf if dmabuf_available => PaintPlan {
            resolved: Req::Dmabuf,
            want_gpu_paint: false,
            shared_texture_unsupported: false,
            note: "EGL/GBM dmabuf shared texture",
        },
        Req::Dmabuf => PaintPlan {
            resolved: Req::Gpu,
            want_gpu_paint: true,
            shared_texture_unsupported: true,
            note: "EGL dmabuf unavailable; trying gpu",
        },
    }
}

struct ProbeDisplay<'a> {
    egl: &'a egl::Egl,
    display: egl::Display,
}

impl ProbeDisplay<'_> {
    fn init(egl: &egl::Egl, native: egl::NativeDisplayType) -> Option<ProbeDisplay<'_>> {
        // SAFETY: `native` is mpv's live `wl_display`.
        let display = unsafe { egl.get_display(native) }?;
        egl.initialize(display).ok()?;
        Some(ProbeDisplay { egl, display })
    }
}

impl Drop for ProbeDisplay<'_> {
    fn drop(&mut self) {
        let _ = self.egl.terminate(self.display);
    }
}

fn dmabuf_available(native_display: *mut c_void) -> bool {
    let Ok(egl) = egl::load() else {
        return false;
    };
    let Some(probe) = ProbeDisplay::init(&egl, native_display.cast()) else {
        return false;
    };
    unsafe { jfn_wl_dmabuf_probe(c"wayland".as_ptr(), probe.display.as_ptr()) }
}

// =====================================================================
// init / cleanup
// =====================================================================

pub(crate) fn init(rt: &'static crate::runtime::WlRuntime) -> bool {
    let Some(display) = crate::app_conn::app_display(rt) else {
        tracing::error!("Failed to get app Wayland display");
        return false;
    };
    let display = display.as_ptr();

    // Prepare the input layer first so its xkb context is ready before
    // any seat_caps wires up keyboard listeners that need xkb.
    crate::input_lifecycle::lifecycle_init(rt, display);

    let mut core = match unsafe { crate::wl_state::init(rt, display) } {
        Ok(state) => state,
        Err(e) => {
            tracing::error!("wayland core init failed: {e}");
            return false;
        }
    };

    // Seed Rust state with mpv's current fullscreen — first configure
    // after this point won't start a spurious transition.
    core.was_fullscreen = jfn_playback::ingest_driver::jfn_playback_fullscreen();

    use crate::paint_override::WlPaintOverride as Req;
    let requested = rt.paint_request();
    let explicit = requested.is_some();
    let entry = requested.unwrap_or(Req::Dmabuf);

    // The probe opens an EGL display, so it only runs for the entry that needs
    // the answer.
    let plan = plan_paint(entry, entry == Req::Dmabuf && dmabuf_available(display));
    tracing::info!("paint: {}", plan.note);
    if plan.shared_texture_unsupported {
        jfn_platform_abi::get().set_shared_texture_unsupported();
    }
    let want_gpu_paint = plan.want_gpu_paint;
    let mut resolved = plan.resolved;

    if want_gpu_paint {
        match jfn_gpu_paint::Surfaces::init(None, None) {
            Some(gpu) => core.install_gpu_paint(Box::leak(Box::new(gpu))),
            None => {
                tracing::info!("paint: no usable GPU device; using wl_shm");
                resolved = Req::Shm;
            }
        }
    }

    if rt.set_core(core).is_err() {
        tracing::error!("wayland core already initialised");
        return false;
    }

    if explicit
        && let Some(req) = requested
        && req != resolved
    {
        tracing::warn!(
            "--platform-paint={} unavailable; using {}",
            paint_name(req),
            paint_name(resolved)
        );
    }

    #[cfg(feature = "kde-palette")]
    crate::kde_palette::init(rt);

    rt.clipboard().init();
    if !rt.clipboard().available() {
        jfn_platform_abi::get().clear_clipboard_handler();
    }

    jfn_platform_abi::MenuHost::warm(rt.menu());

    true
}

pub(crate) fn cleanup(rt: &'static crate::runtime::WlRuntime) {
    // KDE palette: KWin atomically drops the palette object with the
    // window. The scheme file is unlinked separately via
    // kde_palette::post_window_cleanup after mpv tears down the surface.
    jfn_linux_util::idle_inhibit::cleanup();
    rt.clipboard().cleanup();
    // Must precede root_window::cleanup: the menu's teardown ops go through
    // the root thread's queue.
    if let Some(menu) = rt.try_menu() {
        jfn_platform_abi::MenuHost::shutdown(menu);
    }
    // Stop the app-owned toplevel thread before mpv's VO-teardown roundtrip;
    // otherwise it holds a wl_display read barrier and the roundtrip hangs when
    // no video ever played (a quiet display never wakes its poll).
    crate::root_window::cleanup(rt);
    crate::input_lifecycle::lifecycle_cleanup(rt);
    // Rust-side WlState lives until process exit (mirrors C++ globals).
}

#[cfg(test)]
mod tests {
    use super::{PaintPlan, paint_name, plan_paint};
    use crate::paint_override::WlPaintOverride as Req;

    #[test]
    fn each_paint_path_has_the_name_the_command_line_uses() {
        // These are the `--platform-paint=` values, so they are a contract.
        assert_eq!(paint_name(Req::Dmabuf), "dmabuf");
        assert_eq!(paint_name(Req::Gpu), "gpu");
        assert_eq!(paint_name(Req::Shm), "shm");
    }

    #[test]
    fn an_explicit_shm_request_never_opens_a_gpu_device() {
        let plan = plan_paint(Req::Shm, false);
        assert_eq!(plan.resolved, Req::Shm);
        assert!(!plan.want_gpu_paint);
        assert!(plan.shared_texture_unsupported);
    }

    #[test]
    fn an_explicit_gpu_request_ignores_dmabuf_availability() {
        for available in [false, true] {
            let plan = plan_paint(Req::Gpu, available);
            assert_eq!(plan.resolved, Req::Gpu);
            assert!(plan.want_gpu_paint);
            assert!(plan.shared_texture_unsupported);
        }
    }

    #[test]
    fn dmabuf_is_the_only_path_that_keeps_shared_textures() {
        let plan = plan_paint(Req::Dmabuf, true);
        assert_eq!(plan.resolved, Req::Dmabuf);
        assert!(!plan.want_gpu_paint);
        assert!(!plan.shared_texture_unsupported);
    }

    #[test]
    fn dmabuf_without_egl_support_falls_back_to_gpu_paint() {
        let plan = plan_paint(Req::Dmabuf, false);
        assert_eq!(plan.resolved, Req::Gpu);
        assert!(plan.want_gpu_paint);
        // CEF must be told, or it keeps handing us shared textures we cannot
        // import.
        assert!(plan.shared_texture_unsupported);
    }

    #[test]
    fn every_plan_explains_itself_distinctly() {
        let plans: Vec<PaintPlan> = vec![
            plan_paint(Req::Shm, false),
            plan_paint(Req::Gpu, false),
            plan_paint(Req::Dmabuf, true),
            plan_paint(Req::Dmabuf, false),
        ];
        for (i, a) in plans.iter().enumerate() {
            assert!(!a.note.is_empty());
            for b in &plans[i + 1..] {
                assert_ne!(a.note, b.note, "two plans share the note {:?}", a.note);
            }
        }
    }

    #[test]
    fn only_a_gpu_plan_asks_for_a_gpu_device() {
        for plan in [plan_paint(Req::Shm, false), plan_paint(Req::Dmabuf, true)] {
            assert!(!plan.want_gpu_paint, "{plan:?} wanted a GPU device");
        }
        for plan in [plan_paint(Req::Gpu, true), plan_paint(Req::Dmabuf, false)] {
            assert_eq!(plan.resolved, Req::Gpu);
            assert!(plan.want_gpu_paint, "{plan:?} skipped the GPU device");
        }
    }
}
