//! Native file choosers on Linux, through the XDG desktop portal
//! (`org.freedesktop.portal.FileChooser`).
//!
//! CEF's own chooser assumes a windowed, aura-backed browser and cannot run
//! for a windowless client, so the CEF dialog handler claims `OnFileDialog`
//! and hands every ask down to the platform (see the dialog module in
//! `jfn_cef`). Windows opens a Common Item Dialog and macOS an `NSOpenPanel`;
//! on Linux the portal is the one answer that covers every case at once. It
//! behaves the same on Wayland and X11, links no toolkit into the process,
//! opens whatever chooser the desktop actually uses, and is the only route
//! that returns a readable path from inside a Flatpak sandbox — there it
//! answers with a document-portal URI rather than the raw file.
//!
//! The exchange is asynchronous by nature. `OpenFile` returns a request
//! object path immediately; the chosen files only arrive later, on that
//! object's `Response` signal, once the user dismisses the dialog — which may
//! be minutes. The whole exchange therefore runs on its own thread, and
//! `on_done` is called from that thread exactly once. That is safe because
//! `jfn_cef`'s `deliver` re-posts the answer to `TID_UI` itself, so nothing
//! here needs thread affinity.
//!
//! The signal subscription is opened *before* the method call and matches on
//! interface and member rather than on the request path. Matching the path
//! would mean knowing it first, and the reply that carries it can lose the
//! race against a portal that answers immediately; filtering the delivered
//! signals by path instead has no race and no dependence on the portal
//! honouring `handle_token`.
//!
//! The chooser is parented to the app window so the desktop can tie the two
//! together — modal, stacked above, and grouped in the task switcher. The
//! portal names the parent with an `x11:<xid>` or `wayland:<handle>` string,
//! which each backend builds through [`x11_parent`] / [`wayland_parent`]: X11
//! has the toplevel id to hand, and Wayland exports its toplevel once through
//! `zxdg_exporter_v2` and caches the handle. Exporting is a round trip on the
//! Wayland queue, so it happens at window creation rather than here — this
//! runs on CEF's UI thread, which must not block on the compositor.
//!
//! An empty parent is still honoured, and is what a compositor without
//! `xdg_foreign` falls back to: the chooser opens and returns files exactly
//! as before, the desktop simply does not tie it to the window. Losing the
//! parent is never a reason to lose the dialog.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use jfn_platform_abi::{FileDialogFilter, FileDialogKind, FileDialogRequest};
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::zvariant::{DeserializeDict, OwnedObjectPath, SerializeDict, Type};

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const FILE_CHOOSER: &str = "org.freedesktop.portal.FileChooser";
const REQUEST: &str = "org.freedesktop.portal.Request";

/// `XdgDesktopPortal::Response::Success`. Anything else is a cancel or a
/// failure, and both reach the page as a cancel.
const RESPONSE_OK: u32 = 0;

/// A portal filter rule is `(kind, value)`; kind 0 is a shell glob and 1 a
/// MIME type. Extensions are all we are given, so every rule is a glob.
const RULE_GLOB: u32 = 0;

/// Trailing entry so a user can always reach a file the filters exclude —
/// the same escape hatch the Windows chooser appends.
const ALL_FILES: &str = "All files";

/// One entry of the portal's type dropdown: a label and its match rules,
/// matching the `a(sa(us))` the `filters` option is declared as.
type PortalFilter = (String, Vec<(u32, String)>);

/// The `a{sv}` options dict. `None` fields are left out entirely, which is
/// what the portal expects for an option that does not apply — a `multiple`
/// key on `SaveFile`, say, is not merely redundant but wrong.
#[derive(SerializeDict, Type)]
#[zvariant(signature = "a{sv}")]
struct ChooserOptions {
    handle_token: String,
    multiple: Option<bool>,
    directory: Option<bool>,
    filters: Option<Vec<PortalFilter>>,
    current_folder: Option<Vec<u8>>,
    current_name: Option<String>,
}

/// The `Response` signal's results dict. Unknown keys are ignored (the derive
/// only denies them when asked to), so a portal that grows new results stays
/// readable.
#[derive(DeserializeDict, Type, Default)]
#[zvariant(signature = "a{sv}")]
struct ChooserResults {
    uris: Option<Vec<String>>,
}

