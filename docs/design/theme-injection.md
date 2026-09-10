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

### Playback verification (real mpv, 2026-09-07)

Everything above was re-checked against **real playback** in the built app on
Windows, against the reference server (jellyfin-web 10.11.11, Dune 2021 —
3840×1608 HDR direct play — and a 4:3 episode), driven over the CEF remote
debugging port at 1280×720 CSS / dpr 3.

Every state below measured identical values:

| Measured | Value |
| --- | --- |
| `html`, `body`, `.backgroundContainer`, `.backdropContainer`, `.videoPlayerContainer` background | `rgba(0, 0, 0, 0)` |
| `#af-space`, `#af-spotlight`, `#af-server-panel`, `#af-hint` | `display: none` |
| `html` classes | `af-video transparentDocument` (and `af-home` is dropped) |
| `.backgroundContainer` classes | `backgroundContainer backgroundContainer-transparent` |

States exercised: **playing with the OSD shown**, **playing with the OSD hidden**
(5 s idle, `.videoOsdBottom-hidden hide`, `opacity: 0`), **paused**, **after a
seek**, **resumed**, **subtitle picker open**, **subtitle picker closed**, and
**after stop**.

`.mpvPoster` behaves exactly as the design assumes — sampled every 300 ms from
the click:

```
    3ms  no .videoPlayerContainer yet
  315ms  poster bg=rgb(0, 0, 0) art display=block opacity=1   html=rgba(0,0,0,0)  html.af-video (no transparentDocument yet)
  620ms  poster bg=rgb(0, 0, 0) art display=block opacity=1   html=rgba(0,0,0,0)  html.af-video.transparentDocument
  930ms  .videoPlayerContainer present, .mpvPoster removed
```

The 315 ms sample is the hazard window the section above describes — it is real,
it lasts roughly 300 ms per start, and `html.af-video` is the only thing holding
the root canvas transparent through it.

After stop, everything is restored: `html` back to `rgb(7, 10, 20)`, classes
back to `af-home af-backdrop`, `#af-space` `block`, `#af-spotlight` `flex`, no
`.videoPlayerContainer`, no stuck `af-video`, and the hover spotlight works
again. (`#af-server-panel` stays `none` at 720 p — that is the documented
`900px` viewport-height cut-off, not a video-mode leftover.)

Only two things paint anything at all during playback besides mpv: the two OSD
bands. Both are sub-1 alpha by construction (see below); an automated sweep of
every visible descendant of `.videoOsdBottom` and `.skinHeader.osdHeader` found
no opaque background other than the cyan progress fill itself.

`.appfooter` is worth knowing about: it survives into playback at
`z-index: 1201` — *above* `.videoPlayerContainer` — and the theme gives it
`--af-surface-raised`. It is harmless only because jf-web leaves it empty and
`0px` tall. If it ever grows content during playback it would paint over mpv.

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
| `.skinHeader` | `999` | jellyfin-web's own value; computes to `1` once `.osdHeader` is added during playback |
| `.videoPlayerContainer` | `1000` | inline style from `mpv-video-player.js` when fullscreen |
| `.videoOsdBottom`, `.skinHeader.osdHeader` | auto | inside `#reactRoot`, painted over `.videoPlayerContainer`; both sub-1 alpha so mpv shows through |
| `.appfooter` | `1201` | above `.videoPlayerContainer`, and opaque — harmless only because jf-web leaves it empty and `0px` tall during playback |

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

**Player OSD** — all read off a live playing session, not off the CSS chunks.

`.videoOsdBottom` (fixed, bottom, the band), `.videoOsdBottom-maincontrols`,
`.videoOsdBottom-hidden` + `.hide` (the hidden state; `opacity: 0`,
`display: none`), `.osdControls` (the bar), `.osdTextContainer`
`.osdMainTextContainer` > `h3.osdTitle`, `.osdMediaStatus`, the
`.flex.flex-direction-row.align-items-center` row holding
`.osdTextContainer.startTimeText.osdPositionText` /
`.sliderContainer.mdl-slider-container` /
`.osdTextContainer.endTimeText.osdDurationText`, then
`.buttons.focuscontainer-x` with `.btnPreviousChapter`, `.btnRewind`,
`.btnPause`, `.btnFastForward`, `.btnNextChapter`, `div.osdTimeText >
span.endsAtText`, `.btnUserRating`, `.btnSubtitles`, `.btnAudio`,
`.volumeButtons` (`.buttonMute` + `.osdVolumeSlider`),
`.btnVideoOsdSettings`, `.btnFullscreen` — all
`button.paper-icon-button-light` with a
`span.xlargePaperIconButton.material-icons` inside.

