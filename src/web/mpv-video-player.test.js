// Unit tests for the mpvVideoPlayer plugin. Run with:
//
//     node --test src/web/mpv-video-player.test.js
//
// (or `just test-js`). This is the widest surface in src/web: jellyfin-web
// calls it as an HTML video element it cannot see, while the picture is drawn
// by mpv in its own window underneath. Three things are worth testing here
// and are what these tests are about.
//
//   * Track selection. jellyfin-web speaks in global `MediaStream.Index`
//     values and mpv in 1-based per-type track ids, and everything about
//     transcodes, external audio and external subtitles is a special case in
//     that translation.
//   * The dialog. The `.videoPlayerContainer` is a transparent hole punched
//     through the browser layer with a poster behind it; creating it, reusing
//     it, dropping the poster when the first frame arrives and putting it back
//     on stop are what the user actually sees.
//   * Auto video mode, which runs immediately before loadfile and must never
//     be able to stop playback — every failure has to degrade to live-action.
//
// The fakes come from test/player-fakes.js; the video-mode resolver is the
// real module, loaded into the same fake window. Nothing here touches a
// profile dir, a server or the app.
const test = require('node:test');
const assert = require('node:assert');
const {
    loadModule,
    makeWindow,
    makeApiPlayer,
    makeJmpNative,
    makePlayerArgs,
    makeApiClient,
    makeAppSettings
} = require('./test/player-fakes.js');

function setup(extra) {
    const win = makeWindow();
    win.jmpNative = makeJmpNative();
    win.api = { player: makeApiPlayer(win) };
    win._isFullscreen = false;
    loadModule('mpv-player-base.js', win); // installs window.MpvPlayerBase
    loadModule('video-mode-resolver.js', win); // installs window.AstrofinVideoMode
    const mod = loadModule('mpv-video-player.js', win);
    const args = makePlayerArgs(win, extra);
    const player = new mod.mpvVideoPlayer(args);
    return {
        win,
        mod,
        player,
        args,
        doc: win.document,
        api: win.api.player,
        native: win.jmpNative,
        events: args.events,
        appSettings: args.appSettings
    };
}

// A play() that also answers the two things mpv would: the zoom-in animation
// the dialog waits for, and the loadfile callback.
function startPlay(ctx, options) {
    ctx.api.autoResolveLoad = true;
    const started = ctx.player.play(options);
    const dlg = ctx.doc.querySelector('.videoPlayerContainer');
    if (dlg) dlg.dispatchEvent({ type: 'animationend' });
    return started;
}

function dialog(ctx) {
    return ctx.doc.querySelector('.videoPlayerContainer');
}

// One episode, one video stream, two internal audio tracks and one subtitle.
function mediaSource(extra) {
    return Object.assign({
        Id: 'ms1',
        DefaultAudioStreamIndex: 1,
        DefaultSubtitleStreamIndex: -1,
        MediaStreams: [
            { Index: 0, Type: 'Video' },
            { Index: 1, Type: 'Audio', Language: 'jpn' },
            { Index: 2, Type: 'Audio', Language: 'eng' },
            { Index: 3, Type: 'Subtitle', Language: 'eng' }
        ]
    }, extra);
}

function playOptions(extra) {
    return Object.assign({
        url: 'http://server/Videos/1/stream.mkv',
        item: { Id: 'i1', Name: 'Episode' },
        mediaSource: mediaSource()
    }, extra);
}

// ---- identity -------------------------------------------------------------

test('the plugin introduces itself to jellyfin-web as the local video player', () => {
    const { player, win } = setup();
    assert.equal(player.id, 'mpvvideoplayer');
    assert.equal(player.name, 'MPV Video Player');
    assert.equal(player.type, 'mediaplayer');
    assert.equal(player.syncPlayWrapAs, 'htmlvideoplayer');
    assert.equal(player.priority, -1);
    assert.equal(player.isLocalPlayer, true);
    assert.equal(player.useFullSubtitleUrls, true);
    assert.equal(player.mediaType, 'video');
    // native-shim.js's fullscreen and OSD callbacks find the player here.
    assert.equal(win._mpvVideoPlayerInstance, player);
});

test('the plugin plays video and nothing else', () => {
    const { player } = setup();
    assert.equal(player.canPlayMediaType('Video'), true);
    assert.equal(player.canPlayMediaType('video'), true);
    assert.equal(player.canPlayMediaType('Audio'), false);
    assert.equal(player.canPlayMediaType(null), false);
    assert.equal(player.canPlayItem({ MediaType: 'Video' }), true);
    assert.equal(player.canPlayItem({ MediaType: 'Audio' }), false);
    assert.equal(player.canPlayItem({}), false);
});

