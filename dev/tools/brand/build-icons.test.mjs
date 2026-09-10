// Unit tests for the icon container writer. Run with:
//
//     node --test dev/tools/brand/build-icons.test.mjs
//
// (or `just test-js`). The interesting half of this script is not the
// rasterization — that is resvg's job — but everything it does afterwards by
// hand: a PNG reader, the un-filtering, the bottom-up BGRA DIB with its 1bpp
// AND mask, and the two container layouts. Those are byte formats, so they are
// tested by building a PNG here, running it through, and reading the bytes back
// out at the offsets Windows and macOS will read them from.
//
// Nothing here needs @resvg/resvg-js (which is why main() imports it lazily),
// downloads anything, or writes outside a temp dir under os.tmpdir().
import test from 'node:test';
import assert from 'node:assert';
import { deflateSync } from 'node:zlib';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

import {
    renderPng, pngInfo, pngToRgba, dibEntry, buildIco, buildIcns, verifyIco, verifyIcns,
    main, SIZES, ICO_SIZES, ICNS_CHUNKS, FLAT_BELOW
} from './build-icons.mjs';

const PNG_MAGIC = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

// The reader ignores CRCs, so the encoder here writes zeros for them.
function chunk(type, data) {
    const out = Buffer.alloc(12 + data.length);
    out.writeUInt32BE(data.length, 0);
    out.write(type, 4, 4, 'ascii');
    data.copy(out, 8);
    return out;
}

function ihdr(width, height, over = {}) {
    const data = Buffer.alloc(13);
    data.writeUInt32BE(width, 0);
    data.writeUInt32BE(height, 4);
    data[8] = over.depth === undefined ? 8 : over.depth;
    data[9] = over.colorType === undefined ? 6 : over.colorType;
    data[12] = over.interlace === undefined ? 0 : over.interlace;
    return chunk('IHDR', data);
}

// Encode RGBA pixels as a PNG, applying `filter` to every row so the decoder's
// un-filtering is what is under test.
function encodePng(width, height, pixel, filter = 0) {
    const bpp = 4;
    const stride = width * bpp;
    const rawRows = [];
    for (let y = 0; y < height; y++) {
        const row = Buffer.alloc(stride);
        for (let x = 0; x < width; x++) {
            const [r, g, b, a] = pixel(x, y);
            row[x * bpp] = r; row[x * bpp + 1] = g; row[x * bpp + 2] = b; row[x * bpp + 3] = a;
        }
        rawRows.push(row);
    }
    const encoded = [];
    for (let y = 0; y < height; y++) {
        const row = rawRows[y];
        const prev = y > 0 ? rawRows[y - 1] : Buffer.alloc(stride);
        const line = Buffer.alloc(stride + 1);
        line[0] = filter;
        for (let x = 0; x < stride; x++) {
            const a = x >= bpp ? row[x - bpp] : 0;
            const b = prev[x];
            const c = x >= bpp ? prev[x - bpp] : 0;
            let sub;
            switch (filter) {
                case 1: sub = a; break;
                case 2: sub = b; break;
                case 3: sub = (a + b) >> 1; break;
                case 4: {
                    const p = a + b - c;
                    const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
                    sub = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
                    break;
                }
                default: sub = 0;
            }
            line[x + 1] = (row[x] - sub) & 0xff;
        }
        encoded.push(line);
    }
    return Buffer.concat([
        PNG_MAGIC,
        ihdr(width, height),
        chunk('IDAT', deflateSync(Buffer.concat(encoded))),
        chunk('IEND', Buffer.alloc(0))
    ]);
}

const solid = (rgba) => () => rgba;

// Every size the containers ask for, as real PNGs.
function pngSet(pixel = solid([1, 2, 3, 255])) {
    const pngs = new Map();
    for (const size of SIZES) pngs.set(size, encodePng(size, size, pixel));
    return pngs;
}

function quiet(t) {
    const lines = [];
    const real = console.log;
    console.log = (...args) => lines.push(args.join(' '));
    t.after(() => { console.log = real; });
    return lines;
}

// ---------------------------------------------------------------------------
// renderPng
// ---------------------------------------------------------------------------

function fakeResvg(record, sizeFor = (s) => s) {
    return class FakeResvg {
        constructor(svg, opts) {
            this.size = opts.fitTo.value;
            record.push({ svg: svg.toString('utf8'), opts });
        }
        render() {
            const drawn = sizeFor(this.size);
            return {
                width: drawn,
                height: drawn,
                asPng: () => encodePng(2, 2, solid([0, 0, 0, 0]))
            };
        }
    };
}

