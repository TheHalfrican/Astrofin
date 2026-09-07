# Astrofin — UI design brief for Claude Design

## What you are designing
Astrofin is a desktop client for Jellyfin (a self-hosted media server) that runs on Windows, macOS and Linux. It is a fork of Jellium Desktop: a Chromium web view showing the Jellyfin web UI, with a native mpv video player rendered underneath. The visual identity is being rebuilt from scratch. Design the complete look and motion language for it.

Creative direction in one line: **the PlayStation 5 home screen, set in deep space.** Immersive full-bleed artwork, huge quiet typography, a single focused element at a time, and motion that feels weightless and expensive.

## Audience and context
- One person's home theater PC and living-room use. 4K display at 300% scaling, also used at 1440p desktop scale. Design at 1920x1080 artboards and assume everything scales.
- Navigation is keyboard, mouse and game controller. Every interactive element needs an obvious focus state. Focus, not hover, is the primary state.
- Content is movies, TV, and anime with rich artwork (posters 2:3, backdrops 16:9, logos, episode thumbnails). Artwork is the hero; the UI is the frame.

## Space theme, defined
- Palette: near-black indigo base (not pure black), with nebula accents. Suggested anchors: base #070A14, surface #0E1326, primary glow cyan #6FE3FF, secondary violet #8B6CFF, warm accent for warnings/"live" #FFB86B, text #EAF0FF at 92% and a muted #9AA6C8. Provide a full token set (semantic names, dark only, plus a contrast check against WCAG AA).
- Background: a subtle starfield with two or three parallax layers and very slow drift. Selected item's backdrop art bleeds in behind everything with a blur and a dark gradient, PS5-style, and crossfades when focus changes.
- Materials: frosted-glass panels (backdrop blur, 1px luminous edge), soft outer glows on focus, thin hairline dividers. No heavy drop shadows, no bevels, no skeuomorphism.
- Motifs: orbit rings for loading, a constellation-line treatment for progress and timelines, a small "fin" wordmark (Astro + fin: a rocket fin or a star with a swept tail). Propose 3 logo directions.
- Type: a geometric sans with a light display weight. Prefer open-source: Sora or Outfit for display, Inter for body. Big type scale: page titles 48-64px, tile labels 18-20px, body 16px. Tracking slightly open on titles.

## PS5 behaviors to borrow
- Home: one horizontal row of large media tiles at the top (libraries, Continue Watching, Next Up), the focused tile enlarged with a glowing ring, and the rest of the screen dedicated to the focused item's artwork, logo, and a short "spotlight" panel with title, year, runtime, rating and a Play button.
- Item detail ("game hub" layout): full-bleed key art, logo top-left, a vertical stack of quick actions (Play / Resume / Trailer / Mark watched), metadata chips, then horizontal shelves for cast, similar titles, and episodes.
- Library browsing: a poster grid with a focused-card lift, and a fast alphabet or genre jump rail.
- Transitions: crossfade plus a gentle scale/parallax between pages, never a hard cut. Tile focus scales 1.00 to 1.06 in ~180 ms ease-out with a glow ring fading in. Background art crossfades over ~600 ms. Idle ambient drift on the starfield. Provide a motion token table (durations, easings, distances) and a storyboard for the three key transitions: home tile focus change, home to item detail, and play button to player.
- Player overlay: appears on input, hides after ~3 s. Frosted bar at the bottom with a constellation-style timeline (chapter markers as stars), current/remaining time, and minimal icons. Subtitles and audio pickers as glass popovers. Nothing else on screen during playback.
- Provide a reduced-motion variant: same layout, transitions become simple fades.

## Screens to deliver (artboards)
1. Server connect and sign in (this screen is fully ours; it is the first thing the app shows).
2. Home.
3. Library grid (Movies) with the jump rail.
4. Item detail for a movie.
5. Item detail for a TV series showing a season and episode list.
6. Player overlay over a paused frame, plus the subtitle picker popover.
7. Settings (ours): sections for Server, Playback (hardware decoding, audio passthrough), Video quality modes with a two-option toggle labeled "Movies" and "Anime" (this switches upscaling shaders and is a headline feature; make it feel like a mode switch, not a checkbox), and About.
8. Component sheet: tiles (poster, wide, episode), buttons (primary, secondary, icon), chips, focus ring, glass panel, popover, toast, loading orbit, empty state.
9. One artboard showing the logo directions and the wordmark in context.

## Technical constraints (keep designs buildable)
- The main screens are the Jellyfin web UI restyled by injected CSS and small JS, so layouts must be expressible with CSS: transforms, gradients, backdrop-filter, CSS animations and transitions. No 3D scenes, no video backgrounds, no WebGL.
- Artwork ratios are fixed by the server: posters 2:3, backdrops 16:9, logos variable. Design placeholders for missing art.
- Text can be long (anime titles, episode names): show truncation and two-line wrapping rules.
- Must remain readable on a TV from 3 m: minimum 16px body, 4.5:1 contrast for text on glass.

## Deliverables
- Design tokens (color, type, spacing, radius, elevation, motion) as a table and as CSS custom properties.
- The nine artboards above, 1920x1080, dark only.
- Motion storyboard sheet with keyframe notes and timings, since the tool cannot animate.
- Short rationale for the layout decisions, noting anywhere you deviated from the PS5 reference on purpose.

Start with the tokens and the Home screen, show them, and then build outward once the direction is confirmed.