test('the advertised features are the playback rate and the aspect ratio', () => {
    const { player, mod } = setup();
    assert.deepEqual(mod.mpvVideoPlayer.getSupportedFeatures(), ['PlaybackRate', 'SetAspectRatio']);
    assert.equal(player.supports('PlaybackRate'), true);
    assert.equal(player.supports('SetAspectRatio'), true);
    assert.equal(player.supports('PictureInPicture'), false);
    // Every play method reaches mpv, so none is ever refused here.
    assert.equal(player.supportsPlayMethod('Transcode', {}), true);
    assert.equal(player.supportsPlayMethod('DirectPlay', {}), true);
});

test('the features mpv does not have are inert rather than missing', () => {
    // jellyfin-web calls these unconditionally; they must exist and do nothing.
    const { player, api } = setup();
    const before = api.calls.length;
    player.setPictureInPictureEnabled(true);
    player.togglePictureInPicture();
    player.setAirPlayEnabled(true);
    player.toggleAirPlay();
    player.setBrightness(50);
    player.setSecondarySubtitleStreamIndex(2);
    assert.equal(player.isPictureInPictureEnabled(), false);
    assert.equal(player.isAirPlayEnabled(), false);
    assert.equal(player.getBrightness(), 100);
    assert.equal(api.calls.length, before, 'mpv is never told about any of it');
});

// ---- the video dialog -----------------------------------------------------

test('the first play builds the transparent container and wires everything up', async () => {
    const ctx = setup();
    await ctx.player.createMediaElement(playOptions());
    const dlg = dialog(ctx);
    assert.ok(dlg, 'the container exists');
    assert.equal(dlg.parentNode, ctx.doc.body);
    assert.equal(ctx.doc.body.firstChild, dlg, 'inserted first, under the web UI');
    assert.ok(dlg.style.cssText.includes('background:transparent'), 'mpv shows through');
    assert.deepEqual(ctx.native.lastCall('playerOsdActive'), [true]);
    assert.deepEqual(ctx.native.lastCall('notifyRateChange'), [1]);
    assert.equal(ctx.api.playing.handlers.length, 1, 'mpv signals connected');
});

test('a second play reuses the container instead of stacking another one', async () => {
    const ctx = setup();
    await ctx.player.createMediaElement(playOptions());
    ctx.native.calls.length = 0;
    await ctx.player.createMediaElement(playOptions());
    assert.equal(ctx.doc.querySelectorAll('.videoPlayerContainer').length, 1);
    assert.deepEqual(ctx.native.names(), [], 'the OSD is already active');
    assert.equal(ctx.api.playing.handlers.length, 1, 'signals connected once');
});

test('a fullscreen container zooms in, hides the scrollbar and goes on top', async () => {
    const ctx = setup();
    const ready = ctx.player.createMediaElement(playOptions({ fullscreen: true }));
    const dlg = dialog(ctx);
    assert.equal(dlg.style.zIndex, 1000);
    assert.ok(dlg.style.animation.startsWith('mpv-video-zoomin'));
    assert.equal(ctx.doc.body.classList.contains('hide-scroll'), true);
    dlg.dispatchEvent({ type: 'animationend' });
    await ready;
});

test('the backdrop is used as the poster, and a plain black one without it', async () => {
    const ctx = setup();
    await ctx.player.createMediaElement(playOptions({ backdropUrl: 'http://server/backdrop.jpg' }));
    let poster = dialog(ctx).querySelector('.mpvPoster');
    assert.ok(poster.style.cssText.includes("url('http://server/backdrop.jpg')"));

    const bare = setup();
    await bare.player.createMediaElement(playOptions());
    poster = dialog(bare).querySelector('.mpvPoster');
    assert.ok(poster.style.cssText.includes('background:#000;'), poster.style.cssText);
});

test('a new poster replaces the old one rather than piling up', async () => {
    const ctx = setup();
    await ctx.player.createMediaElement(playOptions());
    await ctx.player.createMediaElement(playOptions({ backdropUrl: 'http://server/b.jpg' }));
    const posters = dialog(ctx).querySelectorAll('.mpvPoster');
    assert.equal(posters.length, 1);
    assert.ok(posters[0].style.cssText.includes('http://server/b.jpg'));
});

test('the backdrop transparency is only handed to a dashboard that has one', async () => {
    const calls = [];
    const dashboard = { default: { setBackdropTransparency: (level) => calls.push(level) } };
    const ctx = setup({ dashboard });
    await ctx.player.createMediaElement(playOptions());
    assert.deepEqual(calls, [2], 'transparent while the video plays');
    ctx.player.destroy();
    assert.deepEqual(calls, [2, 0], 'opaque again once it is gone');
});

