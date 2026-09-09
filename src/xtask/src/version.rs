use crate::paths;
use anyhow::{Context, Result, anyhow};

pub struct Version {
    /// "<raw>[+<short-hash>[-dirty]]" — adds git suffix iff raw is a
    /// pre-release (has a "-suffix").
    pub full: String,
}

/// Short HEAD hash and dirty flag. `(None, false)` when there is no repo.
pub fn git_info() -> (Option<String>, bool) {
    let Ok(repo) = gix::discover(paths::repo_root()) else {
        return (None, false);
    };
    let hash = repo
        .head_id()
        .ok()
        .map(|id| id.to_hex_with_len(7).to_string());
    let dirty = repo.is_dirty().unwrap_or(false);
    (hash, dirty)
}

/// "<raw>[+<short-hash>[-dirty]]". The git suffix is added only when `raw`
/// is a pre-release (contains a `-`) and a hash is known.
fn compose_version(raw: &str, hash: Option<&str>, dirty: bool) -> String {
    match (raw.contains('-'), hash) {
        (true, Some(hash)) => {
            let suffix = if dirty { "-dirty" } else { "" };
            format!("{raw}+{hash}{suffix}")
        }
        _ => raw.to_owned(),
    }
}

pub fn read() -> Result<Version> {
    let raw = env!("CARGO_PKG_VERSION");
    let (hash, dirty) = git_info();
    let full = compose_version(raw, hash.as_deref(), dirty);
    Ok(Version { full })
}

pub fn cef_package_version() -> Result<String> {
    let lock = paths::repo_root().join("src").join("Cargo.lock");
    let lockfile =
        cargo_lock::Lockfile::load(&lock).with_context(|| format!("parse {}", lock.display()))?;
    lockfile
        .packages
        .iter()
        .find(|pkg| pkg.name.as_str() == "cef")
        .map(|pkg| pkg.version.to_string())
        .ok_or_else(|| anyhow!("`cef` package not found in {}", lock.display()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn release_version_never_gains_a_git_suffix() {
        assert_eq!(compose_version("1.2.3", Some("abc1234"), false), "1.2.3");
        assert_eq!(compose_version("1.2.3", Some("abc1234"), true), "1.2.3");
        assert_eq!(compose_version("1.2.3", None, false), "1.2.3");
    }

    #[test]
    fn pre_release_version_gains_the_short_hash() {
        assert_eq!(
            compose_version("0.4.0-dev", Some("abc1234"), false),
            "0.4.0-dev+abc1234"
        );
    }

    #[test]
    fn dirty_pre_release_version_is_marked_dirty() {
        assert_eq!(
            compose_version("0.4.0-dev", Some("abc1234"), true),
            "0.4.0-dev+abc1234-dirty"
        );
    }

    #[test]
    fn pre_release_version_without_a_hash_is_left_alone() {
        assert_eq!(compose_version("0.4.0-dev", None, false), "0.4.0-dev");
        assert_eq!(compose_version("0.4.0-dev", None, true), "0.4.0-dev");
    }

    #[test]
    fn read_reports_the_crate_version_as_its_prefix() {
        let version = read().unwrap();
        assert!(
            version.full.starts_with(env!("CARGO_PKG_VERSION")),
            "{} should start with {}",
            version.full,
            env!("CARGO_PKG_VERSION")
        );
        // Anything past the raw version is the `+hash[-dirty]` suffix.
        let rest = &version.full[env!("CARGO_PKG_VERSION").len()..];
        assert!(
            rest.is_empty() || rest.starts_with('+'),
            "unexpected {rest}"
        );
    }

    #[test]
    fn git_info_yields_a_seven_digit_hex_hash_when_a_repository_is_found() {
        let (hash, dirty) = git_info();
        if let Some(hash) = hash {
            assert_eq!(hash.len(), 7, "short hash {hash} is not 7 chars");
            assert!(
                hash.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "short hash {hash} is not lowercase hex"
            );
        }
        // `dirty` is a plain flag either way; both values are legitimate.
        let _clean_or_dirty: bool = dirty;
    }

    #[test]
    fn cef_package_version_is_read_from_the_workspace_lockfile() {
        let version = cef_package_version().expect("src/Cargo.lock must pin `cef`");
        let parts: Vec<&str> = version.split('.').collect();
        assert!(
            parts.len() >= 2,
            "{version} does not look like a dotted version"
        );
        assert!(
            parts[0].chars().all(|c| c.is_ascii_digit()),
            "{version} does not start with a number"
        );
    }
}
