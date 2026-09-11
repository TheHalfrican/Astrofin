// Unit tests for the in-page <select> popup. Run with:
//
//     node --test src/web/select-menu.test.js
//
// (or `just test-js`). The module replaces the engine's native dropdown with a
// shadow-DOM menu, so the behaviour worth pinning is everything the native
// popup used to guarantee: which selects it takes over at all, that a row maps
// back to `option.index` rather than to its position (disabled options and
// optgroup headers make those differ), that a commit fires `input` *and*
// `change` exactly once and only when the selection really moved, that every
// dismissal path tears the host down, and that repositioning keeps the menu on
// screen.
const test = require('node:test');
const assert = require('node:assert');

const { makeUiWindow, loadModule, makeSelect, fireEvent } = require('./test/ui-fakes.js');

const OPTIONS = [
    { text: 'Auto', value: 'auto', selected: true },
    { text: 'Never', value: 'never' },
    { text: 'Always', value: 'always' }
];

function load() {
    const win = makeUiWindow();
    const api = loadModule('select-menu.js', win);
    return { win, doc: win.document, api };
}

// A select in the page, with a layout box the menu can be placed against.
function addSelect(ctx, spec = OPTIONS, attrs = {}, rect = {}) {
    const select = makeSelect(ctx.doc, spec, attrs);
    select._rect = Object.assign({ left: 40, top: 100, bottom: 132, width: 200, height: 32 }, rect);
    ctx.doc.body.appendChild(select);
    return select;
}

function host(ctx) {
    return ctx.doc.getElementById('_jselect');
}

function shadow(ctx) {
    const h = host(ctx);
    return h ? h._shadow : null;
}

function rows(ctx) {
    return shadow(ctx).querySelectorAll('.i');
}

// The events the select itself sees, in order.
function watch(select) {
    const seen = [];
    ['input', 'change'].forEach((type) => select.addEventListener(type, () => seen.push(type)));
    return seen;
}

// ---- which selects are taken over -----------------------------------------

test('a plain single select is treated as a dropdown', () => {
    const ctx = load();
    assert.strictEqual(ctx.api.isDropdown(addSelect(ctx)), true);
});

test('multiple, sized, disabled and non-select elements are left alone', () => {
    const ctx = load();
    assert.strictEqual(ctx.api.isDropdown(addSelect(ctx, OPTIONS, { multiple: true })), false);
    assert.strictEqual(ctx.api.isDropdown(addSelect(ctx, OPTIONS, { size: 4 })), false);
    assert.strictEqual(ctx.api.isDropdown(addSelect(ctx, OPTIONS, { disabled: true })), false);
    assert.strictEqual(ctx.api.isDropdown(ctx.doc.createElement('input')), false);
    // A missing element short-circuits to the argument itself, which is the
    // falsy answer every caller tests for.
    assert.ok(!ctx.api.isDropdown(null));
    assert.ok(!ctx.api.isDropdown(undefined));
});

// ---- opening --------------------------------------------------------------

test('mousedown on a select opens the menu instead of the native popup', () => {
    const ctx = load();
    const select = addSelect(ctx);
    const event = fireEvent(select, 'mousedown', { button: 0 });

    assert.strictEqual(event.defaultPrevented, true, 'the native popup is suppressed');
    assert.ok(host(ctx), 'the shadow host is in the page');
    assert.strictEqual(ctx.doc.activeElement, select, 'the select keeps focus');
    assert.deepStrictEqual(rows(ctx).map((r) => r.textContent), ['Auto', 'Never', 'Always']);
    assert.strictEqual(ctx.api.isOpen(), true);
});

test('a non-primary button never opens the menu', () => {
    const ctx = load();
    const select = addSelect(ctx);
    fireEvent(select, 'mousedown', { button: 2 });
    assert.strictEqual(host(ctx), null);
});

test('a second mousedown closes the open menu rather than reopening it', () => {
    const ctx = load();
    const select = addSelect(ctx);
    fireEvent(select, 'mousedown', { button: 0 });
    fireEvent(select, 'mousedown', { button: 0 });
    assert.strictEqual(host(ctx), null);
    assert.strictEqual(ctx.api.isOpen(), false);
});

test('the keys that open a native dropdown open this one', () => {
    for (const key of [{ key: ' ' }, { key: 'Enter' }, { key: 'F4' },
        { key: 'ArrowDown', altKey: true }, { key: 'ArrowUp', altKey: true }]) {
        const ctx = load();
        const select = addSelect(ctx);
        select.focus();
        const event = fireEvent(ctx.doc, 'keydown', Object.assign({ target: select }, key));
        assert.ok(host(ctx), 'opened by ' + JSON.stringify(key));
        assert.strictEqual(event.defaultPrevented, true);
    }
});

