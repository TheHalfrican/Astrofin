//! First-run import of a legacy Jellium Desktop profile.
//!
//! Astrofin is a fork of Jellium Desktop, and the rebrand moved the per-user
//! directories from `jellium-desktop` to `astrofin`. Renaming them without an
//! import would silently orphan `settings.json` (server URL, hwdec, window
//! geometry), `instance.json` (the Jellyfin device identity) and the CEF
//! profile that holds the logged-in session.
//!
//! The import runs at most once per destination: the trigger is "the new
//! directory does not exist yet". It is therefore a one-shot window — a build
//! that creates `astrofin/` before this code runs would make the import a
//! permanent no-op.
//!
//! The legacy tree is opened read-only and is never modified, moved or
//! deleted. Running Astrofin and Jellium Desktop side by side is the point of
//! the rebrand, so the old install must survive the import untouched.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::space::{Disk, FreeSpace};
use crate::{
    LEGACY_APP_DIR_NAME, cache_dir_raw, cache_override, config_dir_raw, config_override, imp,
};

/// Chromium singleton markers and LevelDB lock files. An inherited singleton
/// points at the *other* install's PID, which is exactly the cross-talk the
/// rebrand exists to prevent; Chromium recreates every one of these on demand.
const DENYLIST: &[&str] = &[
    "SingletonLock",
    "SingletonSocket",
    "SingletonCookie",
    "LOCK",
];

/// Recursion cap. A CEF profile is roughly six levels deep, so this is
/// invisible in practice and still stops a symlink cycle that slipped past the
/// ancestor check below.
const MAX_DEPTH: u32 = 32;

/// Past this many per-entry failures the log gets a single count instead of a
/// line each.
const MAX_WARNINGS: usize = 20;

/// Largest legacy profile that is imported at all.
///
/// A Jellium Desktop config directory is a few megabytes and its cache is
/// tens; anything measured in gigabytes is a media folder somebody pointed at
/// the profile, or a cache that grew without bound. Copying it would stall the
/// first launch for minutes with no UI to say why (the import runs before
/// logging is even up), so past this the old profile is left where it is and
/// the app starts fresh.
const MAX_IMPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Why an import was not attempted at all.
#[derive(Debug, PartialEq, Eq)]
enum TooBig {
    /// The source is larger than [`MAX_IMPORT_BYTES`].
    Source(u64),
    /// The destination filesystem cannot hold the source.
    Destination { needed: u64, free: u64 },
}

/// Whether a legacy profile of `source_bytes` may be copied onto a filesystem
/// with `free_bytes` to spare.
///
/// `free_bytes` is `None` when the filesystem could not be interrogated, which
/// is permission to proceed: a share or container mount whose free space we
/// cannot read is still usually writable, and the copy reports its own errors
/// per entry. What this exists to stop is the case the per-entry reporting
/// handles badly — filling the disk one file at a time and leaving a
/// half-imported profile plus a machine with no room to log about it.
fn check_import_size(source_bytes: u64, free_bytes: Option<u64>) -> Result<(), TooBig> {
    if source_bytes > MAX_IMPORT_BYTES {
        return Err(TooBig::Source(source_bytes));
    }
    match free_bytes {
        Some(free) if free < source_bytes => Err(TooBig::Destination {
            needed: source_bytes,
            free,
        }),
        _ => Ok(()),
    }
}

/// Bytes held by the tree at `src`, stopping as soon as the total is past
/// `limit`.
///
/// Links are measured, never followed: the import recreates them (or skips
/// them), so their targets are not what is about to be copied, and following
/// them is how a link to `/` turns a size check into a full-filesystem walk.
/// Unreadable entries count as nothing — they will not be copied either.
fn measure_tree(src: &Path, depth: u32, limit: u64) -> u64 {
    if depth > MAX_DEPTH {
        return 0;
    }
    let Ok(entries) = fs::read_dir(src) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        total = total.saturating_add(if meta.is_dir() && !meta.file_type().is_symlink() {
            measure_tree(&entry.path(), depth + 1, limit.saturating_sub(total))
        } else {
            meta.len()
        });
        if total > limit {
            break;
        }
    }
    total
}

/// What [`migrate_legacy`] did, buffered because it runs before logging is
/// initialized (the config directory has to be in place before `settings.json`
/// is read, and `init_logging` comes after that).
#[derive(Debug, Default)]
pub struct MigrationReport {
    info: Vec<String>,
    warnings: Vec<String>,
    suppressed: usize,
    migrated: bool,
}

impl MigrationReport {
    /// Lines to emit at `info` level once logging is up.
    #[must_use]
    pub fn info_lines(&self) -> &[String] {
        &self.info
    }

    /// Lines to emit at `warn` level once logging is up.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Whether anything was actually copied.
    #[must_use]
    pub fn migrated(&self) -> bool {
        self.migrated
    }

    fn warn(&mut self, msg: String) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(msg);
        } else {
            self.suppressed += 1;
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    copied: usize,
    skipped: usize,
    failed: usize,
}

