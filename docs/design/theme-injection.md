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
`.skinHeader > .headerTop`, `.headerLeft`, `.headerRight`, `.headerButton`,
`h3.pageTitle.pageTitleWithLogo.pageTitleWithDefaultLogo` (the Jellyfin banner is
a `background-image` on this element — replaced with the inline Astrofin mark and
an `::after` wordmark), `.headerTabs`, `.emby-tab-button`,
`.emby-tab-button-active`.

**Verified on Windows at 300% (2026-09-10).** Two section-(d) rules were dead
until this pass. `.skinHeader > .header` **does not exist in 10.11.11** — the
live child is `div.flex.align-items-center.flex-grow.headerTop` — so the
`min-height: var(--af-header-height)` / gutter `clamp()` never matched: the
header measured 58.3 px against the 88 px the A-Z rail and the grid are
positioned from, and the back button sat at x=4.6 against a 72 px gutter. Fixed
to `.skinHeader > .headerTop` (the same name jf-web uses for the OSD banner,
`.skinHeader.osdHeader .headerTop`, and present in the pinned e2e bundle). And
the header never blurred: stock's
`.skinHeader.semiTransparent{backdrop-filter:none!important}` beat the plain
declaration, so scrolled detail content read straight through the home/menu
icons while the glass gradient still painted. Both `backdrop-filter`s are now
`!important`; the `.osdHeader` block restates `backdrop-filter: none !important`
so the video banner still carries no blur.

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
* Column count is 8, dropping to 6 at 1281–1599 px and 5 at 1280 px and below
  (`@media (max-width: 1280px)`). It is ours, not jellyfin-web's nine-step
  ladder. The 5-column boundary is `1280px`, not `1279px`, so the 4K-at-300%
  width lands in the 5-column band — see the verified note below.
* The rail's `top` is `calc(var(--af-header-height) + 108px)` = 196 px, matching
  the artboard.
* Unfocused tile labels sit at `.72` here against Home's `.55`. On Home the
  spotlight panel owns the attention and the rails are peripheral; in a library
  grid the tiles are the whole page. `.72` is the artboard's value.
* Stock hides the rail entirely below a 500 px viewport height
  (`@media (max-height:31.25em){.alphaPicker-fixed{display:none!important}}`).
  That is left alone — at that height there is no room for 27 letters.

### Verified on Windows at 300% (2026-09-10)

`window.innerWidth` 1280, `devicePixelRatio` 3, `innerHeight` **698** — a
maximised window on a 2160 px monitor keeps the ~22 px chrome inset, so the CSS
viewport is 698 px tall, not 720.

Before this pass the grid rendered **6 columns** at 1280: the ladder was
`max-width:1599px` → 6 and `max-width:1279px` → 5, and 1280 was one pixel above
the second. The 5-column boundary moved to `max-width:1280px`, so 1280 now lands
in the 5-column band (`scrollWidth` = 1280, no horizontal overflow; the TV grid
`#/tv` is identical). The 1599/1920 bands are unchanged.

**The A-Z rail overflowed the viewport below 916 px of height.** The rail box is
`top: calc(var(--af-header-height) + 108px)` = 196 px and `bottom: 72px`, so at
698 px it is 430 px tall; its single `.alphaPickerRow-vertical` has a content
min-height of 27 × 24 px = **648 px**, so `space-between` never compressed it and
**T, U, V, W, X, Y, Z sat 146 px below the fold**. Stock's own guard only hides
the rail below 31.25 em (500 px), which this viewport clears. Fixed with an
`@media (max-height: 916px)` — the overflow onset, where 648 = viewport − 196 −
72 — that drops the letters to 15 px (selected 22 px) and the bottom inset to
`--af-space-4` (16 px). At 698 px the box is 486 px against a 412 px content min,
74 px of slack, so all 27 are reachable. The 1064/1080 desktop tuning heights
never reach the query, so the tuned rail is untouched.