Three things here were wrong in the pre-playback guesses:

* **`h3.osdTitle` is empty on this path.** 10.11.11 puts the item title in the
  top OSD banner instead, so the bottom bar must not reserve space for it
  (`.osdMainTextContainer` otherwise contributes an 11 px margin around a 0-height
  box). The theme zeroes that margin and re-adds it via `.osdTitle:not(:empty)`.
* **The top OSD banner is `.skinHeader.osdHeader`, not a `.videoOsdTop`.**
  jf-web reuses the app header and adds `osdHeader`
  (`.skinHeader.focuscontainer-x.skinHeader-withBackground.skinHeader-blurred.osdHeader`),
  carrying `.headerBackButton` and `.headerLeft .pageTitle` (the item title).
  Section (h) restyles it after section (d), which is how it outranks the
  `!important` glass background there at equal specificity.
* **`.mdl-slider-background-upper` is not the track.** Measured inline styles
  are `left: 14.012%; width: 0.401142%` — it is the **buffered** span. The real
  track is `.mdl-slider-background-flex`, which jf-web paints
  `rgba(255,255,255,.3)`; `.mdl-slider-background-lower` is the played span.
  Chapter markers are `span.sliderMarker.watched` / `.unwatched`, 2×12 px ticks
  positioned with `left: calc(N% - 1px)` (jf-web paints watched `#00a4dc`,
  unwatched `rgba(255,255,255,.3)`).

Also present: `.sliderMarkerContainer`, `.sliderBubbleTrack`, `.sliderBubble`,
`input.osdPositionSlider.mdl-slider` (whose `color` is jf's `#00a4dc` and drives
the thumb), `.mdl-slider-background-flex-container`,
`.mdl-slider-background-flex-inner`, `.osdPoster` and `.osdMediaInfo` (neither
appears on the desktop path), `.osdTitleSmall`.

Layout order, hide/show and timing are left to jellyfin-web and the native
shims. The theme changes surfaces, colour and the vertical rhythm only:

| Band | jf-web default | Astrofin |
| --- | --- | --- |
| `.videoOsdBottom` | 274 px tall (120 px top padding) with a `rgba(bg,.92)` scrim | 120 px, no scrim — the bar *is* the band |
| `.osdControls` | 126 px | 96 px, floating glass, `--af-radius-panel`, `--af-edge-luminous` (top edge `--af-edge-strong`) |
| `.skinHeader.osdHeader` | 121 px opaque-reading glass slab | 68 px light scrim, no blur, no bottom hairline |

Neither band may be opaque. The bar is
`linear-gradient(180deg, rgba(surface-raised,.40), rgba(bg-base,.48))` over
`backdrop-filter: blur(20px) brightness(.62) saturate(1.05)` — the
`brightness()` is what buys legibility over a bright frame *without* an opaque
fill, so the picture keeps moving through the band. The banner is
`rgba(bg,.80) → .52 @62% → 0`.

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
* **A "kind" badge element** does not exist in 10.11.11's card markup. The badge
  is synthesised: `astrofin-theme.js` copies `.card[data-type]` into
  `data-af-kind` for Movie / Series / Episode only, and the CSS renders it with
  `content: attr(data-af-kind)` on `.cardScalable::before`. The attribute is set
  on **both** the `.card` and its `.cardScalable`: `attr()` resolves against the
  pseudo-element's own originating element, never against an ancestor, so with
  it only on `.card` every declaration in the rule applied but `content`
  resolved to `""` and the badge measured 0×0. That is why it appeared to be
  missing on "some" cards — it was missing on all of them.
