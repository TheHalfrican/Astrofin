//! The dmabuf → gpu → shm ladder, as plain values.
//!
//! Split out of [`crate::paint`], which opens the GPU device and probes CEF's
//! dmabuf producer. `--platform-paint` picks only the *entry* tier; an
//! unavailable tier degrades to the next one down, and the user is told when
//! what they asked for is not what they got. The probes stay behind closures so
//! this module can decide the ladder without running any of them — and so the
//! tests can prove the expensive ones are not run when the answer is already
//! settled.

use crate::paint_override::X11PaintOverride;

/// Which tiers the entry preference leaves on the table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PaintRequest {
    /// Open a GPU device at all. False only for an explicit `--platform-paint=shm`.
    pub(crate) want_gpu: bool,
    /// Ask CEF for shared textures. True by default and for an explicit
    /// `--platform-paint=dmabuf`; `gpu` means pixel upload on purpose.
    pub(crate) want_dmabuf: bool,
}

pub(crate) fn paint_request(requested: Option<X11PaintOverride>) -> PaintRequest {
    use X11PaintOverride as Req;
    PaintRequest {
        want_gpu: !matches!(requested, Some(Req::Shm)),
        want_dmabuf: matches!(requested, None | Some(Req::Dmabuf)),
    }
}

/// Whether CEF should actually produce shared textures.
///
/// Two independent halves: `can_import` proves only that *our* device can
/// consume a dmabuf; CEF's producer must also work, and it is broken on NVIDIA
/// proprietary X11. The producer probe is the expensive one and runs last.
pub(crate) fn use_dmabuf(
    want_dmabuf: bool,
    can_import: impl FnOnce() -> bool,
    producer_ok: impl FnOnce() -> bool,
) -> bool {
    want_dmabuf && can_import() && producer_ok()
}

/// The tier actually reached, for the "you asked for X, you got Y" comparison.
pub(crate) fn reached_tier(has_gpu: bool, use_dmabuf: bool) -> X11PaintOverride {
    if !has_gpu {
        X11PaintOverride::Shm
    } else if use_dmabuf {
        X11PaintOverride::Dmabuf
    } else {
        X11PaintOverride::Gpu
    }
}

/// The `(requested, reached)` names to warn with, or `None` when nothing was
/// asked for or the request was honoured.
pub(crate) fn degraded_from(
    requested: Option<X11PaintOverride>,
    reached: X11PaintOverride,
) -> Option<(&'static str, &'static str)> {
    let req = requested?;
    (req != reached).then(|| (paint_name(req), paint_name(reached)))
}

pub(crate) fn paint_name(mode: X11PaintOverride) -> &'static str {
    match mode {
        X11PaintOverride::Dmabuf => "dmabuf",
        X11PaintOverride::Gpu => "gpu",
        X11PaintOverride::Shm => "shm",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn no_preference_asks_for_the_top_of_the_ladder() {
        assert_eq!(
            paint_request(None),
            PaintRequest {
                want_gpu: true,
                want_dmabuf: true,
            }
        );
    }

    #[test]
    fn asking_for_gpu_opens_a_device_but_not_shared_textures() {
        assert_eq!(
            paint_request(Some(X11PaintOverride::Gpu)),
            PaintRequest {
                want_gpu: true,
                want_dmabuf: false,
            }
        );
    }

    #[test]
    fn asking_for_shm_opens_no_device_at_all() {
        assert_eq!(
            paint_request(Some(X11PaintOverride::Shm)),
            PaintRequest {
                want_gpu: false,
                want_dmabuf: false,
            }
        );
    }

    #[test]
    fn asking_for_dmabuf_asks_for_both_halves() {
        assert_eq!(
            paint_request(Some(X11PaintOverride::Dmabuf)),
            PaintRequest {
                want_gpu: true,
                want_dmabuf: true,
            }
        );
    }

    #[test]
    fn dmabuf_needs_both_our_importer_and_cefs_producer() {
        assert!(use_dmabuf(true, || true, || true));
        assert!(!use_dmabuf(true, || true, || false));
        assert!(!use_dmabuf(true, || false, || true));
        assert!(!use_dmabuf(false, || true, || true));
    }

    #[test]
    fn the_producer_probe_is_skipped_once_the_answer_is_settled() {
        let probed = Cell::new(false);
        let probe = || {
            probed.set(true);
            true
        };
        assert!(!use_dmabuf(false, || true, probe));
        assert!(!probed.get());

        let probed = Cell::new(false);
        let probe = || {
            probed.set(true);
            true
        };
        assert!(!use_dmabuf(true, || false, probe));
        assert!(!probed.get());
    }

    #[test]
    fn no_gpu_device_lands_on_shm_whatever_was_asked() {
        assert_eq!(reached_tier(false, false), X11PaintOverride::Shm);
        assert_eq!(reached_tier(false, true), X11PaintOverride::Shm);
    }

    #[test]
    fn a_gpu_device_lands_on_dmabuf_or_pixel_upload() {
        assert_eq!(reached_tier(true, true), X11PaintOverride::Dmabuf);
        assert_eq!(reached_tier(true, false), X11PaintOverride::Gpu);
    }

    #[test]
    fn an_honoured_request_does_not_warn() {
        assert_eq!(
            degraded_from(Some(X11PaintOverride::Gpu), X11PaintOverride::Gpu),
            None
        );
    }

    #[test]
    fn an_unavailable_request_names_both_tiers() {
        assert_eq!(
            degraded_from(Some(X11PaintOverride::Dmabuf), X11PaintOverride::Shm),
            Some(("dmabuf", "shm"))
        );
    }

    #[test]
    fn a_default_boot_never_warns_about_the_tier_it_got() {
        assert_eq!(degraded_from(None, X11PaintOverride::Shm), None);
        assert_eq!(degraded_from(None, X11PaintOverride::Dmabuf), None);
    }

    #[test]
    fn every_tier_has_the_name_the_cli_flag_uses() {
        assert_eq!(paint_name(X11PaintOverride::Dmabuf), "dmabuf");
        assert_eq!(paint_name(X11PaintOverride::Gpu), "gpu");
        assert_eq!(paint_name(X11PaintOverride::Shm), "shm");
    }
}
