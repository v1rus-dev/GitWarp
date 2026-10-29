/* gitwarp dialogs: clone / open, create branch / tag here, worktrees, SSH keys.
   Design-prototype code: every state machine here is a spec for the GPUI implementation.
   One controller (D.open) drives every dialog: state object -> view() -> HTML, with
   validation shown on blur (st.touched) and the primary button gated on live validity. */
(function () {
  'use strict';
  const G = window.GL, ic = G.ic, esc = G.esc;
  const D = (window.GLD = {});

  /* ---------- sample data shared by dialogs and the SSH settings page ---------- */
  const PUB1 = 'AAAAC3NzaC1lZDI1NTE5AAAAIEq7ZcN4mV0pT2yHk8Lr3dXwB9aJ1eFg5uOs6iCz1xA0';
  const PUB2 = 'AAAAC3NzaC1lZDI1NTE5AAAAIHn2uP8kWq5Tz0RbYc7LdJ3xVm9aEo1gFs4iXy6NtK2A';
  D.DEFAULT_KEYS = [
    { id: 'k1', name: 'id_ed25519', type: 'ed25519', fp: 'SHA256:q7ZcN4mV0pT2yHk8Lr3dXwB9aJ1eFg5uOs6iCz1xA0E', pub: 'ssh-ed25519 ' + PUB1 + ' mira@kovalenko.dev', created: '2026-03-14', agent: true, pass: true, hosts: ['github.com', 'gitlab.com'] },
    { id: 'k2', name: 'id_ed25519_work', type: 'ed25519', fp: 'SHA256:Hn2uP8kWq5Tz0RbYc7LdJ3xVm9aEo1gFs4iXy6NtK2A', pub: 'ssh-ed25519 ' + PUB2 + ' mira@corp.example', created: '2026-08-02', agent: false, pass: true, hosts: ['git.corp.example'] },
    { id: 'k3', name: 'id_rsa', type: 'rsa-2048', fp: 'SHA256:0bVeRk3Yq8LdTn5ZxJm2WcHa7PfGuEo1iS4yNt9XvB6', pub: 'ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQC0bVeRk3Yq8LdTn5ZxJm2 mira@old-laptop', created: '2021-11-20', agent: false, pass: false, hosts: [], legacy: true }
  ];
  D.HOSTS = [
    { host: 'github.com', alg: 'ED25519', fp: 'SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU', added: '2026-03-14' },
    { host: 'gitlab.com', alg: 'ED25519', fp: 'SHA256:eUXGGm1YGsMAS7vkcx6JOJdOGHPem5gQp4taiCfCLB8', added: '2026-03-14' },
    { host: 'git.corp.example', alg: 'ED25519', fp: 'SHA256:Zk4mT9vQ1pLx7bYdW3nR0aCeHs8UfJ2oGi6tNyVq5Xc', added: '2026-08-02' }
  ];
  D.keys = () => { try { const v = localStorage.getItem('gl-set-ssh-keys'); if (v) return JSON.parse(v); } catch (e) {} return D.DEFAULT_KEYS.map((k) => Object.assign({}, k)); };
  D.saveKeys = (l) => { try { localStorage.setItem('gl-set-ssh-keys', JSON.stringify(l)); } catch (e) {} };
  D.fp = (f) => f.slice(0, 14) + '…' + f.slice(-4);
  const BRANCHES = ['main', 'feat/idle-lanes', 'feat/palette', 'fix/git-detect'];
  const TAGS = ['v0.3.0', 'v0.2.0'];
  const DIRTY = 6; /* matches the Changes counter in the toolbar */

  /* ---------- small builders ---------- */
  D.f = (label, id, control, o) => { o = o || {}; return `<div class="fld2"><label class="field-label" for="${id}">${label}${o.req ? ' <span class="req" aria-hidden="true">*</span>' : ''}</label>${control}${o.err ? `<div class="field-error" id="${id}-e">${ic('err', 12)}<span>${esc(o.err)}</span></div>` : o.help ? `<div class="field-help" id="${id}-h">${o.help}</div>` : ''}</div>`; };
  D.in = (id, val, ph, o) => { o = o || {}; return `<input class="input${o.err ? ' error' : ''}${o.mono ? ' mono' : ''}" id="${id}" data-f="${id}" type="${o.type || 'text'}" value="${esc(val || '')}" placeholder="${esc(ph || '')}" autocomplete="off" spellcheck="false" aria-invalid="${!!o.err}"${o.err ? ` aria-describedby="${id}-e"` : ''}${o.dis ? ' disabled' : ''}>`; };
  D.sel = (id, val, opts, dis) => `<select class="input select" id="${id}" data-f="${id}"${dis ? ' disabled' : ''}>${opts.map((o) => `<option value="${esc(o[0])}"${o[0] === val ? ' selected' : ''}>${esc(o[1])}</option>`).join('')}</select>`;
  D.chk = (k, on, label, dis) => `<span class="check${on ? ' on' : ''}" role="checkbox" aria-checked="${!!on}" ${dis ? 'aria-disabled="true"' : 'tabindex="0"'} data-c="${k}"><span class="box">${on ? ic('check', 10) : ''}</span><span>${label}</span></span>`;
  D.seg = (k, cur, opts, label) => `<div class="seg dtabs" role="group" aria-label="${esc(label || k)}" data-seg="${k}">${opts.map((o) => `<button type="button" aria-pressed="${cur === o[0]}" data-v="${o[0]}">${o[1]}</button>`).join('')}</div>`;
  D.btn = (label, o) => { o = o || {}; return `<button type="button" class="btn${o.kind ? ' btn-' + o.kind : ''}" data-do="${o.act || 'close'}"${o.v ? ` data-v="${esc(o.v)}"` : ''}${o.dis ? ' disabled' : ''}${o.def ? ' data-default' : ''}>${o.icon ? ic(o.icon, 14) : ''}${label}</button>`; };
  D.banner = (kind, t, d, extra) => `<div class="banner ${kind}" role="${kind === 'error' ? 'alert' : 'status'}">${ic(kind === 'error' ? 'err' : kind === 'info' ? 'info' : kind === 'ok' ? 'check' : 'warn', 16)}<div style="min-width:0"><div class="b-title">${t}</div>${d ? `<div>${d}</div>` : ''}${extra || ''}</div></div>`;
  D.foot = (primary, hint, cancel) => `${hint ? `<span class="foot-hint">${hint}</span>` : ''}${D.btn(cancel || 'Cancel')}${primary}`;
  D.commitCard = (c) => `<div class="cc"><span class="hash">${c.hash}</span><span class="sum od-truncate">${esc(c.summary)}</span><span class="meta">${esc(c.author)} · ${G.rel(c.min)}${(c.refs || []).slice(0, 3).map((x) => `<span class="chip">${ic(x.t === 'tag' ? 'tag' : 'branch', 10)}${esc(x.n)}</span>`).join('')}</span></div>`;
  D.refErr = (n, taken) => {
    if (!n) return '';
    if (/\s/.test(n)) return 'Name cannot contain spaces. Use - or / instead.';
    if (/\.\.|[~^:?*\[\\]|@\{/.test(n)) return 'Name contains characters Git does not allow: .. ~ ^ : ? * [ \\ @{';
    if (/^[-/]|[/.]$|\.lock$/.test(n)) return 'Name cannot start with - or /, or end with /, . or .lock';
    if (taken.includes(n)) return `“${n}” already exists. Pick another name.`;
    return '';
  };

  /* ---------- controller ---------- */
  D.closeAll = (win) => win.querySelectorAll('.layer > .scrim').forEach((s) => s._close && s._close());
  D.open = function (win, spec) {
    D.closeAll(win);
    const layer = G.layer(win), prev = document.activeElement, st = spec.st;
    st.touched = st.touched || {};
    const scrim = G.h('<div class="scrim center"><div class="dialog dlg" role="dialog" aria-modal="true" aria-labelledby="dlg-t"></div></div>');
    const dlg = scrim.firstElementChild; dlg.style.width = (spec.width || 520) + 'px';
    layer.appendChild(scrim);
    const timers = [];
    const api = { st, win, el: dlg, close, redraw, toast: (m, k) => G.toast(win, m, k || 'info'), timer: (fn, ms) => { const t = setInterval(fn, ms); timers.push(t); return t; }, stop: (t) => clearInterval(t) };
    function close() { timers.forEach(clearInterval); document.removeEventListener('keydown', onKey, true); scrim.remove(); if (prev && prev.focus && document.body.contains(prev)) prev.focus(); if (spec.onClose) spec.onClose(st); }
    scrim._close = close;
    function redraw(first) {
      const a = document.activeElement;
      let sel = null, ss = 0, se = 0;
      if (a && dlg.contains(a)) {
        sel = a.id ? '#' + a.id : a.dataset.c ? `[data-c="${a.dataset.c}"]` : a.dataset.do ? `[data-do="${a.dataset.do}"]` + (a.dataset.v ? `[data-v="${a.dataset.v}"]` : '') : a.parentNode && a.parentNode.dataset && a.parentNode.dataset.seg ? `[data-seg="${a.parentNode.dataset.seg}"] [data-v="${a.dataset.v}"]` : null;
        try { ss = a.selectionStart; se = a.selectionEnd; } catch (e) {}
      }
      const v = spec.view(st, api);
      dlg.innerHTML = `<div class="dialog-head">${v.icon ? ic(v.icon, 16) : ''}<span id="dlg-t">${v.title}</span><span class="od-fill"></span><button type="button" class="btn btn-icon btn-ghost btn-xs" aria-label="Close" data-do="close">${ic('x', 12)}</button></div><div class="dialog-body">${v.body}</div><div class="dialog-foot">${v.foot}</div>`;
      let el = null;
      if (sel) { try { el = dlg.querySelector(sel); } catch (e) {} }
      if (!el && first) el = dlg.querySelector('[data-autofocus]') || dlg.querySelector('input:not([disabled]),select:not([disabled]),textarea') || dlg.querySelector('[data-default]:not([disabled])');
      if (el) { el.focus(); try { if (ss != null && el.setSelectionRange && sel && sel[0] === '#') el.setSelectionRange(ss, se); } catch (e) {} }
    }
    dlg.addEventListener('input', (e) => { const f = e.target.closest('[data-f]'); if (!f || f.tagName === 'SELECT') return; st[f.dataset.f] = f.value; if (spec.onInput) spec.onInput(st, f.dataset.f); redraw(); });
    dlg.addEventListener('change', (e) => { const f = e.target.closest('[data-f]'); if (!f) return; st[f.dataset.f] = f.value; st.touched[f.dataset.f] = true; redraw(); });
    dlg.addEventListener('focusout', (e) => { const f = e.target.closest && e.target.closest('input[data-f]'); if (f && !st.touched[f.dataset.f] && f.value) { st.touched[f.dataset.f] = true; if (!dlg.contains(e.relatedTarget) || e.relatedTarget.tagName !== 'BUTTON') setTimeout(() => dlg.isConnected && redraw(), 0); } });
    dlg.addEventListener('click', (e) => {
      const c = e.target.closest('[data-c]'), sg = e.target.closest('[data-seg] button'), d = e.target.closest('[data-do]');
      if (c) { if (c.getAttribute('aria-disabled') !== 'true') { st[c.dataset.c] = !st[c.dataset.c]; redraw(); } return; }
      if (sg) { st[sg.parentNode.dataset.seg] = sg.dataset.v; redraw(); return; }
      if (d && !d.disabled) { if (d.dataset.do === 'close') return close(); const r = spec.actions && spec.actions[d.dataset.do] ? spec.actions[d.dataset.do](st, api, d) : undefined; if (r !== false && dlg.isConnected) redraw(); }
    });
    function onKey(e) {
      if (scrim !== win.querySelector('.layer > .scrim:last-child')) return;
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); if (!(spec.busy && spec.busy(st))) close(); return; }
      if (e.key === 'Enter' && !e.target.closest('textarea,button,select,[role=checkbox],a')) { const b = dlg.querySelector('[data-default]:not([disabled])'); if (b) { e.preventDefault(); b.click(); } }
      else if ((e.key === ' ' || e.key === 'Enter') && e.target.matches && e.target.matches('[role=checkbox]')) { e.preventDefault(); e.target.click(); }
      else if (e.key === 'Tab') {
        const fo = Array.from(dlg.querySelectorAll('button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea,a[href],[tabindex="0"]'));
        if (!fo.length) return;
        const i = fo.indexOf(document.activeElement);
        if (e.shiftKey && i <= 0) { e.preventDefault(); fo[fo.length - 1].focus(); } else if (!e.shiftKey && (i === fo.length - 1 || i < 0)) { e.preventDefault(); fo[0].focus(); }
      }
    }
    document.addEventListener('keydown', onKey, true);
    scrim.addEventListener('mousedown', (e) => { if (e.target === scrim && !(spec.busy && spec.busy(st))) close(); });
    redraw(true);
    if (spec.onOpen) spec.onOpen(st, api);
    return api;
  };

  /* ====================================================================
     Clone / open repository
     ==================================================================== */
  const RECENT = [['gitwarp', '~/develop/gitwarp'], ['open-design', '~/develop/open-design'], ['tree-sitter-rust', '~/src/tree-sitter-rust'], ['dotfiles', '~/dotfiles']];
  const EXISTING = ['gitwarp', 'open-design'];
  function parseUrl(u) {
    u = u.trim(); if (!u) return { kind: '' };
    let m;
    if ((m = u.match(/^https?:\/\/([^/\s]+)\/(.+?)(?:\.git)?\/?$/))) return { kind: 'https', host: m[1], name: m[2].split('/').pop() };
    if ((m = u.match(/^(?:ssh:\/\/)?[\w.-]+@([\w.-]+)[:/](.+?)(?:\.git)?\/?$/))) return { kind: 'ssh', host: m[1], name: m[2].split('/').pop() };
    return { kind: 'bad' };
  }
  function probe(p) {
    p = p.trim(); if (!p) return { k: '' };
    if (/nope|missing/.test(p)) return { k: 'missing' };
    const n = p.replace(/\/+$/, '').split('/').pop();
    if (/gitwarp\/(src|crates)/.test(p)) return { k: 'nested', root: p.replace(/\/(src|crates).*/, ''), name: 'gitwarp' };
    if (/notes|scratch|new/.test(p)) return { k: 'plain', name: n };
    return { k: 'repo', name: n };
  }
  function repoView(st) {
    const tabs = D.seg('tab', st.tab, [['clone', ic('clone', 13) + 'Clone from URL'], ['open', ic('folder', 13) + 'Open from folder']], 'Source');
    if (st.tab === 'open') return openView(st, tabs);
    const p = parseUrl(st.url), name = st.nameEdited ? st.name : (p.name || ''), t = st.touched;
    if (st.phase === 'cloning') {
      const stage = st.pct < 8 ? 0 : st.pct < 70 ? 1 : st.pct < 92 ? 2 : 3, L = ['Connecting to ' + esc(p.host || 'host'), 'Receiving objects', 'Resolving deltas', 'Checking out files'];
      return { title: 'Cloning ' + esc(name), icon: 'clone', body: `<div class="od-stack"><div class="cc"><span class="hash">${ic('folder', 14)}</span><span class="sum od-truncate mono">${esc(st.dest)}/${esc(name)}</span><span class="meta od-truncate">${esc(st.url)}</span></div>
        <div class="dprog" role="progressbar" aria-label="Clone progress" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${Math.round(st.pct)}"><i style="width:${st.pct}%"></i></div>
        <ul class="steps" aria-live="polite">${L.map((l, i) => `<li class="${i < stage ? 'done' : i === stage ? 'now' : ''}">${i < stage ? ic('check', 12) : i === stage ? ic('fetch', 12, 'spin') : '<span style="width:12px"></span>'}<span>${l}${i === stage && stage === 1 ? ` — ${Math.round(st.pct)}%, ${(st.pct * 0.42).toFixed(1)} MiB` : ''}</span></li>`).join('')}</ul></div>`,
        foot: D.btn('Cancel clone', { act: 'cancelClone' }) };
    }
    if (st.phase === 'error') {
      const auth = st.err === 'auth';
      return { title: 'Clone failed', icon: 'warn', body: `${D.banner('error', auth ? 'Permission denied (publickey)' : 'Could not resolve host “' + esc(p.host || 'unreachable.example') + '”', auth ? `${esc(p.host || 'github.com')} did not accept the key offered by ssh-agent. The key may not be added to your account, or it is not loaded into the agent.` : 'Check the address and your network connection, then try again.')}
        ${auth ? `<div class="od-stack" style="--od-gap:6px"><b>Things to try</b><ul class="steps" style="list-style:disc;padding-left:18px;display:block"><li style="display:list-item">Add your public key at ${esc(p.host || 'github.com')} → Settings → SSH keys</li><li style="display:list-item">Pick a specific key instead of “Automatic”</li><li style="display:list-item">Clone over HTTPS with your saved account</li></ul></div>` : ''}
        <div class="field-help mono">git clone ${esc(st.url)} — exit code 128</div>`,
        foot: `<a class="btn" href="settings-ssh.html">${ic('key', 14)}SSH keys…</a>${D.btn('Back', { act: 'back' })}${D.btn('Retry', { kind: 'primary', act: 'clone', def: 1 })}` };
    }
    if (st.phase === 'done') {
      return { title: 'Clone complete', icon: 'check', body: `${D.banner('ok', 'Cloned ' + esc(name), '21 commits · 4 branches · 2 tags · 18.4 MiB')}<dl class="kv"><dt>Location</dt><dd>${esc(st.dest)}/${esc(name)}</dd><dt>Branch</dt><dd>${esc(st.branch || 'main')}</dd><dt>Remote</dt><dd>origin → ${esc(st.url)}</dd></dl>`,
        foot: D.btn('Close') + D.btn('Open repository', { kind: 'primary', act: 'openRepo', def: 1, icon: 'folder' }) };
    }
    /* form */
    const urlErr = t.url && p.kind === 'bad' ? 'Not a Git address. Use https://host/user/repo.git, git@host:user/repo.git or ssh://…' : '';
    const conflict = name && EXISTING.includes(name) && p.kind !== 'bad' ? `A folder named “${name}” already exists in ${st.dest} and is not empty.` : '';
    const depthBad = st.shallow && !/^[1-9]\d*$/.test(String(st.depth));
    const keys = st.noKeys ? [] : D.keys();
    const ok = (p.kind === 'https' || p.kind === 'ssh') && name && !conflict && !depthBad && st.dest.trim();
    const hint = !p.kind ? 'Paste a repository address to continue' : p.kind === 'bad' ? 'Fix the address to continue' : conflict ? 'Choose another folder name' : '';
    let proto = '';
    if (p.kind === 'https') proto = `<span class="chip ok">${ic('check', 10)}HTTPS</span> <span class="muted">Uses your saved ${p.host === 'github.com' ? 'GitHub' : esc(p.host)} account</span>`;
    if (p.kind === 'ssh') proto = `<span class="chip acc">${ic('key', 10)}SSH</span> <span class="muted">Authenticates with an SSH key</span>`;
    const keyBlock = p.kind === 'ssh' ? (keys.length
      ? D.f('SSH key', 'key', D.sel('key', st.key, [['auto', 'Automatic — ssh-agent, then default keys']].concat(keys.map((k) => [k.id, `${k.name} · ${k.type}${k.agent ? ' · in agent' : ''}`]))), { help: `Keys are managed in <a class="link" href="settings-ssh.html">Settings → SSH keys</a>.` })
      : D.banner('warn', 'No SSH keys found on this machine', 'Cloning over SSH needs a key registered with the host.', `<div style="margin-top:6px"><a class="btn btn-sm" href="settings-ssh.html">${ic('key', 12)}Generate or import a key…</a></div>`)) : '';
    return { title: 'Clone repository', icon: 'clone', body: `${tabs}
      ${D.f('Repository address', 'url', D.in('url', st.url, 'https://github.com/user/repo.git  or  git@github.com:user/repo.git', { err: urlErr, mono: 1 }), { err: urlErr, req: 1, help: proto })}
      ${keyBlock}
      <div class="pair">${D.f('Parent folder', 'dest', `<div class="od-row">${D.in('dest', st.dest, '~/develop', { mono: 1 })}${D.btn('Browse…', { act: 'browse', icon: 'folder' })}</div>`, { req: 1 })}
      ${D.f('Folder name', 'name', D.in('name', name, 'repo', { err: conflict, mono: 1 }), { err: conflict, help: name ? `Clones into <span class="mono">${esc(st.dest)}/${esc(name)}</span>` : '' })}</div>
      ${D.f('Branch', 'branch', D.in('branch', st.branch, 'Default branch of the remote'), { help: 'Leave empty to check out the remote’s default branch.' })}
      <div class="od-stack" style="--od-gap:8px">${D.chk('recurse', st.recurse, 'Clone submodules recursively')}${D.chk('shallow', st.shallow, 'Shallow clone — only recent history')}
        ${st.shallow ? `<div style="margin-left:22px;max-width:220px">${D.f('Depth (commits)', 'depth', D.in('depth', st.depth, '1', { err: depthBad && 'x', mono: 1 }), { err: depthBad ? 'Enter a whole number, 1 or more' : '', help: 'Full history can be fetched later.' })}</div>` : ''}
        ${D.chk('open', st.open, 'Open the repository after cloning')}</div>`,
      foot: D.foot(D.btn('Clone', { kind: 'primary', act: 'clone', dis: !ok, def: 1 }), hint) };
  }
  function openView(st, tabs) {
    const pr = probe(st.path), t = st.touched;
    const status = {
      '': '',
      repo: D.banner('ok', 'Git repository found', `<span class="mono">${esc(pr.name || '')}</span> · branch <span class="mono">main</span> · 21 commits`),
      nested: D.banner('warn', 'This folder is inside a repository', `The repository root is <span class="mono">${esc(pr.root || '')}</span>. gitwarp will open the root, not this subfolder.`),
      plain: D.banner('info', 'Not a Git repository yet', 'You can initialize one here and open it.', `<div class="od-stack" style="--od-gap:8px;margin-top:8px">${D.chk('initCommit', st.initCommit, 'Create an initial commit with a .gitignore')}<div style="max-width:220px">${D.f('Default branch', 'ibranch', D.in('ibranch', st.ibranch || 'main', 'main', { mono: 1 }))}</div></div>`),
      missing: D.banner('error', 'Folder does not exist', 'Check the path, or choose a folder with Browse.')
    }[pr.k];
    const label = pr.k === 'plain' ? 'Initialize and open' : 'Open';
    const ok = pr.k === 'repo' || pr.k === 'nested' || (pr.k === 'plain' && (st.ibranch || 'main').trim());
    return { title: 'Open from folder', icon: 'folder', body: `${tabs}
      ${D.f('Folder', 'path', `<div class="od-row">${D.in('path', st.path, '~/develop/my-project', { err: pr.k === 'missing' && t.path, mono: 1 })}${D.btn('Browse…', { act: 'browse', icon: 'folder' })}</div>`, { req: 1 })}
      ${status}
      <div class="od-stack" style="--od-gap:6px"><span class="field-label" style="margin:0" id="rl">Recent folders</span><div class="picklist" role="group" aria-labelledby="rl">${RECENT.map((r) => `<button type="button" class="pick" data-do="pick" data-v="${esc(r[1])}">${ic('folder', 14)}<span class="od-truncate">${esc(r[0])}</span><span class="p mono">${esc(r[1])}</span></button>`).join('')}</div></div>`,
      foot: D.foot(D.btn(label, { kind: 'primary', act: 'openFolder', dis: !ok, def: 1 }), !st.path.trim() ? 'Choose a folder to continue' : pr.k === 'missing' ? 'Choose an existing folder' : '') };
  }
  D.repo = function (win, init) {
    const st = Object.assign({ tab: 'clone', phase: 'form', url: '', dest: '~/develop', name: '', nameEdited: false, branch: '', key: 'auto', recurse: false, shallow: false, depth: '1', open: true, path: '', initCommit: true, ibranch: 'main', pct: 0, err: '', noKeys: false }, init || {});
    if (st.name && !init.nameEdited) st.nameEdited = !!init.name;
    const startClone = (api) => {
      const p = parseUrl(st.url);
      api.stop(st._t);
      st._t = api.timer(() => {
        st.pct = Math.min(100, st.pct + 8);
        if (/private/.test(st.url) && st.pct >= 24) { api.stop(st._t); st.phase = 'error'; st.err = 'auth'; }
        else if (/unreachable/.test(st.url) && st.pct >= 8) { api.stop(st._t); st.phase = 'error'; st.err = 'net'; }
        else if (st.pct >= 100) { api.stop(st._t); st.phase = 'done'; }
        api.redraw();
      }, 420);
    };
    return D.open(win, {
      st, width: 540, view: repoView, busy: (s) => s.phase === 'cloning',
      onInput: (s, k) => { if (k === 'name') s.nameEdited = true; },
      onOpen: (s, api) => { if (s.phase === 'cloning') startClone(api); },
      actions: {
        clone: (s, api) => { s.phase = 'cloning'; s.pct = 0; startClone(api); },
        cancelClone: (s, api) => { api.stop(s._t); s.phase = 'form'; api.toast('Clone canceled — partial folder removed', 'info'); },
        back: (s) => { s.phase = 'form'; s.err = ''; },
        browse: (s, api) => { api.toast('Native-style folder picker opens here (custom-drawn dialog in GPUI)', 'info'); return false; },
        pick: (s, api, el) => { s.path = el.dataset.v; s.touched.path = true; },
        openRepo: (s, api) => { api.close(); api.toast('Opened ' + (s.nameEdited ? s.name : parseUrl(s.url).name), 'ok'); return false; },
        openFolder: (s, api) => { const pr = probe(s.path); api.close(); api.toast(pr.k === 'plain' ? 'Initialized repository in ' + s.path : 'Opened ' + (pr.name || s.path), 'ok'); return false; }
      }
    });
  };

  /* ====================================================================
     Create branch here / Create tag here
     ==================================================================== */
  D.branch = function (win, init) {
    const c = (init && init.commit) || G.commits[0];
    const st = Object.assign({ name: '', checkout: true, commit: c }, init || {});
    return D.open(win, {
      st, width: 480,
      view: (s) => {
        const name = s.name.trim(), err = D.refErr(name, BRANCHES), show = s.touched.name && err, isHead = s.commit === G.commits[0];
        const note = s.checkout ? (isHead ? D.banner('info', `${DIRTY} local changes will come with you`, 'The new branch starts at the current commit, so nothing conflicts.') : D.banner('warn', `${DIRTY} local changes may conflict`, `Switching to <span class="mono">${s.commit.hash}</span> rewrites files that differ from your working tree. gitwarp stops and keeps your changes if a file conflicts.`)) : '';
        return { title: 'Create branch here', icon: 'branch', body: `${D.commitCard(s.commit)}
          ${D.f('Branch name', 'name', D.in('name', s.name, 'feat/my-change', { err: show, mono: 1 }), { req: 1, err: show ? err : '', help: `Starts at <span class="mono">${s.commit.hash}</span>. Use <span class="mono">/</span> to group branches, e.g. <span class="mono">feat/…</span>` })}
          ${D.chk('checkout', s.checkout, 'Switch to the new branch after creating it')}${note}`,
          foot: D.foot(D.btn(s.checkout ? 'Create and switch' : 'Create branch', { kind: 'primary', act: 'create', dis: !name || !!err, def: 1 }), !name ? 'Enter a branch name' : err ? 'Fix the name to continue' : '') };
      },
      actions: { create: (s, api) => { api.close(); api.toast(`Created ${s.name.trim()}${s.checkout ? ' and switched to it' : ''}`, 'ok'); return false; } }
    });
  };
  D.tag = function (win, init) {
    const c = (init && init.commit) || G.commits[0];
    const st = Object.assign({ name: '', type: 'annot', msg: '', sign: true, push: false, commit: c }, init || {});
    return D.open(win, {
      st, width: 500,
      view: (s) => {
        const name = s.name.trim(), err = D.refErr(name, TAGS), show = s.touched.name && err, annot = s.type === 'annot';
        const msgErr = annot && !s.msg.trim() && s.touched.msg ? 'An annotated tag needs a message.' : '';
        const bad = !name || !!err || (annot && !s.msg.trim());
        return { title: 'Create tag here', icon: 'tag', body: `${D.commitCard(s.commit)}
          ${D.f('Tag name', 'name', D.in('name', s.name, 'v0.3.1', { err: show, mono: 1 }), { req: 1, err: show ? err : '', help: 'Latest tag: <span class="mono">v0.3.0</span>' })}
          <div class="od-stack" style="--od-gap:6px"><span class="field-label" style="margin:0" id="tt">Type</span>${D.seg('type', s.type, [['annot', 'Annotated'], ['light', 'Lightweight']], 'Tag type')}<div class="field-help">${annot ? 'Stores author, date and a message. Recommended for releases.' : 'Just a name pointing at the commit.'}</div></div>
          ${annot ? D.f('Message', 'msg', `<textarea class="input${msgErr ? ' error' : ''}" id="msg" data-f="msg" rows="3" placeholder="Release notes or a short description" aria-invalid="${!!msgErr}">${esc(s.msg)}</textarea>`, { req: 1, err: msgErr }) : ''}
          <div class="od-stack" style="--od-gap:8px">${annot ? D.chk('sign', s.sign, 'Sign with GPG key <span class="mono">9F3A21C4</span>') : ''}${D.chk('push', s.push, 'Push the tag to <span class="mono">origin</span> after creating')}</div>
          ${s.push ? D.banner('warn', 'Pushed tags are public', 'Others can fetch it right away. Deleting it later needs a separate remote delete.') : ''}`,
          foot: D.foot(D.btn(s.push ? 'Create and push' : 'Create tag', { kind: 'primary', act: 'create', dis: bad, def: 1 }), !name ? 'Enter a tag name' : err ? 'Fix the name to continue' : annot && !s.msg.trim() ? 'Add a message to continue' : '') };
      },
      actions: { create: (s, api) => { api.close(); api.toast(`Created tag ${s.name.trim()}${s.push ? ' and pushed to origin' : ''}`, 'ok'); return false; } }
    });
  };

  /* ====================================================================
     Worktrees
     ==================================================================== */
  const WT_USED = { main: '~/develop/gitwarp', 'fix/git-detect': '~/develop/gitwarp-hotfix' };
  const WT_PATHS = ['~/develop/gitwarp', '~/develop/gitwarp-hotfix', '~/develop/gitwarp-review'];
  D.worktreeAdd = function (win, init, onDone) {
    const st = Object.assign({ path: '~/develop/gitwarp-', mode: 'existing', branch: 'feat/palette', newName: '', base: 'main', open: true }, init || {});
    return D.open(win, {
      st, width: 520,
      view: (s) => {
        const path = s.path.trim().replace(/\/+$/, ''), t = s.touched;
        const perr = !path || path === '~/develop/gitwarp-' ? '' : WT_PATHS.includes(path) ? 'A worktree already exists at this path.' : path.startsWith('~/develop/gitwarp/') ? 'This path is inside the repository. Choose a sibling folder instead.' : '';
        const pshow = t.path && perr;
        const used = s.mode === 'existing' && WT_USED[s.branch];
        const nerr = s.mode === 'new' ? D.refErr(s.newName.trim(), BRANCHES) : '';
        const nshow = t.newName && nerr;
        const bad = !path || path === '~/develop/gitwarp-' || !!perr || !!used || (s.mode === 'new' && (!s.newName.trim() || !!nerr));
        const brOpts = BRANCHES.map((b) => [b, b + (WT_USED[b] ? '  — checked out in another worktree' : '')]);
        return { title: 'Add worktree', icon: 'worktree', body: `<div class="field-help" style="margin:0">A worktree is a second working folder for this repository, so you can work on another branch without stashing.</div>
          ${D.f('Folder', 'path', `<div class="od-row">${D.in('path', s.path, '~/develop/gitwarp-hotfix', { err: pshow, mono: 1 })}${D.btn('Browse…', { act: 'browse', icon: 'folder' })}</div>`, { req: 1, err: pshow ? perr : '', help: 'Pick a folder outside the repository. It is created if it does not exist.' })}
          <div class="od-stack" style="--od-gap:6px"><span class="field-label" style="margin:0">Branch</span>${D.seg('mode', s.mode, [['existing', 'Existing branch'], ['new', 'New branch']], 'Branch source')}</div>
          ${s.mode === 'existing'
            ? D.f('Check out', 'branch', D.sel('branch', s.branch, brOpts), { err: used ? `“${s.branch}” is already checked out in ${used}. A branch can only be open in one worktree.` : '' })
            : `<div class="pair">${D.f('New branch name', 'newName', D.in('newName', s.newName, 'fix/my-change', { err: nshow, mono: 1 }), { req: 1, err: nshow ? nerr : '' })}${D.f('Start from', 'base', D.sel('base', s.base, [['main', 'main'], ['feat/idle-lanes', 'feat/idle-lanes'], ['e4f2a91', 'e4f2a91 — selected commit']]))}</div>`}
          ${D.chk('open', s.open, 'Open the worktree in a new window')}`,
          foot: D.foot(D.btn('Add worktree', { kind: 'primary', act: 'add', dis: bad, def: 1 }), used ? 'Choose a branch that is not in use' : '') };
      },
      actions: {
        browse: (s, api) => { api.toast('Native-style folder picker opens here (custom-drawn dialog in GPUI)', 'info'); return false; },
        add: (s, api) => { const b = s.mode === 'new' ? s.newName.trim() : s.branch; api.close(); api.toast(`Added worktree ${s.path.trim()} on ${b}`, 'ok'); if (onDone) onDone({ path: s.path.trim(), branch: b }); return false; }
      }
    });
  };
  D.worktreeRemove = function (win, init, onDone) {
    const st = Object.assign({ wt: { path: '~/develop/gitwarp-hotfix', branch: 'fix/git-detect', dirty: ['src/settings/git.rs', 'src/settings/detect.rs', 'Cargo.lock'], locked: false, missing: false, main: false }, force: false, delBranch: false }, init || {});
    return D.open(win, {
      st, width: 500,
      view: (s) => {
        const w = s.wt, dirty = w.dirty && w.dirty.length > 0;
        let body = '', primary, hint = '';
        if (w.main) {
          body = D.banner('error', 'This is the main worktree', 'It holds the repository itself and cannot be removed. Close the repository instead if you are done with it.');
          primary = D.btn('Remove worktree', { kind: 'danger', act: 'remove', dis: true }); hint = 'Not available for the main worktree';
        } else {
          body = `<dl class="kv"><dt>Folder</dt><dd>${esc(w.path)}</dd><dt>Branch</dt><dd>${esc(w.branch)}</dd></dl>`;
          if (w.missing) body += D.banner('info', 'The folder no longer exists on disk', 'Git still lists this worktree. Removing it only cleans up the record — no files are touched.');
          if (w.locked) body += D.banner('warn', 'This worktree is locked', `Locked with reason “${esc(w.lockReason || 'review in progress')}”. Unlock it before removing.`, `<div style="margin-top:6px">${D.btn('Unlock', { act: 'unlock', icon: 'lock' })}</div>`);
          if (dirty) body += D.banner('warn', `${w.dirty.length} files have uncommitted changes`, 'Removing the worktree discards them for good.', `<ul class="filelist">${w.dirty.map((f) => `<li>${esc(f)}</li>`).join('')}</ul>`) + D.chk('force', s.force, 'Discard these changes and remove the worktree');
          if (!w.missing && !dirty && !w.locked) body += `<div class="field-help" style="margin:0">The folder is deleted from disk. The branch <span class="mono">${esc(w.branch)}</span> and its commits stay in the repository.</div>`;
          body += D.chk('delBranch', s.delBranch, `Also delete branch <span class="mono">${esc(w.branch)}</span> <span class="muted">(fully merged)</span>`);
          const blocked = w.locked || (dirty && !s.force);
          primary = D.btn(dirty && s.force ? 'Discard and remove' : 'Remove worktree', { kind: 'danger', act: 'remove', dis: !!blocked });
          hint = w.locked ? 'Unlock the worktree first' : dirty && !s.force ? 'Confirm to discard changes' : '';
        }
        return { title: 'Remove worktree', icon: 'trash', body, foot: D.foot(primary, hint) };
      },
      actions: {
        unlock: (s, api) => { s.wt.locked = false; api.toast('Worktree unlocked', 'ok'); },
        remove: (s, api) => { api.close(); api.toast('Removed worktree ' + s.wt.path, 'ok'); if (onDone) onDone(s.wt); return false; }
      }
    });
  };

  /* ====================================================================
     SSH keys: generate, import, delete, host-key prompt
     ==================================================================== */
  D.sshGenerate = function (win, init, onDone) {
    const st = Object.assign({ phase: 'form', name: 'id_ed25519_gitwarp', type: 'ed25519', comment: 'mira@kovalenko.dev', usePass: true, pass: '', pass2: '', addAgent: true }, init || {});
    const strength = (p) => (p.length < 8 ? ['weak', 'Weak', 'warn'] : p.length < 12 ? ['ok', 'Okay', 'info'] : ['strong', 'Strong', 'check']);
    return D.open(win, {
      st, width: 520,
      view: (s) => {
        if (s.phase === 'done') {
          const k = s.result;
          return { title: 'Key created', icon: 'key', body: `${D.banner('ok', 'Add the public key to your Git host', 'Paste it in GitHub → Settings → SSH and GPG keys, or the same page on your host. The private key never leaves this machine.')}
            <div class="od-stack" style="--od-gap:6px"><span class="field-label" style="margin:0">Public key</span><div class="codebox" id="pubkey">${esc(k.pub)}</div></div><dl class="kv"><dt>Fingerprint</dt><dd>${esc(k.fp)}</dd><dt>Private file</dt><dd>~/.ssh/${esc(k.name)}</dd></dl>`,
            foot: D.btn('Done') + D.btn('Copy public key', { kind: 'primary', act: 'copy', icon: 'copy', def: 1 }) };
        }
        const name = s.name.trim(), taken = D.keys().some((k) => k.name === name);
        const nerr = !name ? '' : /[\s/\\]/.test(name) ? 'File name cannot contain spaces or slashes.' : taken ? `~/.ssh/${name} already exists. Choose another name.` : '';
        const nshow = s.touched.name && nerr;
        const mism = s.touched.pass2 && s.pass2 && s.pass !== s.pass2 ? 'Passphrases do not match.' : '';
        const sg = strength(s.pass);
        const passOk = !s.usePass || (s.pass.length >= 5 && s.pass === s.pass2);
        const bad = !name || !!nerr || !passOk;
        return { title: 'Generate SSH key', icon: 'key', body: `
          ${D.f('File name', 'name', `<div class="od-row"><span class="mono muted">~/.ssh/</span>${D.in('name', s.name, 'id_ed25519_gitwarp', { err: nshow, mono: 1 })}</div>`, { req: 1, err: nshow ? nerr : '', help: 'The public key is saved next to it with a .pub extension.' })}
          <div class="od-stack" style="--od-gap:6px"><span class="field-label" style="margin:0">Type</span>${D.seg('type', s.type, [['ed25519', 'Ed25519'], ['rsa', 'RSA 4096'], ['ecdsa', 'ECDSA']], 'Key type')}<div class="field-help">${s.type === 'ed25519' ? 'Recommended: short, fast, supported by GitHub, GitLab and Bitbucket.' : s.type === 'rsa' ? 'Compatible with old servers; slower and larger.' : 'Supported by most hosts; prefer Ed25519 when you can.'}</div></div>
          ${D.f('Comment', 'comment', D.in('comment', s.comment, 'you@example.com'), { help: 'Helps you recognise the key on the host, usually your email.' })}
          ${D.chk('usePass', s.usePass, 'Protect the key with a passphrase <span class="muted">(recommended)</span>')}
          ${s.usePass ? `<div class="od-stack" style="--od-gap:14px">${D.f('Passphrase', 'pass', D.in('pass', s.pass, '', { type: 'password' }), { req: 1, help: s.pass ? `<span class="strength ${sg[0]}">${ic(sg[2] === 'check' ? 'check' : sg[2], 12)}${sg[1]}${s.pass.length < 8 ? ' — use 12+ characters' : ''}</span>` : 'Stored in the system keychain if you allow it.' })}${D.f('Repeat passphrase', 'pass2', D.in('pass2', s.pass2, '', { type: 'password', err: mism }), { req: 1, err: mism })}</div>`
            : D.banner('warn', 'Anyone who can read the file can use this key', 'Without a passphrase, a stolen backup or laptop gives access to every host this key is registered on.')}
          ${D.chk('addAgent', s.addAgent, 'Add the key to ssh-agent now')}`,
          foot: D.foot(D.btn('Generate key', { kind: 'primary', act: 'gen', dis: bad, def: 1 }), !name ? 'Enter a file name' : nerr ? 'Fix the file name' : s.usePass && s.pass.length < 5 ? 'Passphrase needs 5+ characters' : s.usePass && s.pass !== s.pass2 ? 'Passphrases must match' : '') };
      },
      actions: {
        gen: (s, api) => {
          const keys = D.keys(), id = 'k' + (keys.length + 1) + Date.now() % 1000, tp = s.type === 'ed25519' ? 'ed25519' : s.type === 'rsa' ? 'rsa-4096' : 'ecdsa-521';
          const k = { id, name: s.name.trim(), type: tp, fp: 'SHA256:Wm8vJq2nT5xYpL0dKc3RzBe7HaUo1gFs4iNy6XtQ9Vc', pub: (s.type === 'ed25519' ? 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIWm8vJq2nT5xYpL0dKc3RzBe7HaUo1gFs4iNy6XtQ9V' : 'ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQDWm8vJq2nT5xYpL0dKc3Rz') + ' ' + s.comment, created: '2026-10-29', agent: s.addAgent, pass: s.usePass, hosts: [] };
          D.saveKeys(keys.concat([k])); s.result = k; s.phase = 'done'; if (onDone) onDone(k);
        },
        copy: (s, api) => { try { navigator.clipboard.writeText(s.result.pub); } catch (e) {} api.toast('Public key copied to clipboard', 'ok'); return false; }
      }
    });
  };
  D.sshImport = function (win, init, onDone) {
    const st = Object.assign({ path: '', copy: true, pass: '' }, init || {});
    const pr = (p) => { p = p.trim(); if (!p) return { k: '' }; if (/nope|missing/.test(p)) return { k: 'nofile' }; if (/\.pub$|bad|txt/.test(p)) return { k: 'invalid' }; if (/enc|pass/.test(p)) return { k: 'enc', name: p.split('/').pop() }; return { k: 'ok', name: p.split('/').pop() }; };
    return D.open(win, {
      st, width: 520,
      view: (s) => {
        const r = pr(s.path), t = s.touched;
        const stat = { '': '', nofile: D.banner('error', 'File not found', 'Check the path or use Browse.'), invalid: D.banner('error', 'This is not a private key', 'Choose the private file (for example id_ed25519), not the .pub file or another document.'),
          ok: D.banner('ok', 'Private key detected', 'ed25519 · no passphrase', `<div class="field-help" style="margin-top:4px">Consider adding a passphrase with <span class="mono">ssh-keygen -p</span>.</div>`), enc: D.banner('info', 'Key is protected by a passphrase', 'Enter it once so gitwarp can read the fingerprint. It is not stored unless you allow the keychain.') }[r.k];
        const ok = r.k === 'ok' || (r.k === 'enc' && s.pass);
        return { title: 'Import SSH key', icon: 'key', body: `${D.f('Private key file', 'path', `<div class="od-row">${D.in('path', s.path, '~/.ssh/id_ed25519_old', { err: (r.k === 'nofile' || r.k === 'invalid') && t.path, mono: 1 })}${D.btn('Browse…', { act: 'browse', icon: 'folder' })}</div>`, { req: 1 })}${stat}
          ${r.k === 'enc' ? D.f('Passphrase', 'pass', D.in('pass', s.pass, '', { type: 'password' }), { req: 1 }) : ''}
          ${D.chk('copy', s.copy, 'Copy into <span class="mono">~/.ssh</span> <span class="muted">(the original stays where it is)</span>')}`,
          foot: D.foot(D.btn('Import key', { kind: 'primary', act: 'imp', dis: !ok, def: 1 }), !s.path.trim() ? 'Choose a key file to continue' : '') };
      },
      actions: {
        browse: (s, api) => { api.toast('Native-style file picker opens here (custom-drawn dialog in GPUI)', 'info'); return false; },
        imp: (s, api) => { const keys = D.keys(); const nm = s.path.trim().split('/').pop(); keys.push({ id: 'k' + Date.now() % 100000, name: nm, type: 'ed25519', fp: 'SHA256:Pe5Lx0cVb9RtYk2NqJ7dWz3HmUo8gFs1iXa4TyC6nB0', pub: 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPe5Lx0cVb9RtYk2NqJ7dWz3HmUo8gFs1iXa4TyC6nB0 imported', created: '2026-10-29', agent: false, pass: !!s.pass, hosts: [] }); D.saveKeys(keys); api.close(); api.toast('Imported ' + nm, 'ok'); if (onDone) onDone(); return false; }
      }
    });
  };
  D.sshDelete = function (win, init, onDone) {
    const st = Object.assign({ key: D.keys()[2], ack: false }, init || {});
    return D.open(win, {
      st, width: 480,
      view: (s) => {
        const k = s.key;
        return { title: 'Delete SSH key', icon: 'trash', body: `<dl class="kv"><dt>Key</dt><dd>${esc(k.name)}</dd><dt>Fingerprint</dt><dd>${esc(D.fp(k.fp))}</dd></dl>
          ${D.banner('warn', 'The private key file is deleted from ~/.ssh', k.hosts && k.hosts.length ? `Hosts that use it (${k.hosts.map(esc).join(', ')}) will reject you until you add another key. This cannot be undone.` : 'No known host uses this key. This cannot be undone.')}
          ${D.chk('ack', s.ack, 'I have a backup or no longer need this key')}`,
          foot: D.foot(D.btn('Delete key', { kind: 'danger', act: 'del', dis: !s.ack }), !s.ack ? 'Confirm to continue' : '') };
      },
      actions: { del: (s, api) => { D.saveKeys(D.keys().filter((x) => x.id !== s.key.id)); api.close(); api.toast('Deleted ' + s.key.name, 'ok'); if (onDone) onDone(); return false; } }
    });
  };
  D.hostKey = function (win, init, onDone) {
    const st = Object.assign({ changed: false, host: 'git.corp.example', ack: false }, init || {});
    return D.open(win, {
      st, width: 540,
      view: (s) => {
        const old = 'SHA256:Zk4mT9vQ1pLx7bYdW3nR0aCeHs8UfJ2oGi6tNyVq5Xc', neu = s.changed ? 'SHA256:Bq7nV2xKd9LpT4mYc1RzWe0HaUs5oGf8iJt3NyX6Cv1' : old;
        if (s.changed) return { title: 'Host key has changed', icon: 'warn', body: `${D.banner('error', 'Possible man-in-the-middle attack', `The key presented by <span class="mono">${esc(s.host)}</span> is different from the one you trusted on 2026-08-02. Someone may be intercepting the connection, or the server was reinstalled.`)}
            <dl class="kv"><dt>Trusted</dt><dd>${old}</dd><dt>Offered now</dt><dd>${neu}</dd></dl><div class="field-help" style="margin:0">Ask the server’s administrator to confirm the new fingerprint over another channel before you continue.</div>
            ${D.chk('ack', s.ack, 'I verified the new fingerprint with the administrator')}`,
          foot: D.foot(D.btn('Replace trusted key', { kind: 'danger', act: 'trust', dis: !s.ack }), !s.ack ? 'Verify the fingerprint to continue' : '', 'Cancel connection') };
        return { title: 'Trust this host?', icon: 'lock', body: `<div>The authenticity of <b class="mono">${esc(s.host)}</b> (203.0.113.24) cannot be verified yet. This is the first time you connect to it.</div>
          <dl class="kv"><dt>Algorithm</dt><dd>ED25519</dd><dt>Fingerprint</dt><dd>${neu}</dd></dl>
          <div class="field-help" style="margin:0">Compare the fingerprint with the one published by your Git host. If they match, trust the host; gitwarp remembers it in <span class="mono">~/.ssh/known_hosts</span>.</div>`,
          foot: D.foot(D.btn('Trust and continue', { kind: 'primary', act: 'trust', def: 1 }), '', 'Cancel connection') };
      },
      actions: { trust: (s, api) => { api.close(); api.toast(s.changed ? 'Replaced the trusted key for ' + s.host : 'Trusted ' + s.host, 'ok'); if (onDone) onDone(s); return false; } }
    });
  };

  D.remoteAdd = function (win, init) {
    const st = Object.assign({ name: '', url: '', fetch: true }, init || {});
    const REMOTES = ['origin', 'upstream'];
    return D.open(win, {
      st, width: 500,
      view: (s) => {
        const n = s.name.trim(), p = parseUrl(s.url), t = s.touched;
        const nerr = !n ? '' : /\s/.test(n) ? 'Name cannot contain spaces.' : REMOTES.includes(n) ? `Remote “${n}” already exists.` : '';
        const uerr = p.kind === 'bad' ? 'Not a Git address. Use https://host/user/repo.git or git@host:user/repo.git' : '';
        const ok = n && !nerr && (p.kind === 'https' || p.kind === 'ssh');
        const proto = p.kind === 'ssh' ? `<span class="chip acc">${ic('key', 10)}SSH</span> <span class="muted">Uses your SSH keys</span>` : p.kind === 'https' ? `<span class="chip ok">${ic('check', 10)}HTTPS</span> <span class="muted">Uses your saved account</span>` : '';
        return { title: 'Add remote', icon: 'cloud', body: `${D.f('Name', 'name', D.in('name', s.name, 'upstream', { err: t.name && nerr, mono: 1 }), { req: 1, err: t.name ? nerr : '', help: 'Existing remotes: <span class="mono">origin</span>, <span class="mono">upstream</span>' })}
          ${D.f('Address', 'url', D.in('url', s.url, 'git@github.com:user/repo.git', { err: t.url && uerr, mono: 1 }), { req: 1, err: t.url ? uerr : '', help: proto })}
          ${D.chk('fetch', s.fetch, 'Fetch branches and tags right after adding')}`,
          foot: D.foot(D.btn('Add remote', { kind: 'primary', act: 'add', dis: !ok, def: 1 }), !n ? 'Enter a remote name' : nerr ? 'Choose another name' : !p.kind ? 'Enter the remote address' : uerr ? 'Fix the address' : '') };
      },
      actions: { add: (s, api) => { api.close(); api.toast(`Added remote ${s.name.trim()}${s.fetch ? ' and started fetching' : ''}`, 'ok'); return false; } }
    });
  };

  /* ---------- entry used by context menus in the main window ---------- */
  D.openIn = (kind, win) => {
    win = win || document.querySelector('.win'); if (!win) return;
    let c = G.commits[0]; const r = win.querySelector('.lrow.sel'); if (r && r.dataset.i != null && G.commits[+r.dataset.i]) c = G.commits[+r.dataset.i];
    ({ branch: () => D.branch(win, { commit: c }), tag: () => D.tag(win, { commit: c }), worktreeAdd: () => D.worktreeAdd(win), worktreeRemove: () => D.worktreeRemove(win), remoteAdd: () => D.remoteAdd(win), clone: () => D.repo(win, { tab: 'clone' }), open: () => D.repo(win, { tab: 'open' }) })[kind]();
  };

  /* ---------- gallery page: one main window, a scenario switcher, the dialog on top ---------- */
  D.page = function (title, scenes, centerFn) {
    document.title = 'gitwarp — ' + title;
    const sel = `<label class="scn" style="display:inline-flex;gap:8px;align-items:center;font-size:12px">Dialog state <select id="scn" aria-label="Dialog state" style="height:24px;background:var(--bg);color:var(--text);border:1px solid var(--border-strong);border-radius:var(--r-sm)">${scenes.map((s, i) => `<option value="${i}">${esc(s[0])}</option>`).join('')}</select></label><button class="btn btn-sm" id="reopen">Reopen dialog</button>`;
    G.stagebar(sel);
    const stage = G.h('<div class="stage"><div class="winwrap"></div></div>'); document.body.appendChild(stage);
    const sh = G.shell({ host: stage.firstChild, w: 1200, h: 800, view: 'history' });
    const win = sh.win;
    if (centerFn) centerFn(sh, win);
    else { sh.center.innerHTML = '<div class="hist-host" style="display:flex;flex:1;min-width:0;min-height:0;height:100%;width:100%"></div>'; G.history(sh.center.firstChild, G.commits, { gw: 112 }); }
    G.fit(win);
    const s = document.getElementById('scn');
    const run = () => scenes[+s.value][1](win);
    s.addEventListener('change', run); document.getElementById('reopen').addEventListener('click', run);
    document.addEventListener('keydown', (e) => { if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === 'o') { e.preventDefault(); D.repo(win, {}); } });
    run();
    return { win, run };
  };
})();