* **Action-sheet rows are `.emby-button`.** In 10.11.11 each one is
  `button.listItem.listItem-button.actionSheetMenuItem.emby-button`, which the
  secondary-pill rule in section (f) was styling — the subtitle and audio
  pickers came out as a wall of bordered pills. `:not(.listItem)` was added to
  that rule's exclusion chain. The selected track is marked by *visibility*, not
  a class: unselected rows carry an inline `style="visibility:hidden;"` on
  `span.actionsheetMenuItemIcon`, the selected row's icon has no `style`
  attribute at all.
* **`.emby-button.button-link`** has to be spelled out. A bare `.button-link`
  (0,1,0) ties with jf-web's own `.emby-button` colour rule and lost on source
  order, so the tag list on a details page rendered white instead of accent.
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

## The library grid

Design target: `docs/design/canvas/Main.dc.html`, artboard **3 Library grid
(Movies)** — 72 px gutter, 8 columns, 24 px gap, 14 px tile radius, focus scale
1.06 carrying `--af-focus-ring-tile`, unfocused labels at 72 %, 20/26 titles over
16/22 muted subs, and an A–Z rail of 24 px letters whose selected letter is a
30 px cyan disc with a glow. Section (l) of `astrofin-theme.css` is the whole of
it.

### The gate

`html.af-library`, set in `refresh()` from `isLibraryRoute()` — hash-first
(`/movies`, `/tv`, `/music`, `/list`, each optionally `.html` and then bounded by
`?`, `/` or end of string), with a `.libraryPage:not(.homePage)` DOM probe used
only when there is no hash at all. The `:not()` is not decoration: **jf-web
10.11.11 puts `.libraryPage` on Home too**, so a bare `.libraryPage` probe calls
Home a library.

`af-home` and `af-library` are never both set. `refresh()` resolves Home first
and only tests the library route once Home is ruled out.

The focused-card machinery is shared with Home, with one split:

* `cardFrom()` claims a card inside `#homeTab`, `.homePage` **or**
  `.libraryPage .itemsContainer`; everything else on the page keeps plain
  jellyfin-web behaviour.
* `setFocusedCard()` resolves the route **synchronously**, before the item
  fetch, and drives `setBackdrop()` on both routes but `renderSpotlight()` only
  on Home. `placeSpotlight()` inserts the panel into `#homeTab`'s section
  stream, and a library page has no such stream.
* `leaveHome(keepSelection)` takes an argument now. Moving to a library route
  passes it, which keeps the focused card and the art it is driving while still
  tearing the spotlight down — cards stream into the grid for seconds after the
  first hover and every batch queues a `refresh()`, so without it the backdrop
  would blink off under a stationary cursor. Leaving a library grid for anywhere
  else drops the selection outright: its card belongs to no
  `#homeTab .verticalSection`, so carrying it into Home would paint a stale item
  into the spotlight.

### Two deliberate deviations from the artboard

Both are owner decisions, not oversights.

1. **No separate 48 px page title.** jellyfin-web's own header already carries
   the library name and its tabs, and section (d) styles them. A second title
   says it twice.
2. **No Sort / Filter / Genres text chips.** jellyfin-web's library toolbar is
   icon buttons, so they take the circular glass treatment section (d) gives the
   header buttons rather than being rebuilt as the artboard's pills. The
   sort/filter *state* stays where jellyfin-web puts it, in the action sheets
   those buttons open.

### Selectors depended on (verified 10.11.11)

Read off the pinned bundle in `.cache/e2e/jellyfin-web/`, not from memory.

**Page**
`#moviesPage.page.libraryPage.backdropPage.pageWithAbsoluteTabs` >
`.pageTabContent#moviesTab`. `#tvPage`, `#musicPage` and the generic list view
share the shape. No id carries a stock rule, so there is nothing to out-specify
there.

**Toolbar** — `div.flex.align-items-center.justify-content-center.flex-wrap-wrap.padded-top.padded-left.padded-right.padded-bottom.focuscontainer-x`,
rendered **twice** on a library page: above the grid with `.paging` plus
`.btnPlayAll`, `.btnShuffle`, `.btnSelectView`, `.btnSort` and `.btnFilter`
(inside `.btnFilter-wrapper`), and below it with `.paging` alone. Every one of
those `.btn*` classes and `.paging` itself ships **no stock CSS at all**; the
buttons are `is="paper-icon-button-light"`, which the custom element upgrade
turns into a real `.paper-icon-button-light` class.

