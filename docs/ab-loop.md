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
second. Those three guards read the position the OSD has, which is enough to
tell a mistake from an intent; the point itself comes from mpv.

## What you see

- A band across the scrubber from A to B — a light wash with a hairline
  accent rule along its top and bottom edge — with a small pin and an `A` /
  `B` label at each end. With A alone, just the A pin.
- A readout beside the "ends at" time: `A 1:23` with A only, `A↔B 1:23–1:45`
  while looping, nothing when neither point is set.
- A toast on set A, set B, clear, and on a refused `]`. The set-A and set-B
  toasts name mpv's times and so appear when mpv reports the point, a frame
  or two after the key; they sit above the OSD bar rather than on it.

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
where the playhead goes, nothing in the UI decides what the loop is, and — the
point of the whole shape below — nothing in the UI decides *when* the points
are:

1. `src/web/ab-loop.js` sends `jmpNative.playerAbLoop(action)`, where `action`
   is `"set-a"`, `"set-b"` or `"clear"`. Never a time. It does *not* update
   its own state.
2. `src/jfn_cef/src/business_web.rs` routes that to
   `jfn_playback::ab_loop::jfn_playback_ab_loop_action`, which looks at the
   pair mpv last reported and either runs `no-osd ab-loop`
   (`jfn_mpv::api::jfn_mpv_ab_loop_cycle`) or writes `"no"` to both ends
   (`jfn_mpv_clear_ab_loop`). If the observed pair does not match the step
   asked for — a stale UI — it logs at DEBUG and writes nothing; the next
   observation push resyncs the UI.
3. `ab-loop-a` and `ab-loop-b` are observed process-wide
   (`src/playback/src/ingest_driver.rs`), under their own `reply_userdata` like
   the stats set. `src/playback/src/ab_loop.rs` folds each change in and, when
   the pair moved, pushes `window._nativeAbLoop(a, b)` in seconds (or `null`)
   and logs `ab-loop: a=… b=…` at INFO.
4. `ab-loop.js` renders whatever that push carried, and only that — including
   the "Loop start (A) at 1:23" and "Looping 1:23–1:45" toasts, which name
   times only mpv knows. An item change reaches the UI the same way: the
   native side clears the points on load, mpv reports the clear, and the band,
   readout and button empty themselves.

### Why mpv stamps the points

mpv's `ab-loop` command sets the next point from the core's own
`get_current_time()` (`third_party/mpv/player/command.c`, `cmd_ab_loop`), and
that is the only value that works.

Every write to `ab-loop-a` / `ab-loop-b` runs `update_ab_loop_clip`
(`third_party/mpv/player/playloop.c`), which arms the loop only while
`pts <= b` — and `handle_loop_file` loops only while that flag is set. The
position the OSD has is a `time-pos` observation, so it lags the core by up to
half a second; a B written from it is already behind the core's pts, the loop
comes up disarmed, and playback runs past B for ever. Measured live: `b=6.083`
written while the real pts was ≈ 6.5, and zero wraps until something seeked
back before B.

Sending a step instead of a time makes `pts == b` at write time by
construction. The trade is that the command is a *cycle*, so it only does the
right thing when mpv is on the step being asked for; `command_for` in
`ab_loop.rs` is that check, and its truth table is the Rust unit tests.

`no-osd` (`third_party/mpv/DOCS/man/input.rst`, "Input Command Prefixes")
keeps mpv's own "A-B loop: …" text off the video — the OSD for this is ours.

A clear writes `"no"` to `ab-loop-a` first and `ab-loop-b` second, so the
observations pass through the intermediate pair `(no, <old b>)` before
`(no, no)`. That is deliberate rather than folded away: with no A the UI draws
nothing at all, so the intermediate pair renders identically to the cleared
one and there is no flash to remove. The other order — `b` first — would flash
the A-only state on the way out.

## Surviving the player page swap

`playbackstart` fires a few milliseconds *before* jellyfin-web swaps in the
`DIV.page.libraryPage` that holds the player, so the first render can mount
into the page that is about to be detached (measured: probe7 in the live A-B
report). `ab-loop.js` therefore never treats a mount as final:

- the three pieces are looked up inside one `.videoOsdBottom` — the last
  *connected* one, since the incoming page is appended before the outgoing one
  is removed — so they can never end up split across two copies of the page;
- a piece counts as mounted only while `isConnected` says it is still in the
  document and it is inside that OSD;
- a `MutationObserver` on `document.body` (childList, subtree), armed only
  while a video player is up and coalesced to one check per frame, re-renders
  as soon as any of the three goes missing, with a 2 s sweep behind it;
- both are torn down on `playbackstop`.

## Files

- `src/web/ab-loop.js`, tests in `ab-loop.test.js` (`just test-js`).
- CSS: the "A-B repeat loop" block in section (h) of
  `src/web/astrofin-theme.css`, next to the chapter markers. The band is
  inset by `--af-abloop-inset` (`0.54em`) to match jf-web's own
  `.sliderMarkerContainer`, so its percentages land on the same track the
  chapter stars do. The band itself is a neutral wash with an accent rule top
  and bottom: an accent wash vanished against the played part of the scrubber,
  which is accent-coloured already. `.toast.af-abloop-toast--lifted` lifts our
  toasts clear of the OSD bar by its measured height; the class is ours alone,
  so playback-source.js's toasts are unaffected.
- `src/playback/src/ab_loop.rs` (the action table, the observation and the
  push), Rust tests in the same file.
- `src/mpv/src/api.rs` — `jfn_mpv_ab_loop_cycle`, `jfn_mpv_clear_ab_loop`.
- IPC name `playerAbLoop`, declared in `src/jfn_cef/src/injection.rs` and
  dispatched in `src/jfn_cef/src/business_web.rs`.
