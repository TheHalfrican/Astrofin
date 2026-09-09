use super::{APP_DIR_NAME, home};
use std::path::PathBuf;

pub(super) const DEFAULT_LOG_TO_FILE: bool = false;

/// `dirs` resolves `$HOME` through `getpwuid_r` before this fallback is
/// reached.
fn home_subdir(subdir: &str) -> PathBuf {
    PathBuf::from(home()).join(subdir)
}

pub(super) fn config_base() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| home_subdir(".config"))
}

pub(super) fn cache_base() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| home_subdir(".cache"))
}

pub(super) fn log_dir_path() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(|| home_subdir(".local/state"))
        .join(APP_DIR_NAME)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn the_config_base_is_an_absolute_xdg_directory() {
        let base = config_base();
        assert!(base.is_absolute(), "{}", base.display());
    }

    #[test]
    fn the_cache_base_is_an_absolute_xdg_directory() {
        let base = cache_base();
        assert!(base.is_absolute(), "{}", base.display());
    }

    #[test]
    fn the_log_directory_is_the_app_subdirectory_of_the_state_dir() {
        let dir = log_dir_path();
        assert!(dir.is_absolute(), "{}", dir.display());
        assert!(dir.ends_with(APP_DIR_NAME), "{}", dir.display());
    }

    #[test]
    fn home_subdir_hangs_the_name_off_the_home_directory() {
        let dir = home_subdir(".config");
        assert!(dir.starts_with(home()), "{}", dir.display());
        assert!(dir.ends_with(".config"), "{}", dir.display());
    }
}
