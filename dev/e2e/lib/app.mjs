// Launching, driving and reliably killing the staged astrofin build.
//
// Every launch gets its own throwaway profile directory used for BOTH
// --config-dir and --cache-dir, so the harness never reads or writes the
// user's real %APPDATA%\astrofin / %LOCALAPPDATA%\astrofin. The ASTROFIN_*
// environment variables are cleared in the child so a developer shell that
// exports them cannot redirect a test run at the real profile.

import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

import { APP_EXE, IS_WINDOWS, requireBuild } from './paths.mjs';
import { attachToTarget, debuggerVersion, delay, listTargets, waitFor } from './cdp.mjs';

/** ASTROFIN_* variables that would otherwise leak the developer's profile in. */
const SCRUBBED_ENV = [
    'ASTROFIN_CONFIG_DIR',
    'ASTROFIN_CACHE_DIR',
    'ASTROFIN_LOG_FILE',
    'ASTROFIN_LOG_LEVEL',
    'JELLIUM_CONFIG_DIR',
    'JELLIUM_CACHE_DIR',
];

/** An OS-assigned free TCP port, released before the app claims it. */
export function freePort() {
    return new Promise((resolve, reject) => {
        const srv = net.createServer();
        srv.once('error', reject);
        srv.listen(0, '127.0.0.1', () => {
            const { port } = srv.address();
            srv.close(() => resolve(port));
        });
    });
}

/** A fresh profile root under the OS temp dir. */
export function makeProfile(tag = 'astrofin-e2e') {
    return fs.mkdtempSync(path.join(os.tmpdir(), `${tag}-`));
}

/** Set E2E_AUDIO=1 to hear the fixture clip; the suite is silent otherwise. */
export const AUDIO_ENABLED = process.env.E2E_AUDIO === '1';

/**
 * Silence the run by writing an mpv.conf into the throwaway profile.
 *
 * `jfn_paths::mpv_home()` is `<config-dir>/mpv`, and the app boots libmpv with
 * `config=yes`, so mpv reads this file. `ao=null` is the lever that actually
 * holds: mpv's volume and mute are driven from jellyfin-web's saved volume the
 * moment a player is created, so a `volume=0` alone would be undone. The null
 * audio output still consumes samples in real time, so playback timing — which
 * is what the scenarios assert on — is unchanged.
 */
export function writeMpvConf(profile, { audio = AUDIO_ENABLED } = {}) {
    const dir = path.join(profile, 'mpv');
    fs.mkdirSync(dir, { recursive: true });
    const lines = ['# written by dev/e2e — do not edit, the harness rewrites it per launch'];
    if (!audio) lines.push('ao=null', 'volume=0', 'mute=yes');
    fs.writeFileSync(path.join(dir, 'mpv.conf'), `${lines.join('\n')}\n`);
    return path.join(dir, 'mpv.conf');
}

function childEnv(extra = {}) {
    const env = { ...process.env, ...extra };
    for (const k of SCRUBBED_ENV) delete env[k];
    return env;
}

/**
 * Delete a throwaway profile, retrying while CEF still holds its cache dir
 * open. Scenarios that hand the same profile to a second `launchApp` own the
 * cleanup themselves — `dispose()` only removes a profile it minted.
 */
export async function removeProfile(dir, { attempts = 10 } = {}) {
    for (let i = 0; i < attempts; i++) {
        try {
            fs.rmSync(dir, { recursive: true, force: true });
            return true;
        } catch {
            await delay(200);
        }
    }
    return false;
}

/** Force-kill a process and everything it spawned (CEF is multi-process). */
export function killTree(pid) {
    if (!pid) return;
    if (IS_WINDOWS) {
        spawnSync('taskkill', ['/PID', String(pid), '/T', '/F'], { stdio: 'ignore' });
    } else {
        try {
            process.kill(-pid, 'SIGKILL');
        } catch {
            try {
                process.kill(pid, 'SIGKILL');
            } catch {
                /* already gone */
            }
        }
    }
}

/**
 * A single app instance: its process, its throwaway profile, its log file and
 * its CDP port. Always create it with `launchApp` and always `dispose()` it
 * from a `finally` — leaving an instance running breaks every later scenario
 * (the app is single-instance per config dir).
 */
export class AppInstance {
    constructor({ child, profile, logFile, port, args, ownsProfile }) {
        this.child = child;
        this.profile = profile;
        this.logFile = logFile;
        this.port = port;
        this.args = args;
        this.ownsProfile = ownsProfile;
        this.stdout = '';
        this.stderr = '';
        this.exitCode = null;
        this.exitSignal = null;
        this.sessions = new Set();

        child.stdout?.setEncoding('utf8');
        child.stderr?.setEncoding('utf8');
        child.stdout?.on('data', (d) => (this.stdout += d));
        child.stderr?.on('data', (d) => (this.stderr += d));
        this.exited = new Promise((resolve) => {
            child.on('exit', (code, signal) => {
                this.exitCode = code;
                this.exitSignal = signal;
                resolve({ code, signal });
            });
        });
    }

    get pid() {
        return this.child.pid;
    }

    get running() {
        return this.exitCode === null && this.exitSignal === null;
    }

    /** Contents of the run log so far ('' before the first flush). */
    readLog() {
        try {
            return fs.readFileSync(this.logFile, 'utf8');
        } catch {
            return '';
        }
    }

