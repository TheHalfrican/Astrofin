//! AboutBrowser business logic.
//!
//! Self-managing singleton: `jfn_about_open` creates the layer via the
//! Browsers registry and installs handler closures via the JfnCefLayer
//! setters. The unified BeforeClose path in `client::handle_on_before_close`
//! auto-removes the layer from the registry; the Browsers active-stack
//! restores focus to the previous top automatically. This module just
//! tracks open/closed status via `OPEN`.

use cef::{ImplBrowser, ImplBrowserHost};
use std::os::raw::c_void;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::browsers::{jfn_browsers_create, jfn_browsers_set_active};
use crate::client::{
    jfn_cef_layer_create, jfn_cef_layer_inner, jfn_cef_layer_set_name, jfn_cef_layer_set_visible,
};
use crate::ipc::{BrowserMessage, list_string};
use crate::platform_ops;

static OPEN: AtomicBool = AtomicBool::new(false);

/// Entry point. Creates the about layer and installs all Rust handler
/// closures. Subsequent calls while the layer is alive are no-ops.
pub fn jfn_about_open() {
    if OPEN
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    let kind = c"about";
    let layer = unsafe { jfn_browsers_create(kind.as_ptr()) };
    if layer.is_null() {
        OPEN.store(false, Ordering::Release);
        return;
    }

    let name = c"about";
    unsafe { jfn_cef_layer_set_name(layer, name.as_ptr()) };

    let l = unsafe { &*layer };
    let inner = unsafe { jfn_cef_layer_inner(layer) };

    // setCreatedCallback — about wins input whenever it's created.
    let inner_for_created = Arc::clone(&inner);
    l.set_created_callback_rust(Some(Box::new(move |_browser_raw: *mut c_void| {
        let p = inner_for_created.layer_ptr();
        if !p.is_null() {
            jfn_browsers_set_active(p);
        }
    })));

    // setMessageHandler — aboutDismiss / aboutOpenPath.
    l.set_message_handler_rust(Some(Box::new(handle_message)));

    // setContextMenuBuilder / dispatcher — shared app menu.
    l.set_context_menu_builder_rust(Some(crate::app_menu::build_closure()));
    l.set_context_menu_dispatcher_rust(Some(crate::app_menu::dispatch_closure()));

    // BeforeClose: clear the open-status singleton. The Browsers registry
    // removal + active-stack pop are handled unconditionally by
    // `client::handle_on_before_close`.
    l.set_before_close_callback_rust(Some(Box::new(|| {
        OPEN.store(false, Ordering::Release);
    })));

    unsafe {
        jfn_cef_layer_set_visible(layer, true);
        let url = "app://resources/about.html";
        jfn_cef_layer_create(layer, url.as_ptr() as *const _, url.len());
    }
}

fn handle_message(message: BrowserMessage) -> bool {
    let args = message.args();

    match message.name() {
        "aboutDismiss" => {
            if let Some(b) = message.browser()
                && let Some(host) = b.host()
            {
                host.close_browser(0);
            }
            true
        }
        "aboutOpenPath" => {
            let Some(args) = args else { return true };
            open_about_path(&list_string(args, 0));
            true
        }
        _ => false,
    }
}

/// Hand one About-box path to the desktop's file manager, or refuse it.
///
/// Two rules, both new as of 2026-09-10:
///   - the path goes to [`Platform::open_path`], which every backend
///     implements as one `Command`/`ShellExecuteW` argument. Nothing builds a
///     `file://` URL out of it any more, and nothing invokes a shell, so a
///     path containing a space, `&`, `#`, a quote or a `;` is opened as
///     written rather than parsed;
///   - only a path under the config dir, the cache dir or the log file's own
///     directory may be opened at all. The About box offers exactly two, but
///     `aboutOpenPath` is an IPC name, and a bound IPC is a capability.
fn open_about_path(path: &str) {
    let roots = openable_roots();
    let Some(target) = allowed_open_path(path, &roots) else {
        jfn_logging::log(
            jfn_logging::CATEGORY_CEF,
            jfn_logging::LEVEL_WARN,
            &format!(
                "aboutOpenPath: refused {} (not under the config, cache or log directory)",
                jfn_logging::escape_page_string(path)
            ),
        );
        return;
    };
    if let Some(p) = platform_ops::ops() {
        p.open_path(&target);
    }
}

/// The directories `aboutOpenPath` may open something inside: the config dir,
/// the cache dir, and the directory holding the log file actually in use
/// (which `--log-file` can move anywhere).
fn openable_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        absolutise(jfn_paths::config_dir()),
        absolutise(jfn_paths::cache_dir()),
        absolutise(jfn_paths::log_dir()),
    ];
    let active = jfn_logging::active_path();
    if !active.is_empty()
        && let Some(dir) = absolutise(PathBuf::from(active)).parent()
    {
        roots.push(dir.to_path_buf());
    }
    roots
}

/// Absolute-but-not-resolved, matching `resource::abs_path`: a relative path
/// is joined onto the cwd, symlinks and `..` are left alone (the caller
/// rejects `..` outright).
fn absolutise(p: PathBuf) -> PathBuf {
    if p.is_absolute() {
        return p;
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(p),
        Err(_) => p,
    }
}

