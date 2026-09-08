//! Built-in upscaling presets ("Video mode"), switched live through libmpv.
//!
//! Four modes: [`VideoMode::Auto`] (the default — the web layer resolves a
//! concrete mode per title and applies it transiently),
//! [`VideoMode::LiveAction`] (FSRCNNX x2 plus sharp scalers),
//! [`VideoMode::Animation`] (Anime4K Mode A HQ), and [`VideoMode::Off`]
//! (no shaders at all, mpv's own default scalers).
//!
//! Everything is applied by writing mpv properties, so a switch takes effect
//! on the next rendered frame — no restart, no rewriting `mpv.conf`, and
//! nothing at all is written to disk from here.
//!
//! # Baseline
//!
//! [`VideoMode::Animation`] leaves the scalers to `mpv.conf`, because Anime4K
//! does its own downscale gating. That value is only known after
//! `mpv_initialize` has parsed the config, and reading it synchronously at
//! that point would park the caller on mpv's core thread while the VO is
//! still coming up. So [`init`] fires three *async* property reads and
//! [`consume_reply`] latches the answers.
//!
//! Ordering is what makes that safe: libmpv funnels
//! `mpv_get_property_async` and `mpv_set_property_async` through the same
//! dispatch queue (`run_async` → `mp_dispatch_enqueue` in `player/client.c`),
//! so the reads queued by [`init`] are served *before* the writes [`apply`]
//! queues right after them. The baseline therefore reflects the config file
//! even though the boot apply does not wait for it.
//!
//! [`VideoMode::Off`] deliberately does *not* use the baseline: it means "no
//! shaders", and the user's own `mpv.conf` may well set a shader chain of its
//! own, so Off writes an empty `glsl-shaders` plus mpv's compiled-in scaler
//! defaults over whatever the config file asked for.

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

/// Bundled subdirectory + file list for [`VideoMode::LiveAction`].
const FSRCNNX_DIR: &str = "fsrcnnx";
const FSRCNNX_FILES: &[&str] = &["FSRCNNX_x2_16-0-4-1.glsl"];

/// Bundled subdirectory + file list for [`VideoMode::Animation`]. Order is
/// the upstream Anime4K "Mode A (HQ)" recipe and is load-bearing.
const ANIME4K_DIR: &str = "anime4k";
const ANIME4K_FILES: &[&str] = &[
    "Anime4K_Clamp_Highlights.glsl",
    "Anime4K_Restore_CNN_VL.glsl",
    "Anime4K_Upscale_CNN_x2_VL.glsl",
    "Anime4K_AutoDownscalePre_x2.glsl",
    "Anime4K_AutoDownscalePre_x4.glsl",
    "Anime4K_Upscale_CNN_x2_M.glsl",
];

/// Scalers Live-Action mode pairs with FSRCNNX.
const LIVE_ACTION_SCALE: &str = "ewa_lanczossharp";
const LIVE_ACTION_DSCALE: &str = "mitchell";

/// mpv's own compiled-in scaler defaults, from `gl_video_opts_def` in
/// `video/out/gpu/video.c` (`SCALER_LANCZOS` / `SCALER_HERMITE`) and
/// documented in `DOCS/man/options.rst` under `--scale`/`--dscale`.
///
/// [`VideoMode::Off`] writes these rather than the `mpv.conf` baseline: the
/// user's config may pair a shader chain with sharp scalers, and "off" has to
/// undo the whole preset, not half of it.
const MPV_DEFAULT_SCALE: &str = "lanczos";
const MPV_DEFAULT_DSCALE: &str = "hermite";

/// Which upscaling preset is active.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VideoMode {
    /// Resolve per title at play time (tags, genres, library), falling back to
    /// [`VideoMode::LiveAction`]. The resolution itself happens in the web
    /// layer; this crate only ever receives the concrete result.
    #[default]
    Auto,
    /// FSRCNNX x2 plus sharp scalers.
    LiveAction,
    /// Anime4K Mode A (HQ).
    Animation,
    /// No shaders at all, and mpv's built-in default scalers.
    Off,
}

