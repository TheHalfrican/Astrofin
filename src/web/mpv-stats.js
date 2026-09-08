// mpv statistics for jellyfin-web's "Playback Info" overlay.
//
// The native side (`src/playback/src/stats.rs`) observes a set of mpv
// properties while the panel is open and pushes a coalesced snapshot to
// `window._nativeUpdateStats` at most once a second. This module turns the
// latest snapshot into the `{ categories: [...] }` shape jellyfin-web's
// playerstats component renders, and owns the keepalive that tells native
// when the panel stopped asking.
//
// `buildCategories` is pure and has no browser dependency so it can be unit
// tested under plain `node` (see mpv-stats.test.js).
(function (root) {
    'use strict';

    // The panel re-polls `getStats()` on every `timeupdate`, throttled to
    // 700 ms. Two missed polls means it is gone (closed, or playback ended).
    var IDLE_MS = 3000;
    var TICK_MS = 1000;

    var active = false;
    var lastRequest = 0;
    var timer = null;

    // -----------------------------------------------------------------
    // Formatting
    // -----------------------------------------------------------------

    function isNum(v) {
        return typeof v === 'number' && isFinite(v);
    }

    function isText(v) {
        return typeof v === 'string' && v.length > 0;
    }

    // mpv reports bit rates in bits per second.
    function fmtBitrate(bps) {
        if (bps >= 1000000) return (bps / 1000000).toFixed(2) + ' Mbps';
        return (bps / 1000).toFixed(0) + ' kbps';
    }

    function fmtFps(fps) {
        return fps.toFixed(3).replace(/\.?0+$/, '') + ' fps';
    }

    function row(stats, label, value) {
        stats.push({ label: label, value: value });
    }

    // -----------------------------------------------------------------
    // Category builder
    // -----------------------------------------------------------------

    function videoStats(s) {
        var stats = [];
        if (isText(s.videoCodec)) row(stats, 'Video codec', s.videoCodec);
        if (isText(s.hwdec)) {
            // mpv says "no" when it decoded in software.
            row(stats, 'Hardware decoding', s.hwdec === 'no' ? 'No' : s.hwdec);
        }
        if (isNum(s.width) && isNum(s.height)) {
            row(stats, 'Video resolution', s.width + 'x' + s.height);
        }
        if (isText(s.pixelFormat)) row(stats, 'Pixel format', s.pixelFormat);
        if (isText(s.colorMatrix)) row(stats, 'Color matrix', s.colorMatrix);
        if (isNum(s.containerFps)) row(stats, 'Frame rate', fmtFps(s.containerFps));
        if (isNum(s.estimatedFps)) row(stats, 'Measured frame rate', fmtFps(s.estimatedFps));
        if (isNum(s.droppedFramesVo)) row(stats, 'Dropped frames', String(s.droppedFramesVo));
        if (isNum(s.droppedFramesDecoder)) {
            row(stats, 'Dropped frames (decoder)', String(s.droppedFramesDecoder));
        }
        if (isNum(s.videoBitrate)) row(stats, 'Video bitrate', fmtBitrate(s.videoBitrate));
        if (isText(s.vo)) row(stats, 'Video output', s.vo);
        return stats;
    }

    function audioStats(s) {
        var stats = [];
        if (isText(s.audioCodec)) row(stats, 'Audio codec', s.audioCodec);
        if (isText(s.audioFormat)) row(stats, 'Sample format', s.audioFormat);
        if (isNum(s.audioSampleRate)) {
            row(stats, 'Audio sample rate', s.audioSampleRate + ' Hz');
        }
        if (isNum(s.audioChannels)) row(stats, 'Audio channels', String(s.audioChannels));
        if (isNum(s.audioBitrate)) row(stats, 'Audio bitrate', fmtBitrate(s.audioBitrate));
        if (isText(s.ao)) row(stats, 'Audio output', s.ao);
        return stats;
    }

    function playerStats(s) {
        var stats = [];
        if (isText(s.mpvVersion)) row(stats, 'Player version', s.mpvVersion);
        if (isNum(s.cacheDuration)) {
            row(stats, 'Demuxer cache', s.cacheDuration.toFixed(1) + ' s');
        }
        // 100 while the cache is full; anything less is an active buffer fill.
        if (isNum(s.cacheBuffering)) row(stats, 'Cache fill', s.cacheBuffering + '%');
        if (isNum(s.displayFps)) row(stats, 'Display frame rate', fmtFps(s.displayFps));
        if (isNum(s.avsync)) {
            // Math.round can hand back -0, which stringifies as "-0".
            var avsyncMs = Math.round(s.avsync * 1000) || 0;
            row(stats, 'A/V sync', avsyncMs + ' ms');
        }
        return stats;
    }

    /**
     * Build the jellyfin-web playerstats categories from a native snapshot.
     *
     * Categories typed 'video'/'audio' get their heading ("Video Info",
     * "Audio Info") from jellyfin-web's own localization; the player one
     * carries its name directly. A category whose rows are all missing from
     * the snapshot is dropped rather than rendered empty.
     *
     * @param {object|null|undefined} snapshot as pushed by _nativeUpdateStats
     * @returns {Array<{type?: string, name?: string, stats: Array}>}
     */
    function buildCategories(snapshot) {
        if (!snapshot || typeof snapshot !== 'object') return [];
        var out = [];
        var video = videoStats(snapshot);
        if (video.length) out.push({ type: 'video', stats: video });
        var audio = audioStats(snapshot);
        if (audio.length) out.push({ type: 'audio', stats: audio });
        var player = playerStats(snapshot);
        if (player.length) out.push({ name: 'Player Info', stats: player });
        return out;
    }

    // -----------------------------------------------------------------
    // Keepalive
    // -----------------------------------------------------------------

    function setNativeActive(on) {
        active = on;
        var native = root && root.jmpNative;
        if (native && typeof native.playerStatsActive === 'function') {
            native.playerStatsActive(on);
        }
    }

    function stop() {
        if (timer !== null) {
            root.clearInterval(timer);
            timer = null;
        }
        if (active) setNativeActive(false);
        if (root) root._mpvStats = null;
    }

    /**
     * Called from every `getStats()`. Turns the native observations on for
     * the first call and refreshes the deadline for the rest; a poll gap
     * longer than IDLE_MS turns them back off.
     */
    function requestStats() {
        if (!root) return;
        lastRequest = Date.now();
        if (active) return;
        setNativeActive(true);
        if (timer === null && typeof root.setInterval === 'function') {
            timer = root.setInterval(function () {
                if (Date.now() - lastRequest >= IDLE_MS) stop();
            }, TICK_MS);
        }
    }

    var api = {
        buildCategories: buildCategories,
        requestStats: requestStats,
        stop: stop,
        IDLE_MS: IDLE_MS
    };

    if (root) root.AstrofinMpvStats = api;
    // Unit tests run this file under node, where there is no window.
    if (typeof module !== 'undefined' && module.exports) module.exports = api;
})(typeof window !== 'undefined' ? window : null);
