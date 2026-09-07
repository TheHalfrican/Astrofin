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
    if kind == "config" && new_dir.join("mpv").join("mpv.conf").is_file() {
        report.info.push(format!(
            "mpv config copied verbatim: absolute paths and the input-ipc-server pipe name inside \
             {} may still point at the legacy jellium-desktop folder until the mode switcher is \
             re-run from the new location",
            new_dir.join("mpv").join("mpv.conf").display()
        ));
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
    // SeCreateSymbolicLinkPrivilege. Copy through the link when it resolves to
    // a plain file, otherwise leave it out.
    match fs::metadata(from) {
        Ok(meta) if meta.is_file() => match fs::copy(from, to) {
            Ok(_) => counts.copied += 1,
            Err(e) => {
                counts.failed += 1;
                report.warn(format!(
                    "migration: copying link target of {}: {e}",
                    from.display()
                ));
            }
        },
        _ => {
            counts.skipped += 1;
            report.warn(format!(
                "migration: could not recreate symlink {} -> {}, skipped",
                from.display(),
                target.display()
            ));
        }
    }
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

    #[test]
    fn reports_mpv_conf_caveat_only_when_present() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("jellium-desktop");
        write(&legacy.join("mpv").join("mpv.conf"), "");
        let (_, report) = run(&legacy, &tmp.path().join("astrofin"));
        assert!(report.info_lines().iter().any(|l| l.contains("mpv config")));

        let tmp2 = tempfile::tempdir().expect("tempdir");
        let legacy2 = tmp2.path().join("jellium-desktop");
        write(&legacy2.join("settings.json"), "{}");
        let (_, report2) = run(&legacy2, &tmp2.path().join("astrofin"));
        assert!(
            !report2
                .info_lines()
                .iter()
                .any(|l| l.contains("mpv config"))
        );
    }
}
