// Unit tests for the mpvAudioPlayer plugin. Run with:
//
//     node --test src/web/mpv-audio-player.test.js
//
// (or `just test-js`). Everything shared with the video player lives in
// MpvPlayerBase and is tested in mpv-player-base.test.js; what is specific
// here is the music side of the contract: the track defaults that keep an
// mp3's embedded cover art from ending the file, the volume and rate the
// first `playing` signal re-applies to each new file, and `stop(true)`'s
// fade-out — a real timer loop, driven here by the fake clock in
// test/player-fakes.js so no test waits on wall time.
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

function setup(extra) {
    const win = makeWindow();
    win.jmpNative = makeJmpNative();
    win.api = { player: makeApiPlayer(win) };
    loadModule('mpv-player-base.js', win); // installs window.MpvPlayerBase
    const mpvAudioPlayer = loadModule('mpv-audio-player.js', win);
    const args = makePlayerArgs(win, extra);
    const player = new mpvAudioPlayer(args);
    return {
        win,
        mpvAudioPlayer,
        player,
        args,
        api: win.api.player,
        events: args.events,
        appSettings: args.appSettings
    };
}

const OPTIONS = {
    url: 'http://server/Audio/1/stream.flac',
    item: { Id: 'a1', Name: 'Song' },
    mediaSource: { Id: 'ms1' }
};

// Start a file and let the load resolve, the way mpv opening it would.
async function start(ctx, options = OPTIONS) {
    ctx.api.autoResolveLoad = true;
    await ctx.player.play(options);
}

// ---- identity -------------------------------------------------------------

test('the plugin introduces itself to jellyfin-web as an audio player', () => {
    const { player } = setup();
    assert.equal(player.id, 'mpvaudioplayer');
    assert.equal(player.name, 'MPV Audio Player');
    assert.equal(player.type, 'mediaplayer');
    assert.equal(player.syncPlayWrapAs, 'htmlaudioplayer');
    assert.equal(player.useServerPlaybackInfoForAudio, true);
    assert.equal(player.mediaType, 'music');
});

test('the plugin plays audio and nothing else', () => {
    const { player } = setup();
    assert.equal(player.canPlayMediaType('Audio'), true);
    assert.equal(player.canPlayMediaType('audio'), true);
    assert.equal(player.canPlayMediaType('Video'), false);
    assert.equal(player.canPlayMediaType('Book'), false);
    assert.equal(player.canPlayMediaType(undefined), false);
});

test('the only advertised feature is the playback rate', () => {
    const { player } = setup();
    assert.equal(player.supports('PlaybackRate'), true);
    assert.equal(player.supports('SetAspectRatio'), false);
    assert.equal(player.supports('AirPlay'), false);
});

// ---- starting a track -----------------------------------------------------

test('playing a track connects the mpv signals and hands mpv the url', async () => {
    const ctx = setup();
    await start(ctx);
    assert.equal(ctx.player.currentSrc(), OPTIONS.url);
    assert.equal(ctx.api.lastLoad.url, OPTIONS.url);
    assert.equal(ctx.api.playing.handlers.length, 1, 'signals connected');
    assert.deepEqual(ctx.api.lastLoad.streamdata, { type: 'music', metadata: OPTIONS.item });
});

test('a track is loaded with video disabled so cover art cannot end it early', async () => {
    // mp3s with embedded art expose an mjpeg "Image" stream; mpv would treat
    // it as a sparse video track and drain the last frame after a fraction of
    // a second. See _resolveTracks in mpv-player-base.js.
    const ctx = setup();
    await start(ctx);
    const load = ctx.api.lastLoad;
    assert.equal(load.videoStream, 0, 'video disabled');
    assert.equal(load.audioStream, 1, 'the one baked-in audio track');
    assert.equal(load.subtitleStream, 0, 'subtitles disabled');
    assert.equal(load.externalAudioUrl, null);
    assert.equal(load.externalSubUrl, null);
});

test('playing a track tells the source badge before the server answers', async () => {
    const ctx = setup();
    const seen = [];
    ctx.win.AstrofinPlaybackSource = { notePlayOptions: (o) => seen.push(o) };
    await start(ctx);
    assert.deepEqual(seen, [OPTIONS]);
});

test('a source badge that throws cannot stop playback', async () => {
    const ctx = setup();
    ctx.win.AstrofinPlaybackSource = {
        notePlayOptions() { throw new Error('badge exploded'); }
    };
    await start(ctx);
    assert.equal(ctx.api.lastLoad.url, OPTIONS.url, 'the file still loaded');
});

test('playing a new track forgets the previous position and duration', async () => {
    const ctx = setup();
    await start(ctx);
    ctx.player._duration = 200_000;
    ctx.player._currentTime = 150_000;
    ctx.player._started = true;
    await start(ctx, { ...OPTIONS, url: 'http://server/Audio/2/stream.flac' });
    assert.equal(ctx.player.duration(), null);
    assert.equal(ctx.player._started, false);
    assert.equal(ctx.player.currentSrc(), 'http://server/Audio/2/stream.flac');
});

// ---- what mpv reports back ------------------------------------------------

