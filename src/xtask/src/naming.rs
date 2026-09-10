//! The names and layouts the build tasks work in: release-archive names,
//! libmpv's per-OS filenames, and where a CEF distribution unpacks.
//!
//! The tasks themselves shell out and copy files; what a thing is *called* is
//! here, where it is input -> output and answerable for every OS at once, not
//! only the one this xtask was compiled for.

use std::path::{Path, PathBuf};

/// The operating systems the build targets.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

/// The OS this xtask was built for.
pub const HOST_OS: Os = if cfg!(target_os = "windows") {
    Os::Windows
} else if cfg!(target_os = "macos") {
    Os::Mac
} else {
    Os::Linux
};

/// The slug that names an OS in a release-archive filename.
pub fn os_slug(os: Os) -> &'static str {
    match os {
        Os::Windows => "windows",
        Os::Mac => "macos",
        Os::Linux => "linux",
    }
}

/// The archive format a release is published in: zip where the desktop
/// unpacks one natively, a gzipped tar where file modes and symlinks have to
/// survive.
pub fn archive_ext(os: Os) -> &'static str {
    match os {
        Os::Windows | Os::Mac => "zip",
        Os::Linux => "tar.gz",
    }
}

/// The slug that names a CPU architecture in a release-archive filename.
///
/// Windows uses the names its installers and store listings use (`x64`,
/// `arm64`); everywhere else the Rust target names carry through. An
/// architecture with no established slug is passed through unchanged rather
/// than guessed at.
pub fn arch_slug(arch: &str, os: Os) -> &str {
    match (arch, os) {
        ("aarch64", Os::Windows) => "arm64",
        ("aarch64", _) => "aarch64",
        ("x86_64", Os::Windows) => "x64",
        ("x86_64", _) => "x86_64",
        (other, _) => other,
    }
}

/// The stem of a release archive: `Astrofin-<version>-<os>-<arch>`.
pub fn archive_stem(version: &str, os: Os, arch: &str) -> String {
    format!("Astrofin-{version}-{}-{}", os_slug(os), arch_slug(arch, os))
}

/// The full release-archive filename, extension included.
pub fn archive_file_name(version: &str, os: Os, arch: &str) -> String {
    format!("{}.{}", archive_stem(version, os, arch), archive_ext(os))
}

/// The libmpv filename the build links against.
pub fn mpv_link_name(os: Os) -> &'static str {
    match os {
        Os::Windows => "mpv.lib",
        Os::Mac => "libmpv.dylib",
        Os::Linux => "libmpv.so",
    }
}

/// The libmpv filename loaded at runtime — the one carrying the SONAME, which
/// is what has to be staged next to the binary.
pub fn mpv_runtime_name(os: Os) -> &'static str {
    match os {
        Os::Windows => "libmpv-2.dll",
        Os::Mac => "libmpv.2.dylib",
        Os::Linux => "libmpv.so.2",
    }
}

/// Where a CEF distribution lives under the download cache: the versioned
/// directory the archive unpacks into, and the per-target directory inside it
/// that the build stages from.
pub fn cef_layout(root: &Path, cef_version: &str, os_arch: &str) -> (PathBuf, PathBuf) {
    let versioned = root.join(cef_version);
    let cef_dir = versioned.join(os_arch);
    (versioned, cef_dir)
}