test('other keys, and an unfocused select, leave the menu closed', () => {
    const ctx = load();
    const select = addSelect(ctx);
    select.focus();
    fireEvent(ctx.doc, 'keydown', { key: 'ArrowDown' }); // no alt
    assert.strictEqual(host(ctx), null);
    fireEvent(ctx.doc, 'keydown', { key: 'a' });
    assert.strictEqual(host(ctx), null);

    select.blur();
    fireEvent(ctx.doc, 'keydown', { key: 'Enter' });
    assert.strictEqual(host(ctx), null);
});

test('while the menu is open the document key handler stands down', () => {
    const ctx = load();
    const select = addSelect(ctx);
    ctx.api.openMenu(select);
    const first = host(ctx);
    select.focus();
    fireEvent(ctx.doc, 'keydown', { key: 'Enter' });
    // Still the same host: the menu's own capture handler owns the keyboard.
    assert.strictEqual(host(ctx), first);
});

// ---- rendering ------------------------------------------------------------

test('the selected option is marked and starts active', () => {
    const ctx = load();
    const select = addSelect(ctx, [
        { text: 'Auto' }, { text: 'Never', selected: true }, { text: 'Always' }
    ]);
    ctx.api.openMenu(select);
    const list = rows(ctx);
    assert.deepStrictEqual(list.map((r) => r.classList.contains('sel')), [false, true, false]);
    assert.deepStrictEqual(list.map((r) => r.classList.contains('a')), [false, true, false]);
    assert.strictEqual(list[1].scrolledIntoView, 1);
});

test('optgroups render a header and their options stay selectable', () => {
    const ctx = load();
    const select = addSelect(ctx, [
        { text: 'Off' },
        { label: 'Enhanced', options: [{ text: 'Live-Action' }, { text: 'Animation' }] }
    ]);
    ctx.api.openMenu(select);
    assert.deepStrictEqual(shadow(ctx).querySelectorAll('.g').map((g) => g.textContent),
        ['Enhanced']);
    assert.deepStrictEqual(rows(ctx).map((r) => r.textContent),
        ['Off', 'Live-Action', 'Animation']);
});

test('disabled options and disabled groups are shown but not selectable', () => {
    const ctx = load();
    const select = addSelect(ctx, [
        { text: 'Auto' },
        { text: 'Broken', disabled: true },
        { label: 'Unavailable', disabled: true, options: [{ text: 'HDR passthrough' }] }
    ]);
    ctx.api.openMenu(select);
    const all = shadow(ctx).querySelectorAll('.i');
    assert.deepStrictEqual(all.map((r) => r.textContent),
        ['Auto', 'Broken', 'HDR passthrough']);
    assert.deepStrictEqual(all.map((r) => r.classList.contains('off')),
        [false, true, true]);
    // Only the selectable row carries the index used on commit.
    assert.deepStrictEqual(all.map((r) => r.getAttribute('data-idx')), ['0', null, null]);
});

// ---- committing a choice --------------------------------------------------

test('clicking a row selects that option and fires input then change once', () => {
    const ctx = load();
    const select = addSelect(ctx);
    const seen = watch(select);
    ctx.api.openMenu(select);
    fireEvent(rows(ctx)[2], 'mousedown', { button: 0 });

    assert.strictEqual(select.selectedIndex, 2);
    assert.deepStrictEqual(seen, ['input', 'change']);
    assert.strictEqual(host(ctx), null, 'the menu closes on commit');
    assert.strictEqual(ctx.api.isOpen(), false);
});

test('a row maps back to option.index, not to its position in the menu', () => {
    const ctx = load();
    // Two unselectable entries before the row we click: if the module keyed
    // rows by position, this would select "Never" instead of "Always".
    const select = addSelect(ctx, [
        { text: 'Auto', selected: true },
        { text: 'Broken', disabled: true },
        { label: 'Group', options: [{ text: 'Never' }, { text: 'Always' }] }
    ]);
    ctx.api.openMenu(select);
    const target = rows(ctx).find((r) => r.textContent === 'Always');
    fireEvent(target, 'mousedown', { button: 0 });
    assert.strictEqual(select.selectedIndex, 3);
    assert.strictEqual(select.options[3].text, 'Always');
});