**Owner revision (live review, 2026-09-10) — empty tabs and the toolbar.** An
empty tab (Collections with no sets, an empty Favorites) drops a
`.noItemsMessage` straight into the item grid, where it landed in one ~195 px
track and wrapped to one or two words a line; it is a direct grid child, so
`grid-column: 1 / -1` (plus `max-width: 640px`) spans it across every column as
a real empty state. Separately, the view/sort/add toolbar (`.btnSelectView`,
`.btnSort`, `.btnNewCollection`, 56 px discs) was clipped under the section
tabs: once the header took its intended 88 px, the fixed `.skinHeader` + the
70 px `.headerTabs` reached ~159 px, but `.libraryPage.pageWithAbsoluteTabs`
still reserved 120 px. `padding-top: calc(var(--af-header-height) + 84px)
!important` (≈172 px, (0,4,1) over stock's `.libraryPage{padding-top:7em
!important}`) clears the full header plus tabs with a ~29 px gap.

## Item detail

Design targets: `docs/design/canvas/MovieDetail.dc.html` and
`docs/design/canvas/SeriesDetail.dc.html` — a 640 px blurb column at the 72 px
gutter starting 150 px down, an 88 px gap, then the shelves; no poster; the
item's logo (or a 64/72 display title) top left under a cyan eyebrow; the
actions as a 360 px stack of 56 px pills with Resume in accent; a 300 px glass
facts panel top right; and, on a season page, episode rows of 208×117 stills.
Section (o) of `astrofin-theme.css` and section 8 of `astrofin-theme.js` are the
whole of it.

### The skeleton

```
#itemDetailPage.page.libraryPage.itemDetailPage.selfBackdropPage
  #itemBackdrop.itemBackdrop           40vh inline art band — mobile only
  .detailLogo                          absolute; .hide when the item has none
  .detailPageWrapperContainer
    .detailPagePrimaryContainer                        ← left column
      .detailImageContainer.hide-mobile                  the poster
      .detailRibbon.padded-left.padded-right
        .infoWrapper
          .detailImageContainer.hide-desktop.hide-tv     the poster again
          .nameContainer > .parentName, .itemName
          .itemMiscInfo.itemMiscInfo-primary   > .mediaInfoItem…
          .itemMiscInfo.itemMiscInfo-secondary > .mediaInfoItem…
        .mainDetailButtons.focuscontainer-x > .detailButton…
      .detailPagePrimaryContent.padded-right
        .detailSection
          form.trackSelections > .selectContainer > select.emby-select
          .detailSectionContent
            p.itemGenres, h3.tagline, p.overview, .overview-controls,
            #seriesAirTime, .itemTags, .itemExternalLinks
          .itemDetailsGroup > .detailsGroupItem > .label + .content
          .nextUpSection, #listChildrenCollapsible
    .detailPageSecondaryContainer.padded-bottom-page   ← right column
      .detailPageContent
        #childrenCollapsible   present but .hide — see below
        #castCollapsible > #castContent
        #specialsCollapsible, #scenesCollapsible,
        #similarCollapsible > .similarContent
```

Every shelf is a `.verticalSection.detailVerticalSection` with an
`h2.sectionTitle.sectionTitle-cards` and, except the two `…ChildrenCollapsible`
sections, an `emby-scroller` around the `.itemsContainer`. Sections the item has
nothing for keep `.hide`.

**Select the page as `.itemDetailPage:not(.hide)`, never `#itemDetailPage`.**
jf-web 10.11.11 leaves the outgoing view in the DOM, duplicate id and all.

**A series' seasons and a season's episodes render into
`#listChildrenCollapsible`, which is in `.detailPagePrimaryContent` — the *left*
column** — while `#childrenCollapsible` in the right column stays `.hide` and
empty. Measured live on a series page: left `#listChildrenCollapsible`
"Seasons", 8 cards; right `#childrenCollapsible` hidden, 0 cards. Every shelf
rule in section (o) therefore names both containers.

**The page's origin is 22 px below the viewport top**, not 0: `.mainAnimatedPage`
is `position:absolute;top:0` inside a container that jf-web insets. The grid's
own `padding-top: 150px` therefore lands the first row at y = 172, which is what
the live readings below show.

### The gate

`html.af-detail`, set in `refresh()` from `isDetailRoute()` — hash-only,
`/^\/details([?/]|$)/`, with no DOM fallback for the same reason. `af-home`,
`af-library` and `af-detail` are never set together: `refresh()` resolves them
in that order and each test only runs once the ones above it are out.

Nothing on the page says whether the item is a movie, a series, a season or an
episode — all four come from one template — so on entering the route the gate
fetches the item once (`ApiClient.getItem(getCurrentUserId(), id)`, id parsed
out of the hash query) and writes `detailTypeFor(item)` to
`html[data-af-detail-type]`. The eyebrow keys off that attribute with four
literal `content:` rules; `attr()` could not do it, because it resolves against
the pseudo-element's own originating element and never against `<html>`. A
season page is a details route of its own, so a hash change with a new id
re-runs the whole thing; leaving the route clears the attribute, removes the
panel and clears the backdrop.

