// Unit tests for the connect-overlay translations. Run with:
//
//     node --test src/web/overlay.lang.test.js
//
// (or `just test-js`). The module is a plain <script> in overlay.html: it picks
// a table off `navigator`, falls back key by key to English, and writes the
// four strings the overlay shows straight into the DOM. So there are two
// things worth testing — the ladder that chooses the table (exact tag, bare
// language, en-us) and the per-key fallback, which is what keeps a
// half-translated language from rendering `undefined` at the user.
//
// The third is a shape check over the generated data itself: every table has
// to carry the same key set, and en-us has to be complete, because it is the
// last resort every other table falls back to.
const test = require('node:test');
const assert = require('node:assert');

const { loadModule, makeWindow } = require('./test/player-fakes.js');

const KEYS = [
    'HeaderConnectToServer',
    'LabelServerHost',
    'LabelServerHostHelp',
    'Connect',
    'HeaderConnectionFailure',
    'MessageUnableToConnectToServer',
    'ButtonGotIt'
];

// overlay.html's markup, reduced to the three nodes the module writes to.
function overlayWindow(navigator) {
    const win = makeWindow(navigator === undefined ? {} : { navigator });
    const doc = win.document;
    for (const [id, tag] of [['title', 'h1'], ['address', 'input'], ['connect-button', 'button']]) {
        const el = doc.createElement(tag);
        el.id = id;
        doc.body.appendChild(el);
    }
    return win;
}

function loadLang(navigator) {
    const win = overlayWindow(navigator);
    return { win, lang: loadModule('overlay.lang.js', win) };
}

function el(win, id) {
    return win.document.getElementById(id);
}

// ---------------------------------------------------------------------------
// Picking the table
// ---------------------------------------------------------------------------

test('getDefaultLanguage prefers navigator.language', () => {
    const { lang } = loadLang({ language: 'fr-FR', userLanguage: 'de-DE', languages: ['it'] });
    assert.strictEqual(lang.getDefaultLanguage(), 'fr-FR');
});

test('getDefaultLanguage falls back to userLanguage, then to languages[0]', () => {
    const withUser = loadLang({ userLanguage: 'de-DE', languages: ['it'] });
    assert.strictEqual(withUser.lang.getDefaultLanguage(), 'de-DE');

    const withList = loadLang({ languages: ['it-IT', 'en-GB'] });
    assert.strictEqual(withList.lang.getDefaultLanguage(), 'it-IT');
});

test('getDefaultLanguage returns en-us when the navigator says nothing', () => {
    const { lang } = loadLang({});
    assert.strictEqual(lang.getDefaultLanguage(), 'en-us');
    assert.strictEqual(lang.getDefaultLanguage(), lang.fallbackLanguage);
});

test('resolveLanguage takes an exact tag when one exists', () => {
    const { lang } = loadLang();
    for (const tag of ['pt-br', 'PT-BR', 'zh-cn', 'en-gb', 'fr-ca']) {
        assert.strictEqual(lang.resolveLanguage(tag), tag.toLowerCase(), tag);
    }
});

test('resolveLanguage drops the region when only the bare language has a table', () => {
    const { lang } = loadLang();
    assert.strictEqual(lang.resolveLanguage('de-DE'), 'de');
    assert.strictEqual(lang.resolveLanguage('fr-BE'), 'fr');
    assert.strictEqual(lang.resolveLanguage('ja-JP'), 'ja');
});

test('resolveLanguage falls back to en-us for anything unknown', () => {
    const { lang } = loadLang();
    for (const tag of ['tlh', 'xx-YY', '', '-', 'xx_YY']) {
        assert.strictEqual(lang.resolveLanguage(tag), 'en-us', tag);
    }
});

test('resolveLanguage reads an underscore as a separator, not as part of the tag', () => {
    // A tag arrives with either separator: a browser says 'ur-PK', a POSIX
    // LANG says 'ur_PK'. Both have to walk the same ladder — before the fix
    // the underscored spelling was one long primary subtag matching nothing,
    // so every one of these landed on English.
    const { lang } = loadLang();
    assert.strictEqual(lang.resolveLanguage('en_US'), 'en-us');
    assert.strictEqual(lang.resolveLanguage('pt_BR'), 'pt-br');
    assert.strictEqual(lang.resolveLanguage('de_DE'), 'de');
    assert.strictEqual(lang.resolveLanguage('fr_BE'), 'fr');
});

