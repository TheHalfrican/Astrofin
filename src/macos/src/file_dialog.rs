//! macOS file choosers: `NSOpenPanel` / `NSSavePanel` as a sheet on mpv's
//! window, answered through a completion handler.
//!
//! CEF's own chooser cannot run for a windowless browser, so the CEF dialog
//! handler hands every `<input type=file>` to the platform (see the dialog
//! module in `jfn_cef`). AppKit panels must be created and shown on the main
//! thread; CEF's UI thread is the main thread in this single-process build,
//! so the request normally arrives there, and one from any other thread is
//! posted to the main queue. The panel is never run modally: a nested run
//! loop inside a CEF callback would re-enter the external message pump.
//! `beginSheetModalForWindow:` returns at once, and the handler runs later on
//! the main thread, where `on_done` is invoked exactly once.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use block2::RcBlock;
use jfn_platform_abi::{FileDialogFilter, FileDialogKind, FileDialogRequest};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSModalResponse, NSOpenPanel, NSSavePanel, NSWindow};
use objc2_foundation::{NSArray, NSString, NSURL};

/// `NSModalResponseOK`: the crate exposes the response type, not the constant.
const MODAL_RESPONSE_OK: NSModalResponse = 1;

/// Flat, de-duplicated extension list for the panel's type filter. Empty means
/// "any file", which is also what a folder picker gets: its filter is the
/// directory itself.
fn allowed_extensions(kind: FileDialogKind, filters: &[FileDialogFilter]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if kind == FileDialogKind::OpenFolder {
        return out;
    }
    for filter in filters {
        for ext in &filter.extensions {
            let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
            if !ext.is_empty() && !out.contains(&ext) {
                out.push(ext);
            }
        }
    }
    out
}

/// A seed path names either a folder to open in, or a folder plus the file
/// name to pre-fill (save panels).
fn seed_from(path: &Path) -> (Option<PathBuf>, Option<String>) {
    if path.is_dir() {
        return (Some(path.to_path_buf()), None);
    }
    let folder = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map(Path::to_path_buf);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    (folder, name)
}

fn url_path(url: &NSURL) -> Option<PathBuf> {
    url.path().map(|s| PathBuf::from(s.to_string()))
}

/// The chosen paths after `NSModalResponseOK`; `None` when nothing usable
/// came back, which the caller treats like a cancel.
fn collect(panel: &NSSavePanel, open: Option<&NSOpenPanel>) -> Option<Vec<PathBuf>> {
    let paths: Vec<PathBuf> = match open {
        Some(open) => open.URLs().iter().filter_map(|u| url_path(&u)).collect(),
        None => panel.URL().and_then(|u| url_path(&u)).into_iter().collect(),
    };
    (!paths.is_empty()).then_some(paths)
}

