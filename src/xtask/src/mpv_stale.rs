//! Staleness check for the cached meson/ninja mpv build tree.
//!
//! `build.ninja` hardcodes absolute paths to the system libraries mpv links
//! against, and on macOS most of them are version-pinned Homebrew kegs
//! (`/opt/homebrew/Cellar/libass/0.17.5/lib/libass.dylib`). `brew upgrade`
//! pours the new version beside the old one and then deletes the old keg, so
//! a tree configured before the upgrade dies in ninja with "missing and no
//! known rule to make it" before a single object is compiled. Meson will not
//! notice on its own: `meson configure` re-reads options, not dependencies.
//!
//! Seventeen of the nineteen libraries mpv links on this machine are keg
//! paths, so any of ffmpeg, libass, libplacebo, luajit, zimg and friends
//! moving breaks the tree. The two that are stable are `/opt/homebrew/opt/*`
//! symlinks, which brew repoints rather than replaces.
//!
//! This cannot live in `mpv.rs`: that file is on `dev/test-exempt.txt` as
//! glue driving meson, and the rule is to extract logic out of a glue file
//! rather than widen the exemption.

use std::collections::BTreeSet;
use std::path::Path;

/// Absolute shared-library paths referenced by a generated `build.ninja`.
///
/// Ninja separates tokens with whitespace and marks dependency edges with
/// `:` and `|`, so those are trimmed before a token is judged. Deduplicated,
/// because a library is named once per edge that links it.
pub fn referenced_libraries(ninja: &str) -> BTreeSet<&str> {
    ninja
        .split_whitespace()
        .map(|token| token.trim_matches([':', '|']))
        .filter(|token| token.starts_with('/') && is_shared_library(token))
        .collect()
}

/// Whether a path names a shared library on any of our platforms.
fn is_shared_library(path: &str) -> bool {
    path.ends_with(".dylib") || path.ends_with(".so") || path.contains(".so.")
}

/// The first referenced library that no longer exists, if any.
///
/// `exists` is injected so the scan is testable without touching the disk.
pub fn missing_library(ninja: &str, exists: impl Fn(&Path) -> bool) -> Option<&str> {
    referenced_libraries(ninja)
        .into_iter()
        .find(|path| !exists(Path::new(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build.ninja shaped like the real one: a link edge naming several
    /// keg paths, plus relative outputs that must not be mistaken for
    /// system libraries.
    const NINJA: &str = "\
build libmpv.2.dylib: c_LINKER libmpv.2.dylib.p/player_main.c.o | \
/opt/homebrew/Cellar/libass/0.17.5/lib/libass.dylib \
/opt/homebrew/Cellar/ffmpeg/9.0.1_1/lib/libavcodec.dylib \
/opt/homebrew/opt/little-cms2/lib/liblcms2.dylib
 LINK_ARGS = -Wl,-rpath,@loader_path
build mpv: phony mpv.p/osdep_main_fn_unix.c.o
";

    #[test]
    fn referenced_libraries_finds_absolute_shared_objects() {
        let found = referenced_libraries(NINJA);
        assert_eq!(
            found,
            BTreeSet::from([
                "/opt/homebrew/Cellar/ffmpeg/9.0.1_1/lib/libavcodec.dylib",
                "/opt/homebrew/Cellar/libass/0.17.5/lib/libass.dylib",
                "/opt/homebrew/opt/little-cms2/lib/liblcms2.dylib",
            ])
        );
    }

    #[test]
    fn referenced_libraries_ignores_relative_paths_and_objects() {
        // The link target and the .o inputs are relative, and the rpath
        // flag is not a path at all.
        for token in ["libmpv.2.dylib", "libmpv.2.dylib.p/player_main.c.o"] {
            assert!(!referenced_libraries(NINJA).contains(token), "{token}");
        }
    }

    #[test]
    fn referenced_libraries_deduplicates_repeated_edges() {
        let repeated = "/usr/lib/libfoo.so /usr/lib/libfoo.so /usr/lib/libfoo.so";
        assert_eq!(referenced_libraries(repeated).len(), 1);
    }

    #[test]
    fn referenced_libraries_accepts_linux_soname_paths() {
        let linux = "build x: LINK | /usr/lib/x86_64-linux-gnu/libass.so.9";
        assert!(referenced_libraries(linux).contains("/usr/lib/x86_64-linux-gnu/libass.so.9"));
    }

    #[test]
    fn missing_library_reports_an_upgraded_away_keg() {
        // libass moved from 0.17.5 to 0.17.5_1 and brew deleted the old keg.
        let gone = "/opt/homebrew/Cellar/libass/0.17.5/lib/libass.dylib";
        assert_eq!(
            missing_library(NINJA, |path| path != Path::new(gone)),
            Some(gone)
        );
    }

    #[test]
    fn missing_library_is_none_when_every_path_resolves() {
        assert_eq!(missing_library(NINJA, |_| true), None);
    }

    #[test]
    fn missing_library_is_none_for_a_ninja_without_system_libraries() {
        assert_eq!(
            missing_library("build mpv: phony mpv.p/main.c.o", |_| false),
            None
        );
    }
}
