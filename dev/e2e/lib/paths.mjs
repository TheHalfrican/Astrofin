// Repo-relative locations the E2E harness needs. Everything derives from the
// location of this file, so the harness runs the same from any cwd.

import { fileURLToPath } from 'node:url';
import path from 'node:path';
import fs from 'node:fs';

export const E2E_DIR = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
export const REPO_ROOT = path.dirname(path.dirname(E2E_DIR));

/** Staged runnable tree produced by `just build`. */
export const BUILD_DIR = path.join(REPO_ROOT, 'build');

/** Gitignored scratch space shared with the CEF download cache. */
export const CACHE_DIR = path.join(REPO_ROOT, '.cache', 'e2e');

export const IS_WINDOWS = process.platform === 'win32';

export const APP_EXE = path.join(BUILD_DIR, IS_WINDOWS ? 'astrofin.exe' : 'astrofin');

/**
 * Where the mpv built from the submodule may be found, best first. On Windows
 * `mpv.com` is the console-subsystem wrapper that writes to stdout; `mpv.exe`
 * is the GUI one. `build/mpv-build/` is where the staged tree puts it (the
 * justfile's `run-mpv` recipe); `third_party/mpv/build/` is meson's own
 * output directory.
 */
export const MPV_CLI_CANDIDATES = (
    IS_WINDOWS
        ? [
              ['build', 'mpv-build', 'mpv.com'],
              ['build', 'mpv-build', 'mpv.exe'],
              ['third_party', 'mpv', 'build', 'mpv.com'],
              ['third_party', 'mpv', 'build', 'mpv.exe'],
          ]
        : [
              ['build', 'mpv-build', 'mpv'],
              ['third_party', 'mpv', 'build', 'mpv'],
          ]
).map((parts) => path.join(REPO_ROOT, ...parts));

/** The first mpv CLI that exists, or null when the submodule was never built. */
export function findMpvCli() {
    return MPV_CLI_CANDIDATES.find((p) => fs.existsSync(p)) ?? null;
}

/** Where the unpacked jellyfin-web build lands (see setup.mjs). */
export const WEB_ROOT = path.join(CACHE_DIR, 'jellyfin-web');

/** The generated media fixture the mock server streams. */
export const CLIP_PATH = path.join(CACHE_DIR, 'clip.mp4');

export function ensureDir(dir) {
    fs.mkdirSync(dir, { recursive: true });
    return dir;
}

/** Throw with an actionable message when the staged build is missing. */
export function requireBuild() {
    if (!fs.existsSync(APP_EXE)) {
        throw new Error(
            `staged build not found at ${APP_EXE}\n` +
                'Run `just build` (or dev/windows/build.ps1) before the E2E suite.',
        );
    }
    return APP_EXE;
}
