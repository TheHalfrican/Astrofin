//! Per-user filesystem locations.
//!
//! - Linux: XDG Base Directory (config/cache/state) with `$HOME` fallback.
//! - macOS: `~/.config` for config (matches existing installs), `~/Library`
//!   for cache/logs.
//! - Windows: `%APPDATA%` for config, `%LOCALAPPDATA%` for cache/logs.
//!
//! Each directory getter creates the directory (and parents) if missing
//! before returning.

use parking_lot::{Mutex, MutexGuard};
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

const APP_DIR_NAME: &str = "astrofin";
const LOG_FILE_NAME: &str = "astrofin.log";

/// The pre-rebrand directory name, kept only so [`migrate::migrate_legacy`]
/// can find a Jellium Desktop profile to import on first run.
pub(crate) const LEGACY_APP_DIR_NAME: &str = "jellium-desktop";

/// Environment variables that name the config/cache directory. The CLI
/// reads them as fallbacks for `--config-dir`/`--cache-dir`, and the browser
/// process re-exports an explicit flag into them so CEF helper processes —
/// which return from `jfn_cef_start` before argv is ever parsed, and load
/// `settings.json` on their own — resolve the same directory.
pub const ENV_CONFIG_DIR: &str = "ASTROFIN_CONFIG_DIR";
pub const ENV_CACHE_DIR: &str = "ASTROFIN_CACHE_DIR";

struct Overrides {
    config_dir: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
}

static OVERRIDES: Mutex<Overrides> = Mutex::new(Overrides {
    config_dir: None,
    cache_dir: None,
});

fn overrides() -> MutexGuard<'static, Overrides> {
    OVERRIDES.lock()
}

/// An empty path is ignored, exactly as an empty [`ENV_CONFIG_DIR`] is: clap
/// hands `--config-dir ""` (and an empty environment variable) through as
/// `Some("")`, and taking that literally would resolve the profile to the
/// process's working directory in the browser process while the CEF helpers,
/// which read the environment variable directly, kept the real one.
///
/// Returns the absolute path actually stored, so the caller can export *that*
/// into [`ENV_CONFIG_DIR`] rather than the relative spelling it was given —
/// see [`absolutize`].
pub fn set_config_dir_override(path: PathBuf) -> Option<PathBuf> {
    let resolved = resolve_override(path)?;
    overrides().config_dir = Some(resolved.clone());
    Some(resolved)
}

/// See [`set_config_dir_override`].
pub fn set_cache_dir_override(path: PathBuf) -> Option<PathBuf> {
    let resolved = resolve_override(path)?;
    overrides().cache_dir = Some(resolved.clone());
    Some(resolved)
}

/// An empty override is no override; anything else is pinned to the launch
/// working directory before it is stored.
fn resolve_override(path: PathBuf) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Some(absolutize(&path, &cwd))
}

/// Pin a possibly relative profile directory to one absolute path.
///
/// `--config-dir profile` used to mean a different directory in every process:
/// the browser process resolved it against its own working directory and then
/// exported the *relative* spelling, so a CEF helper started from anywhere
/// else — or the same process after something called `set_current_dir` —
/// loaded a different `settings.json`. Resolving once at startup and storing
/// the result makes every reader agree.
///
/// Lexical, not [`fs::canonicalize`]: the directory usually does not exist
/// yet on a first run, and canonicalising would fail. `..` components are
/// deliberately left in place — resolving them textually would step over a
/// symlink to somewhere else — while a bare `.` is dropped as noise. A
/// Windows drive-relative path (`C:profile`) has a prefix but no root, so it
/// cannot be joined onto a cwd on another drive and is returned unchanged.
fn absolutize(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() || path.components().next().is_none() {
        return path.to_path_buf();
    }
    let mut joined = cwd.to_path_buf();
    let mut any = false;
    for component in path.components() {
        if matches!(component, std::path::Component::CurDir) {
            continue;
        }
        // A prefixed-but-relative Windows path: joining would silently change
        // which volume it names.
        if matches!(component, std::path::Component::Prefix(_)) {
            return path.to_path_buf();
        }
        joined.push(component);
        any = true;
    }
    if any { joined } else { cwd.to_path_buf() }
}