// ---- play -----------------------------------------------------------------

test('playing an item resets the per-file state and hands mpv the url', async () => {
    const ctx = setup();
    ctx.player._timeUpdated = true;
    ctx.player._endedPending = true;
    ctx.player._started = true;
    await startPlay(ctx, playOptions());
    assert.equal(ctx.player.currentSrc(), 'http://server/Videos/1/stream.mkv');
    assert.equal(ctx.player._timeUpdated, false);
    assert.equal(ctx.player._endedPending, false);
    assert.equal(ctx.player._started, false, 'still waiting for the first frame');
    assert.equal(ctx.api.lastLoad.streamdata.type, 'video');
});

test('playing tells the source badge before the server has answered', async () => {
    const ctx = setup();
    const seen = [];
    ctx.win.AstrofinPlaybackSource = { notePlayOptions: (o) => seen.push(o.url) };
    await startPlay(ctx, playOptions());
    assert.deepEqual(seen, ['http://server/Videos/1/stream.mkv']);
});

test('a source badge that throws cannot stop playback', async () => {
    const ctx = setup();
    ctx.win.AstrofinPlaybackSource = { notePlayOptions() { throw new Error('badge exploded'); } };
    await startPlay(ctx, playOptions());
    assert.equal(ctx.api.lastLoad.url, 'http://server/Videos/1/stream.mkv');
});

test('playing fullscreen shows the loading spinner until the first frame', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions({ fullscreen: true }));
    assert.deepEqual(ctx.args.loading.names(), ['show']);
    ctx.api.playing.emit();
    assert.deepEqual(ctx.args.loading.names(), ['show', 'hide']);
});

test('playing resets the subtitle offset unless the caller keeps it', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions());
    assert.deepEqual(ctx.api.callsTo('setSubtitleDelay'), [[0]]);

    ctx.api.calls.length = 0;
    await startPlay(ctx, playOptions({ resetSubtitleOffset: false }));
    assert.deepEqual(ctx.api.callsTo('setSubtitleDelay'), [], 'the offset survives');
});

test('a single external audio track is re-selected once the file is open', async () => {
    // The server does not publish a DeliveryUrl for it, so the only way to
    // hear it is to go back through playbackManager and let the server
    // rebuild the stream.
    const ctx = setup();
    const asked = [];
    ctx.args.playbackManager.setAudioStreamIndex = (index, player) => asked.push([index, player]);
    await startPlay(ctx, playOptions({
        mediaSource: mediaSource({
            MediaStreams: [
                { Index: 0, Type: 'Video' },
                { Index: 1, Type: 'Audio', IsExternal: true }
            ]
        })
    }));
    assert.deepEqual(asked, [[1, ctx.player]]);
});

test('a transcode leaves the external audio to the server', async () => {
    const ctx = setup();
    const asked = [];
    ctx.args.playbackManager.setAudioStreamIndex = (index) => asked.push(index);
    await startPlay(ctx, playOptions({
        playMethod: 'Transcode',
        mediaSource: mediaSource({
            MediaStreams: [
                { Index: 0, Type: 'Video' },
                { Index: 1, Type: 'Audio', IsExternal: true }
            ]
        })
    }));
    assert.deepEqual(asked, [], 'the audio is already baked into the stream');
});

// ---- what mpv reports back ------------------------------------------------

test('the first frame hides the spinner, drops the poster and reports playing', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions({ backdropUrl: 'http://server/b.jpg' }));
    assert.ok(dialog(ctx).querySelector('.mpvPoster'), 'the poster covers the hole until then');
    ctx.api.playing.emit();
    assert.equal(dialog(ctx).querySelector('.mpvPoster'), null, 'mpv shows through now');
    assert.ok(ctx.events.names().includes('playing'));
    assert.deepEqual(ctx.api.lastCall('setVideoRectangle'), [0, 0, 0, 0]);
});

test('the first frame of a fullscreen item opens the OSD and lowers the dialog', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions({ fullscreen: true }));
    ctx.api.playing.emit();
    assert.deepEqual(ctx.args.appRouter.names(), ['showVideoOsd']);
    assert.equal(dialog(ctx).style.zIndex, 'unset', 'the OSD is above the video again');
});

test('later playing signals only report playing', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions({ fullscreen: true }));
    ctx.api.playing.emit();
    ctx.args.appRouter.calls.length = 0;
    ctx.args.loading.calls.length = 0;
    ctx.api.playing.emit(); // e.g. after unpausing
    assert.deepEqual(ctx.args.appRouter.names(), [], 'the OSD is not reopened');
    assert.deepEqual(ctx.args.loading.names(), []);
    assert.equal(ctx.events.names().filter((n) => n === 'playing').length, 2);
});

