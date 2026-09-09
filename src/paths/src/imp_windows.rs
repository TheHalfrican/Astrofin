use super::APP_DIR_NAME;
use std::path::PathBuf;

pub(super) const DEFAULT_LOG_TO_FILE: bool = true;

fn local_appdata() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("C:"))
}

pub(super) fn config_base() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("C:"))
}

pub(super) fn cache_base() -> PathBuf {
    local_appdata()
}

pub(super) fn log_dir_path() -> PathBuf {
    local_appdata().join(APP_DIR_NAME).join("Logs")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn the_config_base_is_the_roaming_app_data_directory() {
        let base = config_base();
        assert!(base.is_absolute(), "{}", base.display());
        // dirs::config_dir() is %APPDATA% on Windows; the fallback is the
        // system drive, which is what the assertion below tolerates.
        assert!(
            base.ends_with("Roaming") || base == std::path::Path::new("C:"),
            "{}",
            base.display()
        );
    }

    #[test]
    fn the_cache_base_is_the_local_app_data_directory() {
        let base = cache_base();
        assert!(base.is_absolute(), "{}", base.display());
        assert!(
            base.ends_with("Local") || base == std::path::Path::new("C:"),
            "{}",
            base.display()
        );
    }

    #[test]
    fn the_log_directory_sits_under_the_cache_base() {
        let dir = log_dir_path();
        assert!(dir.starts_with(cache_base()), "{}", dir.display());
        assert!(dir.ends_with(std::path::Path::new(APP_DIR_NAME).join("Logs")));
    }

    #[test]
    fn the_config_and_cache_bases_are_distinct_roots() {
        // Roaming config vs. non-roaming cache: a profile that roams must not
        // drag the CEF cache with it.
        assert_ne!(config_base(), cache_base());
    }
}
