// Unit tests for the mpv Playback Info category builder. Run with:
//
//     node --test src/web/mpv-stats.test.js
//
// (or `just test-js`). `buildCategories` is pure — it maps one native
// snapshot to the `{ type|name, stats: [{label, value}] }` shape
// jellyfin-web's playerstats component renders — so this needs no browser.
const test = require('node:test');
const assert = require('node:assert');

const { buildCategories } = require('./mpv-stats.js');

// A full snapshot, shaped exactly like the JSON `_nativeUpdateStats` receives.
const FULL = {
    videoCodec: 'hevc',
    hwdec: 'd3d11va-copy',
    width: 1920,
    height: 1080,
    pixelFormat: 'yuv420p',
    colorMatrix: 'bt.709',
    containerFps: 23.976,
    estimatedFps: 23.974,
    droppedFramesVo: 0,
    droppedFramesDecoder: 2,
    videoBitrate: 5123456,
    vo: 'gpu-next',
    audioCodec: 'eac3',
    audioFormat: 'floatp',
    audioSampleRate: 48000,
    audioChannels: 6,
    audioBitrate: 384000,
    ao: 'wasapi',
    mpvVersion: 'mpv 0.40.0',
    cacheDuration: 30.52,
    cacheBuffering: 100,
    displayFps: 119.998,
    avsync: -0.0021
};

function byType(categories, key) {
    return categories.find((c) => c.type === key || c.name === key);
}

function labels(category) {
    return category.stats.map((s) => s.label);
}

function value(categories, key, label) {
    const stat = byType(categories, key).stats.find((s) => s.label === label);
    return stat ? stat.value : undefined;
}

test('a full snapshot yields video, audio and player categories', () => {
    const c = buildCategories(FULL);
    assert.strictEqual(c.length, 3);
    // jellyfin-web replaces the name of a typed category with its own
    // localized "Video Info" / "Audio Info"; the third carries its own.
    assert.deepStrictEqual(
        c.map((x) => x.type || x.name),
        ['video', 'audio', 'Player Info']
    );
});

test('video rows carry the expected labels and formatted values', () => {
    const c = buildCategories(FULL);
    assert.deepStrictEqual(labels(byType(c, 'video')), [
        'Video codec',
        'Hardware decoding',
        'Video resolution',
        'Pixel format',
        'Color matrix',
        'Frame rate',
        'Measured frame rate',
        'Dropped frames',
        'Dropped frames (decoder)',
        'Video bitrate',
        'Video output'
    ]);
    assert.strictEqual(value(c, 'video', 'Video codec'), 'hevc');
    assert.strictEqual(value(c, 'video', 'Hardware decoding'), 'd3d11va-copy');
    assert.strictEqual(value(c, 'video', 'Video resolution'), '1920x1080');
    assert.strictEqual(value(c, 'video', 'Pixel format'), 'yuv420p');
    assert.strictEqual(value(c, 'video', 'Color matrix'), 'bt.709');
    assert.strictEqual(value(c, 'video', 'Frame rate'), '23.976 fps');
    assert.strictEqual(value(c, 'video', 'Measured frame rate'), '23.974 fps');
    assert.strictEqual(value(c, 'video', 'Dropped frames'), '0');
    assert.strictEqual(value(c, 'video', 'Dropped frames (decoder)'), '2');
    assert.strictEqual(value(c, 'video', 'Video bitrate'), '5.12 Mbps');
    assert.strictEqual(value(c, 'video', 'Video output'), 'gpu-next');
});

