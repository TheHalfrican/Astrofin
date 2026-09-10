// Unit tests for MpvPlayerBase, the half of the two jellyfin-web player
// plugins that is the same for audio and video. Run with:
//
//     node --test src/web/mpv-player-base.test.js
//
// (or `just test-js`). The class is a thin translator in both directions, so
// what is worth testing is exactly the translation: ticks to milliseconds and
// a nine-slot loadfile on the way down to mpv, and mpv's signals turned into
// the jellyfin-web events (`pause`, `unpause`, `playing`, `stopped`, `error`,
// `volumechange`) on the way back up. The state it keeps — paused, duration,
// volume, rate, the current source — is only ever an echo of what mpv last
// said, so every test drives it through a signal rather than setting a field.
//
// The fake window, the fake `window.api.player` and the jellyfin-web fakes all
// come from test/player-fakes.js; nothing here touches a profile dir, a
// server or the app.
const test = require('node:test');
const assert = require('node:assert');
const {
    loadModule,
    makeWindow,
    makeApiPlayer,
    makeJmpNative,
    makePlayerArgs,
    makeAppSettings
} = require('./test/player-fakes.js');

// A fresh window, a fresh copy of the module and one player on top of it.
// `extra` is merged into the constructor arguments (an `appSettings` with a
// saved volume, an `appHost` that answers a profile, ...).
function setup(extra) {
    const win = makeWindow();
    win.jmpNative = makeJmpNative();
    win.api = { player: makeApiPlayer(win) };
    const MpvPlayerBase = loadModule('mpv-player-base.js', win);
    const args = makePlayerArgs(win, extra);
    const player = new MpvPlayerBase(args);
    // Subclasses set this; the base only ever logs it.
    player.logTag = 'Base';
    return {
        win,
        MpvPlayerBase,
        player,
        args,
        api: win.api.player,
        events: args.events,
        appSettings: args.appSettings
    };
}

// The signal names the base connects to, in the order connectSignals uses.
const SIGNALS = ['playing', 'positionUpdate', 'seeking', 'finished', 'updateDuration', 'error', 'paused'];

function handlerCounts(api) {
    return SIGNALS.map((name) => api[name].handlers.length);
}

// ---- construction ---------------------------------------------------------

test('a new player starts at the saved volume without saving it again', () => {
    const { player, api, appSettings, events } = setup({
        appSettings: makeAppSettings({ volume: 0.5 })
    });
    assert.equal(player.getVolume(), 50);
    assert.deepEqual(api.lastCall('setVolume'), [50]);
    // Restoring is not a change: the stored value is untouched and the UI is
    // not told the volume moved.
    assert.equal(appSettings.store.volume, 0.5);
    assert.deepEqual(events.names(), []);
});

test('a player with nothing saved starts at full volume', () => {
    const { player } = setup({ appSettings: makeAppSettings({}) });
    assert.equal(player.getVolume(), 100);
});

test('a player that was left silent starts silent', () => {
    const { player, api } = setup({ appSettings: makeAppSettings({ volume: 0 }) });
    assert.equal(player.getVolume(), 0);
    assert.deepEqual(api.lastCall('setVolume'), [0]);
});

test('a new player is not paused, has no source and no duration', () => {
    const { player } = setup();
    assert.equal(player.paused(), false);
    assert.equal(player.currentSrc(), null);
    assert.equal(player.duration(), null);
    assert.equal(player.isMuted(), false);
    assert.equal(player.type, 'mediaplayer');
});

// ---- signal wiring --------------------------------------------------------

test('connecting the mpv signals twice still leaves one handler each', () => {
    const { player, api } = setup();
    player.connectSignals();
    assert.deepEqual(handlerCounts(api), SIGNALS.map(() => 1));
    player.connectSignals();
    assert.deepEqual(handlerCounts(api), SIGNALS.map(() => 1));
});

