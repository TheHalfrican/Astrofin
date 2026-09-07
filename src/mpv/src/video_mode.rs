//! Built-in upscaling presets ("Video mode"), switched live through libmpv.
//!
//! Each mode is a `glsl-shaders` chain plus, for [`VideoMode::Movies`], a pair
//! of scaler settings. Everything is applied by writing mpv properties, so a
//! switch takes effect on the next rendered frame — no restart, no rewriting
//! `mpv.conf`, and nothing at all is written to disk from here.
//!
//! # Baseline
//!
//! [`VideoMode::Off`] means "whatever the user's `mpv.conf` says", and Anime
//! mode leaves the scalers to `mpv.conf` as well. Those values are only known
//! after `mpv_initialize` has parsed the config, and reading them
//! synchronously at that point would park the caller on mpv's core thread
//! while the VO is still coming up. So [`init`] fires three *async* property
//! reads and [`consume_reply`] latches the answers.
//!
//! Ordering is what makes that safe: libmpv funnels
//! `mpv_get_property_async` and `mpv_set_property_async` through the same
//! dispatch queue (`run_async` → `mp_dispatch_enqueue` in `player/client.c`),
//! so the reads queued by [`init`] are served *before* the writes [`apply`]
//! queues right after them. The baseline therefore reflects the config file
//! even though the boot apply does not wait for it.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::event::ReplyUserdata;
use crate::handle::Handle;
use crate::sys;

/// Property holding the active user-shader chain.
const GLSL_SHADERS: &str = "glsl-shaders";
/// Upscaling filter.
const SCALE: &str = "scale";
/// Downscaling filter.
const DSCALE: &str = "dscale";

/// mpv's path-list separator: `;` on Windows, `:` everywhere else
/// (`OPTION_PATH_SEPARATOR` in `options/m_option.c`).
const PATH_SEP: char = if cfg!(windows) { ';' } else { ':' };

/// Reply ids for the async baseline reads. 1 is taken by
/// [`crate::api::BACKGROUND_COLOR_REPLY`].
pub const BASELINE_GLSL_REPLY: ReplyUserdata = 2;
pub const BASELINE_SCALE_REPLY: ReplyUserdata = 3;
pub const BASELINE_DSCALE_REPLY: ReplyUserdata = 4;
/// Reply id for the post-apply read-back, whose only job is to put what mpv
/// actually accepted into the log.
pub const READBACK_REPLY: ReplyUserdata = 5;

/// Bundled subdirectory + file list for [`VideoMode::Movies`].
const FSRCNNX_DIR: &str = "fsrcnnx";
const FSRCNNX_FILES: &[&str] = &["FSRCNNX_x2_16-0-4-1.glsl"];

/// Bundled subdirectory + file list for [`VideoMode::Anime`]. Order is the
/// upstream Anime4K "Mode A (HQ)" recipe and is load-bearing.
const ANIME4K_DIR: &str = "anime4k";
const ANIME4K_FILES: &[&str] = &[
    "Anime4K_Clamp_Highlights.glsl",
    "Anime4K_Restore_CNN_VL.glsl",
    "Anime4K_Upscale_CNN_x2_VL.glsl",
    "Anime4K_AutoDownscalePre_x2.glsl",
    "Anime4K_AutoDownscalePre_x4.glsl",
    "Anime4K_Upscale_CNN_x2_M.glsl",
];

/// Scalers Movies mode pairs with FSRCNNX.
const MOVIES_SCALE: &str = "ewa_lanczossharp";
const MOVIES_DSCALE: &str = "mitchell";

/// Which upscaling preset is active.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VideoMode {
    /// Touch nothing: `mpv.conf` (or mpv's own defaults) decide.
    Off,
    /// FSRCNNX x2 plus sharp scalers — the everyday default.
    #[default]
    Movies,
    /// Anime4K Mode A (HQ).
    Anime,
}

