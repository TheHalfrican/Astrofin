// Scenario 2: connect overlay -> mock server -> sign in -> Home rendered.

import test from 'node:test';
import assert from 'node:assert/strict';

import { readSettings } from '../lib/app.mjs';
import { withApp } from '../lib/harness.mjs';
import { connectToServer, signIn, waitForHome } from '../lib/flow.mjs';
import * as fx from '../lib/fixtures.mjs';

test('the connect overlay probes the server, saves it and loads jellyfin-web', async () => {
    await withApp(async ({ app, mock }) => {
        const { main } = await connectToServer(app, mock.origin);

        // Two-phase probe: HEAD to resolve the base URL, GET to confirm it is
        // a Jellyfin server (src/jfn_cef/src/business_overlay.rs).
        assert.ok(
            mock.requests.some((r) => r.method === 'HEAD'),
            'the overlay never sent the HEAD phase of the probe',
        );
        assert.ok(
            mock.find(/^\/System\/Info\/Public$/i).length > 0,
            'the overlay never fetched /System/Info/Public',
        );

        assert.ok(main.url.startsWith(mock.origin), `main layer went to ${main.url}`);
        assert.equal(readSettings(app.profile).serverUrl, mock.origin, 'the server URL was not persisted');

        // jellyfin-web really booted: it routed itself to sign-in and put the
        // mock's server name in the title (both come from its own startup, so
        // they only settle once it has talked to the server).
        await main.waitForExpression('location.hash.includes("login")', {
            message: 'jellyfin-web to route itself to sign-in',
            timeoutMs: 60_000,
        });
        await main.waitForExpression(`document.title === ${JSON.stringify(fx.SERVER_NAME)}`, {
            message: "the page title to become the mock's server name",
            timeoutMs: 30_000,
        });
    });
});

test('signing in renders Home with the fixture library and no page errors', async () => {
    await withApp(async ({ app, mock }) => {
        const { main } = await connectToServer(app, mock.origin);

        // Record every page-level error from before the sign-in onwards.
        await main.evaluate(`(() => {
            window.__e2eErrors = [];
            addEventListener('error', (e) => window.__e2eErrors.push(String(e.message)));
            addEventListener('unhandledrejection', (e) => window.__e2eErrors.push('rejection: ' + e.reason));
            return true;
        })()`);

        await signIn(main);
        await waitForHome(main);

        const auth = mock.find(/AuthenticateByName/i);
        assert.equal(auth.length, 1, 'expected exactly one authentication attempt');
        assert.equal(JSON.parse(auth[0].body).Username, fx.USERNAME);

        assert.ok(mock.find(/UserViews/i).length > 0, 'the client never asked for the user views');
        assert.ok(mock.capabilities, 'the client never posted /Sessions/Capabilities/Full');
        assert.ok(mock.socketCount >= 1, 'the client never opened the /socket WebSocket');

        assert.deepEqual(await main.evaluate('window.__e2eErrors'), [], 'jellyfin-web reported page errors');
        assert.deepEqual(mock.unhandled.map((r) => `${r.method} ${r.path}`), [], 'the mock saw unmodelled requests');
    });
});