/// Configures and presents the panel. Main thread only.
fn show(req: FileDialogRequest, mtm: MainThreadMarker) {
    let FileDialogRequest {
        kind,
        title,
        default_path,
        filters,
        on_done,
    } = req;

    // NSOpenPanel is an NSSavePanel; keep one handle of the base type for the
    // shared configuration and the open-only handle for its own switches.
    let (panel, open): (Retained<NSSavePanel>, Option<Retained<NSOpenPanel>>) =
        if kind == FileDialogKind::SaveFile {
            (NSSavePanel::savePanel(mtm), None)
        } else {
            let open = NSOpenPanel::openPanel(mtm);
            open.setCanChooseFiles(kind != FileDialogKind::OpenFolder);
            open.setCanChooseDirectories(kind == FileDialogKind::OpenFolder);
            open.setAllowsMultipleSelection(kind.multiple());
            (Retained::into_super(open.clone()), Some(open))
        };

    if let Some(title) = title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        // `message` is the text above the file list; panel titles are not
        // shown on modern macOS.
        panel.setMessage(Some(&NSString::from_str(title)));
    }

    let extensions = allowed_extensions(kind, &filters);
    if !extensions.is_empty() {
        let types: Vec<Retained<NSString>> =
            extensions.iter().map(|e| NSString::from_str(e)).collect();
        // `allowedContentTypes` needs the UniformTypeIdentifiers crate for
        // nothing else; the extension form works from macOS 11, which is the
        // bundle's deployment target.
        #[allow(deprecated)]
        panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&types)));
    }

    if let Some(seed) = default_path.as_deref() {
        let (folder, name) = seed_from(seed);
        if let Some(folder) = folder {
            let ns = NSString::from_str(&folder.to_string_lossy());
            panel.setDirectoryURL(Some(&NSURL::fileURLWithPath_isDirectory(&ns, true)));
        }
        if let (Some(name), FileDialogKind::SaveFile) = (name, kind) {
            panel.setNameFieldStringValue(&NSString::from_str(&name));
        }
    }

    // The block is `Fn`, `on_done` is `FnOnce`: take it out on first call.
    let done_slot = Mutex::new(Some(on_done));
    let panel_for_block = panel.clone();
    let open_for_block = open.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
        let Some(done) = done_slot.lock().ok().and_then(|mut g| g.take()) else {
            return;
        };
        let paths = if response == MODAL_RESPONSE_OK {
            collect(&panel_for_block, open_for_block.as_deref())
        } else {
            None
        };
        done(paths);
    });

    let window = crate::init::jfn_macos_get_window();
    if window.is_null() {
        panel.beginWithCompletionHandler(&handler);
    } else {
        // SAFETY: `jfn_macos_get_window` hands out mpv's live NSWindow, held
        // by the platform state for the whole session.
        let window: &NSWindow = unsafe { &*window.cast::<NSWindow>() };
        panel.beginSheetModalForWindow_completionHandler(window, &handler);
    }
}

/// Takes the request and presents a panel. Always `true`: once here, `on_done`
/// runs exactly once, from the panel's completion handler.
pub(crate) fn open(req: FileDialogRequest) -> bool {
    match MainThreadMarker::new() {
        Some(mtm) => show(req, mtm),
        None => crate::dispatch::post_to_main(move || {
            // SAFETY: the main dispatch queue runs this on the main thread.
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            show(req, mtm);
        }),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(extensions: &[&str]) -> FileDialogFilter {
        FileDialogFilter {
            description: String::new(),
            extensions: extensions.iter().map(|e| (*e).to_string()).collect(),
        }
    }

    #[test]
    fn extensions_are_flattened_normalized_and_unique() {
        let filters = [filter(&["JPG", ".jpeg", "png"]), filter(&["png", " gif "])];
        assert_eq!(
            allowed_extensions(FileDialogKind::OpenFile, &filters),
            ["jpg", "jpeg", "png", "gif"]
        );
    }

    #[test]
    fn folder_pickers_have_no_type_filter() {
        assert!(allowed_extensions(FileDialogKind::OpenFolder, &[filter(&["jpg"])]).is_empty());
    }

    #[test]
    fn seed_splits_a_file_path_into_folder_and_name() {
        let (folder, name) = seed_from(Path::new("/Users/me/Pictures/cover.jpg"));
        assert_eq!(folder.as_deref(), Some(Path::new("/Users/me/Pictures")));
        assert_eq!(name.as_deref(), Some("cover.jpg"));
    }

    #[test]
    fn dots_and_blanks_never_become_extensions() {
        let filters = [filter(&[".", "  ", "", "..jpg"])];
        assert_eq!(
            allowed_extensions(FileDialogKind::OpenFile, &filters),
            ["jpg"]
        );
    }

    #[test]
    fn a_filterless_request_has_no_type_filter() {
        assert!(allowed_extensions(FileDialogKind::SaveFile, &[]).is_empty());
    }

    #[test]
    fn a_seed_that_is_only_a_file_name_has_no_folder() {
        let (folder, name) = seed_from(Path::new("cover.jpg"));
        assert!(folder.is_none());
        assert_eq!(name.as_deref(), Some("cover.jpg"));
    }

    #[test]
    fn seed_of_an_existing_folder_is_the_folder() {
        let (folder, name) = seed_from(Path::new("/tmp"));
        assert_eq!(folder.as_deref(), Some(Path::new("/tmp")));
        assert_eq!(name, None);
    }
}