test('clicking a disabled row neither selects nor closes', () => {
    const ctx = load();
    const select = addSelect(ctx, [{ text: 'Auto', selected: true }, { text: 'Broken', disabled: true }]);
    const seen = watch(select);
    ctx.api.openMenu(select);
    const off = shadow(ctx).querySelectorAll('.i.off')[0];
    fireEvent(off, 'mousedown', { button: 0 });
    assert.strictEqual(select.selectedIndex, 0);
    assert.deepStrictEqual(seen, []);
    assert.ok(host(ctx), 'the menu stays open');
});

test('committing the option that was already selected fires nothing', () => {
    const ctx = load();
    const select = addSelect(ctx);
    const seen = watch(select);
    ctx.api.openMenu(select);
    fireEvent(rows(ctx)[0], 'mousedown', { button: 0 });
    assert.deepStrictEqual(seen, []);
    assert.strictEqual(select.selectedIndex, 0);
    assert.strictEqual(host(ctx), null);
});

test('arrow keys move the active row and wrap at both ends', () => {
    const ctx = load();
    const select = addSelect(ctx);
    ctx.api.openMenu(select);
    const active = () => rows(ctx).findIndex((r) => r.classList.contains('a'));
    assert.strictEqual(active(), 0);

    fireEvent(ctx.win, 'keydown', { key: 'ArrowDown' });
    assert.strictEqual(active(), 1);
    fireEvent(ctx.win, 'keydown', { key: 'ArrowDown' });
    fireEvent(ctx.win, 'keydown', { key: 'ArrowDown' });
    assert.strictEqual(active(), 0, 'past the end wraps to the top');
    fireEvent(ctx.win, 'keydown', { key: 'ArrowUp' });
    assert.strictEqual(active(), 2, 'before the start wraps to the bottom');
});

test('Enter and Space commit the active row', () => {
    for (const key of ['Enter', ' ']) {
        const ctx = load();
        const select = addSelect(ctx);
        const seen = watch(select);
        ctx.api.openMenu(select);
        fireEvent(ctx.win, 'keydown', { key: 'ArrowDown' });
        const event = fireEvent(ctx.win, 'keydown', { key });
        assert.strictEqual(select.selectedIndex, 1, key + ' commits');
        assert.deepStrictEqual(seen, ['input', 'change']);
        assert.strictEqual(event.defaultPrevented, true);
    }
});

test('Escape and Tab close without changing the selection', () => {
    for (const key of ['Escape', 'Tab']) {
        const ctx = load();
        const select = addSelect(ctx);
        const seen = watch(select);
        ctx.api.openMenu(select);
        fireEvent(ctx.win, 'keydown', { key: 'ArrowDown' });
        fireEvent(ctx.win, 'keydown', { key });
        assert.strictEqual(host(ctx), null, key + ' closes the menu');
        assert.strictEqual(select.selectedIndex, 0);
        assert.deepStrictEqual(seen, []);
    }
});

// ---- dismissal ------------------------------------------------------------

test('the backdrop, window blur, resize and scroll all dismiss the menu', () => {
    const cases = [
        (ctx) => fireEvent(shadow(ctx).querySelector('.bg'), 'mousedown', { button: 0 }),
        (ctx) => fireEvent(ctx.win, 'blur'),
        (ctx) => fireEvent(ctx.win, 'resize'),
        (ctx) => fireEvent(ctx.doc, 'scroll')
    ];
    for (const dismiss of cases) {
        const ctx = load();
        const select = addSelect(ctx);
        ctx.api.openMenu(select);
        dismiss(ctx);
        assert.strictEqual(host(ctx), null);
        assert.strictEqual(select.selectedIndex, 0);
    }
});

test('closeOpen closes the menu and is safe when none is open', () => {
    const ctx = load();
    assert.doesNotThrow(() => ctx.api.closeOpen());
    ctx.api.openMenu(addSelect(ctx));
    ctx.api.closeOpen();
    assert.strictEqual(host(ctx), null);
    assert.strictEqual(ctx.api.isOpen(), false);
});

test('a dismissed menu has unhooked its global listeners', () => {
    const ctx = load();
    const select = addSelect(ctx);
    const before = ctx.win.listeners.length;
    ctx.api.openMenu(select);
    assert.ok(ctx.win.listeners.length > before);
    fireEvent(ctx.win, 'blur');
    assert.strictEqual(ctx.win.listeners.length, before);
    // A stray key after teardown must not resurrect anything.
    fireEvent(ctx.win, 'keydown', { key: 'Enter' });
    assert.strictEqual(host(ctx), null);
});