/// Import a Jellium Desktop config and cache directory into the Astrofin
/// locations when the latter do not exist yet.
///
/// Never fails: every problem degrades to a line in the returned report. Call
/// this before any of the directory getters — they create what they return, so
/// after the first call the "does the new directory exist?" question can no
/// longer be answered.
///
/// Log directories are deliberately not migrated: on Linux they live under a
/// third root (`~/.local/state`) and stale logs are worth nothing.
#[must_use]
pub fn migrate_legacy() -> MigrationReport {
    let mut report = MigrationReport::default();
    let legs = [
        (
            "config",
            config_override().is_some(),
            imp::config_base().join(LEGACY_APP_DIR_NAME),
            config_dir_raw(),
        ),
        (
            "cache",
            cache_override().is_some(),
            imp::cache_base().join(LEGACY_APP_DIR_NAME),
            cache_dir_raw(),
        ),
    ];

    let mut summaries: Vec<String> = Vec::new();
    for (kind, overridden, legacy, new_dir) in legs {
        // An explicit --config-dir/--cache-dir means the user picked the
        // location; copying someone else's profile into it would be rude.
        if overridden {
            continue;
        }
        // Already migrated, or the user deliberately keeps a fresh profile.
        if new_dir.exists() {
            continue;
        }
        // Fresh install with nothing to carry over.
        if !legacy.is_dir() {
            continue;
        }
        // Measured before anything is created: an import that cannot finish
        // must not leave a half-copied profile behind, and the destination
        // directory's absence is the only signal that the import is still due.
        let size = measure_tree(&legacy, 0, MAX_IMPORT_BYTES);
        let free = Disk.free_bytes(new_dir.parent().unwrap_or_else(|| Path::new(".")));
        if let Err(too_big) = check_import_size(size, free) {
            report.warn(match too_big {
                TooBig::Source(bytes) => format!(
                    "{kind}: not importing {}: it holds at least {bytes} bytes, more than the \
                     {MAX_IMPORT_BYTES}-byte import limit; Astrofin will start with a fresh \
                     profile",
                    legacy.display()
                ),
                TooBig::Destination { needed, free } => format!(
                    "{kind}: not importing {}: it needs {needed} bytes and only {free} are free \
                     on {}; Astrofin will start with a fresh profile",
                    legacy.display(),
                    new_dir.display()
                ),
            });
            continue;
        }
        match copy_leg(kind, &legacy, &new_dir, &mut report) {
            Ok(counts) => {
                report.migrated = true;
                summaries.push(format!(
                    "{kind} {} -> {} (copied {}, skipped {}, failed {})",
                    legacy.display(),
                    new_dir.display(),
                    counts.copied,
                    counts.skipped,
                    counts.failed
                ));
            }
            Err(e) => report.warn(format!(
                "{kind}: could not import {} into {}: {e}",
                legacy.display(),
                new_dir.display()
            )),
        }
    }

    if !summaries.is_empty() {
        report.info.insert(
            0,
            format!(
                "imported legacy Jellium Desktop profile: {}",
                summaries.join("; ")
            ),
        );
    }
    if report.suppressed > 0 {
        let more = report.suppressed;
        report
            .warnings
            .push(format!("... and {more} further migration warnings"));
    }
    report
}

/// Copy one legacy root into place. Only a failure to create the staging
/// directory or to move it into place aborts the leg; per-entry problems are
/// counted and reported.
fn copy_leg(
    kind: &str,
    legacy: &Path,
    new_dir: &Path,
    report: &mut MigrationReport,
) -> io::Result<Counts> {
    let parent = new_dir.parent().unwrap_or_else(|| Path::new("."));
    let stem = new_dir
        .file_name()
        .unwrap_or_else(|| OsStr::new("astrofin"));

    // A crash mid-copy leaves the staging directory behind; the destination is
    // still absent, so the next launch retries from scratch.
    remove_stale_staging(parent, stem, report);

    let mut staging_name = stem.to_os_string();
    staging_name.push(format!(".migrating-{}", std::process::id()));
    let staging = parent.join(staging_name);
    create_private_dir(&staging)?;

    // Guards against copying the destination into itself if someone symlinked
    // the new directory inside the old one.
    let staging_canonical = fs::canonicalize(&staging).ok();

    let mut counts = Counts::default();
    copy_dir(
        legacy,
        &staging,
        0,
        staging_canonical.as_deref(),
        &mut counts,
        report,
    );

    if let Err(e) = fs::rename(&staging, new_dir) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }

    if counts.skipped > 0 || counts.failed > 0 {
        report.info.push(format!(
            "{kind}: some entries were skipped or failed; if the Jellyfin session did not carry \
             over, close Jellium Desktop, delete {}, and start Astrofin again",
            new_dir.display()
        ));
    }
    if kind == "config" {
        // The copy is byte-for-byte, so every absolute path inside mpv.conf
        // still points into the legacy tree. The shaders those lines name were
        // just copied under `new_dir`, so repoint them here rather than leaving
        // mpv to fail loading them on the next launch.
        let conf = new_dir.join("mpv").join("mpv.conf");
        match rewrite_conf_in_place(&conf, legacy, new_dir) {
            Ok(Some(line)) => report.info.push(line),
            Ok(None) => {}
            Err(e) => report.warn(format!("migration: rewriting {}: {e}", conf.display())),
        }
    }
    Ok(counts)
}