fn env_override(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn config_override() -> Option<PathBuf> {
    overrides()
        .config_dir
        .clone()
        .or_else(|| env_override(ENV_CONFIG_DIR))
}

fn cache_override() -> Option<PathBuf> {
    overrides()
        .cache_dir
        .clone()
        .or_else(|| env_override(ENV_CACHE_DIR))
}

#[cfg(not(windows))]
fn env_or(var: &str, fallback: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => v,
        _ => fallback.to_string(),
    }
}

#[cfg(not(windows))]
fn home() -> String {
    env_or("HOME", "/tmp")
}

/// Create a directory we are about to hand out, and say so when we could not.
///
/// The path is still returned: the caller's next write reports the real
/// failure with the file it was trying to write, which is a better error than
/// anything this could return. What was missing until now is the line naming
/// the directory itself — without it a read-only `%APPDATA%`, a full disk or a
/// plain file sitting where the profile belongs surfaced only as a string of
/// unexplained write failures much later in the log.
fn ensure(path: PathBuf) -> PathBuf {
    if let Err(e) = fs::create_dir_all(&path) {
        tracing::warn!(target: "Paths", "could not create {}: {e}", path.display());
    }
    path
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

/// `Ok(false)` means another process won the race and created `path` first.
pub fn write_atomic_noclobber(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    match tmp.persist_noclobber(path) {
        Ok(_) => Ok(true),
        Err(err) if err.error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err.error),
    }
}

/// The config directory *without* creating it. The migration has to ask
/// "does the new directory exist yet?", which the creating getters below can
/// never answer truthfully.
pub(crate) fn config_dir_raw() -> PathBuf {
    config_override().unwrap_or_else(|| imp::config_base().join(APP_DIR_NAME))
}

/// See [`config_dir_raw`].
pub(crate) fn cache_dir_raw() -> PathBuf {
    cache_override().unwrap_or_else(|| imp::cache_base().join(APP_DIR_NAME))
}

pub fn config_dir() -> PathBuf {
    ensure(config_dir_raw())
}

pub fn cache_dir() -> PathBuf {
    ensure(cache_dir_raw())
}

/// See [`config_dir_raw`]: the log directory *without* creating it, so a test
/// can assert its shape without materialising the real profile.
fn log_dir_raw() -> PathBuf {
    imp::log_dir_path()
}

pub fn log_dir() -> PathBuf {
    ensure(log_dir_raw())
}

pub fn mpv_home() -> PathBuf {
    ensure(config_dir().join("mpv"))
}

/// Directory of the running executable, or `.` when it cannot be determined
/// (a process whose image was unlinked, mostly).
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where the read-only files `cargo xtask build` stages next to the binary
/// live (`shaders/`, …).
///
/// Linux/Windows: the executable's own directory, which is also where CEF's
/// `resources.pak` and the runtime libraries land. macOS: `Contents/Resources`
/// when running from an app bundle — but the staged `build/` tree is flat, so
/// a binary that is not inside `Contents/MacOS` keeps looking beside itself.
pub fn resource_dir() -> PathBuf {
    let dir = exe_dir();
    #[cfg(target_os = "macos")]
    if dir.file_name() == Some(std::ffi::OsStr::new("MacOS"))
        && let Some(contents) = dir.parent()
    {
        return contents.join("Resources");
    }
    dir
}

/// The shader tree shipped with the app. Subdirectories (`anime4k`,
/// `fsrcnnx`, …) group one upstream project each.
pub fn bundled_shader_dir() -> PathBuf {
    resource_dir().join("shaders")
}

/// The user's own flat shader folder, `<config dir>/mpv/shaders`. Not
/// created: its absence is the signal that there is no override.
pub fn user_shader_dir() -> PathBuf {
    config_dir_raw().join("mpv").join("shaders")
}

/// Pick the directory a shader chain loads from: the user's own
/// `<config dir>/mpv/shaders` when it holds *every* file in `files`, else the
/// bundled `<resource dir>/shaders/<subdir>`.
///
/// All-or-nothing per chain, deliberately: mixing a user's updated shader with
/// bundled ones from a different release is how a chain ends up compiling
/// against hooks that moved.
pub fn shader_dir(subdir: &str, files: &[&str]) -> PathBuf {
    pick_shader_dir(user_shader_dir(), bundled_shader_dir().join(subdir), files)
}

/// [`shader_dir`] with both candidates injected, so the choice can be tested
/// without a process-global config directory.
fn pick_shader_dir(user: PathBuf, bundled: PathBuf, files: &[&str]) -> PathBuf {
    if !files.is_empty() && files.iter().all(|f| user.join(f).is_file()) {
        return user;
    }
    bundled
}

