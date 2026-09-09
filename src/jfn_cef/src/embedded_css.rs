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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Declaration order *is* the cascade order: tokens, then fonts, then the
    /// theme that consumes both.
    const NAMES: &[&str] = &[
        "astrofin-tokens.css",
        "astrofin-fonts.css",
        "astrofin-theme.css",
    ];

    #[test]
    fn every_embedded_stylesheet_resolves_to_non_empty_source() {
        for name in NAMES {
            let src = get(name).unwrap_or_else(|| panic!("{name} is not embedded"));
            assert!(!src.trim().is_empty(), "{name} embedded as empty");
        }
    }

    #[test]
    fn every_style_the_injection_profiles_declare_is_embedded() {
        use crate::injection::InjectedStyle as S;
        let all = [S::Tokens, S::Fonts, S::Theme];
        assert_eq!(all.len(), NAMES.len());
        for style in all {
            let name = style.file_name();
            assert!(get(name).is_some(), "{name} declared but not embedded");
            assert!(NAMES.contains(&name), "{name} missing from the test table");
        }
    }

    #[test]
    fn the_tokens_sheet_defines_the_custom_properties_the_theme_reads() {
        let tokens = get("astrofin-tokens.css").expect("tokens embedded");
        assert!(
            tokens.contains("--af-"),
            "the tokens sheet defines no --af-* custom property"
        );
    }

    #[test]
    fn an_unknown_stylesheet_name_is_none() {
        assert!(get("").is_none());
        assert!(get("astrofin-tokens").is_none());
        assert!(get("../../secrets.css").is_none());
        assert!(get("ASTROFIN-THEME.CSS").is_none());
    }
}
