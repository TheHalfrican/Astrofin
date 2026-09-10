//! Pure decisions the X11 [`jfn_platform_abi::Platform`] impl makes before it
//! touches the server.

use jfn_platform_abi::WindowDecorations;

/// Fold a configured decoration preference onto what X11 can actually do.
///
/// X11 has no client-side-decoration path here, so `Csd` degrades to the WM's
/// own frame. `default` is a closure because the platform default is itself a
/// probe (desktop-environment sniffing) that must not run when the user has
/// already chosen.
pub(crate) fn resolve_decorations(
    configured: Option<WindowDecorations>,
    default: impl FnOnce() -> WindowDecorations,
) -> WindowDecorations {
    match configured.unwrap_or_else(default) {
        WindowDecorations::Csd => WindowDecorations::Server,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn client_side_decorations_degrade_to_the_window_manager() {
        let d = resolve_decorations(Some(WindowDecorations::Csd), || {
            WindowDecorations::ServerThemed
        });
        assert_eq!(d, WindowDecorations::Server);
    }

    #[test]
    fn a_server_choice_is_kept_as_is() {
        assert_eq!(
            resolve_decorations(Some(WindowDecorations::Server), || {
                WindowDecorations::Csd
            }),
            WindowDecorations::Server
        );
        assert_eq!(
            resolve_decorations(Some(WindowDecorations::ServerThemed), || {
                WindowDecorations::Csd
            }),
            WindowDecorations::ServerThemed
        );
    }

    #[test]
    fn an_unset_preference_falls_back_to_the_platform_default() {
        assert_eq!(
            resolve_decorations(None, || WindowDecorations::ServerThemed),
            WindowDecorations::ServerThemed
        );
        // ...and a CSD default degrades exactly like a configured one.
        assert_eq!(
            resolve_decorations(None, || WindowDecorations::Csd),
            WindowDecorations::Server
        );
    }

    #[test]
    fn the_default_probe_is_not_run_when_a_preference_is_set() {
        let asked = Cell::new(false);
        let d = resolve_decorations(Some(WindowDecorations::Server), || {
            asked.set(true);
            WindowDecorations::Csd
        });
        assert_eq!(d, WindowDecorations::Server);
        assert!(!asked.get());
    }
}
