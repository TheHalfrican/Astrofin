// About panel. Rendered in a closed Shadow DOM for style/DOM isolation.
// `window._aboutData` is prepended to this file by the browser-side resource
// handler (src/jfn_cef/src/resource.rs) — do NOT depend on any IPC to
// arrive before first paint.
//
// Styling comes from the Astrofin token sheet: about.html links
// astrofin-tokens.css and astrofin-fonts.css into the document, and both cross
// the shadow boundary (custom properties inherit; @font-face is document-wide),
// so everything below can reference var(--af-*) directly. The fallbacks in
// var() keep the panel legible if either sheet ever fails to load.
(function () {
    var data = window._aboutData || {};

    var host = document.createElement('div');
    host.id = '_jabout';
    host.style.cssText =
        'position:fixed;left:0;top:0;width:100vw;height:100vh;' +
        'z-index:2147483647';
    var shadow = host.attachShadow({ mode: 'closed' });

    var style = document.createElement('style');
    style.textContent = `
*{margin:0;padding:0;box-sizing:border-box}

/* ---- backdrop: deep space with two drifting starfield layers ---- */
.bg{position:fixed;inset:0;background:rgba(var(--af-bg-base-rgb,7,10,20),.94);overflow:hidden}
.stars{position:absolute;inset:-200px;background-repeat:repeat;will-change:transform}
.stars-1{
  background-image:var(--af-stars-1);
  background-size:520px 520px;
  opacity:var(--af-stars-1-opacity,.7);
  animation:af-drift-1 var(--af-dur-drift-1,90s) linear infinite alternate;
}
.stars-2{
  background-image:var(--af-stars-2);
  background-size:760px 760px;
  opacity:var(--af-stars-2-opacity,.35);
  animation:af-drift-2 var(--af-dur-drift-2,150s) linear infinite alternate;
}
.nebula{position:absolute;right:-10%;top:-18%;width:70%;height:70%;
  background:var(--af-nebula-glow);pointer-events:none}
@keyframes af-drift-1{
  from{transform:translate3d(0,0,0)}
  to{transform:translate3d(-60px,-24px,0)}
}
@keyframes af-drift-2{
  from{transform:translate3d(0,0,0)}
  to{transform:translate3d(-140px,18px,0)}
}

/* ---- frosted glass card ---- */
.wrap{position:fixed;inset:0;display:flex;align-items:center;justify-content:center;
  padding:var(--af-space-6,24px)}
.box{position:relative;
  min-width:min(520px,92vw);max-width:min(640px,92vw);
  padding:var(--af-space-8,32px) var(--af-space-8,32px) var(--af-space-6,24px);
  background:var(--af-glass,linear-gradient(135deg,rgba(22,28,51,.72),rgba(11,15,32,.55)));
  backdrop-filter:blur(var(--af-glass-blur,20px));
  -webkit-backdrop-filter:blur(var(--af-glass-blur,20px));
  border:1px solid var(--af-edge-luminous,rgba(159,180,255,.18));
  border-radius:var(--af-radius-panel,22px);
  box-shadow:0 24px 80px rgba(0,0,0,.55);
  color:var(--af-text-primary,#EAF0FF);
  font:var(--af-type-body,400 16px/24px "Inter",system-ui,sans-serif)}

/* ---- header: logo mark + wordmark ---- */
.head{display:flex;align-items:center;gap:var(--af-space-3,12px);
  padding-bottom:var(--af-space-6,24px);
  border-bottom:1px solid var(--af-hairline,rgba(159,180,255,.10));
  margin-bottom:var(--af-space-6,24px)}
.logo{display:block;width:36px;height:36px;flex:0 0 auto}
.wordmark{font:var(--af-type-wordmark,300 20px/1 "Sora",system-ui,sans-serif);
  letter-spacing:var(--af-tracking-wordmark,.22em);
  margin-right:calc(-1 * var(--af-tracking-wordmark,.22em));
  text-transform:uppercase;color:var(--af-text-primary,#EAF0FF)}

/* ---- close ---- */
.x{position:absolute;top:var(--af-space-3,12px);right:var(--af-space-3,12px);
  width:32px;height:32px;display:flex;align-items:center;justify-content:center;
  appearance:none;background:transparent;cursor:pointer;
  border:1px solid transparent;border-radius:var(--af-radius-pill,999px);
  font:400 18px/1 var(--af-font-body,system-ui,sans-serif);
  color:var(--af-text-muted,#9AA6C8);
  transition:color var(--af-dur-ring-in,140ms) var(--af-ease-focus,ease-out),
             background-color var(--af-dur-ring-in,140ms) var(--af-ease-focus,ease-out);
  user-select:none}
.x:hover{background:rgba(var(--af-text-primary-rgb,234,240,255),.08);
  color:var(--af-text-primary,#EAF0FF)}
.x:focus-visible{outline:none;color:var(--af-text-primary,#EAF0FF);
  box-shadow:var(--af-focus-ring,0 0 0 3px #6FE3FF)}

/* ---- data rows ---- */
.rows{display:flex;flex-direction:column;gap:var(--af-space-3,12px)}
.row{display:flex;align-items:baseline;gap:var(--af-space-4,16px)}
.row .k{flex:0 0 176px;
  font:var(--af-type-eyebrow,600 13px/1 "Inter",system-ui,sans-serif);
  letter-spacing:var(--af-tracking-eyebrow,.14em);text-transform:uppercase;
  color:var(--af-text-muted,#9AA6C8)}
.row .v{flex:1 1 auto;min-width:0;word-break:break-all;
  font:var(--af-type-meta,500 14px/1 "IBM Plex Mono",ui-monospace,monospace);
  line-height:20px;letter-spacing:var(--af-tracking-meta,.06em);
  color:var(--af-text-primary,#EAF0FF)}
.row .v.text{font:var(--af-type-body,400 16px/24px "Inter",system-ui,sans-serif);
  letter-spacing:normal;word-break:normal}
.path{appearance:none;background:none;border:0;padding:0;text-align:left;
  cursor:pointer;color:var(--af-accent-primary,#6FE3FF);
  text-decoration:underline;text-underline-offset:3px;
  text-decoration-color:rgba(var(--af-accent-primary-rgb,111,227,255),.45);
  transition:text-decoration-color var(--af-dur-ring-in,140ms) var(--af-ease-focus,ease-out)}
.path:hover{text-decoration-color:currentColor}
.path:focus-visible{outline:none;border-radius:var(--af-radius-xs,4px);
  box-shadow:var(--af-focus-ring,0 0 0 3px #6FE3FF)}

@media (prefers-reduced-motion:reduce){
  .stars{animation:none}
}
`;
    shadow.appendChild(style);

    var bg = document.createElement('div');
    bg.className = 'bg';
    var stars1 = document.createElement('div');
    stars1.className = 'stars stars-1';
    var stars2 = document.createElement('div');
    stars2.className = 'stars stars-2';
    var nebula = document.createElement('div');
    nebula.className = 'nebula';
    bg.appendChild(stars1);
    bg.appendChild(stars2);
    bg.appendChild(nebula);
    shadow.appendChild(bg);

    var wrap = document.createElement('div');
    wrap.className = 'wrap';
    shadow.appendChild(wrap);

    var box = document.createElement('div');
    box.className = 'box';
    box.setAttribute('role', 'dialog');
    box.setAttribute('aria-modal', 'true');
    box.setAttribute('aria-label', 'About Astrofin');
    wrap.appendChild(box);

    var head = document.createElement('div');
    head.className = 'head';
    var logo = document.createElement('img');
    logo.className = 'logo';
    logo.src = 'logo-mark.svg';
    logo.alt = 'Astrofin';
    var wordmark = document.createElement('div');
    wordmark.className = 'wordmark';
    wordmark.textContent = 'Astrofin';
    var xBtn = document.createElement('button');
    xBtn.className = 'x';
    xBtn.type = 'button';
    xBtn.setAttribute('aria-label', 'Close');
    xBtn.textContent = '×';
    head.appendChild(logo);
    head.appendChild(wordmark);
    box.appendChild(head);
    box.appendChild(xBtn);

    var rows = document.createElement('div');
    rows.className = 'rows';
    box.appendChild(rows);

    function addRow(label, value, isPath) {
        var row = document.createElement('div');
        row.className = 'row';
        var k = document.createElement('div');
        k.className = 'k';
        k.textContent = label;
        var v;
        if (isPath && value) {
            v = document.createElement('button');
            v.type = 'button';
            v.className = 'v path';
            v.textContent = value;
            v.addEventListener('click', function () {
                if (window.jmpNative) jmpNative.aboutOpenPath(value);
            });
        } else {
            v = document.createElement('div');
            v.className = 'v';
            v.textContent = value || '';
        }
        row.appendChild(k);
        row.appendChild(v);
        rows.appendChild(row);
        return v;
    }

    addRow('App version', data.app, false);
    addRow('CEF version', data.cef, false);
    // GPL-2.0 section 2(a): the upstream credit must stay exactly as given.
    if (data.basedOn) addRow('Based on', data.basedOn, false).classList.add('text');
    if (data.configDir) addRow('Config directory', data.configDir, true);
    if (data.logFile) addRow('Current log file', data.logFile, true);

    var dismissed = false;
    function dismiss() {
        if (dismissed) return;
        dismissed = true;
        window.removeEventListener('keydown', onKeyDown, true);
        host.remove();
        if (window.jmpNative) jmpNative.aboutDismiss();
    }

    function onKeyDown(e) {
        if (e.key === 'Escape') { e.preventDefault(); dismiss(); }
    }

    xBtn.addEventListener('click', dismiss);
    bg.addEventListener('mousedown', function (e) { e.preventDefault(); dismiss(); });
    wrap.addEventListener('mousedown', function (e) { e.preventDefault(); dismiss(); });
    // Stop backdrop click-through from firing when clicking inside the box.
    box.addEventListener('mousedown', function (e) { e.stopPropagation(); });
    window.addEventListener('keydown', onKeyDown, true);

    document.body.appendChild(host);
})();
