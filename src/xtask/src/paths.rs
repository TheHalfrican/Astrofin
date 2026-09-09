use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub fn repo_root() -> &'static PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        // src/xtask → src → repo_root. Falls back to the cwd, which is the
        // repo root under the usual `cargo xtask` invocation.
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    })
}

pub fn workspace_manifest() -> PathBuf {
    repo_root().join("src").join("jfn_rust").join("Cargo.toml")
}

pub fn cef_cache_dir() -> PathBuf {
    repo_root().join(".cache").join("cef")
}

pub fn cargo_target_dir(out: &std::path::Path) -> PathBuf {
    out.join("cargo-target")
}

pub fn mpv_build_dir(out: &std::path::Path) -> PathBuf {
    out.join("mpv-build")
}

/// GLSL user shaders bundled with the app, staged next to the binary as
/// `shaders/`. See `resources/shaders/README.md`.
pub fn shaders_source_dir() -> PathBuf {
    repo_root().join("resources").join("shaders")
}

pub fn mpv_source_dir() -> PathBuf {
    repo_root().join("third_party").join("mpv")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn repo_root_holds_the_workspace_marker_files() {
        let root = repo_root();
        assert!(
            root.join("src").join("Cargo.toml").is_file(),
            "{} has no src/Cargo.toml",
            root.display()
        );
        assert!(
            root.join("justfile").is_file(),
            "{} has no justfile",
            root.display()
        );
        assert!(
            root.join("resources").is_dir(),
            "{} has no resources/",
            root.display()
        );
    }

    #[test]
    fn repo_root_is_stable_across_calls() {
        assert_eq!(repo_root(), repo_root());
    }

    #[test]
    fn workspace_manifest_points_at_the_jfn_rust_crate() {
        let manifest = workspace_manifest();
        assert!(manifest.starts_with(repo_root()));
        assert!(manifest.ends_with(Path::new("src/jfn_rust/Cargo.toml")));
    }

    #[test]
    fn cef_cache_dir_sits_under_the_repo_cache() {
        let dir = cef_cache_dir();
        assert!(dir.starts_with(repo_root()));
        assert!(dir.ends_with(Path::new(".cache/cef")));
    }

    #[test]
    fn cargo_target_dir_is_a_child_of_the_out_dir() {
        let out = Path::new("some").join("out");
        let target = cargo_target_dir(&out);
        assert!(target.starts_with(&out));
        assert!(target.ends_with(Path::new("cargo-target")));
        assert_eq!(target.parent(), Some(out.as_path()));
    }

    #[test]
    fn mpv_build_dir_is_a_child_of_the_out_dir() {
        let out = Path::new("some").join("out");
        let build = mpv_build_dir(&out);
        assert!(build.starts_with(&out));
        assert!(build.ends_with(Path::new("mpv-build")));
        assert_eq!(build.parent(), Some(out.as_path()));
    }

    #[test]
    fn shaders_source_dir_points_at_the_bundled_shaders() {
        let dir = shaders_source_dir();
        assert!(dir.starts_with(repo_root()));
        assert!(dir.ends_with(Path::new("resources/shaders")));
        assert!(dir.is_dir(), "{} is not a directory", dir.display());
    }

    #[test]
    fn mpv_source_dir_points_at_the_mpv_submodule() {
        let dir = mpv_source_dir();
        assert!(dir.starts_with(repo_root()));
        assert!(dir.ends_with(Path::new("third_party/mpv")));
    }
}
