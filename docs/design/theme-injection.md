# Theme injection

How the Astrofin look gets into the remote jellyfin-web UI, and what it depends
on. Companion to `docs/design-brief.md` and `docs/design/Astrofin.dc.html`
(artboard 1c is the Home target).

Verified against **jellyfin-web 10.11.11**. Every selector called out below was
read off a running server, not from memory. When the server is upgraded, re-run
the checks in [Testing](#testing).

## Mechanism

jellyfin-web is served by the Jellyfin server; we do not build or patch it.
Astrofin styles it the same way it already scripts it — by injecting into the
renderer at `OnContextCreated`.

```
injection.rs                  app.rs                          the page
────────────                  ──────                          ────────
WEB_STYLES: &[InjectedStyle]  styles_preamble(profile)  ─┐
  Tokens  astrofin-tokens.css   embedded_css::get(..)    │  <style id="af-theme">
  Fonts   astrofin-fonts.css    concatenate in order     ├─▶   tokens
  Theme   astrofin-theme.css    JSON-escape via          │      fonts
                                jfn_js_json::to_js_json  │      theme
                                                         │
WEB_SCRIPTS: &[InjectedScript] run_user_scripts(..)     ─┘
  ...                            concatenate            ─┐
  AstrofinTheme                  substitute placeholders │  one execute_java_script
    astrofin-theme.js            prepend the preamble    ─┘
```

* **`src/jfn_cef/src/embedded_css.rs`** — `include_str!`s the three sheets at
  compile time. Mirrors `embedded_js.rs`.
* **`src/jfn_cef/src/injection.rs`** — `InjectedStyle` enum + `WEB_STYLES`,
  carried to the renderer process in the `extra_info` dictionary under the
  `styles` key, exactly like `scripts`. Only the `web` profile declares any;
  `overlay` and `about` pass `&[]`.
* **`src/jfn_cef/src/app.rs`** — `styles_preamble()` builds a small JS prelude
  that installs the concatenated CSS as `<style id="af-theme">`, and
  `run_user_scripts()` prepends it to the script bundle. The prepend happens
  *after* placeholder substitution (`__SERVER_URL__` and friends) so stylesheet
  bytes can never swallow a replacement.
* **`src/web/astrofin-theme.js`** — owns the element's position from
  DOMContentLoaded on, plus everything dynamic. `build_for_kind` pushes it
  after `csd.js` and the platform menu scripts so it is genuinely the last
  entry in the bundle; it is also wrapped in a top-level try/catch, because
  the whole bundle is one `execute_java_script` call and a throw at that
  level would abort whatever follows.

The whole thing runs before jellyfin-web's own bundles and before
`DOMContentLoaded`, and again on every navigation that creates a new V8 context.
Both the preamble and the script are idempotent (`window.__afTheme` guard).

### No `app://` assets

The custom scheme is registered `CEF_SCHEME_OPTION_LOCAL` (`app.rs`), so a page
on the server origin cannot load `app://` URLs. **Every asset referenced from
the injected CSS must be inline** — a `data:` URI or plain CSS. The header mark
and the missing-art placeholder are inline SVG data URIs for this reason.

### Cascade ordering

This is the part that bites. jellyfin-web 10.11.11 puts stylesheets in two
places:

1. a `<link>` per lazily loaded webpack chunk, appended to `<head>` throughout
   the session; and
2. **`themes/<name>/theme.css` in a `<div>` inside `<body>`** — which no
   position in `<head>` can ever outrank at equal specificity.

So `astrofin-theme.js`'s `keepThemeLast()` moves `#af-theme` to the end of
`<body>` and puts it back there whenever another *stylesheet node* ends up after
it (only stylesheet nodes count, which keeps it from ping-ponging with the
panels it also appends). It is driven by `MutationObserver`s on `<head>` and
`<body>`, plus jellyfin-web's `document._callbacks.THEME_CHANGE`.

The Rust preamble therefore only *parks* the element once — it deliberately does
not keep re-appending it to `<head>`, or it would fight the script.

Consequence: prefer targeted specificity over `!important`. `!important` is used
only where jellyfin-web itself uses `!important` or an inline style — the
`.skinHeader` background, the default card backgrounds, `.dialog`, and the
video-mode `display: none` gates.

### There are no `--theme-*` variables

jellyfin-web 10.11.11 ships **no CSS custom properties at all**. `themes/dark/
theme.css` hard-codes `#101010`, `#202020`, `#00a4dc` and so on. There is no
variable layer to remap, so section (b) of `astrofin-theme.css` overrides those
colours selector by selector.

## Video-mode gating

mpv renders **under** the web layer and the CEF browser is created with
`background_color: 0` (`client/browser_ops.rs`). If anything paints an opaque
background while video is playing, the video disappears.

Two independent gates, both keyed in CSS:

| Gate | Set by | When |
| --- | --- | --- |
| `html.transparentDocument` | jellyfin-web's `setBackdropTransparency` (`Dashboard.setBackdropTransparency`) | `mpv-video-player.js` calls it on playback start (`setTransparency(2)`) and clears it on stop. Re-checked in the 10.11.11 bundle: **both** the `Full`/`2` and the `Backdrop`/`1` branches add the class; only level `0` removes it |
| `html.af-video` | `astrofin-theme.js`, from a `MutationObserver` on `body` childList | while a `.videoPlayerContainer` exists |

Either one sets `display: none !important` on `#af-space`, `#af-spotlight`,
`#af-server-panel` and `#af-hint`.

Rules that make this safe:

* `html` (and `html.preload`) gets `--af-bg-base` **only** because
  jellyfin-web's own inline `.transparentDocument { background: 0 0 !important }`
  (in `index.html`'s `<head>`) outranks it during playback.
* `body` is never given a background.
* `.backgroundContainer` is forced transparent. `.backgroundContainer.withBackdrop`
  keeps a scrim, in `rgba(var(--af-bg-base-rgb), .86)` instead of jellyfin's
  black, so details and live-TV pages stay legible over item art. That rule is
  (0,2,0) and would outrank jellyfin-web's own
  `.backgroundContainer-transparent { background-color: transparent }` (0,1,0),
  so it is **not** safe to rely on `.backgroundContainer-transparent` alone:
  on the fullscreen path there is a window between `.videoPlayerContainer`
  being inserted and `setTransparency(2)` landing where `withBackdrop` may
  still be set. Both video gates therefore force
  `.backgroundContainer` **and** `.backgroundContainer.withBackdrop` to
  `transparent !important`.
* `.backdropContainer` is only faded (`opacity: 0` under `html.af-backdrop`),
  never hidden, so removing the class restores it.
* `.mpvPoster` is never touched and stays opaque `#000` from its inline style.

Verified live: with a simulated `.videoPlayerContainer` + `transparentDocument`,
computed `html` and `body` background are `rgba(0,0,0,0)`, all four Astrofin
layers are `display: none`, and `.mpvPoster` is `rgb(0,0,0)`.

## Z-index map

| Layer | z-index | Notes |
| --- | --- | --- |
| html canvas | — | `--af-bg-base`, lifted by `.transparentDocument` |
| `#af-space` (stars, nebula, `#af-backdrop`) | `-3` | `position: fixed`, `pointer-events: none`, `contain: strict` |
| `.backdropContainer` (jellyfin-web) | `-1` | its own value; faded under `html.af-backdrop` |
| `.backgroundContainer` (jellyfin-web) | auto | forced transparent |
| page content, `.mainAnimatedPage` | 0 | |
| `#af-server-panel`, `#af-hint` | `900` | fixed, `pointer-events: none` |
| `#af-spotlight` | — | in flow inside `#homeTab .homeSectionsContainer`, not positioned |
| `.skinHeader` | `999` | jellyfin-web's own value |
| `.videoPlayerContainer` | `1000` | inline style from `mpv-video-player.js` when fullscreen |

## jellyfin-web selectors depended on

All confirmed present in 10.11.11 on a live Home page. Anything marked
`/* jf-web 10.11.11 */` in the CSS is on this list.

**Document / layout**
`html.preload`, `html.transparentDocument`, `.backgroundContainer`,
`.backgroundContainer.withBackdrop`, `.backgroundContainer-transparent`,
`.backdropContainer`, `.mainAnimatedPages`, `.mainAnimatedPage`,
`meta[name="theme-color"]` (id `themeColor`, initial content `#202020`).

**Header**
`.skinHeader`, `.skinHeader-withBackground`, `.skinHeader.semiTransparent`,
`.skinHeader > .header`, `.headerLeft`, `.headerRight`, `.headerButton`,
`h3.pageTitle.pageTitleWithLogo.pageTitleWithDefaultLogo` (the Jellyfin banner is
a `background-image` on this element — replaced with the inline Astrofin mark and
an `::after` wordmark), `.headerTabs`, `.emby-tab-button`,
`.emby-tab-button-active`.

**Home**
`#homeTab`, `.homeSectionsContainer`, `.verticalSection.section0…section13`,
`.sectionTitleContainer`, `.sectionTitle`, `.itemsContainer.scrollSlider`,
`.emby-scroller`, `.emby-scrollbuttons`.

**Cards**
`.card[data-id][data-type][data-serverid]`, `.cardBox`, `.cardScalable`,
`.cardPadder`, `a.cardImageContainer` (**no `href`, therefore not focusable** —
focus lands on the `.cardOverlayButton`s, so the CSS uses `:focus-within` on
`.card` and the JS uses `focusin` + `closest('.card[data-id]')`),
`.cardOverlayContainer`,
`button.cardOverlayButton[data-action="resume"|"play"]` /
`.cardOverlayButton.cardOverlayFab-primary` (the spotlight's Play/Resume button
clicks this so jellyfin-web owns resume offsets and media-source selection),
`.cardText`, `.cardText-first`, `.cardText-secondary`, `.cardIndicators`,
`.innerCardFooter`, `.itemProgressBar`, `.itemProgressBarForeground`,
`.defaultCardBackground1…5`.

### Card focus ring: what clips it

Four jf-web rules decide whether the ring survives, and all four had to be
answered. Measured in the running app at 1920×1080.

* `.card:not(.show-animation) { contain: layout style paint }` — **paint
  containment clips the ring**. Combined with
  `[dir=ltr] .itemsContainer > .card > .cardBox { margin-left: 0; margin-right: 1.2em }`,
  the tile is flush with the card's left edge (card and `.cardScalable` both at
  x=63) with 19px of slack only on the right, so the scaled tile, its 3px ring
  and its 44px glow are cut on the **left** first. The theme restates the same
  selector with `contain: layout style`, dropping paint only.
* `contain: layout` makes `.card` a stacking context, so a `z-index` on
  `.cardScalable` can never beat the next sibling card. The focused card itself
  takes `position: relative; z-index: 3`.
* `.cardBox:not(.visualCardBox) .cardPadder { border-radius: .2em }` (0,3,0) and
  `.card.show-focus:not(.show-animation) … .cardScalable { border-radius: .7em;
  border: .5em solid transparent }` (0,4,0) both outrank a plain `.card X`
  (0,2,0), which is why the tile radius is `!important` — verified by removing
  it and watching `.cardPadder` come back at 3.2px. The `.show-focus` border is
  separately zeroed, or the tile would shrink 8px a side the moment keyboard
  focus mode engages.
* **Nothing above the card clips.** `.itemsContainer`, `.emby-scroller`,
  `.verticalSection`, `.homeSectionsContainer` and `#homeTab` are all
  `overflow: visible` with `contain: none`; on a library page `#moviesPage` is
  `contain: size style` (no paint) and only `body` clips, at the viewport. So no
  scroller padding or negative-margin trick is needed — jf-web's own
  `.padded-top-focusscale` / `.padded-bottom-focusscale` on `.emby-scroller`
  only do anything under `.layout-tv`.

Verified on Home backdrop rails (`overflowBackdropCard`) and on a library grid
(`.itemsContainer.vertical-wrap`, `portraitCard`), where the scaled tile
measures 5px outside the card box on the left — the overhang that used to be
clipped away.

**Buttons / inputs**
`.emby-button`, `.raised`, `.button-submit`, `.button-delete`, `.fab`,
`.paper-icon-button-light`, `.emby-button.show-focus:focus`, `.button-link` and
`.button-flat` (both deliberately chrome-less in 10.11.11 — excluded from the
secondary pill, or the tag list on a details page turns into a wall of pills),
`.emby-input`, `.emby-textarea`, `.emby-select-withcolor`, `.emby-checkbox` +
`.checkboxOutline`, `.mediaInfoText`.

**Player OSD**
`.videoOsdBottom`, `.videoOsdBottom-hidden`, `.osdControls`, `.osdTimeText`,
`.osdTitle`, `.osdTitleSmall`, `.osdMediaInfo`, `.mdl-slider`,
`.mdl-slider-background-lower` (progress), `.mdl-slider-background-upper`
(track), `.sliderBubble`. Layout, hide/show and timing are left to jellyfin-web
and the native shims; only colours and surfaces change.

**Dialogs / chrome**
`.dialog`, `.actionSheet`, `.actionSheetContent`, `.actionSheetMenuItem`,
`.focuscontainer`, `.formDialogHeader`, `.formDialogFooter`, `.mainDrawer`,
`.navMenuOption`, `.navMenuOption-selected`, `.listItem`, `.infoBanner`,
`.toast`, `.appfooter`.

**JS APIs**
`window.ApiClient.getItem(userId, id)`, `.getCurrentUserId()`,
`.getScaledImageUrl(id, {type, maxWidth, tag})`, `.serverId()`, `.serverName()`,
`.serverInfo()`; `document._callbacks.HISTORY_UPDATE` and
`document._callbacks.THEME_CHANGE` (jellyfin-web's internal `Events.trigger()`
bus — plain arrays on the object, not DOM events, same hook `native-shim.js`
uses); `window.jmpInfo.settings.playback.hwdec` and
`window.jmpInfo.settings.transcode.forceTranscoding` from `native-shim.js`.

### Not confirmed / deliberately skipped

* **`SHOW_VIDEO_OSD`** — `document._callbacks` only listed `HISTORY_UPDATE`,
  `THEME_CHANGE` and `HEADER_RENDERED` on a Home page. `SHOW_VIDEO_OSD` presumably
  appears once the video OSD mounts; `native-shim.js` already creates the array
  itself, and the theme does not use it.
* **The OSD selectors** were read from the CSS chunks, not from a live playing
  session — playback was never started against the user's server.
* **A "kind" badge element** does not exist in 10.11.11's card markup. The badge
  is synthesised: `astrofin-theme.js` copies `.card[data-type]` into
  `data-af-kind` for Movie / Series / Episode only, and the CSS renders it with
  `content: attr(data-af-kind)`.
* **`.homePage`** was not observed; `#homeTab` is the real hook. `.homePage` is
  kept in the route fallback selector as a cheap safety net.
* **Route detection** uses the hash as authoritative whenever there is one,
  because 10.11.11 leaves the previous view (and therefore `#homeTab`) in the DOM
  after a route change — a DOM probe alone stays true forever once Home has
  rendered.

## The Home spotlight

On Home only (`html.af-home`):

* `focusin` (capture) and a 120 ms-debounced `mouseover` pick the active
  `.card[data-id]` inside `#homeTab`; it gets `.af-focused`.
* The item is fetched once (cache capped at 64 entries, oldest dropped), then:
  * `#af-backdrop`'s two layers crossfade over `--af-dur-backdrop` with the
    `--af-backdrop-scale-from` → 1 scale and a `--af-backdrop-hold` delay. Art
    preference: `BackdropImageTags` → `ParentBackdropItemId` +
    `ParentBackdropImageTags` → `ImageTags.Primary`. The image is preloaded
    before the swap; a load error clears the backdrop rather than flashing.
    The art is desaturated and dimmed (`saturate(.32) brightness(.5)`, layer
    opacity `.5`) and the scrim adds a base wash plus accent blooms — at full
    strength jellyfin-web's high-chroma poster art blurs into large yellow and
    green blobs that belong to no part of the Astrofin palette.
  * `#af-spotlight` renders title (series name for episodes), chips, a 2-line
    overview and the actions.
* **The spotlight is an in-flow block, not a fixed overlay.** jellyfin-web
  stacks several rails where the design has one, so a viewport-fixed panel
  always covers a rail. `placeSpotlight()` inserts it into
  `#homeTab .homeSectionsContainer` immediately after the `.verticalSection`
  holding the focused card (defaulting to the first section that is not `.hide`
  and has cards). Two things make that stable:
  * a `min-height` of `clamp(220px, 33vh, 360px)` with the content clamped
    (1-line title, one row of chips, 2-line overview) so the band is the same
    height for every item and moving it does not change the page height; and
  * a pointer guard — relocating reflows the rails under a stationary cursor,
    which fires a fresh `mouseover` on whatever slides underneath, so hover is
    ignored until a real `mousemove` arrives. Without it one hover cascades
    down the page.
* Item changes fade `.af-sp-body` on opacity only, over `--af-dur-tile` read
  from the token (so `prefers-reduced-motion` applies). `writeSpotlight()`
  clears the fade class itself, not only the timer that queued it.
* **Actions target the item the panel is showing** (`shownCard`/`shownItem`),
  never the synchronously-set `focusedCard`: the panel only repaints once
  `fetchItem()` resolves, so on a slow server the two differ and Play would
  otherwise start the wrong item.
  * Folder-like types (`CollectionFolder`, `UserView`, `Folder`, `BoxSet`,
    `Season`, `Playlist`) get a single **Browse** action that clicks the card's
    own `[data-action="link"]` element, a `ChildCount` chip if present, no
    overview, no Details, and none of the NEW / remaining-time chips — a
    library is browsed, not played.
  * Playable types get **Resume**/**Play**, which clicks the card's own
    `.cardOverlayButton[data-action="resume"|"play"]` / `.cardOverlayFab-primary`
    so jellyfin-web owns resume offsets and media-source selection. Measured on
    the reference server: 21 of 21 playable Home cards carry one. The fallback
    for a rail that does not is a throwaway `.itemAction[data-action=…]`
    appended to the card and clicked, borrowing the same delegation contract;
    it is unexercised there.
* The panel is kept out of controller and keyboard navigation: jf-web 10.11.11's
  `focusManager` builds its focusable set from
  `INPUT/TEXTAREA/SELECT/BUTTON/A` + `:not([tabindex="-1"]):not(:disabled)`
  plus `.focusable`, and `autoFocus()` additionally skips `.noautofocus`. The
  two buttons carry both `tabindex="-1"` and `noautofocus`; verified live that
  0 of 2 are visible to that selector. They stay fully usable with the mouse.
* `#af-server-panel` and `#af-hint` stay fixed (bottom-right and bottom) and are
  `pointer-events: none` so they can never swallow a click meant for a card.
  The server panel shows `ApiClient.serverName()` plus Mode/Decode rows sourced
  from `window.jmpInfo`; rows that cannot be sourced honestly are omitted (in a
  plain browser, where `jmpInfo` does not exist, only the name shows). It is
  dropped below `900px` viewport height so it never overlaps rail cards at 720p;
  `#af-hint` is dropped below `560px`.
* Panels hide when the route leaves Home or in video mode. There is no scroll
  rule: in flow, the spotlight covers nothing.
* The `.mainAnimatedPages` subtree observer **ignores mutations originating
  inside `#af-spotlight`**. The panel now lives in that subtree, so without the
  filter its own repaint schedules a refresh, which repaints, which schedules a
  refresh — an unbounded loop that also left the fade class permanently on.
  `refresh()` never starts a fade for the same reason; only a genuine item
  change animates.

Everything is wrapped so it cannot throw, uses passive listeners where the event
allows, never calls `preventDefault`, and never moves focus.

## Testing

### Static

```
pwsh -ExecutionPolicy Bypass -Command ". .\dev\windows\env.ps1; cargo fmt --manifest-path src/Cargo.toml --all; cargo clippy --manifest-path src/Cargo.toml --workspace --all-targets -- -D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic"
node --check src/web/astrofin-theme.js
```

### Against a live jellyfin-web, without building the app

Serve `src/web` with permissive CORS and paste the same style element and script
the Rust side would inject:

```bash
python - <<'PY' &
import http.server, functools
class H(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Cache-Control', 'no-store')
        super().end_headers()
http.server.ThreadingHTTPServer(('127.0.0.1', 8765),
    functools.partial(H, directory='src/web')).serve_forever()
PY
```

then, in the browser console on `http://<server>:8096/web/#/home`:

```js
const base = 'http://127.0.0.1:8765/';
const css = (await Promise.all(
  ['astrofin-tokens.css', 'astrofin-fonts.css', 'astrofin-theme.css']
    .map(f => fetch(base + f + '?t=' + Date.now()).then(r => r.text()))
)).join('\n');
const el = document.createElement('style');
el.id = 'af-theme';
el.textContent = css;
document.body.appendChild(el);
window.__afInstallTheme = () => el;
(0, eval)(await fetch(base + 'astrofin-theme.js?t=' + Date.now()).then(r => r.text()));
```

Hover a Home tile and the spotlight plus backdrop should come up.

### Re-checking selectors after a server upgrade

`http://<server>:8096/web/index.html` lists the entry bundles; the webpack
runtime's `miniCssF` map yields every lazily loaded CSS chunk. Fetch them all and
grep, or just inspect the live DOM. The things worth re-confirming are: the
document transparency class (`transparentDocument`), where `themes/<name>/
theme.css` is inserted, the card structure and `data-action` values, and the
`.mdl-slider-background-lower|upper` names.

### Video safety check

With the app running, start playback and confirm `html`/`body` computed
background are transparent, `#af-space` is `display: none`, and the video is
visible. The same can be simulated in a browser by inserting a
`div.videoPlayerContainer` at `body.firstChild` and adding `transparentDocument`
to `<html>`.
