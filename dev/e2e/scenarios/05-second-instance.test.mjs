// Scenario 5: a second launch against the same profile hands off and exits.
//
// The instance identity lives in `<config-dir>/instance.json` and names a
// per-instance pipe (src/instance_ipc). A second process that finds the pipe
// taken pings the owner and exits 0 without booting CEF or mpv.

import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';

import { launchApp, removeProfile } from '../lib/app.mjs';

test('a second launch on the same profile signals the first and exits', async () => {
    const first = await launchApp({ tag: 'astrofin-e2e-instance' });
    try {
        await first.waitForDebugger();
        await first.waitForLayers();
        assert.ok(
            fs.existsSync(path.join(first.profile, 'instance.json')),
            'the first instance did not mint an instance.json',
        );

        const secondLog = path.join(first.profile, 'second.log');
        const second = await launchApp({
            profile: first.profile,
            logFile: secondLog,
            tag: 'astrofin-e2e-instance',
        });
        try {
            const exit = await second.waitForExit(60_000);
            assert.ok(exit, 'the second instance never exited');
            assert.equal(exit.code, 0, 'the second instance exited non-zero');
            assert.match(
                fs.readFileSync(secondLog, 'utf8'),
                /Signaled existing instance, exiting/,
                'the second instance did not report a hand-off',
            );
            assert.ok(!/could not signal existing instance/.test(fs.readFileSync(secondLog, 'utf8')));
        } finally {
            await second.dispose({ keepProfile: true });
        }

        // The owner is untouched: still running, still debuggable.
        assert.ok(first.running, 'the first instance died when the second one launched');
        await first.waitForDebugger(10_000);
    } finally {
        await first.dispose();
    }
});

test('the same profile can be claimed again once the owner is gone', async () => {
    const first = await launchApp({ tag: 'astrofin-e2e-instance-reclaim' });
    const profile = first.profile;
    await first.waitForDebugger();
    await first.waitForLayers();
    await first.dispose({ keepProfile: true });

    const second = await launchApp({ profile, tag: 'astrofin-e2e-instance-reclaim' });
    try {
        // A stale pipe name must not be mistaken for a live owner: the second
        // launch has to come all the way up rather than hand off and exit.
        await second.waitForDebugger(90_000);
        await second.waitForLayers();
        assert.ok(second.running, 'the relaunch exited instead of taking ownership');
        assert.doesNotMatch(
            second.readLog(),
            /Signaled existing instance/,
            'the relaunch handed off to an owner that was already gone',
        );
    } finally {
        // `second` inherited the profile from `first`, so neither disposal
        // removes it — this test owns the cleanup.
        await second.dispose();
        await removeProfile(profile);
    }
});
