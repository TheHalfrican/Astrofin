// Unit tests for the web-font fetcher. Run with:
//
//     node --test dev/tools/brand/fetch-fonts.test.mjs
//
// (or `just test-js`). No test here reaches the network: `fetch` is replaced
// for the duration and every write goes to a temp dir under os.tmpdir(), never
// resources/. What is being tested is the part that is ours rather than
// Google's — the CSS parse, the per-family grouping that collapses a variable
// font's three weights into one payload, and the refusals (not woff2, not an
// OFL, a family the response never mentioned) that keep a bad download from
// being written into the repo.
import test from 'node:test';
import assert from 'node:assert';
import { mkdtempSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

import { get, parseFaces, main, UA, CSS_URL, FAMILIES } from './fetch-fonts.mjs';

const GSTATIC = 'https://fonts.gstatic.com/s';
const SORA_URL = `${GSTATIC}/sora/v14/sora-variable.woff2`;
const INTER_URL = `${GSTATIC}/inter/v18/inter-variable.woff2`;
const MONO_400 = `${GSTATIC}/ibmplexmono/v20/mono-400.woff2`;
const MONO_500 = `${GSTATIC}/ibmplexmono/v20/mono-500.woff2`;
const LATIN_RANGE = 'U+0000-00FF, U+0131, U+2000-206F';

function woff2(fill) {
    const buf = Buffer.alloc(64, fill.charCodeAt(0));
    buf.write('wOF2', 0, 4, 'ascii');
    return buf;
}

function block(subset, family, weight, url, range = LATIN_RANGE) {
    return `/* ${subset} */
@font-face {
  font-family: '${family}';
  font-style: normal;
  font-weight: ${weight};
  src: url(${url}) format('woff2');
  unicode-range: ${range};
}
`;
}

// What the css2 API answers with, reduced to the shape the parser cares about.
const GOOGLE_CSS = [
    block('cyrillic', 'Sora', 200, `${GSTATIC}/sora/v14/sora-cyr.woff2`, 'U+0400-045F'),
    block('latin', 'Sora', 200, SORA_URL),
    block('latin', 'Sora', 300, SORA_URL),
    block('latin', 'Sora', 400, SORA_URL),
    block('latin', 'Inter', 400, INTER_URL),
    block('latin', 'Inter', 500, INTER_URL),
    block('latin', 'Inter', 600, INTER_URL),
    block('latin', 'IBM Plex Mono', 400, MONO_400),
    block('latin', 'IBM Plex Mono', 500, MONO_500)
].join('\n');

const OFL_TEXT = 'Copyright 2026\n\nSIL OPEN FONT LICENSE Version 1.1\n';

function routes(over = {}) {
    return Object.assign({
        [CSS_URL]: { text: GOOGLE_CSS },
        [SORA_URL]: { bytes: woff2('S') },
        [INTER_URL]: { bytes: woff2('I') },
        [MONO_400]: { bytes: woff2('4') },
        [MONO_500]: { bytes: woff2('5') },
        'ofl/sora/OFL.txt': { text: OFL_TEXT },
        'ofl/inter/OFL.txt': { text: OFL_TEXT },
        'ofl/ibmplexmono/OFL.txt': { text: OFL_TEXT }
    }, over);
}

// Replaces global fetch for one test and records what was asked for.
function stubFetch(t, table) {
    const asked = [];
    const real = globalThis.fetch;
    globalThis.fetch = async (url, init) => {
        asked.push({ url, init });
        const key = Object.keys(table).find((k) => url === k || url.endsWith(k));
        const hit = key === undefined ? undefined : table[key];
        if (!hit) return { ok: false, status: 404, statusText: 'Not Found' };
        return {
            ok: hit.ok === undefined ? true : hit.ok,
            status: hit.status || 200,
            statusText: hit.statusText || 'OK',
            text: async () => (hit.text === undefined ? hit.bytes.toString('binary') : hit.text),
            arrayBuffer: async () => (hit.bytes || Buffer.from(hit.text)).buffer.slice(
                hit.bytes ? hit.bytes.byteOffset : 0,
                hit.bytes ? hit.bytes.byteOffset + hit.bytes.length : Buffer.from(hit.text).length
            )
        };
    };
    t.after(() => { globalThis.fetch = real; });
    return asked;
}

function tempFonts(t) {
    const dir = mkdtempSync(join(tmpdir(), 'astrofin-fetch-'));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    return dir;
}

function quiet(t) {
    const lines = [];
    const real = console.log;
    console.log = (...args) => lines.push(args.join(' '));
    t.after(() => { console.log = real; });
    return lines;
}

// ---------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------

test('get asks with the Chrome User-Agent the css2 API needs', async (t) => {
    const asked = stubFetch(t, routes());
    const css = await get(CSS_URL);
    assert.strictEqual(css, GOOGLE_CSS);
    assert.deepStrictEqual(asked[0].init, { headers: { 'User-Agent': UA } });
    assert.match(UA, /Chrome\/\d+/);
});

test('get returns a Buffer when asked for bytes', async (t) => {
    stubFetch(t, routes());
    const bytes = await get(SORA_URL, false);
    assert.ok(Buffer.isBuffer(bytes));
    assert.strictEqual(bytes.toString('ascii', 0, 4), 'wOF2');
});

test('get throws with the status and the url when the response is not ok', async (t) => {
    stubFetch(t, { [CSS_URL]: { ok: false, status: 429, statusText: 'Too Many Requests' } });
    await assert.rejects(() => get(CSS_URL), /429 Too Many Requests for https:\/\/fonts\.googleapis/);
    await assert.rejects(() => get('https://example.invalid/x'), /404 Not Found/);
});

// ---------------------------------------------------------------------------
// parseFaces
// ---------------------------------------------------------------------------

test('parseFaces reads every @font-face with its subset comment', () => {
    const faces = parseFaces(GOOGLE_CSS);
    assert.strictEqual(faces.length, 9);
    assert.deepStrictEqual(faces[0], {
        subset: 'cyrillic',
        family: 'Sora',
        weight: 200,
        url: `${GSTATIC}/sora/v14/sora-cyr.woff2`,
        unicodeRange: 'U+0400-045F'
    });
    assert.deepStrictEqual(faces[1], {
        subset: 'latin',
        family: 'Sora',
        weight: 200,
        url: SORA_URL,
        unicodeRange: LATIN_RANGE
    });
});

test('parseFaces unquotes the family name and keeps multi-word ones whole', () => {
    const faces = parseFaces(block('latin', 'IBM Plex Mono', 500, MONO_500));
    assert.strictEqual(faces[0].family, 'IBM Plex Mono');
    assert.strictEqual(faces[0].weight, 500);
    assert.strictEqual(typeof faces[0].weight, 'number');
});

test('parseFaces ignores CSS with no font faces in it', () => {
    assert.deepStrictEqual(parseFaces(''), []);
    assert.deepStrictEqual(parseFaces('/* latin */ body { color: red }'), []);
    assert.deepStrictEqual(parseFaces('@font-face { font-family: "Sora" }'), [],
        'without the subset comment the block is not ours to read');
});

test('parseFaces throws rather than inventing a family when one is missing', () => {
    // The css2 API always sends font-family; a response that does not is a
    // change worth failing on, not one to write half a manifest from.
    const noFamily = '/* latin */\n@font-face {\n  font-weight: 400;\n}\n';
    assert.throws(() => parseFaces(noFamily), TypeError);
});

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

test('main stores one payload per variable family and one per static weight', async (t) => {
    const dir = tempFonts(t);
    stubFetch(t, routes());
    quiet(t);
    const manifest = await main({ fontsDir: dir });

    assert.deepStrictEqual(manifest.faces.map((f) => f.file), [
        'ibm-plex-mono/IBMPlexMono-400.woff2',
        'ibm-plex-mono/IBMPlexMono-500.woff2',
        'inter/Inter-Variable.woff2',
        'sora/Sora-Variable.woff2'
    ], 'sorted by file, one file per payload');

    const sora = manifest.faces.find((f) => f.family === 'Sora');
    assert.strictEqual(sora.variable, true);
    assert.strictEqual(sora.weight, '200 400', 'the whole range in one face');
    assert.strictEqual(sora.version, 'v14', 'read off the gstatic path');
    assert.strictEqual(sora.style, 'normal');
    assert.strictEqual(sora.bytes, 64);
    assert.strictEqual(sora.source, SORA_URL);
    assert.strictEqual(sora.unicodeRange, LATIN_RANGE);

    const mono = manifest.faces.filter((f) => f.family === 'IBM Plex Mono');
    assert.deepStrictEqual(mono.map((f) => f.weight), ['400', '500']);
    assert.deepStrictEqual(mono.map((f) => f.variable), [false, false]);
});

test('main writes the payloads, the licences and the manifest', async (t) => {
    const dir = tempFonts(t);
    stubFetch(t, routes());
    quiet(t);
    await main({ fontsDir: dir });

    assert.deepStrictEqual(readFileSync(resolve(dir, 'sora/Sora-Variable.woff2')), woff2('S'));
    assert.deepStrictEqual(readFileSync(resolve(dir, 'ibm-plex-mono/IBMPlexMono-500.woff2')), woff2('5'));
    for (const [, [sub]] of Object.entries(FAMILIES)) {
        assert.ok(existsSync(resolve(dir, sub, 'OFL.txt')), sub + '/OFL.txt');
        assert.strictEqual(readFileSync(resolve(dir, sub, 'OFL.txt'), 'utf8'), OFL_TEXT);
    }

    const onDisk = JSON.parse(readFileSync(resolve(dir, 'manifest.json'), 'utf8'));
    assert.strictEqual(onDisk.cssUrl, CSS_URL);
    assert.strictEqual(onDisk.userAgent, UA);
    assert.strictEqual(onDisk.license, 'OFL-1.1');
    assert.match(onDisk.fetchedAt, /^\d{4}-\d{2}-\d{2}$/);
    assert.strictEqual(onDisk.faces.length, 4);
    assert.ok(readFileSync(resolve(dir, 'manifest.json'), 'utf8').endsWith('\n'));
});

test('main downloads each distinct url exactly once', async (t) => {
    const dir = tempFonts(t);
    const asked = stubFetch(t, routes());
    quiet(t);
    await main({ fontsDir: dir });
    const fontHits = asked.filter((a) => a.url.startsWith(GSTATIC)).map((a) => a.url);
    assert.deepStrictEqual(fontHits, [SORA_URL, INTER_URL, MONO_400, MONO_500],
        'the three Sora weights share one payload');
});

test('main refuses a response with no latin faces', async (t) => {
    const dir = tempFonts(t);
    stubFetch(t, routes({
        [CSS_URL]: { text: block('greek', 'Sora', 400, SORA_URL, 'U+0370-03FF') }
    }));
    quiet(t);
    await assert.rejects(() => main({ fontsDir: dir }), /no latin faces in the Google CSS response/);
    assert.ok(!existsSync(resolve(dir, 'manifest.json')));
});

test('main refuses a payload that is not woff2', async (t) => {
    const dir = tempFonts(t);
    stubFetch(t, routes({ [SORA_URL]: { bytes: Buffer.alloc(64, 0x41) } }));
    quiet(t);
    await assert.rejects(() => main({ fontsDir: dir }), /sora-variable\.woff2 is not woff2/);
});

test('main refuses a response that is missing one of the families', async (t) => {
    const dir = tempFonts(t);
    stubFetch(t, routes({
        [CSS_URL]: { text: block('latin', 'Sora', 400, SORA_URL) }
    }));
    quiet(t);
    await assert.rejects(() => main({ fontsDir: dir }), /no latin face for Inter/);
});

test('main refuses a licence file that is not an OFL', async (t) => {
    const dir = tempFonts(t);
    stubFetch(t, routes({ 'ofl/sora/OFL.txt': { text: 'All rights reserved.' } }));
    quiet(t);
    await assert.rejects(
        () => main({ fontsDir: dir }), /ofl\/sora\/OFL\.txt does not look like an OFL/
    );
    assert.ok(!existsSync(resolve(dir, 'manifest.json')), 'the manifest is written last');
});

test('main records an unknown version when the url carries none', async (t) => {
    const dir = tempFonts(t);
    const plain = `${GSTATIC}/sora/sora.woff2`;
    stubFetch(t, routes({
        [CSS_URL]: {
            text: [
                block('latin', 'Sora', 400, plain),
                block('latin', 'Inter', 400, INTER_URL),
                block('latin', 'IBM Plex Mono', 400, MONO_400)
            ].join('\n')
        },
        [plain]: { bytes: woff2('S') }
    }));
    quiet(t);
    const manifest = await main({ fontsDir: dir });
    assert.strictEqual(manifest.faces.find((f) => f.family === 'Sora').version, 'unknown');
});

test('main prints a line per stored file and one per licence', async (t) => {
    const dir = tempFonts(t);
    stubFetch(t, routes());
    const logged = quiet(t);
    await main({ fontsDir: dir });
    assert.strictEqual(logged.length, 4 + 3 + 1);
    assert.match(logged[0], /sora\/Sora-Variable\.woff2\s+64 bytes\s+v14\s+wght 200\/300\/400 \(variable\)/);
    assert.match(logged[logged.length - 1], /wrote resources\/fonts\/manifest\.json/);
});

test('importing the module fetches nothing', async (t) => {
    const asked = stubFetch(t, routes());
    const mod = await import('./fetch-fonts.mjs');
    assert.strictEqual(typeof mod.main, 'function');
    assert.deepStrictEqual(asked, [], 'no request was made at import time');
});
