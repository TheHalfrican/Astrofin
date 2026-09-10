//! KDE/KWin per-window titlebar color support.

use parking_lot::Mutex;
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const COLOR_SCHEME_TEMPLATE: &str = include_str!("kde_palette_template.ini");

struct PaletteState {
    colors_dir: PathBuf,
    current_path: Option<CString>,
}

pub(crate) struct Palette {
    state: Mutex<Option<PaletteState>>,
}

impl Palette {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(None),
        }
    }
}

/// Active and inactive titlebar foregrounds for a background colour, chosen by
/// BT.709 luminance. A dark titlebar gets near-white text dimmed to grey when
/// the window loses focus; a light one gets KDE's own near-black, with the
/// same colour when inactive — dimming dark text on a light bar makes it
/// unreadable rather than subtle.
fn scheme_foregrounds(r: u8, g: u8, b: u8) -> (&'static str, &'static str) {
    let lum = 0.2126 * (f64::from(r) / 255.0)
        + 0.7152 * (f64::from(g) / 255.0)
        + 0.0722 * (f64::from(b) / 255.0);
    if lum < 0.5 {
        ("252,252,252", "126,126,126")
    } else {
        ("35,38,41", "35,38,41")
    }
}

/// The KWin colour-scheme file for one titlebar colour: the template with
/// every placeholder filled in. Pure text — writing it is the caller's job.
fn color_scheme_ini(r: u8, g: u8, b: u8) -> String {
    let bg = format!("{},{},{}", r, g, b);
    let (active_fg, inactive_fg) = scheme_foregrounds(r, g, b);
    COLOR_SCHEME_TEMPLATE
        .replace("%HEADER_BG%", &bg)
        .replace("%INACTIVE_BG%", &bg)
        .replace("%ACTIVE_FG%", active_fg)
        .replace("%INACTIVE_FG%", inactive_fg)
}

fn write_color_scheme(r: u8, g: u8, b: u8, path: &std::path::Path) -> std::io::Result<()> {
    fs::write(path, color_scheme_ini(r, g, b))
}

/// The six hex digits of a `#rrggbb` colour, or `None` for anything else. The
/// digits go straight into a file name, so a malformed colour must not get
/// that far.
fn palette_hex_body(s: &str) -> Option<&str> {
    if s.len() != 7 {
        return None;
    }
    s.strip_prefix('#')
}

/// The scheme file's name for a colour. It doubles as the cache key that
/// decides whether the file has to be rewritten, so it is one-to-one with the
/// colour and never varies for the same one.
fn palette_file_name(hex_body: &str) -> String {
    format!("Astrofin-{}.colors", hex_body)
}

fn make_colors_dir() -> Option<PathBuf> {
    let runtime = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(s) if !s.is_empty() => s,
        _ => return None,
    };
    let mut dir = PathBuf::from(runtime);
    dir.push("astrofin");
    if let Err(e) = fs::create_dir_all(&dir) {
        tracing::warn!("kde_palette: mkdir {} failed: {}", dir.display(), e);
        return None;
    }
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    Some(dir)
}

pub(crate) fn init(rt: &crate::runtime::WlRuntime) -> bool {
    if rt.palette().state.lock().is_some() {
        return true;
    }
    let Some(colors_dir) = make_colors_dir() else {
        return false;
    };
    *rt.palette().state.lock() = Some(PaletteState {
        colors_dir,
        current_path: None,
    });
    true
}

pub(crate) fn set_color(
    rt: &'static crate::runtime::WlRuntime,
    r: u8,
    g: u8,
    b: u8,
    hex: &std::ffi::CStr,
) {
    let Some(hex_str) = hex.to_str().ok().and_then(palette_hex_body) else {
        return;
    };

    let mut guard = rt.palette().state.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return,
    };

    let mut new_path = state.colors_dir.clone();
    new_path.push(palette_file_name(hex_str));

    let new_path_c = match CString::new(new_path.as_os_str().as_encoded_bytes()) {
        Ok(c) => c,
        Err(_) => return,
    };
    if state.current_path.as_ref() == Some(&new_path_c) {
        return;
    }

    if let Err(e) = write_color_scheme(r, g, b, &new_path) {
        tracing::warn!("kde_palette: write {} failed: {}", new_path.display(), e);
        return;
    }

    if let Some(old) = state.current_path.take() {
        let old_path = std::path::Path::new(std::ffi::OsStr::from_bytes(old.as_bytes()));
        let _ = fs::remove_file(old_path);
    }

    rt.root().set_titlebar_palette(&new_path);
    state.current_path = Some(new_path_c);
}

