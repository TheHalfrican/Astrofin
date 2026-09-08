# Video modes

**Video mode** is an in-app upscaling preset, in Settings → Playback. It
replaces the old out-of-repo workflow (a PowerShell script that rewrote
`mpv.conf` and two Start-menu shortcuts that restarted the app) with a setting
that switches **live**, mid-playback, through libmpv.

| Mode | `glsl-shaders` | `scale` | `dscale` |
| --- | --- | --- | --- |
| `auto` (default) | *per title* — see [Auto](#auto) | *per title* | *per title* |
| `live-action` | `FSRCNNX_x2_16-0-4-1.glsl` | `ewa_lanczossharp` | `mitchell` |
| `animation` | Anime4K v4.0.1 Mode A (HQ), six files in order | *baseline* | *baseline* |
| `off` | *(empty)* | `lanczos` | `hermite` |

*baseline* = whatever the user's `mpv.conf` (or mpv's own default) had at
startup; see [Baseline](#baseline). Anime4K does its own downscale gating, so
Animation deliberately leaves the scalers to the config file.

`off` is **not** "leave `mpv.conf` alone" — it clears the shader chain outright
and puts mpv's compiled-in scaler defaults back, because the user's own
`mpv.conf` may well set an FSRCNNX chain and sharp scalers of its own and "off"
has to undo the whole preset, not half of it. Those defaults are `lanczos` and
`hermite`, from `gl_video_opts_def` in
`third_party/mpv/video/out/gpu/video.c` (`SCALER_LANCZOS` / `SCALER_HERMITE`),
documented under `--scale` / `--dscale` in `DOCS/man/options.rst`.

Both shader chains gate themselves on the output/source size ratio — FSRCNNX
above ~1.3x, Anime4K above ~1.2x — so neither costs anything on content that is
already at display resolution. Leaving either mode on permanently is safe; that
is why the default is a shader mode rather than `off`.

## Auto

`auto` picks a concrete mode **per title**, immediately before mpv is told to
load the file, and applies it *transiently*: nothing is persisted, and the next
play resolves from scratch. Any other setting is a statement about every title,
so while the mode selected for this run — the stored setting, or a
`--video-mode` override — is not `auto`, the resolver does not run at all and
the native side (`video_mode::apply_resolved`, gated on `video_mode::selected`)
ignores a resolution that arrives anyway (a page script racing a settings
change, say).

Resolution lives in `src/web/video-mode-resolver.js` as a pure function,
`resolveVideoMode(item, series, library, settings) -> { mode, reason }`. First
rule that matches wins:

1. **Tag** on the item, then on its series/parent. Case-insensitive, matched
   against the whole tag:

   | Tag | Mode |
   | --- | --- |
   | `astrofin:animation`, `astrofin:anime` | `animation` |
   | `astrofin:live-action`, `astrofin:live` | `live-action` |

2. **Genre** on the item, then on its series/parent: `Animation` or `Anime`
   (case-insensitive, whole genre) → `animation`. Genres never select
   `live-action`; that is the fallback, so "no animation genre" and "a
   live-action genre" are the same thing.
3. **Library** — the item's top-level library, taken from
   `/Items/{id}/Ancestors?userId=` as the first ancestor with a
   `CollectionType`. (A `ParentId` walk cannot find it: a movie's parent is the
   physical media folder, whose parent is the aggregate root; the library is a
   virtual `CollectionFolder` that only the ancestors endpoint maps in.) First
   an explicit mapping from settings.json:

   ```json
   "videoModeLibraries": {
     "f137a2dd21bbc1b99aa5c0f6bf02a805": "animation",
     "a656b907eb3a73532e40e44b968d0225": "live-action"
   }
   ```

   The key is the library item's id (visible in the URL of the library page, or
   from `/Users/<id>/Views`); the value takes the same spellings as the setting,
   legacy names included. There is no UI for this — it is hand-edited, and the
   app writes it back untouched on every save.

   With no mapping, a name heuristic: a library whose name contains "anime" or
   "animation" (case-insensitive) → `animation`.
4. **Fallback**: `live-action`.