#[cfg(unix)]
pub fn runtime_dir() -> io::Result<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR")
        && !dir.is_empty()
    {
        return Ok(ensure(PathBuf::from(dir)));
    }
    private_dir(PathBuf::from(format!(
        "/tmp/{APP_DIR_NAME}-{}",
        nix::unistd::getuid()
    )))
}

/// `/tmp` is world-writable and the name is predictable, so a squatter can
/// pre-create the directory and then own every socket placed inside it.
/// Accept the path only if we just created it 0700, or it is still a real
/// directory owned by us that nobody else can reach into.
#[cfg(unix)]
fn private_dir(path: PathBuf) -> io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    match fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => return Ok(path),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_dir() || meta.uid() != nix::unistd::getuid().as_raw() || meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a private directory we own", path.display()),
        ));
    }
    Ok(path)
}

/// Longest instance id accepted by [`instance_listener_path`]. A real id is a
/// 32-character UUID; the Windows pipe namespace caps the whole name at 256.
const INSTANCE_ID_MAX: usize = 64;

/// Where the second-instance listener binds. The id becomes a path component
/// on unix and a pipe name on Windows, so it is validated rather than
/// interpolated blind — see [`check_instance_id`].
pub fn instance_listener_path(id: impl std::fmt::Display) -> io::Result<PathBuf> {
    let id = id.to_string();
    check_instance_id(&id)?;
    #[cfg(unix)]
    {
        // Already per-user: `runtime_dir` is `$XDG_RUNTIME_DIR` or a 0700
        // directory of our own under /tmp, both of which nobody else can
        // write into.
        Ok(runtime_dir()?.join(format!("{APP_DIR_NAME}-{id}")))
    }
    #[cfg(windows)]
    {
        Ok(PathBuf::from(pipe_name(&id, &user_win::current_user_key())))
    }
}

/// Longest user key spliced into a pipe name. The Windows namespace caps the
/// whole name at 256: `\\.\pipe\astrofin-` is 18, the id at most
/// [`INSTANCE_ID_MAX`], and the separator one more.
#[cfg(any(windows, test))]
const USER_KEY_MAX: usize = 96;

/// Used when no per-user value can be derived at all. Restores the old,
/// unscoped name rather than failing the launch.
#[cfg(any(windows, test))]
const USER_KEY_FALLBACK: &str = "user";

/// The Windows pipe name for one instance of one user.
///
/// `\\.\pipe\` is a single machine-wide namespace: every session and every
/// user sees the same names, so the instance id on its own lets any other
/// account on the machine bind our name first and make every launch conclude
/// "already running" and exit. Splicing a per-user value in scopes the name
/// the way `runtime_dir` already scopes the unix socket. It is not a secret —
/// a SID is not private — but it means the squatter has to be *this* user,
/// who could stop the app anyway.
///
/// The user key is sanitised the same way [`check_instance_id`] validates an
/// id: a separator, a NUL or a control character in a user name would
/// otherwise reshape the name (`\\.\pipe\astrofin-a\b-<id>` names a pipe in a
/// different directory of the namespace).
#[cfg(any(windows, test))]
fn pipe_name(instance_id: &str, user_key: &str) -> String {
    format!(
        r"\\.\pipe\{APP_DIR_NAME}-{}-{instance_id}",
        sanitize_user_key(user_key)
    )
}

/// Reduce an arbitrary per-user string to the same alphabet
/// [`check_instance_id`] accepts, so it can be spliced into a pipe name.
/// Everything outside it collapses to `_`, which cannot merge two users that
/// the alphabet would otherwise keep apart *and* cannot escape the name.
#[cfg(any(windows, test))]
fn sanitize_user_key(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(USER_KEY_MAX));
    for c in raw.chars() {
        if out.len() == USER_KEY_MAX {
            break;
        }
        out.push(if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            c
        } else {
            '_'
        });
    }
    if out.is_empty() {
        return USER_KEY_FALLBACK.to_string();
    }
    out
}

#[cfg(windows)]
mod user_win;