test('a position update from mpv becomes a timeupdate', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions());
    ctx.player._seeking = true;
    ctx.api.positionUpdate.emit(61_000);
    assert.equal(ctx.player.currentTime(), 61_000);
    assert.equal(ctx.player._seeking, false);
    assert.equal(ctx.player._timeUpdated, true);
    assert.ok(ctx.events.names().includes('timeupdate'));
});

test('the end of a file is reported exactly once', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions());
    ctx.events.triggered.length = 0;
    ctx.api.finished.emit();
    ctx.api.finished.emit(); // mpv can report the end twice around a stop
    assert.deepEqual(ctx.events.names(), ['stopped']);
});

test('an mpv error tears the dialog down and reports a decode error', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions());
    ctx.api.error.emit('no decoder');
    assert.equal(dialog(ctx), null, 'the black hole is not left on screen');
    assert.deepEqual(ctx.events.triggered.at(-1).args, [{ type: 'mediadecodeerror' }]);
    assert.deepEqual(ctx.native.lastCall('playerOsdActive'), [false]);
});

// ---- track selection ------------------------------------------------------

test('the default audio track reaches mpv as a 1-based per-type index', () => {
    const { player } = setup();
    const tracks = player._resolveTracks({ mediaSource: mediaSource({ DefaultAudioStreamIndex: 2 }) });
    assert.deepEqual(tracks, {
        videoParam: 1,
        audioParam: 2, // global index 2 is the second audio track
        subParam: 0,
        externalAudioUrl: null,
        externalSubUrl: null
    });
});

test('a transcode always plays the single track the server produced', () => {
    const { player } = setup();
    const tracks = player._resolveTracks({
        playMethod: 'Transcode',
        mediaSource: mediaSource({ DefaultAudioStreamIndex: 2, DefaultSubtitleStreamIndex: 3 })
    });
    assert.equal(tracks.audioParam, 1, 'source indices do not apply to the m3u8');
    assert.equal(tracks.externalAudioUrl, null);
});

test('audio delivered as a separate file is loaded by url, not by index', () => {
    const { player } = setup();
    const tracks = player._resolveTracks({
        mediaSource: mediaSource({
            DefaultAudioStreamIndex: 4,
            MediaStreams: [
                { Index: 0, Type: 'Video' },
                { Index: 4, Type: 'Audio', DeliveryMethod: 'External', DeliveryUrl: '/audio/4.mka' }
            ]
        })
    });
    assert.equal(tracks.externalAudioUrl, '/audio/4.mka');
    assert.equal(tracks.audioParam, 0, 'no embedded track is selected');
});

test('an item with no default audio falls back to its first internal track', () => {
    const { player } = setup();
    const tracks = player._resolveTracks({
        mediaSource: mediaSource({
            DefaultAudioStreamIndex: undefined,
            MediaStreams: [
                { Index: 0, Type: 'Video' },
                { Index: 1, Type: 'Audio', IsExternal: true },
                { Index: 2, Type: 'Audio' }
            ]
        })
    });
    assert.equal(tracks.audioParam, 1, 'the first embedded audio track');
});

test('an item with no audio at all plays with audio disabled', () => {
    const { player } = setup();
    const tracks = player._resolveTracks({
        mediaSource: mediaSource({
            DefaultAudioStreamIndex: -1,
            MediaStreams: [{ Index: 0, Type: 'Video' }]
        })
    });
    assert.equal(tracks.audioParam, 0);
    assert.equal(tracks.videoParam, 1);
});

test('a chosen subtitle track reaches mpv as a 1-based subtitle index', () => {
    const { player } = setup();
    const tracks = player._resolveTracks({ mediaSource: mediaSource({ DefaultSubtitleStreamIndex: 3 }) });
    assert.equal(tracks.subParam, 1, 'the first subtitle track');
    assert.equal(tracks.externalSubUrl, null);
});

test('a subtitle delivered as a separate file is loaded by url', () => {
    const { player } = setup();
    const tracks = player._resolveTracks({
        mediaSource: mediaSource({
            DefaultSubtitleStreamIndex: 3,
            MediaStreams: [
                { Index: 0, Type: 'Video' },
                { Index: 1, Type: 'Audio' },
                { Index: 3, Type: 'Subtitle', DeliveryMethod: 'External', DeliveryUrl: '/subs/3.ass' }
            ]
        })
    });
    assert.equal(tracks.externalSubUrl, '/subs/3.ass');
    assert.equal(tracks.subParam, 0);
});