**Grid** — `div[is=emby-itemscontainer].itemsContainer.vertical-wrap.padded-left.padded-right.padded-right-withalphapicker`
> `.card.portraitCard` > `.cardBox` > `.cardScalable` > `.cardImageContainer` …
`.cardText`.

**Rail** — `.alphaPicker.alphaPicker-fixed.alphaPicker-vertical`, to which the
view JS adds `alphabetPicker-right` (no stock rule) and `alphaPicker-fixed-right`
(`right: 1em` above 62.5em). The letters are **not** children of `.alphaPicker`:
the component wraps them in one `div.alphaPickerRow.alphaPickerRow-vertical`, so
the vertical distribution has to happen on that row or `space-between` would be
spacing a single child. Buttons are bare
`button.alphaPickerButton.alphaPickerButton-vertical`, and the component adds and
removes `.alphaPickerButton-selected` itself.

### Stock rules that had to be answered

| Stock | Specificity | How |
| --- | --- | --- |
| `.itemsContainer{display:flex;margin:0 auto}` + `.vertical-wrap{flex-wrap:wrap}` | (0,1,0) | replaced with `display: grid` at (0,3,1) |
| `.portraitCard{width:33.3%…10%}`, a nine-step media ladder | (0,1,0) | `width: auto` on `> .card` at (0,4,1); the track owns the width, and matching the card rather than the aspect class covers every layout the view switcher produces |
| `[dir=ltr] .itemsContainer>.card>.cardBox{margin-left:0;margin-right:1.2em}` | (0,4,0) | `margin: 0 !important` at (0,4,1). This, **not** `.cardBox{margin:.6em}`, is the horizontal rule in play — a plain `.cardBox` override cannot reach it |
| `.cardBox-bottompadded{margin-bottom:1.8em!important}` | (0,1,0) **!important** | the same `margin: 0 !important`. Measured live before it was added: 28.8 px under every card, so rows sat 52.8 px apart against 24 px between columns |
| `[dir=ltr] .padded-left` / `.padded-right` / `.padded-right-withalphapicker` (7.5 %, added to the container by the view JS), plus their `@supports` safe-area twins | (0,2,0) | out-specified at (0,3,1); no `!important` needed |
| `.alphaPicker-fixed{bottom:5.5em;position:fixed}`, `[dir=ltr] .alphaPicker-fixed-right{right:1em}`, and a `max-height` font-size ladder down to 74 % | (0,1,0)–(0,2,0) | out-specified at (0,4,1); the letters carry explicit pixel sizes, so the ladder cannot reach them either |
| `@media (max-height:50em){.alphaPickerButton-vertical{padding-block:1px!important}}` | (0,1,0) **!important** | not fought — `box-sizing: border-box` on the letter keeps it 24 px outside regardless |
| `[dir=ltr] .sectionTitleButton{margin-left:1.5em!important}` and `[dir=ltr] .sectionTitleButton+.sectionTitleButton{margin-left:.5em!important}` | (0,2,0) **!important** | `margin: 0 !important`. The generic list view spells its toolbar buttons `.btnSort.sectionTitleButton`, and those margins would otherwise leave a ragged 24 px / 8 px rhythm through the flex gap |
| `.paper-icon-button-light>.material-icons{font-size:1.6695652174em}` | (0,1,1) child combinator | answered with a child combinator of its own |
| `.backgroundContainer.withBackdrop` — this sheet's own scrim, section (b) | (0,2,0) | transparent under `html.af-library.af-backdrop` at (0,4,1) |

Those two card margins are the **only** `!important` declarations section (l)
adds; everything else is plain specificity.

`.alphaPickerButton-selected` and `.paging` have **zero** stock rules anywhere in
the bundle. Section (l) is the entire styling those two ever get.

### The backdrop on a library page

`#af-space`, `#af-backdrop` and the scrim are global and unchanged. Two things
would otherwise double up with them, and a library page is the only place both
appear at once:

* jellyfin-web's own `.backdropContainer` — already faded by
  `html.af-backdrop .backdropContainer { opacity: 0 }` in section (c).