    /** Log lines at ERROR level, which the clean-quit scenario asserts on. */
    errorLines() {
        return this.readLog()
            .split(/\r?\n/)
            .filter((l) => /\bERROR\b/.test(l));
    }

    /** Wait for the remote debugger to answer; proves CEF finished booting. */
    async waitForDebugger(timeoutMs = 60_000) {
        return waitFor(() => debuggerVersion(this.port), {
            timeoutMs,
            message: `the CEF remote debugger on port ${this.port}`,
        });
    }

    /**
     * Wait until CEF has created both browser layers. The debug port answers
     * as soon as CEF's network service is up, which is well before the app
     * has any browsers, so a bare `waitForDebugger` proves too little.
     */
    async waitForLayers(timeoutMs = 90_000) {
        return waitFor(
            async () => {
                const targets = await listTargets(this.port);
                const pages = targets.filter((t) => t.type === 'page');
                const overlay = pages.some((t) => t.url?.startsWith('app://resources/overlay.html'));
                const main = pages.some((t) => !t.url?.startsWith('app://'));
                return overlay && main ? targets : null;
            },
            { timeoutMs, message: 'CEF to create the overlay and main browser layers' },
        );
    }

    /** Attach to the never-navigated connect overlay layer. */
    async overlaySession(opts = {}) {
        return this.#track(
            await attachToTarget(this.port, (t) => t.url?.startsWith('app://resources/overlay.html'), {
                message: 'the connect overlay target',
                ...opts,
            }),
        );
    }

    /**
     * Attach to the main web layer — the one that carries the native bridge.
     * `predicate` narrows it further (e.g. "already on the mock server").
     */
    async mainSession(predicate = () => true, opts = {}) {
        return this.#track(
            await attachToTarget(
                this.port,
                (t) => !t.url?.startsWith('app://resources/overlay.html') && t.type === 'page' && predicate(t),
                { message: 'the main web layer target', ...opts },
            ),
        );
    }

    #track(session) {
        this.sessions.add(session);
        return session;
    }

    /** Ask the app to shut itself down the way the UI's Quit does. */
    async requestExit() {
        const main = await this.mainSession();
        // The reply never arrives: the browser tears down mid-call.
        main.evaluate('window.jmpNative.appExit(), true').catch(() => {});
        return this.waitForExit(30_000);
    }

    async waitForExit(timeoutMs = 20_000) {
        const timer = new Promise((resolve) => setTimeout(() => resolve('timeout'), timeoutMs).unref?.());
        const r = await Promise.race([this.exited, timer]);
        return r === 'timeout' ? null : r;
    }

    /** Idempotent teardown. Safe to call twice; safe to call after a crash. */
    async dispose({ keepProfile = false } = {}) {
        for (const s of this.sessions) s.close();
        this.sessions.clear();
        if (this.running) {
            killTree(this.pid);
            await this.waitForExit(10_000);
        }
        if (this.ownsProfile && !keepProfile) await removeProfile(this.profile);
    }
}

/**
 * Spawn the staged build with a throwaway profile.
 *
 * @param {object} opts
 * @param {string} [opts.profile]   reuse an existing profile dir (second-instance tests)
 * @param {string[]} [opts.extraArgs] appended verbatim
 * @param {string} [opts.logLevel]  --log-level (default `debug`, as `just run`)
 * @param {number} [opts.port]      --remote-debug-port (default: a free one)
 * @param {boolean} [opts.audio]    let mpv open a real audio device (default: E2E_AUDIO=1)
 */
export async function launchApp(opts = {}) {
    requireBuild();
    const ownsProfile = !opts.profile;
    const profile = opts.profile ?? makeProfile(opts.tag);
    const logFile = opts.logFile ?? path.join(profile, 'run.log');
    const port = opts.port ?? (await freePort());
    writeMpvConf(profile, { audio: opts.audio ?? AUDIO_ENABLED });
    const args = [
        '--config-dir',
        profile,
        '--cache-dir',
        path.join(profile, 'cache'),
        '--log-file',
        logFile,
        '--log-level',
        opts.logLevel ?? 'debug',
        '--remote-debug-port',
        String(port),
        ...(opts.extraArgs ?? []),
    ];
    const child = spawn(APP_EXE, args, {
        cwd: path.dirname(APP_EXE),
        env: childEnv(opts.env),
        stdio: ['ignore', 'pipe', 'pipe'],
        detached: !IS_WINDOWS,
        windowsHide: false,
    });
    return new AppInstance({ child, profile, logFile, port, args, ownsProfile });
}

/** `astrofin --version`, run to completion. Never boots CEF or mpv's VO. */
export function runVersionProbe() {
    requireBuild();
    return spawnSync(APP_EXE, ['--version'], {
        cwd: path.dirname(APP_EXE),
        env: childEnv(),
        encoding: 'utf8',
        timeout: 60_000,
    });
}

/** Seed a profile's settings.json before the app first reads it. */
export function seedSettings(profile, settings) {
    fs.mkdirSync(profile, { recursive: true });
    fs.writeFileSync(path.join(profile, 'settings.json'), JSON.stringify(settings, null, 2));
    return path.join(profile, 'settings.json');
}

/** Read a profile's settings.json back ({} when it has not been written). */
export function readSettings(profile) {
    try {
        return JSON.parse(fs.readFileSync(path.join(profile, 'settings.json'), 'utf8'));
    } catch {
        return {};
    }
}
