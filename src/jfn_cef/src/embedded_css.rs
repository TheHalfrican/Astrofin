//! Embedded stylesheet sources, included at compile time from `src/web/*.css`.
//!
//! Mirrors [`crate::embedded_js`]. The renderer process concatenates the
//! sheets a browser profile declares (see [`crate::injection::InjectedStyle`])
//! and installs them as a single `<style id="af-theme">` element before
//! jellyfin-web's own bundles run.

pub fn get(name: &str) -> Option<&'static str> {
    Some(match name {
        "astrofin-tokens.css" => include_str!("../../web/astrofin-tokens.css"),
        "astrofin-fonts.css" => include_str!("../../web/astrofin-fonts.css"),
        "astrofin-theme.css" => include_str!("../../web/astrofin-theme.css"),
        _ => return None,
    })
}