test('a stream index the media source does not have disables the track', () => {
    const { player } = setup();
    const tracks = player._resolveTracks({
        mediaSource: mediaSource({ DefaultAudioStreamIndex: 99, DefaultSubtitleStreamIndex: 98 })
    });
    assert.equal(tracks.audioParam, 0);
    assert.equal(tracks.subParam, 0);
});

test('per-type indices count only that type and skip external files', () => {
    const { mod } = setup();
    const streams = [
        { Index: 0, Type: 'Video' },
        { Index: 1, Type: 'Audio' },
        { Index: 2, Type: 'Subtitle' },
        { Index: 3, Type: 'Audio', IsExternal: true },
        { Index: 4, Type: 'Audio' },
        { Index: 5, Type: 'Subtitle' }
    ];
    assert.equal(mod.getRelativeIndexByType(streams, 1, 'Audio'), 1);
    assert.equal(mod.getRelativeIndexByType(streams, 4, 'Audio'), 2, 'the external one is not counted');
    assert.equal(mod.getRelativeIndexByType(streams, 3, 'Audio'), null, 'external files have no track id');
    assert.equal(mod.getRelativeIndexByType(streams, 5, 'Subtitle'), 2);
    assert.equal(mod.getRelativeIndexByType(streams, 9, 'Audio'), null);
    assert.equal(mod.getRelativeIndexByType([], 0, 'Audio'), null);
});

test('a stream is looked up by its global index, or not at all', () => {
    const { mod } = setup();
    const streams = mediaSource().MediaStreams;
    assert.deepEqual(mod.getStreamByIndex(streams, 2), { Index: 2, Type: 'Audio', Language: 'eng' });
    assert.equal(mod.getStreamByIndex(streams, 42), null);
    assert.equal(mod.getStreamByIndex([], 0), null);
});

test('the aspect mode is applied before the file, from the options or the setting', () => {
    const { player, api } = setup();
    player._beforeLoad({ aspectRatio: 'cover' });
    assert.deepEqual(api.lastCall('setAspectMode'), ['cover']);
    player.setAspectRatio('fill');
    player._beforeLoad({});
    assert.deepEqual(api.lastCall('setAspectMode'), ['fill'], 'the stored ratio is used');
});

// ---- changing tracks mid-file ---------------------------------------------

test('choosing a subtitle track sends mpv the per-type index', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions());
    ctx.player.setSubtitleStreamIndex(3);
    assert.deepEqual(ctx.api.lastCall('setSubtitleStream'), [1]);
});

test('turning subtitles off disables the mpv track', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions());
    ctx.player.setSubtitleStreamIndex(-1);
    assert.deepEqual(ctx.api.lastCall('setSubtitleStream'), [0]);
    ctx.player.setSubtitleStreamIndex(null);
    assert.deepEqual(ctx.api.lastCall('setSubtitleStream'), [0]);
    ctx.player.setSubtitleStreamIndex(undefined);
    assert.deepEqual(ctx.api.lastCall('setSubtitleStream'), [0]);
});

test('choosing an external subtitle loads the file instead of a track', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions({
        mediaSource: mediaSource({
            MediaStreams: [
                { Index: 0, Type: 'Video' },
                { Index: 1, Type: 'Audio' },
                { Index: 3, Type: 'Subtitle', DeliveryMethod: 'External', DeliveryUrl: '/subs/3.ass' }
            ]
        })
    }));
    ctx.player.setSubtitleStreamIndex(3);
    assert.deepEqual(ctx.api.lastCall('addSubtitleStream'), ['/subs/3.ass']);
});

test('a subtitle index that is not in the file disables subtitles', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions());
    ctx.player.setSubtitleStreamIndex(99);
    assert.deepEqual(ctx.api.lastCall('setSubtitleStream'), [0]);
});

test('choosing an audio track sends mpv the per-type index', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions());
    ctx.player.setAudioStreamIndex(2);
    assert.deepEqual(ctx.api.lastCall('setAudioStream'), [2]);
    ctx.player.setAudioStreamIndex(-1);
    assert.deepEqual(ctx.api.lastCall('setAudioStream'), [0]);
    ctx.player.setAudioStreamIndex(null);
    assert.deepEqual(ctx.api.lastCall('setAudioStream'), [0]);
});

