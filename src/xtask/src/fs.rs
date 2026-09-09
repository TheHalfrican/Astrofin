use anyhow::{Context, Result};
use std::path::Path;

pub fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create_dir_all {}", parent.display()))?;
    }
    std::fs::copy(src, dst)
        .with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
    Ok(())
}

pub fn copy_executable(src: &Path, dst: &Path) -> Result<()> {
    copy_file(src, dst)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(dst)?.permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(dst, perm)?;
    }
    Ok(())
}

pub fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("create_dir_all {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("read_dir {}", src.display()))? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if ty.is_symlink() {
            let _ = std::fs::remove_file(&dst_path);
            #[cfg(unix)]
            {
                let target = std::fs::read_link(&src_path)?;
                std::os::unix::fs::symlink(&target, &dst_path).with_context(|| {
                    format!("symlink {} -> {}", dst_path.display(), target.display())
                })?;
            }
            #[cfg(not(unix))]
            {
                // No symlinks expected in our staged trees on non-unix; fall back
                // to copying the resolved target so the layout still works.
                std::fs::copy(std::fs::canonicalize(&src_path)?, &dst_path).with_context(|| {
                    format!("copy {} -> {}", src_path.display(), dst_path.display())
                })?;
            }
        } else {
            std::fs::copy(&src_path, &dst_path).with_context(|| {
                format!("copy {} -> {}", src_path.display(), dst_path.display())
            })?;
        }
    }
    Ok(())
}