test('disconnecting removes every handler, and is safe when never connected', () => {
    const { player, api } = setup();
    player.disconnectSignals(); // never connected: a no-op, not a throw
    assert.deepEqual(handlerCounts(api), SIGNALS.map(() => 0));
    player.connectSignals();
    player.disconnectSignals();
    assert.deepEqual(handlerCounts(api), SIGNALS.map(() => 0));
    // ...and reconnecting works, because the guard was cleared with it.
    player.connectSignals();
    assert.deepEqual(handlerCounts(api), SIGNALS.map(() => 1));
});

test('mpv seeking marks the player as seeking', () => {
    const { player, api } = setup();
    player.connectSignals();
    assert.equal(player._seeking, false);
    api.seeking.emit();
    assert.equal(player._seeking, true);
});

test('mpv pausing marks the player paused and tells jellyfin-web', () => {
    const { player, api, events } = setup();
    player.connectSignals();
    api.paused.emit();
    assert.equal(player.paused(), true);
    assert.deepEqual(events.names(), ['pause']);
});

test('mpv reporting a duration makes the player seekable', () => {
    const { player, api } = setup();
    player.connectSignals();
    assert.equal(player.seekable(), false);
    api.updateDuration.emit(1_800_000);
    assert.equal(player.duration(), 1_800_000);
    assert.equal(player.seekable(), true);
});

test('a duration of zero is reported as no duration at all', () => {
    const { player, api } = setup();
    player.connectSignals();
    api.updateDuration.emit(0);
    assert.equal(player.duration(), null);
    assert.equal(player.seekable(), false);
});

test('an mpv error becomes a media decode error for jellyfin-web', () => {
    const { player, api, events, win } = setup();
    player.connectSignals();
    api.error.emit('demuxer failed');
    assert.deepEqual(events.triggered.at(-1).name, 'error');
    assert.deepEqual(events.triggered.at(-1).args, [{ type: 'mediadecodeerror' }]);
    // The raw error is logged, not swallowed.
    assert.ok(win.console.lines.some((l) => l.level === 'error'));
});

test('mpv finishing the file ends playback', () => {
    const { player, api, events } = setup();
    player.connectSignals();
    player._currentSrc = 'http://server/video.mkv';
    api.finished.emit();
    assert.equal(events.triggered.at(-1).name, 'stopped');
});

// ---- end of playback ------------------------------------------------------

test('ending playback reports the source that stopped and then forgets it', async () => {
    const { player, events, api } = setup();
    api.autoResolveLoad = true;
    await player.setCurrentSrc({ url: 'http://server/video.mkv' });
    player.onEndedInternal();
    assert.deepEqual(events.triggered.at(-1), {
        obj: player,
        name: 'stopped',
        args: [{ src: 'http://server/video.mkv' }]
    });
    assert.equal(player.currentSrc(), null);
    assert.equal(player.currentTime(), null);
    assert.equal(player._currentPlayOptions, null);
});

// ---- Playback Info panel --------------------------------------------------

test('the stats panel is empty when the native stats module is not loaded', async () => {
    const { player } = setup();
    assert.deepEqual(await player.getStats(), { categories: [] });
});

test('asking for stats also asks the native side to keep observing', async () => {
    const { player, win } = setup();
    const asked = [];
    win._mpvStats = { fps: 23.976 };
    win.AstrofinMpvStats = {
        requestStats() { asked.push('requestStats'); },
        buildCategories(snapshot) { return [{ stats: [snapshot] }]; }
    };
    const stats = await player.getStats();
    assert.deepEqual(asked, ['requestStats']);
    assert.deepEqual(stats, { categories: [{ stats: [{ fps: 23.976 }] }] });
});

// ---- device profile -------------------------------------------------------