test('choosing external audio goes back through the server, once', async () => {
    const ctx = setup();
    const seen = [];
    await startPlay(ctx, playOptions({
        mediaSource: mediaSource({
            MediaStreams: [
                { Index: 0, Type: 'Video' },
                { Index: 1, Type: 'Audio' },
                { Index: 2, Type: 'Audio', IsExternal: true }
            ]
        })
    }));
    assert.equal(ctx.player.canSetAudioStreamIndex(), true);
    ctx.args.playbackManager.setAudioStreamIndex = (index, player) => {
        // While the call is in flight the player must claim it cannot switch
        // audio itself, or playbackManager takes the client-side path again.
        seen.push([index, player.canSetAudioStreamIndex()]);
    };
    ctx.player.setAudioStreamIndex(2);
    assert.deepEqual(seen, [[2, false]]);
    assert.equal(ctx.player.canSetAudioStreamIndex(), true, 'the flag is cleared again');
});

test('the reload flag is cleared even when playbackManager throws', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions({
        mediaSource: mediaSource({
            MediaStreams: [
                { Index: 0, Type: 'Video' },
                { Index: 2, Type: 'Audio', IsExternal: true }
            ]
        })
    }));
    ctx.args.playbackManager.setAudioStreamIndex = () => { throw new Error('no such stream'); };
    assert.throws(() => ctx.player.setAudioStreamIndex(2), /no such stream/);
    assert.equal(ctx.player.canSetAudioStreamIndex(), true);
});

// ---- subtitle offset ------------------------------------------------------

test('the subtitle offset is sent to mpv in milliseconds', () => {
    const { player, api } = setup();
    player.setSubtitleOffset(1.25);
    assert.deepEqual(api.lastCall('setSubtitleDelay'), [1250]);
    assert.equal(player.getSubtitleOffset(), 1.25);
});

test('an unusable subtitle offset is treated as none', () => {
    const { player, api } = setup();
    player.setSubtitleOffset('later please');
    assert.deepEqual(api.lastCall('setSubtitleDelay'), [0]);
    assert.equal(player.getSubtitleOffset(), 0);
});

test('resetting the subtitle offset clears mpv and the stored value', () => {
    const { player, api } = setup();
    player.setSubtitleOffset(-2);
    player.resetSubtitleOffset();
    assert.deepEqual(api.lastCall('setSubtitleDelay'), [0]);
    assert.equal(player.getSubtitleOffset(), 0);
    assert.equal(player.isShowingSubtitleOffsetEnabled(), false);
});

test('the subtitle offset control can be shown and hidden', () => {
    const { player } = setup();
    assert.equal(player.isShowingSubtitleOffsetEnabled(), false);
    player.enableShowingSubtitleOffset();
    assert.equal(player.isShowingSubtitleOffsetEnabled(), true);
    player.disableShowingSubtitleOffset();
    assert.equal(player.isShowingSubtitleOffsetEnabled(), false);
});

// ---- aspect ratio and fullscreen ------------------------------------------

test('the aspect ratios offered are auto, cover and fill, translated', () => {
    const { player } = setup({ globalize: { translate: (key) => 'T:' + key } });
    assert.deepEqual(player.getSupportedAspectRatios(), [
        { id: 'auto', name: 'T:Auto' },
        { id: 'cover', name: 'T:AspectRatioCover' },
        { id: 'fill', name: 'T:AspectRatioFill' }
    ]);
});

test('the aspect ratio comes from appSettings when jellyfin-web has one', () => {
    const appSettings = makeAppSettings({ volume: 1 });
    let stored = 'cover';
    appSettings.aspectRatio = (value) => {
        if (value !== undefined) stored = value;
        return stored;
    };
    const { player, api } = setup({ appSettings });
    assert.equal(player.getAspectRatio(), 'cover');
    player.setAspectRatio('fill');
    assert.equal(stored, 'fill', 'the setting is what persists');
    assert.deepEqual(api.lastCall('setAspectMode'), ['fill']);
});

test('an older jellyfin-web without the setting keeps the ratio in the player', () => {
    const { player, api } = setup();
    assert.equal(player.getAspectRatio(), 'auto', 'the default');
    player.setAspectRatio('cover');
    assert.equal(player.getAspectRatio(), 'cover');
    assert.deepEqual(api.lastCall('setAspectMode'), ['cover']);
    player.destroy();
    assert.equal(player.getAspectRatio(), 'auto', 'forgotten with the player');
});

test('fullscreen is whatever the native side last said, and toggling asks it', () => {
    const { player, win, native } = setup();
    assert.equal(player.isFullscreen(), false);
    win._isFullscreen = true;
    assert.equal(player.isFullscreen(), true);
    win._isFullscreen = 'yes';
    assert.equal(player.isFullscreen(), false, 'only a real boolean counts');
    player.toggleFullscreen();
    assert.deepEqual(native.names().slice(-1), ['toggleFullscreen']);
});

