# Bundled web fonts

The three families the Astrofin design system names in `src/web/astrofin-tokens.css`
(`--af-font-display`, `--af-font-body`, `--af-font-mono`), bundled so the UI looks
the same on a machine with none of them installed.

| Family | Weights | File | Version | Bytes |
| --- | --- | --- | --- | --- |
| Sora (display) | 200–400, variable | `sora/Sora-Variable.woff2` | v17 | 25 104 |
| Inter (body) | 400–600, variable | `inter/Inter-Variable.woff2` | v20 | 48 256 |
| IBM Plex Mono (metadata) | 400 | `ibm-plex-mono/IBMPlexMono-400.woff2` | v20 | 14 708 |
| IBM Plex Mono (metadata) | 500 | `ibm-plex-mono/IBMPlexMono-500.woff2` | v20 | 14 888 |

Google serves Sora and Inter as **variable** fonts: the CSS API returns the same
woff2 for every requested weight of those families and pins the weight in the
`@font-face` rule. Each is therefore stored once and declared with a weight
*range*. IBM Plex Mono is still shipped as static instances, one file per weight.

## Source and provenance

- Fetched **2026-09-07** from the Google Fonts CSS API, latin subset only
  (no latin-ext, cyrillic, greek or vietnamese blocks).
- Request (a modern Chrome `User-Agent` is required, otherwise the API answers
  with `ttf` and no `unicode-range`):

  ```
  https://fonts.googleapis.com/css2?family=Sora:wght@200;300;400&family=Inter:wght@400;500;600&family=IBM+Plex+Mono:wght@400;500&display=swap
  ```

- `manifest.json` records, per file, the exact `fonts.gstatic.com` URL it came
  from, the upstream version, the byte length and the latin `unicode-range`.
- License texts are the upstream ones from
  [`google/fonts`](https://github.com/google/fonts): `ofl/sora/OFL.txt`,
  `ofl/inter/OFL.txt`, `ofl/ibmplexmono/OFL.txt`.

## License

All three families are licensed under the **SIL Open Font License, Version 1.1**
(OFL-1.1). The full text ships next to each family's woff2 as
`<family>/OFL.txt`. OFL-1.1 permits bundling and redistribution with an
application, including a GPL-2.0 one; the fonts are not modified.

## Regenerating

```
node dev/tools/brand/fetch-fonts.mjs   # refresh this directory + manifest.json
node dev/tools/brand/build-fonts.mjs   # regenerate src/web/astrofin-fonts.css
```

`src/web/astrofin-fonts.css` is generated, not hand-written: it inlines each
woff2 as a `data:font/woff2;base64,…` URL so the sheet can be injected into a
page served from the Jellyfin server's origin without a cross-scheme font load.