test('audio rows carry the expected labels and formatted values', () => {
    const c = buildCategories(FULL);
    assert.deepStrictEqual(labels(byType(c, 'audio')), [
        'Audio codec',
        'Sample format',
        'Audio sample rate',
        'Audio channels',
        'Audio bitrate',
        'Audio output'
    ]);
    assert.strictEqual(value(c, 'audio', 'Audio codec'), 'eac3');
    assert.strictEqual(value(c, 'audio', 'Sample format'), 'floatp');
    assert.strictEqual(value(c, 'audio', 'Audio sample rate'), '48000 Hz');
    assert.strictEqual(value(c, 'audio', 'Audio channels'), '6');
    assert.strictEqual(value(c, 'audio', 'Audio bitrate'), '384 kbps');
    assert.strictEqual(value(c, 'audio', 'Audio output'), 'wasapi');
});

test('player rows carry the expected labels and formatted values', () => {
    const c = buildCategories(FULL);
    assert.deepStrictEqual(labels(byType(c, 'Player Info')), [
        'Player version',
        'Demuxer cache',
        'Cache fill',
        'Display frame rate',
        'A/V sync'
    ]);
    assert.strictEqual(value(c, 'Player Info', 'Player version'), 'mpv 0.40.0');
    assert.strictEqual(value(c, 'Player Info', 'Demuxer cache'), '30.5 s');
    assert.strictEqual(value(c, 'Player Info', 'Cache fill'), '100%');
    assert.strictEqual(value(c, 'Player Info', 'Display frame rate'), '119.998 fps');
    assert.strictEqual(value(c, 'Player Info', 'A/V sync'), '-2 ms');
});

test('hwdec "no" is spelled out as No', () => {
    const c = buildCategories({ hwdec: 'no' });
    assert.strictEqual(value(c, 'video', 'Hardware decoding'), 'No');
});

test('missing fields are omitted, not rendered blank', () => {
    const c = buildCategories({ videoCodec: 'h264', vo: 'gpu-next' });
    assert.strictEqual(c.length, 1);
    assert.deepStrictEqual(labels(byType(c, 'video')), ['Video codec', 'Video output']);
});

test('a category with no rows at all is dropped', () => {
    // Audio-only playback: no video properties in the snapshot.
    const c = buildCategories({ audioCodec: 'flac', audioChannels: 2 });
    assert.deepStrictEqual(
        c.map((x) => x.type || x.name),
        ['audio']
    );
});

test('a half-known resolution is not rendered', () => {
    const c = buildCategories({ width: 1920, videoCodec: 'h264' });
    assert.deepStrictEqual(labels(byType(c, 'video')), ['Video codec']);
});

test('no snapshot yields no categories', () => {
    assert.deepStrictEqual(buildCategories(null), []);
    assert.deepStrictEqual(buildCategories(undefined), []);
    assert.deepStrictEqual(buildCategories('nonsense'), []);
    assert.deepStrictEqual(buildCategories({}), []);
});

test('frame rates drop trailing zeros and bitrates switch unit at 1 Mbps', () => {
    let c = buildCategories({ containerFps: 60, videoBitrate: 999999 });
    assert.strictEqual(value(c, 'video', 'Frame rate'), '60 fps');
    assert.strictEqual(value(c, 'video', 'Video bitrate'), '1000 kbps');
    c = buildCategories({ containerFps: 29.97, videoBitrate: 1000000 });
    assert.strictEqual(value(c, 'video', 'Frame rate'), '29.97 fps');
    assert.strictEqual(value(c, 'video', 'Video bitrate'), '1.00 Mbps');
});

test('a near-zero A/V sync never renders as "-0 ms"', () => {
    const c = buildCategories({ avsync: -0.0004 });
    assert.strictEqual(value(c, 'Player Info', 'A/V sync'), '0 ms');
});

test('zero counters still render — they are meaningful', () => {
    const c = buildCategories({ droppedFramesVo: 0, droppedFramesDecoder: 0, cacheBuffering: 0 });
    assert.strictEqual(value(c, 'video', 'Dropped frames'), '0');
    assert.strictEqual(value(c, 'video', 'Dropped frames (decoder)'), '0');
    assert.strictEqual(value(c, 'Player Info', 'Cache fill'), '0%');
});
