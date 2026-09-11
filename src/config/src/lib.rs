//! Settings store. Owns the in-memory state, JSON persistence, and the
//! singleton accessor that the rest of the workspace calls into.
//!
//! On-disk schema is [`SettingsFile`]. Missing, unknown, and malformed keys
//! keep their defaults on load; save suppresses fields that are at their
//! default (empty strings, sentinel values, zero geometry) so existing config
//! files round-trip unchanged.

use jfn_mailbox::Mailbox;
use jfn_platform_abi::WindowDecorations;
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};

const DEVICE_NAME_MAX: usize = 64;
// Mirrors `jfn_mpv::HWDEC_DEFAULT` (this crate cannot depend on jfn_mpv); a
// test in jfn_rust::cli keeps the two in step.
const HWDEC_DEFAULT: &str = if cfg!(target_os = "macos") {
    "videotoolbox"
} else {
    "no"
};

/// Largest `settings.json` that is read at all.
///
/// The document is a flat object of scalars plus one small map; a real file is
/// a few hundred bytes and the largest plausible one — a `videoModeLibraries`
/// entry per library on a very large server — is a few kilobytes. A megabyte
/// is therefore not a limit anybody can reach by using the app, and the file
/// is loaded into memory in one piece by a process that has to stay
/// responsive, so something that has grown past it is damage (a log appended
/// to the wrong path, a partial disk image) rather than settings.
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// Bounds on a stored `windowScale`. Below the first the UI is unreadable and
/// the window may be smaller than its own titlebar; above the second a saved
/// geometry can put the window off every monitor. Both are far outside the
/// 1.0-3.0 range real displays report.
const WINDOW_SCALE_MIN: f32 = 0.5;
const WINDOW_SCALE_MAX: f32 = 4.0;

/// What a `windowScale` read from the file resolves to.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ScaleCheck {
    /// Usable as written.
    Ok(f32),
    /// Outside the range, pulled to the nearer end.
    Clamped(f32),
    /// Not a scale at all: NaN, an infinity, or zero and below — which is
    /// also how "no saved scale" is spelled in memory, so it resolves to the
    /// default rather than clamping up to [`WINDOW_SCALE_MIN`].
    Unusable,
}

/// Validate the `windowScale` a hand-edited file offers.
fn check_window_scale(raw: f64) -> ScaleCheck {
    if !raw.is_finite() || raw <= 0.0 {
        return ScaleCheck::Unusable;
    }
    if raw < f64::from(WINDOW_SCALE_MIN) {
        return ScaleCheck::Clamped(WINDOW_SCALE_MIN);
    }
    if raw > f64::from(WINDOW_SCALE_MAX) {
        return ScaleCheck::Clamped(WINDOW_SCALE_MAX);
    }
    ScaleCheck::Ok(raw as f32)
}

/// Whether a settings file of `len` bytes is worth reading into memory.
fn settings_size_ok(len: u64) -> bool {
    len <= MAX_SETTINGS_BYTES
}

// =====================================================================
// Load notices
// =====================================================================

/// The level a buffered notice would have been logged at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warn,
}

/// One line a settings read wanted in the log, held until there is a log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadNotice {
    pub level: NoticeLevel,
    pub message: String,
}

/// Most a single thread buffers. A read produces at most a handful of lines,
/// so the cap is only reached by a process that reloads a file that is still
/// wrong and never drains (a CEF renderer re-reads `settings.json` on its
/// own and has no subscriber to replay them to).
const MAX_LOAD_NOTICES: usize = 32;

thread_local! {
    /// What the reads on this thread have complained about.
    ///
    /// [`settings_load`] runs *before* logging is initialized — the log level
    /// the subscriber is built from is itself read from `settings.json` — so a
    /// `tracing::warn!` raised during the read reaches no subscriber and is
    /// dropped. The read buffers its lines here and the caller replays them
    /// once logging is up, the same way `jfn_paths::MigrationReport` carries
    /// the legacy import's lines across the same ordering problem.
    ///
    /// Per thread, not global: the notices belong to whoever did the read, and
    /// the browser process both loads and replays on its main thread.
    static LOAD_NOTICES: RefCell<Vec<LoadNotice>> = const { RefCell::new(Vec::new()) };
}

/// Buffer one line for replay. Silently dropped past [`MAX_LOAD_NOTICES`].
fn note(level: NoticeLevel, message: String) {
    LOAD_NOTICES.with_borrow_mut(|buf| {
        if buf.len() < MAX_LOAD_NOTICES {
            buf.push(LoadNotice { level, message });
        }
    });
}

/// Take what the settings reads on this thread have buffered, leaving the
/// buffer empty. Call it right after logging is initialized and emit each
/// line at its own level; nothing else will.
#[must_use]
pub fn take_load_notices() -> Vec<LoadNotice> {
    LOAD_NOTICES.with_borrow_mut(std::mem::take)
}

#[derive(Clone, Copy, Debug)]
pub struct JfnWindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub logical_width: i32,
    pub logical_height: i32,
    pub scale: f32,
    pub maximized: bool,
}

impl Default for JfnWindowGeometry {
    fn default() -> Self {
        Self {
            x: -1,
            y: -1,
            width: 0,
            height: 0,
            logical_width: 0,
            logical_height: 0,
            scale: 0.0,
            maximized: false,
        }
    }
}

#[derive(Clone, Debug)]
struct SettingsData {
    server_url: String,
    hwdec: String,
    video_mode: String,
    /// True when the loaded file was written by a build that knows the
    /// post-rename video-mode names. Absent means the stored `videoMode` is a
    /// legacy value and needs one-shot normalisation by the caller.
    video_mode_migrated: bool,
    /// Per-library video-mode overrides, `<library item id> -> wire value`.
    /// Hand-edited only; the app reads it and writes it back untouched.
    video_mode_libraries: BTreeMap<String, String>,
    /// Which transcodes raise the one-time warning at playback start
    /// (`off` | `cpu` | `any`). Empty means "never chosen"; the web UI
    /// resolves that to `cpu`.
    transcode_notice: String,
    audio_passthrough: String,
    audio_channels: String,
    log_level: String,
    device_name: String,
    window: JfnWindowGeometry,
    audio_exclusive: bool,
    disable_gpu_compositing: bool,
    transparent_titlebar: bool,
    force_transcoding: bool,
    window_decorations: Option<WindowDecorations>,
    hide_scrollbar: bool,
}

impl Default for SettingsData {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            hwdec: String::new(),
            video_mode: String::new(),
            video_mode_migrated: false,
            video_mode_libraries: BTreeMap::new(),
            transcode_notice: String::new(),
            audio_passthrough: String::new(),
            audio_channels: String::new(),
            log_level: String::new(),
            device_name: String::new(),
            window: JfnWindowGeometry::default(),
            audio_exclusive: false,
            disable_gpu_compositing: false,
            transparent_titlebar: true,
            force_transcoding: false,
            window_decorations: None,
            hide_scrollbar: true,
        }
    }
}