The series is only fetched (`ApiClient.getItem`) when the item's own tags and
genres decided nothing *and* it has a `SeriesId`; fetched series and ancestor
lists are cached for the session (ancestors by the nearest shared parent), so
a second episode of the same show costs no requests. Nothing here can stop
playback: any failure logs and falls back to `live-action`.

`jmpInfo` — and so `videoModeLibraries` — is built in the CEF renderer process
from its own read of `settings.json`. That process never parses argv, so the
browser process re-exports `--config-dir`/`--cache-dir` as
`ASTROFIN_CONFIG_DIR`/`ASTROFIN_CACHE_DIR` (`app.rs::jfn_app_main`) and
`jfn_paths` honours those variables; without that, a `--config-dir` run showed
the page the default profile's settings.

Each resolution writes one line to the log, naming the rule that matched:

```
INFO mpv: video mode auto -> animation (genre: Anime) for "Dragon Ball Z Kai"
INFO mpv: video mode auto -> live-action (default) for "Dune"
INFO mpv: video mode live-action applied: scale=ewa_lanczossharp dscale=mitchell shaders=[…]
```

Reason strings are `tag: <tag>`, `series tag: <tag>`, `genre: <genre>`,
`series genre: <genre>`, `library: <name>`, `library name: <name>`, `default`,
or `fallback after error: <message>`.

The path is JS → `jmpNative.setPlaybackVideoMode(mode, reason, name)`
(`NativeFunction::SetPlaybackVideoMode` in `src/jfn_cef/src/injection.rs`) →
`business_web::handle_playback_video_mode` →
`jfn_mpv::video_mode::apply_resolved`.

Before any title has been resolved — at boot, for instance — `auto` behaves as
`live-action`, which is also its documented fallback (`VideoMode::effective`).

Unit tests: `src/web/video-mode-resolver.test.js`, run with `just test-js` (or
`node --test src/web/video-mode-resolver.test.js`). They are node-only and are
deliberately not part of `just test`, which is the cargo suite.

## Where the shaders come from

Two locations, checked per mode:

1. **User override** — `<config dir>/mpv/shaders/`, flat. Used when *every*
   file the mode needs is present there. All-or-nothing per chain, so a user's
   updated Anime4K set is never half-mixed with the bundled one.
2. **Bundled** — `<resource dir>/shaders/<anime4k|fsrcnnx>/`, shipped with the
   app.

`<config dir>` is `%APPDATA%\astrofin` (Windows), `~/.config/astrofin`
(Linux/macOS), or whatever `--config-dir` says. `<resource dir>` is the
executable's own directory on Windows and Linux, and `Contents/Resources`
inside the macOS app bundle (`jfn_paths::resource_dir`).

`cargo xtask build` stages `resources/shaders/` into `build/shaders/`;
`cargo xtask install` carries it to the install prefix, the macOS installer
moves it into `Contents/Resources/shaders`, and the AppImage script copies it
next to the binary in `usr/bin/`.

A shader file that is missing from the resolved directory is logged at `warn`
and dropped from the chain; the rest of the chain still applies. Nothing here
ever panics or aborts a launch.

Provenance and licensing of the bundled files: `resources/shaders/README.md`.

## How the chain is applied

mpv keeps the chain in the `glsl-shaders` **path-list option**, so it is set by
writing that property — no file is loaded, no restart is needed, and mpv
recompiles on the next rendered frame.

- Value syntax: entries joined by mpv's path separator, `;` on Windows and `:`
  elsewhere (`OPTION_PATH_SEPARATOR` in `third_party/mpv/options/m_option.c`).
  Windows paths are written with forward slashes. A literal separator inside a
  path is escaped as `\<sep>`, which is what `get_nextsep` expects.
- The write is `mpv_set_property_async` with `reply_userdata == 0` —
  fire-and-forget, never a sync call (see the CLAUDE.md deadlock rule).
  `change-list glsl-shaders append` would also work but takes one command per
  file, leaving a frame or two of partial chain.
- Right after each apply the code re-reads `glsl-shaders` asynchronously and
  logs what came back, so a run log proves what mpv actually accepted:

```
INFO mpv: video mode animation applied: scale=<mpv default> dscale=<mpv default> shaders=[…]
INFO mpv: video mode animation: mpv reports glsl-shaders=…
```

