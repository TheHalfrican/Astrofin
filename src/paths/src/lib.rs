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

pub fn set_config_dir_override(path: PathBuf) {
    overrides().config_dir = Some(path);
}

pub fn set_cache_dir_override(path: PathBuf) {
    overrides().cache_dir = Some(path);
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

fn ensure(path: PathBuf) -> PathBuf {
    let _ = fs::create_dir_all(&path);
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

pub fn log_dir() -> PathBuf {
    ensure(imp::log_dir_path())
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
    if dir.file_name() == Some(std::ffi::OsStr::new("MacOS")) {
        if let Some(contents) = dir.parent() {
            return contents.join("Resources");
        }
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
    let user = user_shader_dir();
    if !files.is_empty() && files.iter().all(|f| user.join(f).is_file()) {
        return user;
    }
    bundled_shader_dir().join(subdir)
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

pub fn instance_listener_path(id: impl std::fmt::Display) -> io::Result<PathBuf> {
    #[cfg(unix)]
    {
        Ok(runtime_dir()?.join(format!("{APP_DIR_NAME}-{id}")))
    }
    #[cfg(windows)]
    {
        Ok(PathBuf::from(format!(r"\\.\pipe\{APP_DIR_NAME}-{id}")))
    }
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
pub use migrate::{MigrationReport, migrate_legacy, repair_mpv_conf};

#[cfg_attr(target_os = "linux", path = "imp_linux.rs")]
#[cfg_attr(target_os = "macos", path = "imp_macos.rs")]
#[cfg_attr(windows, path = "imp_windows.rs")]
mod imp;