/// The settings.json document. Every key is optional on load; a key at its
/// default is absent on save. Field order is the on-disk key order.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct SettingsFile {
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    server_url: Option<String>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    window_width: Option<i32>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    window_height: Option<i32>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    window_logical_width: Option<i32>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    window_logical_height: Option<i32>,

    // f64 on the wire: the stored value is an f32, and widening before
    // formatting keeps the digits identical to files written so far.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    window_scale: Option<f64>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    window_x: Option<i32>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    window_y: Option<i32>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    window_maximized: Option<bool>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    hwdec: Option<String>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    video_mode: Option<String>,

    // Always written by this build, never by the ones that predate the
    // auto/live-action/animation rename — which is exactly what makes it a
    // usable marker for the one-shot legacy normalisation.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    video_mode_migrated: Option<bool>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    video_mode_libraries: Option<BTreeMap<String, String>>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    transcode_notice: Option<String>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    audio_passthrough: Option<String>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    audio_exclusive: Option<bool>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    audio_channels: Option<String>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    disable_gpu_compositing: Option<bool>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    transparent_titlebar: Option<bool>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    log_level: Option<String>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    force_transcoding: Option<bool>,

    #[serde(
        deserialize_with = "lenient_decorations",
        serialize_with = "serialize_decorations",
        skip_serializing_if = "Option::is_none"
    )]
    window_decorations: Option<WindowDecorations>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    hide_scrollbar: Option<bool>,

    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    device_name: Option<String>,
}

/// Reads any JSON value and yields `None` unless it deserializes as `T`, so a
/// key of the wrong type is ignored instead of failing the whole load.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).ok())
}

/// Decoration names outside the wire contract are ignored like any other
/// malformed key.
fn lenient_decorations<'de, D>(deserializer: D) -> Result<Option<WindowDecorations>, D::Error>
where
    D: Deserializer<'de>,
{
    let name: Option<String> = lenient(deserializer)?;
    Ok(name.as_deref().and_then(WindowDecorations::parse))
}

/// Emits the wire literal from `WindowDecorations::as_str`.
fn serialize_decorations<S>(
    value: &Option<WindowDecorations>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(d) => serializer.serialize_str(d.as_str()),
        None => serializer.serialize_none(),
    }
}

/// The settings blob the web UI parses. `windowDecorations` is absent:
/// resolving its effective value needs the Platform default, unavailable in
/// the CEF renderer where this is built.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CliSettings<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    hwdec: Option<&'a str>,

    #[serde(skip_serializing_if = "Option::is_none")]
    video_mode: Option<&'a str>,

    #[serde(skip_serializing_if = "Option::is_none")]
    video_mode_libraries: Option<&'a BTreeMap<String, String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    transcode_notice: Option<&'a str>,

    #[serde(skip_serializing_if = "Option::is_none")]
    audio_passthrough: Option<&'a str>,

    #[serde(skip_serializing_if = "Option::is_none")]
    audio_exclusive: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    audio_channels: Option<&'a str>,

    #[serde(skip_serializing_if = "Option::is_none")]
    disable_gpu_compositing: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    transparent_titlebar: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    log_level: Option<&'a str>,

    force_transcoding: bool,

    hide_scrollbar: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    device_name: Option<&'a str>,

    device_name_default: String,

    hwdec_options: &'a [&'a str],

    /// What an unset `hwdec` resolves to, so the UI can show the real value
    /// instead of guessing (the file omits `hwdec` when it equals this).
    hwdec_default: &'static str,
}

impl SettingsData {
    fn overlay(&mut self, file: SettingsFile) {
        if let Some(v) = file.server_url {
            self.server_url = v;
        }
        if let Some(v) = file.hwdec {
            self.hwdec = v;
        }
        if let Some(v) = file.video_mode {
            self.video_mode = v;
        }
        if let Some(v) = file.video_mode_migrated {
            self.video_mode_migrated = v;
        }
        if let Some(v) = file.video_mode_libraries {
            self.video_mode_libraries = v;
        }
        if let Some(v) = file.transcode_notice {
            self.transcode_notice = v;
        }
        if let Some(v) = file.audio_passthrough {
            self.audio_passthrough = v;
        }
        if let Some(v) = file.audio_channels {
            self.audio_channels = v;
        }
        if let Some(v) = file.log_level {
            self.log_level = v;
        }
        if let Some(mut v) = file.device_name {
            truncate_device_name(&mut v);
            self.device_name = v;
        }
        if let Some(v) = file.window_width {
            self.window.width = v;
        }
        if let Some(v) = file.window_height {
            self.window.height = v;
        }
        if let Some(v) = file.window_logical_width {
            self.window.logical_width = v;
        }
        if let Some(v) = file.window_logical_height {
            self.window.logical_height = v;
        }
        if let Some(v) = file.window_scale {
            match check_window_scale(v) {
                ScaleCheck::Ok(scale) => self.window.scale = scale,
                ScaleCheck::Clamped(scale) => {
                    note(
                        NoticeLevel::Warn,
                        format!(
                            "windowScale {v} is outside {WINDOW_SCALE_MIN}..={WINDOW_SCALE_MAX}, \
                             using {scale}"
                        ),
                    );
                    self.window.scale = scale;
                }
                ScaleCheck::Unusable => note(
                    NoticeLevel::Warn,
                    format!("windowScale {v} is not a usable scale, keeping the default"),
                ),
            }
        }
        if let Some(v) = file.window_x {
            self.window.x = v;
        }
        if let Some(v) = file.window_y {
            self.window.y = v;
        }
        if let Some(v) = file.window_maximized {
            self.window.maximized = v;
        }
        if let Some(v) = file.audio_exclusive {
            self.audio_exclusive = v;
        }
        if let Some(v) = file.disable_gpu_compositing {
            self.disable_gpu_compositing = v;
        }
        if let Some(v) = file.transparent_titlebar {
            self.transparent_titlebar = v;
        }
        if let Some(v) = file.force_transcoding {
            self.force_transcoding = v;
        }
        if let Some(v) = file.window_decorations {
            self.window_decorations = Some(v);
        }
        if let Some(v) = file.hide_scrollbar {
            self.hide_scrollbar = v;
        }
    }

    fn to_file(&self) -> SettingsFile {
        let size = self.window.width > 0 && self.window.height > 0;
        let logical = self.window.logical_width > 0 && self.window.logical_height > 0;
        let position = self.window.x >= 0 && self.window.y >= 0;
        SettingsFile {
            server_url: Some(self.server_url.clone()),
            window_width: size.then_some(self.window.width),
            window_height: size.then_some(self.window.height),
            window_logical_width: logical.then_some(self.window.logical_width),
            window_logical_height: logical.then_some(self.window.logical_height),
            window_scale: (self.window.scale > 0.0).then(|| f64::from(self.window.scale)),
            window_x: position.then_some(self.window.x),
            window_y: position.then_some(self.window.y),
            window_maximized: Some(self.window.maximized),
            hwdec: (!self.hwdec.is_empty() && self.hwdec != HWDEC_DEFAULT)
                .then(|| self.hwdec.clone()),
            video_mode: (!self.video_mode.is_empty()).then(|| self.video_mode.clone()),
            // Unconditional: any file this build writes is by definition
            // written in the new names, whatever was loaded.
            video_mode_migrated: Some(true),
            video_mode_libraries: (!self.video_mode_libraries.is_empty())
                .then(|| self.video_mode_libraries.clone()),
            transcode_notice: (!self.transcode_notice.is_empty())
                .then(|| self.transcode_notice.clone()),
            audio_passthrough: (!self.audio_passthrough.is_empty())
                .then(|| self.audio_passthrough.clone()),
            audio_exclusive: self.audio_exclusive.then_some(true),
            audio_channels: (!self.audio_channels.is_empty()).then(|| self.audio_channels.clone()),
            disable_gpu_compositing: self.disable_gpu_compositing.then_some(true),
            transparent_titlebar: (!self.transparent_titlebar).then_some(false),
            log_level: (!self.log_level.is_empty()).then(|| self.log_level.clone()),
            force_transcoding: self.force_transcoding.then_some(true),
            window_decorations: self.window_decorations,
            hide_scrollbar: (!self.hide_scrollbar).then_some(false),
            device_name: (!self.device_name.is_empty()).then(|| self.device_name.clone()),
        }
    }