pub(crate) fn post_window_cleanup(rt: &crate::runtime::WlRuntime) {
    let mut guard = rt.palette().state.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return,
    };
    if let Some(old) = state.current_path.take() {
        let old_path = std::path::Path::new(std::ffi::OsStr::from_bytes(old.as_bytes()));
        let _ = fs::remove_file(old_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dark_titlebar_gets_light_text_that_dims_when_inactive() {
        assert_eq!(
            scheme_foregrounds(0x10, 0x10, 0x10),
            ("252,252,252", "126,126,126")
        );
        assert_eq!(scheme_foregrounds(0, 0, 0), ("252,252,252", "126,126,126"));
    }

    #[test]
    fn a_light_titlebar_keeps_the_same_dark_text_when_inactive() {
        assert_eq!(
            scheme_foregrounds(0xFF, 0xFF, 0xFF),
            ("35,38,41", "35,38,41")
        );
    }

    #[test]
    fn luminance_is_weighted_per_channel_not_averaged() {
        // Pure green is bright enough for dark text; pure blue is not, even
        // though a flat average would put them on the same side.
        assert_eq!(scheme_foregrounds(0, 0xFF, 0).0, "35,38,41");
        assert_eq!(scheme_foregrounds(0, 0, 0xFF).0, "252,252,252");
        assert_eq!(scheme_foregrounds(0xFF, 0, 0).0, "252,252,252");
    }

    #[test]
    fn the_scheme_leaves_no_placeholder_unfilled() {
        let ini = color_scheme_ini(0x10, 0x20, 0x30);
        assert!(!ini.contains('%'), "unsubstituted placeholder in scheme");
    }

    #[test]
    fn the_scheme_carries_the_colour_as_a_kde_rgb_triple() {
        // KDE writes colours decimal, so 0x10,0x20,0x30 is 16,32,48.
        let ini = color_scheme_ini(0x10, 0x20, 0x30);
        assert!(ini.contains("activeBackground=16,32,48"));
        assert!(ini.contains("inactiveBackground=16,32,48"));
        assert!(ini.contains("BackgroundNormal=16,32,48"));
    }

    #[test]
    fn the_titlebar_foregrounds_follow_the_background_luminance() {
        let dark = color_scheme_ini(0, 0, 0);
        assert!(dark.contains("activeForeground=252,252,252"));
        assert!(dark.contains("inactiveForeground=126,126,126"));

        let light = color_scheme_ini(0xFF, 0xFF, 0xFF);
        assert!(light.contains("activeForeground=35,38,41"));
        assert!(light.contains("inactiveForeground=35,38,41"));
        assert_ne!(dark, light);
    }

    #[test]
    fn a_well_formed_hex_colour_yields_its_six_digits() {
        assert_eq!(palette_hex_body("#abcdef"), Some("abcdef"));
        assert_eq!(palette_hex_body("#000000"), Some("000000"));
    }

    #[test]
    fn a_hex_colour_of_the_wrong_length_or_shape_is_rejected() {
        assert_eq!(palette_hex_body(""), None);
        assert_eq!(palette_hex_body("#abc"), None);
        assert_eq!(palette_hex_body("#abcdef0"), None);
        assert_eq!(palette_hex_body("abcdef0"), None);
        // Seven bytes, but not a colour: the '#' is what makes it one.
        assert_eq!(palette_hex_body("/etc/pw"), None);
    }

    #[test]
    fn a_multibyte_string_of_seven_bytes_is_not_mistaken_for_a_colour() {
        // Seven UTF-8 bytes, four characters — slicing at 1 would have split a
        // code point.
        let s = "\u{e9}\u{e9}\u{e9}a";
        assert_eq!(s.len(), 7);
        assert_eq!(palette_hex_body(s), None);
    }

    #[test]
    fn the_file_name_is_one_to_one_with_the_colour() {
        assert_eq!(palette_file_name("abcdef"), "Astrofin-abcdef.colors");
        assert_ne!(palette_file_name("abcdef"), palette_file_name("abcdee"));
    }
}
