/* GitWarp settings shell: a centred modal card over the (dimmed) main window, eight sections,
   shared persistence, row builders and a small dialog controller for dialogs opened from Settings. */
(function () {
  'use strict';
  const G = window.GL, ic = G.ic, esc = G.esc;
  const S = (window.GLS = {});
  S.SECTIONS = [
    ['gear', 'General', 'settings-general.html'],
    ['eye', 'Appearance', 'settings-appearance.html'],
    ['git', 'Git', 'settings-git.html'],
    ['worktree', 'Worktrees', 'settings-worktrees.html'],
    ['key', 'SSH keys', 'settings-ssh.html'],
    ['keyboard', 'Keybindings', 'settings-keybindings.html'],
    ['split', 'Diff &amp; merge', 'settings-diff.html'],
    ['user', 'Identity', 'settings-identity.html'],
    ['cloud', 'Accounts', 'settings-accounts.html']
  ];
  S.nav = (cur) => `<nav class="snav" aria-label="Settings sections">${S.SECTIONS.map(([i, l, h]) => `<a class="node${h === cur ? ' sel' : ''}" href="${h}" style="--lvl:0"${h === cur ? ' aria-current="page"' : ''}>${ic(i, 14)}<span class="nlabel">${l}</span></a>`).join('')}</nav>`;

  /* persistence (design prototype: localStorage stands in for the settings file) */
  S.get = (k, d) => { try { const v = localStorage.getItem('gl-set-' + k); return v == null ? d : JSON.parse(v); } catch (e) { return d; } };
  S.set = (k, v) => { try { localStorage.setItem('gl-set-' + k, JSON.stringify(v)); } catch (e) {} };

  /* git label in the status bar: the same on every page ("git {version} · {source}", "git {found} too old", "git not found").
     Not clickable; the tooltip carries the full path. The Git page overrides it from its detection scenario. */
  S.GIT = { label: 'git 2.56.0 · bundled', tip: 'git 2.56.0 · bundled · /opt/gitwarp/git/bin/git', bad: false };
  S.gitHTML = () => `${ic(S.GIT.bad ? 'err' : 'git', 12)}${esc(S.GIT.label)}`;
  S.setGit = (g) => { Object.assign(S.GIT, g); const el = document.querySelector('.gw-sb-git'); if (el) { el.innerHTML = S.gitHTML(); el.dataset.tip = S.GIT.tip; } };

  /* row builders */
  S.row = (title, sub, ctl, off) => `<div class="srow${off ? ' off' : ''}"><div class="st">${title}</div>${sub ? `<div class="sd">${sub}</div>` : ''}<div class="ctl">${ctl}</div></div>`;
  S.sw = (k, on, label, dis) => `<span class="switch${on ? ' on' : ''}" role="switch" aria-checked="${!!on}" aria-label="${esc(label)}" ${dis ? 'aria-disabled="true"' : 'tabindex="0"'} data-sw="${k}"><span class="track"></span></span>`;
  S.seg = (k, cur, opts, dis) => `<div class="seg${dis ? ' dis' : ''}" role="group" data-seg="${k}">${opts.map(([v, l]) => `<button aria-pressed="${cur === v}" data-v="${v}"${dis ? ' tabindex="-1"' : ''}>${l}</button>`).join('')}</div>`;
  S.sel = (k, cur, opts, label, dis) => `<select class="input" data-sel="${k}" aria-label="${esc(label)}"${dis ? ' disabled' : ''}>${opts.map(([v, l]) => `<option value="${v}"${v === cur ? ' selected' : ''}>${l}</option>`).join('')}</select>`;
  S.chk = (k, on, label) => `<span class="check${on ? ' on' : ''}" role="checkbox" aria-checked="${!!on}" tabindex="0" data-chk="${k}"><span class="box">${on ? ic('check', 10) : ''}</span>${label}</span>`;
  S.group = (title, rows) => `<section class="sgroup"><h3>${title}</h3><div class="slist">${rows.join('')}</div></section>`;
  /* banner: title is semibold, body optional (a banner can be title-only) */
  S.banner = (kind, t, d) => `<div class="banner ${kind} stack-banner" role="${kind === 'error' ? 'alert' : 'status'}">${ic(kind === 'error' ? 'err' : kind === 'info' ? 'info' : kind === 'ok' ? 'check' : 'warn', 16)}<div style="min-width:0"><div class="b-title">${esc(t)}</div>${d ? `<div>${esc(d)}</div>` : ''}</div></div>`;
  /* read-only stepper: the value is a plain box, buttons carry "Decrease" / "Increase" tooltips */
  S.stepper = (attr, val, lo, hi, label) => `<div class="stepper"><button class="btn btn-icon btn-sm" data-${attr}="-1" data-tip="Decrease" aria-label="Decrease ${esc(label)}"${val <= lo ? ' disabled' : ''}>${ic('minus', 14)}</button><span class="rv" role="status" aria-label="${esc(label)}">${val}</span><button class="btn btn-icon btn-sm" data-${attr}="1" data-tip="Increase" aria-label="Increase ${esc(label)}"${val >= hi ? ' disabled' : ''}>${ic('plus', 14)}</button></div>`;

  /* ---------- dialogs opened from Settings ----------
     S.dialog(win, spec): spec = { st, width, view(st) -> {title, icon, body, foot}, actions: {name(st, api, el)}, busy(st), onInput(st, field) }.
     Markup hooks: data-do="<action>" (data-do="close" closes), data-c="<flag>" checkbox, data-f="<field>" input/select.
     Esc / outside click / Enter follow the dialog kit: ignored while busy; Enter runs the [data-default] button. */
  S.dialog = function (win, spec) {
    const layer = G.layer(win), st = spec.st, prev = document.activeElement;
    const scrim = G.h('<div class="scrim center"><div class="dialog dlg setdlg" role="dialog" aria-modal="true" aria-labelledby="dlg-t"></div></div>');
    const dlg = scrim.firstElementChild; dlg.style.width = (spec.width || 520) + 'px';
    layer.appendChild(scrim);
    const isBusy = () => !!(spec.busy && spec.busy(st));
    const api = { st, win, el: dlg, close, redraw, toast: (m, k) => G.toast(win, m, k || 'info') };
    function close() { document.removeEventListener('keydown', onKey, true); scrim.remove(); if (prev && prev.focus && document.body.contains(prev)) prev.focus(); }
    function redraw(first) {
      const a = document.activeElement; let sel = null, ss = 0, se = 0;
      if (a && dlg.contains(a)) { sel = a.dataset.f ? `[data-f="${a.dataset.f}"]` : a.dataset.c ? `[data-c="${a.dataset.c}"]` : a.dataset.do ? `[data-do="${a.dataset.do}"]` : null; try { ss = a.selectionStart; se = a.selectionEnd; } catch (e) {} }
      const v = spec.view(st), b = isBusy();
      dlg.innerHTML = `<div class="dialog-head">${v.icon ? ic(v.icon, 16) : ''}<span id="dlg-t">${v.title}</span><span class="od-fill"></span><button type="button" class="btn btn-icon btn-ghost btn-xs" aria-label="Close" data-tip="Close" data-do="close"${b ? ' disabled' : ''}>${ic('x', 12)}</button></div><div class="dialog-body">${v.body}</div><div class="dialog-foot">${v.foot}</div>`;
      let el = null; if (sel) { try { el = dlg.querySelector(sel); } catch (e) {} }
      if (!el && first) el = dlg.querySelector('[data-autofocus]') || dlg.querySelector('input:not([disabled]),select:not([disabled])') || dlg.querySelector('[data-default]:not([disabled])');
      if (el) { el.focus(); try { if (sel && sel.startsWith('[data-f') && el.setSelectionRange) el.setSelectionRange(ss, se); } catch (e) {} }
    }
    dlg.addEventListener('input', (e) => { const f = e.target.closest('[data-f]'); if (!f || f.tagName === 'SELECT') return; st[f.dataset.f] = f.value; if (spec.onInput) spec.onInput(st, f.dataset.f); redraw(); });
    dlg.addEventListener('change', (e) => { const f = e.target.closest('select[data-f]'); if (!f) return; st[f.dataset.f] = f.value; redraw(); });
    dlg.addEventListener('click', (e) => {
      const c = e.target.closest('[data-c]'), d = e.target.closest('[data-do]');
      if (c) { if (!isBusy()) { st[c.dataset.c] = !st[c.dataset.c]; redraw(); } return; }
      if (d && !d.disabled) { if (d.dataset.do === 'close') { if (!isBusy()) close(); return; } const r = spec.actions[d.dataset.do](st, api, d); if (r !== false && dlg.isConnected) redraw(); }
    });
    function onKey(e) {
      const all = layer.querySelectorAll(':scope > .scrim'); if (scrim !== all[all.length - 1]) return;
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); if (!isBusy()) close(); return; }
      if (e.key === 'Enter' && !e.target.closest('textarea,button,select,[role=checkbox],a')) { const b = dlg.querySelector('[data-default]:not([disabled])'); if (b && !isBusy()) { e.preventDefault(); b.click(); } }
      else if ((e.key === ' ' || e.key === 'Enter') && e.target.matches && e.target.matches('[role=checkbox]')) { e.preventDefault(); e.target.click(); }
      else if (e.key === 'Tab') {
        const fo = Array.from(dlg.querySelectorAll('button:not([disabled]),input:not([disabled]),select:not([disabled]),[tabindex="0"]')); if (!fo.length) return;
        const i = fo.indexOf(document.activeElement);
        if (e.shiftKey && i <= 0) { e.preventDefault(); fo[fo.length - 1].focus(); } else if (!e.shiftKey && (i === fo.length - 1 || i < 0)) { e.preventDefault(); fo[0].focus(); }
      }
    }
    document.addEventListener('keydown', onKey, true);
    scrim.addEventListener('mousedown', (e) => { if (e.target === scrim && !isBusy()) close(); });
    redraw(true);
    return api;
  };
  /* dialog builders */
  S.dfield = (label, id, control, help, err) => `<div class="fld2"><label class="field-label" for="${id}">${label}</label>${control}${err ? `<div class="field-error">${ic('err', 12)}<span>${esc(err)}</span></div>` : help ? `<div class="field-help">${help}</div>` : ''}</div>`;
  S.din = (id, val, ph, o) => { o = o || {}; return `<input class="input${o.mono ? ' mono' : ''}${o.type === 'password' ? ' masked' : ''}" id="${id}" data-f="${id}" type="${o.type || 'text'}" value="${esc(val || '')}" placeholder="${esc(ph || '')}" autocomplete="off" spellcheck="false"${o.mono ? ' style="font-family:var(--font-mono);font-size:12px"' : ''}>`; };
  S.dchk = (k, on, label) => `<span class="check${on ? ' on' : ''}" role="checkbox" aria-checked="${!!on}" tabindex="0" data-c="${k}"><span class="box">${on ? ic('check', 10) : ''}</span><span>${label}</span></span>`;
  S.dbtn = (label, o) => { o = o || {}; return `<button type="button" class="btn${o.kind ? ' btn-' + o.kind : ''}" data-do="${o.act || 'close'}"${o.dis ? ' disabled' : ''}${o.def ? ' data-default' : ''}${o.af ? ' data-autofocus' : ''}>${o.icon ? ic(o.icon, 14) : ''}${label}</button>`; };
  S.dbanner = (kind, t, d) => `<div class="banner ${kind}" role="${kind === 'error' ? 'alert' : 'status'}">${ic(kind === 'error' ? 'err' : kind === 'info' ? 'info' : kind === 'ok' ? 'check' : 'warn', 16)}<div style="min-width:0"><div class="b-title">${esc(t)}</div>${d ? `<div>${esc(d)}</div>` : ''}</div></div>`;

  /* mount a section. render(ctx) returns the pane HTML; bind(kind, key, value, ctx) runs after a generic control edit.
     opts.stagebar: extra presentation-only controls for the top bar (scenario switchers). */
  S.mount = function (cur, title, render, bind, opts) {
    opts = opts || {};
    document.title = 'GitWarp — Settings → ' + title;
    G.stagebar(opts.stagebar || '');
    const stage = G.h('<div class="stage"><div class="winwrap"></div></div>'); document.body.appendChild(stage);
    /* backdrop: the real main window, so the card reads as a modal over the repository */
    const sh = G.shell({ host: stage.firstChild, w: 1200, h: 800, view: 'history' });
    const win = sh.win;
    try { const hh = G.h('<div class="hist-host" style="display:flex;flex:1;min-width:0;min-height:0"></div>'); sh.center.appendChild(hh); G.history(hh, G.commits, {}); } catch (e) {}
    const layer = G.layer(win);
    if (!layer.querySelector('.toasts')) layer.appendChild(G.h('<div class="toasts"></div>')); /* created first so dialog scrims stay the last scrim */
    const scrim = G.h(`<div class="scrim center gw-set-scrim"><div class="setcard" role="dialog" aria-modal="true" aria-label="Settings">
      <header class="titlebar"><span class="ttl">${ic('gear', 14)}Settings</span><span class="od-fill"></span><button class="btn btn-icon btn-ghost" data-act="close-settings" data-tip="Close settings (Esc)" aria-label="Close settings">${ic('x', 14)}</button></header>
      <div class="set">${S.nav(cur)}<div class="spane"></div></div>
      <footer class="statusbar"><span>Changes are saved automatically</span><span class="od-fill"></span><span class="gw-sb-git" data-tip="${esc(S.GIT.tip)}">${S.gitHTML()}</span></footer></div></div>`);
    layer.appendChild(scrim);
    const close = () => { location.href = 'main-1200.html'; };
    scrim.querySelector('[data-act=close-settings]').onclick = close;
    scrim.addEventListener('mousedown', (e) => { if (e.target === scrim) close(); }); /* a click on the scrim closes Settings */
    document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && layer.querySelectorAll(':scope > .scrim').length === 1) close(); }); /* a dialog / recording handler stops Esc first */
    const pane = scrim.querySelector('.spane');
    const ctx = { win, pane, toast: (m, k) => G.toast(win, m, k || 'info') };
    const draw = () => { const y = pane.scrollTop, f = document.activeElement && document.activeElement.dataset ? Object.assign({}, document.activeElement.dataset) : null; pane.innerHTML = `<div class="spwrap">${render(ctx)}</div>`; pane.scrollTop = y; if (f) { const sel = Object.keys(f).map((k) => `[data-${k.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase())}="${f[k]}"]`).join(''); const el = sel && pane.querySelector(sel); if (el && el.focus) el.focus(); } };
    ctx.draw = draw;
    /* generic controls: data-sw / data-seg / data-sel / data-chk write into the store and rerender */
    pane.addEventListener('click', (e) => {
      const sw = e.target.closest('[data-sw]'), sg = e.target.closest('[data-seg] button'), ck = e.target.closest('[data-chk]');
      if (sw && sw.getAttribute('aria-disabled') !== 'true') { const k = sw.dataset.sw; S.set(k, !S.get(k, false)); bind && bind('sw', k, S.get(k), ctx); draw(); }
      else if (sg) { const k = sg.parentNode.dataset.seg; S.set(k, sg.dataset.v); bind && bind('seg', k, sg.dataset.v, ctx); draw(); }
      else if (ck) { const k = ck.dataset.chk; S.set(k, !S.get(k, false)); bind && bind('chk', k, S.get(k), ctx); draw(); }
    });
    pane.addEventListener('keydown', (e) => { if ((e.key === ' ' || e.key === 'Enter') && e.target.matches('[data-sw],[data-chk]')) { e.preventDefault(); e.target.click(); } });
    pane.addEventListener('change', (e) => { const s = e.target.closest('[data-sel]'); if (s) { S.set(s.dataset.sel, s.value); bind && bind('sel', s.dataset.sel, s.value, ctx); draw(); } });
    G.initTips(win); draw(); G.fit(win);
    return ctx;
  };
})();