test('the first playing signal restores the saved volume and announces it', async () => {
    const ctx = setup({ appSettings: makeAppSettings({ volume: 1 }) });
    await start(ctx);
    ctx.player.setVolume(30); // the user turned it down for the previous track
    ctx.appSettings.set('volume', 0.6);
    ctx.events.triggered.length = 0;
    ctx.api.playing.emit();
    assert.equal(ctx.player.getVolume(), 60);
    assert.ok(ctx.events.names().includes('volumechange'), 'the UI is told it moved');
    assert.ok(ctx.events.names().includes('playing'));
});

test('a playing signal that does not move the volume stays quiet about it', async () => {
    const ctx = setup({ appSettings: makeAppSettings({ volume: 1 }) });
    await start(ctx);
    ctx.events.triggered.length = 0;
    ctx.api.playing.emit();
    assert.equal(ctx.player.getVolume(), 100);
    assert.deepEqual(ctx.events.names(), ['playing']);
});

test('only the first playing signal of a track re-applies the volume', async () => {
    const ctx = setup();
    await start(ctx);
    ctx.api.playing.emit();
    ctx.player.setVolume(20, false);
    ctx.api.playing.emit(); // e.g. after a seek
    assert.equal(ctx.player.getVolume(), 20, 'the volume is left alone');
});

test('playing re-applies the playback rate, which mpv resets per file', async () => {
    const ctx = setup();
    await start(ctx);
    ctx.player.setPlaybackRate(1.5);
    ctx.api.calls.length = 0;
    ctx.api.playing.emit();
    assert.deepEqual(ctx.api.lastCall('setPlaybackRate'), [1500]);
});

test('a position update from mpv becomes a timeupdate', async () => {
    const ctx = setup();
    await start(ctx);
    ctx.player._seeking = true;
    ctx.api.positionUpdate.emit(42_000);
    assert.equal(ctx.player.currentTime(), 42_000);
    assert.equal(ctx.player._seeking, false, 'the seek is over');
    assert.ok(ctx.events.names().includes('timeupdate'));
});

test('position updates are ignored while the track is fading out', async () => {
    const ctx = setup();
    await start(ctx);
    ctx.player._currentTime = 1000;
    ctx.player._isFadingOut = true;
    ctx.events.triggered.length = 0;
    ctx.api.positionUpdate.emit(2000);
    assert.equal(ctx.player.currentTime(), 1000, 'the position is frozen');
    assert.deepEqual(ctx.events.names(), []);
});

// ---- stopping -------------------------------------------------------------

test('stopping between tracks pauses and reports the item ended', async () => {
    const ctx = setup();
    await start(ctx);
    ctx.api.calls.length = 0;
    ctx.events.triggered.length = 0;
    await ctx.player.stop(false);
    assert.deepEqual(ctx.api.names(), ['pause'], 'mpv is paused, not stopped');
    assert.equal(ctx.events.triggered.at(-1).name, 'stopped');
    assert.equal(ctx.player.currentSrc(), null);
    assert.equal(ctx.api.playing.handlers.length, 1, 'still connected for the next track');
});

test('stopping with nothing playing does nothing at all', async () => {
    const ctx = setup();
    await ctx.player.stop(true);
    assert.deepEqual(ctx.api.names(), ['setVolume'], 'only the constructor volume');
    assert.deepEqual(ctx.events.names(), []);
});

test('destroying the player fades the volume out before it stops mpv', async () => {
    const ctx = setup();
    await start(ctx);
    ctx.api.calls.length = 0;

    const stopped = ctx.player.stop(true);
    // The first step runs synchronously; the rest are 100 ms apart.
    assert.deepEqual(ctx.api.lastCall('setVolume'), [85]);
    assert.equal(ctx.player._isFadingOut, true);
    ctx.win.timers.advance(1000);
    await stopped;

    const volumes = ctx.api.callsTo('setVolume').map((c) => c[0]);
    assert.deepEqual(volumes, [85, 70, 55, 40, 25, 10, 0, 100],
        'fifteen points every 100 ms, then the original volume restored');
    assert.equal(ctx.player._isFadingOut, false);
    assert.deepEqual(ctx.api.names().slice(-1), ['stop'], 'mpv stopped last');
    assert.equal(ctx.api.playing.handlers.length, 0, 'signals disconnected');
    assert.equal(ctx.player.duration(), null);
});

test('a second stop cancels the fade timer the first one left running', async () => {
    const ctx = setup();
    await start(ctx);
    const first = ctx.player.stop(true);
    assert.equal(ctx.win.timers.pendingCount, 1, 'a fade step is queued');
    const second = ctx.player.stop(true);
    assert.equal(ctx.win.timers.pendingCount, 1, 'still one, not two');
    ctx.win.timers.advance(2000);
    await second;
    await Promise.race([first, Promise.resolve('abandoned')]);
    assert.equal(ctx.player._isFadingOut, false);
});

test('destroy stops mpv, disconnects and forgets the duration', async () => {
    const ctx = setup();
    await start(ctx);
    ctx.player._duration = 123;
    ctx.api.calls.length = 0;
    ctx.player.destroy();
    assert.deepEqual(ctx.api.names(), ['stop']);
    assert.equal(ctx.api.playing.handlers.length, 0);
    assert.equal(ctx.player.duration(), null);
});