* `.backgroundContainer.withBackdrop` — a library page is `.backdropPage`, so
  jellyfin-web sets `withBackdrop`, and the `rgba(bg,.86)` scrim section (b)
  gives that class is `position: fixed` with `z-index: auto`, i.e. **above**
  `#af-space` at `-3`. Left alone it would all but erase our art. It stands down
  for exactly as long as `html.af-backdrop` is up, and comes straight back when
  it is not, so a library page with no card focused keeps its stock legibility
  scrim.

Neither change touches the video gates: `html.af-video` and
`html.transparentDocument` still hide `#af-space` outright.

### Layout notes

* The grid's right padding is `calc(var(--af-gutter) + 64px)` — 40 px of letter
  rail plus one `--af-rail-gap` beside it.
* `padding-top` is `--af-space-6` (24 px), the artboard's own inset, so the first
  row's 1.06 scale and its 3 px ring are not clipped by the page's top edge.
* Column count is 8, dropping to 6 below 1600 px and 5 below 1280 px. It is ours,
  not jellyfin-web's nine-step ladder.
* The rail's `top` is `calc(var(--af-header-height) + 108px)` = 196 px, matching
  the artboard.
* Unfocused tile labels sit at `.72` here against Home's `.55`. On Home the
  spotlight panel owns the attention and the rails are peripheral; in a library
  grid the tiles are the whole page. `.72` is the artboard's value.
* Stock hides the rail entirely below a 500 px viewport height
  (`@media (max-height:31.25em){.alphaPicker-fixed{display:none!important}}`).
  That is left alone — at that height there is no room for 27 letters.

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

The cheap version: in a browser, insert a `div.videoPlayerContainer` at
`body.firstChild` and add `transparentDocument` to `<html>`, then read the
computed backgrounds.

The real version, and the one the numbers in
[Playback verification](#playback-verification-real-mpv-2026-09-07) come from —
run the built app against the server on a **copy** of the profile so a crash
cannot damage the user's, and drive it over the CEF remote debugging port:

```powershell
robocopy "$env:APPDATA\astrofin"      "$scratch\profile-p" /E /NFL /NDL /NJH /NJS
robocopy "$env:LOCALAPPDATA\astrofin" "$scratch\cache-p"   /E /NFL /NDL /NJH /NJS
build\astrofin.exe --config-dir $scratch\profile-p --cache-dir $scratch\cache-p `
  --remote-debug-port 9223 --log-level debug --log-file $scratch\playback-run.log
```

Then, from Node (26+ has a global `WebSocket`), attach to
`http://127.0.0.1:9223/json`, `Emulation.setDeviceMetricsOverride` to
1280×720 at `deviceScaleFactor: 3`, and:

* start playback by clicking a real card button —
  `.card[data-id][data-type="Movie"] .cardOverlayButton[data-action="resume"]` —
  so jellyfin-web owns the resume offset and media-source choice;
* wake and sleep the OSD with `Input.dispatchMouseEvent` `mouseMoved`
  (5 s of stillness hides it and sets `body.mouseIdle`);
* pause with `.btnPause`, seek by setting `.osdPositionSlider.value` and firing
  `change`, open the pickers with `.btnSubtitles` / `.btnAudio`;
* **dismiss an action sheet by clicking the backdrop**, not with Escape — a
  synthetic Escape does not reach it, and an open sheet then swallows the click
  on `.headerBackButton` that stops playback. Dismissing without choosing makes
  jellyfin-web log its own `Uncaught (in promise) Error: ActionSheet closed
  without resolving`; that is jf-web, not the theme;
* stop with `.headerBackButton`.

`Page.captureScreenshot` only ever returns the **web layer** — mpv is not in the
CEF surface. For a composited picture (video + OSD) capture the window off the
screen instead, from a **DPI-aware** process: a DPI-unaware
`GetWindowRect`/`CopyFromScreen` returns virtualised 1292×732 logical
coordinates and silently grabs the top-left third of the 3876×2196 window.
Call `SetProcessDpiAwarenessContext(-4)` first. The OSD hides after ~3 s, so
pump `mouseMoved` from the CDP side while the capture runs.
