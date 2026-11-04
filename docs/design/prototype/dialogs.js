/* GitWarp dialogs: clone / open, create branch / tag here, worktrees, SSH keys.
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
  D.commitCard = (c) => `<div class="cc"><span class="hash">${c.hash}</span><span class="sum od-truncate">${esc(c.summary || 'Commit')}</span>${c.author ? `<span class="meta">${esc(c.author)} · ${G.rel(c.min)}${(c.refs || []).map((x) => `<span class="chip">${ic(x.t === 'tag' ? 'tag' : 'branch', 10)}<span class="od-truncate" style="max-width:150px">${esc(x.n)}</span></span>`).join('')}</span>` : ''}</div>`;
  /* Shared ref-dialog states (running / error): footer + error banner. kind 'danger' primaries are never the Enter default. */
  D.rfoot = (s, label, o) => {
    o = o || {}; const run = s.phase === 'running';
    const b = D.btn(label, { kind: o.kind || 'primary', act: o.act || 'ok', dis: run || o.dis, def: o.kind === 'danger' ? 0 : 1 });
    return `${run || o.hint ? `<span class="foot-hint">${run ? 'Working…' : o.hint}</span>` : ''}${D.btn('Cancel', { dis: run })}${b}`;
  };
  D.rerr = (s) => s.phase === 'error' && s.err ? D.banner('error', 'The operation failed', `<span style="white-space:pre-wrap">${esc(s.err)}</span>`) : '';
  D.fail = (s, api, text) => { s.phase = 'running'; api.redraw(); setTimeout(() => { if (!api.el.isConnected) return; s.phase = 'error'; s.err = text; api.redraw(); }, 650); return false; };
  D.refErr = (n, taken) => {
    if (!n) return '';
    if (/\s/.test(n)) return 'Name cannot contain spaces. Use - or / instead.';
    if (/\.\.|[~^:?*\[\\]|@\{/.test(n)) return 'Name contains characters Git does not allow: .. ~ ^ : ? * [ \\ @{';
    if (/^[-/]|[/.]$|\.lock$/.test(n)) return 'Name cannot start with - or /, or end with /, . or .lock';
    if (taken.includes(n)) return `“${n}” already exists. Pick another name.`;
    return '';
  };

  /* Toasts: error toasts stay until dismissed, the others auto-hide after 5 s (G.toast hides every kind, so errors are re-pinned here). */
  D.toast = (win, msg, kind) => {
    G.toast(win, msg, kind);
    if (kind === 'error') { const c = G.layer(win).querySelector('.toasts'), t = c && c.lastElementChild; if (t) { const k = t.cloneNode(true); k.querySelector('button').onclick = () => k.remove(); c.replaceChild(k, t); } }
  };
  /* A static strip of toasts for gallery scenes: [[kind, text], ...]; nothing auto-hides so the drawing stays put. */
  D.toastStrip = (win, list) => {
    const layer = G.layer(win); let c = layer.querySelector('.toasts'); if (c) c.remove();
    c = G.h('<div class="toasts"></div>'); layer.appendChild(c);
    list.forEach(([kind, msg]) => { const t = G.h(`<div class="toast ${kind}" role="status">${ic(kind === 'error' ? 'err' : kind === 'warn' ? 'warn' : kind === 'ok' ? 'check' : 'info', 14)}<span class="toast-msg">${esc(msg)}</span><button class="btn btn-icon btn-ghost btn-xs" aria-label="Dismiss">${ic('x', 12)}</button></div>`); t.querySelector('button').onclick = () => t.remove(); c.appendChild(t); });
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
    const api = { st, win, el: dlg, close, redraw, toast: (m, k) => D.toast(win, m, k || 'info'), timer: (fn, ms) => { const t = setInterval(fn, ms); timers.push(t); return t; }, stop: (t) => clearInterval(t) };
    function close() { timers.forEach(clearInterval); document.removeEventListener('keydown', onKey, true); scrim.remove(); if (prev && prev.focus && document.body.contains(prev)) prev.focus(); if (spec.onClose) spec.onClose(st); }
    scrim._close = close;
    function redraw(first) {
      const a = document.activeElement;
      let sel = null, ss = 0, se = 0;
      if (a && dlg.contains(a)) {
        sel = a.id ? '#' + a.id : a.dataset.c ? `[data-c="${a.dataset.c}"]` : a.dataset.do ? `[data-do="${a.dataset.do}"]` + (a.dataset.v ? `[data-v="${a.dataset.v}"]` : '') : a.parentNode && a.parentNode.dataset && a.parentNode.dataset.seg ? `[data-seg="${a.parentNode.dataset.seg}"] [data-v="${a.dataset.v}"]` : null;
        try { ss = a.selectionStart; se = a.selectionEnd; } catch (e) {}
      }
      const v = spec.view(st, api); if (v.width) dlg.style.width = v.width + 'px';
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
     Clone / open / initialize repository
     The app has no Clone/Open tabs: Clone, Open and Initialize are three independent dialogs.
     D.repo({tab}) keeps its signature for the pages that call it and just picks the dialog.
     ==================================================================== */
  const RECENT = [['gitwarp', '~/develop/gitwarp'], ['open-design', '~/develop/open-design'], ['tree-sitter-rust', '~/src/tree-sitter-rust'], ['dotfiles', '~/dotfiles']];
  const EXISTING = ['gitwarp', 'notes'];
  function parseUrl(u) {
    u = u.trim(); if (!u) return { kind: '' };
    let m;
    if ((m = u.match(/^https?:\/\/([^/\s]+)\/(.+?)(?:\.git)?\/?$/))) return { kind: 'https', host: m[1], name: m[2].split('/').pop() };
    if ((m = u.match(/^(?:ssh|git):\/\/([^/\s@]+)\/(.+?)(?:\.git)?\/?$/))) return { kind: 'ssh', host: m[1], name: m[2].split('/').pop() };
    if ((m = u.match(/^(?:ssh:\/\/)?[\w.-]+@([\w.-]+)[:/](.+?)(?:\.git)?\/?$/))) return { kind: 'ssh', host: m[1], name: m[2].split('/').pop() };
    /* local paths and ~ paths are accepted too (protocol line "Local") */
    if ((m = u.match(/^(?:\/|~\/|\.{1,2}\/)(.*?)(?:\.git)?\/?$/))) return { kind: 'local', host: '', name: (m[1].split('/').pop() || '') };
    return { kind: 'bad' };
  }
  /* Folder probe shared by Open and Initialize. `force` lets a scene pin the git-unavailable / read-failed outcomes. */
  function probe(p, force) {
    p = p.trim(); if (force === 'gitmissing') return { k: 'gitmissing', msg: 'git: command not found' }; if (force === 'readfail') return { k: 'readfail', msg: 'Permission denied (os error 13)' };
    if (!p) return { k: '' };
    if (/nope|missing/.test(p)) return { k: 'missing' };
    const n = p.replace(/\/+$/, '').split('/').pop();
    if (/gitwarp\/(src|crates)/.test(p)) return { k: 'nested', root: p.replace(/\/(src|crates).*/, ''), name: 'gitwarp' };
    if (/notes|scratch|new/.test(p)) return { k: 'plain', name: n };
    return { k: 'repo', name: n };
  }
  const detail = (pr) => pr.k === 'nested'
    ? `<div class="muted">The repository root is <span class="mono od-truncate" style="display:inline-block;max-width:420px;vertical-align:bottom">${esc(pr.root)}</span>. GitWarp will open the root, not this subfolder.</div>`
    : `<div class="muted"><span class="mono">${esc(pr.name || '')}</span> · branch <span class="mono">main</span> · 21 commits</div>`;
  /* Banner + details line for the folder field. mode: 'open' | 'init' (the two dialogs word some states differently). */
  function folderStatus(pr, st, mode) {
    let h = '';
    if (pr.k === 'repo') h = mode === 'init' ? D.banner('warn', 'This folder is already a repository', '') + detail(pr) : D.banner('ok', 'Git repository found', '') + detail(pr);
    else if (pr.k === 'nested') h = D.banner('warn', mode === 'init' ? 'This folder is already a repository' : 'This folder is inside a repository', '') + detail(pr);
    else if (pr.k === 'plain') h = D.banner('info', 'Not a Git repository yet', 'You can initialize one here and open it.');
    else if (pr.k === 'missing') h = mode === 'init' ? D.banner('info', 'Not a Git repository yet', 'The folder will be created by git init.') : D.banner('error', 'Folder does not exist', 'Check the path, or choose a folder with Browse.');
    else if (pr.k === 'gitmissing') h = D.banner('error', 'Git is unavailable', esc(pr.msg));
    else if (pr.k === 'readfail') h = D.banner('error', 'Could not read this folder', esc(pr.msg || 'git failed without a message'));
    return h ? `<div class="od-stack" style="--od-gap:8px">${h}</div>` : '';
  }
  const initable = (pr) => pr.k === 'plain' || (pr.k === 'missing');
  function cloneView(st) {
    const p = parseUrl(st.url), name = st.nameEdited ? st.name : (p.name || ''), t = st.touched, busy = st.phase === 'cloning';
    if (busy) {
      const ind = st.pct == null;
      return { title: 'Cloning repository', icon: 'clone', width: 520, body: `<div class="od-stack" style="--od-gap:12px"><div class="od-row" style="gap:6px;color:var(--muted);font-size:var(--fs-sm)">${ic('folder', 14)}<span class="mono od-truncate">${esc(st.dest)}/${esc(name)}</span></div>
        <div class="muted" style="font-size:var(--fs-sm)">${esc(st.url)}</div>
        <div class="dprog${ind ? ' ind' : ''}" role="progressbar" aria-label="Clone progress" aria-valuemin="0" aria-valuemax="100"${ind ? '' : ` aria-valuenow="${Math.round(st.pct)}"`}><i${ind ? '' : ` style="width:${st.pct}%"`}></i></div>
        <div class="od-row" style="gap:8px;color:var(--muted);font-size:var(--fs-sm)">${ic('fetch', 12, 'spin')}<span class="od-truncate" aria-live="polite">${st.cancelling ? 'Cancelling…' : ind ? 'Connecting…' : `Receiving objects: ${Math.round(st.pct)}% (${Math.round(st.pct * 2.1)}/210)`}</span></div></div>`,
        foot: D.btn('Cancel clone', { act: 'cancelClone', dis: !!st.cancelling }).replace('class="btn"', 'class="btn" style="min-width:96px"') };
    }
    if (st.phase === 'done') {
      return { title: 'Clone complete', icon: 'check', width: 520, body: `${D.banner('ok', 'Cloned ' + esc(name), '21 commits · 4 branches · 2 tags')}<dl class="kv" style="grid-template-columns:84px minmax(0,1fr)"><dt>Location</dt><dd>${esc(st.dest)}/${esc(name)}</dd><dt>Branch</dt><dd>${esc(st.branch || 'HEAD')}</dd><dt>Remote</dt><dd>origin → ${esc(st.url)}</dd></dl>`,
        foot: D.btn('Close') + D.btn('Open', { kind: 'primary', act: 'openRepo', def: 1 }) };
    }
    /* form (a failed clone keeps the form and appends a banner at the bottom of the body) */
    const urlErr = t.url && p.kind === 'bad' ? 'Not a Git address. Use https://host/user/repo.git, git@host:user/repo.git or ssh://…' : '';
    const slash = /[\\/]/.test(name), conflict = !slash && name && EXISTING.includes(name) && p.kind !== 'bad' ? `${st.dest}/${name} already exists.` : '';
    const nameErr = slash ? 'The folder name cannot contain slashes.' : conflict;
    const depthBad = st.shallow && !/^[1-9]\d*$/.test(String(st.depth));
    const ok = ['https', 'ssh', 'local'].includes(p.kind) && name && !nameErr && !depthBad && st.dest.trim();
    const proto = { https: ['link', 'HTTPS'], ssh: ['key', 'SSH'], local: ['folder', 'Local'] }[p.kind];
    const f = st.failed;
    return { title: 'Clone repository', icon: 'clone', width: 560, body: `<div class="od-stack" style="--od-gap:12px">
      ${D.f('Repository URL', 'url', D.in('url', st.url, 'https://host/user/repo.git', { err: urlErr, mono: 1 }) + (urlErr ? `<div class="field-error" id="url-e">${ic('err', 12)}<span>${esc(urlErr)}</span></div>` : ''), { req: 1, help: proto ? `<span class="od-row" style="gap:6px;color:var(--muted)">${ic(proto[0], 12)}${proto[1]}</span>` : '' })}
      <div class="od-row" style="align-items:flex-start;gap:12px"><div style="flex:1;min-width:0">${D.f('Folder', 'dest', `<div class="od-row">${D.in('dest', st.dest, '~/develop', { mono: 1 })}${D.btn('Browse…', { act: 'browse', icon: 'folder' })}</div>`, { req: 1 })}</div><div style="width:200px">${D.f('Folder name', 'name', D.in('name', name, 'repo', { err: nameErr, mono: 1 }), { req: 1, err: nameErr })}</div></div>
      <div class="od-row" style="align-items:flex-start;gap:12px"><div style="flex:1;min-width:0">${D.f('Branch (optional)', 'branch', D.in('branch', st.branch, 'Default branch'))}</div><div style="width:96px">${D.f('Depth', 'depth', D.in('depth', st.depth, '1', { err: depthBad, mono: 1 }), { err: depthBad ? 'Use a number above 0.' : '' })}</div></div>
      <div class="od-stack" style="--od-gap:6px">${D.chk('recurse', st.recurse, 'Clone submodules recursively')}${D.chk('shallow', st.shallow, 'Shallow clone — only recent history')}</div></div>
      ${f ? D.banner(f.cancelled ? 'info' : 'error', f.cancelled ? 'Clone cancelled' : 'Clone failed', esc(f.text).replace(/\n/g, '<br>')) : ''}`,
      foot: D.btn('Cancel', { act: f ? 'clearFail' : 'close' }) + D.btn(f ? 'Retry' : 'Clone', { kind: 'primary', act: 'clone', dis: !ok, def: 1 }) };
  }
  /* friendly error texts, network_ops.rs:364-407 */
  const CLONE_ERR = {
    auth: 'The SSH key was rejected. Check that your key is loaded in ssh-agent and added to the account on the Git host.',
    cred: 'Authentication failed. Check your credentials or configure a git credential helper.',
    net: 'Could not resolve the host. Check the address and your connection.\nfatal: unable to access \'https://unreachable.example/a/b.git/\': Could not resolve host: unreachable.example',
    hostkey: 'The SSH host key was not trusted, so the connection was stopped.'
  };
  function openView(st) {
    const pr = probe(st.path, st.force), t = st.touched, run = st.phase === 'running', plain = pr.k === 'plain';
    const bad = ['', 'missing', 'gitmissing', 'readfail'].includes(pr.k);
    const initUi = plain ? `<div class="od-stack" style="--od-gap:8px">${D.chk('initCommit', st.initCommit, 'Create an initial commit with a .gitignore', run)}<div style="max-width:220px">${D.f('Default branch', 'ibranch', D.in('ibranch', st.ibranch, 'main', { mono: 1, dis: run }))}</div></div>` : '';
    const noBranch = plain && !st.ibranch.trim();
    const hint = !st.path.trim() ? 'Choose a folder to continue' : st.probing ? 'Checking the folder…' : pr.k === 'missing' ? 'Choose an existing folder' : (pr.k === 'gitmissing' || pr.k === 'readfail') ? 'Choose another folder' : noBranch ? 'Enter a default branch name' : '';
    const recent = `<div class="od-stack" style="--od-gap:6px"><span class="field-label" style="margin:0;font-size:var(--fs-sm);color:var(--muted)" id="rl">Recent folders</span><div class="picklist" role="group" aria-labelledby="rl">${RECENT.map((r) => `<button type="button" class="pick" data-do="pick" data-v="${esc(r[1])}"${run ? ' disabled' : ''}>${ic('folder', 14)}<span>${esc(r[0])}</span><span class="p mono od-truncate" style="max-width:220px">${esc(r[1])}</span></button>`).join('')}</div></div>`;
    return { title: 'Open from folder', icon: 'folder', width: 540, body: `${D.f('Folder', 'path', `<div class="od-row">${D.in('path', st.path, '~/develop/my-project', { err: pr.k === 'missing' && t.path, mono: 1, dis: run })}${D.btn('Browse…', { act: 'browse', icon: 'folder', dis: run })}</div>`, { req: 1 })}
      ${folderStatus(pr, st, 'open')}${initUi}
      ${st.initErr ? D.banner('error', 'Could not initialize the repository', esc(st.initErr)) : ''}
      ${recent}`,
      foot: (hint || run ? `<span class="foot-hint">${run ? 'Working…' : hint}</span>` : '') + D.btn('Cancel', { dis: run }) + D.btn(plain ? 'Initialize and open' : 'Open', { kind: 'primary', act: 'openFolder', dis: bad || st.probing || noBranch || run, def: 1 }) };
  }
  D.go = (s, api, msg, o) => {
    o = o || {}; s.phase = 'running'; s.err = ''; api.redraw();
    setTimeout(() => {
      if (!api.el.isConnected) return;
      if (o.then) { o.then(s); s.phase = 'idle'; api.redraw(); return; }
      api.close(); api.toast(msg, o.kind || 'ok');
    }, o.ms || 650);
    return false;
  };
  D.repo = function (win, init) {
    init = init || {};
    const st = Object.assign({ tab: 'clone', phase: 'form', url: '', dest: '~/develop', name: '', nameEdited: false, branch: '', recurse: false, shallow: false, depth: '1', path: '', initCommit: true, ibranch: 'main', pct: null, failed: null, probing: false, force: '', initErr: '' }, init);
    if (st.name && !init.nameEdited) st.nameEdited = !!init.name;
    /* legacy scene shape: {phase:'error', err:'auth'|'net'} -> form with the failure banner */
    if (st.phase === 'error') { st.failed = { text: CLONE_ERR[st.err === 'auth' ? 'auth' : st.err] || CLONE_ERR.net }; st.phase = 'form'; }
    if (st.tab === 'open') return D.open(win, {
      st, width: 540, view: openView, busy: (s) => s.phase === 'running',
      actions: {
        browse: (s, api) => { api.toast('Native-style folder picker opens here (custom-drawn dialog in GPUI)', 'info'); return false; },
        pick: (s, api, el) => { s.path = el.dataset.v; s.touched.path = true; },
        openFolder: (s, api) => { const pr = probe(s.path, s.force); if (pr.k === 'plain') return D.go(s, api, 'Initialized repository in ' + s.path.trim()); api.close(); api.toast('Opened ' + (pr.name || s.path), 'ok'); return false; }
      }
    });
    const startClone = (api) => {
      api.stop(st._t); const t0 = Date.now();
      st._t = api.timer(() => {
        if (st.cancelling) { api.stop(st._t); st.cancelling = false; st.phase = 'form'; st.pct = null; api.toast('Clone cancelled', 'info'); api.redraw(); return; }
        if (Date.now() - t0 > 900) st.pct = Math.min(100, (st.pct || 0) + 9);
        const fail = /private/.test(st.url) && st.pct >= 20 ? 'auth' : /unreachable/.test(st.url) && st.pct == null && Date.now() - t0 > 800 ? 'net' : '';
        if (fail) { api.stop(st._t); st.phase = 'form'; st.pct = null; st.failed = { text: CLONE_ERR[fail] }; }
        else if (st.pct >= 100) { api.stop(st._t); st.phase = 'done'; }
        api.redraw();
      }, 420);
    };
    return D.open(win, {
      st, width: 560, view: cloneView, busy: (s) => s.phase === 'cloning',
      onInput: (s, k) => { if (k === 'name') s.nameEdited = true; },
      actions: {
        browse: (s, api) => { api.toast('Native-style folder picker opens here (custom-drawn dialog in GPUI)', 'info'); return false; },
        clone: (s, api) => { s.phase = 'cloning'; s.pct = null; s.failed = null; startClone(api); },
        cancelClone: (s, api) => { s.cancelling = true; startClone(api); },
        clearFail: (s) => { s.failed = null; },
        openRepo: (s, api) => { api.close(); api.toast('Opened ' + (s.nameEdited ? s.name : parseUrl(s.url).name), 'ok'); return false; }
      }
    });
  };
  /* Initialize repository: opened from the welcome screen (Init new repository…, Ctrl+N). */
  D.initRepo = function (win, init) {
    const st = Object.assign({ path: '', ibranch: 'main', initCommit: true, phase: 'idle', force: '', probing: false, initErr: '' }, init || {});
    return D.open(win, {
      st, width: 520, busy: (s) => s.phase === 'running',
      view: (s) => {
        const pr = probe(s.path, s.force), run = s.phase === 'running', exists = pr.k === 'repo' || pr.k === 'nested', bad = ['', 'gitmissing', 'readfail'].includes(pr.k);
        const noBranch = !exists && !bad && !s.ibranch.trim();
        const hint = run ? 'Working…' : !s.path.trim() ? 'Choose a folder to continue' : s.probing ? 'Checking the folder…' : (pr.k === 'gitmissing' || pr.k === 'readfail') ? 'Choose another folder' : noBranch ? 'Enter a default branch name' : '';
        return { title: 'Initialize repository', body: `${D.f('Folder', 'path', `<div class="od-row">${D.in('path', s.path, '~/develop/new-project', { mono: 1, dis: run })}${D.btn('Browse…', { act: 'browse', icon: 'folder', dis: run })}</div>`, { req: 1 })}
          ${folderStatus(pr, s, 'init')}
          ${D.f('Default branch', 'ibranch', D.in('ibranch', s.ibranch, 'main', { mono: 1, dis: run }), { req: 1 })}
          ${D.chk('initCommit', s.initCommit, 'Create an initial commit with a .gitignore', run)}
          ${s.initErr ? D.banner('error', 'Could not initialize the repository', esc(s.initErr)) : ''}`,
          foot: (hint ? `<span class="foot-hint">${hint}</span>` : '') + D.btn('Cancel', { dis: run }) + D.btn(exists ? 'Open' : 'Initialize', { kind: 'primary', act: 'go', dis: bad || s.probing || noBranch || run, def: 1 }).replace('class="btn btn-primary"', 'class="btn btn-primary" style="min-width:88px"') };
      },
      actions: {
        browse: (s, api) => { api.toast('Native-style folder picker opens here (custom-drawn dialog in GPUI)', 'info'); return false; },
        go: (s, api) => { const pr = probe(s.path, s.force); if (pr.k === 'repo' || pr.k === 'nested') { api.close(); api.toast('Opened ' + (pr.name || s.path), 'ok'); return false; } return D.go(s, api, 'Initialized repository in ' + s.path.trim()); }
      }
    });
  };
  D.probe = probe;

  /* ====================================================================
     Create branch here / Create tag here
     ==================================================================== */
  D.branch = function (win, init) {
    const c = (init && init.commit) || G.commits[0];
    const st = Object.assign({ name: '', checkout: true, commit: c, phase: 'idle', err: '', dirty: DIRTY }, init || {});
    return D.open(win, {
      st, width: 480, busy: (s) => s.phase === 'running',
      view: (s) => {
        const name = s.name.trim(), err = D.refErr(name, BRANCHES), show = s.touched.name && err, isHead = s.commit === G.commits[0];
        /* the banner exists only while the work tree is dirty and the checkbox is on */
        const note = s.checkout && s.dirty > 0 ? (isHead ? D.banner('info', `${s.dirty} local ${s.dirty === 1 ? 'change' : 'changes'} will come with you`, 'The new branch starts at the current commit, so nothing conflicts.') : D.banner('warn', `${s.dirty} local ${s.dirty === 1 ? 'change' : 'changes'} may conflict`, 'Switching to another commit rewrites files that differ from your working tree. GitWarp stops and keeps your changes if a file conflicts.')) : '';
        return { title: 'Create branch here', icon: 'branch', body: `${D.commitCard(s.commit)}
          ${D.f('Branch name', 'name', D.in('name', s.name, 'feat/my-change', { err: show, mono: 1, dis: s.phase === 'running' }), { req: 1, err: show ? err : '', help: `Starts at <span class="mono">${s.commit.hash}</span>. Use <span class="mono">/</span> to group branches, e.g. <span class="mono">feat/…</span>` })}
          ${D.chk('checkout', s.checkout, 'Switch to the new branch after creating it', s.phase === 'running')}${note}${D.rerr(s)}`,
          foot: D.rfoot(s, s.checkout ? 'Create and switch' : 'Create branch', { act: 'create', dis: !name || !!err, hint: !name ? 'Enter a branch name' : err ? 'Fix the name to continue' : '' }) };
      },
      actions: { create: (s, api) => D.go(s, api, `Created ${s.name.trim()}${s.checkout ? ' and switched to it' : ''}`) }
    });
  };
  D.tag = function (win, init) {
    const c = (init && init.commit) || G.commits[0];
    const st = Object.assign({ name: '', type: 'annot', msg: '', sign: true, signKey: true, push: false, commit: c, phase: 'idle', err: '', latest: 'v0.3.0' }, init || {});
    return D.open(win, {
      st, width: 500, busy: (s) => s.phase === 'running',
      view: (s) => {
        const name = s.name.trim(), err = D.refErr(name, TAGS), show = s.touched.name && err, annot = s.type === 'annot', run = s.phase === 'running';
        const msgErr = annot && !s.msg.trim() && s.touched.msg ? 'An annotated tag needs a message.' : '';
        const bad = !name || !!err || (annot && !s.msg.trim());
        /* the GPG checkbox is rendered only when user.signingkey is configured (s.signKey) */
        const boxes = [annot && s.signKey ? D.chk('sign', s.sign, 'Sign with GPG key <span class="mono">9F3A21C4</span>', run) : '', D.chk('push', s.push, 'Push the tag to <span class="mono">origin</span> after creating', run)].join('');
        return { title: 'Create tag here', icon: 'tag', body: `${D.commitCard(s.commit)}
          ${D.f('Tag name', 'name', D.in('name', s.name, 'v0.3.1', { err: show, mono: 1, dis: run }), { req: 1, err: show ? err : '', help: s.latest ? `Latest tag: <span class="mono">${esc(s.latest)}</span>` : '' })}
          <div class="od-stack" style="--od-gap:6px"><span class="field-label" style="margin:0" id="tt">Type</span>${D.seg('type', s.type, [['annot', 'Annotated'], ['light', 'Lightweight']], 'Tag type')}<div class="field-help">${annot ? 'Stores author, date and a message. Recommended for releases.' : 'Just a name pointing at the commit.'}</div></div>
          ${annot ? D.f('Message', 'msg', `<textarea class="input${msgErr ? ' error' : ''}" id="msg" data-f="msg" rows="3" placeholder="Release notes or a short description" aria-invalid="${!!msgErr}"${run ? ' disabled' : ''}>${esc(s.msg)}</textarea>`, { req: 1, err: msgErr }) : ''}
          <div class="od-stack" style="--od-gap:8px">${boxes}</div>
          ${s.push ? D.banner('warn', 'Pushed tags are public', 'Others can fetch it right away. Deleting it later needs a separate remote delete.') : ''}${D.rerr(s)}`,
          foot: D.rfoot(s, s.push ? 'Create and push' : 'Create tag', { act: 'create', dis: bad, hint: !name ? 'Enter a tag name' : err ? 'Fix the name to continue' : annot && !s.msg.trim() ? 'Add a message to continue' : '' }) };
      },
      actions: { create: (s, api) => D.go(s, api, `Created tag ${s.name.trim()}${s.push ? ' and pushed to origin' : ''}`) }
    });
  };

  /* ====================================================================
     Worktrees
     ==================================================================== */
  const WT_USED = { main: '~/develop/gitwarp', 'fix/git-detect': '~/develop/gitwarp-hotfix' };
  const WT_PATHS = ['~/develop/gitwarp', '~/develop/gitwarp-hotfix', '~/develop/gitwarp-review'];
  D.worktreeAdd = function (win, init, onDone) {
    const st = Object.assign({ path: '', mode: 'existing', branch: 'feat/palette', newName: '', base: 'main', open: true, phase: 'idle', err: '', noBranches: false }, init || {});
    return D.open(win, {
      st, width: 520, busy: (s) => s.phase === 'running',
      view: (s) => {
        const path = s.path.trim().replace(/\/+$/, ''), t = s.touched, run = s.phase === 'running';
        const perr = !path ? '' : WT_PATHS.includes(path) ? 'A worktree already exists at this path.' : path.startsWith('~/develop/gitwarp/') ? 'This path is inside the repository. Choose a sibling folder instead.' : /\.(txt|md|rs|toml)$/.test(path) ? 'This path is not a folder.' : '';
        const pshow = t.path && perr;
        const used = s.mode === 'existing' && WT_USED[s.branch];
        const nerr = s.mode === 'new' ? D.refErr(s.newName.trim(), BRANCHES) : '';
        const nshow = t.newName && nerr;
        const nobr = s.mode === 'existing' && s.noBranches;
        const bad = !path || !!perr || !!used || nobr || (s.mode === 'new' && (!s.newName.trim() || !!nerr));
        const brOpts = BRANCHES.map((b) => [b, b + (WT_USED[b] ? `  — checked out in ${WT_USED[b]}` : '')]);
        const hint = run ? '' : !path ? 'Choose a folder for the worktree' : perr ? 'Fix the folder to continue' : nobr ? 'Choose a branch to check out' : used ? 'Choose a branch that is not in use' : s.mode === 'new' && !s.newName.trim() ? 'Enter a branch name' : nerr ? 'Fix the name to continue' : '';
        return { title: 'Add worktree', icon: 'worktree', body: `<div class="muted" style="font-size:var(--fs-sm)">A worktree is a second working folder for this repository, so you can work on another branch without stashing.</div>
          ${D.f('Folder', 'path', `<div class="od-row">${D.in('path', s.path, '~/develop/gitwarp-hotfix', { err: pshow, mono: 1, dis: run })}${D.btn('Browse…', { act: 'browse', icon: 'folder', dis: run })}</div>`, { req: 1, err: pshow ? perr : '', help: 'Pick a folder outside the repository. It is created if it does not exist.' })}
          <div class="od-stack" style="--od-gap:6px"><span class="field-label" style="margin:0">Branch</span>${D.seg('mode', s.mode, [['existing', 'Existing branch'], ['new', 'New branch']], 'Branch source')}</div>
          ${s.mode === 'existing'
            ? D.f('Check out', 'branch', D.sel('branch', s.branch, nobr ? [['', '']] : brOpts, run || nobr), { err: used ? `“${s.branch}” is already checked out in ${used}. A branch can only be open in one worktree.` : '' })
            : `<div class="pair">${D.f('New branch name', 'newName', D.in('newName', s.newName, 'fix/my-change', { err: nshow, mono: 1, dis: run }), { req: 1, err: nshow ? nerr : '' })}${D.f('Start from', 'base', D.sel('base', s.base, [['main', 'main'], ['feat/idle-lanes', 'feat/idle-lanes'], ['feat/palette', 'feat/palette'], ['fix/git-detect', 'fix/git-detect'], ['e4f2a91', 'e4f2a91 — selected commit']], run))}</div>`}
          ${D.chk('open', s.open, 'Open the worktree in a new window', run)}${D.rerr(s)}`,
          foot: `${run || hint ? `<span class="foot-hint">${run ? 'Working…' : hint}</span>` : ''}${D.btn('Cancel', { dis: run })}${D.btn('Add worktree', { kind: 'primary', act: 'add', dis: bad || run, def: 1 }).replace('class="btn btn-primary"', 'class="btn btn-primary" style="min-width:96px"')}` };
      },
      actions: {
        browse: (s, api) => { api.toast('Native-style folder picker opens here (custom-drawn dialog in GPUI)', 'info'); return false; },
        add: (s, api) => { const b = s.mode === 'new' ? s.newName.trim() : s.branch; if (onDone) onDone({ path: s.path.trim(), branch: b }); return D.go(s, api, `Added worktree ${s.path.trim()} on ${b}`); }
      }
    });
  };
  D.worktreeRemove = function (win, init, onDone) {
    const st = Object.assign({ wt: { path: '~/develop/gitwarp-hotfix', branch: 'fix/git-detect', dirty: ['src/settings/git.rs', 'src/settings/detect.rs', 'Cargo.lock'], locked: false, missing: false, main: false }, force: false, delBranch: false, phase: 'idle', err: '' }, init || {});
    return D.open(win, {
      st, width: 500, busy: (s) => s.phase === 'running' || s.phase === 'unlocking',
      view: (s) => {
        const w = s.wt, dirty = w.dirty && w.dirty.length > 0, run = s.phase === 'running', busy = run || s.phase === 'unlocking';
        let body = '', primary, hint = '';
        if (w.main) {
          body = D.banner('error', 'This is the main worktree', 'It holds the repository itself and cannot be removed. Close the repository instead if you are done with it.');
          primary = D.btn('Remove worktree', { kind: 'danger', act: 'remove', dis: true }); hint = 'Not available for the main worktree';
        } else {
          body = `<dl class="kv" style="grid-template-columns:64px minmax(0,1fr);gap:3px 12px"><dt>Folder</dt><dd>${esc(w.path)}</dd><dt>Branch</dt><dd>${w.branch ? esc(w.branch) : 'detached'}</dd></dl>`;
          if (w.missing) body += D.banner('info', 'The folder no longer exists on disk', 'Git still lists this worktree. Removing it only cleans up the record — no files are touched.');
          if (w.locked) body += D.banner('warn', 'This worktree is locked', `Locked with reason “${esc(w.lockReason || 'review in progress')}”. Unlock it before removing.`) + `<div>${D.btn('Unlock', { act: 'unlock', icon: 'lock', dis: busy })}</div>`;
          if (dirty) body += D.banner('warn', `${w.dirty.length} ${w.dirty.length === 1 ? 'file has' : 'files have'} uncommitted changes`, 'Removing the worktree discards them for good.', `<ul class="filelist">${w.dirty.map((f) => `<li>${esc(f)}</li>`).join('')}</ul>`) + D.chk('force', s.force, 'Discard these changes and remove the worktree', busy);
          if (!w.missing && !dirty && !w.locked) body += `<div class="muted" style="font-size:var(--fs-sm)">The folder is deleted from disk. ${w.branch ? `The branch <span class="mono">${esc(w.branch)}</span> and its commits stay in the repository.` : 'Its commits stay in the repository.'}</div>`;
          if (w.branch) body += D.chk('delBranch', s.delBranch, `Also delete branch <span class="mono">${esc(w.branch)}</span> (fully merged)`, busy);
          const blocked = w.locked || (dirty && !s.force);
          primary = D.btn(dirty && s.force ? 'Discard and remove' : 'Remove worktree', { kind: 'danger', act: 'remove', dis: !!blocked || busy }).replace('class="btn btn-danger"', 'class="btn btn-danger" style="min-width:96px"');
          hint = s.phase === 'unlocking' ? 'Unlocking…' : run ? 'Working…' : w.locked ? 'Unlock the worktree first' : dirty && !s.force ? 'Confirm to discard changes' : '';
        }
        return { title: 'Remove worktree', icon: 'trash', body: body + D.rerr(s), foot: `${hint ? `<span class="foot-hint">${hint}</span>` : ''}${D.btn('Cancel', { dis: busy })}${primary}` };
      },
      actions: {
        unlock: (s, api) => { s.phase = 'unlocking'; api.redraw(); setTimeout(() => { if (!api.el.isConnected) return; s.wt.locked = false; s.phase = 'idle'; api.toast('Worktree unlocked', 'ok'); api.redraw(); }, 700); return false; },
        remove: (s, api) => { if (onDone) onDone(s.wt); return D.go(s, api, s.keptErr ? `Removed worktree ${s.wt.path}; kept ${s.wt.branch}: ${s.keptErr}` : 'Removed worktree ' + s.wt.path); }
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
          ok: D.banner('ok', 'Private key detected', 'ed25519 · no passphrase', `<div class="field-help" style="margin-top:4px">Consider adding a passphrase with <span class="mono">ssh-keygen -p</span>.</div>`), enc: D.banner('info', 'Key is protected by a passphrase', 'Enter it once so GitWarp can read the fingerprint. It is not stored unless you allow the keychain.') }[r.k];
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
  /* SSH host key prompt: pops up before any SSH network operation when the host is unknown or its key changed (host_key_dialog.rs). */
  D.hostKey = function (win, init, onDone) {
    const st = Object.assign({ changed: false, host: 'github.com' }, init || {});
    const KEYS = ['SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU (ED25519)', 'SHA256:p2QAMXNIC1TJYWeIOttrVc98/R1BUFWu3/LiyKgUfQM (ECDSA)', 'SHA256:uNiVztksCsDhcc0u9e8BujQXVUpKZIDTMczCvj3tD2s (RSA)'];
    const OLD = ['SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU (ED25519)'], NEW = ['SHA256:Bq7nV2xKd9LpT4mYc1RzWe0HaUs5oGf8iJt3NyX6Cv1 (ED25519)'];
    const box = (l) => `<div class="fpbox">${l.map((k) => `<div class="mono">${esc(k)}</div>`).join('')}</div>`;
    const note = '<div class="muted" style="font-size:var(--fs-sm)">The key is remembered in <span class="mono">~/.ssh/known_hosts</span>.</div>';
    return D.open(win, {
      st, width: 500,
      view: (s) => s.changed
        ? { title: 'Host key has changed', icon: 'warn', body: `<div class="od-stack" style="--od-gap:12px">${D.banner('error', 'Possible man-in-the-middle attack', `The key presented by <span class="mono">${esc(s.host)}</span> is different from the one trusted before. Someone may be intercepting the connection, or the server was reinstalled.`)}
            <div class="od-stack" style="--od-gap:4px"><span class="field-label" style="margin:0;font-size:var(--fs-sm);color:var(--muted)">Previously trusted</span>${box(OLD)}</div>
            <div class="od-stack" style="--od-gap:4px"><span class="field-label" style="margin:0;font-size:var(--fs-sm);color:var(--muted)">Presented now</span>${box(NEW)}</div>${note}</div>`,
          /* danger primary: enabled at once (no acknowledgement checkbox), but never the Enter default */
          foot: D.btn('Cancel') + D.btn('Replace key', { kind: 'danger', act: 'trust' }).replace('class="btn btn-danger"', 'class="btn btn-danger" style="min-width:96px"') }
        : { title: 'SSH host key', icon: 'lock', body: `<div class="od-stack" style="--od-gap:12px">${D.banner('info', 'Trust this host?', `The authenticity of <span class="mono">${esc(s.host)}</span> cannot be verified yet. This is the first time you connect to it.`)}${box(KEYS)}
            <div class="muted" style="font-size:var(--fs-sm)">Compare the fingerprint with the one published by your Git host.</div>${note}</div>`,
          foot: D.btn('Cancel') + D.btn('Trust host', { kind: 'primary', act: 'trust', def: 1 }).replace('class="btn btn-primary"', 'class="btn btn-primary" style="min-width:96px"').replace('<button', '<button data-autofocus') },
      actions: { trust: (s, api) => { api.close(); api.toast(s.changed ? 'Replaced the trusted key for ' + s.host : 'Trusted ' + s.host, 'ok'); if (onDone) onDone(s); return false; } }
    });
  };

  /* Add / Edit remote share one builder; Edit prefills the fields and swaps the fetch checkbox for a note. */
  function remoteDialog(win, init, edit) {
    const st = Object.assign({ name: edit ? 'upstream' : '', url: edit ? 'git@github.com:acme/gitwarp.git' : '', fetch: true, remotes: ['origin', 'upstream'], phase: 'idle', err: '', old: edit ? 'upstream' : '' }, init || {});
    return D.open(win, {
      st, width: 500, busy: (s) => s.phase === 'running',
      view: (s) => {
        const n = s.name.trim(), p = parseUrl(s.url), t = s.touched, run = s.phase === 'running';
        const taken = s.remotes.filter((r) => r !== s.old);
        const nerr = !n ? '' : /\s/.test(n) ? 'Name cannot contain spaces.' : taken.includes(n) ? `Remote “${n}” already exists.` : '';
        const uerr = p.kind === 'bad' ? 'Not a Git address. Use https://host/user/repo.git or git@host:user/repo.git' : '';
        const ok = n && !nerr && (p.kind === 'https' || p.kind === 'ssh');
        /* address note: coloured icon + text, no chip */
        const proto = p.kind === 'ssh' ? `<span class="od-row" style="gap:6px;color:var(--info)">${ic('key', 12)}SSH · uses your SSH keys</span>` : p.kind === 'https' ? `<span class="od-row" style="gap:6px;color:var(--success)">${ic('check', 12)}HTTPS · uses your saved account</span>` : '';
        return { title: edit ? 'Edit remote' : 'Add remote', icon: 'cloud', body: `${D.f('Name', 'name', D.in('name', s.name, 'upstream', { err: t.name && nerr, mono: 1, dis: run }), { req: 1, err: t.name ? nerr : '', help: s.remotes.length ? 'Existing remotes: ' + s.remotes.map((r) => `<span class="mono">${esc(r)}</span>`).join(', ') : 'No remotes yet.' })}
          ${D.f('Address', 'url', D.in('url', s.url, 'git@github.com:user/repo.git', { err: t.url && uerr, mono: 1, dis: run }), { req: 1, err: t.url ? uerr : '', help: proto })}
          ${edit ? '<div class="field-help" style="margin:0">Renaming the remote also renames its remote-tracking branches.</div>' : D.chk('fetch', s.fetch, 'Fetch branches and tags right after adding', run)}${D.rerr(s)}`,
          foot: D.rfoot(s, edit ? 'Save' : 'Add remote', { act: 'ok', dis: !ok, hint: !n ? 'Enter a remote name' : nerr ? 'Choose another name' : !p.kind ? 'Enter the remote address' : uerr ? 'Fix the address' : '' }) };
      },
      actions: { ok: (s, api) => D.go(s, api, edit ? `Updated remote ${s.name.trim()}` : `Added remote ${s.name.trim()}`) }
    });
  }
  D.remoteAdd = (win, init) => remoteDialog(win, init, false);
  D.remoteEdit = (win, init) => remoteDialog(win, init, true);

  /* ====================================================================
     Rename branch / tag / worktree, Reset, Cherry-pick, Revert, Delete*, Drop stash
     One shared shell: ref_dialog.rs. Footer = Cancel + primary; running = "Working…", both disabled.
     ==================================================================== */
  const WT_CUR = '~/develop/gitwarp-hotfix';
  D.rename = function (win, kind, init) {
    const old = kind === 'worktree' ? 'gitwarp-hotfix' : kind === 'tag' ? 'v0.3.0' : 'feat/palette';
    const st = Object.assign({ name: old, old, phase: 'idle', err: '' }, init || {});
    const T = { branch: ['Rename branch', 'New branch name', 'feat/my-change', 'Enter a branch name'], tag: ['Rename tag', 'New tag name', 'v0.3.1', 'Enter a tag name'], worktree: ['Rename worktree', 'New folder name', 'new-folder-name', 'Enter a folder name'] }[kind];
    return D.open(win, {
      st, width: 480, busy: (s) => s.phase === 'running',
      view: (s) => {
        const n = s.name.trim(), run = s.phase === 'running';
        const err = !n ? '' : kind === 'worktree' ? (/[\\/]/.test(n) ? 'Use a single folder name, without / or \\' : '') : D.refErr(n, (kind === 'tag' ? TAGS : BRANCHES).filter((x) => x !== s.old));
        const same = n === s.old, show = s.touched.name && err;
        const help = kind === 'worktree' ? `Moves <span class="mono">${esc(WT_CUR)}</span> next to its current folder. Git refuses a locked or main worktree.` : `Renames <span class="mono">${esc(s.old)}</span> in this repository.`;
        return { title: T[0], icon: 'edit', body: `${D.f(T[1], 'name', D.in('name', s.name, T[2], { err: show, mono: 1, dis: run }), { req: 1, err: show ? err : '', help })}${D.rerr(s)}`,
          foot: D.rfoot(s, 'Rename', { act: 'ok', dis: !n || !!err || same, hint: !n ? T[3] : err ? (kind === 'worktree' ? 'Fix the name to continue' : 'Fix the name to continue') : same ? 'Enter a new name' : '' }) };
      },
      actions: { ok: (s, api) => D.go(s, api, kind === 'worktree' ? `Moved worktree to ~/develop/${s.name.trim()}` : kind === 'tag' ? `Renamed tag ${s.old} to ${s.name.trim()}` : `Renamed ${s.old} to ${s.name.trim()}`) }
    });
  };
  D.reset = function (win, init) {
    const st = Object.assign({ mode: 'mixed', commit: G.commits[3], phase: 'idle', err: '', branch: 'main' }, init || {});
    const HELP = { soft: 'Moves HEAD only. The index and work tree keep their changes.', mixed: 'Moves HEAD and clears the index. The work tree keeps its files.', hard: 'Moves HEAD and discards index and work tree changes.' };
    return D.open(win, {
      st, width: 500, busy: (s) => s.phase === 'running',
      view: (s) => ({ title: 'Reset branch to here', icon: 'undo', body: `${D.commitCard(s.commit)}
        <div class="od-stack" style="--od-gap:6px"><span class="field-label" style="margin:0">Mode</span>${D.seg('mode', s.mode, [['soft', 'Soft'], ['mixed', 'Mixed'], ['hard', 'Hard']], 'Reset mode')}<div class="field-help">${HELP[s.mode]}</div></div>
        ${s.mode === 'hard' ? D.banner('warn', `Hard reset to ${s.commit.hash} discards every local change`, 'Staged and unstaged changes in the work tree are lost. This cannot be undone.') : D.banner('info', `Moves ${s.branch} to ${s.commit.hash}`, 'Local commits after this point become unreachable from the branch.')}${D.rerr(s)}`,
        foot: D.rfoot(s, 'Reset', { kind: 'danger', act: 'ok' }) }),
      actions: { ok: (s, api) => D.go(s, api, `Reset ${s.branch} to ${s.commit.hash}`) }
    });
  };
  /* Cherry-pick / Revert: commit card + info banner, primary (not danger) button. */
  D.commitOp = function (win, kind, init) {
    const st = Object.assign({ commit: G.commits[2], phase: 'idle', err: '', branch: 'main' }, init || {});
    const cp = kind === 'cherry';
    return D.open(win, {
      st, width: 480, busy: (s) => s.phase === 'running',
      view: (s) => ({ title: cp ? 'Cherry-pick commit' : 'Revert commit', icon: 'undo', body: `${D.commitCard(s.commit)}
        ${cp ? D.banner('info', `Applies ${s.commit.hash} on top of ${s.branch}.`, 'A conflict aborts the cherry-pick and leaves the repository as it was.') : D.banner('info', `Creates a new commit that undoes ${s.commit.hash}.`, 'A conflict aborts the revert and leaves the repository as it was.')}${D.rerr(s)}`,
        foot: D.rfoot(s, cp ? 'Cherry-pick' : 'Revert', { act: 'ok' }) }),
      actions: { ok: (s, api) => D.go(s, api, cp ? `Cherry-picked ${s.commit.hash}` : `Reverted ${s.commit.hash}`) }
    });
  };
  D.cherry = (win, init) => D.commitOp(win, 'cherry', init);
  D.revert = (win, init) => D.commitOp(win, 'revert', init);
  /* Delete branch (two-step) / Delete tag / Remove remote / Drop stash: one warn banner, danger primary (never the Enter default). */
  D.del = function (win, kind, init) {
    const st = Object.assign({ name: kind === 'tag' ? 'v0.2.0' : kind === 'remote' ? 'upstream' : 'feat/idle-lanes', n: 0, msg: 'WIP on feat/palette: 3a91c0e Add palette skeleton', step: 1, unmerged: false, phase: 'idle', err: '' }, init || {});
    const L = { branch: ['Delete branch', 'Delete branch', 'Deleted branch'], tag: ['Delete tag', 'Delete tag', 'Deleted tag'], remote: ['Remove remote', 'Remove remote', 'Removed remote'], stash: ['Drop stash', 'Drop stash', 'Dropped stash@{' + st.n + '}'] }[kind];
    return D.open(win, {
      st, width: 480, busy: (s) => s.phase === 'running',
      view: (s) => {
        const n = esc(s.name); let b;
        if (kind === 'branch') b = s.step === 2 ? D.banner('warn', `The branch “${n}” has commits that are not merged anywhere else. Deleting it loses them.`, 'Deleting an unmerged branch is permanent.') : D.banner('warn', `Deletes the local branch “${n}”. This cannot be undone.`, 'The commits stay in the repository until they are unreachable.');
        else if (kind === 'tag') b = D.banner('warn', `Deletes the tag “${n}”. This cannot be undone.`, 'Other clones keep their copy until it is deleted there too.');
        else if (kind === 'remote') b = D.banner('warn', `Removes the remote “${n}” and its remote-tracking branches from this repository.`, 'The repository on the server is not touched.');
        else b = D.banner('warn', `Drops stash@{${s.n}}: ${esc(s.msg)}`, 'The changes are gone; this cannot be undone.');
        return { title: L[0], icon: 'trash', body: b + D.rerr(s), foot: D.rfoot(s, kind === 'branch' && s.step === 2 ? 'Delete anyway' : L[1], { kind: 'danger', act: 'ok', hint: kind === 'branch' && s.step === 2 ? 'This cannot be undone' : '' }) };
      },
      actions: {
        ok: (s, api) => {
          /* git refuses an unmerged branch: the same dialog switches in place to step 2 (no error banner) */
          if (kind === 'branch' && s.unmerged && s.step === 1) return D.go(s, api, '', { then: (x) => { x.step = 2; } });
          return D.go(s, api, L[2] + (kind === 'stash' ? '' : ' ' + s.name));
        }
      }
    });
  };
  /* The one generic confirm dialog: delete a remote branch. Autofocus is on Cancel; Enter does not delete. */
  D.confirmRemoteBranch = function (win, init) {
    const st = Object.assign({ remote: 'origin', branch: 'feat/old-spike' }, init || {});
    return D.open(win, {
      st, width: 440,
      view: (s) => ({ title: `Delete ${esc(s.remote)}/${esc(s.branch)}?`, icon: 'warn', body: `<div>This deletes the branch “${esc(s.branch)}” on ${esc(s.remote)}. It cannot be undone.</div>`,
        foot: `<button type="button" class="btn" style="min-width:72px" data-do="close" data-autofocus>Cancel</button>${D.btn('Delete branch', { kind: 'danger', act: 'ok' }).replace('class="btn btn-danger"', 'class="btn btn-danger" style="min-width:96px"')}` }),
      actions: { ok: (s, api) => { api.close(); api.toast(`Deleted remote branch ${s.branch}`, 'ok'); return false; } }
    });
  };
  /* Uncommitted changes before checkout: Stash and switch is the default and autofocused (Enter = stash). */
  D.checkout = function (win, init) {
    const st = Object.assign({ origin: 'branch', target: 'feat/palette', n: DIRTY, conflict: false }, init || {});
    return D.open(win, {
      st, width: 520,
      view: (s) => {
        const T = { branch: `Switch to ${s.target}?`, remote: `Check out ${s.target}?`, tag: `Check out tag ${s.target}?`, commit: `Check out ${s.target}?` }[s.origin];
        return { title: esc(T), icon: 'warn', body: `<div class="od-stack" style="--od-gap:8px"><div>${s.n === 1 ? '1 uncommitted change' : s.n + ' uncommitted changes'} in the working tree. What should happen to them?</div><div class="muted" style="font-size:var(--fs-sm)">Carrying the changes fails when the other branch edits the same lines.</div></div>`,
          foot: `${D.btn('Cancel')}${D.btn('Discard', { kind: 'danger', act: 'discard' })}${D.btn('Bring changes', { act: 'bring' })}${D.btn('Stash and switch', { act: 'stash', def: 1 }).replace('<button', '<button data-autofocus')}` };
      },
      actions: {
        stash: (s, api) => { api.close(); api.toast('Stashed local changes', 'ok'); api.toast(done(s), 'ok'); return false; },
        discard: (s, api) => { api.close(); api.toast(done(s), 'ok'); return false; },
        bring: (s, api) => { api.close(); if (s.conflict) api.toast('error: Your local changes to the following files would be overwritten by checkout:\n\tsrc/settings/git.rs\nPlease commit your changes or stash them before you switch branches.', 'error'); else api.toast(done(s), 'ok'); return false; }
      }
    });
    function done(s) { return { branch: `Switched to ${s.target}`, remote: `Checked out ${s.target}`, tag: `Checked out tag ${s.target} (detached HEAD)`, commit: `Checked out ${s.target} (detached HEAD)` }[s.origin]; }
  };

  /* ====================================================================
     Network: credential prompt (git askpass)
     ==================================================================== */
  D.credentials = function (win, init) {
    const st = Object.assign({ prompt: "Password for 'https://user@github.com': ", v: '' }, init || {});
    const secret = (s) => /password|passphrase|token/i.test(s.prompt);
    return D.open(win, {
      st, width: 460,
      view: (s) => { const sec = secret(s), label = sec ? 'Password' : 'User name';
        return { title: 'Authentication required', icon: 'key', body: `<div class="od-stack" style="--od-gap:12px">${D.banner('info', 'The Git server asked for credentials', `<span class="mono" style="white-space:pre-wrap">${esc(s.prompt)}</span>`)}
          ${D.f(label, 'v', D.in('v', s.v, label, { type: sec ? 'password' : 'text', mono: !sec }))}
          <div class="muted" style="font-size:var(--fs-sm)">The answer is passed to git and is not saved by GitWarp.</div></div>`,
          foot: D.btn('Cancel', { act: 'cancel' }) + D.btn('Continue', { kind: 'primary', act: 'ok', dis: !s.v, def: 1 }) }; },
      onClose: null,
      actions: {
        ok: (s, api) => { api.close(); api.toast('Fetch complete', 'ok'); return false; },
        cancel: (s, api) => { api.close(); api.toast('Authentication failed. Check your credentials or configure a git credential helper.', 'error'); return false; }
      }
    });
  };

  /* ---------- entry used by context menus in the main window ---------- */
  D.openIn = (kind, win) => {
    win = win || document.querySelector('.win'); if (!win) return;
    let c = G.commits[0]; const r = win.querySelector('.lrow.sel'); if (r && r.dataset.i != null && G.commits[+r.dataset.i]) c = G.commits[+r.dataset.i];
    ({ branch: () => D.branch(win, { commit: c }), tag: () => D.tag(win, { commit: c }), worktreeAdd: () => D.worktreeAdd(win), worktreeRemove: () => D.worktreeRemove(win), remoteAdd: () => D.remoteAdd(win), remoteEdit: () => D.remoteEdit(win), renameBranch: () => D.rename(win, 'branch'), renameTag: () => D.rename(win, 'tag'), renameWorktree: () => D.rename(win, 'worktree'), reset: () => D.reset(win, { commit: c }), cherry: () => D.cherry(win, { commit: c }), revert: () => D.revert(win, { commit: c }), delBranch: () => D.del(win, 'branch'), delTag: () => D.del(win, 'tag'), delRemote: () => D.del(win, 'remote'), dropStash: () => D.del(win, 'stash'), checkout: () => D.checkout(win), clone: () => D.repo(win, { tab: 'clone' }), open: () => D.repo(win, { tab: 'open' }) })[kind]();
  };

  /* ---------- gallery page: one main window, a scenario switcher, the dialog on top ---------- */
  D.page = function (title, scenes, centerFn, o) {
    o = o || {};
    document.title = 'GitWarp — ' + title;
    const sel = `<label class="scn" style="display:inline-flex;gap:8px;align-items:center;font-size:12px">${o.label || 'Dialog state'} <select id="scn" aria-label="${o.label || 'Dialog state'}" style="height:24px;background:var(--bg);color:var(--text);border:1px solid var(--border-strong);border-radius:var(--r-sm)">${scenes.map((s, i) => `<option value="${i}">${esc(s[0])}</option>`).join('')}</select></label><button class="btn btn-sm" id="reopen">${o.reopen || 'Reopen dialog'}</button>`;
    G.stagebar(sel);
    const stage = G.h('<div class="stage"><div class="winwrap"></div></div>'); document.body.appendChild(stage);
    const sh = G.shell({ host: stage.firstChild, w: 1200, h: 800, view: 'history' });
    const win = sh.win;
    if (centerFn) centerFn(sh, win);
    else { sh.center.innerHTML = '<div class="hist-host" style="display:flex;flex:1;min-width:0;min-height:0;height:100%;width:100%"></div>'; G.history(sh.center.firstChild, G.commits, { gw: 112 }); }
    G.fit(win);
    const s = document.getElementById('scn');
    const run = () => { D.closeAll(win); const tc = G.layer(win).querySelector('.toasts'); if (tc) tc.remove(); scenes[+s.value][1](win); };
    /* deep link: ?s=<scene index>&t=<theme> opens a given state */
    const q = new URLSearchParams(location.search); if (q.has('s') && scenes[+q.get('s')]) s.value = q.get('s'); if (q.get('t') && G.THEMES[q.get('t')]) G.setTheme(q.get('t'));
    s.addEventListener('change', run); document.getElementById('reopen').addEventListener('click', run);
    document.addEventListener('keydown', (e) => { if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === 'o') { e.preventDefault(); D.repo(win, {}); } });
    run();
    return { win, run };
  };
})();
