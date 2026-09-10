# Handoff 2026-09-10 (MacBook -> Windows, evening session)

Written on the M3 Max MacBook at the end of the 2026-09-10 session. Memory
files do not carry between machines, so this file plus `CLAUDE.md` is the
briefing. Read it top to bottom, then start at §1. The previous handoff
(`docs/handoff-2026-09-10-macos.md`) is a done-note now.

## Where things stand

- `main` = e830ef1 plus this docs commit, pushed to both remotes. Tree clean,
  no worktrees, no stashes. GitHub CI on 6bb0538 was green on all seven
  workflows (Flatpak included, for the first time ever); the run on e830ef1
  (item detail restyle, web + docs only) was in progress when this was
  written: check `gh run list -R TheHalfrican/Astrofin -L 8` first.
- Landed today, in order: CI test fixes (05486fa), pinned frontend bugs
  (1120488, aca6cd9), Flatpak icon fix + A-B band reset (1ac5589),
  double-click on every platform (adf6a92), x11 §6 fixes (9a356fa), the
  test-plan §6 security pass with owner-approved defaults (bdf1ac1 web/CEF,
  632e47f ipc/paths/settings), library grid restyle (501ba7d), design canvas
  working files (6bb0538, 063a50a), item detail restyle (e830ef1). CHANGELOG
  `[Unreleased]` records all of it.
- The UI redesign moved from "connect, Home, OSD" to also "library grid,
  movie/series/season/episode detail", all verified live on the Mac against
  the user's server. Design canvas: "Astrofin UI" (the URL is in the Mac
  session's memory; the working files are `docs/design/canvas/`, re-seed from
  there with the design skill; owner decisions on the canvas notes: no poster
  on detail pages, backdrop + logo with the title as fallback).

## 1. Verify today's cross-platform changes on Windows (first, ~30 min)

Everything below compiled and passed on the Windows x64 and arm64 runners,
but only the Mac saw it live. Build (`pwsh -File dev\windows\build.ps1`), run
with debug logging, and check:

1. **Double-click** (adf6a92, `src/input/src/click_count.rs`): Windows has no
   native count, so the shared counter derives it (under 500 ms, within 4 px,
   same button, cap 3). Double-click the player: fullscreen must toggle.
   Double-click a card title: nothing should misfire. Triple-click in a text
   field should select the line (count 3).
2. **Second-instance IPC** (632e47f, `src/instance_ipc/src/policy.rs`,
   `src/paths/src/user_win.rs`): the pipe name now carries the user SID.
   Launch the app, launch it again with a URL argument: the second instance
   must hand off and exit as before. Quit and relaunch immediately: must not
   say "already running" (stale-rebind classification, 5 x 50 ms).
3. **Legacy import** (632e47f, `src/paths/src/migrate.rs`): with a fresh
   profile dir and an old `jellium-desktop` profile present, import should
   still work; symlinks in the source are skipped with a warn; a >2 GiB source
   is skipped with a warn (only if easy to fake).
4. **Settings caps** (632e47f, `src/config`): `windowScale` outside 0.5..=4.0
   is clamped with a warn; a settings.json over 1 MiB is refused whole.
5. **Library grid and detail pages at 300 % scaling** (501ba7d, e830ef1):
   both were tuned at 1708-1920 CSS px. On the 4K TV at 300 % the CSS width is
   1280: expect 5 grid columns and the single-column detail layout (<1600px).
   Eyeball both, screenshot anything off, and note it in
   `docs/design/theme-injection.md` under the relevant subsection. The
   `--remote-debug-port 9223` CDP recipe in that doc's Testing section works
   on Windows too.
6. **E2E**: `just e2e` (manual Gitea workflow, or locally; takes the desktop
   ~20 s, muted). The web layer changed a lot today; the suite should still be
   green (it pins jellyfin-web 10.11.11 and drives Home only).

Anything broken on Windows only: fix on `main` with a test, the usual
`just fmt` + `just lint` (dot-source `dev\windows\env.ps1` first), push, check
CI.

## 2. Next design screen: Astrofin Settings (brief screen 7)

Order agreed with the owner: Settings (ours) -> admin dashboard light pass ->
component sheet + logo. The survey of the current Settings page is in §5
below; the artboard has not been drafted yet. Recipe that worked today for
each screen: (1) draft the artboard in `docs/design/canvas/<Name>.dc.html`
in the Home/Library vocabulary (copy the header + tile markup from
`Main.dc.html`; tokens from `src/web/astrofin-tokens.css`), add it to
`canvas.json` on page-1, re-seed and republish the canvas, get the owner's
OK; (2) survey the real selectors (for Settings they are ours, in
`src/web/client-settings.js`); (3) implement as a gated section in
`astrofin-theme.css`/`.js` with tests, a `theme-injection.md` subsection, a
build and a live CDP check with screenshots; (4) commit with the changelog.

## 3. Open todos (unchanged unless noted)

- `OnBeforeBrowse` navigation pin: decided, written up in
  `docs/test-plan.md` §6 (same-origin or `app://` allow; server-initiated
  redirects allow; other cross-origin -> `Platform::open_external_url`). A
  pure three-case decision function with tests; the hook stays thin.
- Chapter markers not yet eyeballed on a chaptered title (fix 21209ff).
- `jmpInfo.videoMode` stale after a script-driven mode switch.
- Design-brief screens 7-9 (Settings, component sheet, logo board) and the
  admin dashboard light pass.
- Secret split across two log records: accepted as a known limit (documented).

## 4. Gotchas learned today

- Worktree-isolated agents must NOT share `build/cargo-target`: it poisons
  the rlibs (a false "unresolved import" and a suspect green). Give each its
  own `CARGO_TARGET_DIR`; after merging worktree patches, rerun verification
  in the main tree.
- Each push cancels the in-progress GitHub runs on `main` (concurrency group
  per workflow + ref); batch pushes while a verdict is pending.
  `gh run list -c <short-sha>` returns nothing; filter on `headSha` from
  `--json`.
- jellyfin-web's `themes/*/theme.css` paints `.detailPagePrimaryContainer`
  and `.detailPageSecondaryContainer` `#101010`; Home's art treatment passes
  ~4 % of source luminance and reads as "no backdrop" on a mostly empty
  page. Both handled under `html.af-detail`; see theme-injection.md.
- gdk-pixbuf sniffs an SVG's format from its first 256 bytes: keep any XML
  comment INSIDE `<svg>` (that was the Flatpak appstream failure).
- freedesktop.org serves the uchardet tarball again; the Flatpak job is
  green and stays in the required set.