Entering also drops the focused card, once. The card that got us here belongs
to a page on its way out, and carrying it into Home would paint a stale item
into the spotlight. The art it was driving stays up until the fetched item's
own backdrop replaces it — which is why `refresh()` passes
`leaveHome(library || detail)`: a detail page refreshes constantly while it
streams in, and a `clearBackdrop()` on each one would strobe the art.

### The backdrop

Ours, not jellyfin-web's, for consistency with Home and the grids:
`setBackdrop(backdropUrlFor(item))` off the fetched item (Backdrop tag, then
ParentBackdrop, then Primary). jellyfin-web's own two layers stand down —
`.backdropContainer` (which the details controller paints via
`backdrop.setBackdrops([item])`) and `#itemBackdrop` are both `display: none`
under the gate, and `.backgroundContainer.withBackdrop` goes transparent for
exactly as long as `html.af-backdrop` is up, the same trade section (l) makes
on a library page.

#### The art treatment is the detail page's own

Section (c)'s treatment is tuned for **Home**, where the art is a peripheral
wash behind dense rails. Stacked up it passes

```
brightness .5 x layer opacity .5 x (1-.42 horizontal at the right edge)
  x (1-.58 base wash) x (1-.35 vertical minimum)  =  4%
```

of the source luminance. Measured on a running detail page with a **pure white
frame substituted for the art**: the canvas composited to `rgb(11,14,24)`
against a base of `rgb(7,10,20)`. That is the whole reason a detail page read
as having no backdrop — not a missing URL, not a stacking fault. Everything in
the pipeline was correct: `af-backdrop` set, the active layer carrying a real
Backdrop URL at `opacity: .5`, and every ancestor of the page transparent.

On a detail page the backdrop **is** the subject: one item fills the screen and
two thirds of the canvas is empty. So `html.af-detail` retunes it. The values
were not computed, they were tuned against a harness — white frame, page
content set to `visibility: hidden`, bare canvas screenshotted and sampled on a
9x5 grid — and re-measured after every change:

| | Home | detail |
| --- | --- | --- |
| layer `filter` | `saturate(.32) brightness(.5) contrast(.92)` | `saturate(.62) brightness(.78) contrast(.96)` |
| layer `opacity` when on | `.5` | `.84` |
| `--af-glass-blur-backdrop` | 38 px | **6 px** (26 px for a poster, below) |
| `--af-scrim-horizontal` | `.94 → .72@42% → .42` | `.96 → .82@36% → .50` |
| `--af-scrim-vertical` | `.96 → .35@38% → .55` | `.94 → .22@44% → .52` |
| `--af-scrim-wash` | `.58` | `.28` |

The base wash was a hard-coded `rgba(bg,.58)` inside section (c)'s
`.af-backdrop-scrim::after`; it is now
`var(--af-scrim-wash, rgba(var(--af-bg-base-rgb), .58))`, so the fallback keeps
Home byte-identical and only a gate that sets the variable changes anything.

Measured against the Home treatment on the identical white frame:

| point | Home | detail | gain |
| --- | --- | --- | --- |
| blurb column (x=400) | `rgb(9,12,21)` | `rgb(18,21,30)` | **x6.3** |
| mid canvas (x=1100) | `rgb(13,15,26)` | `rgb(34,36,45)` | **x4.6** |
| right edge (x=1636) | `rgb(21,22,38)` | `rgb(51,52,66)` | **x3.0** |

The blurb column is nonetheless **darker** than Home's, because the horizontal
scrim's first two stops are deepened (`.94 → .96`, `.72@42% → .82@36%`) even as
its tail is lifted (`.42 → .50`). The art is bought at the right, where the page
is empty, not underneath the text. On the 9x5 grid, over the whole area text can
occupy (x ≤ the 72 px right gutter), `--af-text-muted` measures between
**4.51:1** and **7.95:1**, worst at (1500, 400) — and pure white is the
adversarial case no real backdrop reaches.

#### A poster is not a backdrop

`backdropUrlFor()` falls back Backdrop tag → `ParentBackdropImageTags` →
`ImageTags.Primary`. `backdropSourceFor()` is a pure helper beside it that names
which branch an item landed on, and the gate writes it to
`html[data-af-backdrop-src]`. On `primary` the sheet blurs the art back towards
a wash (26 px, `brightness(.62)`): `background-size: cover` crops a 2:3 poster
to a 16:9 canvas, which is a blown-up detail of somebody's chin.