/// True when `path` is `root` or lies inside it, compared component by
/// component (case-insensitively on Windows, where the filesystem is).
fn is_under(path: &Path, root: &Path) -> bool {
    let mut root_components = root.components();
    let mut path_components = path.components();
    loop {
        match (root_components.next(), path_components.next()) {
            (None, _) => return true,
            (Some(_), None) => return false,
            (Some(r), Some(p)) => {
                let same = if cfg!(windows) {
                    r.as_os_str()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&p.as_os_str().to_string_lossy())
                } else {
                    r == p
                };
                if !same {
                    return false;
                }
            }
        }
    }
}

/// The path `aboutOpenPath` may open, or `None` when it is refused.
///
/// Pure so the rule can be tested without a profile: `roots` is what
/// [`openable_roots`] resolves at runtime. A path must be non-empty,
/// absolute, free of `..` components and of interior NULs, and inside one of
/// the roots.
fn allowed_open_path(path: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    if path.is_empty() || path.contains('\0') {
        return None;
    }
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return None;
    }
    if candidate
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return None;
    }
    roots
        .iter()
        .any(|root| is_under(candidate, root))
        .then(|| candidate.to_path_buf())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[cfg(windows)]
    fn roots() -> Vec<PathBuf> {
        vec![
            PathBuf::from(r"C:\Users\x\AppData\Roaming\astrofin"),
            PathBuf::from(r"C:\Users\x\AppData\Local\astrofin"),
        ]
    }

    #[cfg(not(windows))]
    fn roots() -> Vec<PathBuf> {
        vec![
            PathBuf::from("/home/x/.config/astrofin"),
            PathBuf::from("/home/x/.cache/astrofin"),
        ]
    }

    #[cfg(windows)]
    const INSIDE: &str = r"C:\Users\x\AppData\Roaming\astrofin\mpv";
    #[cfg(not(windows))]
    const INSIDE: &str = "/home/x/.config/astrofin/mpv";

    #[test]
    fn allowed_open_path_accepts_a_path_inside_a_root_and_the_root_itself() {
        assert_eq!(
            allowed_open_path(INSIDE, &roots()),
            Some(PathBuf::from(INSIDE))
        );
        let root = roots()[0].clone();
        assert_eq!(
            allowed_open_path(&root.to_string_lossy(), &roots()),
            Some(root)
        );
    }

    #[test]
    fn allowed_open_path_refuses_anything_outside_the_roots() {
        // The About box offers two paths; `aboutOpenPath` is an IPC name, and
        // a bound IPC is a capability. Everything else is refused.
        #[cfg(windows)]
        let outside = [
            r"C:\Windows\System32\cmd.exe",
            r"C:\Users\x\Desktop\evil.exe",
            r"C:\Users\x\AppData\Roaming\astrofin-not-ours\x",
        ];
        #[cfg(not(windows))]
        let outside = [
            "/etc/passwd",
            "/home/x/Desktop/evil.sh",
            "/home/x/.config/astrofin-not-ours/x",
        ];
        for bad in outside {
            assert_eq!(allowed_open_path(bad, &roots()), None, "{bad}");
        }
    }

    #[test]
    fn allowed_open_path_refuses_traversal_relative_paths_and_degenerate_input() {
        let mut bad = vec![
            String::new(),
            "  ".to_string(),
            "relative/path".to_string(),
            format!("{INSIDE}\0/etc/passwd"),
        ];
        // `..` is refused even when the string still starts inside a root:
        // the prefix check is lexical, so it must not be walked past.
        bad.push(format!("{INSIDE}/../../../../etc/passwd"));
        for path in &bad {
            assert_eq!(allowed_open_path(path, &roots()), None, "{path:?}");
        }
    }

    #[test]
    fn allowed_open_path_does_not_mangle_a_shell_metacharacter() {
        // Nothing invokes a shell any more: the path is one argument, so a
        // space, `&`, `;`, `#` or a quote survives verbatim into `open`.
        let hostile = format!("{INSIDE}/a b&c;d#e\"f");
        assert_eq!(
            allowed_open_path(&hostile, &roots()),
            Some(PathBuf::from(&hostile))
        );
    }

    #[test]
    fn is_under_needs_a_whole_component_to_match() {
        assert!(is_under(Path::new(INSIDE), &roots()[0]));
        assert!(!is_under(&roots()[0], Path::new(INSIDE)));
    }

    #[test]
    fn openable_roots_lists_the_config_cache_and_log_directories() {
        let roots = openable_roots();
        assert!(roots.len() >= 3);
        assert!(roots.iter().all(|r| r.is_absolute()), "{roots:?}");
        assert!(roots.contains(&absolutise(jfn_paths::config_dir())));
        assert!(roots.contains(&absolutise(jfn_paths::cache_dir())));
    }

    #[test]
    fn absolutise_leaves_an_absolute_path_alone() {
        let abs = PathBuf::from(INSIDE);
        assert_eq!(absolutise(abs.clone()), abs);
        assert!(absolutise(PathBuf::from("rel")).is_absolute());
    }

    #[test]
    fn open_about_path_refuses_a_path_outside_the_roots_without_panicking() {
        // No platform installed in this test, so the accepted branch is a
        // no-op; what matters is that neither branch panics.
        open_about_path("");
        #[cfg(windows)]
        open_about_path(r"C:\Windows\System32\cmd.exe");
        #[cfg(not(windows))]
        open_about_path("/etc/passwd");
    }
}
