// Per-title video-mode resolution for the `auto` setting.
//
// Pure and synchronous: everything it needs is handed in, so it can be unit
// tested under plain `node` and so the caller (mpv-video-player.js) owns all
// the network work and caching. First rule that matches wins:
//
//   1. an `astrofin:` tag on the item, then on its series/parent
//   2. an Animation/Anime genre on the item, then on its series/parent
//   3. the library: an explicit `videoModeLibraries` mapping, else a name
//      heuristic ("anime"/"animation" in the library name)
//   4. live-action
//
// Never throws and never returns null: an unusable argument just falls
// through to the next rule and, in the end, to live-action.
(function (root) {
    'use strict';

    var ANIMATION = 'animation';
    var LIVE_ACTION = 'live-action';

    // Tag overrides. Both the canonical names and the shorthands the user is
    // likely to type by hand.
    var TAGS = {
        'astrofin:animation': ANIMATION,
        'astrofin:anime': ANIMATION,
        'astrofin:live-action': LIVE_ACTION,
        'astrofin:live': LIVE_ACTION
    };

    // Genres that mean animation. Genres never imply live-action: that is the
    // fallback, so an absent genre and a live-action genre are the same thing.
    var ANIMATION_GENRES = { animation: true, anime: true };

    // Library names that mean animation when no explicit mapping exists.
    var ANIMATION_NAME_HINTS = ['anime', 'animation'];

    function norm(value) {
        return typeof value === 'string' ? value.trim().toLowerCase() : '';
    }

    function list(value) {
        return Array.isArray(value) ? value : [];
    }

    // A mode value from settings.json, which is hand-edited and therefore
    // untrusted. Legacy spellings are accepted the same way the Rust side
    // accepts them.
    function normMode(value) {
        switch (norm(value)) {
            case 'animation':
            case 'anime':
                return ANIMATION;
            case 'live-action':
            case 'live':
            case 'movies':
                return LIVE_ACTION;
            default:
                return null;
        }
    }

    function tagMode(item) {
        var tags = list(item && item.Tags);
        for (var i = 0; i < tags.length; i++) {
            var tag = norm(tags[i]);
            if (Object.prototype.hasOwnProperty.call(TAGS, tag)) {
                return { mode: TAGS[tag], label: tag };
            }
        }
        return null;
    }

    function genreMode(item) {
        var genres = list(item && item.Genres);
        for (var i = 0; i < genres.length; i++) {
            if (Object.prototype.hasOwnProperty.call(ANIMATION_GENRES, norm(genres[i]))) {
                return { mode: ANIMATION, label: String(genres[i]) };
            }
        }
        return null;
    }

    function libraryMode(library, settings) {
        if (!library) return null;
        var name = typeof library.Name === 'string' ? library.Name : '';
        var map = (settings && settings.videoModeLibraries) || {};
        var id = library.Id;
        if (id && Object.prototype.hasOwnProperty.call(map, id)) {
            var mapped = normMode(map[id]);
            if (mapped) return { mode: mapped, reason: 'library: ' + (name || id) };
        }
        var lowered = norm(name);
        for (var i = 0; i < ANIMATION_NAME_HINTS.length; i++) {
            if (lowered.indexOf(ANIMATION_NAME_HINTS[i]) !== -1) {
                return { mode: ANIMATION, reason: 'library name: ' + name };
            }
        }
        return null;
    }

    /**
     * @param {object|null} item     the item being played
     * @param {object|null} series   its series/parent, when one was fetched
     * @param {object|null} library  its top-level library ({ Id, Name })
     * @param {object|null} settings { videoModeLibraries }
     * @returns {{ mode: string, reason: string }}
     */
    function resolveVideoMode(item, series, library, settings) {
        var hit = tagMode(item);
        if (hit) return { mode: hit.mode, reason: 'tag: ' + hit.label };
        hit = tagMode(series);
        if (hit) return { mode: hit.mode, reason: 'series tag: ' + hit.label };

        hit = genreMode(item);
        if (hit) return { mode: hit.mode, reason: 'genre: ' + hit.label };
        hit = genreMode(series);
        if (hit) return { mode: hit.mode, reason: 'series genre: ' + hit.label };

        hit = libraryMode(library, settings);
        if (hit) return { mode: hit.mode, reason: hit.reason };

        return { mode: LIVE_ACTION, reason: 'default' };
    }

    var api = {
        resolveVideoMode: resolveVideoMode,
        ANIMATION: ANIMATION,
        LIVE_ACTION: LIVE_ACTION
    };

    if (root) root.AstrofinVideoMode = api;
    // Unit tests run this file under node, where there is no window.
    if (typeof module !== 'undefined' && module.exports) module.exports = api;
})(typeof window !== 'undefined' ? window : null);