### No poster, logo or title

The poster is gone outright (`display: none` on both copies of
`.detailImageContainer` — jf-web renders it twice, `.hide-mobile` beside the
ribbon and `.hide-desktop` inside it): the backdrop is the art on this page.

`.detailLogo` moves from stock's `right:25vw;top:10vh;width:25vw;height:16vh`
to `left: var(--af-gutter); top: 150px; width: 520px; height: 140px`,
`background-position: left center`. jf-web already hides it below 68.75 em; the
sheet carries that up to the 1280 px the two-column layout needs. It stays
`position: absolute`, so when it is present the left column takes a 164 px top
padding to clear it and `.itemName` is hidden — both through
`.detailLogo:not(.hide) ~ .detailPageWrapperContainer …`, which works because
the logo is a previous sibling of the wrapper and gains `.hide` when the item
has neither `ImageTags.Logo` nor a `ParentLogoImageTag`. **The title is hidden
only for `data-af-detail-type` `movie` and `series`.** On a season or an episode
the logo is the *series* logo (jf-web falls back to `ParentLogoImageTag`) while
`.itemName` is "Season 1" or the episode's own name; measured live before this
was scoped, a season page showed nothing but the show's wordmark. With no logo,
`.itemName` renders as the display title: `--af-type-display` (64/72 Sora 200),
wrapping, clamped to two lines, against stock's `font-weight:600;
white-space:nowrap` one-liner.

**Owner revision (live review, 2026-09-10) — the poster comes back.** In the
single-column layout only (`@media (max-width: 1599px)`, the 4K-at-300% case)
the `.hide-mobile` copy is shown again for `movie`, `series` and `season`: the
blurb caps at 640 px on the left and the shelves stack far below, leaving the
top-right of the ribbon empty. It is a standard `portraitCard` whose
`.cardImageContainer` lazy-loads the Primary image; parked
`position: absolute; top: 210px; right: var(--af-gutter); width: 300px` on the
wrapper (made `position: relative`), dropped from the logo line to sit centred
against the action column, its right edge on the content edge, `z-index: 2`,
`pointer-events: none`. The two-column layout (≥1600 px) keeps the no-poster
treatment — the empty space there is the shelf column, a separate placement.
Same review lifted the backdrop art forward (see *The art treatment* above):
blur 6 px → 3 px, the horizontal scrim tail .50 → .40, the art layer to
`saturate(.72) brightness(.86)` at `.92` opacity; the two protected stops over
the blurb (.96 at 0 %, .82 at 36 %) are unchanged.

### The two columns

`.detailPageWrapperContainer` becomes
`grid-template-columns: 640px minmax(0,1fr)` with an 88 px column gap and
`padding: 150px var(--af-gutter) var(--af-gutter)` — 72 + 640 + 88 = 800, which
is where both artboards put the right-hand column. One column below 1600 px.

The artboard reads name → chips → **actions** → blurb → credits, and the
actions go directly after the chips for one reason: **the fold**. Everything
below them is variable-length — a 3-line blurb, a tag wall, a credits block
that runs to four wrapping rows on a film with six directors — so any order
that puts the stack after `.detailPagePrimaryContent` puts it off screen on
exactly the items with the most to say. Measured at 1064 px before the order
was settled: `#btnPlay` at y=**1094** on The Animatrix (17 tags, 6 directors,
4 writers, 6 studios), present and correct and entirely below the fold. After:
y=**452** on that item and on a short one alike, the stack's bottom edge at
712 px, whole stack in view at both 1708x1064 and 1920x1080.

`.detailRibbon` gets **`display: contents`**, which drops its box and promotes
`.infoWrapper` and `.mainDetailButtons` to siblings of
`.detailPagePrimaryContent`. The three then carry explicit orders:

| order | element |
| --- | --- |
| 1 | `.infoWrapper` — eyebrow, `.parentName`, `.itemName`, the chip strip |
| **2** | **`.mainDetailButtons`** — `margin: 22px 0 6px` |
| 3 | `.detailPagePrimaryContent` — track pickers, tagline, blurb, tags, links, credits, and the season/episode shelves |

