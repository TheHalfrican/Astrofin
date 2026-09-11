// Client settings page — rendered into .mainAnimatedPages so it participates
// in breadcrumb/back-button navigation like Display settings.
//
// viewManager is an ES module we can't import from this script, so we
// replicate its page lifecycle: insert into .mainAnimatedPages, hide siblings,
// dispatch the same events that libraryMenu.js listens on (pageshow via
// pageClassOn) to update the header.
(function() {
    function dispatchPageEvents(target, isRestored) {
        const detail = {
            detail: { type: null, properties: [], params: {}, isRestored: !!isRestored, options: {} },
            bubbles: true
        };
        target.dispatchEvent(new CustomEvent('viewbeforeshow', detail));
        target.dispatchEvent(new CustomEvent('pagebeforeshow', detail));
        target.dispatchEvent(new CustomEvent('viewshow', detail));
        target.dispatchEvent(new CustomEvent('pageshow', detail));
    }

    // `html.af-settings` is the gate every Settings rule in astrofin-theme.css
    // hangs off. Guarded because the test fakes (and a context this script can
    // in principle run in before the document has a root) may have no
    // documentElement.
    function setSettingsGate(on) {
        const root = document.documentElement;
        if (!root || !root.classList) return false;
        if (on) root.classList.add('af-settings');
        else root.classList.remove('af-settings');
        return true;
    }

    // Fired on `document` (not on the page: the listener is installed once, at
    // injection time, long before any page exists).
    function dispatchSettingsEvent(type, page) {
        if (typeof CustomEvent !== 'function' || !document.dispatchEvent) return false;
        document.dispatchEvent(new CustomEvent(type, { detail: { page }, bubbles: false }));
        return true;
    }

    function showSettingsPage() {
        const mainAnimatedPages = document.querySelector('.mainAnimatedPages');
        if (!mainAnimatedPages) return;

        // Hide all existing legacy pages (viewContainer manages multiple slots)
        const visiblePages = mainAnimatedPages.querySelectorAll('.mainAnimatedPage:not(.hide)');
        for (const p of visiblePages) {
            p.dispatchEvent(new CustomEvent('viewbeforehide', { bubbles: true, cancelable: true }));
            p.classList.add('hide');
            p.dispatchEvent(new CustomEvent('viewhide', { bubbles: true }));
        }

        // Hide the React page container (sibling .skinBody that holds React routes)
        const reactContainer = mainAnimatedPages.nextElementSibling;
        if (reactContainer) reactContainer.classList.add('hide');

        // Build the page element matching jellyfin-web's legacy page structure.
        // Display settings uses: div[data-role=page] > div.settingsContainer > form
        const page = document.createElement('div');
        page.id = 'clientSettingsPage';
        page.setAttribute('data-role', 'page');
        page.setAttribute('data-title', 'Client Settings');
        page.setAttribute('data-backbutton', 'true');
        page.className = 'mainAnimatedPage page libraryPage userPreferencesPage noSecondaryNavPage';
        page.style.overflow = 'auto';

        const settingsContainer = document.createElement('div');
        settingsContainer.className = 'settingsContainer padded-left padded-right padded-bottom-page';
        page.appendChild(settingsContainer);

        const form = document.createElement('form');
        form.style.margin = '0 auto';
        settingsContainer.appendChild(form);

        buildSettingsForm(form);

        mainAnimatedPages.appendChild(page);

        // The Astrofin gate. astrofin-settings.js decorates the mounted page
        // (rail, glass panels, segmented switch) and astrofin-theme.css keys
        // every settings rule off `html.af-settings`, so the class goes on
        // before the event fires. Both are best-effort: the stock page has to
        // work when the theme is not installed at all.
        setSettingsGate(true);
        dispatchSettingsEvent('af-settings-show', page);

        // Push history so the back button navigates away from this page
        history.pushState({ clientSettings: true }, '');

        dispatchPageEvents(page, false);

        // Tear down when navigating away. jellyfin-web's router fires
        // HISTORY_UPDATE on document._callbacks for every navigation.
        function teardown() {
            const cbs = document._callbacks && document._callbacks['HISTORY_UPDATE'];
            if (cbs) {
                const idx = cbs.indexOf(teardown);
                if (idx !== -1) cbs.splice(idx, 1);
            }
            page.dispatchEvent(new CustomEvent('viewbeforehide', { bubbles: true }));
            page.dispatchEvent(new CustomEvent('viewhide', { bubbles: true }));
            // Before the removal: the listener reads the page it decorated.
            dispatchSettingsEvent('af-settings-hide', page);
            setSettingsGate(false);
            page.remove();

            if (reactContainer) reactContainer.classList.remove('hide');
            for (const p of visiblePages) p.classList.remove('hide');

            // The React <Page> component was never unmounted (just CSS-hidden),
            // so its useEffect won't re-fire pageshow to update the header.
            // Find the active React page and re-dispatch pageshow for it.
            if (reactContainer) {
                const activePage = reactContainer.querySelector('[data-role="page"]');
                if (activePage) dispatchPageEvents(activePage, true);
            }
        }
        document._callbacks = document._callbacks || {};
        document._callbacks['HISTORY_UPDATE'] = document._callbacks['HISTORY_UPDATE'] || [];
        document._callbacks['HISTORY_UPDATE'].push(teardown);
    }

    // Codec checkbox/reorder widget. Renders one row per codec — enabled
    // codecs first in user-preference order, then the remaining mpv-supported
    // codecs at the bottom. Toggling a row enables/disables it; ↑/↓ reorder
    // within the enabled set. Every change re-emits the ordered enabled list.
    function renderCodecList({ enabled, all, onChange }) {
        const widget = document.createElement('div');
        widget.className = 'codecList';
        widget.style.cssText = 'border:1px solid rgba(255,255,255,0.15); border-radius:4px; padding:0.5em;';

        // Working state: list of {codec, enabled} in display order.
        const enabledSet = new Set(enabled || []);
        const allSet = new Set(all || []);
        const rows = [];
        for (const c of (enabled || []))     if (allSet.has(c)) rows.push({ codec: c, enabled: true });
        for (const c of (all || []))         if (!enabledSet.has(c)) rows.push({ codec: c, enabled: false });

        function rerender() {
            widget.replaceChildren();
            const lastEnabledIdx = rows.reduce((acc, r, i) => r.enabled ? i : acc, -1);
            rows.forEach((row, idx) => {
                const r = document.createElement('div');
                r.style.cssText = 'display:flex; align-items:center; gap:0.5em; padding:0.25em 0;';

                const cb = document.createElement('input');
                cb.type = 'checkbox';
                cb.checked = row.enabled;
                cb.addEventListener('change', () => {
                    row.enabled = cb.checked;
                    if (cb.checked) {
                        rows.splice(idx, 1);
                        rows.splice(lastEnabledIdx + 1, 0, row);
                    } else {
                        rows.splice(idx, 1);
                        const newLastEnabled = rows.reduce((acc, r, i) => r.enabled ? i : acc, -1);
                        rows.splice(newLastEnabled + 1, 0, row);
                    }
                    emit();
                    rerender();
                });
                r.appendChild(cb);

                const name = document.createElement('span');
                name.textContent = row.codec;
                name.style.flex = '1';
                r.appendChild(name);

                const up = document.createElement('button');
                up.type = 'button';
                up.textContent = '↑';
                up.disabled = !row.enabled || idx === 0 || !rows[idx - 1].enabled;
                up.addEventListener('click', () => {
                    [rows[idx - 1], rows[idx]] = [rows[idx], rows[idx - 1]];
                    emit();
                    rerender();
                });
                r.appendChild(up);

                const down = document.createElement('button');
                down.type = 'button';
                down.textContent = '↓';
                down.disabled = !row.enabled || idx >= lastEnabledIdx;
                down.addEventListener('click', () => {
                    [rows[idx], rows[idx + 1]] = [rows[idx + 1], rows[idx]];
                    emit();
                    rerender();
                });
                r.appendChild(down);

                widget.appendChild(r);
            });
        }

        function emit() {
            onChange(rows.filter(r => r.enabled).map(r => r.codec));
        }

        rerender();
        return widget;
    }

    // Populate the settings form with controls driven by window.jmpInfo.
    function buildSettingsForm(form) {
        const jmpInfo = window.jmpInfo;

        const notice = document.createElement('div');
        notice.className = 'infoBanner';
        notice.setAttribute('data-af-notice', '');
        notice.textContent = 'Changes take effect after restarting the application.';
        form.appendChild(notice);

        for (const sectionOrder of jmpInfo.sections.sort((a, b) => a.order - b.order)) {
            const section = sectionOrder.key;
            const values = jmpInfo.settings[section];
            const descriptions = jmpInfo.settingsDescriptions[section];
            if (!descriptions || !descriptions.length) continue;

            const group = document.createElement('div');
            group.className = 'verticalSection';
            group.setAttribute('data-af-section', section);
            form.appendChild(group);

            const sectionHeader = document.createElement('h2');
            sectionHeader.className = 'sectionTitle';
            sectionHeader.textContent = section.charAt(0).toUpperCase() + section.slice(1);
            group.appendChild(sectionHeader);

            for (const setting of descriptions) {
                const container = document.createElement('div');
                // Stable hooks for astrofin-settings.js and the theme sheet:
                // the generated control ids are non-semantic (`embyselect0`),
                // so the key and its section are stamped on the container.
                // `data-af-applies` is the LIVE/RESTART tag — only videoMode
                // is applied to the running mpv (see video_mode.rs); every
                // other setting is read at boot.
                container.setAttribute('data-af-setting', setting.key);
                container.setAttribute('data-af-section', section);
                container.setAttribute(
                    'data-af-applies', setting.key === 'videoMode' ? 'live' : 'restart');

                if (setting.options) {
                    container.className = 'selectContainer';
                    const control = document.createElement('select');
                    control.setAttribute('is', 'emby-select');
                    control.className = 'emby-select-withcolor';
                    control.setAttribute('label', setting.displayName);
                    for (const option of setting.options) {
                        const val = typeof option === 'string' ? option : option.value;
                        const optTitle = typeof option === 'string' ? option : option.title;
                        const opt = document.createElement('option');
                        if (val !== null) opt.value = val;
                        opt.selected = val === null
                            ? values[setting.key] == null
                            : String(val) === String(values[setting.key]);
                        opt.textContent = optTitle;
                        control.appendChild(opt);
                    }
                    control.addEventListener('change', () => {
                        // A null option value can't round-trip through control.value (DOM strings).
                        const selected = setting.options[control.selectedIndex];
                        const value = typeof selected === 'string' ? selected : selected.value;
                        jmpInfo.settings[section][setting.key] = value;
                        window.api.settings.setValue(section, setting.key, value);
                    });
                    container.appendChild(control);
                    if (setting.help) {
                        const helpText = document.createElement('div');
                        helpText.className = 'fieldDescription';
                        helpText.textContent = setting.help;
                        container.appendChild(helpText);
                    }
                } else if (setting.inputType === 'codecList') {
                    container.className = 'inputContainer';
                    const labelText = document.createElement('label');
                    labelText.className = 'inputLabel';
                    labelText.textContent = setting.displayName;
                    container.appendChild(labelText);
                    const widget = renderCodecList({
                        enabled: values[setting.key],
                        all: jmpInfo[setting.codecListSource],
                        onChange: (enabledOrdered) => {
                            jmpInfo.settings[section][setting.key] = enabledOrdered;
                            window.api.settings.setValue(section, setting.key, enabledOrdered);
                        }
                    });
                    container.appendChild(widget);
                    if (setting.help) {
                        const helpText = document.createElement('div');
                        helpText.className = 'fieldDescription';
                        helpText.textContent = setting.help;
                        container.appendChild(helpText);
                    }
                } else if (setting.inputType === 'text' || setting.inputType === 'textarea') {
                    const isTextarea = setting.inputType === 'textarea';
                    container.className = 'inputContainer';
                    const labelText = document.createElement('label');
                    labelText.className = 'inputLabel';
                    labelText.textContent = setting.displayName;
                    container.appendChild(labelText);
                    const control = document.createElement(isTextarea ? 'textarea' : 'input');
                    control.className = 'emby-input';
                    control.value = values[setting.key] || '';
                    if (isTextarea) {
                        control.style.resize = 'none';
                        control.rows = 2;
                    } else {
                        control.type = 'text';
                        if (setting.placeholder) control.placeholder = setting.placeholder;
                        if (setting.maxLength) control.maxLength = setting.maxLength;
                    }
                    control.addEventListener('change', () => {
                        jmpInfo.settings[section][setting.key] = control.value;
                        window.api.settings.setValue(section, setting.key, control.value);
                    });
                    container.appendChild(control);
                    if (setting.help) {
                        const helpText = document.createElement('div');
                        helpText.className = 'fieldDescription';
                        helpText.textContent = setting.help;
                        container.appendChild(helpText);
                    }
                } else {
                    container.className = setting.help
                        ? 'checkboxContainer checkboxContainer-withDescription'
                        : 'checkboxContainer';
                    const lbl = document.createElement('label');
                    const control = document.createElement('input');
                    control.type = 'checkbox';
                    control.setAttribute('is', 'emby-checkbox');
                    control.checked = !!values[setting.key];
                    control.addEventListener('change', () => {
                        jmpInfo.settings[section][setting.key] = control.checked;
                        window.api.settings.setValue(section, setting.key, control.checked);
                    });
                    lbl.appendChild(control);
                    const checkSpan = document.createElement('span');
                    checkSpan.className = 'checkboxLabel';
                    checkSpan.textContent = setting.displayName;
                    lbl.appendChild(checkSpan);
                    container.appendChild(lbl);
                    if (setting.help) {
                        const helpText = document.createElement('div');
                        helpText.className = 'fieldDescription checkboxFieldDescription';
                        helpText.textContent = setting.help;
                        container.appendChild(helpText);
                    }
                }

                group.appendChild(container);
            }
        }

        // Open mpv config button
        if (jmpInfo.settings.main && jmpInfo.settings.main.userWebClient) {
            const group = document.createElement('div');
            group.className = 'verticalSection';
            group.setAttribute('data-af-section', 'mpv');
            form.appendChild(group);

            const sectionHeader = document.createElement('h2');
            sectionHeader.className = 'sectionTitle';
            sectionHeader.textContent = 'MPV config';
            group.appendChild(sectionHeader);

            const btn = document.createElement('button');
            btn.className = 'raised button-cancel block emby-button';
            btn.setAttribute('data-af-action', 'open-config-dir');
            btn.textContent = 'Open mpv config directory';
            btn.type = 'button';
            btn.addEventListener('click', () => {
                if (window.jmpNative && window.jmpNative.openConfigDir) {
                    console.debug('[SETTINGS] called openConfigDir');
                    window.jmpNative.openConfigDir();
                }
            });
            group.appendChild(btn);
        }

        // Reset server button
        if (jmpInfo.settings.main && jmpInfo.settings.main.userWebClient) {
            const group = document.createElement('div');
            group.className = 'verticalSection';
            group.setAttribute('data-af-section', 'server');
            form.appendChild(group);

            const sectionHeader = document.createElement('h2');
            sectionHeader.className = 'sectionTitle';
            sectionHeader.textContent = 'Server';
            group.appendChild(sectionHeader);

            const btn = document.createElement('button');
            btn.className = 'raised button-cancel block emby-button';
            btn.setAttribute('data-af-action', 'reset-server');
            // "Reset Saved Server" read like a destructive server-side action;
            // it only forgets the address this client connects to.
            btn.textContent = 'Sign out of this server';
            // Without this it is a submit button inside the settings form.
            btn.type = 'button';
            btn.addEventListener('click', () => {
                jmpInfo.settings.main.userWebClient = '';
                if (window.jmpNative && window.jmpNative.saveServerUrl) {
                    window.jmpNative.saveServerUrl('');
                }
                window.location.reload();
            });
            group.appendChild(btn);
        }
    }

    window._openClientSettings = showSettingsPage;

    // Test hook (see src/web/client-settings.test.js); a no-op in the browser,
    // where `module` is undefined.
    if (typeof module !== 'undefined' && module.exports) {
        module.exports = {
            showSettingsPage,
            buildSettingsForm,
            renderCodecList,
            dispatchPageEvents,
            setSettingsGate,
            dispatchSettingsEvent
        };
    }
})();