// Used only by the Linux/Windows install paths; the macOS bundle stages files
// individually.
#[cfg(not(target_os = "macos"))]
pub fn copy_glob(src_dir: &Path, dst_dir: &Path, patterns: &[&str]) -> Result<()> {
    std::fs::create_dir_all(dst_dir)?;
    for entry in std::fs::read_dir(src_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if patterns.iter().any(|p| match_pattern(p, &name)) {
            let dst = dst_dir.join(&name);
            if entry.file_type()?.is_dir() {
                copy_dir_recursive(&entry.path(), &dst)?;
            } else {
                std::fs::copy(entry.path(), &dst)?;
            }
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn match_pattern(pat: &str, name: &str) -> bool {
    // Trivial glob: leading `*` (suffix match), trailing `*` (prefix match),
    // contains `.so` style middle match, or exact.
    if let Some(rest) = pat.strip_prefix('*') {
        if let Some(rest) = rest.strip_suffix('*') {
            name.contains(rest)
        } else {
            name.ends_with(rest)
        }
    } else if let Some(rest) = pat.strip_suffix('*') {
        name.starts_with(rest)
    } else {
        name == pat
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn copy_file_creates_missing_parent_directories() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src.txt");
        std::fs::write(&src, b"payload").unwrap();
        let dst = tmp.path().join("a").join("b").join("c").join("out.txt");

        copy_file(&src, &dst).unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), b"payload");
    }

    #[test]
    fn copy_file_errors_when_the_source_is_missing() {
        let tmp = tempdir().unwrap();
        let err = copy_file(&tmp.path().join("nope.txt"), &tmp.path().join("out.txt"))
            .expect_err("copying a missing file must fail");
        assert!(err.to_string().contains("copy"), "{err}");
        assert!(!tmp.path().join("out.txt").exists());
    }

    #[test]
    fn copy_executable_copies_the_bytes_and_marks_the_file_runnable() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("tool");
        std::fs::write(&src, b"#!/bin/sh\n").unwrap();
        let dst = tmp.path().join("bin").join("tool");

        copy_executable(&src, &dst).unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), b"#!/bin/sh\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dst).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755, "mode was {:o}", mode & 0o777);
        }
    }

    #[test]
    fn copy_executable_errors_when_the_source_is_missing() {
        let tmp = tempdir().unwrap();
        assert!(
            copy_executable(
                &tmp.path().join("ghost"),
                &tmp.path().join("bin").join("ghost")
            )
            .is_err()
        );
    }

    #[test]
    fn copy_dir_recursive_reproduces_nested_directories_and_files() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("deep").join("deeper")).unwrap();
        std::fs::write(src.join("top.txt"), b"top").unwrap();
        std::fs::write(src.join("deep").join("mid.txt"), b"mid").unwrap();
        std::fs::write(src.join("deep").join("deeper").join("leaf.txt"), b"leaf").unwrap();
        std::fs::create_dir(src.join("empty")).unwrap();
        let dst = tmp.path().join("dst");

        copy_dir_recursive(&src, &dst).unwrap();

        assert_eq!(std::fs::read(dst.join("top.txt")).unwrap(), b"top");
        assert_eq!(
            std::fs::read(dst.join("deep").join("mid.txt")).unwrap(),
            b"mid"
        );
        assert_eq!(
            std::fs::read(dst.join("deep").join("deeper").join("leaf.txt")).unwrap(),
            b"leaf"
        );
        assert!(
            dst.join("empty").is_dir(),
            "empty directories are copied too"
        );
    }

    #[test]
    fn copy_dir_recursive_errors_when_the_source_is_missing() {
        let tmp = tempdir().unwrap();
        let err = copy_dir_recursive(&tmp.path().join("absent"), &tmp.path().join("dst"))
            .expect_err("copying a missing tree must fail");
        assert!(err.to_string().contains("read_dir"), "{err}");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn copy_glob_copies_only_the_matching_entries() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("libmpv.so.2"), b"a").unwrap();
        std::fs::write(src.join("astrofin.exe"), b"b").unwrap();
        std::fs::write(src.join("notes.txt"), b"c").unwrap();
        std::fs::write(src.join("icudtl.dat"), b"d").unwrap();
        std::fs::create_dir_all(src.join("locales").join("nested")).unwrap();
        std::fs::write(src.join("locales").join("nested").join("en.pak"), b"e").unwrap();
        let dst = tmp.path().join("dst");

        copy_glob(&src, &dst, &["*.so*", "icudtl.dat", "locales"]).unwrap();

        assert!(dst.join("libmpv.so.2").is_file());
        assert!(dst.join("icudtl.dat").is_file());
        assert_eq!(
            std::fs::read(dst.join("locales").join("nested").join("en.pak")).unwrap(),
            b"e",
            "matching directories are copied recursively"
        );
        assert!(!dst.join("notes.txt").exists());
        assert!(!dst.join("astrofin.exe").exists());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn copy_glob_creates_the_destination_even_with_no_matches() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("notes.txt"), b"c").unwrap();
        let dst = tmp.path().join("dst");

        copy_glob(&src, &dst, &["*.dll"]).unwrap();

        assert!(dst.is_dir());
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn copy_glob_errors_when_the_source_directory_is_missing() {
        let tmp = tempdir().unwrap();
        assert!(copy_glob(&tmp.path().join("absent"), &tmp.path().join("dst"), &["*"]).is_err());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn match_pattern_handles_prefix_suffix_contains_and_exact_forms() {
        // Leading `*`: suffix match.
        assert!(match_pattern("*.dll", "cef.dll"));
        assert!(!match_pattern("*.dll", "cef.dll.lib"));
        // Trailing `*`: prefix match.
        assert!(match_pattern("libmpv*", "libmpv-2.dll"));
        assert!(!match_pattern("libmpv*", "mpv.dll"));
        // `*mid*`: contains.
        assert!(match_pattern("*.so*", "libmpv.so.2"));
        assert!(!match_pattern("*.so*", "libmpv.dylib"));
        // No `*`: exact.
        assert!(match_pattern("icudtl.dat", "icudtl.dat"));
        assert!(!match_pattern("icudtl.dat", "icudtl.dat.bak"));
        // A bare `*` matches anything (empty suffix).
        assert!(match_pattern("*", "anything"));
    }
}