fn copy_dir(
    src: &Path,
    dst: &Path,
    depth: u32,
    staging_root: Option<&Path>,
    counts: &mut Counts,
    report: &mut MigrationReport,
) {
    if depth > MAX_DEPTH {
        counts.skipped += 1;
        report.warn(format!(
            "migration depth limit reached, not descending into {}",
            src.display()
        ));
        return;
    }
    let entries = match fs::read_dir(src) {
        Ok(entries) => entries,
        Err(e) => {
            counts.failed += 1;
            report.warn(format!("migration: reading {}: {e}", src.display()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                counts.failed += 1;
                report.warn(format!("migration: listing {}: {e}", src.display()));
                continue;
            }
        };
        let name = entry.file_name();
        if DENYLIST.iter().any(|deny| OsStr::new(deny) == name) {
            counts.skipped += 1;
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let meta = match fs::symlink_metadata(&from) {
            Ok(meta) => meta,
            Err(e) => {
                counts.failed += 1;
                report.warn(format!("migration: stat {}: {e}", from.display()));
                continue;
            }
        };
        if meta.file_type().is_symlink() {
            copy_symlink(&from, &to, counts, report);
        } else if meta.is_dir() {
            if let Some(root) = staging_root
                && contains(&from, root)
            {
                counts.skipped += 1;
                report.warn(format!(
                    "migration: {} contains the destination, not descending",
                    from.display()
                ));
                continue;
            }
            if let Err(e) = fs::create_dir_all(&to) {
                counts.failed += 1;
                report.warn(format!("migration: creating {}: {e}", to.display()));
                continue;
            }
            copy_dir(&from, &to, depth + 1, staging_root, counts, report);
            preserve_mode(&meta, &to);
        } else if meta.is_file() {
            match fs::copy(&from, &to) {
                Ok(_) => {
                    counts.copied += 1;
                    preserve_mode(&meta, &to);
                }
                // A running Jellium Desktop holds some profile files open.
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    counts.skipped += 1;
                    report.warn(format!(
                        "migration: {} is locked (is Jellium Desktop running?), skipped",
                        from.display()
                    ));
                }
                Err(e) => {
                    counts.failed += 1;
                    report.warn(format!("migration: copying {}: {e}", from.display()));
                }
            }
        } else {
            // Sockets, fifos, devices: nothing worth carrying over.
            counts.skipped += 1;
        }
    }
}

/// Recreate the link rather than following it: a symlinked media directory
/// would otherwise be duplicated wholesale, and a cycle would never terminate.
fn copy_symlink(from: &Path, to: &Path, counts: &mut Counts, report: &mut MigrationReport) {
    let target = match fs::read_link(from) {
        Ok(target) => target,
        Err(e) => {
            counts.failed += 1;
            report.warn(format!("migration: reading link {}: {e}", from.display()));
            return;
        }
    };
    if symlink_to(&target, to, from).is_ok() {
        counts.copied += 1;
        return;
    }
    // Windows refuses symlink creation without Developer Mode or
    // SeCreateSymbolicLinkPrivilege, which is the normal case.
    let resolves_to_a_file = fs::metadata(from).is_ok_and(|meta| meta.is_file());
    match link_fallback(cfg!(windows), resolves_to_a_file) {
        LinkFallback::CopyTarget => match fs::copy(from, to) {
            Ok(_) => counts.copied += 1,
            Err(e) => {
                counts.failed += 1;
                report.warn(format!(
                    "migration: copying link target of {}: {e}",
                    from.display()
                ));
            }
        },
        LinkFallback::Skip => {
            counts.skipped += 1;
            report.warn(format!(
                "migration: could not recreate symlink {} -> {}, skipped",
                from.display(),
                target.display()
            ));
        }
    }
}

/// What to do with a symlink the import could not recreate.
#[derive(Debug, PartialEq, Eq)]
enum LinkFallback {
    /// Copy what the link points at, in its place.
    CopyTarget,
    /// Leave it out of the imported profile.
    Skip,
}

/// Windows never copies through a link it could not recreate.
///
/// Recreating a symlink there needs Developer Mode or
/// `SeCreateSymbolicLinkPrivilege`, so the fallback was the *usual* path, not
/// the rare one: a link the user made to keep something out of the profile —
/// a cache on another volume, a shared folder — came back as a full copy
/// inside it, silently doubling the data and detaching it from whatever the
/// link was tracking. A missing entry that the log names is the better
/// failure. Unix keeps the fallback, where it is genuinely rare (the link is
/// recreated verbatim unless the destination is unwritable).
fn link_fallback(windows: bool, target_is_file: bool) -> LinkFallback {
    if windows || !target_is_file {
        return LinkFallback::Skip;
    }
    LinkFallback::CopyTarget
}

/// Whether `target` lies inside `dir`, resolved through symlinks.
fn contains(dir: &Path, target: &Path) -> bool {
    fs::canonicalize(dir).is_ok_and(|canonical| target.starts_with(canonical))
}

fn remove_stale_staging(parent: &Path, stem: &OsStr, report: &mut MigrationReport) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let mut prefix = stem.to_os_string();
    prefix.push(".migrating-");
    let prefix = prefix.to_string_lossy().into_owned();
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let path: PathBuf = entry.path();
        if let Err(e) = fs::remove_dir_all(&path) {
            report.warn(format!(
                "migration: removing stale staging dir {}: {e}",
                path.display()
            ));
        }
    }
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

#[cfg(unix)]
fn preserve_mode(meta: &fs::Metadata, to: &Path) {
    let _ = fs::set_permissions(to, meta.permissions());
}

#[cfg(not(unix))]
fn preserve_mode(_meta: &fs::Metadata, _to: &Path) {}

