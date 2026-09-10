// Scenario 1: the app starts, reports its versions and comes up debuggable.

import test from 'node:test';
import assert from 'node:assert/strict';

import { runVersionProbe } from '../lib/app.mjs';
import { listTargets, debuggerVersion } from '../lib/cdp.mjs';
import { withApp } from '../lib/harness.mjs';

test('--version prints the app, CEF and mpv versions and exits 0', () => {
    const r = runVersionProbe();
    assert.equal(r.status, 0, `astrofin --version exited ${r.status}: ${r.stderr}`);
    assert.match(r.stdout, /^astrofin \d+\.\d+\.\d+/, 'no astrofin version line');
    assert.match(r.stdout, /^CEF \d+\.\d+\.\d+/m, 'no CEF version line');
    assert.match(r.stdout, /^mpv-version mpv v\d/m, 'no mpv version line');
    assert.match(r.stdout, /^ffmpeg-version /m, 'no FFmpeg version line');
});

test('a launch with a throwaway profile boots CEF, mpv and both browser layers', async () => {
    await withApp(
        async ({ app }) => {
            const version = await debuggerVersion(app.port);
            assert.match(version.Browser ?? '', /Chrome/i, 'the debugger did not identify as Chrome');

            const targets = await listTargets(app.port);
            const urls = targets.map((t) => t.url);
            assert.ok(
                urls.some((u) => u.startsWith('app://resources/overlay.html')),
                `no connect overlay layer among ${JSON.stringify(urls)}`,
            );
            assert.ok(
                urls.some((u) => !u.startsWith('app://')),
                `no main web layer among ${JSON.stringify(urls)}`,
            );

            // The main layer is the one that carries the JS -> Rust bridge,
            // even before it has navigated anywhere.
            const main = await app.mainSession();
            assert.equal(await main.evaluate('typeof window.jmpNative'), 'object');
            assert.equal(await main.evaluate('typeof window.jmpNative.appExit'), 'function');

            const log = app.readLog();
            assert.match(log, /astrofin \d+\.\d+\.\d+/, 'the run log has no version banner');
            assert.match(log, /mpv-version|mpv v\d/, 'the run log never mentions mpv');
        },
        { mock: false },
    );
});
