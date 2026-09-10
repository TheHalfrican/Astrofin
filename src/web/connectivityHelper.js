// Connectivity helper - uses native C++ for HTTP requests (no CORS issues)
window.jmpCheckServerConnectivity = (() => {
    let pendingResolve = null;
    let pendingReject = null;
    let pendingUrl = null;

    // Called by native code when result is ready.
    //
    // `detail` is native's fourth slot: on success a notice key for the
    // connect screen ('insecure-http' when the address was typed without a
    // scheme, the https attempt failed and the connection is therefore plain
    // http), on failure a message to show instead of the generic one (a
    // refused redirect names the address to enter instead). Empty for
    // everything that needs no comment.
    window._onServerConnectivityResult = (url, success, resolvedUrl, detail) => {
        console.debug('Connectivity result:', url, success, resolvedUrl, detail);
        // `pendingUrl` starts as null, so a null-url result would match on its
        // own and call a resolver that is not there yet.
        if (pendingUrl === url && pendingResolve) {
            if (success) {
                // The note is delivered beside the promise, not through it:
                // the resolve value stays the URL the caller saves.
                if (detail && typeof checkFunc.onNotice === 'function') {
                    checkFunc.onNotice(detail);
                }
                pendingResolve(resolvedUrl);
            } else {
                const failure = new Error(detail || 'Connection failed');
                // Only a reason native actually gave; the connect screen
                // shows its own localised text for a plain failure.
                if (detail) failure.detail = detail;
                pendingReject(failure);
            }
            pendingResolve = null;
            pendingReject = null;
            pendingUrl = null;
        }
    };

    const checkFunc = async function(url) {
        // Wait for jmpNative
        let attempts = 0;
        while (!window.jmpNative?.checkServerConnectivity && attempts < 50) {
            await new Promise(resolve => setTimeout(resolve, 100));
            attempts++;
        }
        if (!window.jmpNative?.checkServerConnectivity) {
            throw new Error('Native connectivity check not available');
        }

        return new Promise((resolve, reject) => {
            pendingResolve = resolve;
            pendingReject = reject;
            pendingUrl = url;

            console.debug('Checking connectivity:', url);
            window.jmpNative.checkServerConnectivity(url);
        });
    };

    // Set by the connect screen to hear about a successful probe that came
    // with something to say (today: the plain-http fallback). Never called
    // for a probe that has nothing to report.
    checkFunc.onNotice = null;

    checkFunc.abort = () => {
        if (window.jmpNative?.cancelServerConnectivity) {
            window.jmpNative.cancelServerConnectivity();
        }
        if (pendingReject) {
            const cancelled = new Error('Connection cancelled');
            // The connect screen must never report a user cancel as a
            // failure; the flag says so without matching on the message,
            // which a native detail string could otherwise collide with.
            cancelled.cancelled = true;
            pendingReject(cancelled);
            pendingResolve = null;
            pendingReject = null;
            pendingUrl = null;
        }
    };

    return checkFunc;
})();

// Test hook: `require()` of this file yields the installed window API. In the
// browser `module` is undefined, so this is a no-op there.
if (typeof module !== 'undefined' && module.exports) {
    module.exports = window.jmpCheckServerConnectivity;
}
