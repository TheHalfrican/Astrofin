// Harness setup: everything the scenarios need that is not in the repo.
//
//   1. a pinned jellyfin-web build, downloaded once into .cache/e2e/ and
//      verified against the SHA256 in dev/e2e/jellyfin-web.json;
//   2. a short video fixture (lib/fixtures.mjs `CLIP_SECONDS`), encoded once
//      by the mpv built from the submodule, so the harness needs no ffmpeg of
//      its own.
//
// Both steps are idempotent: a second run with the artifacts already in place
// does nothing. `node dev/e2e/setup.mjs` runs them standalone; run.mjs calls
// `prepare()` before the scenarios.

import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawnSync } from 'node:child_process';

import { CACHE_DIR, CLIP_PATH, E2E_DIR, BUILD_DIR, WEB_ROOT, ensureDir, findMpvCli, MPV_CLI_CANDIDATES, IS_WINDOWS } from './lib/paths.mjs';
import { CLIP_SECONDS } from './lib/fixtures.mjs';

const PIN = JSON.parse(fs.readFileSync(path.join(E2E_DIR, 'jellyfin-web.json'), 'utf8'));

/** bsdtar, which reads both `ar` (.deb) and `.tar.xz`. Windows ships it. */
const TAR = IS_WINDOWS ? path.join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'tar.exe') : 'bsdtar';

function log(msg) {
    process.stdout.write(`[e2e setup] ${msg}\n`);
}

function sha256(file) {
    return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

function run(cmd, args, opts = {}) {
    const r = spawnSync(cmd, args, { stdio: 'pipe', encoding: 'utf8', ...opts });
    if (r.error) throw r.error;
    if (r.status !== 0) {
        throw new Error(`${path.basename(cmd)} ${args.join(' ')} exited ${r.status}\n${r.stderr ?? ''}`);
    }
    return r;
}

async function download(url, dest) {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`GET ${url} -> ${res.status} ${res.statusText}`);
    const buf = Buffer.from(await res.arrayBuffer());
    fs.writeFileSync(dest, buf);
    return dest;
}

/**
 * Download + unpack the pinned jellyfin-web into WEB_ROOT. Returns the path.
 * A `version.txt` stamp beside the tree makes the check a single stat once
 * the tree is in place.
 */
export async function prepareJellyfinWeb({ printSha = false } = {}) {
    ensureDir(CACHE_DIR);
    const stamp = path.join(WEB_ROOT, '.astrofin-e2e-version');
    if (!printSha && fs.existsSync(stamp) && fs.readFileSync(stamp, 'utf8').trim() === PIN.version) {
        return WEB_ROOT;
    }

    const deb = path.join(CACHE_DIR, `jellyfin-web_${PIN.version}.deb`);
    if (!fs.existsSync(deb)) {
        log(`downloading jellyfin-web ${PIN.version} (~32 MB)`);
        await download(PIN.url, deb);
    }

    const digest = sha256(deb);
    if (printSha) {
        process.stdout.write(`${digest}\n`);
        return WEB_ROOT;
    }
    if (digest !== PIN.sha256) {
        fs.rmSync(deb, { force: true });
        throw new Error(`jellyfin-web package digest mismatch: got ${digest}, expected ${PIN.sha256}`);
    }

    log(`unpacking jellyfin-web ${PIN.version}`);
    const work = path.join(CACHE_DIR, 'unpack');
    fs.rmSync(work, { recursive: true, force: true });
    ensureDir(work);
    // .deb is an `ar` archive holding data.tar.xz; bsdtar reads both.
    run(TAR, ['-xf', deb, '-C', work]);
    const dataTar = fs.readdirSync(work).find((f) => f.startsWith('data.tar'));
    if (!dataTar) throw new Error(`no data.tar.* inside ${deb}`);
    const data = path.join(work, 'data');
    ensureDir(data);
    run(TAR, ['-xf', path.join(work, dataTar), '-C', data]);

    const src = path.join(data, ...PIN.webRootInPackage.split('/'));
    if (!fs.existsSync(path.join(src, 'index.html'))) {
        throw new Error(`no index.html under ${src}`);
    }
    fs.rmSync(WEB_ROOT, { recursive: true, force: true });
    fs.renameSync(src, WEB_ROOT);
    fs.rmSync(work, { recursive: true, force: true });
    fs.writeFileSync(stamp, `${PIN.version}\n`);
    return WEB_ROOT;
}

