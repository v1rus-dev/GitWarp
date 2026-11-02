/* gitwarp mock runtime: icons, sample data, graph layout/render, diff render, overlays, window shell.
   This is design-prototype code; every behaviour here is a spec for the GPUI implementation. */
(function () {
  'use strict';
  const G = (window.GL = {});

  /* shell.css carries the window-shell, history and state styles; load it after gitwarp.css on every page that uses this runtime */
  if (!document.querySelector('link[href$="shell.css"]')) { const l = document.createElement('link'); l.rel = 'stylesheet'; l.href = 'shell.css'; document.head.appendChild(l); }

  /* ---------- theme ---------- */
  G.THEMES = { dark: 'Dark', light: 'Light' };
  try { G.theme = localStorage.getItem('gl-theme') || 'dark'; } catch (e) { G.theme = 'dark'; }
  if (!G.THEMES[G.theme]) G.theme = 'dark';
  document.documentElement.dataset.theme = G.theme;
  G.setTheme = (t) => { G.theme = t; document.documentElement.dataset.theme = t; try { localStorage.setItem('gl-theme', t); } catch (e) {} };

  /* ---------- helpers ---------- */
  G.esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  G.$ = (s, r) => (r || document).querySelector(s);
  G.$$ = (s, r) => Array.from((r || document).querySelectorAll(s));
  G.h = (html) => { const t = document.createElement('template'); t.innerHTML = html.trim(); return t.content.firstElementChild; };
  const esc = G.esc;

  /* ---------- icons: one family, 16px grid, 1.4px stroke, round caps ---------- */
  const ICONS = {
    branch: '<circle cx="4.5" cy="3.5" r="1.7"/><circle cx="4.5" cy="12.5" r="1.7"/><circle cx="11.5" cy="5.5" r="1.7"/><path d="M4.5 5.2v5.6M11.5 7.2c0 2.6-3.5 2-6.2 4"/>',
    tag: '<path d="M2.5 2.5h5l6 6-5 5-6-6z"/><circle cx="5.5" cy="5.5" r=".7"/>',
    cloud: '<path d="M4.5 12.5a3 3 0 0 1-.4-5.97A4 4 0 0 1 11.8 6.3 3.1 3.1 0 0 1 11.5 12.5z"/>',
    stash: '<path d="M2.5 4.5h11v3h-11zM3.5 7.5v5h9v-5M6.5 10h3"/>',
    box: '<path d="M8 1.8l5.5 3v6.4L8 14.2l-5.5-3V4.8zM2.5 4.8L8 7.8l5.5-3M8 7.8v6.4"/>',
    search: '<circle cx="7" cy="7" r="4.2"/><path d="M10.2 10.2l3.3 3.3"/>',
    down: '<path d="M8 2.5v8M4.8 7.5L8 10.7l3.2-3.2M3 13.5h10"/>',
    up: '<path d="M8 13.5v-8M4.8 8.5L8 5.3l3.2 3.2M3 2.5h10"/>',
    fetch: '<path d="M13 8a5 5 0 1 1-1.6-3.7M13 2.5v3h-3"/>',
    chevR: '<path d="M6 3.5L10.5 8 6 12.5"/>',
    chevD: '<path d="M3.5 6L8 10.5 12.5 6"/>',
    x: '<path d="M3.5 3.5l9 9M12.5 3.5l-9 9"/>',
    check: '<path d="M3 8.5l3.2 3.2L13 4.8"/>',
    warn: '<path d="M8 2.2l6.3 11H1.7zM8 6.5v3M8 11.6v.1"/>',
    err: '<circle cx="8" cy="8" r="6"/><path d="M8 4.8v3.6M8 10.8v.1"/>',
    info: '<circle cx="8" cy="8" r="6"/><path d="M8 7.2v4M8 4.9v.1"/>',
    folder: '<path d="M1.8 4.2h4.2l1.4 1.6h6.8v6.8H1.8z"/>',
    file: '<path d="M3.5 1.8h5.2l3.8 3.8v8.6h-9zM8.5 1.8v4h4"/>',
    plus: '<path d="M8 3v10M3 8h10"/>',
    minus: '<path d="M3 8h10"/>',
    gear: '<circle cx="8" cy="8" r="2"/><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M12.6 3.4l-1.4 1.4M4.8 11.2l-1.4 1.4"/>',
    panelR: '<rect x="2" y="3" width="12" height="10" rx="1"/><path d="M10 3v10"/>',
    panelB: '<rect x="2" y="3" width="12" height="10" rx="1"/><path d="M2 9.5h12"/>',
    sidebar: '<rect x="2" y="3" width="12" height="10" rx="1"/><path d="M6 3v10"/>',
    split: '<rect x="2" y="3" width="12" height="10" rx="1"/><path d="M8 3v10"/>',
    unified: '<rect x="2" y="3" width="12" height="10" rx="1"/><path d="M4.5 6.5h7M4.5 9.5h7"/>',
    git: '<circle cx="8" cy="8" r="2.2"/><path d="M1.5 8h4.3M10.2 8h4.3"/>',
    command: '<path d="M6 6v4h4V6zM6 6a2 2 0 1 0-2 2h2M10 6a2 2 0 1 1 2 2h-2M6 10a2 2 0 1 0-2-2M10 10a2 2 0 1 1 2-2"/>',
    copy: '<rect x="5.5" y="5.5" width="8" height="8" rx="1"/><path d="M10.5 5.5v-3h-8v8h3"/>',
    clone: '<path d="M8 2v7M5 6.5L8 9.5l3-3M2.5 10v3.5h11V10"/>',
    init: '<path d="M1.8 4.2h4.2l1.4 1.6h6.8v6.8H1.8zM8 7.5v3.5M6.2 9.25h3.6"/>',
    min: '<path d="M4 8h8"/>',
    max: '<rect x="3.5" y="3.5" width="9" height="9" rx="1"/>',
    undo: '<path d="M4 6.5h5.5a3 3 0 0 1 0 6H6M6.5 3.5L3.5 6.5l3 3"/>',
    edit: '<path d="M2.5 13.5l.7-3L10.8 3l2.2 2.2-7.6 7.6zM9.3 4.5l2.2 2.2"/>',
    trash: '<path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5h5.8l.6-8.5"/>',
    link: '<path d="M6.5 9.5l3-3M7 4.5l1-1a2.5 2.5 0 0 1 3.5 3.5l-1 1M9 11.5l-1 1A2.5 2.5 0 0 1 4.5 9l1-1"/>',
    lanes: '<path d="M4 2.5v11M8 2.5v11M12 2.5v11"/>',
    eye: '<path d="M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8z"/><circle cx="8" cy="8" r="1.8"/>',
    play: '<path d="M5 3.5v9l7-4.5z"/>',
    user: '<circle cx="8" cy="5.5" r="2.5"/><path d="M3 13.5c.6-2.6 2.5-3.8 5-3.8s4.4 1.2 5 3.8"/>',
    hash: '<path d="M6 2.5L5 13.5M11 2.5L10 13.5M3 6h10M2.5 10h10"/>',
    keyboard: '<rect x="1.5" y="4" width="13" height="8" rx="1"/><path d="M4 7h.1M6.5 7h.1M9 7h.1M11.5 7h.1M4.5 9.5h7"/>',
    key: '<circle cx="5" cy="10.5" r="2.6"/><path d="M7 8.7l6-6M11 4.7l1.8 1.8M9.2 6.5l1.5 1.5"/>',
    worktree: '<rect x="2" y="2.5" width="5" height="4" rx=".8"/><rect x="9" y="9.5" width="5" height="4" rx=".8"/><path d="M4.5 6.5V11H9"/>',
    lock: '<rect x="3.5" y="7" width="9" height="6.5" rx="1"/><path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2"/>',
    palette: '<path d="M8 2.2a5.8 5.8 0 1 0 0 11.6c1 0 1.4-.7 1-1.5-.5-1 .1-2 1.2-2H12a2.3 2.3 0 0 0 2.3-2.3C14.3 4.8 11.5 2.2 8 2.2z"/><circle cx="5" cy="7.5" r=".6"/><circle cx="7.5" cy="5" r=".6"/><circle cx="10.5" cy="6" r=".6"/>'
  };
  /* Octicons 19.42.0 (MIT, (c) GitHub Inc.) replace the hand-drawn set; git, keyboard, command and window controls stay our own. */
  const OCTI = { "branch": "<path d=\"M9.5 3.25a2.25 2.25 0 1 1 3 2.122V6A2.5 2.5 0 0 1 10 8.5H6a1 1 0 0 0-1 1v1.128a2.251 2.251 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.5 0v1.836A2.493 2.493 0 0 1 6 7h4a1 1 0 0 0 1-1v-.628A2.25 2.25 0 0 1 9.5 3.25Zm-6 0a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0Zm8.25-.75a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5ZM4.25 12a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Z\"/>", "check": "<path d=\"M13.78 4.22a.75.75 0 0 1 0 1.06l-7.25 7.25a.75.75 0 0 1-1.06 0L2.22 9.28a.751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018L6 10.94l6.72-6.72a.75.75 0 0 1 1.06 0Z\"/>", "chevD": "<path d=\"M12.78 5.22a.749.749 0 0 1 0 1.06l-4.25 4.25a.749.749 0 0 1-1.06 0L3.22 6.28a.749.749 0 1 1 1.06-1.06L8 8.939l3.72-3.719a.749.749 0 0 1 1.06 0Z\"/>", "chevR": "<path d=\"M6.22 3.22a.75.75 0 0 1 1.06 0l4.25 4.25a.75.75 0 0 1 0 1.06l-4.25 4.25a.751.751 0 0 1-1.042-.018.751.751 0 0 1-.018-1.042L9.94 8 6.22 4.28a.75.75 0 0 1 0-1.06Z\"/>", "clone": "<path d=\"M3.5 0A2.5 2.5 0 0 0 1 2.5v9A2.5 2.5 0 0 0 3.5 14h2.75a.75.75 0 0 0 0-1.5H3.5a1 1 0 0 1 0-2h2.75a.75.75 0 0 0 0-1.5H3.5c-.356 0-.694.074-1 .208V2.5a1 1 0 0 1 1-1h8v2.75a.75.75 0 0 0 1.5 0V.75a.75.75 0 0 0-.75-.75H3.5Z\"/><path d=\"M8 8.058C8 7.023 8.75 6 9.887 6h5.363a.75.75 0 0 1 .75.75v8.5a.75.75 0 0 1-.75.75h-5A2.25 2.25 0 0 1 8 13.75Zm6.5 3.442v-4H9.887c-.07 0-.156.031-.238.125a.663.663 0 0 0-.149.433v3.57c.235-.083.487-.128.75-.128Zm-5 2.25c0 .414.336.75.75.75h4.25V13h-4.25a.75.75 0 0 0-.75.75Z\"/>", "cloud": "<path d=\"M2 7.25A5.225 5.225 0 0 1 7.25 2a5.222 5.222 0 0 1 4.767 3.029A4.472 4.472 0 0 1 16 9.5c0 2.505-1.995 4.5-4.5 4.5h-8A3.474 3.474 0 0 1 0 10.5c0-1.41.809-2.614 2.001-3.17Zm1.54.482a.75.75 0 0 1-.556.832c-.86.22-1.484.987-1.484 1.936 0 1.124.876 2 2 2h8c1.676 0 3-1.324 3-3s-1.324-3-3-3a.75.75 0 0 1-.709-.504A3.72 3.72 0 0 0 7.25 3.5C5.16 3.5 3.5 5.16 3.5 7.25c.002.146.014.292.035.436l.004.036.001.008Z\"/>", "copy": "<path d=\"M0 6.75C0 5.784.784 5 1.75 5h1.5a.75.75 0 0 1 0 1.5h-1.5a.25.25 0 0 0-.25.25v7.5c0 .138.112.25.25.25h7.5a.25.25 0 0 0 .25-.25v-1.5a.75.75 0 0 1 1.5 0v1.5A1.75 1.75 0 0 1 9.25 16h-7.5A1.75 1.75 0 0 1 0 14.25Z\"/><path d=\"M5 1.75C5 .784 5.784 0 6.75 0h7.5C15.216 0 16 .784 16 1.75v7.5A1.75 1.75 0 0 1 14.25 11h-7.5A1.75 1.75 0 0 1 5 9.25Zm1.75-.25a.25.25 0 0 0-.25.25v7.5c0 .138.112.25.25.25h7.5a.25.25 0 0 0 .25-.25v-7.5a.25.25 0 0 0-.25-.25Z\"/>", "down": "<path d=\"M13.03 8.22a.75.75 0 0 1 0 1.06l-4.25 4.25a.75.75 0 0 1-1.06 0L3.47 9.28a.751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018l2.97 2.97V3.75a.75.75 0 0 1 1.5 0v7.44l2.97-2.97a.75.75 0 0 1 1.06 0Z\"/>", "up": "<path d=\"M3.47 7.78a.75.75 0 0 1 0-1.06l4.25-4.25a.75.75 0 0 1 1.06 0l4.25 4.25a.751.751 0 0 1-.018 1.042.751.751 0 0 1-1.042.018L9 4.81v7.44a.75.75 0 0 1-1.5 0V4.81L4.53 7.78a.75.75 0 0 1-1.06 0Z\"/>", "edit": "<path d=\"M11.013 1.427a1.75 1.75 0 0 1 2.474 0l1.086 1.086a1.75 1.75 0 0 1 0 2.474l-8.61 8.61c-.21.21-.47.364-.756.445l-3.251.93a.75.75 0 0 1-.927-.928l.929-3.25c.081-.286.235-.547.445-.758l8.61-8.61Zm.176 4.823L9.75 4.81l-6.286 6.287a.253.253 0 0 0-.064.108l-.558 1.953 1.953-.558a.253.253 0 0 0 .108-.064Zm1.238-3.763a.25.25 0 0 0-.354 0L10.811 3.75l1.439 1.44 1.263-1.263a.25.25 0 0 0 0-.354Z\"/>", "err": "<path d=\"M2.344 2.343h-.001a8 8 0 0 1 11.314 11.314A8.002 8.002 0 0 1 .234 10.089a8 8 0 0 1 2.11-7.746Zm1.06 10.253a6.5 6.5 0 1 0 9.108-9.275 6.5 6.5 0 0 0-9.108 9.275ZM6.03 4.97 8 6.94l1.97-1.97a.749.749 0 0 1 1.275.326.749.749 0 0 1-.215.734L9.06 8l1.97 1.97a.749.749 0 0 1-.326 1.275.749.749 0 0 1-.734-.215L8 9.06l-1.97 1.97a.749.749 0 0 1-1.275-.326.749.749 0 0 1 .215-.734L6.94 8 4.97 6.03a.751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018Z\"/>", "eye": "<path d=\"M8 2c1.981 0 3.671.992 4.933 2.078 1.27 1.091 2.187 2.345 2.637 3.023a1.62 1.62 0 0 1 0 1.798c-.45.678-1.367 1.932-2.637 3.023C11.67 13.008 9.981 14 8 14c-1.981 0-3.671-.992-4.933-2.078C1.797 10.83.88 9.576.43 8.898a1.62 1.62 0 0 1 0-1.798c.45-.677 1.367-1.931 2.637-3.022C4.33 2.992 6.019 2 8 2ZM1.679 7.932a.12.12 0 0 0 0 .136c.411.622 1.241 1.75 2.366 2.717C5.176 11.758 6.527 12.5 8 12.5c1.473 0 2.825-.742 3.955-1.715 1.124-.967 1.954-2.096 2.366-2.717a.12.12 0 0 0 0-.136c-.412-.621-1.242-1.75-2.366-2.717C10.824 4.242 9.473 3.5 8 3.5c-1.473 0-2.825.742-3.955 1.715-1.124.967-1.954 2.096-2.366 2.717ZM8 10a2 2 0 1 1-.001-3.999A2 2 0 0 1 8 10Z\"/>", "fetch": "<path d=\"M1.705 8.005a.75.75 0 0 1 .834.656 5.5 5.5 0 0 0 9.592 2.97l-1.204-1.204a.25.25 0 0 1 .177-.427h3.646a.25.25 0 0 1 .25.25v3.646a.25.25 0 0 1-.427.177l-1.38-1.38A7.002 7.002 0 0 1 1.05 8.84a.75.75 0 0 1 .656-.834ZM8 2.5a5.487 5.487 0 0 0-4.131 1.869l1.204 1.204A.25.25 0 0 1 4.896 6H1.25A.25.25 0 0 1 1 5.75V2.104a.25.25 0 0 1 .427-.177l1.38 1.38A7.002 7.002 0 0 1 14.95 7.16a.75.75 0 0 1-1.49.178A5.5 5.5 0 0 0 8 2.5Z\"/>", "file": "<path d=\"M2 1.75C2 .784 2.784 0 3.75 0h6.586c.464 0 .909.184 1.237.513l2.914 2.914c.329.328.513.773.513 1.237v9.586A1.75 1.75 0 0 1 13.25 16h-9.5A1.75 1.75 0 0 1 2 14.25Zm1.75-.25a.25.25 0 0 0-.25.25v12.5c0 .138.112.25.25.25h9.5a.25.25 0 0 0 .25-.25V6h-2.75A1.75 1.75 0 0 1 9 4.25V1.5Zm6.75.062V4.25c0 .138.112.25.25.25h2.688l-.011-.013-2.914-2.914-.013-.011Z\"/>", "folder": "<path d=\"M0 2.75C0 1.784.784 1 1.75 1H5c.55 0 1.07.26 1.4.7l.9 1.2a.25.25 0 0 0 .2.1h6.75c.966 0 1.75.784 1.75 1.75v8.5A1.75 1.75 0 0 1 14.25 15H1.75A1.75 1.75 0 0 1 0 13.25Zm1.75-.25a.25.25 0 0 0-.25.25v10.5c0 .138.112.25.25.25h12.5a.25.25 0 0 0 .25-.25v-8.5a.25.25 0 0 0-.25-.25H7.5c-.55 0-1.07-.26-1.4-.7l-.9-1.2a.25.25 0 0 0-.2-.1Z\"/>", "gear": "<path d=\"M8 0a8.2 8.2 0 0 1 .701.031C9.444.095 9.99.645 10.16 1.29l.288 1.107c.018.066.079.158.212.224.231.114.454.243.668.386.123.082.233.09.299.071l1.103-.303c.644-.176 1.392.021 1.82.63.27.385.506.792.704 1.218.315.675.111 1.422-.364 1.891l-.814.806c-.049.048-.098.147-.088.294.016.257.016.515 0 .772-.01.147.038.246.088.294l.814.806c.475.469.679 1.216.364 1.891a7.977 7.977 0 0 1-.704 1.217c-.428.61-1.176.807-1.82.63l-1.102-.302c-.067-.019-.177-.011-.3.071a5.909 5.909 0 0 1-.668.386c-.133.066-.194.158-.211.224l-.29 1.106c-.168.646-.715 1.196-1.458 1.26a8.006 8.006 0 0 1-1.402 0c-.743-.064-1.289-.614-1.458-1.26l-.289-1.106c-.018-.066-.079-.158-.212-.224a5.738 5.738 0 0 1-.668-.386c-.123-.082-.233-.09-.299-.071l-1.103.303c-.644.176-1.392-.021-1.82-.63a8.12 8.12 0 0 1-.704-1.218c-.315-.675-.111-1.422.363-1.891l.815-.806c.05-.048.098-.147.088-.294a6.214 6.214 0 0 1 0-.772c.01-.147-.038-.246-.088-.294l-.815-.806C.635 6.045.431 5.298.746 4.623a7.92 7.92 0 0 1 .704-1.217c.428-.61 1.176-.807 1.82-.63l1.102.302c.067.019.177.011.3-.071.214-.143.437-.272.668-.386.133-.066.194-.158.211-.224l.29-1.106C6.009.645 6.556.095 7.299.03 7.53.01 7.764 0 8 0Zm-.571 1.525c-.036.003-.108.036-.137.146l-.289 1.105c-.147.561-.549.967-.998 1.189-.173.086-.34.183-.5.29-.417.278-.97.423-1.529.27l-1.103-.303c-.109-.03-.175.016-.195.045-.22.312-.412.644-.573.99-.014.031-.021.11.059.19l.815.806c.411.406.562.957.53 1.456a4.709 4.709 0 0 0 0 .582c.032.499-.119 1.05-.53 1.456l-.815.806c-.081.08-.073.159-.059.19.162.346.353.677.573.989.02.03.085.076.195.046l1.102-.303c.56-.153 1.113-.008 1.53.27.161.107.328.204.501.29.447.222.85.629.997 1.189l.289 1.105c.029.109.101.143.137.146a6.6 6.6 0 0 0 1.142 0c.036-.003.108-.036.137-.146l.289-1.105c.147-.561.549-.967.998-1.189.173-.086.34-.183.5-.29.417-.278.97-.423 1.529-.27l1.103.303c.109.029.175-.016.195-.045.22-.313.411-.644.573-.99.014-.031.021-.11-.059-.19l-.815-.806c-.411-.406-.562-.957-.53-1.456a4.709 4.709 0 0 0 0-.582c-.032-.499.119-1.05.53-1.456l.815-.806c.081-.08.073-.159.059-.19a6.464 6.464 0 0 0-.573-.989c-.02-.03-.085-.076-.195-.046l-1.102.303c-.56.153-1.113.008-1.53-.27a4.44 4.44 0 0 0-.501-.29c-.447-.222-.85-.629-.997-1.189l-.289-1.105c-.029-.11-.101-.143-.137-.146a6.6 6.6 0 0 0-1.142 0ZM11 8a3 3 0 1 1-6 0 3 3 0 0 1 6 0ZM9.5 8a1.5 1.5 0 1 0-3.001.001A1.5 1.5 0 0 0 9.5 8Z\"/>", "hash": "<path d=\"M6.368 1.01a.75.75 0 0 1 .623.859L6.57 4.5h3.98l.46-2.868a.75.75 0 0 1 1.48.237L12.07 4.5h2.18a.75.75 0 0 1 0 1.5h-2.42l-.64 4h2.56a.75.75 0 0 1 0 1.5h-2.8l-.46 2.869a.75.75 0 0 1-1.48-.237l.42-2.632H5.45l-.46 2.869a.75.75 0 0 1-1.48-.237l.42-2.632H1.75a.75.75 0 0 1 0-1.5h2.42l.64-4H2.25a.75.75 0 0 1 0-1.5h2.8l.46-2.868a.75.75 0 0 1 .858-.622ZM9.67 10l.64-4H6.33l-.64 4Z\"/>", "info": "<path d=\"M0 8a8 8 0 1 1 16 0A8 8 0 0 1 0 8Zm8-6.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM6.5 7.75A.75.75 0 0 1 7.25 7h1a.75.75 0 0 1 .75.75v2.75h.25a.75.75 0 0 1 0 1.5h-2a.75.75 0 0 1 0-1.5h.25v-2h-.25a.75.75 0 0 1-.75-.75ZM8 6a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z\"/>", "init": "<path d=\"M2 2.5A2.5 2.5 0 0 1 4.5 0h8.75a.75.75 0 0 1 .75.75v12.5a.75.75 0 0 1-.75.75h-2.5a.75.75 0 0 1 0-1.5h1.75v-2h-8a1 1 0 0 0-.714 1.7.75.75 0 1 1-1.072 1.05A2.495 2.495 0 0 1 2 11.5Zm10.5-1h-8a1 1 0 0 0-1 1v6.708A2.486 2.486 0 0 1 4.5 9h8ZM5 12.25a.25.25 0 0 1 .25-.25h3.5a.25.25 0 0 1 .25.25v3.25a.25.25 0 0 1-.4.2l-1.45-1.087a.249.249 0 0 0-.3 0L5.4 15.7a.25.25 0 0 1-.4-.2Z\"/>", "key": "<path d=\"M10.5 0a5.499 5.499 0 1 1-1.288 10.848l-.932.932a.749.749 0 0 1-.53.22H7v.75a.749.749 0 0 1-.22.53l-.5.5a.749.749 0 0 1-.53.22H5v.75a.749.749 0 0 1-.22.53l-.5.5a.749.749 0 0 1-.53.22h-2A1.75 1.75 0 0 1 0 14.25v-2c0-.199.079-.389.22-.53l4.932-4.932A5.5 5.5 0 0 1 10.5 0Zm-4 5.5c-.001.431.069.86.205 1.269a.75.75 0 0 1-.181.768L1.5 12.56v1.69c0 .138.112.25.25.25h1.69l.06-.06v-1.19a.75.75 0 0 1 .75-.75h1.19l.06-.06v-1.19a.75.75 0 0 1 .75-.75h1.19l1.023-1.025a.75.75 0 0 1 .768-.18A4 4 0 1 0 6.5 5.5ZM11 6a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z\"/>", "lanes": "<path d=\"M2.75 0h2.5C6.216 0 7 .784 7 1.75v12.5A1.75 1.75 0 0 1 5.25 16h-2.5A1.75 1.75 0 0 1 1 14.25V1.75C1 .784 1.784 0 2.75 0Zm8 0h2.5C14.216 0 15 .784 15 1.75v12.5A1.75 1.75 0 0 1 13.25 16h-2.5A1.75 1.75 0 0 1 9 14.25V1.75C9 .784 9.784 0 10.75 0ZM2.5 1.75v12.5c0 .138.112.25.25.25h2.5a.25.25 0 0 0 .25-.25V1.75a.25.25 0 0 0-.25-.25h-2.5a.25.25 0 0 0-.25.25Zm8 0v12.5c0 .138.112.25.25.25h2.5a.25.25 0 0 0 .25-.25V1.75a.25.25 0 0 0-.25-.25h-2.5a.25.25 0 0 0-.25.25Z\"/>", "link": "<path d=\"m7.775 3.275 1.25-1.25a3.5 3.5 0 1 1 4.95 4.95l-2.5 2.5a3.5 3.5 0 0 1-4.95 0 .751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018 1.998 1.998 0 0 0 2.83 0l2.5-2.5a2.002 2.002 0 0 0-2.83-2.83l-1.25 1.25a.751.751 0 0 1-1.042-.018.751.751 0 0 1-.018-1.042Zm-4.69 9.64a1.998 1.998 0 0 0 2.83 0l1.25-1.25a.751.751 0 0 1 1.042.018.751.751 0 0 1 .018 1.042l-1.25 1.25a3.5 3.5 0 1 1-4.95-4.95l2.5-2.5a3.5 3.5 0 0 1 4.95 0 .751.751 0 0 1-.018 1.042.751.751 0 0 1-1.042.018 1.998 1.998 0 0 0-2.83 0l-2.5 2.5a1.998 1.998 0 0 0 0 2.83Z\"/>", "lock": "<path d=\"M4 4a4 4 0 0 1 8 0v2h.25c.966 0 1.75.784 1.75 1.75v5.5A1.75 1.75 0 0 1 12.25 15h-8.5A1.75 1.75 0 0 1 2 13.25v-5.5C2 6.784 2.784 6 3.75 6H4Zm8.25 3.5h-8.5a.25.25 0 0 0-.25.25v5.5c0 .138.112.25.25.25h8.5a.25.25 0 0 0 .25-.25v-5.5a.25.25 0 0 0-.25-.25ZM10.5 6V4a2.5 2.5 0 1 0-5 0v2Z\"/>", "minus": "<path d=\"M2 7.75A.75.75 0 0 1 2.75 7h10a.75.75 0 0 1 0 1.5h-10A.75.75 0 0 1 2 7.75Z\"/>", "palette": "<path d=\"M11.134 1.535c.7-.509 1.416-.942 2.076-1.155.649-.21 1.463-.267 2.069.34.603.601.568 1.411.368 2.07-.202.668-.624 1.39-1.125 2.096-1.011 1.424-2.496 2.987-3.775 4.249-1.098 1.084-2.132 1.839-3.04 2.3a3.744 3.744 0 0 1-1.055 3.217c-.431.431-1.065.691-1.657.861-.614.177-1.294.287-1.914.357A21.151 21.151 0 0 1 .797 16H.743l.007-.75H.749L.742 16a.75.75 0 0 1-.743-.742l.743-.008-.742.007v-.054a21.25 21.25 0 0 1 .13-2.284c.067-.647.187-1.287.358-1.914.17-.591.43-1.226.86-1.657a3.746 3.746 0 0 1 3.227-1.054c.466-.893 1.225-1.907 2.314-2.982 1.271-1.255 2.833-2.75 4.245-3.777ZM1.62 13.089c-.051.464-.086.929-.104 1.395.466-.018.932-.053 1.396-.104a10.511 10.511 0 0 0 1.668-.309c.526-.151.856-.325 1.011-.48a2.25 2.25 0 1 0-3.182-3.182c-.155.155-.329.485-.48 1.01a10.515 10.515 0 0 0-.309 1.67Zm10.396-10.34c-1.224.89-2.605 2.189-3.822 3.384l1.718 1.718c1.21-1.205 2.51-2.597 3.387-3.833.47-.662.78-1.227.912-1.662.134-.444.032-.551.009-.575h-.001V1.78c-.014-.014-.113-.113-.548.027-.432.14-.995.462-1.655.942Zm-4.832 7.266-.001.001a9.859 9.859 0 0 0 1.63-1.142L7.155 7.216a9.7 9.7 0 0 0-1.161 1.607c.482.302.889.71 1.19 1.192Z\"/>", "panelB": "<path d=\"M16 10.75v2.5A1.75 1.75 0 0 1 14.25 15H1.75A1.75 1.75 0 0 1 0 13.25v-2.5C0 9.784.784 9 1.75 9h12.5c.966 0 1.75.784 1.75 1.75Zm0-8v2.5A1.75 1.75 0 0 1 14.25 7H1.75A1.75 1.75 0 0 1 0 5.25v-2.5C0 1.784.784 1 1.75 1h12.5c.966 0 1.75.784 1.75 1.75Zm-1.75-.25H1.75a.25.25 0 0 0-.25.25v2.5c0 .138.112.25.25.25h12.5a.25.25 0 0 0 .25-.25v-2.5a.25.25 0 0 0-.25-.25Zm0 8H1.75a.25.25 0 0 0-.25.25v2.5c0 .138.112.25.25.25h12.5a.25.25 0 0 0 .25-.25v-2.5a.25.25 0 0 0-.25-.25Z\"/>", "panelR": "<path d=\"M2.75 0h2.5C6.216 0 7 .784 7 1.75v12.5A1.75 1.75 0 0 1 5.25 16h-2.5A1.75 1.75 0 0 1 1 14.25V1.75C1 .784 1.784 0 2.75 0Zm8 0h2.5C14.216 0 15 .784 15 1.75v12.5A1.75 1.75 0 0 1 13.25 16h-2.5A1.75 1.75 0 0 1 9 14.25V1.75C9 .784 9.784 0 10.75 0ZM2.5 1.75v12.5c0 .138.112.25.25.25h2.5a.25.25 0 0 0 .25-.25V1.75a.25.25 0 0 0-.25-.25h-2.5a.25.25 0 0 0-.25.25Zm8 0v12.5c0 .138.112.25.25.25h2.5a.25.25 0 0 0 .25-.25V1.75a.25.25 0 0 0-.25-.25h-2.5a.25.25 0 0 0-.25.25Z\"/>", "play": "<path d=\"M8 0a8 8 0 1 1 0 16A8 8 0 0 1 8 0ZM1.5 8a6.5 6.5 0 1 0 13 0 6.5 6.5 0 0 0-13 0Zm4.879-2.773 4.264 2.559a.25.25 0 0 1 0 .428l-4.264 2.559A.25.25 0 0 1 6 10.559V5.442a.25.25 0 0 1 .379-.215Z\"/>", "plus": "<path d=\"M7.75 2a.75.75 0 0 1 .75.75V7h4.25a.75.75 0 0 1 0 1.5H8.5v4.25a.75.75 0 0 1-1.5 0V8.5H2.75a.75.75 0 0 1 0-1.5H7V2.75A.75.75 0 0 1 7.75 2Z\"/>", "search": "<path d=\"M10.68 11.74a6 6 0 0 1-7.922-8.982 6 6 0 0 1 8.982 7.922l3.04 3.04a.749.749 0 0 1-.326 1.275.749.749 0 0 1-.734-.215ZM11.5 7a4.499 4.499 0 1 0-8.997 0A4.499 4.499 0 0 0 11.5 7Z\"/>", "sidebar": "<path d=\"M6.823 7.823a.25.25 0 0 1 0 .354l-2.396 2.396A.25.25 0 0 1 4 10.396V5.604a.25.25 0 0 1 .427-.177Z\"/><path d=\"M1.75 0h12.5C15.216 0 16 .784 16 1.75v12.5A1.75 1.75 0 0 1 14.25 16H1.75A1.75 1.75 0 0 1 0 14.25V1.75C0 .784.784 0 1.75 0ZM1.5 1.75v12.5c0 .138.112.25.25.25H9.5v-13H1.75a.25.25 0 0 0-.25.25ZM11 14.5h3.25a.25.25 0 0 0 .25-.25V1.75a.25.25 0 0 0-.25-.25H11Z\"/>", "split": "<path d=\"M1.75 0h12.5C15.216 0 16 .784 16 1.75v12.5c0 .464-.184.909-.513 1.237A1.746 1.746 0 0 1 14.25 16H1.75c-.464 0-.909-.184-1.237-.513A1.746 1.746 0 0 1 0 14.25V1.75C0 .784.784 0 1.75 0ZM1.5 1.75v12.5c0 .138.112.25.25.25H7.5v-13H1.75a.25.25 0 0 0-.25.25ZM9 14.5h5.25a.25.25 0 0 0 .25-.25V1.75a.25.25 0 0 0-.25-.25H9Z\"/>", "unified": "<path d=\"M16 10.75v2.5A1.75 1.75 0 0 1 14.25 15H1.75A1.75 1.75 0 0 1 0 13.25v-2.5C0 9.784.784 9 1.75 9h12.5c.966 0 1.75.784 1.75 1.75Zm0-8v2.5A1.75 1.75 0 0 1 14.25 7H1.75A1.75 1.75 0 0 1 0 5.25v-2.5C0 1.784.784 1 1.75 1h12.5c.966 0 1.75.784 1.75 1.75Zm-1.75-.25H1.75a.25.25 0 0 0-.25.25v2.5c0 .138.112.25.25.25h12.5a.25.25 0 0 0 .25-.25v-2.5a.25.25 0 0 0-.25-.25Zm0 8H1.75a.25.25 0 0 0-.25.25v2.5c0 .138.112.25.25.25h12.5a.25.25 0 0 0 .25-.25v-2.5a.25.25 0 0 0-.25-.25Z\"/>", "stash": "<path d=\"M7.122.392a1.75 1.75 0 0 1 1.756 0l5.003 2.902c.83.481.83 1.68 0 2.162L8.878 8.358a1.75 1.75 0 0 1-1.756 0L2.119 5.456a1.251 1.251 0 0 1 0-2.162ZM8.125 1.69a.248.248 0 0 0-.25 0l-4.63 2.685 4.63 2.685a.248.248 0 0 0 .25 0l4.63-2.685ZM1.601 7.789a.75.75 0 0 1 1.025-.273l5.249 3.044a.248.248 0 0 0 .25 0l5.249-3.044a.75.75 0 0 1 .752 1.298l-5.248 3.044a1.75 1.75 0 0 1-1.756 0L1.874 8.814A.75.75 0 0 1 1.6 7.789Zm0 3.5a.75.75 0 0 1 1.025-.273l5.249 3.044a.248.248 0 0 0 .25 0l5.249-3.044a.75.75 0 0 1 .752 1.298l-5.248 3.044a1.75 1.75 0 0 1-1.756 0l-5.248-3.044a.75.75 0 0 1-.273-1.025Z\"/>", "tag": "<path d=\"M1 7.775V2.75C1 1.784 1.784 1 2.75 1h5.025c.464 0 .91.184 1.238.513l6.25 6.25a1.75 1.75 0 0 1 0 2.474l-5.026 5.026a1.75 1.75 0 0 1-2.474 0l-6.25-6.25A1.752 1.752 0 0 1 1 7.775Zm1.5 0c0 .066.026.13.073.177l6.25 6.25a.25.25 0 0 0 .354 0l5.025-5.025a.25.25 0 0 0 0-.354l-6.25-6.25a.25.25 0 0 0-.177-.073H2.75a.25.25 0 0 0-.25.25ZM6 5a1 1 0 1 1 0 2 1 1 0 0 1 0-2Z\"/>", "trash": "<path d=\"M11 1.75V3h2.25a.75.75 0 0 1 0 1.5H2.75a.75.75 0 0 1 0-1.5H5V1.75C5 .784 5.784 0 6.75 0h2.5C10.216 0 11 .784 11 1.75ZM4.496 6.675l.66 6.6a.25.25 0 0 0 .249.225h5.19a.25.25 0 0 0 .249-.225l.66-6.6a.75.75 0 0 1 1.492.149l-.66 6.6A1.748 1.748 0 0 1 10.595 15h-5.19a1.75 1.75 0 0 1-1.741-1.575l-.66-6.6a.75.75 0 1 1 1.492-.15ZM6.5 1.75V3h3V1.75a.25.25 0 0 0-.25-.25h-2.5a.25.25 0 0 0-.25.25Z\"/>", "undo": "<path d=\"M1.22 6.28a.749.749 0 0 1 0-1.06l3.5-3.5a.749.749 0 1 1 1.06 1.06L3.561 5h7.188l.001.007L10.749 5c.058 0 .116.007.171.019A4.501 4.501 0 0 1 10.5 14H8.796a.75.75 0 0 1 0-1.5H10.5a3 3 0 1 0 0-6H3.561L5.78 8.72a.749.749 0 1 1-1.06 1.06l-3.5-3.5Z\"/>", "user": "<path d=\"M10.561 8.073a6.005 6.005 0 0 1 3.432 5.142.75.75 0 1 1-1.498.07 4.5 4.5 0 0 0-8.99 0 .75.75 0 0 1-1.498-.07 6.004 6.004 0 0 1 3.431-5.142 3.999 3.999 0 1 1 5.123 0ZM10.5 5a2.5 2.5 0 1 0-5 0 2.5 2.5 0 0 0 5 0Z\"/>", "warn": "<path d=\"M6.457 1.047c.659-1.234 2.427-1.234 3.086 0l6.082 11.378A1.75 1.75 0 0 1 14.082 15H1.918a1.75 1.75 0 0 1-1.543-2.575Zm1.763.707a.25.25 0 0 0-.44 0L1.698 13.132a.25.25 0 0 0 .22.368h12.164a.25.25 0 0 0 .22-.368Zm.53 3.996v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM9 11a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z\"/>", "worktree": "<path d=\"M0 2.75C0 1.784.784 1 1.75 1H5c.55 0 1.07.26 1.4.7l.9 1.2a.25.25 0 0 0 .2.1h6.75c.966 0 1.75.784 1.75 1.75v8.5A1.75 1.75 0 0 1 14.25 15H1.75A1.75 1.75 0 0 1 0 13.25Zm9.42 9.36 2.883-2.677a.25.25 0 0 0 0-.366L9.42 6.39a.249.249 0 0 0-.42.183V8.5H4.75a.75.75 0 0 0 0 1.5H9v1.927c0 .218.26.331.42.183Z\"/>", "box": "<path d=\"m8.878.392 5.25 3.045c.54.314.872.89.872 1.514v6.098a1.75 1.75 0 0 1-.872 1.514l-5.25 3.045a1.75 1.75 0 0 1-1.756 0l-5.25-3.045A1.75 1.75 0 0 1 1 11.049V4.951c0-.624.332-1.201.872-1.514L7.122.392a1.75 1.75 0 0 1 1.756 0ZM7.875 1.69l-4.63 2.685L8 7.133l4.755-2.758-4.63-2.685a.248.248 0 0 0-.25 0ZM2.5 5.677v5.372c0 .09.047.171.125.216l4.625 2.683V8.432Zm6.25 8.271 4.625-2.683a.25.25 0 0 0 .125-.216V5.677L8.75 8.432Z\"/>", "x": "<path d=\"M3.72 3.72a.75.75 0 0 1 1.06 0L8 6.94l3.22-3.22a.749.749 0 0 1 1.275.326.749.749 0 0 1-.215.734L9.06 8l3.22 3.22a.749.749 0 0 1-.326 1.275.749.749 0 0 1-.734-.215L8 9.06l-3.22 3.22a.751.751 0 0 1-1.042-.018.751.751 0 0 1-.018-1.042L6.94 8 3.72 4.78a.75.75 0 0 1 0-1.06Z\"/>" };
  G.ic = (name, size, cls) => OCTI[name] ? `<svg class="od-icon${cls ? ' ' + cls : ''}" viewBox="0 0 16 16" fill="currentColor" style="--od-icon-size:${size || 14}px" aria-hidden="true">${OCTI[name]}</svg>` : `<svg class="od-icon${cls ? ' ' + cls : ''}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" style="--od-icon-size:${size || 14}px" aria-hidden="true">${ICONS[name] || ''}</svg>`;
  const ic = G.ic;
  /* App mark: the small variant (thicker strokes, no lane dots) up to 32px, the full one above. */
  G.logo = (size) => `<img class="app-logo" src="assets/app-icon/${size > 32 ? 'gitwarp' : 'gitwarp-small'}.svg" width="${size}" height="${size}" alt="" style="flex:none;border-radius:${Math.round(size * 0.22)}px">`;

  /* ---------- stage bar (presentation only) ---------- */
  G.PAGES = [['index.html', 'Overview'], ['tokens.html', 'Tokens'], ['components.html', 'Components'], ['graph.html', 'Graph'], ['main-1200.html', 'Main 1200×800'], ['main-1600.html', 'Main 1600×1000'], ['changes.html', 'Local changes'], ['activity.html', 'Activity'], ['settings-general.html', 'Settings'], ['settings-git.html', 'Settings → Git'], ['history-states.html', 'History states'], ['welcome.html', 'Welcome'], ['dialogs-repo.html', 'Dialogs: clone / open'], ['dialogs-ref.html', 'Dialogs: branch / tag'], ['dialogs-checkout.html', 'Dialogs: checkout'], ['dialogs-network.html', 'Dialogs: network'], ['dialogs-worktree.html', 'Worktrees'], ['worktrees.html', 'Worktrees workspace'], ['settings-ssh.html', 'Settings → SSH keys'], ['palette.html', 'Palette']];
  G.stagebar = function (extra) {
    const cur = location.pathname.split('/').pop() || 'index.html';
    const bar = G.h(`<header class="stagebar"><span class="brand">${G.logo(16)} GitWarp <span class="muted" style="font-weight:400">design</span></span>${G.PAGES.map(([h, l]) => `<a class="nav" href="${h}"${h === cur ? ' aria-current="page"' : ''}>${l}</a>`).join('')}<span class="od-fill"></span>${extra || ''}<button class="btn btn-sm" id="gl-theme" aria-label="Switch theme"></button></header>`);
    document.body.prepend(bar);
    const b = bar.querySelector('#gl-theme');
    const lab = () => { b.textContent = 'Theme: ' + G.THEMES[G.theme]; };
    lab();
    b.onclick = () => { const k = Object.keys(G.THEMES); G.setTheme(k[(k.indexOf(G.theme) + 1) % k.length]); lab(); document.dispatchEvent(new Event('gl-theme')); };
    return bar;
  };
  G.fit = function (win) {
    const wrap = win.parentElement;
    const apply = () => { const W = win.offsetWidth, H = win.offsetHeight, s = Math.min(1, (document.documentElement.clientWidth - 32) / W); win.style.transform = `scale(${s})`; win.style.setProperty('--ui-scale', s); wrap.style.width = W * s + 'px'; wrap.style.height = H * s + 'px'; };
    win._fit = apply; apply(); addEventListener('resize', apply);
  };
  G.scale = (el) => { const w = el.closest('.win'); return w ? w.getBoundingClientRect().width / w.offsetWidth || 1 : 1; };

  /* ---------- sample data: relative time ---------- */
  const NOW = Date.UTC(2026, 9, 29, 14, 20);
  const pad = (n) => String(n).padStart(2, '0');
  G.abs = (min) => { const d = new Date(NOW - min * 60000); return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`; };
  /* relative formats as the app shows them: just now / n min / n h / yesterday / n d / n weeks / n mo / n y. The Appearance "date format" setting switches the column to absolute UTC (G.dateMode). */
  G.dateMode = 'relative';
  G.rel = (m) => (m < 1 ? 'just now' : m < 60 ? `${m} min ago` : m < 1440 ? `${Math.floor(m / 60)} h ago` : m < 2880 ? 'yesterday' : m < 10080 ? `${Math.floor(m / 1440)} d ago` : m < 43200 ? (m < 20160 ? '1 week ago' : `${Math.floor(m / 10080)} weeks ago`) : m < 525600 ? `${Math.floor(m / 43200)} mo ago` : `${Math.floor(m / 525600)} y ago`);
  G.dateCell = (m) => (G.dateMode === 'absolute' ? G.abs(m) : G.rel(m));

  /* Author avatar: uploaded image when the commit has one (or it fails to load → onerror swaps in the fallback);
     otherwise a stable generated badge — initials on a colour derived from the email, so one person always looks the same. */
  G.avatar = (c, size) => {
    size = size || 16;
    const key = (c.email || c.author || '?').toLowerCase();
    let h = 0; for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) >>> 0;
    const parts = String(c.author || '?').trim().split(/\s+/);
    const ini = ((parts[0] || '?')[0] + (parts.length > 1 ? parts[parts.length - 1][0] : '')).toUpperCase();
    const fb = `<span class="avt gen" style="--av:${size}px;--hue:${h % 360}" aria-hidden="true">${G.esc(ini)}</span>`;
    return c.avatar
      ? `<span class="avt" style="--av:${size}px" aria-hidden="true"><img src="${G.esc(c.avatar)}" alt="" width="${size}" height="${size}" onerror="this.parentNode.outerHTML=this.parentNode.dataset.fb" data-x=""></span>`.replace('<span class="avt"', `<span class="avt" data-fb="${G.esc(fb)}"`)
      : fb;
  };
  const AVA = 'data:image/svg+xml;utf8,' + encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" fill="#2f6f73"/><circle cx="16" cy="13" r="6" fill="#d9b89c"/><path d="M4 32c1-8 7-11 12-11s11 3 12 11z" fill="#1f3a5f"/></svg>');
  const AU = { mo: ['Mira Okonkwo', 'mira.okonkwo@example.dev', AVA], tr: ['Tomás Reyes', 'tomas.reyes@example.dev'], lz: ['Lin Zhao', 'lin.zhao@example.dev'], pn: ['Priya Natarajan', 'priya.n@example.dev'], jb: ['Jonas Berg', 'jonas.berg@example.dev'] };
  // [id, short hash, parents, summary, author, minutes ago, refs, body]
  const RAW = [
    ['a01', 'e4f2a91', ['a02', 'a05'], "Merge branch 'feat/idle-lanes'", 'mo', 12, ['head:main'], 'Brings in idle-lane collapsing so graphs with many stale remote branches stay readable.\n\nCloses #187.'],
    ['a02', '7b3c0d2', ['a03'], 'fix(diff): keep scroll offset when toggling side-by-side', 'lz', 55, [], ''],
    ['a05', '19d8e4f', ['a06'], 'feat(graph): collapse idle lanes into a single gap lane', 'mo', 130, ['br:feat/idle-lanes'], 'Runs of three or more lanes that only pass through are drawn as one dashed 12px gap lane.\nToggle with “Collapse idle lanes” in the history header.'],
    ['a03', 'c60a5b8', ['a04'], 'perf(history): build row models in 512-commit chunks', 'tr', 190, ['rm:origin/main'], ''],
    ['a06', 'f3a71c9', ['a07'], 'feat(graph): clip wide graphs with an edge fade and +N badge', 'mo', 300, ['rm:origin/feat/idle-lanes'], ''],
    ['a04', '2e9b8a0', ['a07', 'a09'], "Merge branch 'fix/git-detect'", 'tr', 420, [], ''],
    ['a09', '8d41f6e', ['a10'], 'fix(settings): list both reasons when bundled and system git fail', 'pn', 600, ['br:fix/git-detect', 'rm:origin/fix/git-detect', 'rm:upstream/fix/git-detect', 'tag:v0.3.1-rc1'], ''],
    ['a07', '5c0e92b', ['a08'], 'refactor(ui): extract Tree component from the sidebar', 'lz', 780, ['br:refactor/tree', 'br:spike/virtual-list', 'br:wip/sidebar', 'rm:origin/refactor/tree', 'rm:upstream/refactor/tree-v2'], ''],
    ['a10', 'b7a3d15', ['a08'], 'feat(settings): validate custom git path, require git ≥ 2.31', 'pn', 1100, ['br:release/0.3-hotfix-for-the-detached-head-checkout-crash-on-windows'], ''],
    ['a08', '04f9c7a', ['a11'], 'chore: update gpui dependency', 'jb', 1500, [], ''],
    ['a11', 'd19e3b6', ['a12'], 'feat(sidebar): tags and stashes sections', 'lz', 2100, [], ''],
    ['a12', '63b8f20', ['a13', 'a15'], "Merge branch 'feat/palette'", 'tr', 3000, ['tag:v0.3.0'], ''],
    ['a15', 'a82c4d7', ['a16'], 'feat(palette): fuzzy match with hash-prefix lookup', 'jb', 3400, ['br:feat/palette'], ''],
    ['a13', '3f5e1a9', ['a14'], 'fix(staging): line staging was off by one on CRLF files', 'pn', 4300, [], 'Hunk line offsets were computed on LF-normalised text, while git applies the patch against the raw file.'],
    ['a16', '91c7b0e', ['a14'], 'feat(palette): command registry', 'jb', 5200, [], ''],
    ['a14', 'e08d6f3', ['a17'], 'release: v0.2.0', 'tr', 7000, ['tag:v0.2.0'], ''],
    ['a17', '47a2c85', ['a18'], 'feat(diff): syntax highlighting with tree-sitter', 'lz', 11000, [], ''],
    ['a18', 'b5f0e19', ['a19'], 'feat(history): virtualised list, tested with 100k commits', 'mo', 16000, [], ''],
    ['a19', '2c93d7a', ['a20'], 'feat(graph): lane layout with golden-angle colours', 'mo', 25000, [], ''],
    ['a20', 'f81b46c', ['a21'], 'chore: set up CI for linux', 'jb', 41000, [], ''],
    ['a21', '06e5a3d', [], 'Initial commit', 'mo', 60000, [], '']
  ];
  G.commits = RAW.map(([id, hash, parents, summary, au, min, refs, body]) => ({
    id, hash, parents, summary, author: AU[au][0], email: AU[au][1], avatar: AU[au][2] || '', min, body,
    refs: refs.map((r) => { const i = r.indexOf(':'); return { t: r.slice(0, i), n: r.slice(i + 1) }; }),
    full: (hash + '9c1e7a40b2d5f8036ae1c94d7b52e08f1a35c7').slice(0, 40)
  }));

  /* files per commit + diffs */
  G.FILES = [
    { p: 'src/history/graph.rs', s: 'M', a: 9, d: 3 },
    { p: 'src/ui/sidebar.rs', s: 'M', a: 6, d: 4 },
    { p: 'src/settings/git_exec.rs', s: 'A', a: 11, d: 0 },
    { p: 'src/ui/tree.rs', s: 'R', from: 'src/ui/list.rs', a: 3, d: 2 },
    { p: 'assets/themes/legacy.toml', s: 'D', a: 0, d: 6 }
  ];
  const L = (t, s) => [t, s];
  G.DIFFS = {
    'src/history/graph.rs': [
      { head: '@@ -38,9 +38,14 @@ impl GraphLayout', lines: [
        L('c', '    pub fn lane_x(&self, col: usize) -> f32 {'),
        L('d', '        col as f32 * LANE_W + LANE_W / 2.0'),
        L('a', '        match self.collapse.as_ref() {'),
        L('a', '            Some(map) => map.x_for(col),'),
        L('a', '            None => col as f32 * LANE_W + LANE_W / 2.0,'),
        L('a', '        }'),
        L('c', '    }'), L('c', ''),
        L('c', '    /// Golden-angle hue rotation keeps neighbouring lanes distinct.'),
        L('d', '    pub fn lane_color(col: usize, l: f32) -> Hsla {'),
        L('a', '    pub fn lane_color(col: usize, theme: Theme) -> Hsla {'),
        L('c', '        let hue = (col as f32 * 137.508) % 360.0;'),
        L('d', '        hsla(hue / 360.0, 0.65, l, 1.0)'),
        L('a', '        let l = if theme.is_dark() { 0.62 } else { 0.48 };'),
        L('a', '        hsla(hue / 360.0, 0.65, l, 1.0)'),
        L('c', '    }')] },
      { head: '@@ -96,7 +101,10 @@ fn paint_row', lines: [
        L('c', '    for edge in row.edges() {'),
        L('c', '        let (x0, x1) = (self.lane_x(edge.from), self.lane_x(edge.to));'),
        L('d', '        paint_line(cx, x0, x1, 2.0);'),
        L('a', '        if x0 > clip_right && x1 > clip_right {'),
        L('a', '            continue; // fully clipped, skip the path'),
        L('a', '        }'),
        L('a', '        paint_curve(cx, x0, x1, LINE_W);'),
        L('c', '    }')] }
    ],
    'src/ui/sidebar.rs': [
      { head: '@@ -12,8 +12,10 @@ pub struct Sidebar', lines: [
        L('c', '    filter: Model<String>,'),
        L('d', '    sections: Vec<Section>,'),
        L('a', '    sections: Vec<Section>,'),
        L('a', '    tags: Vec<TagItem>,'),
        L('a', '    stashes: Vec<StashItem>,'),
        L('c', '    width: Pixels,'),
        L('c', '}'), L('c', ''),
        L('d', '    fn row_height() -> Pixels { px(24.0) }'),
        L('a', '    fn row_height() -> Pixels { px(26.0) }')] }
    ],
    'src/settings/git_exec.rs': [
      { head: '@@ -0,0 +1,11 @@', lines: [
        L('a', 'pub const MIN_GIT: Version = Version::new(2, 31, 0);'), L('a', ''),
        L('a', 'pub enum GitChoice {'), L('a', '    Auto,'), L('a', '    Bundled,'), L('a', '    System,'),
        L('a', '    Custom(PathBuf),'), L('a', '}'), L('a', ''),
        L('a', 'pub fn resolve(choice: &GitChoice) -> Result<GitExec, GitError> {'), L('a', '    todo!("probe bundled, then PATH")'), L('a', '}')] }
    ],
    'src/ui/tree.rs': [
      { head: '@@ -1,6 +1,7 @@ rename from src/ui/list.rs', lines: [
        L('c', 'use gpui::*;'), L('c', ''),
        L('d', 'pub struct List<T> {'),
        L('a', 'pub struct Tree<T> {'),
        L('a', '    indent: Pixels,'),
        L('c', '    items: Vec<T>,'),
        L('c', '}')] }
    ],
    'assets/themes/legacy.toml': [
      { head: '@@ -1,6 +0,0 @@', lines: [
        L('d', '[colors]'), L('d', 'background = "#1e1e1e"'), L('d', 'text = "#d4d4d4"'), L('d', 'accent = "#569cd6"'), L('d', ''), L('d', '# superseded by tokens.rs')] }
    ]
  };

  /* ---------- tiny Rust-ish highlighter ---------- */
  const RX = /(\/\/.*$)|("(?:[^"\\]|\\.)*")|\b(\d[\d_.]*)\b|\b(fn|let|mut|pub|struct|impl|use|if|else|match|for|in|return|const|self|Self|mod|enum|trait|where|as|ref|move|async|await|loop|while|true|false|continue|None|Some)\b|\b([A-Z][A-Za-z0-9_]*)\b|\b([a-z_][a-z0-9_]*)(?=\()/gm;
  G.hl = function (src) {
    let out = '', last = 0, m;
    RX.lastIndex = 0;
    while ((m = RX.exec(src))) {
      out += esc(src.slice(last, m.index));
      const cls = m[1] ? 'com' : m[2] ? 'str' : m[3] ? 'num' : m[4] ? 'kw' : m[5] ? 'ty' : 'fn';
      out += `<span class="t-${cls}">${esc(m[0])}</span>`;
      last = RX.lastIndex;
      if (m[0] === '') RX.lastIndex++;
    }
    return out + esc(src.slice(last));
  };
  function inline(a, b) {
    let p = 0; while (p < a.length && p < b.length && a[p] === b[p]) p++;
    let s = 0; while (s < a.length - p && s < b.length - p && a[a.length - 1 - s] === b[b.length - 1 - s]) s++;
    const f = (t) => (t.slice(0, p) ? G.hl(t.slice(0, p)) : '') + `<span class="chg">${G.hl(t.slice(p, t.length - s))}</span>` + G.hl(t.slice(t.length - s));
    return [a.length - p - s > 0 ? f(a) : G.hl(a), b.length - p - s > 0 ? f(b) : G.hl(b)];
  }
  /* diff -> HTML. mode: 'unified' | 'split'. opts.checks adds per-line stage checkboxes (Local changes). */
  G.renderDiff = function (hunks, mode, opts) {
    opts = opts || {};
    let html = '';
    hunks.forEach((hk, hi) => {
      const m = /@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(hk.head);
      let o = m ? +m[1] : 1, n = m ? +m[2] : 1;
      const rows = [];
      const L = hk.lines;
      for (let i = 0; i < L.length; i++) {
        const [t, s] = L[i];
        if (t === 'c') { rows.push({ t, o: o++, n: n++, a: s, b: s }); continue; }
        if (t === 'd') {
          // pair a run of deletions with the following run of additions
          let dels = [], adds = [], j = i;
          while (j < L.length && L[j][0] === 'd') dels.push(L[j++][1]);
          while (j < L.length && L[j][0] === 'a') adds.push(L[j++][1]);
          const k = Math.max(dels.length, adds.length);
          for (let x = 0; x < k; x++) {
            const d = dels[x], a = adds[x];
            let hd = d != null ? G.hl(d) : '', ha = a != null ? G.hl(a) : '';
            if (d != null && a != null) [hd, ha] = inline(d, a);
            rows.push({ t: 'p', o: d != null ? o++ : null, n: a != null ? n++ : null, a: hd, b: ha, hasD: d != null, hasA: a != null, raw: true });
          }
          i = j - 1; continue;
        }
        rows.push({ t: 'a', o: null, n: n++, a: '', b: s });
      }
      const chk = (r) => (opts.checks ? `<label class="check ${opts.on ? 'on' : ''}" style="min-height:0"><span class="box lcb">${opts.on ? ic('check', 10) : ''}</span></label>` : '');
      html += `<div class="dh"><span class="od-fill">${esc(hk.head)}</span>${opts.hunkAction ? `<button class="btn btn-sm">${opts.hunkAction}</button>` : ''}</div>`;
      rows.forEach((r) => {
        if (mode === 'split') {
          const left = r.t === 'c' ? G.hl(r.a) : r.t === 'p' ? (r.hasD ? r.a : '') : '';
          const right = r.t === 'c' ? G.hl(r.b) : r.t === 'a' ? G.hl(r.b) : r.hasA ? r.b : '';
          const lc = r.t === 'p' ? (r.hasD ? 'del' : 'empty') : r.t === 'a' ? 'empty' : '';
          const rc = r.t === 'p' ? (r.hasA ? 'add' : 'empty') : r.t === 'a' ? 'add' : '';
          html += `<div class="dl s"><span class="ln ${lc === 'del' ? 'del' : ''}">${r.o ?? ''}</span><span class="c ${lc}">${left}</span><span class="ln ${rc === 'add' ? 'add' : ''}">${r.n ?? ''}</span><span class="c ${rc}">${right}</span></div>`;
        } else if (r.t === 'c') {
          html += `<div class="dl u"><span class="ln">${r.o}</span><span class="ln">${r.n}</span><span class="sg"></span><span class="c">${G.hl(r.a)}</span></div>`;
        } else if (r.t === 'a') {
          html += `<div class="dl u add"><span class="ln o"></span><span class="ln n">${r.n}</span><span class="sg">+</span><span class="c">${G.hl(r.b)}</span></div>`;
        } else {
          if (r.hasD) html += `<div class="dl u del"><span class="ln o">${r.o}</span><span class="ln n"></span><span class="sg">−</span><span class="c">${r.a}</span></div>`;
          if (r.hasA) html += `<div class="dl u add"><span class="ln o"></span><span class="ln n">${r.n}</span><span class="sg">+</span><span class="c">${r.b}</span></div>`;
        }
      });
    });
    return html;
  };

  /* ---------- commit graph ---------- */
  G.hue = (i) => ((i * 137.508) % 360).toFixed(1);
  G.layout = function (commits) {
    let active = [];
    const rows = []; let maxCol = 0;
    commits.forEach((c) => {
      const before = active.slice();
      const cols = [];
      before.forEach((v, i) => { if (v === c.id) cols.push(i); });
      let col;
      if (cols.length) col = cols[0];
      else { col = before.findIndex((v) => v === null); if (col < 0) { col = before.length; before.push(null); } }
      const after = before.slice();
      cols.forEach((i) => { after[i] = null; });
      const outs = [];
      c.parents.forEach((p, k) => {
        let j = after.indexOf(p);
        if (j < 0) {
          if (k === 0) j = col;
          else { j = after.findIndex((v, x) => v === null && x !== col); if (j < 0) j = after.length; }
          after[j] = p;
        }
        outs.push(j);
      });
      while (after.length && after[after.length - 1] === null) after.pop();
      rows.push({ id: c.id, col, before, cols, outs, merge: c.parents.length > 1 });
      maxCol = Math.max(maxCol, col, before.length - 1, after.length - 1);
      active = after;
    });
    return { rows, lanes: maxCol + 1 };
  };
  /* x positions per lane. collapse=true merges runs (>=3) of lanes that only have 1 event (a stale tip dot) into one 12px gap lane */
  // 2px left inset: the HEAD ring (r 7.5 + 0.75 stroke) on lane 0 would otherwise poke 1.25px past the column edge and be clipped.
  const PAD = 2;
  G.geometry = function (lay, collapse) {
    const n = lay.lanes, ev = new Array(n).fill(0), xs = [], grp = [];
    lay.rows.forEach((r) => { ev[r.col]++; r.cols.forEach((i) => { if (i !== r.col) ev[i]++; }); r.outs.forEach((j) => { if (j !== r.col) ev[j]++; }); });
    if (!collapse) { for (let i = 0; i < n; i++) xs.push(i * 14 + PAD + 7); return { xs, grp, width: n * 14 + PAD, idle: 0 }; }
    let x = PAD, i = 0, idle = 0;
    while (i < n) {
      if (ev[i] >= 2) { xs[i] = x + 7; x += 14; i++; continue; }
      let j = i; while (j < n && ev[j] < 2) j++;
      if (j - i < 3) { for (let k = i; k < j; k++) { xs[k] = x + 7; x += 14; } }
      else { for (let k = i; k < j; k++) { xs[k] = x + 6; grp[k] = true; } x += 12; idle += j - i; }
      i = j;
    }
    return { xs, grp, width: x, idle };
  };
  G.rowSVG = function (row, geo, o) {
    o = o || {};
    const xs = geo.xs, maxX = o.maxX == null ? 1e9 : o.maxX, seen = new Set();
    let p = '';
    const pass = (i) => { const x = xs[i]; if (x > maxX) return; const k = 'v' + x; if (seen.has(k)) return; seen.add(k); p += `<path class="g-e v${geo.grp[i] ? ' idle' : ''}" style="--h:${G.hue(i)}" d="M${x} -1V27"/>`; };
    row.before.forEach((v, i) => { if (v !== null && !row.cols.includes(i)) pass(i); });
    const xc = xs[row.col];
    row.cols.forEach((i) => {
      const xi = xs[i];
      if (Math.min(xi, xc) > maxX) return;
      p += i === row.col ? `<path class="g-e v" style="--h:${G.hue(i)}" d="M${xi} -1V13"/>` : `<path class="g-e" style="--h:${G.hue(i)}" d="M${xi} -1V0C${xi} 7 ${xc} 6 ${xc} 13"/>`;
    });
    row.outs.forEach((j) => {
      const xj = xs[j];
      if (Math.min(xj, xc) > maxX) return;
      p += j === row.col ? `<path class="g-e v" style="--h:${G.hue(j)}" d="M${xc} 13V27"/>` : `<path class="g-e" style="--h:${G.hue(j)}" d="M${xc} 13C${xc} 20 ${xj} 19 ${xj} 26V27"/>`;
    });
    if (row.stub && xc <= maxX) p += `<path class="g-e stub" style="--h:${G.hue(row.col + 1)}" d="M${xc} 13L${xc + 9} 5"/><circle class="g-stub" style="--h:${G.hue(row.col + 1)}" cx="${xc + 9}" cy="5" r="1.8"/>`;
    if (xc <= maxX + 6) {
      if (o.head) p += `<circle class="g-h" cx="${xc}" cy="13" r="7.5"/>`;
      p += row.merge ? `<circle class="g-d m" style="--h:${G.hue(row.col)}" cx="${xc}" cy="13" r="3.5"/>` : `<circle class="g-d" style="--h:${G.hue(row.col)}" cx="${xc}" cy="13" r="4.5"/>`;
    }
    return `<svg width="${Math.min(geo.width, maxX + 20)}" height="26" viewBox="0 0 ${Math.min(geo.width, maxX + 20)} 26" aria-hidden="true">${p}</svg>`;
  };

  /* ---------- shared repository state (design prototype: stands in for the app's repo model) ---------- */
  G.state = { head: 'main', detached: null, dirty: false, dirtyCount: 6, branches: 'loaded', bgfetch: true, remotes: ['main', 'feat/idle-lanes', 'fix/git-detect'] };
  G.commits.forEach((c) => { c.base = c.refs.map((r) => Object.assign({}, r)); });
  /* move HEAD: {branch} checks a local branch out, {commit} detaches at a commit. The badges, toolbar label, sidebar marker and row menus follow. */
  G.applyHead = function (spec) {
    G.commits.forEach((c) => { c.refs = c.base.map((r) => ({ t: r.t === 'head' ? 'br' : r.t, n: r.n })); });
    if (spec.branch) {
      G.state.head = spec.branch; G.state.detached = null;
      const c = G.commits.find((x) => x.refs.some((r) => r.t === 'br' && r.n === spec.branch));
      if (c) c.refs.find((r) => r.t === 'br' && r.n === spec.branch).t = 'head';
    } else { G.state.head = null; G.state.detached = spec.commit.hash; spec.commit.refs.push({ t: 'head', n: 'HEAD' }); }
    document.dispatchEvent(new Event('gl-head'));
  };
  G.headLabel = () => G.state.detached || G.state.head || '…';
  G.headName = () => G.state.head || 'HEAD';

  /* Dialog hand-off: opens the matching dialog from dialogs.js when it exists; the dialog designs live in dialogs-ref / dialogs-checkout / dialogs-worktree / dialogs-network. */
  const DLG = { branch: 'Create branch here', tag: 'Create tag here', renameBranch: 'Rename branch', deleteBranch: 'Delete branch', renameTag: 'Rename tag', deleteTag: 'Delete tag', dropStash: 'Drop stash', deleteRemoteBranch: 'Delete remote branch', cherryPick: 'Cherry-pick commit', revert: 'Revert commit', reset: 'Reset branch to here', editRemote: 'Edit remote', removeRemote: 'Remove remote', renameWorktree: 'Rename worktree', checkout: 'Uncommitted changes before checkout' };
  G.act = function (win, name, arg) {
    const D = window.GLD; if (!D) return G.toast(win, `Opens the “${DLG[name] || name}” dialog — see dialogs-ref.html`, 'info');
    const br = G.headName();
    const m = {
      renameBranch: () => D.rename(win, 'branch', { name: arg, old: arg }),
      renameTag: () => D.rename(win, 'tag', { name: arg, old: arg }),
      renameWorktree: () => D.rename(win, 'worktree', { name: arg, old: arg }),
      deleteBranch: () => D.del(win, 'branch', { name: arg }),
      deleteTag: () => D.del(win, 'tag', { name: arg }),
      removeRemote: () => D.del(win, 'remote', { name: arg }),
      dropStash: () => D.del(win, 'stash', { n: +(/\{(\d+)\}/.exec(arg) || [0, 0])[1], msg: 'WIP on feat/idle-lanes: 19d8e4f ' + (arg === 'stash@{0}' ? 'collapse idle lanes' : 'tree icon') }),
      editRemote: () => D.remoteEdit(win, { name: arg }),
      deleteRemoteBranch: () => { const i = arg.indexOf('/'); D.confirmRemoteBranch(win, { remote: arg.slice(0, i), branch: arg.slice(i + 1) }); },
      cherryPick: () => D.cherry(win, { commit: arg, branch: br }),
      revert: () => D.revert(win, { commit: arg, branch: br }),
      reset: () => D.reset(win, { commit: arg, branch: br })
    };
    if (m[name]) return m[name]();
    G.toast(win, `Opens the “${DLG[name] || name}” dialog — see dialogs-ref.html`, 'info');
  };

  /* checkout with the dirty-tree guard: a dirty working tree opens the checkout dialog first */
  G.checkout = function (win, spec) {
    const done = () => {
      if (spec.branch) { G.applyHead({ branch: spec.branch }); G.toast(win, `Switched to ${spec.branch}`, 'ok'); }
      else if (spec.remote) { G.toast(win, `Checked out ${spec.remote}`, 'ok'); }
      else { G.applyHead({ commit: spec.commit }); G.toast(win, spec.tag ? `Checked out tag ${spec.tag} (detached HEAD)` : `Checked out ${spec.commit.hash} (detached HEAD)`, 'ok'); }
    };
    if (G.state.dirty) {
      const n = G.state.dirtyCount, what = spec.branch ? `Switch to ${spec.branch}?` : spec.remote ? `Check out ${spec.remote}?` : spec.tag ? `Check out tag ${spec.tag}?` : spec.commit ? `Check out ${spec.commit.hash}?` : 'Check out?';
      const D = window.GLD;
      if (D && typeof D.checkout === 'function') return D.checkout(win, { origin: spec.branch ? 'branch' : spec.remote ? 'remote' : spec.tag ? 'tag' : 'commit', target: spec.branch || spec.remote || spec.tag || (spec.commit && spec.commit.hash), n });
      G.toast(win, `${what} ${n} uncommitted changes — opens the checkout dialog (dialogs-checkout.html)`, 'warn'); return;
    }
    done();
  };

  /* ---------- history states (shared by main window and history-states.html) ---------- */
  const SK = (n) => Array.from({ length: n }, (_, i) => `<div class="sk"><div class="lane"></div><div class="skel" style="width:${40 + (i * 37) % 50}%"></div><div class="skel" style="width:${50 + (i * 17) % 35}%"></div><div class="skel" style="width:60%"></div><div class="skel" style="width:80%"></div></div>`).join('');
  const EMPTY = (icon, tone, title, body, extra, btns) => `<div class="empty"><div class="ei"${tone ? ` style="color:var(--${tone})"` : ''}>${ic(icon, 22)}</div><h4>${title}</h4><p>${body}</p>${extra || ''}<div class="row-btns">${btns || ''}</div></div>`;
  G.stateStrip = function (kind) {
    if (kind === 'loading') return `<div class="strip load" role="status">${ic('fetch', 14, 'spin')}<b>Loading… 12 400 commits · 3 idle lanes folded</b><span class="progress"><i style="width:34%"></i></span><span class="pct">34%</span><button class="btn btn-sm" data-act="cancel">Cancel</button></div>`;
    if (kind === 'strip-cancelled') return `<div class="strip" role="status">${ic('warn', 14, 'w')}<span>History is incomplete — loading was cancelled</span><span class="od-fill"></span><button class="btn btn-sm" data-act="retry">Retry</button></div>`;
    if (kind === 'strip-failed') return `<div class="strip" role="alert">${ic('err', 14, 'e')}<span>History is incomplete: git log failed</span><span class="od-fill"></span><button class="btn btn-sm" data-act="retry">Retry</button><button class="btn btn-sm" data-act="copy">Copy details</button></div>`;
    if (kind === 'strip-failed-msg') return `<div class="strip" role="alert">${ic('err', 14, 'e')}<span>History is incomplete: the connection to the repository was lost</span><span class="od-fill"></span><button class="btn btn-sm" data-act="retry">Retry</button><button class="btn btn-sm" data-act="copy">Copy details</button></div>`;
    return '';
  };
  G.stateBody = function (kind) {
    const B = (cls, label, icon) => `<button class="btn${cls}" data-act="${label[1]}">${icon ? ic(icon, 14) : ''}${label[0]}</button>`;
    switch (kind) {
      case 'loading': return SK(10);
      case 'empty': return EMPTY('git', '', 'No commits yet', 'This repository is empty. Stage some files and make your first commit to see history here.', '', B(' btn-primary', ['Go to Local changes', 'changes']) + B('', ['Add .gitignore', 'gitignore']));
      case 'notgit': return EMPTY('folder', '', 'Not a git repository', '<span class="path">/home/mira/projects/notes</span> is not inside a git working tree.', '', B(' btn-primary', ['Initialize repository here', 'init'], 'init') + B('', ['Open another…', 'open']));
      case 'nofolder': return EMPTY('folder', '', 'Folder not found', '<span class="path">/home/mira/projects/notes-archive</span> does not exist.', '', B(' btn-primary', ['Remove from recent', 'remove-recent']) + B('', ['Open another…', 'open']));
      case 'cancelled': return EMPTY('fetch', '', 'Loading cancelled', 'The history was not loaded completely.', '', B(' btn-primary', ['Retry', 'retry']));
      case 'nogit': return EMPTY('err', 'error', 'git was not found', 'GitWarp could not find a usable git. The bundled copy is missing and there is no <span class="mono">git</span> in PATH.', '', B(' btn-primary', ['Open Git settings', 'git-settings'], 'gear') + B('', ['Retry', 'retry']));
      case 'oldgit': return EMPTY('warn', 'warning', 'git 2.28.1 is too old', 'GitWarp needs git 2.31 or newer. System git at <span class="mono">/usr/bin/git</span> reports 2.28.1.', '', B(' btn-primary', ['Open Git settings', 'git-settings'], 'gear') + B('', ['Retry', 'retry']));
      case 'giterr': return EMPTY('err', 'error', 'git could not read the history', 'The command <span class="mono">git log --all --topo-order</span> failed with exit code 128.', `<div class="errbox" tabindex="0" aria-label="git error output">fatal: bad object refs/heads/feat/palette\nerror: refs/heads/feat/palette does not point to a valid object!</div>`, B(' btn-primary', ['Retry', 'retry'], 'fetch') + B('', ['Copy details', 'copy'], 'copy'));
      case 'nongit': return EMPTY('err', 'error', 'GitWarp could not read the history', 'Something went wrong while reading the repository.', `<div class="errbox" tabindex="0" aria-label="error details">failed to parse commit 8d41f6e: unexpected end of the commit header</div>`, B(' btn-primary', ['Retry', 'retry'], 'fetch') + B('', ['Copy details', 'copy'], 'copy'));
    }
    return '';
  };
  /* status-bar text per state: [left html kind, text] */
  G.stateStatus = { loading: ['load', 'Loading history…'], cancelled: ['mute', 'Loading cancelled'], notgit: ['err', 'Not a git repository'], nofolder: ['err', 'Folder not found'], nogit: ['err', 'git was not found'], oldgit: ['err', 'git 2.28.1 is too old'], giterr: ['err', 'git could not read the history'], nongit: ['err', 'GitWarp could not read the history'] };

  /* ---------- virtualised history list ---------- */
  const KIND_ORDER = { head: 0, br: 1, rm: 2, tag: 3 };
  const REFNAME = (r) => (r.t === 'head' ? (r.n === 'HEAD' ? 'HEAD' : 'refs/heads/' + r.n) : r.t === 'br' ? 'refs/heads/' + r.n : r.t === 'rm' ? 'refs/remotes/' + r.n : 'refs/tags/' + r.n);
  G.history = function (host, commits, o) {
    o = o || {};
    let lay = G.layout(commits);
    const allCommits = commits, fullLay = lay;
    let merged = new Map(), forkId = null, bfName = null, bfInfo = null;
    let collapse = !!o.collapse, gw = o.gw || 112, geo = G.geometry(lay, collapse), sel = o.sel == null ? 0 : o.sel, RH = 26, query = '', matches = [], mi = -1;
    host.classList.add('hist');
    host.innerHTML = `<div class="lhead" role="row"><div class="gh"><span>Graph</span><button class="more" hidden></button><span class="grip" data-tip="Drag to resize the graph column · double-click to fit"></span></div><div>Summary</div><div>Author</div><div>Date</div><div>Hash</div></div><div class="lbody" tabindex="0" role="listbox" aria-label="Commit history"></div>`;
    const head = host.querySelector('.lhead'), body = host.querySelector('.lbody'), more = head.querySelector('.more'), grip = head.querySelector('.grip');
    const OV = 8;
    const api = { el: host, get sel() { return sel; } };
    const menuHost = () => host.closest('.win,[data-menu-host]');
    function setGw() { head.style.setProperty('--gw', gw + 'px'); body.style.setProperty('--gw', gw + 'px'); }
    const mk = (t) => { if (!query) return esc(t); const i = t.toLowerCase().indexOf(query); return i < 0 ? esc(t) : esc(t.slice(0, i)) + '<mark class="hl">' + esc(t.slice(i, i + query.length)) + '</mark>' + esc(t.slice(i + query.length)); };
    function badge(x) {
      const icon = x.t === 'rm' ? 'cloud' : x.t === 'tag' ? 'tag' : 'branch';
      return `<span class="ref ${x.t === 'rm' ? 'remote' : x.t === 'tag' ? 'tag' : x.t === 'head' ? 'head' : ''}" data-tip="${esc(REFNAME(x))}" data-full="${esc(REFNAME(x))}" data-rn="${esc(x.n)}" data-rt="${x.t}">${ic(icon, 11)}<span class="rl">${esc(x.n)}</span></span>`;
    }
    function rowHTML(i) {
      const c = commits[i], r = lay.rows[i], clipped = geo.width > gw;
      const head_ = c.refs.some((x) => x.t === 'head');
      const refs = c.refs.slice().sort((a, b) => (KIND_ORDER[a.t] - KIND_ORDER[b.t]) || a.n.localeCompare(b.n)).map(badge).join('');
      const hx = /^[0-9a-f]{4,40}$/.test(query) && c.hash.startsWith(query);
      /* branch filter: a merge keeps its second parent as a dashed stub in the graph and a "from {branch}" chip here; the chip drills into that branch */
      const mf = (merged.get(c.id) || []).map((n) => `<span class="ref mf" role="button" data-bf="${esc(n)}" data-tip="Merged from ${esc(n)} · click to show its commits">${ic('branch', 11)}<span class="rl">from ${esc(n)}</span></span>`).join('');
      return `<div class="lrow${head_ ? ' head' : ''}${c.id === forkId ? ' fork' : ''}${i === sel ? ' sel' : ''}" role="option" aria-selected="${i === sel}" id="cm-${i}" data-i="${i}"><div class="gcol${clipped ? ' clip' : ''}">${G.rowSVG(r, geo, { maxX: clipped ? gw : undefined, head: head_ }).replace('<svg ', `<svg preserveAspectRatio="none" style="height:${RH}px" `)}</div><div class="summary">${refs}${mf}<span class="txt" data-tip="${esc(c.summary)}">${mk(c.summary)}</span>${c.id === forkId ? '<span class="fork-note">fork point</span>' : ''}</div><div class="au">${G.avatar(c, 16)}<span class="od-truncate" data-tip="${esc(c.author)}">${mk(c.author)}</span></div><div class="dt" data-tip="${G.abs(c.min)} UTC">${G.dateCell(c.min)}</div><div class="hash">${hx ? `<mark class="hl">${c.hash.slice(0, query.length)}</mark>${c.hash.slice(query.length)}` : c.hash}</div></div>`;
    }
    /* badges that do not fit collapse into one muted "+N" badge; the summary keeps at least min(80px, 40% of the cell); HEAD is never hidden */
    function fitRefs() {
      body.querySelectorAll('.summary').forEach((s) => {
        const refs = Array.from(s.querySelectorAll('.ref:not(.more)')); if (!refs.length) return;
        const W = s.clientWidth, minTxt = Math.min(80, W * 0.4), w = (r) => r.offsetWidth + 6;
        let used = refs.reduce((a, r) => a + w(r), 0);
        if (W - used >= minTxt) return;
        const hid = [];
        for (let i = refs.length - 1; i >= 0 && W - used - 40 < minTxt; i--) { if (refs[i].classList.contains('head')) continue; used -= w(refs[i]); refs[i].classList.add('hid'); hid.unshift(refs[i]); }
        if (hid.length) { const m = G.h(`<span class="ref more" data-tip="${esc(hid.map((x) => x.dataset.full).join('\n'))}" style="white-space:pre">+${hid.length}</span>`); s.insertBefore(m, s.querySelector('.txt')); m.removeAttribute('style'); m.dataset.tip = hid.map((x) => x.dataset.full).join('\n'); }
      });
    }
    function paint() {
      const st = body.scrollTop, vh = body.clientHeight || 420;
      const a = Math.max(0, Math.floor(st / RH) - OV), b = Math.min(commits.length, Math.ceil((st + vh) / RH) + OV);
      if (!commits.length && bfName) { body.innerHTML = `<div class="bf-empty"><b>No commits to show</b><span>${esc(bfName)} has no commits in this repository view. It may be gone or not fetched yet.</span><button type="button" class="bf-x" data-bf-clear>Show all commits</button></div>`; return; }
      let h = `<div class="vs" style="height:${a * RH}px"></div>`;
      for (let i = a; i < b; i++) h += rowHTML(i);
      h += `<div class="vs" style="height:${(commits.length - b) * RH}px"></div>`;
      body.innerHTML = h; fitRefs();
    }
    function header() {
      const hidden = geo.xs.filter((x) => x > gw).length;
      more.hidden = hidden === 0;
      more.textContent = `+${hidden}`;
      more.dataset.tip = `${hidden} lanes are clipped. Click to widen the column.`;
    }
    function render() { setGw(); header(); paint(); }
    function select(i, scroll) {
      sel = Math.max(0, Math.min(commits.length - 1, i));
      if (scroll !== false) { const t = sel * RH; if (t < body.scrollTop) body.scrollTop = t; else if (t + RH > body.scrollTop + body.clientHeight) body.scrollTop = t + RH - body.clientHeight; }
      paint(); body.setAttribute('aria-activedescendant', 'cm-' + sel);
      body.querySelectorAll('.lrow.sel').forEach((e) => e.classList.add('kf'));
      o.onSelect && o.onSelect(commits[sel], sel);
    }
    /* checkout items of the row menu: one "Switch to {branch}" per local branch other than HEAD, else "Checkout this commit" */
    function others(c) { return c.refs.filter((x) => x.t === 'br'); }
    function checkoutRow(c) { const b = others(c); if (b.length) G.checkout(menuHost(), { branch: b[0].n }); else G.checkout(menuHost(), { commit: c }); }
    /* branches a row action can target: the HEAD branch first, then local A-Z, then remote A-Z; a remote ref is dropped when a local branch of the same name sits on the same commit (a duplicate for this purpose) */
    function filterable(c) {
      const loc = c.refs.filter((x) => (x.t === 'head' || x.t === 'br') && x.n !== 'HEAD').sort((a, b) => (a.t === 'head' ? -1 : b.t === 'head' ? 1 : a.n.localeCompare(b.n))).map((x) => ({ n: x.n, head: x.t === 'head' }));
      const names = new Set(loc.map((x) => x.n));
      const rem = c.refs.filter((x) => x.t === 'rm' && !names.has(x.n.slice(x.n.indexOf('/') + 1))).sort((a, b) => a.n.localeCompare(b.n)).map((x) => ({ n: x.n, remote: true }));
      return loc.concat(rem);
    }
    /* Branch-bound actions pick their branch first (G.menu has no submenus, so the picker REPLACES the menu and starts with "Back").
       1 branch on the commit: the action names it. 2 or more: "Action…" opens the picker. "Branch…" goes one step further: pick the branch, then get its full sidebar menu.
       The last-picked branch is listed first, but never acted on automatically. */
    let lastPick = null;
    const branchMenuItems = (w, x) => (MENUS(w)[x.remote ? 'rbranch' : 'branch'](x.n));
    function picker(c, ev, w, fl, run) {
      const back = { label: 'Back', icon: 'undo', key: '‹', onClick: () => G.menu(w, ev, rowMenu(c, ev)) };
      const list = fl.slice(); const i = list.findIndex((x) => x.n === lastPick); if (i > 0) list.unshift(list.splice(i, 1)[0]);
      G.menu(w, ev, [back, { sep: 1 }].concat(list.map((x) => ({ label: x.n, icon: x.remote ? 'cloud' : 'branch', key: x.head ? 'HEAD' : x.remote ? 'remote' : '', onClick: () => { lastPick = x.n; run(x); } }))));
    }
    function branchItems(c, ev, w) {
      const fl = filterable(c), sw = others(c), cur = (n) => n === bfName, out = [];
      const one = fl[0], many = fl.length > 1;
      if (sw.length === 1) out.push({ label: `Switch to ${sw[0].n}`, icon: 'check', onClick: () => G.checkout(w, { branch: sw[0].n }) });
      else if (sw.length > 1) out.push({ label: 'Switch to…', icon: 'check', key: '›', onClick: () => picker(c, ev, w, sw.map((x) => ({ n: x.n })), (x) => G.checkout(w, { branch: x.n })) });
      else out.push({ label: 'Checkout this commit', icon: 'check', onClick: () => G.checkout(w, { commit: c }) });
      if (bfName) out.push({ label: 'Show all commits', icon: 'x', onClick: () => api.setBranch(null) });
      if (fl.length === 1) out.push({ label: `Show all commits of ${one.n}`, icon: cur(one.n) ? 'check' : 'branch', disabled: cur(one.n), onClick: () => api.setBranch(one.n) });
      else if (many) out.push({ label: 'Show all commits of…', icon: 'branch', key: '›', onClick: () => picker(c, ev, w, fl, (x) => api.setBranch(x.n)) });
      const back = (x) => ({ label: 'Back', icon: 'undo', key: '‹', onClick: () => G.menu(w, ev, rowMenu(c, ev)) });
      if (fl.length === 1) out.push({ label: `Branch ${one.n}`, icon: one.remote ? 'cloud' : 'branch', key: '›', onClick: () => G.menu(w, ev, [back(), { sep: 1 }].concat(branchMenuItems(w, one))) });
      else if (many) out.push({ label: 'Branch…', icon: 'branch', key: '›', onClick: () => picker(c, ev, w, fl, (x) => G.menu(w, ev, [back(), { sep: 1 }].concat(branchMenuItems(w, x)))) });
      return out;
    }
    function rowMenu(c, ev) {
      const w = menuHost(), b = others(c);
      const mfi = (merged.get(c.id) || []).map((n) => ({ label: `Show all commits of ${n}`, icon: 'branch', onClick: () => api.setBranch(n) }));
      const own = branchItems(c, ev, w);
      return [{ label: 'Copy commit hash', icon: 'copy', key: 'Ctrl+C', onClick: () => { try { navigator.clipboard.writeText(c.full); } catch (e) {} } }].concat(own, mfi.length ? [{ sep: 1 }] : [], mfi, [
        { label: 'Create branch here…', icon: 'branch', key: 'B', onClick: () => window.GLD && window.GLD.openIn('branch', w) },
        { label: 'Create tag here…', icon: 'tag', onClick: () => window.GLD && window.GLD.openIn('tag', w) },
        { sep: 1 },
        { label: 'Cherry-pick', icon: 'undo', onClick: () => G.act(w, 'cherryPick', c) },
        { label: 'Revert commit', icon: 'undo', onClick: () => G.act(w, 'revert', c) },
        { label: `Reset ${G.headName()} to here…`, icon: 'undo', danger: 1, onClick: () => G.act(w, 'reset', c) }]);
    }
    body.addEventListener('scroll', () => requestAnimationFrame(paint));
    body.addEventListener('mousedown', (e) => { const r = e.target.closest('.lrow'); if (r) { body.focus({ preventScroll: true }); select(+r.dataset.i, false); body.querySelectorAll('.lrow.sel').forEach((x) => x.classList.remove('kf')); } });
    body.addEventListener('dblclick', (e) => {
      const r = e.target.closest('.lrow'); if (!r || !menuHost()) return;
      const c = commits[+r.dataset.i], b = others(c), onlyHead = c.refs.some((x) => x.t === 'head' && x.n !== 'HEAD');
      if (b.length) checkoutRow(c); else if (!onlyHead && !c.refs.some((x) => x.t === 'head')) G.checkout(menuHost(), { commit: c });
    });
    body.addEventListener('contextmenu', (e) => {
      const r = e.target.closest('.lrow'); if (!r || !menuHost()) return; select(+r.dataset.i, false);
      /* right-click on a branch badge: that branch's own menu (the most direct way to pick the branch) */
      const rb = e.target.closest('.ref[data-rn]');
      if (rb && rb.dataset.rn !== 'HEAD' && ['head', 'br', 'rm'].includes(rb.dataset.rt)) { G.menu(menuHost(), e, branchMenuItems(menuHost(), { n: rb.dataset.rn, remote: rb.dataset.rt === 'rm' })); return; }
      G.menu(menuHost(), e, rowMenu(commits[+r.dataset.i], e)); });
    body.addEventListener('keydown', (e) => {
      const page = Math.floor(body.clientHeight / RH) - 1;
      const k = { ArrowDown: 1, ArrowUp: -1, PageDown: page, PageUp: -page, j: 1, k: -1 }[e.key];
      if (k) { select(sel + k); e.preventDefault(); }
      else if (e.key === 'Home') { select(0); e.preventDefault(); }
      else if (e.key === 'End') { select(commits.length - 1); e.preventDefault(); }
      else if ((e.ctrlKey || e.metaKey) && e.key === 'c') { try { navigator.clipboard.writeText(commits[sel].full); } catch (x) {} }
    });
    const fit = () => { gw = Math.max(40, Math.min(geo.width + 8, 420)); render(); };
    more.addEventListener('click', fit);
    grip.addEventListener('dblclick', fit);
    grip.addEventListener('mousedown', (e) => {
      e.preventDefault(); grip.classList.add('drag');
      const x0 = e.clientX, w0 = gw, s = G.scale(host);
      const mv = (ev) => { gw = Math.max(40, Math.min(480, w0 + (ev.clientX - x0) / s)); render(); };
      const up = () => { grip.classList.remove('drag'); removeEventListener('mousemove', mv); removeEventListener('mouseup', up); };
      addEventListener('mousemove', mv); addEventListener('mouseup', up);
    });
    /* graph header menu: collapse idle lanes (check when on, lanes icon when off) and fit the graph to its content (width + 8, clamped 40-420) */
    head.querySelector('.gh').addEventListener('contextmenu', (e) => {
      const w = menuHost(); if (!w) return;
      G.menu(w, e, [{ label: 'Collapse idle lanes', icon: collapse ? 'check' : 'lanes', onClick: () => { collapse = !collapse; geo = G.geometry(lay, collapse); render(); o.onCollapse && o.onCollapse(collapse); } }, { label: 'Fit graph to content', icon: 'split', onClick: fit }]);
    });
    api.select = select;
    api.selectCommit = (c) => { const i = commits.indexOf(c); if (i >= 0) select(i); };
    api.branch = () => bfName;
    /* ---- branch filter: "Show all commits of {branch}" ----
       Shows the branch's own commits (git log --first-parent). A branch off the main line stops at its fork point (shown dimmed);
       the main line keeps its merge commits, each with a dashed stub + "from {branch}" chip instead of pulling the merged branch's commits in. */
    const byId = new Map(allCommits.map((c) => [c.id, c]));
    const tipOf = (n) => allCommits.find((c) => c.refs.some((r) => (r.t === 'br' || r.t === 'head' || r.t === 'rm') && r.n === n));
    const mergedName = (c, pid) => {
      const m = /Merge (?:remote-tracking )?branch '([^']+)'/.exec(c.summary); if (m && c.parents.indexOf(pid) === 1) return m[1];
      const pc = byId.get(pid), r = pc && pc.refs.find((x) => x.t === 'br'); return r ? r.n : (pc ? pc.hash : pid);
    };
    const bar = () => host.querySelector(':scope > .bf-bar');
    function drawBar() {
      let b = bar();
      if (!bfName) { if (b) b.remove(); return; }
      if (!b) { b = document.createElement('div'); b.className = 'bf-bar'; b.setAttribute('role', 'status'); head.after(b); }
      const n = commits.length, mg = merged.size, i = bfInfo && bfInfo.into;
      b.innerHTML = `<span class="bf-l">${ic('branch', 13)}<span>Commits of</span><b class="bf-n" data-tip="${esc(bfName)}">${esc(bfName)}</b></span><span class="bf-m">${n} commit${n === 1 ? '' : 's'}${mg ? ` · ${mg} merge${mg === 1 ? '' : 's'}` : ''}</span>${i ? `<button type="button" class="bf-into" data-bf-into="${esc(i.c.id)}" data-tip="Select the merge commit ${esc(i.c.hash)}">Merged into ${esc(i.into)} · ${esc(i.c.hash)}</button>` : ''}<button type="button" class="bf-x" data-bf-clear aria-label="Show all commits">${ic('x', 11)}<span>Show all commits</span></button>`;
    }
    api.setBranch = (name) => {
      query = ''; matches = []; mi = -1;
      if (!name) { commits = allCommits; lay = fullLay; merged = new Map(); forkId = null; bfName = null; bfInfo = null; }
      else {
        const tip = tipOf(name), line = new Set(); let list = [], fork = null;
        for (let c = tipOf('main') || allCommits[0]; c; c = byId.get(c.parents[0])) line.add(c.id);
        if (tip) { const isMain = line.has(tip.id); for (let c = tip; c; c = byId.get(c.parents[0])) { list.push(c); if (!isMain && c !== tip && line.has(c.id)) { fork = c; break; } } }
        const inSet = new Set(list.map((c) => c.id));
        merged = new Map();
        list.forEach((c) => { const ns = c.parents.slice(1).filter((p) => !inSet.has(p)).map((p) => mergedName(c, p)); if (ns.length) merged.set(c.id, ns); });
        let into = null;
        if (tip && !line.has(tip.id)) { const m = allCommits.find((c) => !inSet.has(c.id) && c.parents.slice(1).some((p) => inSet.has(p))); if (m) into = { c: m, into: (tipOf('main') ? 'main' : G.headName()) }; }
        commits = list; bfName = name; forkId = fork && fork.id; bfInfo = { into };
        lay = G.layout(list.map((c) => ({ id: c.id, parents: c.parents.filter((p) => inSet.has(p)) })));
        lay.rows.forEach((r) => { r.merge = merged.has(r.id); r.stub = r.merge; });
      }
      geo = G.geometry(lay, collapse); sel = 0; body.scrollTop = 0; drawBar(); render();
      body.setAttribute('aria-activedescendant', 'cm-0');
      if (commits.length && o.onSelect) o.onSelect(commits[0], 0);
      return api;
    };
    body.addEventListener('click', (e) => { const c = e.target.closest('.ref.mf'); if (c) api.setBranch(c.dataset.bf); else if (e.target.closest('[data-bf-clear]')) api.setBranch(null); });
    host.addEventListener('click', (e) => { const x = e.target.closest('.bf-bar [data-bf-clear]'), t = e.target.closest('.bf-bar [data-bf-into]'); if (x) api.setBranch(null); else if (t) { const c = byId.get(t.dataset.bfInto); api.setBranch(null); api.selectCommit(c); } });
    body.addEventListener('keydown', (e) => { if (e.key === 'Escape' && bfName) { api.setBranch(null); e.preventDefault(); } });
    if (host._bfh) document.removeEventListener('gl-branch-filter', host._bfh);
    host._bfh = (e) => { const w = menuHost(); if (w && e.detail && e.detail.win === w) api.setBranch(e.detail.branch); };
    document.addEventListener('gl-branch-filter', host._bfh);
    api.setCollapse = (b) => { collapse = b; geo = G.geometry(lay, collapse); render(); };
    api.setWidth = (w) => { gw = w; render(); };
    api.geo = () => geo;
    api.refresh = paint;
    api.collapsed = () => collapse;
    api.setRow = (n) => { RH = n; host.style.setProperty('--row', n + 'px'); render(); };
    api.setMono = (b) => host.classList.toggle('mono', !!b);
    api.setLine = (w) => host.style.setProperty('--line-w', w + 'px');
    /* in-list search: case-insensitive substring of summary / author / e-mail, plus hash prefix for 4-40 hex characters */
    api.search = (q) => {
      query = (q || '').trim().toLowerCase(); matches = [];
      if (query) { const hx = /^[0-9a-f]{4,40}$/.test(query); commits.forEach((c, i) => { if (c.summary.toLowerCase().includes(query) || c.author.toLowerCase().includes(query) || (c.email || '').toLowerCase().includes(query) || (hx && (c.full || c.hash).startsWith(query))) matches.push(i); }); }
      mi = matches.length ? 0 : -1;
      if (mi >= 0) select(matches[0]); else paint();
      return api.stat();
    };
    api.step = (d) => { if (!matches.length) return api.stat(); const cur = matches.indexOf(sel); mi = ((cur < 0 ? (d > 0 ? -1 : 0) : cur) + d + matches.length) % matches.length; select(matches[mi]); return api.stat(); };
    api.stat = () => ({ query, total: matches.length, pos: matches.indexOf(sel) >= 0 ? matches.indexOf(sel) + 1 : 0 });
    api.strip = (html) => {
      let s = host.querySelector(':scope > .strip-slot');
      if (!html) { if (s) s.remove(); paint(); return; }
      if (!s) { s = document.createElement('div'); s.className = 'strip-slot'; head.after(s); }
      s.innerHTML = html; paint();
    };
    render();
    if (o.onSelect) o.onSelect(commits[sel], sel);
    return api;
  };

  /* ---------- overlays: layer, menu, tooltip, toast ---------- */
  G.layer = (win) => { let l = win.querySelector(':scope > .layer'); if (!l) { l = document.createElement('div'); l.className = 'layer'; win.appendChild(l); } return l; };
  G.pos = (win, e) => { const r = win.getBoundingClientRect(), s = r.width / win.offsetWidth || 1; return { x: (e.clientX - r.left) / s, y: (e.clientY - r.top) / s }; };
  G.menu = function (win, e, items) {
    if (e.preventDefault) e.preventDefault();
    win.querySelectorAll('.menu').forEach((m) => m.remove());
    const layer = G.layer(win), p = G.pos(win, e);
    const m = G.h(`<div class="menu" role="menu" tabindex="-1">${items.map((it, i) => it.sep ? '<div class="menu-sep" role="separator"></div>' : `<div class="menu-item${it.danger ? ' danger' : ''}${it.disabled ? ' disabled' : ''}" role="menuitem"${it.disabled ? ' aria-disabled="true"' : ''} data-i="${i}">${it.icon ? ic(it.icon, 14) : '<span class="menu-gap"></span>'}<span class="menu-label">${esc(it.label)}</span>${it.key ? `<span class="menu-key">${esc(it.key)}</span>` : ''}</div>`).join('')}</div>`);
    layer.appendChild(m);
    m.style.left = Math.max(4, Math.min(p.x, win.offsetWidth - m.offsetWidth - 4)) + 'px';
    m.style.top = Math.max(4, Math.min(p.y, win.offsetHeight - m.offsetHeight - 4)) + 'px';
    m.focus();
    const its = Array.from(m.querySelectorAll('.menu-item:not(.disabled)')); let k = -1;
    const hi = (n) => { if (!its.length) return; its.forEach((x) => x.classList.remove('hi')); k = (n + its.length) % its.length; its[k].classList.add('hi'); };
    const out = (ev) => { if (!m.contains(ev.target)) close(); };
    const close = () => { m.remove(); document.removeEventListener('mousedown', out, true); };
    m.addEventListener('keydown', (ev) => {
      if (ev.key === 'ArrowDown') { hi(k + 1); ev.preventDefault(); }
      else if (ev.key === 'ArrowUp') { hi(k < 0 ? -1 : k - 1); ev.preventDefault(); }
      else if (ev.key === 'Escape') close();
      else if (ev.key === 'Enter' && k >= 0) its[k].click();
    });
    m.addEventListener('click', (ev) => { const it = ev.target.closest('.menu-item'); if (!it || it.classList.contains('disabled')) return; const d = items[+it.dataset.i]; close(); if (d.onClick) d.onClick(); });
    m.addEventListener('mousemove', (ev) => { const it = ev.target.closest('.menu-item:not(.disabled)'); if (it) { its.forEach((x) => x.classList.remove('hi')); it.classList.add('hi'); k = its.indexOf(it); } });
    setTimeout(() => document.addEventListener('mousedown', out, true), 0);
    return close;
  };
  /* tooltip: the platform tooltip, 500ms delay, placed at the cursor (decision 4) */
  G.initTips = function (win) {
    const layer = G.layer(win); let t, tip, last, mx = 0, my = 0;
    const hide = () => { clearTimeout(t); if (tip) { tip.remove(); tip = null; } last = null; };
    win.addEventListener('mousemove', (e) => { mx = e.clientX; my = e.clientY; });
    win.addEventListener('mouseover', (e) => {
      const el = e.target.closest && e.target.closest('[data-tip]');
      if (el === last) return;
      hide(); if (!el) return; last = el; mx = e.clientX; my = e.clientY;
      t = setTimeout(() => {
        tip = G.h(`<div class="tip" role="tooltip" style="white-space:pre-line">${esc(el.dataset.tip)}${el.dataset.kbd ? `<span class="kbd">${esc(el.dataset.kbd)}</span>` : ''}</div>`);
        layer.appendChild(tip);
        const wr = win.getBoundingClientRect(), s = wr.width / win.offsetWidth || 1;
        let x = (mx - wr.left) / s + 4, y = (my - wr.top) / s + 20;
        x = Math.max(4, Math.min(x, win.offsetWidth - tip.offsetWidth - 6));
        if (y + tip.offsetHeight > win.offsetHeight - 4) y = (my - wr.top) / s - tip.offsetHeight - 8;
        tip.style.left = x + 'px'; tip.style.top = y + 'px';
      }, 500);
    });
    win.addEventListener('mouseleave', hide); win.addEventListener('mousedown', hide);
  };
  /* toasts: error toasts stay until dismissed, all others hide after 5 s (decision 2) */
  G.toast = function (win, msg, kind) {
    const layer = G.layer(win); let c = layer.querySelector('.toasts');
    if (!c) { c = G.h('<div class="toasts"></div>'); layer.appendChild(c); }
    const t = G.h(`<div class="toast ${kind || 'info'}" role="status">${ic(kind === 'error' ? 'err' : kind === 'warn' ? 'warn' : kind === 'ok' ? 'check' : 'info', 14)}<span class="toast-msg">${esc(msg)}</span><button class="btn btn-icon btn-ghost btn-xs" aria-label="Dismiss">${ic('x', 12)}</button></div>`);
    t.querySelector('button').onclick = () => t.remove(); c.appendChild(t); if (kind !== 'error') setTimeout(() => t.remove(), 5000);
  };
  /* max may be a number or a function; dblclick resets to `reset` */
  G.resizer = function (handle, target, axis, min, max, invert, reset) {
    const lim = () => (typeof max === 'function' ? max() : max);
    handle.addEventListener('mousedown', (e) => {
      e.preventDefault(); handle.classList.add('drag');
      const p0 = axis === 'x' ? e.clientX : e.clientY, s0 = axis === 'x' ? target.offsetWidth : target.offsetHeight, sc = G.scale(handle);
      const mv = (ev) => { const d = ((axis === 'x' ? ev.clientX : ev.clientY) - p0) / sc * (invert ? -1 : 1); const v = Math.max(min, Math.min(lim(), s0 + d)); target.style[axis === 'x' ? 'width' : 'height'] = v + 'px'; };
      const up = () => { handle.classList.remove('drag'); removeEventListener('mousemove', mv); removeEventListener('mouseup', up); };
      addEventListener('mousemove', mv); addEventListener('mouseup', up);
    });
    if (reset != null) handle.addEventListener('dblclick', () => { target.style[axis === 'x' ? 'width' : 'height'] = reset + 'px'; });
  };

  /* ---------- window shell: toolbar, sidebar, status bar ---------- */
  function node(lvl, icon, label, o) {
    o = o || {};
    const tr = o.gone ? '<span class="st gone">gone</span>' : (o.ahead != null || o.behind != null) ? `<span class="ab">${o.ahead ? `<span class="up" aria-label="${o.ahead} ahead">↑${o.ahead}</span>` : ''}${o.behind ? `<span class="dn" aria-label="${o.behind} behind">↓${o.behind}</span>` : ''}</span>` : o.state ? `<span class="st">${esc(o.state)}</span>` : '';
    const cnt = o.count != null ? `<span class="ncount" data-n="${o.count}">${o.count}</span>` : '';
    return `<div class="node${o.sec ? ' sec' : ''}${o.cur ? ' cur' : ''}" role="treeitem" tabindex="-1" style="--lvl:${lvl}" data-kind="${o.kind || ''}" data-name="${esc(o.name || label)}" data-q="${esc((o.q || (o.name || label)).toLowerCase())}"${o.exp != null ? ` aria-expanded="${o.exp}"` : ''}${o.cur ? ' aria-current="true"' : ''}><span class="twisty">${o.exp == null ? '' : ic(o.exp ? 'chevD' : 'chevR', 12)}</span>${icon ? ic(icon, 14) : ''}<span class="nlabel">${esc(label)}</span>${tr}${cnt}</div>`;
  }
  const LONG = 'release/0.3-hotfix-for-the-detached-head-checkout-crash-on-windows';
  G.sidebarHTML = function () {
    return [
      node(0, '', 'Local branches', { sec: 1, kind: 'sec-local', exp: true, count: 6 }),
      node(1, 'branch', 'main', { cur: 1, kind: 'branch', ahead: 4 }),
      node(1, 'branch', LONG, { kind: 'branch', name: LONG }),
      node(1, 'folder', 'feat', { exp: true, kind: 'folder' }),
      node(2, 'branch', 'idle-lanes', { kind: 'branch', name: 'feat/idle-lanes', ahead: 1 }),
      node(2, 'branch', 'palette', { kind: 'branch', name: 'feat/palette', behind: 2 }),
      node(1, 'folder', 'fix', { exp: true, kind: 'folder' }),
      node(2, 'branch', 'git-detect', { kind: 'branch', name: 'fix/git-detect' }),
      node(2, 'branch', 'old-experiment', { kind: 'branch', name: 'fix/old-experiment', gone: 1 }),
      node(0, '', 'Remotes', { sec: 1, kind: 'sec-remotes', exp: true, count: 2 }),
      node(1, 'cloud', 'origin', { exp: true, kind: 'remote' }),
      node(2, 'branch', 'main', { kind: 'rbranch', name: 'origin/main' }),
      node(2, 'branch', 'feat/idle-lanes', { kind: 'rbranch', name: 'origin/feat/idle-lanes' }),
      node(2, 'branch', 'fix/git-detect', { kind: 'rbranch', name: 'origin/fix/git-detect' }),
      node(1, 'cloud', 'upstream', { exp: false, kind: 'remote' }),
      node(2, 'branch', 'main', { kind: 'rbranch', name: 'upstream/main' }),
      node(0, '', 'Tags', { sec: 1, kind: 'sec-tags', exp: true, count: 2 }),
      node(1, 'tag', 'v0.3.0', { kind: 'tag' }),
      node(1, 'tag', 'v0.2.0', { kind: 'tag' }),
      node(0, '', 'Stashes', { sec: 1, kind: 'sec-stash', exp: true, count: 2 }),
      node(1, 'stash', 'WIP on feat/idle-lanes', { kind: 'stash', name: 'stash@{0}', q: 'WIP on feat/idle-lanes stash@{0}' }),
      node(1, 'stash', 'Try a different tree icon', { kind: 'stash', name: 'stash@{1}', q: 'Try a different tree icon stash@{1}' }),
      node(0, '', 'Worktrees', { sec: 1, kind: 'sec-wt', exp: true, count: 5 }),
      node(1, 'worktree', 'gitwarp', { cur: 1, kind: 'wt', state: 'main', q: 'gitwarp ~/work/gitwarp' }),
      node(1, 'worktree', 'gitwarp-hotfix', { kind: 'wt', state: 'fix/git-detect', q: 'gitwarp-hotfix ~/work/gitwarp-hotfix' }),
      node(1, 'worktree', 'gitwarp-review', { kind: 'wt', state: 'locked', q: 'gitwarp-review ~/work/gitwarp-review' }),
      node(1, 'worktree', 'gitwarp-bisect', { kind: 'wt', state: 'detached', q: 'gitwarp-bisect ~/work/gitwarp-bisect' }),
      node(1, 'worktree', 'gitwarp-old', { kind: 'wt', state: 'missing', q: 'gitwarp-old ~/work/gitwarp-old' }),
      node(0, '', 'Submodules', { sec: 1, kind: 'sec-sub', exp: true, count: 2 }),
      node(1, 'box', 'vendor/tree-sitter-rust', { kind: 'sub', state: 'modified' }),
      node(1, 'box', 'vendor/gpui-pre', { kind: 'sub', state: 'not initialized' })
    ].join('');
  };
  const WTPATH = (n) => '~/work/' + n;
  /* context menus with the app's real items, labels and disabled states; shortcut hints are the app's bindings (Settings → Keybindings) */
  const MENUS = (win) => ({
    branch: (n) => { const cur = n === G.state.head, h = G.headName(); return [
      { label: 'Checkout', icon: 'check', key: 'Enter', disabled: cur, onClick: () => G.checkout(win, { branch: n }) },
      { label: `Show all commits of ${n}`, icon: 'branch', onClick: () => document.dispatchEvent(new CustomEvent('gl-branch-filter', { detail: { win, branch: n } })) },
      { label: `Merge ${n} into ${h}`, icon: 'branch', disabled: true },
      { label: `Rebase ${h} onto ${n}`, icon: 'branch', disabled: true },
      { sep: 1 },
      { label: 'Rename…', icon: 'edit', key: 'F2', onClick: () => G.act(win, 'renameBranch', n) },
      { label: 'Set upstream', icon: 'link', onClick: () => (G.state.remotes.includes(n) ? G.toast(win, `${n} now tracks origin/${n}`, 'ok') : G.toast(win, `There is no origin/${n} yet. Push ${n} first to create it.`, 'error')) },
      { label: 'Delete branch', icon: 'trash', key: 'Del', danger: 1, disabled: cur, onClick: () => G.act(win, 'deleteBranch', n) },
      { sep: 1 },
      { label: 'Copy branch name', icon: 'copy', onClick: () => { try { navigator.clipboard.writeText(n); } catch (e) {} } }]; },
    rbranch: (n) => [
      { label: 'Checkout as local branch', icon: 'check', onClick: () => G.checkout(win, { remote: n }) },
      { label: `Show all commits of ${n}`, icon: 'branch', onClick: () => document.dispatchEvent(new CustomEvent('gl-branch-filter', { detail: { win, branch: n } })) },
      { label: `Merge ${n} into ${G.headName()}`, icon: 'branch', disabled: true },
      { sep: 1 },
      { label: 'Delete remote branch…', icon: 'trash', danger: 1, onClick: () => G.act(win, 'deleteRemoteBranch', n) },
      { label: 'Copy branch name', icon: 'copy', onClick: () => { try { navigator.clipboard.writeText(n); } catch (e) {} } }],
    tag: (n) => { const c = G.commits.find((x) => x.refs.some((r) => r.t === 'tag' && r.n === n)); return [
      { label: 'Checkout', icon: 'check', onClick: () => (c ? G.checkout(win, { commit: c, tag: n }) : G.toast(win, 'The tag has no commit to check out', 'warn')) },
      { label: 'Create branch here…', icon: 'branch', disabled: !c, onClick: () => window.GLD && window.GLD.openIn('branch', win) },
      { label: 'Push tag to origin', icon: 'up', onClick: () => win._shell.net.run('tags') },
      { sep: 1 },
      { label: 'Rename tag…', icon: 'edit', onClick: () => G.act(win, 'renameTag', n) },
      { label: 'Delete tag', icon: 'trash', danger: 1, onClick: () => G.act(win, 'deleteTag', n) }]; },
    stash: (n) => [
      { label: 'Apply stash', icon: 'check', onClick: () => G.toast(win, `Applied ${n}`, 'ok') },
      { label: 'Pop stash', icon: 'undo', onClick: () => G.toast(win, `Popped ${n}`, 'ok') },
      { label: 'View changes', icon: 'eye', onClick: () => G.toast(win, 'Showing the stash in the details dock', 'info') },
      { sep: 1 },
      { label: 'Drop stash', icon: 'trash', danger: 1, onClick: () => G.act(win, 'dropStash', n) }],
    wt: (n, el) => { const locked = el && /locked/.test(el.textContent), cur = el && el.classList.contains('cur'); return [
      { label: 'Open in new window', icon: 'folder', onClick: () => G.toast(win, `Opened ${WTPATH(n)} in a new window`, 'ok') },
      { label: 'Rename worktree…', icon: 'edit', onClick: () => G.act(win, 'renameWorktree', n) },
      { label: 'Manage worktrees…', icon: 'worktree', onClick: () => { location.href = 'worktrees.html'; } },
      { label: 'Add worktree…', icon: 'plus', onClick: () => window.GLD && window.GLD.openIn('worktreeAdd', win) },
      { sep: 1 },
      { label: locked ? 'Unlock' : 'Lock', icon: 'lock', onClick: () => G.toast(win, `${locked ? 'Unlocked' : 'Locked'} ${WTPATH(n)}`, 'ok') },
      { label: 'Remove worktree…', icon: 'trash', danger: 1, disabled: cur, onClick: () => window.GLD && window.GLD.openIn('worktreeRemove', win) }]; },
    remote: (n) => [
      { label: 'Fetch', icon: 'fetch', onClick: () => win._shell.net.run('fetch') },
      { label: 'Edit remote…', icon: 'edit', onClick: () => G.act(win, 'editRemote', n) },
      { label: 'Remove remote', icon: 'trash', danger: 1, onClick: () => G.act(win, 'removeRemote', n) }],
    sub: () => [{ label: 'Update submodule', icon: 'fetch', onClick: () => G.toast(win, 'Updated submodule vendor/a', 'ok') }, { label: 'Open in new window', icon: 'folder', onClick: () => G.toast(win, 'Opened vendor/a in a new window', 'ok') }],
    'sec-remotes': () => [
      { label: 'Add remote…', icon: 'plus', onClick: () => window.GLD && window.GLD.openIn('remoteAdd', win) },
      { sep: 1 },
      { label: 'Fetch all remotes', icon: 'fetch', key: 'Ctrl+Shift+F', onClick: () => win._shell.net.run('fetch') },
      { label: 'Prune stale remote branches', icon: 'trash', onClick: () => win._shell.net.run('fetch') }],
    'sec-tags': () => [
      { label: 'Create tag…', icon: 'plus', onClick: () => window.GLD && window.GLD.openIn('tag', win) },
      { sep: 1 },
      { label: 'Fetch tags from origin', icon: 'fetch', onClick: () => win._shell.net.run('fetch') },
      { label: 'Push all tags to origin', icon: 'up', onClick: () => win._shell.net.run('tags') }],
    'sec-wt': () => [
      { label: 'Manage worktrees…', icon: 'worktree', onClick: () => { location.href = 'worktrees.html'; } },
      { label: 'Add worktree…', icon: 'plus', onClick: () => window.GLD && window.GLD.openIn('worktreeAdd', win) },
      { sep: 1 },
      { label: 'Prune missing worktrees', icon: 'trash', onClick: () => G.toast(win, 'Pruned stale worktree records', 'ok') },
      { label: 'Refresh list', icon: 'fetch' }]
  });
  /* re-derive which nodes are visible from the expansion state (used after filtering) */
  function applyVis(tree) {
    let skip = null;
    Array.from(tree.querySelectorAll('.node')).forEach((n) => {
      const l = +n.style.getPropertyValue('--lvl');
      if (skip !== null && l > skip) { n.hidden = true; return; }
      skip = null; n.hidden = false;
      if (n.hasAttribute('aria-expanded') && n.getAttribute('aria-expanded') === 'false') skip = l;
    });
  }
  function toggleNode(n, tree) {
    const open = n.getAttribute('aria-expanded') !== 'true';
    n.setAttribute('aria-expanded', open); n.querySelector('.twisty').innerHTML = ic(open ? 'chevD' : 'chevR', 12);
    applyVis(tree);
  }
  const NETV = { fetch: 'Fetching', pull: 'Pulling', push: 'Pushing', tags: 'Pushing tags', delbranch: 'Deleting remote branch' };
  const SETTINGS_PAGES = [['gear', 'General', 'settings-general.html', 'Language, startup, updates, background fetch, confirmations and privacy.'], ['eye', 'Appearance', 'settings-appearance.html', 'Theme, accent, row density, fonts and the graph style (monochrome lanes, line width).'], ['git', 'Git', 'settings-git.html', 'Which git executable GitWarp uses: Auto, Bundled, System or a custom path.'], ['key', 'SSH keys', 'settings-ssh.html', 'Keys, the agent and trusted hosts.'], ['keyboard', 'Keybindings', 'settings-keybindings.html', 'Searchable shortcut table, recording, conflicts, reset and export.'], ['split', 'Diff &amp; merge', 'settings-diff.html', 'Layout, whitespace, context lines, line numbers and the merge tool.'], ['user', 'Identity', 'settings-identity.html', 'Author, signing key and hosting accounts.']];
  G.shell = function (o) {
    const repo = o.repo || 'gitwarp', view = o.view || 'history';
    const win = G.h(`<div class="win" style="width:${o.w}px;height:${o.h}px">
      <header class="titlebar">
        <button class="btn btn-icon btn-ghost" data-act="sidebar" data-tip="Toggle sidebar" data-kbd="Ctrl B" aria-label="Toggle sidebar">${ic('sidebar', 16)}</button>
        <button class="btn btn-tool" data-act="repo" data-tip="Switch repository">${ic('folder', 14)}<b class="tb-name">${esc(repo)}</b>${ic('chevD', 12)}</button>
        <button class="btn btn-tool" data-act="branch" data-tip="Switch branch" data-kbd="Ctrl Shift B">${ic('branch', 14)}<b class="tb-name" id="tb-branch">${esc(G.headLabel())}</b>${ic('chevD', 12)}</button>
        <span class="tb-sep"></span>
        <button class="btn btn-tool net" data-act="fetch" data-tip="Fetch all remotes, tags and pruned branches">${ic('fetch', 14)}<span class="lbl">Fetch</span></button>
        <button class="btn btn-tool net" data-act="pull" data-tip="Pull the current branch (fast-forward only) · Ctrl+Shift+L">${ic('down', 14)}<span class="lbl">Pull</span></button>
        <button class="btn btn-tool net" data-act="push" data-tip="Push the current branch · Ctrl+Shift+P">${ic('up', 14)}<span class="lbl">Push</span></button>
        <button class="btn btn-tool" data-act="stash" data-tip="Stash local changes" data-kbd="Ctrl Shift S">${ic('stash', 14)}<span class="lbl">Stash</span><span class="counter">2</span></button>
        <span class="od-fill"></span>
        <div class="seg" role="tablist" aria-label="View"><button role="tab" class="${view === 'history' ? 'on' : ''}" data-tip="History" onclick="location.href='main-1200.html'">${ic('lanes', 14)}<span class="lbl">History</span></button><button role="tab" class="${view === 'changes' ? 'on' : ''}" data-tip="Local changes" onclick="location.href='changes.html'">${ic('edit', 14)}<span class="lbl">Changes</span></button><button role="tab" class="${view === 'worktrees' ? 'on' : ''}" data-tip="Worktrees" data-kbd="Ctrl Alt W" onclick="location.href='worktrees.html'">${ic('worktree', 14)}<span class="lbl">Worktrees</span></button></div>
        <label class="input-wrap tb-search" data-tip="Search commits, authors, hashes" data-kbd="Ctrl K">${ic('search', 14)}<input class="input" placeholder="Search commits, authors, hashes" aria-label="Search commits, authors, hashes"><span class="kbd tb-kbd">Ctrl K</span><span class="tb-stat" hidden></span></label>
        <button class="btn btn-icon btn-ghost" data-act="settings" data-tip="Settings" data-kbd="Ctrl ," aria-label="Settings">${ic('gear', 16)}</button>
        <div class="wc"><button class="btn btn-ghost" aria-label="Minimise">${ic('min', 14)}</button><button class="btn btn-ghost" aria-label="Maximise">${ic('max', 14)}</button><button class="btn btn-ghost close" aria-label="Close">${ic('x', 14)}</button></div>
      </header>
      <div class="win-body">
        <aside class="sidebar" aria-label="Repository">
          <div class="sbfilter"><label class="input-wrap">${ic('search', 14)}<input class="input" placeholder="Filter branches, tags…" aria-label="Filter sidebar"></label></div>
          <div class="tree" role="tree" tabindex="0">${G.sidebarHTML()}</div>
        </aside>
        <div class="splitter v" role="separator" aria-orientation="vertical" aria-label="Resize sidebar"></div>
        <main class="center"></main>
      </div>
      <footer class="statusbar">
        <span class="op" hidden></span>
        <span class="sb-state" hidden></span>
        <span class="od-fill"></span>
        <span class="sb-info">${o.statusInfo || '21 commits loaded'}</span>
        <span class="sb-git" data-tip="git 2.47.1 · bundled · /opt/gitwarp/git/bin/git">${ic('git', 12)}<span>git 2.47.1 · bundled</span></span>
      </footer>
    </div>`);
    o.host.appendChild(win);
    const q = (s) => win.querySelector(s);
    const sb = q('.sidebar'), tree = q('.tree'), split = q('.splitter.v'), titlebar = q('.titlebar');
    const sh = { win, center: q('.center'), search: q('.tb-search input'), onReveal: null };
    win._shell = sh;
    /* sidebar: resizable up to half the window, double-click resets to 240 */
    G.resizer(split, sb, 'x', 160, () => Math.max(160, win.offsetWidth / 2), false, 240);
    q('[data-act=sidebar]').onclick = () => { sb.classList.toggle('collapsed'); split.hidden = sb.classList.contains('collapsed'); };

    /* ---- network job: status-bar segment + disabled Fetch / Pull / Push ---- */
    const op = q('.op'), netBtns = win.querySelectorAll('.btn.net'); let netTimers = [];
    sh.net = {
      busy: false,
      set(kind, detail, frac) {
        sh.net.busy = true; netBtns.forEach((b) => { b.disabled = true; });
        op.hidden = false;
        op.innerHTML = `${ic('fetch', 12, 'spin')}<span>${NETV[kind]}</span><span class="op-detail">${esc(detail || 'Connecting…')}</span><span class="progress${frac == null ? ' indet' : ''}"><i${frac == null ? '' : ` style="width:${Math.round(frac * 100)}%"`}></i></span>`;
      },
      clear() { netTimers.forEach(clearTimeout); netTimers = []; sh.net.busy = false; netBtns.forEach((b) => { b.disabled = false; }); op.hidden = true; op.innerHTML = ''; },
      run(kind) {
        if (sh.net.busy) return;
        sh.net.set(kind, 'Connecting…', null);
        const steps = [[900, 'remote: Counting objects: 100% (34/34), done.', null], [1700, 'Receiving objects: 38% (13/34)', 0.38], [2500, 'Resolving deltas: 80% (8/10)', 0.8]];
        steps.forEach(([ms, d, f]) => netTimers.push(setTimeout(() => sh.net.set(kind, d, f), ms)));
        netTimers.push(setTimeout(() => { sh.net.clear(); G.toast(win, { fetch: 'Fetch complete', pull: 'Pull complete', push: 'Pushed the current branch', tags: 'Pushed tags', delbranch: 'Deleted the remote branch' }[kind], 'ok'); }, 3300));
      }
    };
    /* ---- status bar left/right ---- */
    const sbs = q('.sb-state'), sbi = q('.sb-info'), sbg = q('.sb-git');
    sh.setStatus = (s) => {
      s = s || {};
      if (!s.left) { sbs.hidden = true; sbs.className = 'sb-state'; sbs.innerHTML = ''; }
      else { sbs.hidden = false; sbs.className = 'sb-state' + (s.left[0] === 'err' ? ' err' : ''); sbs.innerHTML = `${s.left[0] === 'load' ? ic('fetch', 12, 'spin') : ''}<span>${esc(s.left[1])}</span>`; }
      if (s.info !== undefined) { sbi.textContent = s.info; sbi.hidden = !s.info; }
      if (s.git) { sbg.className = 'sb-git' + (s.git === 'old' ? ' warn' : s.git === 'none' ? ' err' : ''); sbg.querySelector('span').textContent = { ok: 'git 2.47.1 · bundled', old: 'git 2.28.1 too old', none: 'git not found' }[s.git]; sbg.dataset.tip = { ok: 'git 2.47.1 · bundled · /opt/gitwarp/git/bin/git', old: 'git 2.28.1 is too old', none: 'git was not found' }[s.git]; }
    };
    sh.setBranchLabel = () => { q('#tb-branch').textContent = G.headLabel(); };

    /* ---- sidebar tree ---- */
    const clearSel = () => tree.querySelectorAll('.node.sel').forEach((x) => x.classList.remove('sel'));
    const setCur = () => {
      tree.querySelectorAll('.node[data-kind=branch]').forEach((n) => { const cur = n.dataset.name === G.state.head; n.classList.toggle('cur', cur); if (cur) n.setAttribute('aria-current', 'true'); else n.removeAttribute('aria-current'); });
    };
    document.addEventListener('gl-head', () => { setCur(); sh.setBranchLabel(); });
    tree.addEventListener('click', (e) => {
      const n = e.target.closest('.node'); if (!n) return;
      if (n.hasAttribute('aria-expanded')) { if (!flt.value.trim()) toggleNode(n, tree); }
      else { clearSel(); n.classList.add('sel'); sh.onReveal && sh.onReveal(n.dataset.kind, n.dataset.name); }
    });
    tree.addEventListener('dblclick', (e) => {
      const n = e.target.closest('.node'); if (!n) return; const k = n.dataset.kind, nm = n.dataset.name;
      if (k === 'branch') { if (nm !== G.state.head) G.checkout(win, { branch: nm }); }
      else if (k === 'rbranch') G.checkout(win, { remote: nm });
      else if (k === 'tag') { const c = G.commits.find((x) => x.refs.some((r) => r.t === 'tag' && r.n === nm)); if (c) G.checkout(win, { commit: c, tag: nm }); else G.toast(win, 'The tag has no commit to check out', 'warn'); }
      else if (k === 'wt') location.href = 'worktrees.html';
    });
    /* right-click selects the row without jumping to its commit; folders and the Local branches / Stashes / Submodules headers have no menu */
    tree.addEventListener('contextmenu', (e) => { const n = e.target.closest('.node'); const mk = n && MENUS(win)[n.dataset.kind]; if (!mk) return; clearSel(); n.classList.add('sel'); G.menu(win, e, mk(n.dataset.name, n)); });
    tree.addEventListener('keydown', (e) => {
      const vis = Array.from(tree.querySelectorAll('.node')).filter((x) => !x.hidden);
      let i = vis.findIndex((x) => x.classList.contains('kf'));
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { i = Math.max(0, Math.min(vis.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1))); vis.forEach((x) => x.classList.remove('kf')); vis[i].classList.add('kf'); vis[i].scrollIntoView({ block: 'nearest' }); e.preventDefault(); }
      else if (e.key === 'ArrowRight' && i >= 0 && vis[i].getAttribute('aria-expanded') === 'false') { toggleNode(vis[i], tree); e.preventDefault(); }
      else if (e.key === 'ArrowLeft' && i >= 0 && vis[i].getAttribute('aria-expanded') === 'true') { toggleNode(vis[i], tree); e.preventDefault(); }
      else if ((e.key === 'Enter' || e.key === ' ') && i >= 0) { vis[i].click(); e.preventDefault(); }
    });
    /* filter: keeps the folders and remotes above each match (forced open), a section's count becomes its number of matches, empty sections disappear */
    const flt = q('.sbfilter input');
    let nomatch = null;
    function filter() {
      const s = flt.value.trim().toLowerCase(); if (nomatch) { nomatch.remove(); nomatch = null; }
      const nodes = Array.from(tree.querySelectorAll('.node'));
      if (!s) { nodes.forEach((n) => { const c = n.querySelector(':scope > .ncount'); if (c && n.classList.contains('sec')) c.textContent = c.dataset.n; }); applyVis(tree); return; }
      let total = 0, i = 0;
      while (i < nodes.length) {
        const sec = nodes[i]; let j = i + 1; while (j < nodes.length && !nodes[j].classList.contains('sec')) j++;
        const body = nodes.slice(i + 1, j), hit = new Set();
        body.forEach((n, k) => { if (n.hasAttribute('aria-expanded')) return; if (n.dataset.q.includes(s)) { hit.add(n); const l = +n.style.getPropertyValue('--lvl'); let need = l - 1; for (let x = k - 1; x >= 0 && need >= 1; x--) if (+body[x].style.getPropertyValue('--lvl') === need) { hit.add(body[x]); need--; } } });
        const matches = body.filter((n) => !n.hasAttribute('aria-expanded') && hit.has(n)).length;
        total += matches; sec.hidden = !matches; body.forEach((n) => { n.hidden = !hit.has(n); });
        const c = sec.querySelector(':scope > .ncount'); if (c) c.textContent = matches;
        i = j;
      }
      if (!total) { nomatch = G.h('<div class="nomatch">No matches</div>'); tree.appendChild(nomatch); }
    }
    flt.addEventListener('input', filter);
    /* sidebar loading: refs not loaded yet, headers without a count pill (Remotes counts its distinct prefixes) */
    sh.setSidebar = (st) => {
      tree.classList.toggle('loading', st === 'loading');
      tree.querySelectorAll('.node.sec').forEach((n) => { const c = n.querySelector(':scope > .ncount'); if (c) c.hidden = st === 'loading' && n.dataset.kind !== 'sec-remotes'; });
      if (st === 'loading') { tree.querySelectorAll('.node:not(.sec)').forEach((n) => { n.hidden = true; }); } else { filter(); }
    };
    sh.filter = filter;

    /* ---- dropdowns ---- */
    const under = (el) => { const r = el.getBoundingClientRect(); return { clientX: r.left, clientY: r.bottom + 2 }; };
    function repoMenu(el) {
      const recents = ['gitwarp', 'kovalenko-site', 'dotfiles'];
      G.menu(win, under(el), recents.map((n) => ({ label: n, icon: n === repo ? 'check' : 'folder' })).concat([{ sep: 1 },
        { label: 'Open repository…', icon: 'folder', key: 'Ctrl O', onClick: () => window.GLD && window.GLD.openIn('open', win) },
        { label: 'Worktrees…', icon: 'worktree', onClick: () => { location.href = 'worktrees.html'; } },
        { label: 'Close repository', icon: 'x', onClick: () => { location.href = 'welcome.html'; } },
        { sep: 1 },
        G.state.bgfetch ? { label: 'Fetch in the background', icon: 'check', onClick: () => { G.state.bgfetch = false; } } : { label: 'Background fetch is off', icon: 'fetch', onClick: () => { G.state.bgfetch = true; } }]));
    }
    function branchMenu(el) {
      const st = G.state.branches, names = ['main', 'feat/idle-lanes', 'feat/palette', 'fix/git-detect', LONG, 'fix/old-experiment'];
      const list = st === 'loading' ? [{ label: 'Loading branches…', icon: 'branch', disabled: true }] : st === 'none' ? [{ label: 'No local branches', disabled: true }] : names.map((n) => (n === G.state.head ? { label: n, icon: 'check', disabled: true } : { label: n, icon: 'branch', onClick: () => G.checkout(win, { branch: n }) }));
      G.menu(win, under(el), list.concat([{ sep: 1 }, { label: 'Create branch…', icon: 'plus', disabled: st === 'loading', onClick: () => window.GLD && window.GLD.openIn('branch', win) }]));
    }
    q('[data-act=repo]').onclick = (e) => repoMenu(e.currentTarget);
    q('[data-act=branch]').onclick = (e) => branchMenu(e.currentTarget);
    q('[data-act=fetch]').onclick = () => sh.net.run('fetch');
    q('[data-act=pull]').onclick = () => sh.net.run('pull');
    q('[data-act=push]').onclick = () => sh.net.run('push');
    q('[data-act=stash]').onclick = () => (G.state.dirty ? G.toast(win, 'Stashed local changes', 'ok') : G.toast(win, 'No local changes to stash', 'info'));

    /* ---- settings: a centred modal card over the window (scrim `overlay`, 32px padding, max 1080x760) ---- */
    sh.openSettings = function (page) {
      sh.closeSettings();
      let cur = Math.max(0, SETTINGS_PAGES.findIndex((p) => p[1] === page));
      const sc = G.h(`<div class="set-scrim" role="presentation"><div class="set-card" role="dialog" aria-modal="true" aria-label="Settings">
        <div class="set-tb">${ic('gear', 14)}<span>Settings</span><span class="od-fill"></span><button class="btn btn-icon btn-ghost" data-tip="Close settings (Esc)" aria-label="Close settings">${ic('x', 14)}</button><div class="wc"><button class="btn btn-ghost" aria-label="Minimise">${ic('min', 14)}</button><button class="btn btn-ghost" aria-label="Maximise">${ic('max', 14)}</button><button class="btn btn-ghost close" aria-label="Close">${ic('x', 14)}</button></div></div>
        <div class="set-body"><nav class="snav" aria-label="Settings sections"></nav><div class="spane" tabindex="0"><div></div></div></div>
        <div class="set-sb"><span>Changes are saved automatically</span><span class="od-fill"></span><span class="sb-git" style="display:inline-flex;align-items:center;gap:4px">${ic('git', 12)}git 2.47.1 · bundled</span></div></div></div>`);
      G.layer(win).appendChild(sc); sh._settings = sc;
      const nav = sc.querySelector('.snav'), pane = sc.querySelector('.spane > div');
      const draw = () => {
        nav.innerHTML = SETTINGS_PAGES.map((p, i) => `<button${i === cur ? ' aria-current="page"' : ''} data-i="${i}">${ic(p[0], 14)}<span>${p[1]}</span></button>`).join('');
        const p = SETTINGS_PAGES[cur];
        pane.innerHTML = `<h2>${p[1]}</h2><p>${p[3]}</p><a class="btn" href="${p[2]}">Open the ${p[1].replace('&amp;', '&')} page spec</a><p style="margin-top:16px;font-size:var(--fs-sm)">The full page is drawn on its own design page; this card shows only how Settings opens — a centred modal over the current screen. Click outside the card or press Esc to close.</p>`;
      };
      draw();
      nav.addEventListener('click', (e) => { const b = e.target.closest('button'); if (b) { cur = +b.dataset.i; draw(); } });
      sc.addEventListener('mousedown', (e) => { if (e.target === sc) sh.closeSettings(); });
      sc.querySelector('[aria-label="Close settings"]').onclick = () => sh.closeSettings();
      sc.querySelector('.snav button[aria-current]').focus();
    };
    sh.closeSettings = () => { if (sh._settings) { sh._settings.remove(); sh._settings = null; } };
    q('[data-act=settings]').onclick = () => sh.openSettings('General');

    /* ---- search status in the toolbar field: "3 / 17", "3 / 17+" while scanning, "– / 0+", "No matches" ---- */
    const kbdEl = q('.tb-kbd'), statEl = q('.tb-stat');
    sh.setSearchStat = (txt) => { statEl.hidden = !txt; kbdEl.hidden = !!txt; statEl.textContent = txt || ''; };

    /* ---- global keys the app binds ---- */
    addEventListener('keydown', (e) => {
      const k = e.key.toLowerCase(), ctrl = e.ctrlKey || e.metaKey;
      if (e.key === 'Escape' && sh._settings) { sh.closeSettings(); return; }
      if (!ctrl) return;
      if (k === 'b' && !e.shiftKey) { e.preventDefault(); q('[data-act=sidebar]').click(); }
      else if (k === 'b' && e.shiftKey) { e.preventDefault(); q('[data-act=branch]').click(); }
      else if (k === 'k') { e.preventDefault(); sh.search.focus(); sh.search.select(); }
      else if (k === ',') { e.preventDefault(); sh.openSettings('General'); }
      else if (k === 'o' && !e.shiftKey) { e.preventDefault(); window.GLD && window.GLD.openIn('open', win); }
      else if (k === 'f' && e.shiftKey) { e.preventDefault(); sh.net.run('fetch'); }
      else if (k === 's' && e.shiftKey) { e.preventDefault(); q('[data-act=stash]').click(); }
    });
    G.initTips(win);
    return sh;
  };
})();
