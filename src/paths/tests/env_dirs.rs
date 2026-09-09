//! `ASTROFIN_CONFIG_DIR` / `ASTROFIN_CACHE_DIR` resolution.
//!
//! Both variables are process-global and the directory getters *create* what
//! they return, so every test here takes one lock and points the variables at
//! a temp directory before calling anything that resolves them. Nothing in
//! this file may ever create or read the real profile.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use jfn_paths::{ENV_CACHE_DIR, ENV_CONFIG_DIR};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn set(key: &str, value: &OsStr) {
    // SAFETY: every writer of these variables in this binary holds ENV_LOCK,
    // and the crate under test only reads them.
    unsafe { std::env::set_var(key, value) };
}

fn clear(key: &str) {
    // SAFETY: as above.
    unsafe { std::env::remove_var(key) };
}

/// Holds the lock and both variables for the length of a test.
struct Env {
    _lock: MutexGuard<'static, ()>,
}

impl Env {
    fn with(config: &OsStr, cache: &OsStr) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set(ENV_CONFIG_DIR, config);
        set(ENV_CACHE_DIR, cache);
        Self { _lock: lock }
    }

    fn at(config: &Path, cache: &Path) -> Self {
        Self::with(config.as_os_str(), cache.as_os_str())
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        clear(ENV_CONFIG_DIR);
        clear(ENV_CACHE_DIR);
    }
}

#[test]
fn config_and_cache_dirs_come_from_the_environment_and_are_created() {
    let config = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("tempdir");
    let _env = Env::at(config.path(), cache.path());

    assert_eq!(jfn_paths::config_dir(), config.path());
    assert_eq!(jfn_paths::cache_dir(), cache.path());
    assert!(config.path().is_dir());
    assert!(cache.path().is_dir());
}

#[test]
fn an_env_dir_that_does_not_exist_yet_is_created_with_its_parents() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = tmp.path().join("deep").join("er").join("profile");
    let cache = tmp.path().join("deep").join("er").join("cache");
    let _env = Env::at(&config, &cache);

    assert_eq!(jfn_paths::config_dir(), config);
    assert!(config.is_dir());
    assert!(jfn_paths::cache_dir().is_dir());
}

#[test]
fn mpv_home_hangs_off_the_config_dir() {
    let config = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("tempdir");
    let _env = Env::at(config.path(), cache.path());

    let home = jfn_paths::mpv_home();
    assert_eq!(home, config.path().join("mpv"));
    assert!(home.is_dir());
}

/// The user shader folder is deliberately *not* created: its absence is the
/// signal that there is no override.
#[test]
fn user_shader_dir_tracks_the_config_dir_without_creating_it() {
    let config = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("tempdir");
    let _env = Env::at(config.path(), cache.path());

    let dir = jfn_paths::user_shader_dir();
    assert_eq!(dir, config.path().join("mpv").join("shaders"));
    assert!(!dir.exists());
}

/// clap hands an empty environment variable through as `Some("")`. Taking it
/// literally would put the profile in the working directory, so it is ignored
/// and the platform default stands.
#[test]
fn an_empty_env_value_is_ignored() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    clear(ENV_CONFIG_DIR);
    // Non-creating getters only: with no override in force these name the
    // real profile, which no test may materialise.
    let unset = jfn_paths::user_shader_dir();

    set(ENV_CONFIG_DIR, OsStr::new(""));
    let empty = jfn_paths::user_shader_dir();
    clear(ENV_CONFIG_DIR);

    assert_eq!(empty, unset);
    assert!(empty.is_absolute(), "{empty:?}");
}

/// A relative override is taken verbatim, which makes the profile follow the
/// process's working directory. Asserted, not endorsed — see the audit notes.
#[test]
fn a_relative_env_dir_is_taken_verbatim() {
    let _env = Env::with(OsStr::new("rel-profile"), OsStr::new("rel-cache"));
    assert_eq!(
        jfn_paths::user_shader_dir(),
        Path::new("rel-profile").join("mpv").join("shaders")
    );
}

#[test]
fn a_trailing_separator_names_the_same_directory() {
    let config = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("tempdir");
    let mut with_sep = config.path().as_os_str().to_os_string();
    with_sep.push(std::path::MAIN_SEPARATOR_STR);
    let _env = Env::with(&with_sep, cache.path().as_os_str());

    let target = jfn_paths::config_dir().join("probe");
    jfn_paths::write_atomic(&target, b"x").expect("write");
    assert!(config.path().join("probe").is_file());
}

/// `\\?\C:\...` skips Win32 path normalisation; the profile still resolves to
/// the same directory.
#[cfg(windows)]
#[test]
fn a_verbatim_unc_prefix_names_the_same_directory() {
    let config = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("tempdir");
    let verbatim = format!(r"\\?\{}", config.path().display());
    let _env = Env::with(OsStr::new(&verbatim), cache.path().as_os_str());

    let target = jfn_paths::config_dir().join("probe");
    jfn_paths::write_atomic(&target, b"x").expect("write");
    assert!(config.path().join("probe").is_file());
}

/// An explicit `--config-dir`/`--cache-dir` means the user picked the
/// location, so the legacy import must keep its hands off it.
#[test]
fn migrate_legacy_is_a_no_op_when_both_directories_are_overridden() {
    let config = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("tempdir");
    let _env = Env::at(config.path(), cache.path());

    let report = jfn_paths::migrate_legacy();

    assert!(!report.migrated());
    assert!(report.info_lines().is_empty(), "{:?}", report.info_lines());
    assert!(report.warnings().is_empty(), "{:?}", report.warnings());
    let entries: Vec<PathBuf> = std::fs::read_dir(config.path())
        .expect("read_dir")
        .flatten()
        .map(|e| e.path())
        .collect();
    assert!(entries.is_empty(), "{entries:?}");
}

#[test]
fn repair_mpv_conf_is_silent_without_a_legacy_path_to_repair() {
    let config = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("tempdir");
    let _env = Env::at(config.path(), cache.path());

    // No mpv.conf at all.
    assert!(jfn_paths::repair_mpv_conf().is_empty());

    // One that names nothing legacy.
    let conf = jfn_paths::mpv_home().join("mpv.conf");
    std::fs::write(&conf, "hwdec=auto-copy\n").expect("write");
    assert!(jfn_paths::repair_mpv_conf().is_empty());
    assert_eq!(
        std::fs::read_to_string(&conf).expect("read"),
        "hwdec=auto-copy\n"
    );
    assert!(!conf.with_file_name("mpv.conf.bak").exists());
}