/// `InstanceId` renders as 32 hex digits, so a whitelist costs nothing and
/// keeps a future caller from handing this a separator, a `..`, a NUL or a
/// control character — any of which would escape the runtime directory on unix
/// or reshape the pipe name on Windows.
fn check_instance_id(id: &str) -> io::Result<()> {
    let ok = !id.is_empty()
        && id.len() <= INSTANCE_ID_MAX
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{id:?} is not a usable instance id"),
    ))
}

pub fn log_path() -> PathBuf {
    log_dir().join(LOG_FILE_NAME)
}

/// Where logs go when no log file was requested explicitly. Linux: `None` —
/// stderr/journalctl is the norm. macOS/Windows: GUI processes have no
/// user-visible stderr, so default to the platform log file.
pub fn default_log_file() -> Option<PathBuf> {
    imp::DEFAULT_LOG_TO_FILE.then(log_path)
}

mod migrate;
mod space;
pub use migrate::{MigrationReport, migrate_legacy, repair_mpv_conf};

#[cfg_attr(target_os = "linux", path = "imp_linux.rs")]
#[cfg_attr(target_os = "macos", path = "imp_macos.rs")]
#[cfg_attr(windows, path = "imp_windows.rs")]
mod imp;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        APP_DIR_NAME, INSTANCE_ID_MAX, LOG_FILE_NAME, USER_KEY_FALLBACK, USER_KEY_MAX, absolutize,
        bundled_shader_dir, check_instance_id, ensure, instance_listener_path, log_dir_raw,
        pick_shader_dir, pipe_name, resource_dir, sanitize_user_key, user_shader_dir, write_atomic,
        write_atomic_noclobber,
    };
    use std::ffi::OsStr;
    use std::fs;
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};

    #[test]
    fn ensure_creates_the_directory_and_hands_the_path_back() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("a").join("b");
        assert_eq!(ensure(nested.clone()), nested);
        assert!(nested.is_dir());
        // Idempotent: the second call finds it there and is still quiet.
        assert_eq!(ensure(nested.clone()), nested);
    }

    /// A directory it cannot create is logged, not returned as an error: the
    /// caller's own write reports the real problem with the file it names.
    #[test]
    fn ensure_still_returns_a_path_it_could_not_create() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let blocker = tmp.path().join("blocker");
        fs::write(&blocker, b"a file, not a directory").expect("write");
        let under = blocker.join("profile");
        assert_eq!(ensure(under.clone()), under);
        assert!(!under.exists());
    }

    #[test]
    fn absolutize_pins_a_relative_directory_to_the_launch_cwd() {
        let cwd = Path::new(if cfg!(windows) {
            r"C:\launch"
        } else {
            "/launch"
        });
        assert_eq!(
            absolutize(Path::new("profile"), cwd),
            cwd.join("profile"),
            "a bare name resolves against the launch directory"
        );
        assert_eq!(
            absolutize(Path::new("a/b"), cwd),
            cwd.join("a").join("b"),
            "so does a nested one"
        );
    }

    #[test]
    fn absolutize_leaves_an_absolute_directory_alone() {
        let cwd = Path::new(if cfg!(windows) {
            r"C:\launch"
        } else {
            "/launch"
        });
        let absolute = if cfg!(windows) {
            PathBuf::from(r"C:\elsewhere\profile")
        } else {
            PathBuf::from("/elsewhere/profile")
        };
        assert_eq!(absolutize(&absolute, cwd), absolute);
    }

    /// `.` is noise; `..` is not resolved textually, because doing so steps
    /// over a symlink instead of through it.
    #[test]
    fn absolutize_drops_a_bare_dot_and_keeps_parent_components() {
        let cwd = Path::new(if cfg!(windows) {
            r"C:\launch"
        } else {
            "/launch"
        });
        assert_eq!(absolutize(Path::new("."), cwd), cwd);
        assert_eq!(absolutize(Path::new("./profile"), cwd), cwd.join("profile"));
        assert_eq!(
            absolutize(Path::new("../profile"), cwd),
            cwd.join("..").join("profile")
        );
    }

    #[test]
    fn sanitize_user_key_keeps_a_sid_verbatim() {
        assert_eq!(
            sanitize_user_key("S-1-5-21-3623811015-3361044348-30300820-1013"),
            "S-1-5-21-3623811015-3361044348-30300820-1013"
        );
    }

    #[test]
    fn sanitize_user_key_neutralises_anything_that_could_reshape_a_pipe_name() {
        assert_eq!(sanitize_user_key(r"dom\user"), "dom_user");
        assert_eq!(sanitize_user_key("a/b"), "a_b");
        assert_eq!(sanitize_user_key("a\0b"), "a_b");
        assert_eq!(sanitize_user_key("a\nb"), "a_b");
        assert_eq!(sanitize_user_key("a:b"), "a_b");
        assert_eq!(sanitize_user_key(".."), "__");
        assert_eq!(sanitize_user_key("Ünïcode"), "_n_code");
    }

    #[test]
    fn sanitize_user_key_caps_the_length_and_never_yields_nothing() {
        assert_eq!(sanitize_user_key("").len(), USER_KEY_FALLBACK.len());
        assert_eq!(sanitize_user_key(""), USER_KEY_FALLBACK);
        assert_eq!(sanitize_user_key(&"x".repeat(500)).len(), USER_KEY_MAX);
    }

    /// The Windows pipe namespace is machine-wide, so the name has to carry
    /// the user as well as the instance.
    #[test]
    fn pipe_name_scopes_the_instance_to_one_user() {
        let id = "0123456789abcdef0123456789abcdef";
        let mine = pipe_name(id, "S-1-5-21-99-1001");
        assert_eq!(
            mine,
            r"\\.\pipe\astrofin-S-1-5-21-99-1001-0123456789abcdef0123456789abcdef"
        );
        assert_ne!(mine, pipe_name(id, "S-1-5-21-99-1002"));
        assert_ne!(
            mine,
            pipe_name("0123456789abcdef0123456789abcdee", "S-1-5-21-99-1001")
        );
    }

    #[test]
    fn pipe_name_stays_inside_the_windows_name_limit() {
        let name = pipe_name(&"i".repeat(INSTANCE_ID_MAX), &"u".repeat(USER_KEY_MAX * 2));
        assert!(name.len() < 256, "{} chars", name.len());
        assert!(name.starts_with(r"\\.\pipe\astrofin-"), "{name}");
        // One pipe, not a path into the namespace.
        assert_eq!(
            name.matches('\\').count(),
            r"\\.\pipe\".matches('\\').count()
        );
    }

    /// Everything in `dir` except `keep`. The atomic writers stage a temp file
    /// beside the target; a failed write must not leave it behind.
    fn strays(dir: &Path, keep: &str) -> Vec<String> {
        fs::read_dir(dir)
            .expect("read_dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name != keep)
            .collect()
    }

    #[test]
    fn write_atomic_creates_and_then_replaces_the_target() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("settings.json");

        write_atomic(&target, b"first").expect("first write");
        assert_eq!(fs::read(&target).expect("read"), b"first");

        write_atomic(&target, b"second").expect("second write");
        assert_eq!(fs::read(&target).expect("read"), b"second");
        assert!(
            strays(tmp.path(), "settings.json").is_empty(),
            "temp files left behind"
        );
    }

    #[test]
    fn write_atomic_writes_bytes_verbatim_including_nul_and_invalid_utf8() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("blob");
        let bytes = [0x00, 0xff, b'\n', 0x80, b'{'];
        write_atomic(&target, &bytes).expect("write");
        assert_eq!(fs::read(&target).expect("read"), bytes);
    }

    #[test]
    fn write_atomic_fails_without_a_parent_directory_and_leaves_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("missing").join("settings.json");
        let err = write_atomic(&target, b"x").expect_err("no parent");
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(!target.exists());
        assert!(strays(tmp.path(), "").is_empty());
    }

    #[test]
    fn write_atomic_refuses_a_directory_as_the_target() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("settings.json");
        fs::create_dir(&target).expect("mkdir");
        assert!(write_atomic(&target, b"x").is_err());
        assert!(target.is_dir(), "the directory must survive");
        assert!(strays(tmp.path(), "settings.json").is_empty());
    }

    /// A read-only `settings.json` (or one on a read-only volume) must fail
    /// loudly and leave both the original and the directory clean.
    #[cfg(windows)]
    // Clearing the read-only bit is how the test undoes its own setup so the
    // temp directory can be removed; the lint's Unix caveat does not apply.
    #[allow(clippy::permissions_set_readonly_false)]
    #[test]
    fn write_atomic_fails_on_a_read_only_target() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("settings.json");
        fs::write(&target, b"original").expect("seed");
        let mut perms = fs::metadata(&target).expect("meta").permissions();
        perms.set_readonly(true);
        fs::set_permissions(&target, perms).expect("chmod");

        let result = write_atomic(&target, b"replacement");

        let mut perms = fs::metadata(&target).expect("meta").permissions();
        perms.set_readonly(false);
        fs::set_permissions(&target, perms).expect("chmod back");

        assert!(result.is_err(), "read-only target was overwritten");
        assert_eq!(fs::read(&target).expect("read"), b"original");
        assert!(strays(tmp.path(), "settings.json").is_empty());
    }

    /// `rename(2)` replaces the link, it does not write through it: a symlink
    /// planted where `settings.json` belongs cannot be used to clobber a file
    /// somewhere else.
    #[cfg(unix)]
    #[test]
    fn write_atomic_replaces_a_symlinked_target_instead_of_following_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let victim = tmp.path().join("victim");
        fs::write(&victim, b"do not touch").expect("seed");
        let target = tmp.path().join("settings.json");
        std::os::unix::fs::symlink(&victim, &target).expect("symlink");

        write_atomic(&target, b"new").expect("write");

        assert_eq!(fs::read(&victim).expect("read"), b"do not touch");
        assert_eq!(fs::read(&target).expect("read"), b"new");
        assert!(
            !fs::symlink_metadata(&target)
                .expect("meta")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn write_atomic_noclobber_creates_once_and_reports_the_loser() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("instance.json");

        assert!(write_atomic_noclobber(&target, b"mine").expect("first write"));
        assert!(!write_atomic_noclobber(&target, b"theirs").expect("second write"));
        assert_eq!(fs::read(&target).expect("read"), b"mine");
        assert!(strays(tmp.path(), "instance.json").is_empty());
    }

    #[test]
    fn write_atomic_noclobber_fails_without_a_parent_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("missing").join("instance.json");
        assert!(write_atomic_noclobber(&target, b"x").is_err());
        assert!(strays(tmp.path(), "").is_empty());
    }

    #[test]
    fn pick_shader_dir_takes_the_user_folder_only_when_it_holds_every_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user = tmp.path().join("user");
        let bundled = tmp.path().join("bundled");
        fs::create_dir_all(&user).expect("mkdir");
        fs::write(user.join("a.glsl"), "// a").expect("write");

        assert_eq!(
            pick_shader_dir(user.clone(), bundled.clone(), &["a.glsl"]),
            user
        );
        // One file short: the whole chain falls back, never mixes.
        assert_eq!(
            pick_shader_dir(user.clone(), bundled.clone(), &["a.glsl", "b.glsl"]),
            bundled
        );
        assert_eq!(pick_shader_dir(user, bundled.clone(), &[]), bundled);
    }

    /// A directory named where a shader file belongs is not a shader.
    #[test]
    fn pick_shader_dir_ignores_a_directory_standing_in_for_a_shader() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user = tmp.path().join("user");
        let bundled = tmp.path().join("bundled");
        fs::create_dir_all(user.join("a.glsl")).expect("mkdir");
        assert_eq!(pick_shader_dir(user, bundled.clone(), &["a.glsl"]), bundled);
    }

    #[test]
    fn shader_dirs_hang_off_the_resource_and_config_trees() {
        assert_eq!(bundled_shader_dir(), resource_dir().join("shaders"));
        assert!(user_shader_dir().ends_with(Path::new("mpv").join("shaders")));
    }

    #[test]
    fn check_instance_id_accepts_a_rendered_uuid() {
        check_instance_id("0123456789abcdef0123456789abcdef").expect("uuid simple form");
        check_instance_id("a").expect("single char");
        check_instance_id(&"x".repeat(INSTANCE_ID_MAX)).expect("at the cap");
    }

    #[test]
    fn check_instance_id_rejects_anything_that_could_reshape_the_name() {
        let over_cap = "x".repeat(INSTANCE_ID_MAX + 1);
        let hostile = [
            "",
            "..",
            ".",
            "../../etc/passwd",
            r"..\..\pipe",
            "a/b",
            r"a\b",
            "a:b",
            "a b",
            "a\0b",
            "a\nb",
            "pipe\\srvsvc",
            "\u{e9}",
            over_cap.as_str(),
        ];
        for id in hostile {
            let err = check_instance_id(id).expect_err("hostile id accepted");
            assert_eq!(err.kind(), ErrorKind::InvalidInput, "{id:?}");
        }
    }

    #[test]
    fn instance_listener_path_rejects_a_hostile_id_before_touching_the_filesystem() {
        for id in ["", "../../../tmp/evil", r"..\evil", "a/b"] {
            let err = instance_listener_path(id).expect_err("hostile id accepted");
            assert_eq!(err.kind(), ErrorKind::InvalidInput, "{id:?}");
        }
    }

    // Shape, not equality: the name now carries this machine's own user key
    // between the prefix and the id, which no fixed string can spell out.
    #[cfg(windows)]
    #[test]
    fn instance_listener_path_names_a_pipe_under_the_app_prefix() {
        const ID: &str = "0123456789abcdef0123456789abcdef";
        let path = instance_listener_path(ID).expect("valid id");
        let name = path.to_str().expect("the pipe name is utf-8");
        let user = name
            .strip_prefix(r"\\.\pipe\astrofin-")
            .expect("under the app prefix");
        let user = user
            .strip_suffix(&format!("-{ID}"))
            .expect("ends with the instance id");
        assert!(!user.is_empty(), "{name:?}");
        assert!(
            user.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{name:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn instance_listener_path_names_a_socket_in_the_runtime_dir() {
        let path = instance_listener_path("0123456789abcdef0123456789abcdef").expect("valid id");
        assert_eq!(
            path.file_name().and_then(OsStr::to_str),
            Some("astrofin-0123456789abcdef0123456789abcdef")
        );
    }

    /// Not calling `log_dir`/`log_path`/`default_log_file`: they create the
    /// real profile's log directory, which no test may touch. The derivation
    /// they wrap is what is asserted here.
    #[test]
    fn log_dir_is_named_after_the_app_and_holds_one_named_file() {
        let dir = log_dir_raw();
        assert!(
            dir.components().any(|c| c.as_os_str() == APP_DIR_NAME),
            "{dir:?}"
        );
        assert_eq!(
            dir.join(LOG_FILE_NAME).file_name(),
            Some(OsStr::new("astrofin.log"))
        );
    }

    #[test]
    fn logging_to_a_file_by_default_is_a_per_platform_policy() {
        assert_eq!(super::imp::DEFAULT_LOG_TO_FILE, !cfg!(target_os = "linux"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn default_log_file_is_none_on_linux() {
        assert!(super::default_log_file().is_none());
    }

    #[cfg(unix)]
    mod private {
        use super::super::private_dir;
        use std::fs;
        use std::io::ErrorKind;
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

        #[test]
        fn private_dir_creates_it_0700() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("runtime");
            let made = private_dir(path.clone()).expect("created");
            assert_eq!(made, path);
            let mode = fs::metadata(&path).expect("meta").mode() & 0o777;
            assert_eq!(mode, 0o700, "mode {mode:o}");
        }

        #[test]
        fn private_dir_accepts_an_existing_0700_directory_we_own() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("runtime");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .expect("mkdir");
            assert_eq!(private_dir(path.clone()).expect("accepted"), path);
        }

        /// The squatter case: a predictable name under a world-writable /tmp
        /// that somebody else can reach into.
        #[test]
        fn private_dir_rejects_a_group_or_world_reachable_directory() {
            let tmp = tempfile::tempdir().expect("tempdir");
            for mode in [0o777, 0o750, 0o701] {
                let path = tmp.path().join(format!("runtime{mode:o}"));
                fs::create_dir(&path).expect("mkdir");
                // DirBuilder honours the umask, so set the mode explicitly.
                fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod");
                let err = private_dir(path).expect_err("reachable dir accepted");
                assert_eq!(err.kind(), ErrorKind::PermissionDenied, "mode {mode:o}");
            }
        }

        #[test]
        fn private_dir_rejects_a_plain_file_planted_at_the_path() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("runtime");
            fs::write(&path, b"not a dir").expect("write");
            let err = private_dir(path).expect_err("file accepted");
            assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        }

        /// `symlink_metadata`, not `metadata`: a link pointing at a directory
        /// we do own is still somebody else's link.
        #[test]
        fn private_dir_rejects_a_symlink_even_to_a_directory_we_own() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let real = tmp.path().join("real");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&real)
                .expect("mkdir");
            let path = tmp.path().join("runtime");
            std::os::unix::fs::symlink(&real, &path).expect("symlink");
            let err = private_dir(path).expect_err("symlink accepted");
            assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        }
    }
}