test('opening a second menu closes the first', () => {
    const ctx = load();
    const first = addSelect(ctx);
    const second = addSelect(ctx);
    ctx.api.openMenu(first);
    const firstHost = host(ctx);
    ctx.api.openMenu(second);
    assert.notStrictEqual(host(ctx), firstHost);
    assert.strictEqual(ctx.doc.querySelectorAll('#_jselect').length, 1);
});

// ---- placement ------------------------------------------------------------

test('the menu is placed under the select and at least as wide', () => {
    const ctx = load();
    const select = addSelect(ctx);
    ctx.api.openMenu(select);
    const menu = shadow(ctx).querySelector('.m');
    assert.strictEqual(menu.style.left, '40px');
    assert.strictEqual(menu.style.top, '132px');
    assert.strictEqual(menu.style.minWidth, '200px');
    assert.strictEqual(menu.style.maxHeight, '712px');
});

test('a menu that would overflow the bottom flips above the select', () => {
    const ctx = load();
    const select = addSelect(ctx, OPTIONS, {}, { top: 500, bottom: 532 });
    ctx.api.openMenu(select);
    const menu = shadow(ctx).querySelector('.m');
    menu._rect = { top: 532, bottom: 832, height: 300, left: 40, right: 240, width: 200 };
    ctx.win.timers.advance(20); // the rAF pass
    assert.strictEqual(menu.style.top, '200px', 'placed above: 500 - 300');
});

test('a menu too tall to fit either way is clamped to the viewport', () => {
    const ctx = load();
    const select = addSelect(ctx, OPTIONS, {}, { top: 100, bottom: 132 });
    ctx.api.openMenu(select);
    const menu = shadow(ctx).querySelector('.m');
    menu._rect = { top: 132, bottom: 932, height: 800, left: 40, right: 240, width: 200 };
    ctx.win.timers.advance(20);
    // 720 - 800 is negative, so it pins to the top edge rather than off-screen.
    assert.strictEqual(menu.style.top, '0px');
});

test('a menu that would overflow the right edge is pulled back', () => {
    const ctx = load();
    const select = addSelect(ctx, OPTIONS, {}, { left: 1200, top: 100, bottom: 132, width: 200 });
    ctx.api.openMenu(select);
    const menu = shadow(ctx).querySelector('.m');
    menu._rect = { top: 132, bottom: 232, height: 100, left: 1200, right: 1400, width: 200 };
    ctx.win.timers.advance(20);
    assert.strictEqual(menu.style.left, '1076px', '1280 - 200 - 4');
});

// ---- the menu's own skin ---------------------------------------------------
//
// The menu lives in a *closed* shadow root, so its stylesheet is a string
// inside the module and nothing else can reach it. Custom properties inherit
// through the shadow boundary, which is how it wears the Astrofin palette; a
// literal fallback on every one is what keeps it readable in a page that has
// no theme (the E2E mock server, and any layer the stylesheet is not injected
// into).

function menuCss(ctx) {
    return shadow(ctx).querySelector('style').textContent;
}

test('the menu is drawn from the Astrofin tokens, not the old grey box', () => {
    const ctx = load();
    ctx.api.openMenu(addSelect(ctx));
    const css = menuCss(ctx);
    for (const token of ['--af-surface-raised', '--af-edge-strong', '--af-text-primary',
        '--af-text-muted', '--af-accent-primary', '--af-radius-md', '--af-radius-sm']) {
        assert.ok(css.includes(token), `stylesheet should use ${token}`);
    }
    for (const grey of ['#2b2b2b', '#3d3d3d', '#e0e0e0', '#9a9a9a', '#555', '#666']) {
        assert.ok(!css.includes(grey), `stylesheet should not hard-code ${grey}`);
    }
});

test('every token the menu names carries a literal fallback', () => {
    const ctx = load();
    ctx.api.openMenu(addSelect(ctx));
    const css = menuCss(ctx);
    const named = [...css.matchAll(/var\(\s*(--[\w-]+)\s*([,)])/g)];
    assert.ok(named.length > 0, 'the stylesheet should reference custom properties');
    for (const [, name, next] of named) {
        assert.strictEqual(next, ',', `var(${name}) must carry a fallback`);
    }
});

test('every token the menu names is one the tokens sheet defines', () => {
    const fs = require('node:fs');
    const path = require('node:path');
    const tokens = fs.readFileSync(path.join(__dirname, 'astrofin-tokens.css'), 'utf8');
    const ctx = load();
    ctx.api.openMenu(addSelect(ctx));
    for (const [, name] of menuCss(ctx).matchAll(/var\(\s*(--[\w-]+)/g)) {
        assert.ok(tokens.includes(name + ':'), `${name} is not defined in astrofin-tokens.css`);
    }
});