test('a rate change reaches both mpv and the OS media session', () => {
    const { player, api, native } = setup();
    player.setPlaybackRate(1.5);
    assert.deepEqual(api.lastCall('setPlaybackRate'), [1500], 'mpv takes thousandths');
    assert.deepEqual(native.lastCall('notifyRateChange'), [1.5], 'MPRIS takes the rate itself');
    assert.equal(player.getPlaybackRate(), 1.5);
});

// ---- stopping -------------------------------------------------------------

test('stopping between episodes puts the backdrop back over the hole', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions({ backdropUrl: 'http://server/b.jpg' }));
    ctx.api.playing.emit(); // the poster is removed on the first frame
    await ctx.player.stop(false);
    const poster = dialog(ctx).querySelector('.mpvPoster');
    assert.ok(poster, 'the container is transparent without it');
    assert.ok(poster.style.cssText.includes('http://server/b.jpg'));
    assert.deepEqual(ctx.api.names().slice(-1), ['stop']);
    assert.equal(ctx.events.triggered.at(-1).name, 'stopped');
});

test('stopping twice reports the item stopped only once', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions());
    await ctx.player.stop(false);
    ctx.events.triggered.length = 0;
    await ctx.player.stop(false);
    assert.deepEqual(ctx.events.names(), []);
});

test('stopping for good removes the container and disconnects from mpv', async () => {
    const ctx = setup();
    await startPlay(ctx, playOptions({ fullscreen: true }));
    await ctx.player.stop(true);
    assert.equal(dialog(ctx), null);
    assert.equal(ctx.doc.body.classList.contains('hide-scroll'), false);
    assert.deepEqual(ctx.native.lastCall('playerOsdActive'), [false]);
    assert.deepEqual(ctx.api.lastCall('setVideoRectangle'), [-1, 0, 0, 0], 'the video layer is hidden');
    assert.equal(ctx.api.playing.handlers.length, 0);
});

test('tearing down without a container is safe', () => {
    const { player, api, native } = setup();
    player.removeMediaDialog();
    assert.deepEqual(api.names().slice(-1), ['setVideoRectangle']);
    assert.deepEqual(native.lastCall('playerOsdActive'), [false]);
});

// ---- auto video mode ------------------------------------------------------

function withAutoMode(ctx, opts = {}) {
    ctx.win.jmpInfo = {
        settings: { playback: { videoMode: opts.videoMode || 'auto' } },
        videoModeLibraries: opts.videoModeLibraries || {}
    };
    ctx.win.ApiClient = makeApiClient();
    return ctx.win.ApiClient;
}

test('a fixed video mode is left alone, tags and libraries included', async () => {
    const ctx = setup();
    const client = withAutoMode(ctx, { videoMode: 'animation' });
    client.getItem = () => { throw new Error('no lookup should happen'); };
    await ctx.player.applyAutoVideoMode({ item: { Id: 'i1', Tags: ['astrofin:live'] } });
    assert.deepEqual(ctx.native.callsTo('setPlaybackVideoMode'), []);
});

test('an astrofin tag on the item decides the mode without asking the server', async () => {
    const ctx = setup();
    withAutoMode(ctx);
    await ctx.player.applyAutoVideoMode({ item: { Id: 'i1', Name: 'Akira', Tags: ['astrofin:anime'] } });
    assert.deepEqual(ctx.native.lastCall('setPlaybackVideoMode'),
        ['animation', 'tag: astrofin:anime', 'Akira']);
});

test('an item that decides nothing is looked up through its series', async () => {
    const ctx = setup();
    const client = withAutoMode(ctx);
    client.items.set('s1', { Id: 's1', Name: 'Cowboy Bebop', Genres: ['Anime'] });
    await ctx.player.applyAutoVideoMode({
        item: { Id: 'e1', Name: 'Asteroid Blues', SeriesId: 's1' }
    });
    assert.deepEqual(ctx.native.lastCall('setPlaybackVideoMode'),
        ['animation', 'series genre: Anime', 'Asteroid Blues']);
    assert.deepEqual(client.lastGetItem, ['user-1', 's1']);
});

test('the library the item lives in is the last word before live-action', async () => {
    const ctx = setup();
    const client = withAutoMode(ctx, { videoModeLibraries: { lib1: 'animation' } });
    client.ancestors.set('m1', [
        { Id: 'folder', Type: 'Folder' },
        { Id: 'lib1', Name: 'Cartoons', CollectionType: 'movies' }
    ]);
    await ctx.player.applyAutoVideoMode({ item: { Id: 'm1', Name: 'Fantasia' } });
    assert.deepEqual(ctx.native.lastCall('setPlaybackVideoMode'),
        ['animation', 'library: Cartoons', 'Fantasia']);
});