test('renderPng takes the flat master below 48px and the full one at or above it', () => {
    const seen = [];
    const Resvg = fakeResvg(seen);
    const svgs = { full: Buffer.from('<svg>full</svg>'), flat: Buffer.from('<svg>flat</svg>') };
    renderPng(Resvg, svgs, FLAT_BELOW - 1);
    renderPng(Resvg, svgs, FLAT_BELOW);
    assert.deepStrictEqual(seen.map((c) => c.svg), ['<svg>flat</svg>', '<svg>full</svg>']);
    assert.deepStrictEqual(seen[1].opts, {
        fitTo: { mode: 'width', value: 48 }, background: 'rgba(0,0,0,0)'
    });
});

test('renderPng refuses a raster that came back the wrong size', () => {
    const Resvg = fakeResvg([], () => 31);
    const svgs = { full: Buffer.from('f'), flat: Buffer.from('l') };
    assert.throws(() => renderPng(Resvg, svgs, 32), /resvg returned 31x31 for size 32/);
});

// ---------------------------------------------------------------------------
// The PNG reader
// ---------------------------------------------------------------------------

test('pngInfo reads the header and joins split IDATs', () => {
    const halves = Buffer.concat([
        PNG_MAGIC,
        ihdr(4, 3),
        chunk('IDAT', Buffer.from([1, 2, 3])),
        chunk('tEXt', Buffer.from('ignored')),
        chunk('IDAT', Buffer.from([4, 5])),
        chunk('IEND', Buffer.alloc(0))
    ]);
    const info = pngInfo(halves);
    assert.strictEqual(info.width, 4);
    assert.strictEqual(info.height, 3);
    assert.strictEqual(info.depth, 8);
    assert.strictEqual(info.colorType, 6);
    assert.strictEqual(info.interlace, 0);
    assert.deepStrictEqual([...info.idat], [1, 2, 3, 4, 5]);
});

test('pngInfo stops at IEND and rejects anything that is not a PNG', () => {
    const trailing = Buffer.concat([
        PNG_MAGIC, ihdr(1, 1), chunk('IEND', Buffer.alloc(0)), Buffer.from('junk after the end')
    ]);
    assert.strictEqual(pngInfo(trailing).width, 1);
    assert.throws(() => pngInfo(Buffer.alloc(64)), /not a PNG/);
});

test('pngInfo refuses a stream with no IHDR', () => {
    const headless = Buffer.concat([PNG_MAGIC, chunk('IDAT', Buffer.from([1]))]);
    assert.throws(() => pngInfo(headless), /PNG has no IHDR/);
});

test('pngToRgba round-trips unfiltered pixels', () => {
    const pixel = (x, y) => [x * 10, y * 20, 30, 255 - x];
    const decoded = pngToRgba(encodePng(3, 2, pixel));
    assert.strictEqual(decoded.width, 3);
    assert.strictEqual(decoded.height, 2);
    for (let y = 0; y < 2; y++) {
        for (let x = 0; x < 3; x++) {
            const at = (y * 3 + x) * 4;
            assert.deepStrictEqual([...decoded.data.subarray(at, at + 4)], pixel(x, y));
        }
    }
});

test('pngToRgba reverses the sub, up, average and paeth filters', () => {
    const pixel = (x, y) => [x * 7 + y, 255 - x * 3, y * 11, 200 + x];
    const plain = pngToRgba(encodePng(4, 4, pixel, 0)).data;
    for (const filter of [1, 2, 3, 4]) {
        const decoded = pngToRgba(encodePng(4, 4, pixel, filter)).data;
        assert.deepStrictEqual(decoded, plain, 'filter ' + filter);
    }
});

test('pngToRgba refuses a depth or colour type it cannot read', () => {
    const paletted = Buffer.concat([
        PNG_MAGIC, ihdr(1, 1, { colorType: 3 }),
        chunk('IDAT', deflateSync(Buffer.alloc(2))), chunk('IEND', Buffer.alloc(0))
    ]);
    assert.throws(() => pngToRgba(paletted), /unsupported PNG \(depth 8, color 3\)/);

    const interlaced = Buffer.concat([
        PNG_MAGIC, ihdr(1, 1, { interlace: 1 }),
        chunk('IDAT', deflateSync(Buffer.alloc(5))), chunk('IEND', Buffer.alloc(0))
    ]);
    assert.throws(() => pngToRgba(interlaced), /unsupported PNG/);
});

