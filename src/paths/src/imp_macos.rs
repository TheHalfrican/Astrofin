use super::{APP_DIR_NAME, env_or, home};
use std::path::PathBuf;

pub(super) const DEFAULT_LOG_TO_FILE: bool = true;

pub(super) fn config_base() -> PathBuf {
    PathBuf::from(env_or("XDG_CONFIG_HOME", &format!("{}/.config", home())))
}

pub(super) fn cache_base() -> PathBuf {
    PathBuf::from(home()).join("Library/Caches")
}

pub(super) fn log_dir_path() -> PathBuf {
    PathBuf::from(home())
        .join("Library/Logs")
        .join(APP_DIR_NAME)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn the_config_base_follows_xdg_config_home_when_it_is_set() {
        let base = config_base();
        assert!(base.is_absolute(), "{}", base.display());
    }

    #[test]
    fn the_cache_base_is_the_user_library_caches_directory() {
        let base = cache_base();
        assert!(base.starts_with(home()), "{}", base.display());
        assert!(
            base.ends_with(std::path::Path::new("Library").join("Caches")),
            "{}",
            base.display()
        );
    }

    #[test]
    fn the_log_directory_is_the_app_subdirectory_of_user_library_logs() {
        let dir = log_dir_path();
        assert!(dir.starts_with(home()), "{}", dir.display());
        assert!(
            dir.ends_with(
                std::path::Path::new("Library")
                    .join("Logs")
                    .join(APP_DIR_NAME)
            ),
            "{}",
            dir.display()
        );
    }
}