test('the underscored tables are reachable through resolveLanguage', () => {
    // jellyfin-web names four of its string files with an underscore
    // (bn_BD, es_419, es_DO, ur_PK). Matching with '_' folded to '-' on both
    // sides is what reaches them, in either spelling; the table's own key is
    // what comes back, so the caller can look the strings up with it.
    const { lang } = loadLang();
    const underscored = lang.languages.filter((l) => l.lang.includes('_')).map((l) => l.lang);
    assert.deepStrictEqual(underscored, ['bn_BD', 'es_419', 'es_DO', 'ur_PK']);
    for (const [tag, table] of [
        ['ur-PK', 'ur_PK'], ['ur_PK', 'ur_PK'],
        ['es-419', 'es_419'], ['es_419', 'es_419'],
        ['es-DO', 'es_DO'], ['es_DO', 'es_DO'],
        ['bn-BD', 'bn_BD'], ['bn_BD', 'bn_BD']
    ]) {
        assert.strictEqual(lang.resolveLanguage(tag), table, tag);
        assert.ok(lang.languages.find((l) => l.lang === table), table);
    }
});

test('an underscored tag drops to its bare language, or to English when there is none', () => {
    // 'zh_Hant' has no table and no bare 'zh' either, so English is right for
    // it; 'es_MX' does have a bare 'es' one step down, and 'sr_Latn' resolves
    // to plain Serbian the same way.
    const { lang } = loadLang();
    assert.strictEqual(lang.resolveLanguage('es_MX'), 'es-mx');
    assert.strictEqual(lang.resolveLanguage('sr_Latn'), 'sr');
    assert.strictEqual(lang.resolveLanguage('zh_Hant'), 'en-us');
});

test('the language picked at load follows the navigator', () => {
    assert.strictEqual(loadLang({ language: 'de-DE' }).lang.language, 'de');
    assert.strictEqual(loadLang({ language: 'pt-BR' }).lang.language, 'pt-br');
    assert.strictEqual(loadLang({ language: 'ur_PK' }).lang.language, 'ur_PK');
    assert.strictEqual(loadLang({ language: 'tlh' }).lang.language, 'en-us');
});

// ---------------------------------------------------------------------------
// Per-key fallback
// ---------------------------------------------------------------------------

test('languageStrings and fallbackStrings are the two tables in play', () => {
    const { lang } = loadLang({ language: 'de' });
    assert.strictEqual(lang.languageStrings.lang, 'de');
    assert.strictEqual(lang.fallbackStrings.lang, 'en-us');
});

test('a fully translated language uses its own strings', () => {
    const { win, lang } = loadLang({ language: 'de' });
    const de = lang.languages.find((l) => l.lang === 'de');
    assert.strictEqual(lang.titleText, de.LabelServerHost);
    assert.strictEqual(lang.connectText, de.Connect);
    assert.strictEqual(lang.headerConnectionFailureText, de.HeaderConnectionFailure);
    assert.strictEqual(lang.messageUnableToConnectToServerText, de.MessageUnableToConnectToServer);
    assert.strictEqual(lang.buttonGotItText, de.ButtonGotIt);
    assert.strictEqual(el(win, 'title').innerText, de.LabelServerHost);
});

test('a half-translated language falls back to English key by key', () => {
    // cy has LabelServerHost, Connect and ButtonGotIt and nothing else.
    const { lang } = loadLang({ language: 'cy' });
    const cy = lang.languages.find((l) => l.lang === 'cy');
    const en = lang.fallbackStrings;
    assert.strictEqual(lang.titleText, cy.LabelServerHost);
    assert.strictEqual(lang.connectText, cy.Connect);
    assert.strictEqual(lang.buttonGotItText, cy.ButtonGotIt);
    assert.strictEqual(cy.MessageUnableToConnectToServer, null, 'the gap this covers');
    assert.strictEqual(lang.messageUnableToConnectToServerText, en.MessageUnableToConnectToServer);
    assert.strictEqual(lang.headerConnectionFailureText, en.HeaderConnectionFailure);
});

