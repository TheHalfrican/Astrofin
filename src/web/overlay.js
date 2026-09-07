let cancelWait = null;

// Saved server URL comes from the native side via IPC. Fire the request at
// script load; the reply arrives through _onSavedServerUrl (defined below)
// and resolves savedServerUrlReady.
let savedServerUrl = null;
const savedServerUrlReady = new Promise((resolve) => {
    window._onSavedServerUrl = (url) => {
        savedServerUrl = url || null;
        resolve(savedServerUrl);
    };
});
window.jmpNative.getSavedServerUrl();

// True whenever the main browser is loading the URL we currently care about.
// Set by the auto-connect path once we know a saved URL exists (main.cpp
// has already pre-loaded it), by navigateMain on user-initiated success,
// cleared whenever native resets main (cancel or user edits URL).
let mainLoaded = false;

// Sleep for `ms` milliseconds, rejecting if cancelWait() is called first.
// Stores the cancel hook in the module-level `cancelWait` so a single cancel
// path handles whichever wait is currently outstanding.
function cancellableDelay(ms, label) {
    return new Promise((resolve, reject) => {
        const t = setTimeout(() => { cancelWait = null; resolve(); }, ms);
        cancelWait = () => {
            console.debug('Cancelling ' + label + ' timer', t);
            clearTimeout(t);
            cancelWait = null;
            reject(new Error('cancelled'));
        };
    });
}

async function tryConnect(server, spinnerStartTime = Date.now()) {
    try {
        console.debug("Checking connectivity to:", server);

        const resolvedUrl = await window.jmpCheckServerConnectivity(server);
        console.log("Server connectivity check passed");
        console.debug("Resolved URL:", resolvedUrl);

        if (!isConnecting) return false;

        // Save the normalized URL returned by native, not the raw user input.
        savedServerUrl = resolvedUrl;
        if (window.jmpNative && window.jmpNative.saveServerUrl) {
            window.jmpNative.saveServerUrl(resolvedUrl);
        }

        // Kick off main-browser navigation immediately, then wait long enough
        // to satisfy both constraints simultaneously: spinner visible ≥1s AND
        // main browser has ≥1s to render after navigate. For fast probes this
        // saves up to ~900ms compared to running the two waits sequentially.
        // Skip when main is already loading (startup pre-load or prior nav).
        if (!mainLoaded) {
            if (window.jmpNative && window.jmpNative.navigateMain) {
                window.jmpNative.navigateMain(resolvedUrl);
                mainLoaded = true;
            } else {
                console.error("navigateMain IPC not available");
                return false;
            }
        }

        const elapsed = Date.now() - spinnerStartTime;
        const waitMs = Math.max(1000 - elapsed, 1000);
        await cancellableDelay(waitMs, 'pre-fade');
        if (!isConnecting) return false;

        if (window.jmpNative && window.jmpNative.dismissOverlay) {
            window.jmpNative.dismissOverlay();
            const onEnd = (e) => {
                if (e.animationName !== 'fadeOut') return;
                document.body.removeEventListener('animationend', onEnd);
                window.close();
            };
            document.body.addEventListener('animationend', onEnd);
            document.body.classList.add('fade-out');
        }
        return true;
    } catch (e) {
        if (/cancel/i.test(e && e.message)) {
            console.debug("Connection cancelled");
        } else {
            console.error("Server connectivity check failed:", e);
        }
        return false;
    }
}

let isConnecting = false;
// Set by cancelConnection so the "connect failed" dialog is not raised for a
// stop the user asked for.
let userCancelled = false;

// Single switch for the screen's three visual states; overlay.css keys every
// show/hide off body[data-state] so the markup never carries inline styles.
//   'boot'       – waiting on getSavedServerUrl (orbit only)
//   'connecting' – probing a server (orbit + address + cancel)
//   'idle'       – the form is live
const setState = (state) => {
    document.body.dataset.state = state;
};

const updateButtonState = () => {
    const address = document.getElementById('address');
    const button = document.getElementById('connect-button');
    const hasValue = address.value.trim().length > 0;

    if (!isConnecting) {
        button.disabled = !hasValue;
    }
};

const cancelOnEscape = (e) => {
    if (isConnecting && e.key === 'Escape') {
        cancelConnection();
    }
};