/// `OpenFile` covers every open flavour — the folder and multi-select
/// variants are options on it, not separate methods.
fn method_for(kind: FileDialogKind) -> &'static str {
    match kind {
        FileDialogKind::SaveFile => "SaveFile",
        _ => "OpenFile",
    }
}

/// The globs one extension contributes.
///
/// Portal filters are matched with a shell glob, which is case-sensitive, so
/// a lowercase pattern alone would hide `IMG_0001.JPG` from a `jpg` filter.
/// The uppercase form is emitted alongside it whenever the two differ.
fn globs_for_extension(ext: &str) -> Vec<String> {
    let ext = ext.trim().trim_start_matches('.');
    if ext.is_empty() {
        return Vec::new();
    }
    let lower = ext.to_ascii_lowercase();
    let upper = ext.to_ascii_uppercase();
    if lower == upper {
        vec![format!("*.{lower}")]
    } else {
        vec![format!("*.{lower}"), format!("*.{upper}")]
    }
}

/// One dropdown entry for `filter`, or `None` when it names no usable
/// extension. The label carries the globs so the entry says what it covers,
/// which is what the Windows chooser shows too.
fn portal_filter(filter: &FileDialogFilter) -> Option<PortalFilter> {
    let mut rules: Vec<(u32, String)> = Vec::new();
    // The label lists one glob per extension, not both cases of it: the
    // uppercase twin is there to make the match work, not to be read.
    let mut shown: Vec<String> = Vec::new();
    for ext in &filter.extensions {
        let globs = globs_for_extension(ext);
        let Some(first) = globs.first() else { continue };
        if rules.iter().any(|(_, g)| g == first) {
            continue; // The same extension in another case, already covered.
        }
        shown.push(first.clone());
        rules.extend(globs.into_iter().map(|g| (RULE_GLOB, g)));
    }
    if rules.is_empty() {
        return None;
    }
    let description = filter.description.trim();
    let globs = shown.join(", ");
    let label = if description.is_empty() {
        globs
    } else {
        format!("{description} ({globs})")
    };
    Some((label, rules))
}

/// Every dropdown entry for `filters`, with "All files" appended.
///
/// A folder chooser gets none: its filter is the directory itself, and a type
/// dropdown over folders only misleads. When nothing usable survives, the
/// list is left empty rather than reduced to a lone "All files" entry that
/// filters nothing.
fn portal_filters(kind: FileDialogKind, filters: &[FileDialogFilter]) -> Vec<PortalFilter> {
    if kind == FileDialogKind::OpenFolder {
        return Vec::new();
    }
    let mut out: Vec<PortalFilter> = filters.iter().filter_map(portal_filter).collect();
    if out.is_empty() {
        return out;
    }
    out.push((
        format!("{ALL_FILES} (*)"),
        vec![(RULE_GLOB, "*".to_string())],
    ));
    out
}

/// The object path the portal will answer a request on, given our connection's
/// unique name and the token we asked it to use.
///
/// The portal builds this from the sender's unique name with the leading `:`
/// dropped and every `.` turned into `_`, so that it is a legal object-path
/// element.
fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/request/{sender}/{token}")
}

/// A token unique to this request. Object-path elements admit only
/// `[A-Za-z0-9_]`, so the pid and a counter are all it carries.
fn handle_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("astrofin_{}_{n}", std::process::id())
}

/// A seed path names either a folder to open in, or a folder plus the file
/// name to pre-fill. Mirrors the macOS panel's reading of the same field.
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

/// `current_folder` is declared `ay` and the portal reads it as a C string,
/// so the trailing NUL is part of the value, not an artefact.
fn nul_terminated(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    let mut bytes = path.as_os_str().as_bytes().to_vec();
    bytes.push(0);
    bytes
}