impl VideoMode {
    /// Wire value, as stored in `settings.json` and passed to `--video-mode`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Movies => "movies",
            Self::Anime => "anime",
        }
    }

    /// Parse a wire value. `None` for anything else, so a stale or hand-edited
    /// setting falls back to the default instead of failing the load.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "movies" => Some(Self::Movies),
            "anime" => Some(Self::Anime),
            _ => None,
        }
    }

    /// Every accepted wire value, in menu order.
    #[must_use]
    pub fn options() -> &'static [&'static str] {
        &["movies", "anime", "off"]
    }

    /// Shader file names this mode needs, in chain order.
    #[must_use]
    pub fn shader_files(self) -> &'static [&'static str] {
        match self {
            Self::Off => &[],
            Self::Movies => FSRCNNX_FILES,
            Self::Anime => ANIME4K_FILES,
        }
    }

    /// The bundled subdirectory holding this mode's shaders.
    #[must_use]
    pub fn bundled_subdir(self) -> &'static str {
        match self {
            Self::Off => "",
            Self::Movies => FSRCNNX_DIR,
            Self::Anime => ANIME4K_DIR,
        }
    }

    /// Where this mode's shaders load from: the user's own
    /// `<config dir>/mpv/shaders` when it holds all of them, else the bundled
    /// copy staged next to the binary.
    #[must_use]
    pub fn shader_dir(self) -> PathBuf {
        jfn_paths::shader_dir(self.bundled_subdir(), self.shader_files())
    }

    /// Resolve this mode's chain inside `shader_dir`, dropping (and naming)
    /// any file that is not there. An empty result means "no shaders": either
    /// [`VideoMode::Off`], or a mode whose files are missing — in both cases
    /// the caller clears `glsl-shaders` rather than pointing mpv at a path
    /// that does not exist.
    #[must_use]
    pub fn shader_chain(self, shader_dir: &Path) -> Vec<PathBuf> {
        let mut chain = Vec::with_capacity(self.shader_files().len());
        for name in self.shader_files() {
            let path = shader_dir.join(name);
            if path.is_file() {
                chain.push(path);
            } else {
                tracing::warn!(
                    target: "mpv",
                    "video mode {}: shader {} missing, skipping it",
                    self.as_str(),
                    path.display()
                );
            }
        }
        chain
    }
}

/// mpv's path-list syntax for `chain`: separator-joined, with Windows paths
/// spelled in forward slashes (mpv accepts them and it keeps `\` out of a
/// value whose parser treats `\` as an escape), and any literal separator
/// inside an entry escaped the way `get_nextsep` in `options/m_option.c`
/// expects.
#[must_use]
pub fn chain_to_property(chain: &[PathBuf]) -> String {
    chain
        .iter()
        .map(|p| {
            let text = p.to_string_lossy();
            // Only on Windows: elsewhere `\` is an ordinary filename byte.
            let text = if cfg!(windows) {
                text.replace('\\', "/")
            } else {
                text.into_owned()
            };
            text.replace(PATH_SEP, &format!("\\{PATH_SEP}"))
        })
        .collect::<Vec<_>>()
        .join(&PATH_SEP.to_string())
}

/// What `mpv.conf` (or mpv's defaults) had before any mode was applied.
#[derive(Default)]
struct Baseline {
    glsl_shaders: Option<String>,
    scale: Option<String>,
    dscale: Option<String>,
}

impl Baseline {
    fn complete(&self) -> bool {
        self.glsl_shaders.is_some() && self.scale.is_some() && self.dscale.is_some()
    }
}

struct State {
    mode: VideoMode,
    baseline: Baseline,
}

fn state() -> &'static Mutex<State> {
    static SLOT: std::sync::OnceLock<Mutex<State>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| {
        Mutex::new(State {
            mode: VideoMode::Off,
            baseline: Baseline::default(),
        })
    })
}

/// The mode last applied (or requested).
#[must_use]
pub fn current() -> VideoMode {
    state().lock().mode
}

/// Queue the three async baseline reads. Call once, immediately after
/// `mpv_initialize`, and *before* the first [`apply`] — see the module docs
/// for why the ordering is what makes the baseline correct.
pub fn init(handle: &Handle) {
    for (reply, name) in [
        (BASELINE_GLSL_REPLY, GLSL_SHADERS),
        (BASELINE_SCALE_REPLY, SCALE),
        (BASELINE_DSCALE_REPLY, DSCALE),
    ] {
        request_string(handle, reply, name);
    }
}

