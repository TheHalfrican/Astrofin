//! The explicit `set_config_dir_override` / `set_cache_dir_override` half of
//! directory resolution.
//!
//! Those setters write a process-global slot that can never be cleared again,
//! so this binary holds exactly one test and walks the states in order. A
//! second test function would race it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::OsStr;
use std::path::PathBuf;

use jfn_paths::{ENV_CACHE_DIR, ENV_CONFIG_DIR};

fn set_env(key: &str, value: &OsStr) {
    // SAFETY: single-threaded test binary, one test function.
    unsafe { std::env::set_var(key, value) };
}

#[test]
fn an_explicit_override_beats_the_environment_and_ignores_an_empty_path() {
    let from_env = tempfile::tempdir().expect("tempdir");
    let from_flag = tempfile::tempdir().expect("tempdir");
    let cache_env = tempfile::tempdir().expect("tempdir");
    let cache_flag = tempfile::tempdir().expect("tempdir");
    set_env(ENV_CONFIG_DIR, from_env.path().as_os_str());
    set_env(ENV_CACHE_DIR, cache_env.path().as_os_str());

    // 1. Nothing set explicitly: the environment decides.
    assert_eq!(jfn_paths::config_dir(), from_env.path());
    assert_eq!(jfn_paths::cache_dir(), cache_env.path());

    // 2. An empty path is not a directory. Taking `--config-dir ""`
    //    literally would resolve the profile against the working directory
    //    here while the CEF helpers, which read the environment variable
    //    directly, kept the real one.
    assert_eq!(jfn_paths::set_config_dir_override(PathBuf::new()), None);
    assert_eq!(jfn_paths::set_cache_dir_override(PathBuf::new()), None);
    assert_eq!(jfn_paths::config_dir(), from_env.path());
    assert_eq!(jfn_paths::cache_dir(), cache_env.path());

    // 3. A real path wins over the environment, and the setter hands back the
    //    absolute path it stored — that, not the spelling on the command
    //    line, is what the browser process re-exports to the CEF helpers.
    assert_eq!(
        jfn_paths::set_config_dir_override(from_flag.path().to_path_buf()),
        Some(from_flag.path().to_path_buf())
    );
    assert_eq!(
        jfn_paths::set_cache_dir_override(cache_flag.path().to_path_buf()),
        Some(cache_flag.path().to_path_buf())
    );
    assert_eq!(jfn_paths::config_dir(), from_flag.path());
    assert_eq!(jfn_paths::cache_dir(), cache_flag.path());
    assert_eq!(
        jfn_paths::user_shader_dir(),
        from_flag.path().join("mpv").join("shaders")
    );
    assert_eq!(jfn_paths::mpv_home(), from_flag.path().join("mpv"));

    // 4. And the import still keeps its hands off an overridden location.
    let report = jfn_paths::migrate_legacy();
    assert!(!report.migrated());
    assert!(report.warnings().is_empty(), "{:?}", report.warnings());
}