/// The filesystem path behind one `file://` URI.
///
/// Decoding is done on bytes rather than on a `str`: a Linux file name is an
/// arbitrary byte string, and one that is not UTF-8 still has to survive the
/// round trip. Anything that is not a local `file://` URI — a `trash://` or
/// `smb://` entry the chooser may allow — has no path and is dropped.
fn path_from_uri(uri: &str) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let rest = uri.strip_prefix("file://")?;
    // "file:///tmp/a" leaves "/tmp/a", already the path. "file://localhost/tmp/a"
    // leaves "localhost/tmp/a": slicing from the first '/' drops the authority
    // and keeps the separator, giving "/tmp/a". A form with neither — a bare
    // "file://" — names nothing.
    let path = if rest.starts_with('/') {
        rest
    } else {
        &rest[rest.find('/')?..]
    };
    let decoded = percent_encoding::percent_decode_str(path).collect::<Vec<u8>>();
    if decoded.is_empty() {
        return None;
    }
    Some(PathBuf::from(OsString::from_vec(decoded)))
}

/// Every usable path in `uris`, or `None` when none survives — which the
/// caller treats exactly like a cancel.
fn paths_from_uris(uris: &[String]) -> Option<Vec<PathBuf>> {
    let paths: Vec<PathBuf> = uris.iter().filter_map(|u| path_from_uri(u)).collect();
    (!paths.is_empty()).then_some(paths)
}

/// The portal's `parent_window` string for an X11 toplevel. The window id is
/// lowercase hex with no `0x`, which is the form the portal parses.
pub fn x11_parent(window: u32) -> String {
    format!("x11:{window:x}")
}

/// The portal's `parent_window` string for a Wayland toplevel that has been
/// exported through `zxdg_exporter_v2`. The handle is opaque; it only has to
/// survive the trip.
pub fn wayland_parent(handle: &str) -> String {
    format!("wayland:{handle}")
}

/// Runs one portal exchange to completion: subscribe, call, wait, read.
fn exchange(
    kind: FileDialogKind,
    title: &str,
    parent: &str,
    default_path: Option<&Path>,
    filters: &[FileDialogFilter],
) -> zbus::Result<Option<Vec<PathBuf>>> {
    let conn = Connection::session()?;

    // Subscribed before the call is made, so a portal that answers at once
    // cannot answer into a gap. The rule deliberately omits the request path:
    // see the module comment.
    let rule = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(PORTAL_BUS)?
        .interface(REQUEST)?
        .member("Response")?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &conn, None)?;

    let token = handle_token();
    let unique = conn
        .unique_name()
        .map(|n| n.as_str().to_owned())
        .unwrap_or_default();
    let expected = request_path(&unique, &token);

    let (folder, name) = match default_path {
        Some(p) => seed_from(p),
        None => (None, None),
    };
    let portal_filters = portal_filters(kind, filters);
    let options = ChooserOptions {
        handle_token: token,
        multiple: kind.multiple().then_some(true),
        directory: (kind == FileDialogKind::OpenFolder).then_some(true),
        filters: (!portal_filters.is_empty()).then_some(portal_filters),
        current_folder: folder.as_deref().map(nul_terminated),
        current_name: name,
    };

    let reply = conn.call_method(
        Some(PORTAL_BUS),
        PORTAL_PATH,
        Some(FILE_CHOOSER),
        method_for(kind),
        &(parent, title, options),
    )?;
    let handle: OwnedObjectPath = reply.body().deserialize()?;
    if handle.as_str() != expected {
        // Only a portal ignoring `handle_token` gets here. Harmless — the
        // path we actually watch for is the one it just told us.
        tracing::debug!(
            "file dialog: portal chose request path {} (asked for {expected})",
            handle.as_str()
        );
    }

    for msg in signals {
        let msg = msg?;
        // Our own requests are the only ones this private connection can be
        // told about, but a second dialog open at the same time would be one
        // of them, so the path still decides.
        let is_ours = {
            let header = msg.header();
            header.path().is_some_and(|p| p.as_str() == handle.as_str())
        };
        if !is_ours {
            continue;
        }
        let (response, results): (u32, ChooserResults) = msg.body().deserialize()?;
        if response != RESPONSE_OK {
            return Ok(None);
        }
        return Ok(paths_from_uris(&results.uris.unwrap_or_default()));
    }
    // The iterator only ends when the connection does, which means the answer
    // is never coming.
    Ok(None)
}