const showConnectionFailedDialog = () => {
    const dialog = document.createElement('div');
    dialog.className = 'dialog';
    dialog.setAttribute('role', 'alertdialog');
    dialog.setAttribute('aria-modal', 'true');

    // The glass panel is a child of the scrim so the scrim can blur the
    // starfield behind it without blurring the panel's own contents.
    const panel = document.createElement('div');
    panel.className = 'dialog-panel scaleIn';

    const header = document.createElement('h1');
    header.innerText = headerConnectionFailureText;

    const message = document.createElement('div');
    message.innerText = messageUnableToConnectToServerText;
    message.className = 'dialog-message';

    const button = document.createElement('button');
    button.innerText = buttonGotItText;
    button.type = 'button';
    button.className = 'dialog-button af-secondary';

    const dismiss = () => {
        document.removeEventListener('keydown', onDialogKey);
        dialog.remove();
        const address = document.getElementById('address');
        if (!isConnecting && address) address.focus();
    };

    // Enter/Escape dismiss too: the app has to be usable from a remote.
    function onDialogKey(e) {
        if (e.key === 'Escape' || e.key === 'Enter') {
            e.preventDefault();
            e.stopPropagation();
            dismiss();
        }
    }

    button.addEventListener('click', dismiss);
    document.addEventListener('keydown', onDialogKey);

    panel.appendChild(header);
    panel.appendChild(message);
    panel.appendChild(button);
    dialog.appendChild(panel);
    document.body.appendChild(dialog);
    button.focus();
};

const startConnecting = async () => {
    const address = document.getElementById('address');
    const status = document.getElementById('connect-status');
    const server = address.value;

    // Show connecting UI
    isConnecting = true;
    userCancelled = false;
    // The URL under the orbit is the only feedback about *what* we are
    // reaching for; it needs no translation.
    status.textContent = server;
    address.disabled = true;
    setState('connecting');
    const spinnerStart = Date.now();
    document.addEventListener('keydown', cancelOnEscape);

    // C++ handles retries, just wait for result
    const connected = await tryConnect(server, spinnerStart);

    if (!connected) {
        isConnecting = false;
        address.disabled = false;
        setState('idle');
        document.removeEventListener('keydown', cancelOnEscape);
        updateButtonState();
        // An explicit cancel is not a failure; only report real ones.
        if (userCancelled) {
            address.focus();
        } else {
            showConnectionFailedDialog();
        }
    }
};

const cancelConnection = () => {
    if (!isConnecting) return;

    console.debug("Cancelling connection");
    userCancelled = true;
    // Native resets main on cancelServerConnectivity.
    mainLoaded = false;
    isConnecting = false;

    // Cancel C++ connectivity check and abort JS promise.
    // jmpCheckServerConnectivity.abort() calls jmpNative.cancelServerConnectivity
    // internally (see connectivityHelper.js).
    if (window.jmpCheckServerConnectivity.abort) {
        window.jmpCheckServerConnectivity.abort();
    }
    if (cancelWait) cancelWait();
};

// Button click handler
document.getElementById('connect-button').addEventListener('click', (e) => {
    e.preventDefault();
    e.stopPropagation();

    if (!e.target.disabled) {
        startConnecting();
    }
});

// Form submit handler
document.getElementById('connect-form').addEventListener('submit', (e) => {
    e.preventDefault();
    if (!isConnecting) {
        startConnecting();
    }
});

// Input change handler
document.getElementById('address').addEventListener('input', updateButtonState);

// Cancel affordance while connecting (Escape has always worked; the button
// makes it reachable with a pointer and on a TV remote).
const cancelButton = document.getElementById('cancel-button');
cancelButton.innerText = window.cancelButtonText || 'Cancel';
cancelButton.addEventListener('click', (e) => {
    e.preventDefault();
    cancelConnection();
});

// Enter key handler
document.addEventListener('keydown', (e) => {
    const address = document.getElementById('address');
    // The failure dialog owns Enter while it is up.
    if (document.querySelector('.dialog')) return;
    if (e.key === 'Enter' && !isConnecting && !address.disabled && address.value.trim()) {
        e.preventDefault();
        startConnecting();
    }
});

// Auto-connect on load
(async () => {
    console.log('Auto-connect: starting');

    const savedServer = await savedServerUrlReady;
    console.debug('Auto-connect: savedServer =', savedServer);

    if (savedServer) {
        console.debug('Auto-connect: checking saved server', savedServer);

        // main.cpp pre-loads the saved URL into the main browser in parallel
        // with overlay startup, so don't issue a redundant navigateMain.
        mainLoaded = true;

        const address = document.getElementById('address');

        // Set address value for potential display later
        address.value = savedServer;

        startConnecting();
    } else {
        setState('idle');
        document.getElementById('address').focus();
        updateButtonState();
    }
})();
