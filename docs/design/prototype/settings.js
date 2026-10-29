/* gitwarp settings shell: one window, six sections, shared persistence + row builders. */
(function () {
  'use strict';
  const G = window.GL, ic = G.ic, esc = G.esc;
  const S = (window.GLS = {});
  S.SECTIONS = [
    ['gear', 'General', 'settings-general.html'],
    ['palette', 'Appearance', 'settings-appearance.html'],
    ['git', 'Git', 'settings-git.html'],
    ['key', 'SSH keys', 'settings-ssh.html'],
    ['keyboard', 'Keybindings', 'settings-keybindings.html'],
    ['split', 'Diff &amp; merge', 'settings-diff.html'],
    ['user', 'Identity', 'settings-identity.html']
  ];
  S.nav = (cur) => `<nav class="snav" aria-label="Settings sections">${S.SECTIONS.map(([i, l, h]) => `<a class="node${h === cur ? ' sel' : ''}" href="${h}" style="--lvl:0"${h === cur ? ' aria-current="page"' : ''}>${ic(i, 14)}<span class="nlabel">${l}</span></a>`).join('')}</nav>`;

  /* persistence (design prototype: localStorage stands in for the settings file) */
  S.get = (k, d) => { try { const v = localStorage.getItem('gl-set-' + k); return v == null ? d : JSON.parse(v); } catch (e) { return d; } };
  S.set = (k, v) => { try { localStorage.setItem('gl-set-' + k, JSON.stringify(v)); } catch (e) {} };

  /* row builders */
  S.row = (title, sub, ctl, off) => `<div class="srow${off ? ' off' : ''}"><div class="st">${title}</div>${sub ? `<div class="sd">${sub}</div>` : ''}<div class="ctl">${ctl}</div></div>`;
  S.sw = (k, on, label, dis) => `<span class="switch${on ? ' on' : ''}" role="switch" aria-checked="${!!on}" aria-label="${esc(label)}" ${dis ? 'aria-disabled="true"' : 'tabindex="0"'} data-sw="${k}"><span class="track"></span></span>`;
  S.seg = (k, cur, opts, dis) => `<div class="seg${dis ? ' dis' : ''}" role="group" data-seg="${k}">${opts.map(([v, l]) => `<button aria-pressed="${cur === v}" data-v="${v}"${dis ? ' tabindex="-1"' : ''}>${l}</button>`).join('')}</div>`;
  S.sel = (k, cur, opts, label, dis) => `<select class="input" data-sel="${k}" aria-label="${esc(label)}"${dis ? ' disabled' : ''}>${opts.map(([v, l]) => `<option value="${v}"${v === cur ? ' selected' : ''}>${l}</option>`).join('')}</select>`;
  S.chk = (k, on, label) => `<span class="check${on ? ' on' : ''}" role="checkbox" aria-checked="${!!on}" tabindex="0" data-chk="${k}"><span class="box">${on ? ic('check', 10) : ''}</span>${label}</span>`;
  S.group = (title, rows) => `<section class="sgroup"><h3>${title}</h3><div class="slist">${rows.join('')}</div></section>`;
  S.banner = (kind, t, d) => `<div class="banner ${kind} stack-banner" role="${kind === 'error' ? 'alert' : 'status'}">${ic(kind === 'error' ? 'err' : kind === 'info' ? 'info' : 'warn', 16)}<div><div class="b-title">${esc(t)}</div><div>${esc(d)}</div></div></div>`;

  /* mount a section. render() returns the pane HTML; on(change) rerenders after a control edit. */
  S.mount = function (cur, title, render, bind) {
    document.title = 'gitwarp — Settings → ' + title;
    G.stagebar();
    const stage = G.h('<div class="stage"><div class="winwrap"></div></div>'); document.body.appendChild(stage);
    const win = G.h(`<div class="win" style="width:1100px;height:760px;grid-template-rows:var(--bar) minmax(0,1fr) var(--status)">
      <header class="titlebar"><span style="display:inline-flex;gap:8px;align-items:center;font-weight:600;padding-left:4px">${ic('gear', 14)}Settings</span><span class="od-fill"></span><div class="wc"><button class="btn btn-ghost" aria-label="Close settings" onclick="history.length>1?history.back():location.href='main-1200.html'">${ic('x', 14)}</button></div></header>
      <div class="set">${S.nav(cur)}<div class="spane"></div></div>
      <footer class="statusbar"><span class="muted">Changes are saved automatically</span><span class="od-fill"></span></footer></div>`);
    stage.firstChild.appendChild(win);
    const pane = win.querySelector('.spane');
    const ctx = { win, pane, toast: (m, k) => G.toast(win, m, k || 'info') };
    const draw = () => { const y = pane.scrollTop, f = document.activeElement && document.activeElement.dataset ? Object.assign({}, document.activeElement.dataset) : null; pane.innerHTML = render(ctx); pane.scrollTop = y; if (f) { const sel = Object.keys(f).map((k) => `[data-${k.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase())}="${f[k]}"]`).join(''); const el = sel && pane.querySelector(sel); if (el && el.focus) el.focus(); } };
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