impl VideoMode {
    /// Wire value, as stored in `settings.json` and passed to `--video-mode`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::LiveAction => "live-action",
            Self::Animation => "animation",
            Self::Off => "off",
        }
    }

    /// Parse a wire value, accepting the pre-rename spellings (`movies`,
    /// `anime`) as aliases. `None` for anything else, so a stale or
    /// hand-edited setting falls back to the default instead of failing the
    /// load.
    ///
    /// Note that `off` maps to [`VideoMode::Off`] here. A *stored* `off`
    /// written before the rename meant "leave `mpv.conf` alone", which is not
    /// the same thing — see [`VideoMode::parse_legacy`].
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "live-action" | "movies" => Some(Self::LiveAction),
            "animation" | "anime" => Some(Self::Animation),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    /// [`VideoMode::parse`] for a value persisted by a build that predates the
    /// rename, where `off` meant "leave `mpv.conf` as it is" rather than
    /// "no shaders". The closest new behaviour is [`VideoMode::Auto`].
    ///
    /// Only ever applied once, to a `settings.json` that carries no
    /// `videoModeMigrated` marker; the normalised name is written straight
    /// back so this never runs twice.
    #[must_use]
    pub fn parse_legacy(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Auto),
            other => Self::parse(other),
        }
    }

    /// Every accepted wire value, in menu order.
    #[must_use]
    pub fn options() -> &'static [&'static str] {
        &["auto", "live-action", "animation", "off"]
    }

    /// The mode actually handed to mpv. [`VideoMode::Auto`] has no chain of
    /// its own: until the web layer resolves a title it behaves as
    /// [`VideoMode::LiveAction`], which is also Auto's documented fallback.
    #[must_use]
    pub fn effective(self) -> Self {
        match self {
            Self::Auto => Self::LiveAction,
            other => other,
        }
    }

    /// Shader file names this mode needs, in chain order.
    #[must_use]
    pub fn shader_files(self) -> &'static [&'static str] {
        match self.effective() {
            Self::Animation => ANIME4K_FILES,
            Self::Off => &[],
            _ => FSRCNNX_FILES,
        }
    }

    /// The bundled subdirectory holding this mode's shaders.
    #[must_use]
    pub fn bundled_subdir(self) -> &'static str {
        match self.effective() {
            Self::Animation => ANIME4K_DIR,
            Self::Off => "",
            _ => FSRCNNX_DIR,
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

/// One chain entry as mpv spells it: Windows paths in forward slashes (mpv
/// accepts them, it keeps `\` out of a value whose parser treats `\` as an
/// escape, and it is the form mpv echoes back when `glsl-shaders` is read).
///
/// Unescaped, so this is also what a log line should print: the escaping in
/// [`chain_to_property`] belongs to the list syntax, not to the path.
#[must_use]
fn normalise_entry(path: &Path) -> String {
    let text = path.to_string_lossy();
    // Only on Windows: elsewhere `\` is an ordinary filename byte.
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

/// mpv's path-list syntax for `chain`: [`normalise_entry`] per path, joined on
/// the platform separator, with any literal separator inside an entry escaped
/// the way `get_nextsep` in `options/m_option.c` expects.
///
/// The result is byte-identical to what mpv reports back when `glsl-shaders`
/// is read again (the read-back logged by [`consume_reply`]), which is what
/// makes the memo in [`apply_inner`] — and any future compare against mpv's
/// own value — a comparison of chains rather than of two spellings.
#[must_use]
pub fn chain_to_property(chain: &[PathBuf]) -> String {
    chain
        .iter()
        .map(|p| normalise_entry(p).replace(PATH_SEP, &format!("\\{PATH_SEP}")))
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

/// The three property values last written to mpv, in mpv's own spelling.
///
/// Compared as a unit: two modes can share a scaler pair and differ only in
/// the chain, and Animation's scalers come from the [`Baseline`], which lands
/// asynchronously *after* the boot apply — so the first resolution under Auto
/// legitimately re-applies the same chain with different scalers.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Applied {
    chain: String,
    scale: String,
    dscale: String,
}

/// Memo of the last *successful* write, so an apply that resolves to what mpv
/// is already running writes nothing.
///
/// Under Auto — the default — [`apply_resolved`] runs on every item start, and
/// every episode of one show resolves to the same mode, so without this the
/// whole chain is re-written once per episode.
///
/// This is hygiene, not a leak fix: mpv absorbs a redundant write by itself.
/// `glsl-shaders` is an `OPT_PATHLIST` with no `force_update`, so
/// `m_config_cache_write_opt` compares the list with `str_list_equal`, an
/// identical write never bumps the config timestamp `vo_gpu_next`'s
/// `update_options` gates on, and nothing is rebuilt — measured, not assumed
/// (`docs/memory-growth-findings.md` §5). What the memo buys is that the
/// behaviour is ours rather than borrowed from an mpv implementation detail,
/// that the log says what actually happened, and one fewer round trip through
/// libmpv's dispatch queue per item start.
#[derive(Default)]
struct Memo(Option<Applied>);

impl Memo {
    /// Whether `next` has to be written at all.
    fn should_apply(&self, next: &Applied) -> bool {
        self.0.as_ref() != Some(next)
    }

    /// Latch what mpv accepted. Only called once every write has succeeded: a
    /// failed write leaves the memo alone so the next apply retries.
    fn record(&mut self, next: Applied) {
        self.0 = Some(next);
    }

    /// Forget what mpv is running — for a fresh handle, whose properties are
    /// back at whatever `mpv.conf` said.
    fn clear(&mut self) {
        self.0 = None;
    }
}

struct State {
    /// What the user chose for this run: the stored setting, or the
    /// `--video-mode` override. Only [`apply`] moves it.
    selected: VideoMode,
    /// What mpv is actually running: `selected`, or — while `selected` is
    /// [`VideoMode::Auto`] — the mode last resolved for a title.
    mode: VideoMode,
    baseline: Baseline,
    /// What was last written for that mode; see [`Memo`].
    applied: Memo,
}

fn state() -> &'static Mutex<State> {
    static SLOT: std::sync::OnceLock<Mutex<State>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| {
        Mutex::new(State {
            selected: VideoMode::default(),
            mode: VideoMode::default(),
            baseline: Baseline::default(),
            applied: Memo::default(),
        })
    })
}