/// Runs `answer` on its own thread and hands what it returns to `on_done`.
///
/// Split out of [`open`] so the contract either side of the thread boundary —
/// answered exactly once, or not taken at all — can be tested without a
/// session bus or a user to click the dialog.
fn spawn_answering<F>(
    on_done: Box<dyn FnOnce(Option<Vec<PathBuf>>) + Send>,
    answer: F,
) -> std::io::Result<()>
where
    F: FnOnce() -> Option<Vec<PathBuf>> + Send + 'static,
{
    std::thread::Builder::new()
        // Linux caps a thread name at 15 bytes; this is exactly 15.
        .name("astrofin-dialog".to_string())
        .spawn(move || on_done(answer()))
        .map(drop)
}

/// Takes the request and opens a portal chooser on its own thread.
///
/// `true` means `on_done` will run exactly once, from that thread. The only
/// `false` is a thread that would not start, in which case the closure — and
/// with it `on_done` — is dropped uncalled, which is the contract's way of
/// saying "not taken" and leaves the caller to cancel.
pub fn open(req: FileDialogRequest, parent: String) -> bool {
    let FileDialogRequest {
        kind,
        title,
        default_path,
        filters,
        on_done,
    } = req;
    let answer = move || {
        let title = title.unwrap_or_default();
        match exchange(kind, &title, &parent, default_path.as_deref(), &filters) {
            Ok(paths) => paths,
            Err(e) => {
                tracing::warn!("file dialog: portal exchange failed: {e}");
                None
            }
        }
    };
    match spawn_answering(on_done, answer) {
        Ok(()) => true,
        Err(e) => {
            tracing::error!("file dialog: could not start the portal thread: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(description: &str, extensions: &[&str]) -> FileDialogFilter {
        FileDialogFilter {
            description: description.to_string(),
            extensions: extensions.iter().map(|e| (*e).to_string()).collect(),
        }
    }

    fn globs(f: &PortalFilter) -> Vec<&str> {
        f.1.iter().map(|(_, g)| g.as_str()).collect()
    }

    #[test]
    fn only_a_save_dialog_uses_the_save_method() {
        assert_eq!(method_for(FileDialogKind::SaveFile), "SaveFile");
        for kind in [
            FileDialogKind::OpenFile,
            FileDialogKind::OpenFiles,
            FileDialogKind::OpenFolder,
        ] {
            assert_eq!(method_for(kind), "OpenFile", "{kind:?}");
        }
    }

    #[test]
    fn an_extension_contributes_both_cases_because_portal_globs_are_case_sensitive() {
        assert_eq!(globs_for_extension("jpg"), vec!["*.jpg", "*.JPG"]);
        // A leading dot and stray space are tolerated, as the ABI allows.
        assert_eq!(globs_for_extension(" .PNG "), vec!["*.png", "*.PNG"]);
        // A digit keeps its case-bearing neighbours: "7z" still has a "7Z".
        assert_eq!(globs_for_extension("7z"), vec!["*.7z", "*.7Z"]);
        // Nothing to upper-case means one glob, not a pair that is the same
        // string twice — the portal would match it a second time for nothing.
        assert_eq!(globs_for_extension("123"), vec!["*.123"]);
        assert!(globs_for_extension("  ").is_empty());
    }

    #[test]
    fn a_filter_label_lists_its_globs_once_per_extension() {
        let f = portal_filter(&filter("JPEG Image", &["jpg", "jpeg"])).expect("a filter");
        assert_eq!(f.0, "JPEG Image (*.jpg, *.jpeg)");
        assert_eq!(globs(&f), vec!["*.jpg", "*.JPG", "*.jpeg", "*.JPEG"]);
    }

    #[test]
    fn a_filter_without_a_description_is_labelled_by_its_globs() {
        let f = portal_filter(&filter("  ", &["png"])).expect("a filter");
        assert_eq!(f.0, "*.png");
    }

    #[test]
    fn a_filter_naming_no_usable_extension_is_dropped() {
        assert!(portal_filter(&filter("Empty", &[])).is_none());
        assert!(portal_filter(&filter("Blank", &[" ", "."])).is_none());
    }

    #[test]
    fn duplicate_extensions_collapse_to_one_rule() {
        let f = portal_filter(&filter("Image", &["png", "PNG", ".png"])).expect("a filter");
        assert_eq!(globs(&f), vec!["*.png", "*.PNG"]);
    }

    #[test]
    fn every_file_filter_list_ends_in_all_files() {
        let list = portal_filters(
            FileDialogKind::OpenFile,
            &[filter("PNG Image", &["png"]), filter("Junk", &[])],
        );
        assert_eq!(list.len(), 2, "the unusable filter is gone");
        assert_eq!(list[0].0, "PNG Image (*.png)");
        assert_eq!(list[1].0, "All files (*)");
        assert_eq!(globs(&list[1]), vec!["*"]);
    }

    #[test]
    fn a_folder_chooser_and_an_unusable_list_both_get_no_filters() {
        assert!(
            portal_filters(FileDialogKind::OpenFolder, &[filter("PNG", &["png"])]).is_empty(),
            "a folder chooser filters on being a folder"
        );
        assert!(
            portal_filters(FileDialogKind::OpenFile, &[filter("Junk", &[])]).is_empty(),
            "a lone All files entry filters nothing, so it is not offered"
        );
    }

    #[test]
    fn an_x11_parent_is_the_window_id_in_bare_lowercase_hex() {
        assert_eq!(x11_parent(0x3c0_0007), "x11:3c00007");
        // No `0x`, and no zero padding: the portal parses it as plain hex.
        assert_eq!(x11_parent(0), "x11:0");
        assert_eq!(x11_parent(u32::MAX), "x11:ffffffff");
    }

    #[test]
    fn a_wayland_parent_carries_the_exported_handle_verbatim() {
        assert_eq!(
            wayland_parent("stable_id_1234"),
            "wayland:stable_id_1234",
            "the handle is opaque and must not be reshaped"
        );
    }

    #[test]
    fn the_request_path_escapes_the_unique_name_the_way_the_portal_does() {
        assert_eq!(
            request_path(":1.238", "astrofin_42_0"),
            "/org/freedesktop/portal/desktop/request/1_238/astrofin_42_0"
        );
    }

    #[test]
    fn every_handle_token_is_a_legal_object_path_element_and_differs_from_the_last() {
        let (a, b) = (handle_token(), handle_token());
        assert_ne!(a, b);
        for token in [&a, &b] {
            assert!(
                token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "{token}"
            );
        }
    }

    #[test]
    fn a_seed_names_a_folder_to_open_in_or_a_name_to_pre_fill() {
        // A real directory is a folder seed with no name.
        let (folder, name) = seed_from(Path::new("/tmp"));
        assert_eq!(folder.as_deref(), Some(Path::new("/tmp")));
        assert_eq!(name, None);

        // A path that is not a directory seeds both halves.
        let (folder, name) = seed_from(Path::new("/tmp/astrofin-does-not-exist/poster.png"));
        assert_eq!(
            folder.as_deref(),
            Some(Path::new("/tmp/astrofin-does-not-exist"))
        );
        assert_eq!(name.as_deref(), Some("poster.png"));

        // A bare name has no folder to open in.
        let (folder, name) = seed_from(Path::new("poster.png"));
        assert_eq!(folder, None);
        assert_eq!(name.as_deref(), Some("poster.png"));
    }

    #[test]
    fn the_current_folder_option_carries_its_terminating_nul() {
        assert_eq!(nul_terminated(Path::new("/tmp/a")), b"/tmp/a\0");
    }

    #[test]
    fn a_file_uri_decodes_to_its_path() {
        assert_eq!(
            path_from_uri("file:///tmp/My%20Poster.png"),
            Some(PathBuf::from("/tmp/My Poster.png"))
        );
        // The authority form names the local host; the path is what matters.
        assert_eq!(
            path_from_uri("file://localhost/tmp/a.png"),
            Some(PathBuf::from("/tmp/a.png"))
        );
        // A percent escape that is not UTF-8 still round-trips as bytes.
        use std::os::unix::ffi::OsStringExt;
        assert_eq!(
            path_from_uri("file:///tmp/%FF.bin"),
            Some(PathBuf::from(std::ffi::OsString::from_vec(
                b"/tmp/\xFF.bin".to_vec()
            )))
        );
    }

    #[test]
    fn a_uri_that_names_no_local_file_has_no_path() {
        assert_eq!(path_from_uri("trash:///Poster.png"), None);
        assert_eq!(path_from_uri("smb://server/share/a.png"), None);
        assert_eq!(path_from_uri("file://"), None);
    }

    #[test]
    fn unusable_uris_read_as_a_cancel() {
        assert_eq!(
            paths_from_uris(&["file:///tmp/a.png".to_string()]),
            Some(vec![PathBuf::from("/tmp/a.png")])
        );
        // The non-local entry is dropped, the local one survives.
        assert_eq!(
            paths_from_uris(&["trash:///x".to_string(), "file:///tmp/b".to_string()]),
            Some(vec![PathBuf::from("/tmp/b")])
        );
        assert_eq!(paths_from_uris(&[]), None);
        assert_eq!(paths_from_uris(&["trash:///x".to_string()]), None);
    }

    /// The real portal is never driven from a test: it would need a session
    /// bus and a user to click the dialog. `spawn_answering` is the whole of
    /// `open` either side of that, so the contract is tested with a stub in
    /// the portal's place.
    #[test]
    fn a_taken_request_is_answered_exactly_once_off_the_calling_thread() {
        let (tx, rx) = std::sync::mpsc::channel();
        let calling_thread = std::thread::current().id();
        let picked = vec![PathBuf::from("/tmp/a.png")];
        let expected = picked.clone();

        spawn_answering(
            Box::new(move |paths| {
                let _ = tx.send((paths, std::thread::current().id()));
            }),
            move || Some(picked),
        )
        .expect("the answering thread starts");

        let (paths, answered_on) = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("on_done runs");
        assert_eq!(paths, Some(expected));
        assert_ne!(
            answered_on, calling_thread,
            "the portal must never block the caller"
        );
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "on_done must not run a second time"
        );
    }

    /// A real portal round trip, for verifying by hand what no automated test
    /// can reach: the D-Bus subscription, the live `OpenFile` call and the
    /// `Response` signal that carries the answer back. It opens an actual
    /// chooser and waits for a person to pick a file or cancel, so it is
    /// `#[ignore]`d and never runs in `just test` or CI.
    ///
    /// ```text
    /// cargo test --manifest-path src/Cargo.toml -p jfn-linux-util \
    ///     live_portal_round_trip -- --ignored --nocapture
    /// ```
    ///
    /// Watch the wire at the same time with:
    /// `dbus-monitor --session "interface='org.freedesktop.portal.FileChooser'"`
    #[test]
    #[ignore = "opens a real file chooser and needs a person to answer it"]
    fn live_portal_round_trip() {
        let (tx, rx) = std::sync::mpsc::channel();
        let taken = open(
            FileDialogRequest {
                kind: FileDialogKind::OpenFile,
                title: Some("Astrofin portal probe".to_string()),
                default_path: None,
                filters: vec![filter("Images", &["png", "jpg"])],
                on_done: Box::new(move |paths| {
                    let _ = tx.send(paths);
                }),
            },
            // A synthetic handle: the portal ignores one it cannot resolve and
            // simply does not parent, which is enough to see the argument go
            // out on the wire.
            wayland_parent("astrofin-probe-handle"),
        );
        assert!(taken, "the portal backend takes the request");
        match rx.recv_timeout(std::time::Duration::from_secs(120)) {
            Ok(Some(paths)) => println!("picked: {paths:?}"),
            Ok(None) => println!("cancelled, or the portal refused the request"),
            Err(e) => panic!("no answer within 120s: {e}"),
        }
    }

    #[test]
    fn a_cancel_reaches_on_done_as_no_paths() {
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_answering(
            Box::new(move |paths| {
                let _ = tx.send(paths);
            }),
            || None,
        )
        .expect("the answering thread starts");
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(10)),
            Ok(None),
            "a cancelled dialog still answers, with nothing"
        );
    }
}