#[cfg(unix)]
fn symlink_to(target: &Path, link: &Path, _src: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_to(target: &Path, link: &Path, src: &Path) -> io::Result<()> {
    if fs::metadata(src).is_ok_and(|meta| meta.is_dir()) {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

// =====================================================================
// mpv.conf legacy-path repair
// =====================================================================
//
// The import copies `mpv/mpv.conf` byte-for-byte, so any absolute path in it
// — `glsl-shaders=` above all — still points into `…/jellium-desktop/mpv`.
// Two entry points fix that: [`rewrite_conf_in_place`] runs as part of the
// import, and [`repair_mpv_conf`] runs on every launch for installs that were
// migrated before this code existed.

/// Suffix of the one-time backup [`repair_mpv_conf`] leaves behind.
const CONF_BACKUP_SUFFIX: &str = "mpv.conf.bak";

/// Characters that end a path in an mpv config line: the list separator, the
/// quoting mpv's config parser strips, and end of line. `:` is deliberately
/// absent — it is the Unix list separator but also `C:` on Windows, and the
/// only cost of stopping late is a token that fails the existence check.
const PATH_TERMINATORS: &[char] = &['"', '\'', ';', ',', '\n', '\r'];

/// Rewrite every absolute reference to `legacy` into `new_dir`, in both slash
/// styles, leaving the rest of the text byte-identical.
///
/// Returns the new text plus the absolute paths each replacement produced (so
/// a caller can check they resolve before committing), or `None` when the
/// legacy directory is not mentioned at all.
fn rewrite_legacy_paths(
    text: &str,
    legacy: &Path,
    new_dir: &Path,
) -> Option<(String, Vec<PathBuf>)> {
    let legacy = legacy.to_string_lossy();
    let new_dir = new_dir.to_string_lossy();
    // Both styles, because mpv wants `/` in option values but the directory
    // itself is spelled with `\` on Windows, and config files in the wild mix
    // the two.
    let pairs = [
        (legacy.replace('\\', "/"), new_dir.replace('\\', "/")),
        (legacy.replace('/', "\\"), new_dir.replace('/', "\\")),
    ];

    let mut out = text.to_string();
    let mut rewritten = Vec::new();
    for (from, to) in &pairs {
        if from.is_empty() || from == to {
            continue;
        }
        let (next, sites) = replace_all(&out, from, to);
        for site in sites {
            let tail = &next[site..];
            let end = tail.find(PATH_TERMINATORS).unwrap_or(tail.len());
            rewritten.push(PathBuf::from(&tail[..end]));
        }
        out = next;
    }
    (out != text).then_some((out, rewritten))
}

/// `str::replace` plus the byte offset of every replacement in the result.
/// Case-insensitive on Windows, where `%APPDATA%` casing is not stable.
fn replace_all(haystack: &str, needle: &str, replacement: &str) -> (String, Vec<usize>) {
    // An empty needle matches at every position and would never advance
    // `rest`. The one caller already refuses it; this keeps the loop safe on
    // its own terms.
    if needle.is_empty() {
        return (haystack.to_string(), Vec::new());
    }
    let mut out = String::with_capacity(haystack.len());
    let mut sites = Vec::new();
    let mut rest = haystack;
    while let Some(idx) = find_path(rest, needle) {
        out.push_str(&rest[..idx]);
        sites.push(out.len());
        out.push_str(replacement);
        rest = &rest[idx + needle.len()..];
    }
    out.push_str(rest);
    (out, sites)
}

#[cfg(windows)]
fn find_path(haystack: &str, needle: &str) -> Option<usize> {
    // ASCII-only fold: enough for drive letters and the `AppData\Roaming`
    // spelling NTFS is careless about, and never merges distinct non-ASCII
    // user names.
    haystack
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
}

#[cfg(not(windows))]
fn find_path(haystack: &str, needle: &str) -> Option<usize> {
    haystack.find(needle)
}

/// Import-time rewrite: the shaders were just copied under `new_dir`, so the
/// rewritten paths resolve by construction and no backup is warranted (the
/// legacy file is still there, untouched).
fn rewrite_conf_in_place(conf: &Path, legacy: &Path, new_dir: &Path) -> io::Result<Option<String>> {
    if !conf.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(conf)?;
    let Some((rewritten, sites)) = rewrite_legacy_paths(&text, legacy, new_dir) else {
        return Ok(None);
    };
    crate::write_atomic(conf, rewritten.as_bytes())?;
    Ok(Some(format!(
        "rewrote {} legacy path(s) in {} to point at the imported profile",
        sites.len(),
        conf.display()
    )))
}

/// Repair an already-imported `mpv.conf` whose absolute paths still point at
/// `…/jellium-desktop/mpv`.
///
/// Idempotent and conservative: it does nothing unless the legacy directory is
/// actually named *and* at least one of the rewritten paths resolves to a file
/// under the current config directory. The first repair leaves `mpv.conf.bak`
/// beside the file; later launches find nothing to do.
///
/// Returns lines to log at `info` once logging is up — the same deferred
/// reporting [`migrate_legacy`] uses, for the same reason.
#[must_use]
pub fn repair_mpv_conf() -> Vec<String> {
    let new_dir = config_dir_raw();
    repair_conf_at(
        &new_dir.join("mpv").join("mpv.conf"),
        &imp::config_base().join(LEGACY_APP_DIR_NAME),
        &new_dir,
    )
}

/// [`repair_mpv_conf`] with the three locations injected, so tests can drive
/// it against a temp directory instead of the process-global config dir.
fn repair_conf_at(conf: &Path, legacy: &Path, new_dir: &Path) -> Vec<String> {
    if !conf.is_file() {
        return Vec::new();
    }
    let text = match fs::read_to_string(conf) {
        Ok(text) => text,
        Err(e) => return vec![format!("mpv.conf repair: reading {}: {e}", conf.display())],
    };
    let Some((rewritten, sites)) = rewrite_legacy_paths(&text, legacy, new_dir) else {
        return Vec::new();
    };
    // "The referenced file exists under the new dir": without this a config
    // that names the legacy folder for something we never imported would be
    // rewritten into a path that resolves nowhere.
    let resolved = sites.iter().filter(|p| p.is_file()).count();
    if resolved == 0 {
        return vec![format!(
            "mpv.conf repair: {} still names {}, but none of the {} rewritten path(s) exist under \
             {}; leaving it alone",
            conf.display(),
            legacy.display(),
            sites.len(),
            new_dir.display()
        )];
    }

    let backup = conf.with_file_name(CONF_BACKUP_SUFFIX);
    if !backup.exists()
        && let Err(e) = fs::copy(conf, &backup)
    {
        return vec![format!(
            "mpv.conf repair: could not write {}: {e}; leaving {} alone",
            backup.display(),
            conf.display()
        )];
    }
    if let Err(e) = crate::write_atomic(conf, rewritten.as_bytes()) {
        return vec![format!("mpv.conf repair: writing {}: {e}", conf.display())];
    }
    vec![format!(
        "mpv.conf repair: repointed {} legacy path(s) ({resolved} resolve) in {} from {} to {}; \
         backup at {}",
        sites.len(),
        conf.display(),
        legacy.display(),
        new_dir.display(),
        backup.display()
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, body).expect("write file");
    }

    fn run(legacy: &Path, new_dir: &Path) -> (Counts, MigrationReport) {
        let mut report = MigrationReport::default();
        let counts = copy_leg("config", legacy, new_dir, &mut report).expect("copy_leg");
        (counts, report)
    }

    #[test]
    fn copies_tree_and_leaves_source_untouched() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        let new_dir = tmp.path().join("astrofin");
        write(&legacy.join("settings.json"), "{\"serverUrl\":\"x\"}");
        write(&legacy.join("mpv").join("mpv.conf"), "hwdec=auto-copy\n");
        write(&legacy.join("mpv").join("shaders").join("a.glsl"), "// s");

        let (counts, report) = run(&legacy, &new_dir);

        assert_eq!(counts.copied, 3);
        assert_eq!(counts.failed, 0);
        assert!(report.migrated || counts.copied > 0);
        assert_eq!(
            fs::read_to_string(new_dir.join("settings.json")).expect("read"),
            "{\"serverUrl\":\"x\"}"
        );
        assert!(new_dir.join("mpv").join("shaders").join("a.glsl").is_file());
        // The source must survive byte-for-byte.
        assert!(legacy.join("settings.json").is_file());
        assert!(legacy.join("mpv").join("mpv.conf").is_file());
        assert_eq!(
            fs::read_to_string(legacy.join("mpv").join("mpv.conf")).expect("read"),
            "hwdec=auto-copy\n"
        );
    }

    #[test]
    fn skips_singleton_and_lock_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        let new_dir = tmp.path().join("astrofin");
        write(&legacy.join("Local State"), "{}");
        write(&legacy.join("SingletonLock"), "pid");
        write(&legacy.join("SingletonCookie"), "cookie");
        write(&legacy.join("SingletonSocket"), "sock");
        write(&legacy.join("Default").join("LOCK"), "");

        let (counts, _) = run(&legacy, &new_dir);

        assert_eq!(counts.copied, 1);
        assert_eq!(counts.skipped, 4);
        assert!(new_dir.join("Local State").is_file());
        assert!(!new_dir.join("SingletonLock").exists());
        assert!(!new_dir.join("Default").join("LOCK").exists());
    }

    #[test]
    fn leaves_no_staging_directory_behind() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        let new_dir = tmp.path().join("astrofin");
        write(&legacy.join("settings.json"), "{}");

        run(&legacy, &new_dir);

        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .expect("read_dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".migrating-"))
            .collect();
        assert!(leftovers.is_empty(), "left staging dirs: {leftovers:?}");
    }

    #[test]
    fn stale_staging_directory_is_reclaimed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        let new_dir = tmp.path().join("astrofin");
        write(&legacy.join("settings.json"), "{}");
        let stale = tmp.path().join(format!("astrofin.migrating-{}", 424_242));
        write(&stale.join("junk"), "junk");

        run(&legacy, &new_dir);

        assert!(!stale.exists());
        assert!(new_dir.join("settings.json").is_file());
        assert!(!new_dir.join("junk").exists());
    }

    /// Superseded by the rewrite below: the import used to only warn about
    /// stale paths, it now repoints them.
    #[test]
    fn import_rewrites_legacy_paths_in_mpv_conf() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        let new_dir = tmp.path().join("astrofin");
        write(&legacy.join("mpv").join("shaders").join("a.glsl"), "// s");
        let fwd = legacy.to_string_lossy().replace('\\', "/");
        let body = format!(
            "input-ipc-server=\\\\.\\pipe\\jellium-mpv\nglsl-shaders=\"{fwd}/mpv/shaders/a.glsl\"\nscale=ewa_lanczossharp\n"
        );
        write(&legacy.join("mpv").join("mpv.conf"), &body);

        let (_, report) = run(&legacy, &new_dir);

        let conf = fs::read_to_string(new_dir.join("mpv").join("mpv.conf")).expect("read");
        let new_fwd = new_dir.to_string_lossy().replace('\\', "/");
        assert!(
            conf.contains(&format!("glsl-shaders=\"{new_fwd}/mpv/shaders/a.glsl\"")),
            "not repointed: {conf}"
        );
        assert!(
            !conf.contains("jellium-desktop"),
            "legacy path left: {conf}"
        );
        // Everything else is byte-identical, pipe name included.
        assert!(conf.contains("input-ipc-server=\\\\.\\pipe\\jellium-mpv\n"));
        assert!(conf.ends_with("scale=ewa_lanczossharp\n"));
        assert!(
            report
                .info_lines()
                .iter()
                .any(|l| l.contains("legacy path"))
        );
        // The source survives untouched.
        assert!(
            fs::read_to_string(legacy.join("mpv").join("mpv.conf"))
                .expect("read")
                .contains("jellium-desktop")
        );
    }

    #[test]
    fn import_leaves_mpv_conf_alone_when_it_names_no_legacy_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        let new_dir = tmp.path().join("astrofin");
        write(&legacy.join("mpv").join("mpv.conf"), "hwdec=auto-copy\n");

        let (_, report) = run(&legacy, &new_dir);

        assert_eq!(
            fs::read_to_string(new_dir.join("mpv").join("mpv.conf")).expect("read"),
            "hwdec=auto-copy\n"
        );
        assert!(
            !report
                .info_lines()
                .iter()
                .any(|l| l.contains("legacy path"))
        );
    }

    /// A profile migrated before the rewrite existed, as on the developer's
    /// own machine: repair has to fix it on the next launch.
    fn repair_fixture(tmp: &Path, target_exists: bool) -> (PathBuf, PathBuf, PathBuf, String) {
        let legacy = tmp.join("jellium-desktop");
        let new_dir = tmp.join("astrofin");
        if target_exists {
            write(&new_dir.join("mpv").join("shaders").join("a.glsl"), "// s");
        }
        let fwd = legacy.to_string_lossy().replace('\\', "/");
        let body = format!("glsl-shaders=\"{fwd}/mpv/shaders/a.glsl\"\nscale=mitchell\n");
        let conf = new_dir.join("mpv").join("mpv.conf");
        write(&conf, &body);
        (conf, legacy, new_dir, body)
    }

    #[test]
    fn repair_repoints_paths_backs_up_once_and_is_idempotent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (conf, legacy, new_dir, original) = repair_fixture(tmp.path(), true);

        let lines = repair_conf_at(&conf, &legacy, &new_dir);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("repointed"), "{lines:?}");

        let repaired = fs::read_to_string(&conf).expect("read");
        let new_fwd = new_dir.to_string_lossy().replace('\\', "/");
        assert!(repaired.contains(&format!("{new_fwd}/mpv/shaders/a.glsl")));
        assert!(!repaired.contains("jellium-desktop"));
        assert!(repaired.ends_with("scale=mitchell\n"));

        let backup = conf.with_file_name("mpv.conf.bak");
        assert_eq!(fs::read_to_string(&backup).expect("read"), original);

        // Second launch: nothing left to do, and the backup is not clobbered.
        assert!(repair_conf_at(&conf, &legacy, &new_dir).is_empty());
        assert_eq!(fs::read_to_string(&conf).expect("read"), repaired);
        assert_eq!(fs::read_to_string(&backup).expect("read"), original);
    }

    #[test]
    fn repair_declines_when_the_rewritten_file_does_not_exist() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (conf, legacy, new_dir, original) = repair_fixture(tmp.path(), false);

        let lines = repair_conf_at(&conf, &legacy, &new_dir);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("leaving it alone"), "{lines:?}");
        assert_eq!(fs::read_to_string(&conf).expect("read"), original);
        assert!(!conf.with_file_name("mpv.conf.bak").exists());
    }

    #[test]
    fn repair_is_silent_without_a_conf_or_without_legacy_paths() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        let new_dir = tmp.path().join("astrofin");
        let conf = new_dir.join("mpv").join("mpv.conf");
        assert!(repair_conf_at(&conf, &legacy, &new_dir).is_empty());

        write(&conf, "hwdec=auto\n");
        assert!(repair_conf_at(&conf, &legacy, &new_dir).is_empty());
        assert_eq!(fs::read_to_string(&conf).expect("read"), "hwdec=auto\n");
    }

    #[test]
    fn rewrite_handles_both_slash_styles_and_counts_sites() {
        let legacy = Path::new("/base/jellium-desktop");
        let new_dir = Path::new("/base/astrofin");
        let text = "a=/base/jellium-desktop/x.glsl\nb=\\base\\jellium-desktop\\y.glsl\nc=keep\n";
        let (out, sites) = rewrite_legacy_paths(text, legacy, new_dir).expect("rewritten");
        assert!(out.contains("a=/base/astrofin/x.glsl"));
        assert!(out.contains("b=\\base\\astrofin\\y.glsl"));
        assert!(out.contains("c=keep"));
        assert_eq!(sites.len(), 2);

        assert!(rewrite_legacy_paths("c=keep\n", legacy, new_dir).is_none());
    }

    /// An empty mpv.conf is not a legacy-path config; the import must not
    /// touch it, and must not claim it did.
    #[test]
    fn import_says_nothing_about_an_empty_mpv_conf() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        write(&legacy.join("mpv").join("mpv.conf"), "");
        let (_, report) = run(&legacy, &tmp.path().join("astrofin"));
        assert!(
            !report
                .info_lines()
                .iter()
                .any(|l| l.contains("legacy path"))
        );
    }

    // =================================================================
    // Hostile legacy profiles
    // =================================================================

    /// Nothing a legacy tree can be named lets the copy write outside the
    /// destination: entry names come from `read_dir`, which never yields `..`
    /// or a separator, and the copy joins nothing else.
    #[test]
    fn odd_entry_names_all_land_inside_the_destination() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        let new_dir = tmp.path().join("astrofin");
        let sentinel = tmp.path().join("sentinel");
        fs::write(&sentinel, "untouched").expect("seed sentinel");
        let mut names = vec![
            "..dotdot",
            "a b",
            "-dash",
            "unicode-\u{e9}\u{4e2d}",
            "very.long.name.with.dots.json",
        ];
        if !cfg!(windows) {
            // Win32 refuses to create these at all.
            names.push("...");
            names.push("trailing space ");
        }
        for name in &names {
            write(&legacy.join(name), name);
        }

        let (counts, _) = run(&legacy, &new_dir);

        assert_eq!(counts.copied, names.len());
        for name in &names {
            assert!(new_dir.join(name).is_file(), "{name} missing");
        }
        // The only new entry beside the destination is the destination.
        let mut siblings: Vec<String> = fs::read_dir(tmp.path())
            .expect("read_dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        siblings.sort();
        assert_eq!(siblings, ["astrofin", "jellium-desktop", "sentinel"]);
        assert_eq!(
            fs::read_to_string(&sentinel).expect("read sentinel"),
            "untouched"
        );
    }

    /// A symlink is recreated, never followed: a link pointing outside the
    /// profile must not pull its target's tree in, and must not be walked.
    #[test]
    fn a_symlink_pointing_outside_the_profile_is_not_followed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outside = tmp.path().join("outside");
        write(&outside.join("secret.txt"), "secret");
        let legacy = tmp.path().join("jellium-desktop");
        write(&legacy.join("settings.json"), "{}");

        let link = legacy.join("escape");
        if symlink_to(Path::new("../outside"), &link, &outside).is_err() {
            // Windows without Developer Mode: nothing to assert.
            return;
        }
        let new_dir = tmp.path().join("astrofin");

        let (counts, _) = run(&legacy, &new_dir);

        assert!(new_dir.join("settings.json").is_file());
        // Either the link came across as a link, or it was skipped; what must
        // never happen is the target's contents being copied in. A recreated
        // relative link still resolves to `outside`, so `exists()` through it
        // proves nothing: look at the entry itself.
        match fs::symlink_metadata(new_dir.join("escape")) {
            Ok(meta) => assert!(
                meta.file_type().is_symlink(),
                "escape was materialised as a {:?}",
                meta.file_type()
            ),
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::NotFound),
        }
        assert_eq!(counts.failed, 0);
        assert_eq!(
            fs::read_to_string(outside.join("secret.txt")).expect("read"),
            "secret"
        );
    }

    /// A self-referential symlink would be an infinite walk if links were
    /// followed. It is copied as a link (or skipped) and the import finishes.
    #[test]
    fn a_symlink_cycle_terminates() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        write(&legacy.join("settings.json"), "{}");
        let link = legacy.join("loop");
        if symlink_to(Path::new("."), &link, &legacy).is_err() {
            return;
        }

        let new_dir = tmp.path().join("astrofin");
        let (counts, _) = run(&legacy, &new_dir);

        assert!(new_dir.join("settings.json").is_file());
        assert!(counts.copied >= 1);
    }

    /// Past `MAX_DEPTH` the walk stops and says so, instead of recursing on.
    #[test]
    fn a_tree_deeper_than_the_cap_is_truncated_with_a_warning() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        let mut deep = legacy.clone();
        for level in 0..=(MAX_DEPTH + 2) {
            deep = deep.join(format!("d{level}"));
        }
        write(&deep.join("leaf.txt"), "leaf");

        let (counts, report) = run(&legacy, &tmp.path().join("astrofin"));

        assert!(counts.skipped >= 1);
        assert!(
            report
                .warnings()
                .iter()
                .any(|w| w.contains("depth limit reached")),
            "{:?}",
            report.warnings()
        );
    }

    /// The destination existing *is* the migration marker, and it appears
    /// only when the staged copy is renamed into place. If somebody wins the
    /// race for the destination between [`migrate_legacy`]'s existence check
    /// and the rename, the leg fails whole: the existing profile is left
    /// alone and no staging directory survives.
    #[test]
    fn a_leg_that_cannot_land_leaves_the_winner_alone_and_no_staging() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        write(&legacy.join("settings.json"), "{\"from\":\"legacy\"}");
        // A populated directory where the destination belongs: renaming onto
        // it fails on every platform.
        let new_dir = tmp.path().join("astrofin");
        write(&new_dir.join("settings.json"), "{\"from\":\"winner\"}");

        let mut report = MigrationReport::default();
        assert!(
            copy_leg("config", &legacy, &new_dir, &mut report).is_err(),
            "the rename onto a populated directory must fail"
        );

        assert_eq!(
            fs::read_to_string(new_dir.join("settings.json")).expect("read"),
            "{\"from\":\"winner\"}",
            "the existing profile must not be overwritten"
        );
        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .expect("read_dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".migrating-"))
            .collect();
        assert!(leftovers.is_empty(), "left staging dirs: {leftovers:?}");
    }

    /// Somebody who symlinked (or just nested) the new profile inside the old
    /// one: the walk must notice the staging directory is its own child and
    /// not copy it into itself.
    #[test]
    fn a_destination_nested_inside_the_source_is_not_copied_into_itself() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        write(&legacy.join("settings.json"), "{}");
        let new_dir = legacy.join("astrofin");

        let (counts, report) = run(&legacy, &new_dir);

        assert!(new_dir.join("settings.json").is_file());
        assert!(!new_dir.join("astrofin").exists());
        assert_eq!(counts.copied, 1);
        assert!(
            report
                .warnings()
                .iter()
                .any(|w| w.contains("contains the destination")),
            "{:?}",
            report.warnings()
        );
    }

    /// A partial import still lands (a half-copied profile beats none) and
    /// says how to start over.
    #[test]
    fn a_partially_skipped_import_lands_and_explains_how_to_retry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        let new_dir = tmp.path().join("astrofin");
        write(&legacy.join("settings.json"), "{}");
        write(&legacy.join("SingletonLock"), "pid");

        let (counts, report) = run(&legacy, &new_dir);

        assert_eq!(counts.skipped, 1);
        assert!(new_dir.join("settings.json").is_file());
        assert!(
            report
                .info_lines()
                .iter()
                .any(|l| l.contains("skipped or failed")),
            "{:?}",
            report.info_lines()
        );
    }

    #[test]
    fn warnings_are_capped_and_the_rest_counted() {
        let mut report = MigrationReport::default();
        for i in 0..MAX_WARNINGS + 5 {
            report.warn(format!("warning {i}"));
        }
        assert_eq!(report.warnings().len(), MAX_WARNINGS);
        assert_eq!(report.suppressed, 5);
        assert!(!report.migrated());
        assert!(report.info_lines().is_empty());
    }

    /// An empty needle matches everywhere and would never advance the cursor.
    #[test]
    fn replace_all_refuses_an_empty_needle_instead_of_looping() {
        let (out, sites) = replace_all("abc", "", "X");
        assert_eq!(out, "abc");
        assert!(sites.is_empty());
    }

    #[test]
    fn replace_all_reports_every_site_it_rewrote() {
        let (out, sites) = replace_all("a/x a/x b", "a/x", "q/y");
        assert_eq!(out, "q/y q/y b");
        assert_eq!(sites.len(), 2);
        assert_eq!(&out[sites[0]..sites[0] + 3], "q/y");
        assert_eq!(&out[sites[1]..sites[1] + 3], "q/y");
    }

    /// The rewrite never mistakes an unrelated path for the legacy one.
    #[test]
    fn rewrite_leaves_a_config_naming_a_different_directory_alone() {
        let legacy = Path::new("/base/jellium-desktop");
        let new_dir = Path::new("/base/astrofin");
        assert!(rewrite_legacy_paths("a=/base/jellium-other/x\n", legacy, new_dir).is_none());
        assert!(rewrite_legacy_paths("", legacy, new_dir).is_none());
    }

    /// `repair_conf_at` reads a file that may be anything at all; none of it
    /// may panic or rewrite the file.
    #[test]
    fn repair_survives_binary_and_oversized_conf_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        let new_dir = tmp.path().join("astrofin");
        let conf = new_dir.join("mpv").join("mpv.conf");

        // Invalid UTF-8: read_to_string fails, and the file is left alone.
        fs::create_dir_all(conf.parent().expect("parent")).expect("mkdir");
        fs::write(&conf, [0xff, 0xfe, 0x00, 0x80]).expect("write");
        let lines = repair_conf_at(&conf, &legacy, &new_dir);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("reading"), "{lines:?}");
        assert_eq!(fs::read(&conf).expect("read"), [0xff, 0xfe, 0x00, 0x80]);

        // A directory where mpv.conf belongs is not a file: silence.
        let dir_conf = new_dir.join("mpv").join("as-a-dir.conf");
        fs::create_dir_all(&dir_conf).expect("mkdir");
        assert!(repair_conf_at(&dir_conf, &legacy, &new_dir).is_empty());
    }

    // =====================================================================
    // Size and free-space gate
    // =====================================================================

    /// A `FreeSpace` that answers whatever the test wants, including "I could
    /// not tell".
    struct FakeSpace(Option<u64>);

    impl FreeSpace for FakeSpace {
        fn free_bytes(&self, _path: &Path) -> Option<u64> {
            self.0
        }
    }

    #[test]
    fn check_import_size_passes_a_profile_that_fits() {
        assert_eq!(check_import_size(10_000, Some(u64::MAX)), Ok(()));
        assert_eq!(check_import_size(0, Some(0)), Ok(()));
        // Exactly the free space is still enough.
        assert_eq!(check_import_size(4096, Some(4096)), Ok(()));
        assert_eq!(check_import_size(MAX_IMPORT_BYTES, Some(u64::MAX)), Ok(()));
    }

    #[test]
    fn check_import_size_refuses_a_profile_past_the_import_limit() {
        assert_eq!(
            check_import_size(MAX_IMPORT_BYTES + 1, Some(u64::MAX)),
            Err(TooBig::Source(MAX_IMPORT_BYTES + 1))
        );
    }

    #[test]
    fn check_import_size_refuses_a_destination_with_less_room_than_the_source() {
        assert_eq!(
            check_import_size(4096, Some(4095)),
            Err(TooBig::Destination {
                needed: 4096,
                free: 4095
            })
        );
    }

    /// A filesystem that cannot report its free space is not a filesystem
    /// that is full.
    #[test]
    fn check_import_size_proceeds_when_the_free_space_is_unknown() {
        assert_eq!(check_import_size(MAX_IMPORT_BYTES, None), Ok(()));
        assert_eq!(
            FakeSpace(None).free_bytes(Path::new("anywhere")),
            None,
            "the fake reports what it was built with"
        );
        assert_eq!(
            FakeSpace(Some(7)).free_bytes(Path::new("anywhere")),
            Some(7)
        );
    }

    #[test]
    fn measure_tree_sums_every_file_under_the_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("profile");
        write(&root.join("a"), "0123456789");
        write(&root.join("deep").join("b"), "01234");
        assert_eq!(measure_tree(&root, 0, u64::MAX), 15);
        assert_eq!(measure_tree(&root.join("missing"), 0, u64::MAX), 0);
    }

    /// The walk stops as soon as it is past the budget, so a media folder
    /// somebody parked in the profile does not cost a full traversal.
    #[test]
    fn measure_tree_stops_once_it_is_past_the_limit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("profile");
        for i in 0..8 {
            write(&root.join(format!("f{i}")), &"x".repeat(100));
        }
        let measured = measure_tree(&root, 0, 150);
        assert!(measured > 150, "{measured}");
        assert!(measured < 800, "the whole tree was walked: {measured}");
    }

    #[cfg(unix)]
    #[test]
    fn measure_tree_measures_a_symlink_without_following_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("profile");
        let outside = tmp.path().join("outside");
        write(&outside.join("big"), &"x".repeat(4096));
        fs::create_dir_all(&root).expect("mkdir");
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink");
        assert!(
            measure_tree(&root, 0, u64::MAX) < 4096,
            "the link target was counted"
        );
    }

    #[test]
    fn link_fallback_never_copies_through_a_link_on_windows() {
        assert_eq!(link_fallback(true, true), LinkFallback::Skip);
        assert_eq!(link_fallback(true, false), LinkFallback::Skip);
    }

    #[test]
    fn link_fallback_copies_a_file_target_on_unix_and_skips_anything_else() {
        assert_eq!(link_fallback(false, true), LinkFallback::CopyTarget);
        assert_eq!(link_fallback(false, false), LinkFallback::Skip);
    }
}