test('pngToRgba refuses an unknown row filter', () => {
    const bad = Buffer.concat([
        PNG_MAGIC, ihdr(1, 1),
        chunk('IDAT', deflateSync(Buffer.from([9, 0, 0, 0, 0]))),
        chunk('IEND', Buffer.alloc(0))
    ]);
    assert.throws(() => pngToRgba(bad), /bad PNG filter 9/);
});

// ---------------------------------------------------------------------------
// ICO
// ---------------------------------------------------------------------------

test('dibEntry writes a 32bpp bottom-up DIB with a doubled height', () => {
    const png = encodePng(2, 2, (x, y) => [x + 1, y + 1, 9, 255]);
    const dib = dibEntry(png);
    assert.strictEqual(dib.readUInt32LE(0), 40, 'biSize');
    assert.strictEqual(dib.readInt32LE(4), 2, 'biWidth');
    assert.strictEqual(dib.readInt32LE(8), 4, 'biHeight is XOR + AND');
    assert.strictEqual(dib.readUInt16LE(12), 1, 'biPlanes');
    assert.strictEqual(dib.readUInt16LE(14), 32, 'biBitCount');
    assert.strictEqual(dib.readUInt32LE(16), 0, 'BI_RGB');
    assert.strictEqual(dib.readUInt32LE(20), 2 * 4 * 2 + 4 * 2, 'image size');
    assert.strictEqual(dib.length, 40 + 16 + 8);

    // First DIB row is the bottom image row, and the channels are BGRA.
    const first = [...dib.subarray(40, 48)];
    assert.deepStrictEqual(first, [9, 2, 1, 255, 9, 2, 2, 255]);
});

test('dibEntry marks fully transparent pixels in the AND mask', () => {
    const png = encodePng(2, 1, (x) => [0, 0, 0, x === 0 ? 0 : 255]);
    const dib = dibEntry(png);
    const mask = dib.subarray(40 + 8);
    assert.strictEqual(mask.length, 4, 'one 4-byte padded row');
    assert.strictEqual(mask[0], 0x80, 'the transparent left pixel is masked out');
});

test('buildIco lays out a directory of seven entries, PNG only at 256', () => {
    const ico = buildIco(pngSet());
    assert.strictEqual(ico.readUInt16LE(0), 0, 'reserved');
    assert.strictEqual(ico.readUInt16LE(2), 1, 'type: icon');
    assert.strictEqual(ico.readUInt16LE(4), ICO_SIZES.length);

    let expectedOffset = 6 + 16 * ICO_SIZES.length;
    ICO_SIZES.forEach((size, i) => {
        const e = 6 + 16 * i;
        assert.strictEqual(ico.readUInt8(e), size >= 256 ? 0 : size, 'width byte ' + size);
        assert.strictEqual(ico.readUInt8(e + 1), size >= 256 ? 0 : size);
        assert.strictEqual(ico.readUInt16LE(e + 4), 1, 'planes');
        assert.strictEqual(ico.readUInt16LE(e + 6), 32, 'bit count');
        const bytes = ico.readUInt32LE(e + 8);
        assert.strictEqual(ico.readUInt32LE(e + 12), expectedOffset, 'offset ' + size);
        const payload = ico.subarray(expectedOffset, expectedOffset + bytes);
        const isPng = payload.readUInt32BE(0) === 0x89504e47;
        assert.strictEqual(isPng, size >= 256, size + ' payload kind');
        expectedOffset += bytes;
    });
    assert.strictEqual(expectedOffset, ico.length, 'no gaps, no tail');
});

test('verifyIco accepts what buildIco wrote and catches a lying directory', (t) => {
    const logged = quiet(t);
    const ico = buildIco(pngSet());
    verifyIco(ico);
    assert.match(logged[0], /ICO: 7 entries/);
    assert.strictEqual(logged.length, 1 + ICO_SIZES.length);

    const badHeader = Buffer.from(ico);
    badHeader.writeUInt16LE(2, 2);
    assert.throws(() => verifyIco(badHeader), /bad ICO header/);

    const badSize = Buffer.from(ico);
    badSize.writeUInt8(20, 6); // first entry claims 20px, payload is 16
    assert.throws(() => verifyIco(badSize), /dir says 20x16, payload is 16x16/);
});

// ---------------------------------------------------------------------------
// ICNS
// ---------------------------------------------------------------------------