Boot order (`src/jfn_rust/src/app.rs::run_app`): the baseline reads and the
first apply are queued straight after `mpv_initialize`, before any file is
loaded and before the VO wait — mpv holds the chain as plain options, so it is
in force for the first frame of the first video.

### Baseline

Animation mode's scalers mean "put back what `mpv.conf` said". Those values are
only knowable after `mpv_initialize` has parsed the config, and reading them
synchronously there would park the caller on mpv's core thread while the VO is
still coming up. So the module fires three async property reads
(`glsl-shaders`, `scale`, `dscale`) and latches the replies.

That is race-free because libmpv funnels `mpv_get_property_async` and
`mpv_set_property_async` through the same dispatch queue (`run_async` →
`mp_dispatch_enqueue` in `player/client.c`): the reads queued first are served
first, so the baseline reflects the config file even though the boot apply does
not wait for it. The replies are consumed by whichever pump is running — the
boot pump in `app.rs::consume_boot_event` before the ingest thread exists, and
`jfn_playback::ingest_driver::ingest_events` afterwards.

`off` deliberately does not use the baseline; see the table above.

## Setting, CLI flag, and the web UI

- **settings.json**: `"videoMode": "auto" | "live-action" | "animation" | "off"`.
  Absent means "never chosen" and resolves to the default (`auto`).
  `"videoModeLibraries"` is the per-library map described under
  [Auto](#auto). `"videoModeMigrated": true` is written unconditionally; see
  [Legacy names](#legacy-names).
- **CLI**: `--video-mode auto|live-action|animation|off`, overriding the stored
  value for that run. The pre-rename spellings are accepted too. An
  unrecognised value is ignored (the stored mode stands) rather than failing
  the launch.
- **Web UI**: `jmpInfo.settings.playback.videoMode` is readable by page
  scripts, and `jmpInfo.settingsDescriptions.playback` carries the select
  (Auto / Live-Action / Animation / Off). Changing it calls
  `window.api.settings.setValue('playback', 'videoMode', v)` → IPC
  `setSettingValue` → `business_common::apply_setting_value`, which persists
  the value *and* applies it to the live mpv handle. It is the only setting
  that takes effect without a restart.
- The Home server panel (`src/web/astrofin-theme.js`) shows the mode in its
  "Mode" row, from the same `jmpInfo` value. It re-renders on the theme's own
  refresh (route changes), and `client-settings.js` writes the new value into
  `jmpInfo` as it saves, so leaving Settings is enough to update it.

Switching from a page script, e.g. for testing:

```js
window.api.settings.setValue('playback', 'videoMode', 'animation');
```

## Legacy names

The modes used to be `movies | anime | off`, where `off` meant "leave
`mpv.conf` as it is". The mapping is:

| Old | New |
| --- | --- |
| `movies` | `live-action` |
| `anime` | `animation` |
| `off` *(stored, pre-rename)* | `auto` |

`movies` and `anime` are accepted anywhere a mode is parsed
(`VideoMode::parse`, `--video-mode`, `setValue`, and the
`videoModeLibraries` values) and are stored back under the new names.

`off` is the awkward one: the string is still valid, but it now means something
different from what a pre-rename `settings.json` meant by it. The
`videoModeMigrated` key disambiguates — every file this build writes carries it,
no file written before the rename does. On load, a `settings.json` without the
marker goes through `VideoMode::parse_legacy` once (`off` → `auto`), the
normalised name is written straight back, and the marker lands with it, so the
one-shot never runs twice and an explicitly chosen `off` survives a restart.
That happens in `app.rs::stored_video_mode`.

## Adding a mode

1. Put the shader files under `resources/shaders/<project>/` with their
   upstream headers and license intact, and describe them in
   `resources/shaders/README.md`.
2. In `src/mpv/src/video_mode.rs`: add the variant, its arm in `as_str`,
   `parse`, `options`, `shader_files`, and `bundled_subdir`, and — if it needs
   scalers — its arm in `apply`. The unit tests in that file cover the wire
   round-trip and the chain order.
3. Add the option to the `videoMode` select in `src/web/native-shim.js`, and a
   label to `VIDEO_MODE_LABELS` in `src/web/astrofin-theme.js`.

Nothing else needs touching: staging, the CLI flag, persistence, and the live
switch are all driven off those lists.