test('the device profile comes from the app host, item and options included', async () => {
    const seen = [];
    const appHost = {
        getDeviceProfile(item, options) {
            seen.push([item, options]);
            return Promise.resolve({ Name: 'Astrofin' });
        }
    };
    const { player } = setup({ appHost });
    const profile = await player.getDeviceProfile({ Id: 'i1' }, { isRetry: false });
    assert.deepEqual(profile, { Name: 'Astrofin' });
    assert.deepEqual(seen, [[{ Id: 'i1' }, { isRetry: false }]]);
});

test('without an app host the device profile is empty rather than a throw', async () => {
    const { player } = setup({ appHost: null });
    assert.deepEqual(await player.getDeviceProfile({}, {}), {});
});

// ---- track defaults -------------------------------------------------------

test('the base class plays one audio track with video and subtitles disabled', () => {
    const { player, MpvPlayerBase } = setup();
    assert.equal(MpvPlayerBase.TRACK_DISABLE, 0);
    assert.deepEqual(player._resolveTracks({}), {
        videoParam: MpvPlayerBase.TRACK_DISABLE,
        audioParam: 1,
        subParam: MpvPlayerBase.TRACK_DISABLE,
        externalAudioUrl: null,
        externalSubUrl: null
    });
    assert.equal(player.mediaType, null);
});

test('the base class asks mpv for nothing before a load', () => {
    const { player, api } = setup();
    const before = api.calls.length;
    player._beforeLoad({ aspectRatio: 'cover' });
    assert.equal(api.calls.length, before);
});

// ---- starting a file ------------------------------------------------------

test('starting a file hands mpv the url, the start position and the tracks', async () => {
    const { player, api, win } = setup();
    api.autoResolveLoad = true;
    await player.setCurrentSrc({
        url: 'http://server/audio.flac',
        playerStartPositionTicks: 30_000_000,
        item: { Id: 'i1', Name: 'Track' }
    });
    assert.equal(player.currentSrc(), 'http://server/audio.flac');
    assert.equal(player.currentTime(), 3000);
    const load = api.lastLoad;
    assert.equal(load.url, 'http://server/audio.flac');
    assert.deepEqual(load.options, {
        startMilliseconds: 3000,
        autoplay: true,
        isInfiniteStream: false
    });
    assert.deepEqual(load.streamdata, { type: null, metadata: { Id: 'i1', Name: 'Track' } });
    assert.equal(load.audioStream, 1);
    assert.equal(load.videoStream, 0);
    assert.equal(load.subtitleStream, 0);
    // ...and that is what reaches the native nine-slot playerLoad.
    assert.deepEqual(win.jmpNative.lastCall('playerLoad'), [
        'http://server/audio.flac', 3000, 0, 1, 0,
        JSON.stringify({ Id: 'i1', Name: 'Track' }), '', '', false
    ]);
});

test('a start position of no ticks starts at zero', async () => {
    const { player, api } = setup();
    api.autoResolveLoad = true;
    await player.setCurrentSrc({ url: 'http://server/a.flac' });
    assert.equal(api.lastLoad.options.startMilliseconds, 0);
    assert.equal(player.currentTime(), 0);
});

test('a start position in ticks is rounded to whole milliseconds', async () => {
    const { player, api } = setup();
    api.autoResolveLoad = true;
    // 1234.5678 ms — mpv is given an integer, not a fraction.
    await player.setCurrentSrc({ url: 'http://server/a.flac', playerStartPositionTicks: 12_345_678 });
    assert.equal(api.lastLoad.options.startMilliseconds, 1235);
});

test('a live stream is flagged as infinite', async () => {
    const { player, api } = setup();
    api.autoResolveLoad = true;
    await player.setCurrentSrc({
        url: 'http://server/live.m3u8',
        mediaSource: { IsInfiniteStream: true }
    });
    assert.equal(api.lastLoad.options.isInfiniteStream, true);
});