test('buildIcns wraps every chunk with its OSType and length', () => {
    const pngs = pngSet();
    const icns = buildIcns(pngs);
    assert.strictEqual(icns.toString('ascii', 0, 4), 'icns');
    assert.strictEqual(icns.readUInt32BE(4), icns.length, 'the header counts itself');

    let off = 8;
    for (const [type, size] of ICNS_CHUNKS) {
        assert.strictEqual(icns.toString('ascii', off, off + 4), type);
        const len = icns.readUInt32BE(off + 4);
        assert.strictEqual(len, 8 + pngs.get(size).length, type + ' length');
        assert.deepStrictEqual(icns.subarray(off + 8, off + len), pngs.get(size));
        off += len;
    }
    assert.strictEqual(off, icns.length);
});

test('verifyIcns accepts what buildIcns wrote and catches a truncation', (t) => {
    const logged = quiet(t);
    const icns = buildIcns(pngSet());
    verifyIcns(icns);
    assert.match(logged[0], /ICNS: \d+ bytes/);
    assert.strictEqual(logged.length, 1 + ICNS_CHUNKS.length);

    assert.throws(() => verifyIcns(Buffer.alloc(16)), /bad ICNS magic/);

    const short = Buffer.from(icns.subarray(0, icns.length - 4));
    assert.throws(() => verifyIcns(short), /ICNS length \d+ != file size/);
});

test('verifyIcns catches a payload that is not the size its OSType promises', (t) => {
    quiet(t);
    const pngs = pngSet();
    pngs.set(128, encodePng(64, 64, solid([0, 0, 0, 255]))); // ic07 must be 128px
    assert.throws(() => verifyIcns(buildIcns(pngs)), /ICNS ic07: expected 128px, payload is 64x64/);
});

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

test('main renders every size and writes the three containers', async (t) => {
    const repo = mkdtempSync(join(tmpdir(), 'astrofin-icons-'));
    t.after(() => rmSync(repo, { recursive: true, force: true }));
    mkdirSync(resolve(repo, 'resources/brand'), { recursive: true });
    writeFileSync(resolve(repo, 'resources/brand/astrofin-icon.svg'), '<svg>full</svg>');
    writeFileSync(resolve(repo, 'resources/brand/astrofin-icon-flat.svg'), '<svg>flat</svg>');

    const seen = [];
    class SizedResvg {
        constructor(svg, opts) {
            this.size = opts.fitTo.value;
            seen.push({ svg: svg.toString('utf8'), size: this.size });
        }
        render() {
            const size = this.size;
            return {
                width: size,
                height: size,
                asPng: () => encodePng(size, size, solid([10, 20, 30, 255]))
            };
        }
    }

    const logged = quiet(t);
    const out = await main({ repo, Resvg: SizedResvg });
    assert.strictEqual(logged[logged.length - 1], '\nok', 'the run reported success');

    assert.deepStrictEqual(
        seen.slice(0, SIZES.length).map((c) => c.size), SIZES, 'every size rendered once'
    );
    assert.deepStrictEqual(
        seen.slice(0, SIZES.length).filter((c) => c.svg.includes('flat')).map((c) => c.size),
        SIZES.filter((s) => s < FLAT_BELOW)
    );
    assert.strictEqual(seen[seen.length - 1].size, 256, 'the linux svg is re-parsed at 256');

    // The files verify by their own reader, which is what the CLI prints.
    verifyIco(readFileSync(out.ico));
    verifyIcns(readFileSync(out.icns));
    assert.strictEqual(readFileSync(out.svg, 'utf8'), '<svg>full</svg>');
});

test('main fails when a master SVG is missing rather than writing half a set', async (t) => {
    const repo = mkdtempSync(join(tmpdir(), 'astrofin-icons-'));
    t.after(() => rmSync(repo, { recursive: true, force: true }));
    mkdirSync(resolve(repo, 'resources/brand'), { recursive: true });
    writeFileSync(resolve(repo, 'resources/brand/astrofin-icon.svg'), '<svg>full</svg>');
    quiet(t);
    await assert.rejects(() => main({ repo, Resvg: fakeResvg([]) }), /ENOENT/);
    assert.throws(() => readFileSync(resolve(repo, 'resources/win/astrofin.ico')), /ENOENT/);
});

test('importing the module neither rasterizes nor needs @resvg/resvg-js', async () => {
    // The static import used to make this file unloadable without the native
    // package installed; it is a dynamic import inside main() now.
    const source = readFileSync(new URL('./build-icons.mjs', import.meta.url), 'utf8');
    assert.ok(!/^import .*@resvg/m.test(source), 'no top-level resvg import');
    const mod = await import('./build-icons.mjs');
    assert.strictEqual(typeof mod.main, 'function');
});