    fn cli_json(&self, hwdec_opts: &[&str]) -> String {
        let view = CliSettings {
            hwdec: (!self.hwdec.is_empty()).then_some(self.hwdec.as_str()),
            video_mode: (!self.video_mode.is_empty()).then_some(self.video_mode.as_str()),
            video_mode_libraries: (!self.video_mode_libraries.is_empty())
                .then_some(&self.video_mode_libraries),
            transcode_notice: (!self.transcode_notice.is_empty())
                .then_some(self.transcode_notice.as_str()),
            audio_passthrough: (!self.audio_passthrough.is_empty())
                .then_some(self.audio_passthrough.as_str()),
            audio_exclusive: self.audio_exclusive.then_some(true),
            audio_channels: (!self.audio_channels.is_empty())
                .then_some(self.audio_channels.as_str()),
            disable_gpu_compositing: self.disable_gpu_compositing.then_some(true),
            transparent_titlebar: (!self.transparent_titlebar).then_some(false),
            log_level: (!self.log_level.is_empty()).then_some(self.log_level.as_str()),
            force_transcoding: self.force_transcoding,
            hide_scrollbar: self.hide_scrollbar,
            device_name: (!self.device_name.is_empty()).then_some(self.device_name.as_str()),
            device_name_default: default_device_name(),
            hwdec_options: hwdec_opts,
            hwdec_default: HWDEC_DEFAULT,
        };
        serde_json::to_string(&view).unwrap_or_default()
    }
}

struct State {
    data: SettingsData,
    path: PathBuf,
}

fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| {
        Mutex::new(State {
            data: SettingsData::default(),
            path: PathBuf::new(),
        })
    })
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static SAVE_LOCK: Mutex<()> = Mutex::new(());

// Single persistent background save worker. save_async() coalesces into
// SavePending::data (only the newest snapshot survives); the worker wakes,
// writes the latest snapshot, then sleeps. Shutdown drains any queued write
// and joins the thread so nothing is lost at exit.

/// Coalescing slot for the background writer: only the newest snapshot
/// survives, and `stop` both drains the slot and ends the worker.
struct SavePending {
    data: Option<SettingsData>,
    path: PathBuf,
    stop: bool,
}

struct SaveWorker {
    mailbox: Mailbox<SavePending>,
    /// `Some` exactly while the worker thread is running; taken by shutdown.
    handle: Mutex<Option<JoinHandle<()>>>,
}

static SAVE_WORKER: OnceLock<SaveWorker> = OnceLock::new();

fn save_worker() -> &'static SaveWorker {
    SAVE_WORKER.get_or_init(|| SaveWorker {
        mailbox: Mailbox::new(SavePending {
            data: None,
            path: PathBuf::new(),
            stop: false,
        }),
        handle: Mutex::new(None),
    })
}

fn save_worker_loop(w: &'static SaveWorker) {
    // A stop with a snapshot still queued writes it, then exits on the next
    // pass with an empty slot.
    while let Some((data, path)) = w.mailbox.wait(
        |p| p.data.is_some() || p.stop,
        |p| p.data.take().map(|d| (d, p.path.clone())),
    ) {
        save_data(&path, &data);
    }
}

fn save_data(path: &Path, data: &SettingsData) -> bool {
    let Ok(mut text) = serde_json::to_string_pretty(&data.to_file()) else {
        return false;
    };
    text.push('\n');
    let _guard = SAVE_LOCK.lock();
    jfn_paths::write_atomic(path, text.as_bytes()).is_ok()
}

// =====================================================================
// Public Rust API
// =====================================================================

/// Initialize the settings store with the on-disk path. Idempotent: only the
/// first call sets the path; subsequent calls are ignored.
pub fn settings_init(path: &Path) {
    let mut st = state().lock();
    if st.path.as_os_str().is_empty() {
        st.path = path.to_path_buf();
    }
}

/// Load settings from the configured path. Missing keys keep their defaults.
/// Returns false if the file is missing or contains invalid JSON.
///
/// Everything the read has to complain about is buffered for
/// [`take_load_notices`] rather than logged: the first call happens before
/// logging exists, since the log level comes out of the file being read.
pub fn settings_load() -> bool {
    let mut st = state().lock();
    let path = st.path.clone();
    let Some(file) = read_file(&path) else {
        return false;
    };
    st.data.overlay(file);
    true
}

/// [`settings_load`] without the process-global store, so the file half can be
/// tested against a temp directory.
///
/// `None` covers everything the *parser* rejects: a missing file, a
/// directory, bytes that are not UTF-8, a truncated or empty document, nesting
/// past serde_json's recursion limit — and, less obviously, a duplicate key or
/// a numeric literal outside the `f64` range, both of which fail the whole
/// document before [`lenient`] ever sees the field. What [`lenient`] does
/// absorb, key by key, is an unknown key, a key of the wrong type, and a
/// number that does not fit its own field; those keep their defaults and the
/// rest of the file still loads.
fn read_file(path: &Path) -> Option<SettingsFile> {
    // Checked before the read, not after: `read_to_string` on a file that has
    // grown to gigabytes is the failure, not a way to detect it.
    if let Ok(meta) = fs::metadata(path)
        && !settings_size_ok(meta.len())
    {
        let shown = path.display();
        let len = meta.len();
        note(
            NoticeLevel::Warn,
            format!(
                "settings file {shown} is {len} bytes, past the {MAX_SETTINGS_BYTES}-byte limit; \
                 ignoring it, every setting at its default"
            ),
        );
        return None;
    }
    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            let shown = path.display();
            note(
                NoticeLevel::Warn,
                format!("settings file {shown} unreadable: {e}"),
            );
            return None;
        }
    };
    match serde_json::from_str::<SettingsFile>(&contents) {
        Ok(file) => Some(file),
        Err(e) => {
            // Loud on purpose, once there is a log to be loud in: the file is
            // hand-edited, and a whole-document
            // failure (duplicate key, out-of-range number, truncation) means
            // every setting silently falls back to its default and the next
            // save overwrites the file with those defaults.
            let shown = path.display();
            note(
                NoticeLevel::Warn,
                format!("settings file {shown} ignored, every setting at its default: {e}"),
            );
            None
        }
    }
}

/// Serialize current state and atomically write to the configured path.
pub fn settings_save() -> bool {
    let (path, snap) = {
        let st = state().lock();
        (st.path.clone(), st.data.clone())
    };
    save_data(&path, &snap)
}

/// Snapshot current state and hand it to the background save worker. Repeated
/// calls coalesce: only the most recent snapshot is written. The worker is
/// started lazily on the first call. After [`settings_shutdown_save_worker`]
/// this becomes a no-op.
pub fn settings_save_async() {
    let (path, snap) = {
        let st = state().lock();
        (st.path.clone(), st.data.clone())
    };
    let w = save_worker();
    // Hold `handle` across the spawn so a second caller racing in between the
    // enqueue and the JoinHandle store can't observe a started worker before
    // the thread actually exists.
    let mut handle = w.handle.lock();
    let queued = w.mailbox.update(|p| {
        if p.stop {
            return false;
        }
        p.data = Some(snap);
        p.path = path;
        true
    });
    if queued && handle.is_none() {
        *handle = Some(thread::spawn(|| save_worker_loop(save_worker())));
    }
}