test('starting a file only resolves once mpv has the file open', async () => {
    const { player, api } = setup();
    let resolved = false;
    const started = player.setCurrentSrc({ url: 'http://server/a.flac' }).then(() => { resolved = true; });
    await Promise.resolve();
    assert.equal(resolved, false, 'still waiting for mpv');
    api.loadCallback();
    await started;
    assert.equal(resolved, true);
});

// ---- play / pause ---------------------------------------------------------

test('playing after a pause fires unpause before playing', () => {
    const { player, events } = setup();
    player._paused = true;
    player._emitPlaying();
    assert.deepEqual(events.names(), ['unpause', 'playing']);
    assert.equal(player.paused(), false);
});

test('playing that was never paused fires playing alone', () => {
    const { player, events } = setup();
    player._emitPlaying();
    assert.deepEqual(events.names(), ['playing']);
});

test('pause and resume are passed straight to mpv', () => {
    const { player, api } = setup();
    player._paused = true;
    player.pause();
    assert.deepEqual(api.names().slice(-1), ['pause']);
    player.resume();
    assert.deepEqual(api.names().slice(-1), ['play']);
    // resume() is the one place the flag is cleared locally, so a resume
    // followed by a pause request is not swallowed as a no-op.
    assert.equal(player.paused(), false);
});

test('unpause asks mpv to play and waits for mpv to confirm the state', () => {
    const { player, api } = setup();
    player._paused = true;
    player.unpause();
    assert.deepEqual(api.names().slice(-1), ['play']);
    // mpv is the authority: until its `playing` signal arrives the player
    // still reports paused.
    assert.equal(player.paused(), true);
    player.connectSignals();
    player._emitPlaying();
    assert.equal(player.paused(), false);
});

// ---- position -------------------------------------------------------------

test('setting the current time seeks mpv there', () => {
    const { player, api } = setup();
    assert.equal(player.currentTime(90_000), undefined);
    assert.deepEqual(api.lastCall('seekTo'), [90_000]);
    assert.equal(player.currentTime(), 90_000);
});

test('seeking back to the very start is a seek, not a read', () => {
    const { player, api } = setup();
    player.currentTime(0);
    assert.deepEqual(api.lastCall('seekTo'), [0]);
    assert.equal(player.currentTime(), 0);
});

test('the async position comes from mpv rather than the cached value', async () => {
    const { player, api } = setup();
    player._currentTime = 1000;
    api.position = 4242;
    assert.equal(await player.currentTimeAsync(), 4242);
});

test('buffered ranges are whatever the native side last pushed', () => {
    const { player, win } = setup();
    assert.deepEqual(player.getBufferedRanges(), []);
    win._bufferedRanges = [{ start: 0, end: 12 }];
    assert.deepEqual(player.getBufferedRanges(), [{ start: 0, end: 12 }]);
});

// ---- playback rate --------------------------------------------------------

test('the playback rate reaches mpv in thousandths, because the ipc is integer', () => {
    const { player, api } = setup();
    player.setPlaybackRate(1.25);
    assert.deepEqual(api.lastCall('setPlaybackRate'), [1250]);
    assert.equal(player.getPlaybackRate(), 1.25);
});

test('a rate of zero reads back as normal speed', () => {
    const { player } = setup();
    player.setPlaybackRate(0);
    assert.equal(player.getPlaybackRate(), 1);
});

test('the supported rates run from half to quadruple speed and are labelled', () => {
    const { player } = setup();
    const rates = player.getSupportedPlaybackRates();
    assert.equal(rates.length, 11);
    assert.deepEqual(rates[0], { name: '0.5x', id: 0.5 });
    assert.deepEqual(rates[2], { name: '1x', id: 1.0 });
    assert.deepEqual(rates.at(-1), { name: '4x', id: 4.0 });
});

// ---- volume ---------------------------------------------------------------

test('saving a volume stores it as a fraction', () => {
    const { player, appSettings } = setup();
    player.saveVolume(0.4);
    assert.equal(appSettings.store.volume, 0.4);
});