/// The mode last applied (or requested), per-title resolutions included.
#[must_use]
pub fn current() -> VideoMode {
    state().lock().mode
}

/// The mode chosen for this run — the stored setting or the CLI override —
/// which is what decides whether per-title resolutions are honoured.
#[must_use]
pub fn selected() -> VideoMode {
    state().lock().selected
}

/// Queue the three async baseline reads. Call once, immediately after
/// `mpv_initialize`, and *before* the first [`apply`] — see the module docs
/// for why the ordering is what makes the baseline correct.
pub fn init(handle: &Handle) {
    // A fresh handle is back on `mpv.conf`'s values, whatever we last wrote
    // to the previous one.
    state().lock().applied.clear();
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
    state().lock().selected = mode;
    apply_inner(handle, mode);
}

/// [`apply`] without moving `selected`: the shared tail for a user's choice
/// and for a per-title resolution under Auto.
///
/// Writes nothing when the resolved `(chain, scale, dscale)` is what mpv is
/// already running — see [`Memo`] for what that is and is not worth.
fn apply_inner(handle: &Handle, mode: VideoMode) {
    state().lock().mode = mode;
    let effective = mode.effective();

    let chain = match effective {
        VideoMode::Off => Vec::new(),
        _ => {
            let dir = effective.shader_dir();
            effective.shader_chain(&dir)
        }
    };
    let chain_value = chain_to_property(&chain);

    let (scale, dscale) = match effective {
        // Anime4K does its own downscale gating; leave the scalers to the
        // user's config.
        VideoMode::Animation => (
            baseline_field(|b| b.scale.clone()),
            baseline_field(|b| b.dscale.clone()),
        ),
        // Off means "no preset at all", so it puts mpv's own defaults back
        // rather than whatever mpv.conf asked for.
        VideoMode::Off => (
            MPV_DEFAULT_SCALE.to_string(),
            MPV_DEFAULT_DSCALE.to_string(),
        ),
        _ => (
            LIVE_ACTION_SCALE.to_string(),
            LIVE_ACTION_DSCALE.to_string(),
        ),
    };

    let label = if mode == VideoMode::Auto {
        "auto (live-action until a title is resolved)"
    } else {
        mode.as_str()
    };
    let shown = |value: &str| {
        if value.is_empty() {
            "<mpv default>".to_string()
        } else {
            value.to_string()
        }
    };
    let next = Applied {
        chain: chain_value,
        scale,
        dscale,
    };

    if !state().lock().applied.should_apply(&next) {
        tracing::info!(
            target: "mpv",
            "video mode {} unchanged, not re-applied: scale={} dscale={} shaders=[{}]",
            label,
            shown(&next.scale),
            shown(&next.dscale),
            chain.iter().map(|p| normalise_entry(p)).collect::<Vec<_>>().join(", ")
        );
        return;
    }

    let ok = set_string(handle, GLSL_SHADERS, &next.chain)
        & set_string(handle, SCALE, &next.scale)
        & set_string(handle, DSCALE, &next.dscale);
    request_string(handle, READBACK_REPLY, GLSL_SHADERS);

    tracing::info!(
        target: "mpv",
        "video mode {} applied: scale={} dscale={} shaders=[{}]",
        label,
        shown(&next.scale),
        shown(&next.dscale),
        chain.iter().map(|p| normalise_entry(p)).collect::<Vec<_>>().join(", ")
    );

    // Only latch a write mpv took: a rejected one leaves the property at its
    // old value, and memoising it would make every later apply a no-op.
    if ok {
        state().lock().applied.record(next);
    }
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

/// Apply a mode the web layer resolved for one title.
///
/// Honoured only while the mode [`selected`] for this run is
/// [`VideoMode::Auto`]: an explicit mode — stored or from `--video-mode` —
/// is a statement about every title, so a stale or racing resolution must
/// not undo it. `Auto` itself is not a resolution and is refused too.
///
/// Transient by construction: nothing is written to `settings.json`, and the
/// next `loadfile` resolves again from scratch. `reason` names the rule that
/// matched and `name` the title, so a run log says why a chain switched.
/// Returns whether the chain was switched.
pub fn apply_resolved(mode: VideoMode, reason: &str, name: &str) -> bool {
    let selected = selected();
    if selected != VideoMode::Auto {
        tracing::debug!(
            target: "mpv",
            "video mode {} resolved for \"{}\" ignored: mode is {}",
            mode.as_str(),
            name,
            selected.as_str()
        );
        return false;
    }
    if mode == VideoMode::Auto {
        tracing::warn!(
            target: "mpv",
            "video mode auto resolved to auto for \"{}\"; leaving the chain alone",
            name
        );
        return false;
    }
    let Some(handle) = crate::boot::current_handle() else {
        tracing::warn!(target: "mpv", "video mode {}: no mpv handle yet", mode.as_str());
        return false;
    };
    tracing::info!(
        target: "mpv",
        "video mode auto -> {} ({}) for \"{}\"",
        mode.as_str(),
        reason,
        name
    );
    apply_inner(&handle, mode);
    true
}

/// Boot-time bring-up against the process-global handle: latch the baseline,
/// then apply `mode`.
///
/// Call right after `mpv_initialize`, before anything is loaded — mpv holds
/// the chain as ordinary options, so it does not need a file to be open.
pub fn boot(mode: VideoMode) {
    let Some(handle) = crate::boot::current_handle() else {
        tracing::warn!(target: "mpv", "video mode: no mpv handle at boot");
        return;
    };
    init(&handle);
    apply(&handle, mode);
}

fn baseline_field(pick: impl Fn(&Baseline) -> Option<String>) -> String {
    pick(&state().lock().baseline).unwrap_or_default()
}

/// Queue one property write. Returns whether libmpv accepted the request —
/// the memo in [`apply_inner`] must not latch a value mpv refused.
fn set_string(handle: &Handle, name: &str, value: &str) -> bool {
    if let Err(e) = handle.set_property_string_async(0, name, value) {
        tracing::warn!(target: "mpv", "video mode: setting {name}={value:?} failed: {e:?}");
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_values_round_trip() {
        for mode in [
            VideoMode::Auto,
            VideoMode::LiveAction,
            VideoMode::Animation,
            VideoMode::Off,
        ] {
            assert_eq!(VideoMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(VideoMode::parse(""), None);
        assert_eq!(VideoMode::parse("Animation"), None);
        assert_eq!(VideoMode::parse("nope"), None);
    }

    #[test]
    fn legacy_names_map_onto_the_new_ones() {
        // Accepted everywhere, including a live setValue from an old page.
        assert_eq!(VideoMode::parse("movies"), Some(VideoMode::LiveAction));
        assert_eq!(VideoMode::parse("anime"), Some(VideoMode::Animation));
        assert_eq!(VideoMode::parse("off"), Some(VideoMode::Off));

        // A *stored* pre-rename value: `off` meant "leave mpv.conf alone",
        // whose closest new behaviour is auto, not the new shader-clearing off.
        assert_eq!(VideoMode::parse_legacy("off"), Some(VideoMode::Auto));
        assert_eq!(
            VideoMode::parse_legacy("movies"),
            Some(VideoMode::LiveAction)
        );
        assert_eq!(VideoMode::parse_legacy("anime"), Some(VideoMode::Animation));
        assert_eq!(VideoMode::parse_legacy("auto"), Some(VideoMode::Auto));
        assert_eq!(
            VideoMode::parse_legacy("live-action"),
            Some(VideoMode::LiveAction)
        );
        assert_eq!(VideoMode::parse_legacy("nope"), None);
    }

    #[test]
    fn default_is_auto_and_options_cover_every_variant() {
        assert_eq!(VideoMode::default(), VideoMode::Auto);
        for value in VideoMode::options() {
            assert!(VideoMode::parse(value).is_some(), "{value}");
        }
        assert_eq!(
            VideoMode::options(),
            ["auto", "live-action", "animation", "off"]
        );
    }

    #[test]
    fn auto_falls_back_to_live_action() {
        assert_eq!(VideoMode::Auto.effective(), VideoMode::LiveAction);
        assert_eq!(VideoMode::Auto.shader_files(), FSRCNNX_FILES);
        assert_eq!(VideoMode::Auto.bundled_subdir(), FSRCNNX_DIR);
        for mode in [VideoMode::LiveAction, VideoMode::Animation, VideoMode::Off] {
            assert_eq!(mode.effective(), mode);
        }
    }

    #[test]
    fn animation_chain_is_the_upstream_mode_a_order() {
        assert_eq!(
            VideoMode::Animation.shader_files(),
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
        assert_eq!(VideoMode::Off.bundled_subdir(), "");
    }

    /// Off has to be self-contained: mpv's own defaults, never the baseline,
    /// because the user's mpv.conf may itself set a shader chain and sharp
    /// scalers. Values from `gl_video_opts_def` in `video/out/gpu/video.c`.
    #[test]
    fn off_restores_mpvs_compiled_in_scaler_defaults() {
        assert_eq!(MPV_DEFAULT_SCALE, "lanczos");
        assert_eq!(MPV_DEFAULT_DSCALE, "hermite");
        assert!(VideoMode::Off.shader_chain(Path::new("/nope")).is_empty());
    }

    #[test]
    fn chain_resolution_keeps_order_and_drops_missing_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        // Everything except the third entry.
        for name in VideoMode::Animation.shader_files() {
            if *name != "Anime4K_Upscale_CNN_x2_VL.glsl" {
                std::fs::write(dir.join(name), "// s").expect("write");
            }
        }
        let chain = VideoMode::Animation.shader_chain(dir);
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

    /// The wire value is exactly the normalised entries joined on the
    /// separator, which is also what mpv echoes back from `glsl-shaders` — so
    /// a memo (or a future compare against mpv's own value) compares chains,
    /// not spellings.
    #[test]
    fn the_wire_value_is_the_normalised_entries_joined() {
        let chain = vec![
            PathBuf::from(if cfg!(windows) {
                r"C:\shaders\a.glsl"
            } else {
                "/shaders/a.glsl"
            }),
            PathBuf::from(if cfg!(windows) {
                r"C:\shaders\b.glsl"
            } else {
                "/shaders/b.glsl"
            }),
        ];
        let joined = chain
            .iter()
            .map(|p| normalise_entry(p))
            .collect::<Vec<_>>()
            .join(&PATH_SEP.to_string());
        assert_eq!(chain_to_property(&chain), joined);
        assert!(!joined.contains('\\'));
    }

    fn applied(chain: &str, scale: &str, dscale: &str) -> Applied {
        Applied {
            chain: chain.to_string(),
            scale: scale.to_string(),
            dscale: dscale.to_string(),
        }
    }

    /// A resolution that lands on what mpv is already running must not
    /// re-write `glsl-shaders`; anything else must.
    #[test]
    fn the_memo_skips_an_unchanged_chain_and_notices_every_change() {
        let anime = applied("a.glsl;b.glsl", "", "");
        let mut memo = Memo::default();

        // Nothing applied yet: the first write always goes out.
        assert!(memo.should_apply(&anime));
        memo.record(anime.clone());

        // The binge case — every later episode of the same show.
        assert!(!memo.should_apply(&anime));
        assert!(!memo.should_apply(&applied("a.glsl;b.glsl", "", "")));

        // Any one of the three fields differing is a real switch.
        assert!(memo.should_apply(&applied("c.glsl", "", "")));
        assert!(memo.should_apply(&applied("a.glsl;b.glsl", "ewa_lanczossharp", "")));
        assert!(memo.should_apply(&applied("a.glsl;b.glsl", "", "mitchell")));

        // Order matters: the same files in a different order is a different
        // chain, and mpv would render it differently.
        assert!(memo.should_apply(&applied("b.glsl;a.glsl", "", "")));

        // Off, then back to the same chain.
        let off = applied("", "lanczos", "hermite");
        assert!(memo.should_apply(&off));
        memo.record(off);
        assert!(memo.should_apply(&anime));

        // A fresh handle is back on mpv.conf's values.
        memo.record(anime.clone());
        assert!(!memo.should_apply(&anime));
        memo.clear();
        assert!(memo.should_apply(&anime));
    }
}