fn request_string(handle: &Handle, reply: ReplyUserdata, name: &str) {
    let Ok(c) = std::ffi::CString::new(name) else {
        return;
    };
    let rc = unsafe {
        sys::mpv_get_property_async(
            handle.raw(),
            reply,
            c.as_ptr(),
            sys::mpv_format::MPV_FORMAT_STRING,
        )
    };
    if rc < 0 {
        tracing::warn!(target: "mpv", "video mode: async read of {name} failed ({rc})");
    }
}

/// Fold one `GetPropertyReply` into the baseline, or log the read-back.
/// Returns true when the reply belonged to this module, so an event pump can
/// stop routing it further.
///
/// Safe from the mpv event thread: it only touches Rust state and `tracing`.
pub fn consume_reply(reply: ReplyUserdata, value: &crate::PropertyValue) -> bool {
    let text = match value {
        crate::PropertyValue::String(s) => Some(s.clone()),
        _ => None,
    };
    let field = match reply {
        BASELINE_GLSL_REPLY => GLSL_SHADERS,
        BASELINE_SCALE_REPLY => SCALE,
        BASELINE_DSCALE_REPLY => DSCALE,
        READBACK_REPLY => {
            tracing::info!(
                target: "mpv",
                "video mode {}: mpv reports glsl-shaders={}",
                current().as_str(),
                text.as_deref().unwrap_or("<unavailable>")
            );
            return true;
        }
        _ => return false,
    };
    // An unavailable property still latches, as `Some("")`: leaving it unset
    // would make the baseline permanently incomplete and block every later
    // restore.
    let text = text.unwrap_or_default();
    let mut st = state().lock();
    match reply {
        BASELINE_GLSL_REPLY => st.baseline.glsl_shaders = Some(text.clone()),
        BASELINE_SCALE_REPLY => st.baseline.scale = Some(text.clone()),
        _ => st.baseline.dscale = Some(text.clone()),
    }
    let complete = st.baseline.complete();
    drop(st);
    tracing::debug!(target: "mpv", "video mode baseline {field}={text:?} (complete={complete})");
    true
}

/// Apply `mode` to a live mpv handle and log what was resolved.
///
/// Writes are async and fire-and-forget; mpv recompiles the chain on the next
/// frame. Missing shader files are warned about and dropped — never a panic,
/// never a path handed to mpv that is not there.
///
/// Must not be called from an mpv event callback (it queues async requests,
/// which is fine, but keep the CLAUDE.md rule in view for future edits: no
/// sync property calls here).
pub fn apply(handle: &Handle, mode: VideoMode) {
    state().lock().mode = mode;

    let (chain_value, resolved) = match mode {
        // Restore whatever the config file had. Before the baseline reply has
        // landed the safest value is "no shaders": the alternative is leaving
        // the previous mode's chain running under a mode that says it is off.
        VideoMode::Off => (baseline_field(|b| b.glsl_shaders.clone()), Vec::new()),
        _ => {
            let dir = mode.shader_dir();
            let chain = mode.shader_chain(&dir);
            (chain_to_property(&chain), chain)
        }
    };

    let (scale, dscale) = match mode {
        VideoMode::Movies => (MOVIES_SCALE.to_string(), MOVIES_DSCALE.to_string()),
        // Anime4K does its own downscale gating; leave the scalers to the
        // user's config, exactly as Off does.
        _ => (
            baseline_field(|b| b.scale.clone()),
            baseline_field(|b| b.dscale.clone()),
        ),
    };

    set_string(handle, GLSL_SHADERS, &chain_value);
    set_string(handle, SCALE, &scale);
    set_string(handle, DSCALE, &dscale);
    request_string(handle, READBACK_REPLY, GLSL_SHADERS);

    let files: Vec<String> = resolved.iter().map(|p| p.display().to_string()).collect();
    tracing::info!(
        target: "mpv",
        "video mode {} applied: scale={} dscale={} shaders=[{}]",
        mode.as_str(),
        if scale.is_empty() { "<mpv default>" } else { &scale },
        if dscale.is_empty() { "<mpv default>" } else { &dscale },
        files.join(", ")
    );
}

/// Apply `mode` to the process-global handle. No-op before
/// [`crate::boot::jfn_mpv_handle_init`] has succeeded.
pub fn apply_current(mode: VideoMode) {
    let Some(handle) = crate::boot::current_handle() else {
        tracing::warn!(target: "mpv", "video mode {}: no mpv handle yet", mode.as_str());
        return;
    };
    apply(&handle, mode);
}

