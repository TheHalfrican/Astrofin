// Fetches the branded web fonts (latin subset, woff2) from Google Fonts into
// resources/fonts/, together with each family's OFL-1.1 license text and a
// manifest recording the source URL, version and unicode-range of every file.
//
// Run: node dev/tools/brand/fetch-fonts.mjs
// Then: node dev/tools/brand/build-fonts.mjs   (regenerates the CSS)
//
// The CSS API is asked with a modern Chrome User-Agent so it answers with
// woff2 and per-subset unicode-range blocks; only the `latin` block of each
// face is kept.
//
// Google serves Sora and Inter as *variable* fonts: every requested weight of
// a family resolves to one identical woff2. Those are stored once as
// <Family>-Variable.woff2 and declared with a font-weight range, so the
// generated CSS carries one payload per family instead of three. IBM Plex Mono
// is still shipped as static instances, one file per weight.
//
// Importing this file downloads nothing: `main()` runs only when the file is
// the process entry point, so the unit tests can drive it with a stubbed
// `fetch` against a temp dir.

import { writeFileSync, mkdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const FONTS = resolve(HERE, '../../..', 'resources/fonts');

export const UA =
    'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 ' +
    '(KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36';

export const CSS_URL =
    'https://fonts.googleapis.com/css2' +
    '?family=Sora:wght@200;300;400' +
    '&family=Inter:wght@400;500;600' +
    '&family=IBM+Plex+Mono:wght@400;500' +
    '&display=swap';

// family name as Google reports it -> [directory, file stem, OFL path in google/fonts]
export const FAMILIES = {
    'Sora': ['sora', 'Sora', 'ofl/sora/OFL.txt'],
    'Inter': ['inter', 'Inter', 'ofl/inter/OFL.txt'],
    'IBM Plex Mono': ['ibm-plex-mono', 'IBMPlexMono', 'ofl/ibmplexmono/OFL.txt'],
};

const OFL_BASE = 'https://raw.githubusercontent.com/google/fonts/main/';

export async function get(url, asText = true) {
    const res = await fetch(url, { headers: { 'User-Agent': UA } });
    if (!res.ok) throw new Error(`${res.status} ${res.statusText} for ${url}`);
    return asText ? res.text() : Buffer.from(await res.arrayBuffer());
}

/** Splits the Google CSS into { subset, family, weight, url, unicodeRange }. */
export function parseFaces(css) {
    const out = [];
    const re = /\/\*\s*([a-z-]+)\s*\*\/\s*@font-face\s*\{([^}]*)\}/g;
    for (let m; (m = re.exec(css));) {
        const [, subset, body] = m;
        const pick = (k) => (body.match(new RegExp(`${k}:\\s*([^;]+);`)) || [])[1]?.trim();
        out.push({
            subset,
            family: pick('font-family').replace(/^['"]|['"]$/g, ''),
            weight: Number(pick('font-weight')),
            url: (body.match(/url\(([^)]+)\)/) || [])[1],
            unicodeRange: pick('unicode-range'),
        });
    }
    return out;
}

/**
 * Downloads every face and writes resources/fonts/. `options.fontsDir` moves
 * the whole write tree (the tests point it at a temp dir); with it unset this
 * is exactly what `node dev/tools/brand/fetch-fonts.mjs` does.
 */
export async function main(options = {}) {
    const fontsDir = options.fontsDir || FONTS;

    const css = await get(CSS_URL);
    const faces = parseFaces(css).filter((f) => f.subset === 'latin');
    if (!faces.length) throw new Error('no latin faces in the Google CSS response');

    // Download every distinct URL once.
    const payloads = new Map();
    for (const face of faces) {
        if (payloads.has(face.url)) continue;
        const bytes = await get(face.url, false);
        if (bytes.toString('ascii', 0, 4) !== 'wOF2') throw new Error(`${face.url} is not woff2`);
        payloads.set(face.url, bytes);
    }

    const manifest = {
        fetchedAt: new Date().toISOString().slice(0, 10),
        cssUrl: CSS_URL,
        userAgent: UA,
        license: 'OFL-1.1',
        faces: [],
    };

    for (const [family, [dir, stem]] of Object.entries(FAMILIES)) {
        const mine = faces.filter((f) => f.family === family);
        if (!mine.length) throw new Error(`no latin face for ${family}`);

        // Group by payload hash: one group == one file we store.
        const groups = new Map();
        for (const f of mine) {
            const bytes = payloads.get(f.url);
            const key = createHash('sha256').update(bytes).digest('hex');
            if (!groups.has(key)) groups.set(key, { bytes, faces: [], url: f.url, unicodeRange: f.unicodeRange });
            groups.get(key).faces.push(f);
        }

        for (const g of groups.values()) {
            const weights = g.faces.map((f) => f.weight).sort((a, b) => a - b);
            const variable = weights.length > 1;
            const rel = `${dir}/${stem}-${variable ? 'Variable' : weights[0]}.woff2`;
            const abs = resolve(fontsDir, rel);
            mkdirSync(dirname(abs), { recursive: true });
            writeFileSync(abs, g.bytes);
            // The gstatic path carries the font version, e.g. .../ibmplexmono/v20/xxx.woff2
            const version = (g.url.match(/\/(v\d+)\//) || [])[1] || 'unknown';
            manifest.faces.push({
                family,
                // A single value for a static instance, "min max" for a variable one.
                weight: variable ? `${weights[0]} ${weights[weights.length - 1]}` : String(weights[0]),
                style: 'normal',
                variable,
                file: rel,
                bytes: g.bytes.length,
                version,
                source: g.url,
                unicodeRange: g.unicodeRange,
            });
            console.log(`${rel.padEnd(36)} ${String(g.bytes.length).padStart(7)} bytes  ${version}  wght ${variable ? weights.join('/') + ' (variable)' : weights[0]}`);
        }
    }

    manifest.faces.sort((a, b) => a.file.localeCompare(b.file));

    for (const [family, [dir, , oflPath]] of Object.entries(FAMILIES)) {
        const text = await get(OFL_BASE + oflPath);
        if (!/SIL OPEN FONT LICENSE/i.test(text)) throw new Error(`${oflPath} does not look like an OFL`);
        writeFileSync(resolve(fontsDir, dir, 'OFL.txt'), text);
        console.log(`${(dir + '/OFL.txt').padEnd(36)} ${String(text.length).padStart(7)} bytes  (${family})`);
    }

    writeFileSync(resolve(fontsDir, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
    console.log('\nwrote resources/fonts/manifest.json');
    return manifest;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    await main();
}
