// Unit tests for the connect screen's connectivity probe. Run with:
//
//     node --test src/web/connectivityHelper.test.js
//
// (or `just test-js`). The module is one promise bridged across two worlds:
// `jmpCheckServerConnectivity(url)` asks native to probe and parks the
// resolve/reject pair, and `_onServerConnectivityResult(url, ok, resolved)` —
// called from Rust — settles it. What is worth testing is the parking: the
// url guard that keeps a stale reply from settling a newer probe, the abort
// path (both while a probe is outstanding and while nothing is), and the
// startup race where the script is injected before `jmpNative` exists.
const test = require('node:test');
const assert = require('node:assert');

const { makeUiWindow, loadModule } = require('./test/ui-fakes.js');

// Drain the microtask queue and fire any timer that is due, `steps` times.
// The module's wait loop is `await setTimeout(100)`, so one step per attempt.
async function settle(win, steps = 3) {
    for (let i = 0; i < steps; i++) {
        await new Promise((r) => setImmediate(r));
        win.timers.advance(100);
    }
    await new Promise((r) => setImmediate(r));
}

// `native: false` loads the module with no `jmpNative` at all (the injection
// race); otherwise the recorder is in place before the script runs.
function load({ native = true } = {}) {
    const win = makeUiWindow();
    if (native) win.jmpNative = makeNative();
    const check = loadModule('connectivityHelper.js', win);
    return { win, check, result: win._onServerConnectivityResult };
}

function makeNative() {
    const calls = [];
    return {
        calls,
        checkServerConnectivity(url) { calls.push(['check', url]); },
        cancelServerConnectivity() { calls.push(['cancel']); }
    };
}

test('a successful native result resolves with the url native normalised', async () => {
    const { win, check, result } = load();
    const pending = check('jellyfin.example');
    await settle(win);
    assert.deepStrictEqual(win.jmpNative.calls, [['check', 'jellyfin.example']]);

    // Native answers with the resolved URL, which is what the caller saves.
    result('jellyfin.example', true, 'https://jellyfin.example:8920');
    assert.strictEqual(await pending, 'https://jellyfin.example:8920');
});

test('a failed native result rejects with a connection error', async () => {
    const { win, check, result } = load();
    const pending = check('http://nope.invalid');
    await settle(win);
    result('http://nope.invalid', false, '');
    await assert.rejects(pending, /Connection failed/);
});

test('a result for a url we are no longer probing is ignored', async () => {
    const { win, check, result } = load();
    let settled = false;
    const pending = check('http://a.example');
    pending.then(() => { settled = true; }, () => { settled = true; });
    await settle(win);

    // A reply for the previous server must not settle the current probe.
    result('http://old.example', true, 'http://old.example');
    await settle(win);
    assert.strictEqual(settled, false);

    result('http://a.example', true, 'http://a.example');
    assert.strictEqual(await pending, 'http://a.example');
});

test('a second result for the same url after settling is ignored', async () => {
    const { win, check, result } = load();
    const pending = check('http://a.example');
    await settle(win);
    result('http://a.example', true, 'http://a.example');
    await pending;
    // The pending state was cleared, so the duplicate finds nothing to do.
    assert.doesNotThrow(() => result('http://a.example', false, ''));
});

test('a result while nothing is pending is ignored, whatever url it carries', () => {
    // `pendingUrl` starts as null, so a null-url result matches on the url
    // alone; without an outstanding probe there is no resolver to call.
    const { result } = load();
    assert.doesNotThrow(() => result(null, true, 'http://x.example'));
    assert.doesNotThrow(() => result(null, false, ''));
    assert.doesNotThrow(() => result('http://a.example', true, 'http://a.example'));
});

test('abort cancels the native probe and rejects the outstanding promise', async () => {
    const { win, check } = load();
    const pending = check('http://a.example');
    await settle(win);
    check.abort();
    await assert.rejects(pending, /Connection cancelled/);
    assert.deepStrictEqual(win.jmpNative.calls.at(-1), ['cancel']);
});

test('abort with nothing outstanding still tells native to cancel', () => {
    const { win, check } = load();
    assert.doesNotThrow(() => check.abort());
    assert.deepStrictEqual(win.jmpNative.calls, [['cancel']]);
});

test('a result arriving after abort no longer settles anything', async () => {
    const { win, check, result } = load();
    const pending = check('http://a.example');
    await settle(win);
    check.abort();
    await assert.rejects(pending, /Connection cancelled/);
    // Native's in-flight reply races the cancel; it must be a no-op.
    assert.doesNotThrow(() => result('http://a.example', true, 'http://a.example'));
});