/// Stop the background save worker after draining any pending write. Safe to
/// call if the worker was never started; safe to call multiple times.
pub fn settings_shutdown_save_worker() {
    let Some(w) = SAVE_WORKER.get() else {
        return;
    };
    if !w.mailbox.update(|p| !std::mem::replace(&mut p.stop, true)) {
        return;
    }
    let handle = w.handle.lock().take();
    if let Some(h) = handle
        && let Err(e) = h.join()
    {
        eprintln!("[config] save worker panicked: {e:?}");
    }
}

macro_rules! string_accessors {
    ($getter:ident, $setter:ident, $field:ident) => {
        pub fn $getter() -> String {
            state().lock().data.$field.clone()
        }
        pub fn $setter(v: &str) {
            state().lock().data.$field = v.to_string();
        }
    };
}

macro_rules! bool_accessors {
    ($getter:ident, $setter:ident, $field:ident) => {
        pub fn $getter() -> bool {
            state().lock().data.$field
        }
        pub fn $setter(v: bool) {
            state().lock().data.$field = v;
        }
    };
}

string_accessors!(server_url, set_server_url, server_url);
string_accessors!(hwdec, set_hwdec, hwdec);
// video_mode: upscaling preset key (`auto` | `live-action` | `animation` |
// `off`). Empty means "never chosen"; the caller resolves that to the built-in
// default.
string_accessors!(video_mode, set_video_mode, video_mode);

/// False when the loaded `settings.json` predates the video-mode rename, so
/// its `videoMode` still uses the old names *and* its `off` means the old
/// "leave mpv.conf alone". The caller normalises once and saves; every file
/// this build writes carries the marker.
#[must_use]
pub fn video_mode_migrated() -> bool {
    state().lock().data.video_mode_migrated
}
// transcode_notice: which transcodes raise the warning toast at playback
// start (`off` | `cpu` | `any`). Empty means "never chosen"; the web UI
// resolves that to `cpu`.
string_accessors!(transcode_notice, set_transcode_notice, transcode_notice);
string_accessors!(audio_passthrough, set_audio_passthrough, audio_passthrough);
string_accessors!(audio_channels, set_audio_channels, audio_channels);
string_accessors!(log_level, set_log_level, log_level);

pub fn device_name() -> String {
    state().lock().data.device_name.clone()
}

