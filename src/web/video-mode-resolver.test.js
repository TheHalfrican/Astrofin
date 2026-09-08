// Unit tests for the auto video-mode resolver. Run with:
//
//     node --test src/web/video-mode-resolver.test.js
//
// (or `just test-js`). Nothing here touches a browser or a server: the
// resolver is a pure function precisely so this can stay a plain node test.
const test = require('node:test');
const assert = require('node:assert');

const { resolveVideoMode, ANIMATION, LIVE_ACTION } = require('./video-mode-resolver.js');

const ANIME_LIB = { Id: 'lib-anime', Name: 'Anime' };
const MOVIE_LIB = { Id: 'lib-movies', Name: 'Movies' };
const NO_SETTINGS = { videoModeLibraries: {} };

test('an astrofin tag on the item wins over everything else', () => {
    const item = { Name: 'X', Tags: ['astrofin:live-action'], Genres: ['Anime'] };
    const r = resolveVideoMode(item, null, ANIME_LIB, NO_SETTINGS);
    assert.strictEqual(r.mode, LIVE_ACTION);
    assert.strictEqual(r.reason, 'tag: astrofin:live-action');
});

test('tag matching is case insensitive and accepts the shorthands', () => {
    for (const [tag, mode] of [
        ['Astrofin:Animation', ANIMATION],
        ['ASTROFIN:ANIME', ANIMATION],
        ['  astrofin:live  ', LIVE_ACTION],
        ['astrofin:live-action', LIVE_ACTION]
    ]) {
        const r = resolveVideoMode({ Tags: [tag] }, null, null, NO_SETTINGS);
        assert.strictEqual(r.mode, mode, tag);
        assert.strictEqual(r.reason, 'tag: ' + tag.trim().toLowerCase());
    }
});

test('a series tag applies when the item has none', () => {
    const r = resolveVideoMode(
        { Name: 'S01E01', Tags: [] },
        { Name: 'Show', Tags: ['astrofin:animation'] },
        MOVIE_LIB,
        NO_SETTINGS
    );
    assert.strictEqual(r.mode, ANIMATION);
    assert.strictEqual(r.reason, 'series tag: astrofin:animation');
});

test('an item tag outranks a series tag', () => {
    const r = resolveVideoMode(
        { Tags: ['astrofin:live'] },
        { Tags: ['astrofin:animation'] },
        null,
        NO_SETTINGS
    );
    assert.strictEqual(r.mode, LIVE_ACTION);
});

test('Animation and Anime genres resolve to animation, item before series', () => {
    let r = resolveVideoMode({ Genres: ['Comedy', 'Animation'] }, null, MOVIE_LIB, NO_SETTINGS);
    assert.deepStrictEqual(r, { mode: ANIMATION, reason: 'genre: Animation' });

    r = resolveVideoMode({ Genres: [] }, { Genres: ['Anime'] }, MOVIE_LIB, NO_SETTINGS);
    assert.deepStrictEqual(r, { mode: ANIMATION, reason: 'series genre: Anime' });
});

test('an unrelated genre does not decide anything', () => {
    const r = resolveVideoMode({ Genres: ['Animals', 'Drama'] }, null, MOVIE_LIB, NO_SETTINGS);
    assert.strictEqual(r.mode, LIVE_ACTION);
    assert.strictEqual(r.reason, 'default');
});

test('the library mapping outranks the name heuristic', () => {
    const settings = { videoModeLibraries: { 'lib-anime': 'live-action' } };
    const r = resolveVideoMode({ Name: 'X' }, null, ANIME_LIB, settings);
    assert.deepStrictEqual(r, { mode: LIVE_ACTION, reason: 'library: Anime' });
});

test('the library mapping accepts the pre-rename spellings', () => {
    const settings = { videoModeLibraries: { 'lib-movies': 'anime' } };
    const r = resolveVideoMode({ Name: 'X' }, null, MOVIE_LIB, settings);
    assert.deepStrictEqual(r, { mode: ANIMATION, reason: 'library: Movies' });
});

test('an unusable mapping value falls through to the heuristic', () => {
    const settings = { videoModeLibraries: { 'lib-anime': 'nonsense' } };
    const r = resolveVideoMode({ Name: 'X' }, null, ANIME_LIB, settings);
    assert.deepStrictEqual(r, { mode: ANIMATION, reason: 'library name: Anime' });
});

test('library names containing anime or animation mean animation', () => {
    for (const name of ['Anime', 'my anime shows', 'Animation', 'Kids Animation']) {
        const r = resolveVideoMode({ Name: 'X' }, null, { Id: 'l', Name: name }, NO_SETTINGS);
        assert.strictEqual(r.mode, ANIMATION, name);
        assert.strictEqual(r.reason, 'library name: ' + name);
    }
    for (const name of ['Movies', 'TV Shows', 'Documentaries']) {
        const r = resolveVideoMode({ Name: 'X' }, null, { Id: 'l', Name: name }, NO_SETTINGS);
        assert.strictEqual(r.mode, LIVE_ACTION, name);
    }
});

test('genres outrank the library', () => {
    const r = resolveVideoMode({ Genres: ['Anime'] }, null, MOVIE_LIB, NO_SETTINGS);
    assert.strictEqual(r.mode, ANIMATION);
    assert.strictEqual(r.reason, 'genre: Anime');
});

test('nothing at all still resolves, and never throws', () => {
    for (const args of [
        [null, null, null, null],
        [undefined, undefined, undefined, undefined],
        [{}, {}, {}, {}],
        [{ Tags: 'not-an-array', Genres: 7 }, null, { Id: 1, Name: 2 }, { videoModeLibraries: 3 }]
    ]) {
        const r = resolveVideoMode(...args);
        assert.strictEqual(r.mode, LIVE_ACTION);
        assert.strictEqual(r.reason, 'default');
    }
});
