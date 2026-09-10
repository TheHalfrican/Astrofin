// Rasterizes the Astrofin icon master SVGs into the per-platform containers.
//
//   resources/brand/astrofin-icon.svg       -> full art (>= 48 px)
//   resources/brand/astrofin-icon-flat.svg  -> no glow/stars (< 48 px)
//
// Writes:
//   resources/win/astrofin.ico                          (16..256)
//   resources/macos/AppIcon.icns                        (ic07..ic14)
//   resources/linux/io.github.thehalfrican.Astrofin.svg (copy of the master)
//
// Run: node dev/tools/brand/build-icons.mjs   (npm install first)
//
// The ICO and ICNS containers are written by hand — no image libraries beyond
// resvg for the rasterization itself. ICO small sizes are 32-bit BMP/DIB
// entries (the widest-compatible form) and 256 is a PNG entry; ICNS carries
// PNG payloads, which every macOS since 10.7 reads.
//
// Importing this file does nothing: `main()` runs only when the file is the
// process entry point, and it is what pulls in @resvg/resvg-js, so the unit
// tests can exercise the container writers without the native dependency.

import { inflateSync } from 'node:zlib';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, '../../..');

// Below this the nebula, starfield and cyan halo just turn into noise.
export const FLAT_BELOW = 48;

export const SIZES = [16, 24, 32, 48, 64, 128, 256, 512, 1024];

// ---- rasterization ---------------------------------------------------------

/**
 * @param {Function} Resvg the resvg-js constructor
 * @param {{full: Buffer, flat: Buffer}} svgs the two masters
 * @returns {Buffer} a PNG of the master rendered at size x size.
 */
export function renderPng(Resvg, svgs, size) {
    const svg = size < FLAT_BELOW ? svgs.flat : svgs.full;
    const r = new Resvg(svg, {
        fitTo: { mode: 'width', value: size },
        background: 'rgba(0,0,0,0)',
    });
    const img = r.render();
    if (img.width !== size || img.height !== size) {
        throw new Error(`resvg returned ${img.width}x${img.height} for size ${size}`);
    }
    return Buffer.from(img.asPng());
}

// ---- a minimal PNG reader (RGBA8, non-interlaced — what resvg emits) --------

export function pngInfo(buf) {
    if (buf.readUInt32BE(0) !== 0x89504e47) throw new Error('not a PNG');
    let off = 8;
    let ihdr = null;
    const idat = [];
    while (off < buf.length) {
        const len = buf.readUInt32BE(off);
        const type = buf.toString('ascii', off + 4, off + 8);
        const data = buf.subarray(off + 8, off + 8 + len);
        if (type === 'IHDR') {
            ihdr = {
                width: data.readUInt32BE(0),
                height: data.readUInt32BE(4),
                depth: data[8],
                colorType: data[9],
                interlace: data[12],
            };
        } else if (type === 'IDAT') {
            idat.push(data);
        } else if (type === 'IEND') {
            break;
        }
        off += 12 + len;
    }
    if (!ihdr) throw new Error('PNG has no IHDR');
    return { ...ihdr, idat: Buffer.concat(idat) };
}

/** Decodes an 8-bit RGBA non-interlaced PNG to a flat RGBA byte array. */
export function pngToRgba(buf) {
    const png = pngInfo(buf);
    if (png.depth !== 8 || png.colorType !== 6 || png.interlace !== 0) {
        throw new Error(`unsupported PNG (depth ${png.depth}, color ${png.colorType})`);
    }
    const { width, height } = png;
    const bpp = 4;
    const stride = width * bpp;
    const raw = inflateSync(png.idat);
    const out = Buffer.alloc(stride * height);
    let ri = 0;
    for (let y = 0; y < height; y++) {
        const filter = raw[ri++];
        const row = out.subarray(y * stride, (y + 1) * stride);
        const prev = y > 0 ? out.subarray((y - 1) * stride, y * stride) : null;
        for (let x = 0; x < stride; x++) {
            const cur = raw[ri + x];
            const a = x >= bpp ? row[x - bpp] : 0;
            const b = prev ? prev[x] : 0;
            const c = x >= bpp && prev ? prev[x - bpp] : 0;
            let v;
            switch (filter) {
                case 0: v = cur; break;
                case 1: v = cur + a; break;
                case 2: v = cur + b; break;
                case 3: v = cur + ((a + b) >> 1); break;
                case 4: {
                    const p = a + b - c;
                    const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
                    v = cur + (pa <= pb && pa <= pc ? a : pb <= pc ? b : c);
                    break;
                }
                default: throw new Error(`bad PNG filter ${filter}`);
            }
            row[x] = v & 0xff;
        }
        ri += stride;
    }
    return { width, height, data: out };
}