/// Boot-time bring-up against the process-global handle: latch the baseline,
/// then apply `mode`.
///
/// Call right after `mpv_initialize`, before anything is loaded — mpv holds
/// the chain as ordinary options, so it does not need a file to be open.
///
/// [`VideoMode::Off`] applies nothing at all: `mpv.conf` is already in force
/// and writing the (not yet known) baseline back over it would be a no-op at
/// best and a race at worst.
pub fn boot(mode: VideoMode) {
    let Some(handle) = crate::boot::current_handle() else {
        tracing::warn!(target: "mpv", "video mode: no mpv handle at boot");
        return;
    };
    init(&handle);
    if mode == VideoMode::Off {
        state().lock().mode = mode;
        tracing::info!(target: "mpv", "video mode off: leaving mpv.conf settings as they are");
        return;
    }
    apply(&handle, mode);
}

fn baseline_field(pick: impl Fn(&Baseline) -> Option<String>) -> String {
    pick(&state().lock().baseline).unwrap_or_default()
}

fn set_string(handle: &Handle, name: &str, value: &str) {
    if let Err(e) = handle.set_property_string_async(0, name, value) {
        tracing::warn!(target: "mpv", "video mode: setting {name}={value:?} failed: {e:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_values_round_trip() {
        for mode in [VideoMode::Off, VideoMode::Movies, VideoMode::Anime] {
            assert_eq!(VideoMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(VideoMode::parse(""), None);
        assert_eq!(VideoMode::parse("Movies"), None);
        assert_eq!(VideoMode::parse("nope"), None);
    }

    #[test]
    fn default_is_movies_and_options_cover_every_variant() {
        assert_eq!(VideoMode::default(), VideoMode::Movies);
        for value in VideoMode::options() {
            assert!(VideoMode::parse(value).is_some(), "{value}");
        }
        assert_eq!(VideoMode::options().len(), 3);
    }

    #[test]
    fn anime_chain_is_the_upstream_mode_a_order() {
        assert_eq!(
            VideoMode::Anime.shader_files(),
            [
                "Anime4K_Clamp_Highlights.glsl",
                "Anime4K_Restore_CNN_VL.glsl",
                "Anime4K_Upscale_CNN_x2_VL.glsl",
                "Anime4K_AutoDownscalePre_x2.glsl",
                "Anime4K_AutoDownscalePre_x4.glsl",
                "Anime4K_Upscale_CNN_x2_M.glsl",
            ]
        );
        assert!(VideoMode::Off.shader_files().is_empty());
    }

    #[test]
    fn chain_resolution_keeps_order_and_drops_missing_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        // Everything except the third entry.
        for name in VideoMode::Anime.shader_files() {
            if *name != "Anime4K_Upscale_CNN_x2_VL.glsl" {
                std::fs::write(dir.join(name), "// s").expect("write");
            }
        }
        let chain = VideoMode::Anime.shader_chain(dir);
        let names: Vec<String> = chain
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert_eq!(
            names,
            [
                "Anime4K_Clamp_Highlights.glsl",
                "Anime4K_Restore_CNN_VL.glsl",
                "Anime4K_AutoDownscalePre_x2.glsl",
                "Anime4K_AutoDownscalePre_x4.glsl",
                "Anime4K_Upscale_CNN_x2_M.glsl",
            ]
        );
    }

    #[test]
    fn property_syntax_joins_on_the_platform_separator() {
        let chain = vec![PathBuf::from("a"), PathBuf::from("b")];
        assert_eq!(chain_to_property(&chain), format!("a{PATH_SEP}b"));
        assert_eq!(chain_to_property(&[]), "");

        // A separator inside a path is escaped so mpv does not split on it.
        let odd = vec![PathBuf::from(format!("dir/x{PATH_SEP}y.glsl"))];
        assert_eq!(chain_to_property(&odd), format!("dir/x\\{PATH_SEP}y.glsl"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_are_spelled_with_forward_slashes() {
        let chain = vec![PathBuf::from(r"C:\shaders\a.glsl")];
        assert_eq!(chain_to_property(&chain), "C:/shaders/a.glsl");
    }
}