That matches jellyfin-web's own DOM order on 10.11.11, so the orders change
nothing today. They are there to say the intent out loud: a future template
that moves `.mainDetailButtons` out of the ribbon, or puts the blurb ahead of
it, cannot silently push the stack under the fold again. Because everything
variable-length sits at order 3, the stack's y is a function of the chip strip
alone — 452 px on every item measured, long or short.

The ribbon has nothing of its own left to draw: its 7.2 em height, its −7.2 em
margin and its 32.45 vw left padding all exist to clear the poster this page no
longer has.

An earlier attempt promoted four wrappers and ordered every block explicitly so
the stack could sit between the credits and the tags. It worked, but it made
the stack's y a function of the blurb and credits length — 977 px on a long
item — and it needed an `order: 40` default to stop unlisted children
(`#itemBirthday`, `.recordingFields`, the shelf sections) jumping to the head of
the column, plus a `display: none` on `.detailPagePrimaryContent::after`,
because a boxless parent still generates its pseudo-elements and stock's float
clearfix (`content:""; display:table`) otherwise survived as a flex item at
`order: 0`. Both are recorded here because they are real traps in
`display: contents`, not because the sheet still carries them.

#### Verified on Windows at 300% (2026-09-10)

At 1280 CSS px the wrapper is single column as designed (one 1136 px track).
`#af-detail-panel` takes the stacked treatment and lands at the head of
`.detailPageSecondaryContainer` — two to three screens down; at this width it is
a footer, not a facts panel beside the blurb.

The single-column track exposed four things the 640 px column measure had been
hiding, all fixed in section (o):

* **`.itemTags` had no rule at all**, so the tag wall spanned the full 1136 px
  track under a 600 px `.overview` — ragged (Dune: Part Two, overview 600×94,
  tags 1136×48). `.itemGenres`, `.tagline` and `.itemExternalLinks` were
  uncapped the same way. All four are now `max-width: 600px`, the overview's own
  measure.
* **`.itemName` had no `max-width`**, so a long episode title ran to 1017.9 px on
  one 64 px line and the two-line clamp never engaged. Now capped to the 640 px
  blurb-column measure (as the credits and track pickers already were).
* **The action stack no longer cleared the fold on a series.** A movie stack
  ends at y=690 (fits 698, 8 px spare); a series adds a `btnShuffle` pill and
  ends at **y=756**, so the `btnUserRating` / `btnMoreCommands` disc row was
  below the fold. An `@media (max-height: 900px)` tightens the gap
  (`--af-space-2`), the top margin (22→12 px) and the 56 px pills/discs (→48 px)
  just enough to bring the series stack back to y=690, matching the movie. The
  Play/Resume pill was already well clear of the fold; the 1064/1080 desktop
  tuning heights never reach the query.
* **The 520 px logo does sit over a single-column page.** The single-column
  breakpoint is 1599, not 1279, so every width in 1280–1599 shows the logo over
  one column. It reads fine — the column takes its 164 px top padding and
  `.itemName` is hidden on movie/series — so the fix was the misleading comment
  on the `max-width:1279px` logo rule, not the rule.

Not ours: `document.documentElement.scrollWidth` is 3508 on a movie detail (2008
on a series), because the cast shelf's `.emby-scroller` computes
`overflow-x: visible` and its ~23 person cards push the *document* sideways —
with `html.af-detail` removed the same page measures 4978. Far more visible at
1280 than at 1920.

#### The track pickers

`form.trackSelections` is the source / video / audio / subtitle picker. jf-web
marks a `<select>` **disabled** when the item offers exactly one option, and
forces it chrome-less with
`.emby-select[disabled]{background:none!important;border-color:transparent!important}`
— which is right, a dead control must not look clickable. But the row then
reads "Video   4K HEVC SDR": a label pretending to be a form field, saying
nothing the chip strip and the facts panel have not already said.

So a `.selectContainer` whose select is disabled is dropped
(`:has(.emby-select[disabled])`), and the form is what the design asked for —
**one row of glass selects, each of them a real decision**. Measured on
Bāhubali 2: one picker shown (Subtitles, enabled, `rgba(22,28,51,.55)` at a
999 px radius), form height 44 px against 122 px before. On The Animatrix,
which offers no choice at all, the form collapses to 0 px and the page simply
does not show one. Nothing is lost: codec, resolution and the track list are on
the chips and in `#af-detail-panel`.

`.overview-expand` ("Show more") is excluded from section (f)'s secondary-pill
rule and rendered as an accent text link. As a 56 px pill it outweighed the
blurb it belongs to.

### The action stack