/**
 * Encode the media fixture with the bundled mpv: `CLIP_SECONDS` of lavfi
 * testsrc2 plus a 440 Hz sine, h264/aac in mp4 so the client direct-plays it.
 * mpv needs the staged build/ tree on PATH for its FFmpeg DLLs. The length is
 * the one the mock advertises as the item's runtime, so the two must agree —
 * both read `CLIP_SECONDS` from lib/fixtures.mjs.
 */
export function prepareClip() {
    ensureDir(CACHE_DIR);
    // The stamp records the length the existing clip was cut to, so changing
    // CLIP_SECONDS re-encodes instead of silently disagreeing with the mock.
    const stamp = `${CLIP_PATH}.seconds`;
    const want = String(CLIP_SECONDS);
    if (fs.existsSync(CLIP_PATH) && fs.existsSync(stamp) && fs.readFileSync(stamp, 'utf8').trim() === want) {
        return CLIP_PATH;
    }
    const mpv = findMpvCli();
    if (!mpv) {
        throw new Error(
            `no mpv CLI to encode the media fixture with; looked in\n  ${MPV_CLI_CANDIDATES.join('\n  ')}\n` +
                'The submodule build produces it (`just build`). On Windows a cached\n' +
                'third_party/mpv-install makes that step skip the compile, so a fresh\n' +
                'checkout may need `dev/windows/build_mpv_source.ps1 -Arch x64 -Force`.\n' +
                'Once encoded the clip is cached in .cache/e2e and no mpv is needed again.',
        );
    }
    log(`encoding the ${CLIP_SECONDS} s media fixture with the bundled mpv`);
    const src = CLIP_SECONDS + 1;
    const graph =
        `av://lavfi:testsrc2=size=640x360:rate=24:duration=${src}[out0];` +
        `sine=frequency=440:sample_rate=48000:duration=${src}[out1]`;
    const sep = IS_WINDOWS ? ';' : ':';
    run(
        mpv,
        [
            '--no-config',
            graph,
            `--length=${CLIP_SECONDS}`,
            `--o=${CLIP_PATH}`,
            '--of=mp4',
            '--ovc=libx264',
            '--ovcopts=preset=ultrafast,crf=30',
            '--oac=aac',
            '--really-quiet',
        ],
        { env: { ...process.env, PATH: `${BUILD_DIR}${sep}${process.env.PATH ?? ''}` } },
    );
    if (!fs.existsSync(CLIP_PATH)) throw new Error('mpv reported success but wrote no clip');
    fs.writeFileSync(stamp, `${want}\n`);
    return CLIP_PATH;
}

export async function prepare(opts = {}) {
    await prepareJellyfinWeb(opts);
    prepareClip();
    return { webRoot: WEB_ROOT, clip: CLIP_PATH, jellyfinWebVersion: PIN.version };
}

export const PINNED = PIN;

if (import.meta.url === `file://${process.argv[1]}` || process.argv[1]?.endsWith('setup.mjs')) {
    const printSha = process.argv.includes('--print-sha256');
    prepare({ printSha }).then(
        (r) => !printSha && log(`ready: jellyfin-web ${r.jellyfinWebVersion}, clip ${r.clip}`),
        (e) => {
            process.stderr.write(`[e2e setup] ${e.message}\n`);
            process.exit(1);
        },
    );
}