test('an entirely untranslated language renders as English', () => {
    // 'as' exists with every value null.
    const { win, lang } = loadLang({ language: 'as' });
    const en = lang.fallbackStrings;
    assert.strictEqual(lang.language, 'as');
    assert.strictEqual(lang.titleText, en.LabelServerHost);
    assert.strictEqual(lang.connectText, en.Connect);
    assert.strictEqual(el(win, 'address').placeholder, en.LabelServerHostHelp);
    assert.strictEqual(el(win, 'connect-button').innerText, en.Connect);
});

test('the server-address label has a last-resort English literal', () => {
    const { lang } = loadLang({ language: 'as' });
    assert.strictEqual(lang.titleText, 'Server Address');
});

// ---------------------------------------------------------------------------
// What the overlay actually gets
// ---------------------------------------------------------------------------

test('loading writes the strings into the overlay markup', () => {
    const { win, lang } = loadLang({ language: 'en-US' });
    assert.strictEqual(el(win, 'title').innerText, 'Server Address');
    assert.strictEqual(el(win, 'address').placeholder, '192.168.1.100:8096 or https://myserver.com');
    assert.strictEqual(el(win, 'connect-button').innerText, 'Connect');
    assert.strictEqual(lang.language, 'en-us');
});

test('the two nodes overlay.js restores carry data-original-text', () => {
    const { win } = loadLang({ language: 'fr' });
    assert.strictEqual(
        el(win, 'title').getAttribute('data-original-text'), el(win, 'title').innerText
    );
    assert.strictEqual(
        el(win, 'connect-button').getAttribute('data-original-text'),
        el(win, 'connect-button').innerText
    );
});

test('window.cancelButtonText is published for the failure dialog', () => {
    const { win } = loadLang({ language: 'de' });
    assert.strictEqual(win.cancelButtonText, 'Cancel');
});

test('the module needs the overlay markup and says so loudly', () => {
    const bare = makeWindow({});
    assert.throws(() => loadModule('overlay.lang.js', bare), TypeError);
});

// ---------------------------------------------------------------------------
// Shape of the generated tables
// ---------------------------------------------------------------------------

test('every table carries exactly the key set of the English one', () => {
    const { lang } = loadLang();
    const want = Object.keys(lang.fallbackStrings).sort();
    assert.deepStrictEqual(want, ['lang'].concat(KEYS).sort());
    for (const table of lang.languages) {
        assert.deepStrictEqual(Object.keys(table).sort(), want, table.lang);
    }
});

test('en-us is present, complete and the declared fallback', () => {
    const { lang } = loadLang();
    assert.strictEqual(lang.fallbackLanguage, 'en-us');
    const en = lang.languages.find((l) => l.lang === 'en-us');
    assert.ok(en, 'the fallback table must exist');
    for (const key of KEYS) {
        assert.strictEqual(typeof en[key], 'string', key);
        assert.ok(en[key].length, key);
    }
});

test('every value is a string or null, never undefined', () => {
    const { lang } = loadLang();
    for (const table of lang.languages) {
        for (const key of KEYS) {
            const value = table[key];
            assert.ok(value === null || typeof value === 'string', table.lang + '.' + key);
        }
    }
});

test('an empty translation falls back like a missing one', () => {
    // fa is the one table with a "" rather than a null (upstream jellyfin-web
    // ships it that way). Both are falsy, so the `||` ladder treats them alike
    // and the overlay never renders an empty failure message.
    const { lang } = loadLang({ language: 'fa' });
    const fa = lang.languages.find((l) => l.lang === 'fa');
    const empties = lang.languages.flatMap((t) => KEYS.filter((k) => t[k] === '').map((k) => t.lang + '.' + k));
    assert.deepStrictEqual(empties, ['fa.MessageUnableToConnectToServer']);
    assert.strictEqual(fa.MessageUnableToConnectToServer, '');
    assert.strictEqual(
        lang.messageUnableToConnectToServerText,
        lang.fallbackStrings.MessageUnableToConnectToServer
    );
    assert.strictEqual(lang.headerConnectionFailureText, fa.HeaderConnectionFailure);
});

test('the tables are unique and sorted by tag', () => {
    const { lang } = loadLang();
    const tags = lang.languages.map((l) => l.lang);
    assert.strictEqual(new Set(tags).size, tags.length, 'no duplicate tables');
    assert.ok(tags.length > 80, 'the generated set is all of jellyfin-web');
    assert.ok(tags.every((t) => t.length >= 2));
});
