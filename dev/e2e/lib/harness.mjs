// Per-scenario setup/teardown.
//
// Only one app instance may run at a time (the app is single-instance per
// config dir, and each one owns an mpv window and a GPU context), so every
// scenario file runs serially and disposes its instance in a `finally`.

import { launchApp } from './app.mjs';
import { startMock } from './mock-server.mjs';

/**
 * Start a mock server and an app instance, hand both to `fn`, then tear them
 * down whatever happens. On failure the app's ERROR log lines are appended to
 * the thrown error, because a scenario that fails on a UI assertion usually
 * failed for a reason the app already wrote down.
 */
export async function withApp(fn, opts = {}) {
    const mock = opts.mock === false ? null : await startMock(opts.mockOptions);
    const app = await launchApp(opts.appOptions);
    try {
        await app.waitForDebugger(opts.bootTimeoutMs ?? 90_000);
        await app.waitForLayers(opts.bootTimeoutMs ?? 90_000);
        return await fn({ app, mock });
    } catch (e) {
        const errors = app.errorLines().slice(-8);
        if (errors.length) e.message += `\n--- app ERROR lines ---\n${errors.join('\n')}`;
        throw e;
    } finally {
        await app.dispose({ keepProfile: opts.keepProfile });
        await mock?.stop();
    }
}