`.detailButton` in 10.11.11 has **no text element**. The template is one
`.detailButton-content` holding a `.material-icons.detailButton-icon` and
nothing else, and the words live in the `title` attribute — there is a
`.detailButton-text` *rule* in the stock sheet, but no such element on this
page. So the label is `content: attr(title)` on a pseudo-element **of the
button itself**, the one element `attr()` can read it from. It is `::before`
with an explicit `order: 2`, not `::after`, because the remaining time has to
come last and a pseudo-element cannot otherwise be placed between
`.detailButton-content` and `::after`.

That remaining time is `content: attr(data-af-left)`, written by
`markResumeButton()` from `UserData.PlaybackPositionTicks` against
`RunTimeTicks` and only when `data-action="resume"`. An **attribute**, not a
child element: attribute writes are invisible to the `childList` observer that
drives `refresh()`, so this cannot feed the repaint loop the spotlight had to
be filtered out of. (`#af-detail-panel` is filtered out of that observer the
same way `#af-spotlight` is.)

`.btnPlay` is a **class**, not an id — the whole button set is classes on
`button.button-flat`.

### The facts panel

`#af-detail-panel` is synthesised: `detailFacts(item, opts)` is a pure
item-JSON-in, rows-out helper and `renderDetailPanel()` inserts the result as
the first child of `.detailPageSecondaryContainer`, where `margin-left: auto`
lands it on the page's right gutter at the artboard's 150 px. Same glass as
`#af-server-panel`, and the same video gate.

| Item | Eyebrow | Rows |
| --- | --- | --- |
| Movie, Episode, anything else | File | Video (codec · resolution · HDR), Audio (codec · layout), Subtitles (language codes, 3 + "+n"), Mode, Size |
| Series | Next up (or Series) | headline `S1 E9 · Title` + remaining time, then Network, Status, Airs, Mode |
| Season | Season | Episodes, Watched *n* of *m*, Mode |

Streams come from `MediaSources[0].MediaStreams`, falling back to
`item.MediaStreams` for the trimmed shapes `/Items` and `/NextUp` return. Next
up is a second, optional pass — `ApiClient.getNextUpEpisodes({SeriesId, UserId,
Limit: 1})` — which re-renders the panel when it lands. Mode is the video mode
from `window.jmpInfo.settings.playback.videoMode` through the same
`videoModeLabel()` the server panel uses.

**Rows with no value are never emitted.** A movie the server has not scanned
yields no rows at all and the panel hides itself rather than printing a stack
of dashes.

### Four deliberate deviations from the artboards

The first three are owner decisions; the fourth came out of the live check.

1. **Seasons stay cards.** SeriesDetail draws them as a row of pills;
   jellyfin-web renders `#childrenCollapsible` as an `.itemsContainer` of
   `.card.overflowPortraitCard`, which already carries the tile rules from
   section (e), and rebuilding them as pills would mean hiding real season art.
2. **`btnMoreCommands`, `btnUserRating`, `btnDownload`, `btnInstantMix` and
   `btnSplitVersions` become 56 px icon discs on a row of their own** under the
   pills. jf-web wraps none of them, so there is no element to make that row
   with: the stack is spelled as a wrapping flex *row* whose pills are
   `flex: 0 0 100%` and whose discs are `flex: 0 0 56px` at `order: 2`, which
   puts them together on the line after the pills whatever order the template
   uses (`btnDownload` is third in the markup).
3. **No separate remaining-time scrubber under the stack.** Both artboards draw
   one; the same number rides the Resume pill instead, and a second copy would
   say it twice.
4. **No per-row overview on a season page** (`.listItem-overview`,
   `display: none`). Both artboards draw an episode row as still +
   "E9 · Title" + "24m · Aired …" and nothing else, and the live check says why:
   jf-web puts the episode list in the 640 px blurb column, where a paragraph
   between a 208 px still and four 40 px buttons measured **215 px wide and
   186 px tall** — six words to a line. jf-web itself hides it below a 50 em
   viewport for the same reason. Deleting the one rule brings it back.

### Selectors depended on (verified 10.11.11)

Read off the pinned bundle in `.cache/e2e/jellyfin-web/`, not from memory.

**Page** — `.itemDetailPage.libraryPage.selfBackdropPage`, `#itemBackdrop`,
`.detailLogo`, `.detailPageWrapperContainer`, `.detailPagePrimaryContainer`,
`.detailImageContainer` (`.hide-mobile` / `.hide-desktop.hide-tv`),
`.detailRibbon`, `.infoWrapper`, `.detailPagePrimaryContent`,
`.detailPageSecondaryContainer.padded-bottom-page`, `.detailPageContent`.