/// The `archive.json` a CEF SDK proxy directory carries.
///
/// `download-cef` reads this to decide the distribution is already unpacked;
/// the checksum is deliberately empty because the proxy links to a tree that
/// was already verified when it was downloaded.
pub fn cef_proxy_archive_json(cef_version: &str) -> String {
    format!(r#"{{"type":"minimal","name":"cef_binary_{cef_version}","sha1":""}}"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_os_has_its_own_archive_slug() {
        assert_eq!(os_slug(Os::Windows), "windows");
        assert_eq!(os_slug(Os::Mac), "macos");
        assert_eq!(os_slug(Os::Linux), "linux");
    }

    #[test]
    fn linux_ships_a_tarball_and_the_others_a_zip() {
        assert_eq!(archive_ext(Os::Linux), "tar.gz");
        assert_eq!(archive_ext(Os::Windows), "zip");
        assert_eq!(archive_ext(Os::Mac), "zip");
    }

    #[test]
    fn windows_uses_its_own_architecture_names() {
        assert_eq!(arch_slug("x86_64", Os::Windows), "x64");
        assert_eq!(arch_slug("aarch64", Os::Windows), "arm64");
    }

    #[test]
    fn the_other_platforms_keep_the_rust_target_names() {
        assert_eq!(arch_slug("x86_64", Os::Linux), "x86_64");
        assert_eq!(arch_slug("aarch64", Os::Mac), "aarch64");
    }

    #[test]
    fn an_unrecognised_architecture_is_passed_through() {
        assert_eq!(arch_slug("riscv64", Os::Linux), "riscv64");
        assert_eq!(arch_slug("riscv64", Os::Windows), "riscv64");
    }

    #[test]
    fn an_archive_stem_names_the_version_os_and_arch() {
        assert_eq!(
            archive_stem("0.5.0+abc1234", Os::Linux, "x86_64"),
            "Astrofin-0.5.0+abc1234-linux-x86_64"
        );
    }

    #[test]
    fn an_archive_filename_carries_the_format_for_its_os() {
        assert_eq!(
            archive_file_name("0.5.0", Os::Windows, "x86_64"),
            "Astrofin-0.5.0-windows-x64.zip"
        );
        assert_eq!(
            archive_file_name("0.5.0", Os::Linux, "aarch64"),
            "Astrofin-0.5.0-linux-aarch64.tar.gz"
        );
        assert_eq!(
            archive_file_name("0.5.0-dev", Os::Mac, "aarch64"),
            "Astrofin-0.5.0-dev-macos-aarch64.zip"
        );
    }

    #[test]
    fn the_link_library_is_the_import_library_on_windows() {
        assert_eq!(mpv_link_name(Os::Windows), "mpv.lib");
        assert_eq!(mpv_link_name(Os::Mac), "libmpv.dylib");
        assert_eq!(mpv_link_name(Os::Linux), "libmpv.so");
    }

    #[test]
    fn the_runtime_library_is_the_soname_carrying_one() {
        assert_eq!(mpv_runtime_name(Os::Windows), "libmpv-2.dll");
        assert_eq!(mpv_runtime_name(Os::Mac), "libmpv.2.dylib");
        assert_eq!(mpv_runtime_name(Os::Linux), "libmpv.so.2");
    }

    #[test]
    fn the_link_and_runtime_names_never_collide() {
        for os in [Os::Windows, Os::Mac, Os::Linux] {
            assert_ne!(mpv_link_name(os), mpv_runtime_name(os), "{os:?}");
        }
    }

    #[test]
    fn a_cef_distribution_lives_under_its_version_and_target() {
        let (versioned, dir) = cef_layout(Path::new("/cache/cef"), "141.0.1+abc", "linux64");
        assert_eq!(versioned, Path::new("/cache/cef/141.0.1+abc"));
        assert_eq!(dir, Path::new("/cache/cef/141.0.1+abc/linux64"));
    }

    #[test]
    fn the_versioned_directory_is_the_parent_of_the_target_directory() {
        let (versioned, dir) = cef_layout(Path::new("cache"), "1.2.3", "windows64");
        assert_eq!(dir.parent(), Some(versioned.as_path()));
    }

    #[test]
    fn the_proxy_archive_json_names_the_distribution_with_no_checksum() {
        assert_eq!(
            cef_proxy_archive_json("141.0.1+g1234"),
            r#"{"type":"minimal","name":"cef_binary_141.0.1+g1234","sha1":""}"#
        );
    }

    #[test]
    fn the_host_os_is_one_of_the_three_the_build_supports() {
        assert!(matches!(HOST_OS, Os::Windows | Os::Mac | Os::Linux));
        assert_eq!(os_slug(HOST_OS), std::env::consts::OS);
    }
}