/// Clamp to the server's 64-byte DeviceName column, never splitting a
/// character.
fn truncate_device_name(s: &mut String) {
    if s.len() <= DEVICE_NAME_MAX {
        return;
    }
    let mut end = DEVICE_NAME_MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

#[cfg(unix)]
pub fn default_device_name() -> String {
    let mut s = gethostname::gethostname().to_string_lossy().into_owned();
    truncate_device_name(&mut s);
    s
}

#[cfg(windows)]
pub fn default_device_name() -> String {
    let mut s = std::env::var("COMPUTERNAME").unwrap_or_default();
    truncate_device_name(&mut s);
    s
}

/// Setter for device_name. Trims and collapses whitespace, truncates to the
/// server's 64-char DeviceName column limit, and clears the override when the
/// result matches `platform_default` (so hostname changes propagate
/// automatically on the next launch).
pub fn set_device_name(raw: &str, platform_default: &str) {
    let cleaned = normalize_device_name(raw, platform_default);
    state().lock().data.device_name = cleaned;
}

bool_accessors!(audio_exclusive, set_audio_exclusive, audio_exclusive);
bool_accessors!(
    disable_gpu_compositing,
    set_disable_gpu_compositing,
    disable_gpu_compositing
);
bool_accessors!(
    transparent_titlebar,
    set_transparent_titlebar,
    transparent_titlebar
);
bool_accessors!(force_transcoding, set_force_transcoding, force_transcoding);
/// The user's explicit decoration choice, unresolved; `None` when unset.
pub fn configured_window_decorations() -> Option<WindowDecorations> {
    state().lock().data.window_decorations
}

/// The effective decoration mode: the user's choice, resolved against what
/// the installed `Platform` can actually honour.
pub fn window_decorations_mode() -> WindowDecorations {
    let configured = state().lock().data.window_decorations;
    resolve_decorations(configured)
}

/// Resolve a stored choice with, or without, a `Platform`.
///
/// The CEF renderer process links this crate — it reads `settings.json` itself
/// to build the injected settings blob — and installs no backend, so
/// `jfn_platform_abi::get()` used to turn any call of the four decoration
/// accessors there into a panic inside a helper process. Client-side
/// decorations are the answer when nobody can be asked: every backend supports
/// them (`DecorationOptions::contains` is unconditionally true for `Csd`),
/// which makes them the one mode that cannot be wrong, and an explicit choice
/// is still reported as made.
fn resolve_decorations(configured: Option<WindowDecorations>) -> WindowDecorations {
    match jfn_platform_abi::try_get() {
        Some(platform) => platform.resolve_window_decorations(configured),
        None => configured.unwrap_or(WindowDecorations::Csd),
    }
}

pub fn window_decorations() -> String {
    window_decorations_mode().as_str().to_string()
}
pub fn set_window_decorations(v: Option<&str>) {
    state().lock().data.window_decorations = v.and_then(WindowDecorations::parse);
}

/// True when the app draws its own (client-side) titlebar.
pub fn client_side_decorations() -> bool {
    window_decorations_mode() == WindowDecorations::Csd
}
pub fn titlebar_theme_color() -> bool {
    window_decorations_mode() == WindowDecorations::ServerThemed
}
bool_accessors!(hide_scrollbar, set_hide_scrollbar, hide_scrollbar);

pub fn window_geometry() -> JfnWindowGeometry {
    state().lock().data.window
}

pub fn set_window_geometry(g: JfnWindowGeometry) {
    state().lock().data.window = g;
}

pub fn cli_json(hwdec_opts: &[&str]) -> String {
    let snap = state().lock().data.clone();
    snap.cli_json(hwdec_opts)
}

fn normalize_device_name(raw: &str, platform_default: &str) -> String {
    // Server's auth header parser preserves whitespace verbatim, so " foo "
    // would round-trip into the Devices table.
    let mut trimmed = String::with_capacity(raw.len());
    let mut in_space = true;
    for c in raw.chars() {
        let ws = matches!(c, ' ' | '\t' | '\r' | '\n' | '\u{0b}' | '\u{0c}');
        if ws {
            if !in_space {
                trimmed.push(' ');
            }
            in_space = true;
        } else {
            trimmed.push(c);
            in_space = false;
        }
    }
    if trimmed.ends_with(' ') {
        trimmed.pop();
    }
    truncate_device_name(&mut trimmed);
    if trimmed == platform_default {
        trimmed.clear();
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::{
        BTreeMap, MAX_SETTINGS_BYTES, ScaleCheck, SettingsData, SettingsFile, WINDOW_SCALE_MAX,
        WINDOW_SCALE_MIN, WindowDecorations, check_window_scale, default_device_name,
        normalize_device_name, settings_size_ok,
    };

    const PLATFORM: &str = "platform-host";

    /// Top-level keys in the order they appear in the text; `serde_json::Value`
    /// would reorder them.
    fn keys(json: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut depth = 0usize;
        let mut chars = json.chars().peekable();
        let mut current = String::new();
        let mut in_string = false;
        while let Some(c) = chars.next() {
            if in_string {
                match c {
                    '\\' => {
                        chars.next();
                    }
                    '"' => in_string = false,
                    _ => current.push(c),
                }
                continue;
            }
            match c {
                '"' => {
                    in_string = true;
                    current.clear();
                }
                ':' if depth == 1 => out.push(std::mem::take(&mut current)),
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                _ => {}
            }
        }
        out
    }

    fn loaded(json: &str) -> SettingsData {
        let file: SettingsFile = serde_json::from_str(json).expect("valid json");
        let mut data = SettingsData::default();
        data.overlay(file);
        data
    }

    #[test]
    fn default_settings_write_only_server_url_and_maximized() {
        let text = serde_json::to_string(&SettingsData::default().to_file()).expect("serializes");
        // videoModeMigrated is the one unconditional key: it marks the file
        // as written in the post-rename video-mode names.
        assert_eq!(
            text,
            r#"{"serverUrl":"","windowMaximized":false,"videoModeMigrated":true}"#
        );
    }

    #[test]
    fn every_key_writes_in_schema_order() {
        let data = SettingsData {
            server_url: "http://host".into(),
            hwdec: "vaapi".into(),
            video_mode: "animation".into(),
            video_mode_migrated: true,
            video_mode_libraries: BTreeMap::from([("lib1".into(), "animation".into())]),
            transcode_notice: "any".into(),
            audio_passthrough: "eac3".into(),
            audio_channels: "stereo".into(),
            log_level: "debug".into(),
            device_name: "box".into(),
            window: super::JfnWindowGeometry {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
                logical_width: 5,
                logical_height: 6,
                scale: 1.5,
                maximized: true,
            },
            audio_exclusive: true,
            disable_gpu_compositing: true,
            transparent_titlebar: false,
            force_transcoding: true,
            window_decorations: Some(WindowDecorations::ServerThemed),
            hide_scrollbar: false,
        };
        let text = serde_json::to_string(&data.to_file()).expect("serializes");
        assert_eq!(
            keys(&text),
            [
                "serverUrl",
                "windowWidth",
                "windowHeight",
                "windowLogicalWidth",
                "windowLogicalHeight",
                "windowScale",
                "windowX",
                "windowY",
                "windowMaximized",
                "hwdec",
                "videoMode",
                "videoModeMigrated",
                "videoModeLibraries",
                "transcodeNotice",
                "audioPassthrough",
                "audioExclusive",
                "audioChannels",
                "disableGpuCompositing",
                "transparentTitlebar",
                "logLevel",
                "forceTranscoding",
                "windowDecorations",
                "hideScrollbar",
                "deviceName",
            ]
        );
        assert!(text.contains(r#""windowDecorations":"serverThemed""#));
        assert!(text.contains(r#""windowScale":1.5"#));
    }

    #[test]
    fn absent_keys_leave_defaults() {
        let data = loaded(r#"{"serverUrl":"http://host"}"#);
        assert_eq!(data.server_url, "http://host");
        assert!(data.transparent_titlebar);
        assert!(data.hide_scrollbar);
        assert_eq!(data.window.x, -1);
    }

    /// The marker that tells the startup path whether `videoMode` still uses
    /// the pre-rename names (and whether its `off` meant "leave mpv.conf
    /// alone"). Only files written by this build have it.
    #[test]
    fn video_mode_migration_marker_tracks_the_file() {
        let legacy = loaded(r#"{"videoMode":"off"}"#);
        assert_eq!(legacy.video_mode, "off");
        assert!(!legacy.video_mode_migrated);

        let current = loaded(r#"{"videoMode":"off","videoModeMigrated":true}"#);
        assert!(current.video_mode_migrated);
    }

    #[test]
    fn video_mode_libraries_round_trip_untouched() {
        let data = loaded(r#"{"videoModeLibraries":{"abc":"animation","def":"live-action"}}"#);
        assert_eq!(
            data.video_mode_libraries.get("abc").map(String::as_str),
            Some("animation")
        );
        assert_eq!(
            data.video_mode_libraries.get("def").map(String::as_str),
            Some("live-action")
        );

        let text = serde_json::to_string(&data.to_file()).expect("serializes");
        assert!(text.contains(r#""videoModeLibraries":{"abc":"animation","def":"live-action"}"#));
        assert!(data.video_mode_libraries.len() == 2);
    }

    #[test]
    fn wrong_typed_key_is_ignored_and_rest_of_file_loads() {
        let data = loaded(r#"{"windowWidth":"wide","serverUrl":"http://host","hideScrollbar":7}"#);
        assert_eq!(data.window.width, 0);
        assert!(data.hide_scrollbar);
        assert_eq!(data.server_url, "http://host");
    }

    #[test]
    fn unknown_keys_and_unknown_decorations_are_ignored() {
        let data = loaded(r#"{"nope":1,"windowDecorations":"fancy","serverUrl":"u"}"#);
        assert_eq!(data.window_decorations, None);
        assert_eq!(data.server_url, "u");
    }

    #[test]
    fn overlong_device_name_loads_truncated_on_char_boundary() {
        let ascii = "x".repeat(100);
        let data = loaded(&format!(r#"{{"deviceName":"{ascii}"}}"#));
        assert_eq!(data.device_name, "x".repeat(64));

        let multibyte = "é".repeat(40);
        let data = loaded(&format!(r#"{{"deviceName":"{multibyte}"}}"#));
        assert!(data.device_name.len() <= 64);
        assert_eq!(data.device_name, "é".repeat(32));
    }

    #[test]
    fn cli_json_emits_the_web_ui_contract() {
        let data = SettingsData {
            hwdec: "vaapi".into(),
            video_mode: "live-action".into(),
            video_mode_libraries: BTreeMap::from([("lib1".into(), "animation".into())]),
            transcode_notice: "any".into(),
            transparent_titlebar: false,
            device_name: "box".into(),
            ..SettingsData::default()
        };
        let text = data.cli_json(&["no", "auto"]);
        assert_eq!(
            keys(&text),
            [
                "hwdec",
                "videoMode",
                "videoModeLibraries",
                "transcodeNotice",
                "transparentTitlebar",
                "forceTranscoding",
                "hideScrollbar",
                "deviceName",
                "deviceNameDefault",
                "hwdecOptions",
                "hwdecDefault",
            ]
        );
        assert!(text.contains(&format!(r#""hwdecDefault":"{}""#, crate::HWDEC_DEFAULT)));
        assert!(text.contains(r#""videoMode":"live-action""#));
        assert!(text.contains(r#""videoModeLibraries":{"lib1":"animation"}"#));
        assert!(text.contains(r#""transcodeNotice":"any""#));
        assert!(text.contains(r#""hwdecOptions":["no","auto"]"#));
        assert!(text.contains(&format!(
            r#""deviceNameDefault":"{}""#,
            default_device_name()
        )));
    }

    #[test]
    fn trims_leading_and_trailing_whitespace() {
        assert_eq!(normalize_device_name("  foo  ", PLATFORM), "foo");
        assert_eq!(normalize_device_name("\t\nfoo\r\n", PLATFORM), "foo");
    }

    #[test]
    fn collapses_internal_whitespace_runs() {
        assert_eq!(normalize_device_name("foo  bar", PLATFORM), "foo bar");
        assert_eq!(normalize_device_name("foo\t\tbar", PLATFORM), "foo bar");
        assert_eq!(
            normalize_device_name("foo \t\nbar   baz", PLATFORM),
            "foo bar baz"
        );
    }

    #[test]
    fn whitespace_only_is_empty() {
        assert_eq!(normalize_device_name("   \t\n  ", PLATFORM), "");
    }

    #[test]
    fn preserves_single_internal_spaces() {
        assert_eq!(
            normalize_device_name("Andrew's MacBook Pro", PLATFORM),
            "Andrew's MacBook Pro"
        );
    }

    #[test]
    fn clamps_to_64_chars() {
        let long_name = "x".repeat(100);
        assert_eq!(normalize_device_name(&long_name, PLATFORM), "x".repeat(64));
    }

    #[test]
    fn clamps_after_whitespace_normalization() {
        let padded = format!("  {}  ", "x".repeat(70));
        assert_eq!(normalize_device_name(&padded, PLATFORM).len(), 64);
    }

    #[test]
    fn clears_override_when_value_equals_platform_default() {
        assert_eq!(normalize_device_name(PLATFORM, PLATFORM), "");
    }

    #[test]
    fn clears_override_when_whitespace_padded_default() {
        let padded = format!("  {}  ", PLATFORM);
        assert_eq!(normalize_device_name(&padded, PLATFORM), "");
    }

    // =================================================================
    // Hostile settings.json
    // =================================================================

    use super::{read_file, save_data};

    fn seed(name: &str, body: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(name);
        std::fs::write(&path, body).expect("write");
        (tmp, path)
    }

    #[test]
    fn read_file_loads_a_well_formed_document() {
        let (_tmp, path) = seed("settings.json", br#"{"serverUrl":"http://host"}"#);
        let file = read_file(&path).expect("parsed");
        let mut data = SettingsData::default();
        data.overlay(file);
        assert_eq!(data.server_url, "http://host");
    }

    #[test]
    fn read_file_rejects_a_missing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(read_file(&tmp.path().join("absent.json")).is_none());
    }

    #[test]
    fn read_file_rejects_a_directory_where_the_file_belongs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.json");
        std::fs::create_dir(&path).expect("mkdir");
        assert!(read_file(&path).is_none());
    }

    #[test]
    fn read_file_rejects_an_empty_or_truncated_document() {
        let (_a, empty) = seed("settings.json", b"");
        assert!(read_file(&empty).is_none());

        let (_b, truncated) = seed("settings.json", br#"{"serverUrl":"http"#);
        assert!(read_file(&truncated).is_none());

        let (_c, half) = seed("settings.json", br#"{"serverUrl":"http://host","#);
        assert!(read_file(&half).is_none());
    }

    #[test]
    fn read_file_rejects_bytes_that_are_not_utf8() {
        let (_tmp, path) = seed("settings.json", &[b'{', 0xff, 0xfe, b'}']);
        assert!(read_file(&path).is_none());
    }

    /// Serde can build a struct out of a *sequence* as well as a map, so a
    /// top-level array is read as "every key at its default" instead of as an
    /// error. Harmless — nothing is carried over — but it means
    /// `settings_load` answers `true` for a file that is not a settings
    /// document. Every other top-level shape is refused.
    #[test]
    fn a_top_level_array_loads_as_all_defaults_and_other_shapes_are_refused() {
        for body in [&b"[]"[..], &b"[1,2,3]"[..]] {
            let (_tmp, path) = seed("settings.json", body);
            let mut data = SettingsData::default();
            data.overlay(read_file(&path).expect("an array fills the struct positionally"));
            assert_eq!(data.server_url, "");
            assert!(data.hide_scrollbar);
        }

        for body in [&b"\"x\""[..], &b"42"[..], &b"null"[..], &b"true"[..]] {
            let (_tmp, path) = seed("settings.json", body);
            assert!(read_file(&path).is_none(), "{body:?}");
        }
    }

    /// serde_json stops at its recursion limit, so a pathological document is
    /// a parse error rather than a blown stack.
    #[test]
    fn read_file_rejects_pathologically_nested_json_without_overflowing() {
        let body = format!("{{\"serverUrl\":{}}}", "[".repeat(100_000));
        let (_tmp, path) = seed("settings.json", body.as_bytes());
        assert!(read_file(&path).is_none());

        let arrays = format!("{}{}", "[".repeat(50_000), "]".repeat(50_000));
        let (_tmp2, path2) = seed("settings.json", arrays.as_bytes());
        assert!(read_file(&path2).is_none());
    }

    /// A byte-order mark is not JSON. It loses the file's settings rather than
    /// corrupting them: every key falls back to its default.
    #[test]
    fn read_file_rejects_a_bom_prefixed_document() {
        let mut body = vec![0xef, 0xbb, 0xbf];
        body.extend_from_slice(br#"{"serverUrl":"http://host"}"#);
        let (_tmp, path) = seed("settings.json", &body);
        assert!(read_file(&path).is_none());
    }

    /// Not last-one-wins: a repeated key is a hard parse error, raised by the
    /// derived `Deserialize` before [`lenient`] ever sees the field. The whole
    /// document is lost and every setting silently falls back to its default —
    /// which matters because `videoModeLibraries` is documented as
    /// hand-edited. Asserted as it is, not as it should be.
    #[test]
    fn a_duplicate_key_fails_the_whole_document() {
        let (_tmp, path) = seed(
            "settings.json",
            br#"{"serverUrl":"http://first","serverUrl":"http://last"}"#,
        );
        assert!(read_file(&path).is_none());
    }

    /// Same class, same consequence: a numeric literal that does not fit an
    /// `f64` is a *parser* error, so `lenient` never gets to drop just that
    /// key. One bad literal resets the profile.
    #[test]
    fn a_number_outside_the_f64_range_fails_the_whole_document() {
        let (_tmp, path) = seed(
            "settings.json",
            br#"{"windowScale":1e400,"serverUrl":"http://host"}"#,
        );
        assert!(read_file(&path).is_none());
    }

    #[test]
    fn numbers_that_do_not_fit_their_field_are_ignored() {
        let data = loaded(
            r#"{"windowWidth":99999999999999999999,"windowHeight":-99999999999999999999,
                "windowX":1.5,"serverUrl":"http://host"}"#,
        );
        assert_eq!(data.window.width, 0);
        assert_eq!(data.window.height, 0);
        assert_eq!(data.window.x, -1);
        assert_eq!(data.server_url, "http://host");
    }

    #[test]
    fn integer_extremes_that_do_fit_are_taken_as_given() {
        let data = loaded(r#"{"windowWidth":2147483647,"windowX":-2147483648}"#);
        assert_eq!(data.window.width, i32::MAX);
        assert_eq!(data.window.x, i32::MIN);
    }

    /// Replaces `an_f32_overflowing_window_scale_is_written_back_as_null`,
    /// which pinned the unvalidated behaviour: a scale too large for an `f32`
    /// used to reach the window code as an infinity and only degrade to the
    /// default on the *next* load. It is now clamped on the way in.
    #[test]
    fn a_window_scale_too_large_for_f32_is_clamped_instead_of_stored_as_an_infinity() {
        let data = loaded(r#"{"windowScale":1e39}"#);
        assert!((data.window.scale - WINDOW_SCALE_MAX).abs() < f32::EPSILON);

        let text = serde_json::to_string(&data.to_file()).expect("serializes");
        assert!(text.contains(r#""windowScale":4.0"#), "{text}");
    }

    /// Replaces `a_negative_window_scale_loads_and_is_dropped_on_save`: a
    /// non-positive scale is not clamped up to the minimum, because zero is
    /// also how "no saved scale" is spelled in memory.
    #[test]
    fn a_negative_or_zero_window_scale_is_ignored_rather_than_loaded() {
        for text in [r#"{"windowScale":-2.0}"#, r#"{"windowScale":0}"#] {
            let data = loaded(text);
            assert_eq!(data.window.scale, 0.0, "{text}");
            let written = serde_json::to_string(&data.to_file()).expect("serializes");
            assert!(!written.contains("windowScale"), "{written}");
        }
    }

    #[test]
    fn a_window_scale_inside_the_range_is_taken_as_given() {
        assert_eq!(check_window_scale(1.0), ScaleCheck::Ok(1.0));
        assert_eq!(check_window_scale(2.5), ScaleCheck::Ok(2.5));
        assert_eq!(
            check_window_scale(f64::from(WINDOW_SCALE_MIN)),
            ScaleCheck::Ok(WINDOW_SCALE_MIN)
        );
        assert_eq!(
            check_window_scale(f64::from(WINDOW_SCALE_MAX)),
            ScaleCheck::Ok(WINDOW_SCALE_MAX)
        );
        assert!((loaded(r#"{"windowScale":1.25}"#).window.scale - 1.25).abs() < f32::EPSILON);
    }

    #[test]
    fn a_window_scale_outside_the_range_is_pulled_to_the_nearer_end() {
        assert_eq!(
            check_window_scale(0.25),
            ScaleCheck::Clamped(WINDOW_SCALE_MIN)
        );
        assert_eq!(
            check_window_scale(100.0),
            ScaleCheck::Clamped(WINDOW_SCALE_MAX)
        );
        assert_eq!(
            check_window_scale(f64::MAX),
            ScaleCheck::Clamped(WINDOW_SCALE_MAX)
        );
    }

    #[test]
    fn a_window_scale_that_is_not_a_number_leaves_the_default() {
        for raw in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.0] {
            assert_eq!(check_window_scale(raw), ScaleCheck::Unusable, "{raw}");
        }
    }

    #[test]
    fn settings_size_ok_admits_a_real_file_and_refuses_a_grown_one() {
        assert!(settings_size_ok(0));
        assert!(settings_size_ok(4096));
        assert!(settings_size_ok(MAX_SETTINGS_BYTES));
        assert!(!settings_size_ok(MAX_SETTINGS_BYTES + 1));
    }

    /// Past the cap the file is unparseable, not partially loaded: every
    /// setting falls back to its default, exactly as a syntax error does.
    #[test]
    fn read_file_ignores_a_settings_file_past_the_size_cap() {
        let mut body = br#"{"serverUrl":"http://host","deviceName":"#.to_vec();
        body.push(b'"');
        body.extend(std::iter::repeat_n(b'x', MAX_SETTINGS_BYTES as usize));
        body.extend_from_slice(br#""}"#);
        let (_tmp, path) = seed("settings.json", &body);
        assert!(read_file(&path).is_none());
    }

    // =================================================================
    // Load notices
    // =================================================================

    use super::{LoadNotice, NoticeLevel, take_load_notices};

    /// What loading `json` buffers. The thread's buffer is cleared first so
    /// that a test sharing this thread with an earlier one cannot bleed into
    /// the assertion (`--test-threads=1` runs them back to back).
    fn notices_for(json: &str) -> Vec<LoadNotice> {
        let _ = take_load_notices();
        let _ = loaded(json);
        take_load_notices()
    }

    /// The clamp is the point of the notice: without it the window silently
    /// comes back at a size the file did not ask for.
    #[test]
    fn a_clamped_window_scale_buffers_a_warning_for_the_log() {
        let notices = notices_for(r#"{"windowScale":9.0}"#);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(notices[0].level, NoticeLevel::Warn);
        assert!(notices[0].message.contains("windowScale 9"), "{notices:?}");
        assert!(notices[0].message.contains("using 4"), "{notices:?}");
    }

    #[test]
    fn an_unusable_window_scale_buffers_a_warning_for_the_log() {
        let notices = notices_for(r#"{"windowScale":0.0}"#);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].message.contains("not a usable scale"),
            "{notices:?}"
        );
    }

    #[test]
    fn a_clean_document_buffers_nothing() {
        assert!(
            notices_for(r#"{"serverUrl":"http://host","windowScale":1.5}"#).is_empty(),
            "a file with nothing wrong with it must not produce a line"
        );
    }

    #[test]
    fn an_oversized_settings_file_buffers_a_warning_for_the_log() {
        let _ = take_load_notices();
        let mut body = br#"{"deviceName":"#.to_vec();
        body.push(b'"');
        body.extend(std::iter::repeat_n(b'x', MAX_SETTINGS_BYTES as usize));
        body.extend_from_slice(br#""}"#);
        let (_tmp, path) = seed("settings.json", &body);
        assert!(read_file(&path).is_none());

        let notices = take_load_notices();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(notices[0].level, NoticeLevel::Warn);
        assert!(notices[0].message.contains("past the"), "{notices:?}");
        assert!(notices[0].message.contains("limit"), "{notices:?}");
    }

    #[test]
    fn an_unparseable_settings_file_buffers_a_warning_for_the_log() {
        let _ = take_load_notices();
        let (_tmp, path) = seed("settings.json", br#"{"serverUrl":"#);
        assert!(read_file(&path).is_none());

        let notices = take_load_notices();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].message.contains("every setting at its default"),
            "{notices:?}"
        );
    }

    /// Taking is draining: the caller replays each line exactly once, so a
    /// second call must come back empty rather than repeat the first.
    #[test]
    fn take_load_notices_empties_the_buffer() {
        let _ = take_load_notices();
        let _ = loaded(r#"{"windowScale":9.0}"#);
        assert_eq!(take_load_notices().len(), 1);
        assert!(take_load_notices().is_empty());
    }

    /// The four decoration accessors are reachable from the CEF renderer,
    /// where no backend is ever installed; they used to panic there.
    #[test]
    fn the_decoration_accessors_answer_without_a_platform_installed() {
        assert!(
            jfn_platform_abi::try_get().is_none(),
            "this test binary must install no backend"
        );
        assert_eq!(super::resolve_decorations(None), WindowDecorations::Csd);
        assert_eq!(
            super::resolve_decorations(Some(WindowDecorations::ServerThemed)),
            WindowDecorations::ServerThemed
        );
    }

    #[test]
    fn a_null_value_leaves_the_field_at_its_default() {
        let data = loaded(r#"{"serverUrl":null,"hideScrollbar":null,"windowWidth":null}"#);
        assert_eq!(data.server_url, "");
        assert!(data.hide_scrollbar);
        assert_eq!(data.window.width, 0);
    }

    /// The map is hand-edited, so every shape of key has to survive a
    /// round-trip without touching anything else.
    #[test]
    fn video_mode_libraries_accepts_any_string_key() {
        let odd = r#"{"videoModeLibraries":{"":"animation","__proto__":"off",
            "a b/c\\d":"live-action","é中":"auto"},"serverUrl":"http://host"}"#;
        let data = loaded(odd);
        assert_eq!(data.video_mode_libraries.len(), 4);
        assert_eq!(
            data.video_mode_libraries.get("").map(String::as_str),
            Some("animation")
        );
        assert_eq!(
            data.video_mode_libraries
                .get("__proto__")
                .map(String::as_str),
            Some("off")
        );
        assert_eq!(data.server_url, "http://host");

        let text = serde_json::to_string(&data.to_file()).expect("serializes");
        assert_eq!(
            loaded(&text).video_mode_libraries,
            data.video_mode_libraries
        );
    }

    /// One bad entry drops the whole map rather than half of it: a partial
    /// override map would silently change which shaders a library plays with.
    #[test]
    fn video_mode_libraries_of_the_wrong_shape_is_dropped_whole() {
        for body in [
            r#"{"videoModeLibraries":{"a":1}}"#,
            r#"{"videoModeLibraries":{"a":{"b":"c"}}}"#,
            r#"{"videoModeLibraries":["a","b"]}"#,
            r#"{"videoModeLibraries":"animation"}"#,
        ] {
            let data = loaded(body);
            assert!(data.video_mode_libraries.is_empty(), "{body}");
        }
    }

    /// The value is never validated against the known presets — the web UI
    /// and the mpv side both resolve unknown names to the default.
    #[test]
    fn unknown_video_mode_and_transcode_notice_values_survive_the_load() {
        let data = loaded(r#"{"videoMode":"../../etc/passwd","transcodeNotice":"<script>"}"#);
        assert_eq!(data.video_mode, "../../etc/passwd");
        assert_eq!(data.transcode_notice, "<script>");
    }

    #[test]
    fn a_device_name_of_only_multibyte_characters_truncates_on_a_boundary() {
        let name = "\u{4e2d}".repeat(40);
        let data = loaded(&format!(r#"{{"deviceName":"{name}"}}"#));
        assert!(data.device_name.len() <= 64);
        assert_eq!(data.device_name.chars().count(), 21);
        assert!(data.device_name.chars().all(|c| c == '\u{4e2d}'));
    }

    // =================================================================
    // Persistence
    // =================================================================

    #[test]
    fn save_data_writes_pretty_json_with_a_trailing_newline() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.json");
        let data = SettingsData {
            server_url: "http://host".into(),
            ..SettingsData::default()
        };

        assert!(save_data(&path, &data));

        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.ends_with("}\n"), "{text:?}");
        assert!(
            text.contains("\n  \"serverUrl\": \"http://host\""),
            "{text}"
        );
        assert!(read_file(&path).is_some());
    }

    #[test]
    fn save_data_reports_failure_instead_of_panicking_when_the_directory_is_gone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("gone").join("settings.json");
        assert!(!save_data(&path, &SettingsData::default()));
        assert!(!path.exists());
    }

    #[test]
    fn a_full_settings_document_round_trips_through_the_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.json");
        let data = SettingsData {
            server_url: "http://host".into(),
            hwdec: "vaapi".into(),
            video_mode: "animation".into(),
            video_mode_migrated: true,
            video_mode_libraries: BTreeMap::from([("lib1".into(), "animation".into())]),
            transcode_notice: "any".into(),
            audio_passthrough: "eac3".into(),
            audio_channels: "stereo".into(),
            log_level: "debug".into(),
            device_name: "box".into(),
            window: super::JfnWindowGeometry {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
                logical_width: 5,
                logical_height: 6,
                scale: 1.5,
                maximized: true,
            },
            audio_exclusive: true,
            disable_gpu_compositing: true,
            transparent_titlebar: false,
            force_transcoding: true,
            window_decorations: Some(WindowDecorations::ServerThemed),
            hide_scrollbar: false,
        };

        assert!(save_data(&path, &data));
        let mut back = SettingsData::default();
        back.overlay(read_file(&path).expect("parsed"));

        assert_eq!(back.server_url, data.server_url);
        assert_eq!(back.hwdec, data.hwdec);
        assert_eq!(back.video_mode, data.video_mode);
        assert!(back.video_mode_migrated);
        assert_eq!(back.video_mode_libraries, data.video_mode_libraries);
        assert_eq!(back.transcode_notice, data.transcode_notice);
        assert_eq!(back.audio_passthrough, data.audio_passthrough);
        assert_eq!(back.audio_channels, data.audio_channels);
        assert_eq!(back.log_level, data.log_level);
        assert_eq!(back.device_name, data.device_name);
        assert_eq!(back.window.x, 1);
        assert_eq!(back.window.height, 4);
        assert!((back.window.scale - 1.5).abs() < f32::EPSILON);
        assert!(back.window.maximized);
        assert!(back.audio_exclusive);
        assert!(back.disable_gpu_compositing);
        assert!(!back.transparent_titlebar);
        assert!(back.force_transcoding);
        assert_eq!(
            back.window_decorations,
            Some(WindowDecorations::ServerThemed)
        );
        assert!(!back.hide_scrollbar);
    }

    /// The file is replaced, not written through: a symlink planted where
    /// `settings.json` belongs cannot redirect the write.
    #[cfg(unix)]
    #[test]
    fn save_data_replaces_a_symlinked_settings_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let victim = tmp.path().join("victim");
        std::fs::write(&victim, "do not touch").expect("seed");
        let path = tmp.path().join("settings.json");
        std::os::unix::fs::symlink(&victim, &path).expect("symlink");

        assert!(save_data(&path, &SettingsData::default()));

        assert_eq!(
            std::fs::read_to_string(&victim).expect("read"),
            "do not touch"
        );
        assert!(
            std::fs::read_to_string(&path)
                .expect("read")
                .contains("serverUrl")
        );
    }

    #[test]
    fn cli_json_survives_values_that_would_break_out_of_a_json_string() {
        let data = SettingsData {
            device_name: "</script><script>alert(1)".into(),
            video_mode: "a\"b\\c\nd".into(),
            ..SettingsData::default()
        };
        let text = data.cli_json(&[]);
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(
            parsed["deviceName"].as_str(),
            Some("</script><script>alert(1)")
        );
        assert_eq!(parsed["videoMode"].as_str(), Some("a\"b\\c\nd"));
        // serde_json does not escape `<`; whatever embeds this in a page must.
        assert!(text.contains("</script>"), "{text}");
    }

    #[test]
    fn truncate_device_name_never_splits_a_character() {
        let mut s = "\u{1f600}".repeat(20); // 4 bytes each
        super::truncate_device_name(&mut s);
        assert_eq!(s.len(), 64);
        assert_eq!(s.chars().count(), 16);

        let mut short = "abc".to_string();
        super::truncate_device_name(&mut short);
        assert_eq!(short, "abc");

        let mut empty = String::new();
        super::truncate_device_name(&mut empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn default_device_name_fits_the_server_column() {
        assert!(default_device_name().len() <= super::DEVICE_NAME_MAX);
    }

    #[test]
    fn normalize_device_name_folds_every_whitespace_form() {
        // Vertical tab and form feed are whitespace here; the rest of C0 is
        // not, and is left for the server to reject.
        assert_eq!(normalize_device_name("a\u{0b}\u{0c}b", PLATFORM), "a b");
        assert_eq!(normalize_device_name("a\r\nb", PLATFORM), "a b");
    }
}
