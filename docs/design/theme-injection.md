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
* **`src/web/astrofin-theme.js`** — runs last in `WEB_SCRIPTS` and owns the
  element's position from then on, plus everything dynamic.

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
| `html.transparentDocument` | jellyfin-web's `setBackdropTransparency(1\|2)` (`Dashboard.setBackdropTransparency`) | `mpv-video-player.js` calls it on playback start (`setTransparency(2)`) and clears it on stop |
| `html.af-video` | `astrofin-theme.js`, from a `MutationObserver` on `body` childList | while a `.videoPlayerContainer` exists |

Either one sets `display: none !important` on `#af-space`, `#af-spotlight`,
`#af-server-panel` and `#af-hint`.

Rules that make this safe:

* `html` (and `html.preload`) gets `--af-bg-base` **only** because
  jellyfin-web's own inline `.transparentDocument { background: 0 0 !important }`
  (in `index.html`'s `<head>`) outranks it during playback.
* `body` is never given a background.
* `.backgroundContainer` is forced transparent; `.backgroundContainer.withBackdrop`
  keeps a scrim, but in `rgba(var(--af-bg-base-rgb), .86)` instead of jellyfin's
  black — it is already neutralised during playback by
  `.backgroundContainer-transparent`.
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
| `#af-spotlight`, `#af-server-panel`, `#af-hint` | `900` | `#af-spotlight::before` is the scrim at `-1` inside that context |
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
* The item is fetched once and cached, then:
  * `#af-backdrop`'s two layers crossfade over `--af-dur-backdrop` with the
    `--af-backdrop-scale-from` → 1 scale and a `--af-backdrop-hold` delay. Art
    preference: `BackdropImageTags` → `ParentBackdropItemId` +
    `ParentBackdropImageTags` → `ImageTags.Primary`. The image is preloaded
    before the swap; a load error clears the backdrop rather than flashing.
  * `#af-spotlight` renders title (series name for episodes), chips (episode
    name, `S… · E…`, year, runtime, resolution, HDR, `★ rating`,
    `left N m` / `New`), a 3-line overview and Play/Resume + Details.
* `#af-server-panel` shows `ApiClient.serverName()` plus Mode/Decode rows sourced
  from `window.jmpInfo`; rows that cannot be sourced honestly are omitted (in a
  plain browser, where `jmpInfo` does not exist, only the name shows).
* Panels hide when the user scrolls more than 35 % of a viewport down, when the
  route leaves Home, or in video mode.
* Below `560px` viewport height the spotlight and hint are dropped entirely;
  below `900px` the overview and the server panel are dropped and the title steps
  down to `--af-type-title`. `#af-spotlight::before` is a viewport-anchored
  radial scrim so the panel stays legible where it overlaps the lower rails —
  which it does by design, the way the PS5 layout does.

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
