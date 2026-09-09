# A-B repeat loop

Repeat the selected bit. Set a begin point (A), set an end point (B), and
playback loops between them until you clear it.

## Controls

| | Action |
| --- | --- |
| `[` | Set A at the playhead. With A already set: seek back to A and resume if paused. |
| `]` | Set B at the playhead. |
| `\` | Clear both points. |
| the `repeat` button in the OSD control row | Cycles the same three steps: set A → set B → clear. It lights up in the accent colour while both points are set. |

Keys work while a video is playing and the focus is not in a text field. The
button sits next to the subtitle and settings buttons.

`]` is refused, with a toast and no change, when there is no A yet, when the
playhead is at or before A, or when the span would be shorter than half a
second.

## What you see

- A translucent accent band across the scrubber from A to B, with a small
  pin and an `A` / `B` label at each end. With A alone, just the A pin.
- A readout beside the "ends at" time: `A 1:23` with A only, `A↔B 1:23–1:45`
  while looping, nothing when neither point is set.
- A toast on set A, set B, clear, and on a refused `]`.

## What mpv guarantees

The loop *is* mpv's `ab-loop-a` / `ab-loop-b`
(`third_party/mpv/DOCS/man/options.rst`, `--ab-loop-a`), so its behaviour is
mpv's, not this client's:

- when playback passes B, mpv seeks to A;
- **seeking past B does not loop.** That is deliberate upstream behaviour, and
  it is how you leave a loop without clearing it: scrub past B and playback
  carries on;
- with either point unset — mpv spells that `no` — looping is off. Setting A
  alone marks the spot and arms nothing;
- if the points arrive out of order mpv treats them as if they had been given
  in the right order.

## Scope

- **Per item.** The points are cleared on every `playerLoad` and every
  `playerStop`, because mpv otherwise keeps `ab-loop-a` / `ab-loop-b` across
  files and a loop set on one episode would apply to the next.
- **Not persisted.** Nothing is written to `settings.json`; a loop lives as
  long as the item does.
- **Audio too, in principle** — mpv applies the properties to whatever it is
  playing — but the controls only appear over the video OSD.

## How it is wired

mpv is the authority (CLAUDE.md, "mpv Event Flow"). Nothing in the UI decides
where the playhead goes, and nothing in the UI decides what the loop is:

1. `src/web/ab-loop.js` sends `jmpNative.playerSetAbLoop(aMs, bMs)` — always
   both ends, `-1` for unset. It does *not* update its own state.
2. `src/jfn_cef/src/business_web.rs` routes that to
   `jfn_playback::ab_loop::jfn_playback_set_ab_loop_ms`, which converts to
   seconds and calls `jfn_mpv::api::jfn_mpv_set_ab_loop` — two async property
   writes, `a` first, `"no"` for an unset end.
3. `ab-loop-a` and `ab-loop-b` are observed process-wide
   (`src/playback/src/ingest_driver.rs`), under their own `reply_userdata` like
   the stats set. `src/playback/src/ab_loop.rs` folds each change in and, when
   the pair moved, pushes `window._nativeAbLoop(a, b)` in seconds (or `null`)
   and logs `ab-loop: a=… b=…` at INFO.
4. `ab-loop.js` renders whatever that push carried, and only that. An item
   change reaches the UI the same way: the native side clears the points on
   load, mpv reports the clear, and the band, readout and button empty
   themselves.

## Files

- `src/web/ab-loop.js`, tests in `ab-loop.test.js` (`just test-js`).
- CSS: the "A-B repeat loop" block in section (h) of
  `src/web/astrofin-theme.css`, next to the chapter markers. The band is
  inset by `--af-abloop-inset` (`0.54em`) to match jf-web's own
  `.sliderMarkerContainer`, so its percentages land on the same track the
  chapter stars do.
- `src/playback/src/ab_loop.rs` (observation, push, ms→seconds), Rust tests in
  the same file.
- `src/mpv/src/api.rs` — `jfn_mpv_set_ab_loop`.
- IPC name `playerSetAbLoop`, declared in `src/jfn_cef/src/injection.rs` and
  dispatched in `src/jfn_cef/src/business_web.rs`.
