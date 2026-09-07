# Bundled mpv GLSL shaders

These files back the built-in **Video mode** setting (Settings → Playback →
Video mode). They are plain mpv user shaders: data files that libplacebo
compiles at runtime when `glsl-shaders` names them. Nothing here is linked
into, or compiled with, the `astrofin` binary — see *Licensing* below.

`cargo xtask build` stages this directory as `shaders/` next to the binary
(`Contents/Resources/shaders/` inside the macOS bundle). At runtime the app
prefers a user override at `<config dir>/mpv/shaders/` when every file a mode
needs is present there, and otherwise falls back to this bundled copy. See
`docs/video-modes.md`.

## anime4k/ — Anime4K v4.0.1, Mode A (HQ)

Upstream: <https://github.com/bloc97/Anime4K>, release v4.0.1
(`Anime4K_v4.0.zip` → `GLSL_Mac_HighEnd` / `GLSL_Windows_HighEnd` share the
same shader sources). Files are byte-identical to the upstream release.

| File | Notice in header |
| --- | --- |
| `Anime4K_Clamp_Highlights.glsl` | MIT, © 2019-2021 bloc97 |
| `Anime4K_Restore_CNN_VL.glsl` | MIT, © 2019-2021 bloc97 |
| `Anime4K_Upscale_CNN_x2_VL.glsl` | MIT, © 2019-2021 bloc97 |
| `Anime4K_Upscale_CNN_x2_M.glsl` | MIT, © 2019-2021 bloc97 |
| `Anime4K_AutoDownscalePre_x2.glsl` | Unlicense (public domain) |
| `Anime4K_AutoDownscalePre_x4.glsl` | Unlicense (public domain) |

`LICENSE` holds the MIT text plus a pointer to the Unlicense text that the two
`AutoDownscalePre` files carry in their own headers.

The chain order matters and is the upstream "Mode A (HQ)" recipe:

```
Clamp_Highlights → Restore_CNN_VL → Upscale_CNN_x2_VL
                 → AutoDownscalePre_x2 → AutoDownscalePre_x4
                 → Upscale_CNN_x2_M
```

Every hook carries a `//!WHEN` guard on the output/native size ratio, so the
chain costs nothing when the video is already at (or above) display
resolution.

## fsrcnnx/ — FSRCNNX_x2_16-0-4-1

Upstream: <https://github.com/igv/FSRCNN-TensorFlow> (releases) — the
`FSRCNNX_x2_16-0-4-1.glsl` artefact, © 2017-2021 igv. Byte-identical to the
upstream file.

**License: LGPL-3.0-or-later**, as stated in the file's own header. The full
license text is included as `COPYING.LESSER` (LGPL-3.0) together with
`COPYING` (GPL-3.0), which the LGPL incorporates by reference.

### Licensing note

Astrofin itself is GPL-2.0 (see the repository `LICENSE`). The FSRCNNX shader
is *not* combined with the program: it is a data file that mpv/libplacebo
reads and compiles at run time, shipped alongside the binary and kept under
its own license with its notices intact. That is aggregation, not a combined
work, so the GPL-2 / LGPL-3 combination question does not arise. If a
distributor prefers not to carry it anyway, deleting `resources/shaders/fsrcnnx/`
is safe: the Movies mode then resolves no shader file, logs a warning, and
applies only its `scale`/`dscale` settings — and a user who drops their own
copy into `<config dir>/mpv/shaders/` gets the full chain back.

## Updating

Replace the files in place, keeping their upstream headers verbatim, then
update the version and the release link above. The file names are referenced
from `src/mpv/src/video_mode.rs`; renaming a file means editing that list too.