// ---- ICO -------------------------------------------------------------------

export const ICO_SIZES = [16, 24, 32, 48, 64, 128, 256];
// Below this a BMP/DIB entry is used; at or above it a PNG entry.
const ICO_PNG_FROM = 256;

/** 32-bit BGRA bottom-up DIB with a 1bpp AND mask, as an ICO image payload. */
export function dibEntry(png) {
    const { width, height, data } = pngToRgba(png);
    const xorStride = width * 4;
    const maskStride = (((width + 31) >> 5) << 2); // 1bpp, rows padded to 4 bytes
    const header = Buffer.alloc(40);
    header.writeUInt32LE(40, 0);            // biSize
    header.writeInt32LE(width, 4);          // biWidth
    header.writeInt32LE(height * 2, 8);     // biHeight = XOR + AND
    header.writeUInt16LE(1, 12);            // biPlanes
    header.writeUInt16LE(32, 14);           // biBitCount
    header.writeUInt32LE(0, 16);            // BI_RGB
    header.writeUInt32LE(xorStride * height + maskStride * height, 20);

    const xor = Buffer.alloc(xorStride * height);
    const mask = Buffer.alloc(maskStride * height); // all zero == fully opaque
    for (let y = 0; y < height; y++) {
        const src = (height - 1 - y) * xorStride; // DIBs are bottom-up
        const dst = y * xorStride;
        for (let x = 0; x < width; x++) {
            const s = src + x * 4;
            const d = dst + x * 4;
            xor[d + 0] = data[s + 2]; // B
            xor[d + 1] = data[s + 1]; // G
            xor[d + 2] = data[s + 0]; // R
            xor[d + 3] = data[s + 3]; // A
            if (data[s + 3] === 0) {
                const bit = y * maskStride + (x >> 3);
                mask[bit] |= 0x80 >> (x & 7);
            }
        }
    }
    return Buffer.concat([header, xor, mask]);
}

export function buildIco(pngs) {
    const images = ICO_SIZES.map((size) => ({
        size,
        isPng: size >= ICO_PNG_FROM,
        data: size >= ICO_PNG_FROM ? pngs.get(size) : dibEntry(pngs.get(size)),
    }));

    const dir = Buffer.alloc(6 + 16 * images.length);
    dir.writeUInt16LE(0, 0);                 // reserved
    dir.writeUInt16LE(1, 2);                 // type: icon
    dir.writeUInt16LE(images.length, 4);
    let offset = dir.length;
    images.forEach((img, i) => {
        const e = 6 + 16 * i;
        dir.writeUInt8(img.size >= 256 ? 0 : img.size, e + 0);
        dir.writeUInt8(img.size >= 256 ? 0 : img.size, e + 1);
        dir.writeUInt8(0, e + 2);            // palette entries
        dir.writeUInt8(0, e + 3);            // reserved
        dir.writeUInt16LE(1, e + 4);         // planes
        dir.writeUInt16LE(32, e + 6);        // bit count
        dir.writeUInt32LE(img.data.length, e + 8);
        dir.writeUInt32LE(offset, e + 12);
        offset += img.data.length;
    });
    return Buffer.concat([dir, ...images.map((i) => i.data)]);
}

// ---- ICNS ------------------------------------------------------------------

// OSType -> pixel size of the PNG payload.
export const ICNS_CHUNKS = [
    ['ic07', 128],   // 128x128
    ['ic08', 256],   // 256x256
    ['ic09', 512],   // 512x512
    ['ic10', 1024],  // 512x512@2x
    ['ic11', 32],    // 16x16@2x
    ['ic12', 64],    // 32x32@2x
    ['ic13', 256],   // 128x128@2x
    ['ic14', 512],   // 256x256@2x
];

export function buildIcns(pngs) {
    const chunks = ICNS_CHUNKS.map(([type, size]) => {
        const payload = pngs.get(size);
        const head = Buffer.alloc(8);
        head.write(type, 0, 4, 'ascii');
        head.writeUInt32BE(8 + payload.length, 4);
        return Buffer.concat([head, payload]);
    });
    const body = Buffer.concat(chunks);
    const head = Buffer.alloc(8);
    head.write('icns', 0, 4, 'ascii');
    head.writeUInt32BE(8 + body.length, 4);
    return Buffer.concat([head, body]);
}

// ---- read-back verification ------------------------------------------------