test('an item that nothing matches plays as live action', async () => {
    const ctx = setup();
    withAutoMode(ctx);
    await ctx.player.applyAutoVideoMode({ item: { Id: 'm1', Name: 'Heat' } });
    assert.deepEqual(ctx.native.lastCall('setPlaybackVideoMode'), ['live-action', 'default', 'Heat']);
});

test('the series and ancestor lookups are cached for the session', async () => {
    const ctx = setup();
    const client = withAutoMode(ctx);
    let getItems = 0;
    let ancestorCalls = 0;
    client.items.set('s1', { Id: 's1', Name: 'Show' });
    const realGetItem = client.getItem;
    client.getItem = (u, id) => { getItems += 1; return realGetItem(u, id); };
    client.getAncestorItems = () => { ancestorCalls += 1; return Promise.resolve([]); };

    const options = { item: { Id: 'e1', Name: 'Ep 1', SeriesId: 's1' } };
    await ctx.player.applyAutoVideoMode(options);
    await ctx.player.applyAutoVideoMode({ item: { Id: 'e2', Name: 'Ep 2', SeriesId: 's1' } });
    assert.equal(getItems, 1, 'the series is fetched once');
    assert.equal(ancestorCalls, 1, 'the ancestors are cached by the shared parent');
});

test('a failed series lookup is not retried and does not stop playback', async () => {
    const ctx = setup();
    const client = withAutoMode(ctx);
    let calls = 0;
    client.getItem = () => { calls += 1; return Promise.reject(new Error('404')); };
    const options = { item: { Id: 'e1', Name: 'Ep 1', SeriesId: 'gone' } };
    await ctx.player.applyAutoVideoMode(options);
    await ctx.player.applyAutoVideoMode(options);
    assert.equal(calls, 1, 'the broken id is remembered, not retried');
    assert.deepEqual(ctx.native.lastCall('setPlaybackVideoMode'), ['live-action', 'default', 'Ep 1']);
});

test('a failed ancestor lookup falls through to live action', async () => {
    const ctx = setup();
    const client = withAutoMode(ctx);
    client.getAncestorItems = () => Promise.reject(new Error('500'));
    await ctx.player.applyAutoVideoMode({ item: { Id: 'm1', Name: 'Heat' } });
    assert.deepEqual(ctx.native.lastCall('setPlaybackVideoMode'), ['live-action', 'default', 'Heat']);
});

test('a missing resolver degrades to live action instead of throwing', async () => {
    const ctx = setup();
    withAutoMode(ctx);
    delete ctx.win.AstrofinVideoMode;
    await ctx.player.applyAutoVideoMode({ item: { Id: 'm1', Name: 'Heat' } });
    const call = ctx.native.lastCall('setPlaybackVideoMode');
    assert.equal(call[0], 'live-action');
    assert.match(call[1], /^fallback after error: video-mode-resolver\.js not loaded$/);
});

test('an item with no name at all still resolves and is logged as unknown', async () => {
    const ctx = setup();
    withAutoMode(ctx);
    await ctx.player.applyAutoVideoMode({});
    assert.deepEqual(ctx.native.lastCall('setPlaybackVideoMode'), ['live-action', 'default', 'unknown']);
});

test('the resolved mode is announced to the native side and the log', () => {
    const ctx = setup();
    ctx.player._vmApply('animation', 'tag: astrofin:anime', { Name: 'Akira' });
    assert.deepEqual(ctx.native.lastCall('setPlaybackVideoMode'),
        ['animation', 'tag: astrofin:anime', 'Akira']);
    assert.ok(ctx.win.console.lines.some((l) => l.level === 'info'));
});

test('an item lookup without an id is answered without a round trip', async () => {
    const ctx = setup();
    const client = withAutoMode(ctx);
    client.getItem = () => { throw new Error('no lookup should happen'); };
    assert.equal(await ctx.player._vmItem(null), null);
    assert.equal(await ctx.player._vmItem(''), null);
});

test('the library is the first ancestor that is one, however it is labelled', async () => {
    const ctx = setup();
    const client = withAutoMode(ctx);
    client.ancestors.set('m1', [
        { Id: 'p1', Type: 'Folder' },
        { Id: 'v1', Name: 'Movies', Type: 'UserView' }
    ]);
    assert.deepEqual(await ctx.player._vmLibrary({ Id: 'm1' }, null), { Id: 'v1', Name: 'Movies' });
});

test('an item outside any library has none', async () => {
    const ctx = setup();
    const client = withAutoMode(ctx);
    client.ancestors.set('m1', [{ Id: 'p1', Type: 'Folder' }]);
    assert.equal(await ctx.player._vmLibrary({ Id: 'm1' }, null), null);
    assert.equal(await ctx.player._vmLibrary(null, null), null, 'and neither has nothing');
});
