// Scenario 4: a settings write survives the round trip through settings.json.
//
// The write goes through the same bridge the client-settings UI uses
// (`window.api.settings.setValue` -> `jmpNative.setSettingValue` ->
// `business_common::apply_setting_value` -> jfn_config), and the read back
// happens in a second process, from the blob the native shim injects.

import test from 'node:test';
import assert from 'node:assert/strict';

import { launchApp, readSettings, removeProfile } from '../lib/app.mjs';
import { waitFor } from '../lib/cdp.mjs';
import { startMock } from '../lib/mock-server.mjs';
import { bootToHome } from '../lib/flow.mjs';

test('a setting written from the page is persisted and read back on the next launch', async () => {
    const mock = await startMock();
    let profile;
    const first = await launchApp({ tag: 'astrofin-e2e-settings' });
    try {
        profile = first.profile;
        await first.waitForDebugger();
        await first.waitForLayers();
        const { main } = await bootToHome(first, mock.origin);

        // Sanity: the shim starts from the defaults.
        assert.equal(await main.evaluate('window.jmpInfo.settings.playback.transcodeNotice'), 'cpu');
        assert.equal(await main.evaluate('window.jmpInfo.settings.advanced.hideScrollbar'), true);

        await main.evaluate(`(() => {
            window.api.settings.setValue('playback', 'transcodeNotice', 'any');
            window.api.settings.setValue('advanced', 'hideScrollbar', false);
            return true;
        })()`);

        // settings_save_async writes on a worker thread; poll the file.
        const saved = await waitFor(
            () => {
                const s = readSettings(profile);
                return s.transcodeNotice === 'any' && s.hideScrollbar === false ? s : null;
            },
            { message: 'settings.json to pick up both writes', timeoutMs: 20_000 },
        );
        assert.equal(saved.serverUrl, mock.origin, 'the saved server URL was lost');
    } finally {
        await first.dispose({ keepProfile: true });
    }

    // Second process, same profile: the values must come back through the
    // settings blob the native shim is built from.
    const second = await launchApp({ profile, tag: 'astrofin-e2e-settings' });
    try {
        await second.waitForDebugger();
        await second.waitForLayers();
        const main = await second.mainSession((t) => t.url.startsWith(mock.origin), { timeoutMs: 60_000 });
        assert.equal(await main.evaluate('window.jmpInfo.settings.playback.transcodeNotice'), 'any');
        assert.equal(await main.evaluate('window.jmpInfo.settings.advanced.hideScrollbar'), false);
        // The saved server is what put the main layer on the mock without the
        // overlay being driven this time.
        assert.match(main.url, new RegExp(`^${mock.origin.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`));
    } finally {
        // `second` did not mint the profile, so it will not remove it.
        await second.dispose();
        await removeProfile(profile);
        await mock.stop();
    }
});

test('an unknown setting key is refused and leaves settings.json alone', async () => {
    const mock = await startMock();
    const app = await launchApp({ tag: 'astrofin-e2e-settings-bad' });
    try {
        await app.waitForDebugger();
        await app.waitForLayers();
        const { main } = await bootToHome(app, mock.origin);
        const before = JSON.stringify(readSettings(app.profile));

        await main.evaluate(`(window.api.settings.setValue('advanced', 'notASetting', 'x'), true)`);
        await waitFor(() => app.readLog().includes('Unknown setting key: advanced.notASetting'), {
            message: 'the app to log the rejected key',
            timeoutMs: 15_000,
        });
        assert.equal(JSON.stringify(readSettings(app.profile)), before, 'settings.json changed');
    } finally {
        await app.dispose();
        await mock.stop();
    }
});