test('saving a volume of zero stores silence rather than keeping the old one', () => {
    const { player, appSettings } = setup({ appSettings: makeAppSettings({ volume: 0.4 }) });
    player.saveVolume(0);
    assert.equal(appSettings.store.volume, 0);
});

test('saving something that is not a volume leaves the stored one alone', () => {
    const { player, appSettings } = setup({ appSettings: makeAppSettings({ volume: 0.4 }) });
    for (const bogus of [null, undefined, '', 'loud', NaN, Infinity]) {
        player.saveVolume(bogus);
        assert.equal(appSettings.store.volume, 0.4, String(bogus));
    }
});

test('the saved volume defaults to full when the setting is missing', () => {
    const { player } = setup({ appSettings: makeAppSettings({}) });
    assert.equal(player.getSavedVolume(), 1);
});

test('a saved volume of zero comes back as silence, not as full volume', () => {
    const { player } = setup({ appSettings: makeAppSettings({ volume: 0 }) });
    assert.equal(player.getSavedVolume(), 0);
});

test('changing the volume tells mpv, saves it and announces the change', () => {
    const { player, api, appSettings, events } = setup();
    player.setVolume(40);
    assert.equal(player.getVolume(), 40);
    assert.deepEqual(api.lastCall('setVolume'), [40]);
    assert.equal(appSettings.store.volume, 0.4);
    assert.deepEqual(events.names(), ['volumechange']);
});

test('a volume given as a string is still a number to mpv', () => {
    const { player, api } = setup();
    player.setVolume('35');
    assert.strictEqual(player.getVolume(), 35);
    assert.deepEqual(api.lastCall('setVolume'), [35]);
});

test('a volume that is not a number is ignored entirely', () => {
    const { player, api, events } = setup();
    const before = api.callsTo('setVolume').length;
    player.setVolume('loud');
    assert.equal(player.getVolume(), 100, 'the old volume survives');
    assert.equal(api.callsTo('setVolume').length, before, 'mpv is not told');
    assert.deepEqual(events.names(), []);
});

test('muting by dragging the volume to zero stores silence', () => {
    // Zero is a volume, not a missing one: it has to survive to the next
    // start rather than being read back as "unset" and restored at full.
    const { player, api, appSettings } = setup();
    player.setVolume(0);
    assert.equal(player.getVolume(), 0);
    assert.deepEqual(api.lastCall('setVolume'), [0]);
    assert.equal(appSettings.store.volume, 0);
});

test('the volume steps up by two and stops at full', () => {
    const { player } = setup({ appSettings: makeAppSettings({ volume: 0.97 }) });
    player.volumeUp();
    assert.equal(player.getVolume(), 99);
    player.volumeUp();
    assert.equal(player.getVolume(), 100);
    player.volumeUp();
    assert.equal(player.getVolume(), 100);
});

test('the volume steps down by two and stops at silence', () => {
    const { player } = setup({ appSettings: makeAppSettings({ volume: 0.03 }) });
    player.volumeDown();
    assert.equal(player.getVolume(), 1);
    player.volumeDown();
    assert.equal(player.getVolume(), 0);
    player.volumeDown();
    assert.equal(player.getVolume(), 0);
});

test('muting tells mpv and announces a volume change', () => {
    const { player, api, events } = setup();
    player.setMute(true);
    assert.equal(player.isMuted(), true);
    assert.deepEqual(api.lastCall('setMuted'), [true]);
    assert.deepEqual(events.names(), ['volumechange']);
    player.setMute(false);
    assert.equal(player.isMuted(), false);
    assert.deepEqual(api.lastCall('setMuted'), [false]);
});

test('muting can be done quietly, without an event', () => {
    const { player, api, events } = setup();
    player.setMute(true, false);
    assert.equal(player.isMuted(), true);
    assert.deepEqual(api.lastCall('setMuted'), [true]);
    assert.deepEqual(events.names(), []);
});