test('abort works when native has no cancel entry point', async () => {
    const { win, check } = load();
    const pending = check('http://a.example');
    await settle(win);
    delete win.jmpNative.cancelServerConnectivity;
    check.abort();
    await assert.rejects(pending, /Connection cancelled/);
});

test('a probe started before jmpNative exists waits for it', async () => {
    const { win, check } = load({ native: false });
    const pending = check('http://a.example');
    // Three failed polls, then native arrives.
    await settle(win, 3);
    win.jmpNative = makeNative();
    await settle(win, 2);
    assert.deepStrictEqual(win.jmpNative.calls, [['check', 'http://a.example']]);
    win._onServerConnectivityResult('http://a.example', true, 'http://a.example/');
    assert.strictEqual(await pending, 'http://a.example/');
});

test('a probe gives up once native has not appeared in 50 polls', async () => {
    const { win, check } = load({ native: false });
    // The assertion is attached before the clock runs: the rejection lands
    // inside `settle`, and an unhandled one fails the whole file.
    const rejected = assert.rejects(check('http://a.example'),
        /Native connectivity check not available/);
    await settle(win, 55);
    await rejected;
});

test('the module installs both halves of the bridge on window', () => {
    const { win, check } = load();
    assert.strictEqual(typeof win.jmpCheckServerConnectivity, 'function');
    assert.strictEqual(win.jmpCheckServerConnectivity, check);
    assert.strictEqual(typeof check.abort, 'function');
    assert.strictEqual(typeof win._onServerConnectivityResult, 'function');
    assert.strictEqual(check.onNotice, null, 'no hook until the screen sets one');
});

// ---- native's fourth slot -------------------------------------------------
//
// Native answers `serverConnectivityResult` with a fourth string: a notice key
// on success, a failure message on failure. Both shapes the owner requires to
// keep working (a raw IP:port and a tailnet name, both plain http) come back
// through this path, so they are what the tests probe with.

const IP_PORT = 'http://192.168.1.10:8096';
const TAILNET = 'http://thehalfrican-truenas.tail1cdca8.ts.net:8096';

test('a notice on a successful probe reaches the hook, not the promise', async () => {
    const { win, check, result } = load();
    const seen = [];
    check.onNotice = (key) => seen.push(key);

    // Bare IP:port, https attempt failed, native connected over plain http.
    const pending = check('192.168.1.10:8096');
    await settle(win);
    result('192.168.1.10:8096', true, IP_PORT, 'insecure-http');

    assert.strictEqual(await pending, IP_PORT, 'the resolve value is still the url');
    assert.deepStrictEqual(seen, ['insecure-http']);
});

test('a tailnet address over plain http resolves exactly as it always did', async () => {
    const { win, check, result } = load();
    const seen = [];
    check.onNotice = (key) => seen.push(key);

    // A typed `http://` scheme: native never tries https, so no notice.
    const pending = check(TAILNET);
    await settle(win);
    result(TAILNET, true, TAILNET, '');

    assert.strictEqual(await pending, TAILNET);
    assert.deepStrictEqual(seen, [], 'a typed scheme is the user’s own choice');
});

test('a success with no hook installed still resolves', async () => {
    const { win, check, result } = load();
    const pending = check('192.168.1.10:8096');
    await settle(win);
    assert.doesNotThrow(() => result('192.168.1.10:8096', true, IP_PORT, 'insecure-http'));
    assert.strictEqual(await pending, IP_PORT);
});

test('a failure detail from native becomes the rejection message', async () => {
    const { win, check, result } = load();
    const pending = check('192.168.1.10:8096');
    await settle(win);
    result('192.168.1.10:8096', false, '192.168.1.10:8096',
        'server redirected to https://jf.example.com; enter that address instead');
    const err = await pending.then(() => null, (e) => e);
    assert.match(err.message, /server redirected to https:\/\/jf\.example\.com/);
    assert.strictEqual(err.detail, err.message, 'a native reason is flagged as one');
});

test('a failure with no detail keeps the generic message and no detail flag', async () => {
    const { win, check, result } = load();
    const pending = check(IP_PORT);
    await settle(win);
    result(IP_PORT, false, IP_PORT, '');
    const err = await pending.then(() => null, (e) => e);
    assert.strictEqual(err.message, 'Connection failed');
    assert.strictEqual(err.detail, undefined);
});

test('a cancellation is flagged so it is never mistaken for a native reason', async () => {
    const { win, check } = load();
    const pending = check(IP_PORT);
    await settle(win);
    check.abort();
    const err = await pending.then(() => null, (e) => e);
    assert.strictEqual(err.cancelled, true);
    assert.strictEqual(err.detail, undefined);
});