**Head** — `.nameContainer` > `.parentName` + `.itemName` (also
`.itemName.parentNameLast`, `.itemName.originalTitle`),
`.itemMiscInfo.itemMiscInfo-primary` / `-secondary` > `.mediaInfoItem`
(sometimes also `.mediaInfoText.mediaInfoText-upper`), `.starRatingContainer` >
`.starIcon`, `.mediaInfoCriticRating`.

**Actions** — `.mainDetailButtons.focuscontainer-x` >
`button.button-flat.detailButton` with `.detailButton-content >
span.material-icons.detailButton-icon`, classed `btnPlay` (`data-action`
`resume`|`play`), `btnReplay`, `btnDownload`, `btnPlayTrailer`, `btnInstantMix`,
`btnShuffle`, `btnCancelSeriesTimer`, `btnCancelTimer`, `btnPlaystate`,
`btnUserRating`, `btnSplitVersions`, `btnMoreCommands`. Each carries `.hide`
until the item earns it.

**Body** — `form.trackSelections` > `.selectContainer` >
`select.emby-select.detailTrackSelect`, `.detailSectionContent` >
`p.itemGenres`, `h3.tagline`, `p.overview` (+ `.detail-clamp-text`),
`.overview-controls > a.overview-expand`, `.itemTags`, `.itemExternalLinks`;
`.itemDetailsGroup > .detailsGroupItem > .label + .content`.

**Season rows** — `.itemsContainer.vertical-list` >
`.listItem.listItem-largeImage.listItem-withContentWrapper` >
**`.listItem-content`** > `.listItemImage.listItemImage-large` (holding
`.listItemImageButton` and, when the item has them,
`.indicators.listItemIndicators > .playedIndicator` and
`.itemProgressBar.listItemProgressBar > .itemProgressBarForeground`),
`.listItemBody > .listItemBodyText` + `.secondary.listItemMediaInfo` +
`.secondary.listItem-overview`, then `.listViewUserDataButtons`.

**The row is `.listItem-content`, not `.listItem`.** Stock lays
`.listItem-withContentWrapper` out as a *column* and puts the real row inside
it, so styling `.listItem` as the row put the episode title and its runtime
*above* the still — measured live at 264 px per row before the fix, 125 px
after.

**JS** — `ApiClient.getNextUpEpisodes({SeriesId, UserId, Limit})` in addition to
the calls listed above.

### Stock rules that had to be answered

