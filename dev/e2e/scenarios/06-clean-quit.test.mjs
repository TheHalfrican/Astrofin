// Scenario 6: the whole journey, then a clean quit with no ERROR log lines.
//
// This is the scenario that covers the code the unit suite exempts: CEF and
// mpv boot, the browser layers, the GPU paint path, the media session and the
// shutdown sequence all have to work for it to pass, and anything that logs at
// ERROR along the way fails it.

import test from 'node:test';
import assert from 'node:assert/strict';

import { launchApp } from '../lib/app.mjs';
import { waitFor } from '../lib/cdp.mjs';
import { startMock } from '../lib/mock-server.mjs';
import { bootToHome, playFixtureItem, waitForPositionPast } from '../lib/flow.mjs';

test('connect, sign in, play, stop and quit without a single ERROR log line', async () => {
    const mock = await startMock();
    const app = await launchApp({ tag: 'astrofin-e2e-quit' });
    let log = '';
    let exit = null;
    try {
        await app.waitForDebugger();
        await app.waitForLayers();

        const { main } = await bootToHome(app, mock.origin);
        await playFixtureItem(main);
        await waitForPositionPast(main, 400, { timeoutMs: 45_000 });
        await main.evaluate('window._mpvVideoPlayerInstance.stop(), true');
        await waitFor(() => mock.reported('stopped'), { message: 'the stop report', timeoutMs: 30_000 });

        // Quit the way the UI's "Exit Application" does.
        exit = await app.requestExit();
        log = app.readLog();
    } finally {
        await app.dispose();
        await mock.stop();
    }

    assert.ok(exit, 'the app did not exit after jmpNative.appExit()');
    assert.equal(exit.code, 0, `the app exited with code ${exit.code}`);

    const errors = log.split(/\r?\n/).filter((l) => /\bERROR\b/.test(l));
    assert.deepEqual(errors, [], `the run log has ERROR lines:\n${errors.join('\n')}`);

    // The shutdown really ran rather than the process being killed under us.
    assert.match(log, /shutdown|Exiting|quit/i, 'the run log shows no shutdown at all');
});
