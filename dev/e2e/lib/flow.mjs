// The user journeys the scenarios share, expressed against the real UI.
//
// Nothing here reaches into jellyfin-web internals: the connect overlay is
// driven through its own form, and login goes through the manual sign-in form,
// so a change that breaks the app's UI breaks these too — which is the point.

import { delay, waitFor } from './cdp.mjs';
import * as fx from './fixtures.mjs';

/**
 * Type `origin` into the connect overlay and press Connect. Resolves with the
 * main-layer session once it has landed on the server.
 */
export async function connectToServer(app, origin, opts = {}) {
    const overlay = await app.overlaySession(opts);
    await overlay.waitForExpression('!!document.getElementById("address") && document.body.dataset.state === "idle"', {
        timeoutMs: opts.timeoutMs ?? 60_000,
        message: 'the connect overlay form',
    });
    await overlay.evaluate(`(() => {
        const address = document.getElementById('address');
        address.value = ${JSON.stringify(origin)};
        address.dispatchEvent(new Event('input', { bubbles: true }));
        document.getElementById('connect-button').click();
        return true;
    })()`);
    const main = await app.mainSession((t) => t.url.startsWith(origin), {
        timeoutMs: opts.timeoutMs ?? 60_000,
        message: 'the main layer to land on the mock server',
    });
    return { overlay, main };
}

/** Wait for jellyfin-web's sign-in form and submit the fixture credentials. */
export async function signIn(main, { username = fx.USERNAME, password = fx.PASSWORD, timeoutMs = 60_000 } = {}) {
    await main.waitForExpression('!!document.querySelector("#txtManualName") && !!document.querySelector("#txtManualPassword")', {
        timeoutMs,
        message: 'the jellyfin-web sign-in form',
    });
    const result = await main.evaluate(`(() => {
        const name = document.querySelector('#txtManualName');
        const pw = document.querySelector('#txtManualPassword');
        const set = (el, v) => {
            el.value = v;
            el.dispatchEvent(new Event('input', { bubbles: true }));
            el.dispatchEvent(new Event('change', { bubbles: true }));
        };
        set(name, ${JSON.stringify(username)});
        set(pw, ${JSON.stringify(password)});
        name.closest('form').dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
        return true;
    })()`);
    return result;
}

/** Resolve once jellyfin-web has rendered Home with the fixture library on it. */
export async function waitForHome(main, { timeoutMs = 60_000 } = {}) {
    await main.waitForExpression(
        `location.hash.startsWith('#/home') && document.body.innerText.includes(${JSON.stringify('E2E Movies')})`,
        { timeoutMs, message: 'the Home screen with the fixture library' },
    );
    return true;
}

/** connect -> sign in -> Home, the preamble most scenarios need. */
export async function bootToHome(app, origin, opts = {}) {
    const { overlay, main } = await connectToServer(app, origin, opts);
    await signIn(main, opts);
    await waitForHome(main, opts);
    return { overlay, main };
}

/** Open the fixture item's details page and press its primary Play button. */
export async function playFixtureItem(main, { timeoutMs = 60_000 } = {}) {
    await main.evaluate(
        `location.hash = ${JSON.stringify(`#/details?id=${fx.ITEM_ID}&serverId=${fx.SERVER_ID}`)}, true`,
    );
    await main.waitForExpression(
        `document.body.innerText.includes(${JSON.stringify(fx.ITEM_NAME)}) && !!document.querySelector('.btnPlay:not(.hide), button[data-action="resume"], .mainDetailButtons button')`,
        { timeoutMs, message: "the item's details page" },
    );
    // Let the details page finish binding its click handlers.
    await delay(500);
    const clicked = await main.evaluate(`(() => {
        const btn = document.querySelector('.mainDetailButtons .btnPlay:not(.hide)')
            || document.querySelector('.btnPlay:not(.hide)')
            || document.querySelector('button[data-action="resume"]');
        if (!btn) return false;
        btn.click();
        return true;
    })()`);
    if (!clicked) throw new Error('no Play button on the details page');
    return true;
}

/** The live mpvVideoPlayer instance's view of playback, or null. */
export function playerStateExpression() {
    return `(() => {
        const p = window._mpvVideoPlayerInstance;
        if (!p) return null;
        return {
            src: p.currentSrc(),
            paused: p.paused(),
            currentTime: p.currentTime(),
            duration: p.duration(),
        };
    })()`;
}

/** Read the player state pushed up from mpv; null before playback starts. */
export function playerState(main) {
    return main.evaluate(playerStateExpression());
}

/** Wait until mpv has pushed a position past `ms`. */
export function waitForPositionPast(main, ms, opts = {}) {
    return waitFor(
        async () => {
            const s = await playerState(main);
            return s && s.currentTime != null && s.currentTime > ms ? s : null;
        },
        { message: `mpv to report a position past ${ms} ms`, timeoutMs: 30_000, ...opts },
    );
}