| Stock | Specificity | How |
| --- | --- | --- |
| `.libraryPage{padding-top:7em!important}` — 8575.css exempts every library page from its own 3.25rem *except* this one | (0,1,0) **!important** | `padding-top:0!important` at (0,2,1); the grid sets its own 150px inset |
| `.layout-desktop .detailRibbon{height:7.2em;margin-top:-7.2em}` + `.layout-desktop [dir=ltr] .detailRibbon{padding-left:32.45vw}` | (0,2,0) / **(0,3,0)** | `display:contents` at (0,3,1). The (0,3,0) is why every rule in the section spells `.itemDetailPage` out |
| `[dir=ltr] .detailPagePrimaryContent` / `.detailPageContent` `{padding-left:32.45vw;padding-right:2%}` | (0,2,0) | `padding:0` at (0,3,1) |
| `.layout-desktop .detailPageWrapperContainer{display:flex;flex-wrap:wrap;margin:1em 0}` | (0,2,0) | `display:grid` at (0,3,1) |
| `.infoWrapper{flex:1 0 0}` | (0,1,0) | `flex:none` — a zero basis that grows would make the blurb column one tall empty box |
| `style="margin-bottom:.6em"` inline on both `.itemMiscInfo` rows | inline | `margin-bottom:0!important` — the column gap owns the rhythm |
| `.detailButton{flex-direction:column;margin:0!important;padding:.7em .7em!important}` plus three more `padding-left`/`-right` `!important` rules in media queries | (0,1,0) **!important** | `margin:0!important;padding:0 24px!important` at (0,4,1) |
| `form.trackSelections` renders its **disabled** selects chrome-less: `.emby-select[disabled]{background:none!important;border-color:transparent!important;color:inherit!important}` | (0,2,0) **!important** | **not fought** — the container is dropped instead, with `:has(.emby-select[disabled])`. A picker offering one option is not a decision, and the value is already on the chips and in the facts panel |
| `.detailButton-icon{font-size:1.6em!important}` | (0,1,0) **!important** | `font-size:20px!important` at (0,4,1) |
| `.detailsGroupItem,.trackSelections .selectContainer{margin:0 0 .5em!important}` | (0,1,0) / (0,2,0) **!important** | `margin:0 0 6px!important` and `margin:0!important` at (0,2,1)/(0,3,1) |
| `.detail-clamp-text{-webkit-line-clamp:12}`, 6 above 40em — added to `.overview` by jf-web and **removed** by "Show more" | (0,1,0) | 3 lines on `.overview.detail-clamp-text` at (0,4,1), so the expand control still has something to undo |
| `.overflowPortraitCard` nine-step vw ladder | (0,1,0) | `width:130px` on `> .card` at (0,4,1) |
| `[dir=ltr] .itemsContainer>.card>.cardBox{margin-right:1.2em}` and `.cardBox-bottompadded{…!important}` | (0,4,0) / (0,1,0) **!** | `margin:0!important`, exactly as section (l) does |
| `.emby-scroller{padding-left:3.3%}` + its `@supports` safe-area twin | (0,1,0) | `padding:0` at (0,3,1) |
| `.listItemImage-large{height:13vw;width:19.5vw}` — 374×250 at 1920 | (0,1,0) | `208px × 117px` at (0,2,1) |
| `.indicator{height:2em;width:2em}` + `.countIndicator,.playedIndicator{box-shadow:…;color:#fff}` | (0,1,0) | 24px cyan disc at (0,4,1) |
| `.listItem:hover` / `:focus` fills from section (b) | (0,1,0) | `background:transparent` — the still carries the state, so a row cannot paint a band across the column |
| `themes/<name>/theme.css` painting `.detailPagePrimaryContainer` / `.detailPageSecondaryContainer` `#101010` and `.detailRibbon` `rgba(32,32,32,.8)` | (0,1,0) | `background:transparent` at (0,3,1). **Only findable on a running page** — that sheet is not in the chunk CSS. Measured before the rule: two opaque slabs, 640×1214 and 836×1057, over the art the page is built around |
| `.listItemBody` blockified — `{display:inline-block}` wins on source order, and as a flex item that computes to `block`, so stock's own `flex-direction:column` never takes | (0,1,0) | restated as a real flex column at (0,4,1) |
| The progress bar carries **both** `.itemProgressBar` (itemDetails.css, `position:relative`) and `.listItemProgressBar` (main.css, `position:absolute`) | (0,1,0) each — the winner is whichever chunk loads last | `position:absolute` restated. Measured live with the relative one winning: the bar left the still entirely and rendered **0 px wide, in flow, at x=296** beside a 208 px image ending at x=286 |

**Nine `!important` declarations**, every one of them answering an `!important`
or an inline style in jellyfin-web: the page's top padding, the inline
misc-info margin, the two stock group margins (`.detailsGroupItem` and
`.trackSelections .selectContainer`), the button margin/padding pair, the icon
disc padding, the icon font size, and the card box margin. Everything else in
the section is plain specificity.

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

### Real OS input at 300% (2026-09-10)

CDP `Input.dispatchMouseEvent` enters CEF **below** the platform layer, so it
cannot exercise `src/input/src/click_count.rs` (the double/triple-click
counter). To drive the Windows double-click path, move the real cursor: a
DPI-aware PowerShell process (`SetProcessDpiAwarenessContext(-4)`),
`ClientToScreen(hwnd, {0,0})` for the client origin, then `SetCursorPos` +
`mouse_event`. On the 4K TV at 300 % the map is `physicalX = cssX * 3`,
`physicalY = cssY * 3 + 67` while maximised and `cssY * 3` fullscreen. Confirm it
once with a page-side `mousemove` recorder rather than `elementFromPoint`: move
to a physical point and read the `clientX`/`clientY` the page saw.

`MouseEvent.detail` on `mousedown` **is** the click count the platform handed
CEF, so `MAX_CLICKS` can be asserted from the page. Measured: 1 then 2 for two
presses 200 ms apart at one point, 1 then 1 for presses 800 ms apart, and 1/2/3
for a triple click — `MAX_CLICKS` holding at 3. Nothing logs the count;
`log_press` in `src/windows/src/input.rs` prints only the coordinates.

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
