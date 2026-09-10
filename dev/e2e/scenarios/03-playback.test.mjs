// Scenario 3: start playback -> mpv reports playing -> pause -> seek -> stop.
//
// Every assertion about playback state reads what mpv pushed up to the page
// (`window._mpvVideoPlayerInstance`, fed by `_nativeUpdatePosition` and the
// `paused` signal) or what the client reported to the server, never a timer.

import test from 'node:test';
import assert from 'node:assert/strict';

import { AUDIO_ENABLED } from '../lib/app.mjs';
import { waitFor } from '../lib/cdp.mjs';
import { withApp } from '../lib/harness.mjs';
import { bootToHome, playFixtureItem, playerState, waitForPositionPast } from '../lib/flow.mjs';
import * as fx from '../lib/fixtures.mjs';

test('playing the fixture item drives mpv through play, pause, seek and stop', async () => {
    await withApp(async ({ app, mock }) => {
        const { main } = await bootToHome(app, mock.origin);
        await playFixtureItem(main);

        // ---- playing ----
        const playing = await waitForPositionPast(main, 400, { timeoutMs: 45_000 });
        assert.ok(playing.src.includes(`/Videos/${fx.ITEM_ID}/stream`), `unexpected media URL ${playing.src}`);
        assert.equal(playing.paused, false);
        assert.ok(
            Math.abs(playing.duration - fx.CLIP_SECONDS * 1000) < 1500,
            `mpv reported duration ${playing.duration} ms for a ${fx.CLIP_SECONDS} s clip`,
        );

        // The negotiation really went through the server, and direct play won.
        const info = mock.find(/PlaybackInfo/i);
        assert.equal(info.length, 1, 'expected exactly one /PlaybackInfo negotiation');
        await waitFor(() => mock.reported('start'), { message: 'a /Sessions/Playing report' });
        assert.equal(mock.lastReport('start').info.PlayMethod, 'DirectPlay');
        assert.ok(mock.find(/\/Videos\/.*\/stream/i).length > 0, 'mpv never fetched the media');

        // The suite is silent unless E2E_AUDIO=1; prove it rather than assume.
        const ao = /AO: \[(\w+)\]/.exec(app.readLog());
        assert.ok(ao, 'mpv never logged which audio output it opened');
        if (!AUDIO_ENABLED) {
            assert.equal(ao[1], 'null', `mpv opened the ${ao[1]} audio device; the suite must stay silent`);
        }

        // ---- pause ----
        await main.evaluate('window._mpvVideoPlayerInstance.pause(), true');
        const paused = await waitFor(
            async () => {
                const s = await playerState(main);
                return s?.paused ? s : null;
            },
            { message: 'mpv to report the pause', timeoutMs: 20_000 },
        );
        await waitFor(() => mock.playbackReports.some((r) => r.kind === 'progress' && r.info.IsPaused === true), {
            message: 'a paused progress report',
            timeoutMs: 20_000,
        });

        // A paused player must not advance.
        const frozen = await playerState(main);
        assert.equal(frozen.currentTime, paused.currentTime, 'position moved while paused');

        // ---- seek ----
        const target = Math.round(fx.CLIP_SECONDS * 1000 * 0.4);
        await main.evaluate(`window._mpvVideoPlayerInstance.currentTime(${target}), true`);
        await main.evaluate('window._mpvVideoPlayerInstance.unpause(), true');
        const seeked = await waitFor(
            async () => {
                const s = await playerState(main);
                return s && !s.paused && s.currentTime > target ? s : null;
            },
            { message: `mpv to resume past the ${target} ms seek target`, timeoutMs: 30_000 },
        );
        assert.ok(seeked.currentTime < target + 8000, `seek overshot to ${seeked.currentTime} ms`);

        // ---- stop ----
        await main.evaluate('window._mpvVideoPlayerInstance.stop(), true');
        await waitFor(() => mock.reported('stopped'), { message: 'a /Sessions/Playing/Stopped report', timeoutMs: 30_000 });
        const stoppedAt = mock.lastReport('stopped').info.PositionTicks / 10_000;
        assert.ok(stoppedAt > target, `the stop was reported at ${stoppedAt} ms, before the seek target`);
        await waitFor(
            async () => {
                const s = await playerState(main);
                return s === null || s.src === null;
            },
            { message: 'the player to drop its media', timeoutMs: 20_000 },
        );

        assert.deepEqual(mock.unhandled.map((r) => `${r.method} ${r.path}`), [], 'the mock saw unmodelled requests');
    });
});