export function verifyIco(buf) {
    if (buf.readUInt16LE(0) !== 0 || buf.readUInt16LE(2) !== 1) throw new Error('bad ICO header');
    const count = buf.readUInt16LE(4);
    console.log(`  ICO: ${count} entries, ${buf.length} bytes`);
    for (let i = 0; i < count; i++) {
        const e = 6 + 16 * i;
        const w = buf.readUInt8(e) || 256;
        const h = buf.readUInt8(e + 1) || 256;
        const bytes = buf.readUInt32LE(e + 8);
        const off = buf.readUInt32LE(e + 12);
        const data = buf.subarray(off, off + bytes);
        const isPng = data.readUInt32BE(0) === 0x89504e47;
        let real;
        if (isPng) {
            const p = pngInfo(data);
            real = `${p.width}x${p.height}`;
        } else {
            real = `${data.readInt32LE(4)}x${data.readInt32LE(8) / 2}`;
        }
        if (real !== `${w}x${h}`) throw new Error(`ICO entry ${i}: dir says ${w}x${h}, payload is ${real}`);
        if (off + bytes > buf.length) throw new Error(`ICO entry ${i} runs past EOF`);
        console.log(`    ${String(w).padStart(4)}x${String(h).padEnd(4)} ${isPng ? 'PNG ' : 'BMP '} ${String(bytes).padStart(8)} bytes @ ${off}`);
    }
}

export function verifyIcns(buf) {
    if (buf.toString('ascii', 0, 4) !== 'icns') throw new Error('bad ICNS magic');
    const total = buf.readUInt32BE(4);
    if (total !== buf.length) throw new Error(`ICNS length ${total} != file size ${buf.length}`);
    console.log(`  ICNS: ${buf.length} bytes`);
    let off = 8;
    while (off < buf.length) {
        const type = buf.toString('ascii', off, off + 4);
        const len = buf.readUInt32BE(off + 4);
        const payload = buf.subarray(off + 8, off + len);
        const p = pngInfo(payload);
        const want = ICNS_CHUNKS.find(([t]) => t === type)?.[1];
        if (p.width !== want || p.height !== want) {
            throw new Error(`ICNS ${type}: expected ${want}px, payload is ${p.width}x${p.height}`);
        }
        console.log(`    ${type} ${String(p.width)}x${p.height} ${String(len - 8).padStart(8)} bytes`);
        off += len;
    }
}

// ---- main ------------------------------------------------------------------

/**
 * Renders every size and writes the three containers.
 *
 * `options.repo` moves the whole read/write tree (the tests point it at a temp
 * dir) and `options.Resvg` substitutes the rasterizer; with neither, this is
 * exactly what `node dev/tools/brand/build-icons.mjs` does against the repo.
 */
export async function main(options = {}) {
    const repo = options.repo || REPO;
    const Resvg = options.Resvg || (await import('@resvg/resvg-js')).Resvg;

    const master = resolve(repo, 'resources/brand/astrofin-icon.svg');
    const masterFlat = resolve(repo, 'resources/brand/astrofin-icon-flat.svg');
    const outIco = resolve(repo, 'resources/win/astrofin.ico');
    const outIcns = resolve(repo, 'resources/macos/AppIcon.icns');
    const outLinuxSvg = resolve(repo, 'resources/linux/io.github.thehalfrican.Astrofin.svg');

    const svgs = { full: readFileSync(master), flat: readFileSync(masterFlat) };

    const pngs = new Map();
    for (const size of SIZES) {
        const png = renderPng(Resvg, svgs, size);
        pngs.set(size, png);
        console.log(`rendered ${String(size).padStart(4)}px ${size < FLAT_BELOW ? '(flat)' : '(full)'} -> ${png.length} bytes`);
    }

    for (const p of [outIco, outIcns, outLinuxSvg]) mkdirSync(dirname(p), { recursive: true });

    const ico = buildIco(pngs);
    writeFileSync(outIco, ico);
    const icns = buildIcns(pngs);
    writeFileSync(outIcns, icns);
    writeFileSync(outLinuxSvg, svgs.full);

    console.log('\nverifying:');
    verifyIco(readFileSync(outIco));
    verifyIcns(readFileSync(outIcns));

    // The Linux icon must also survive a resvg parse at the size the shell uses.
    const linuxCheck = new Resvg(readFileSync(outLinuxSvg), { fitTo: { mode: 'width', value: 256 } }).render();
    console.log(`  linux SVG: parses, renders ${linuxCheck.width}x${linuxCheck.height}`);

    console.log('\nok');
    return { ico: outIco, icns: outIcns, svg: outLinuxSvg };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    await main();
}
