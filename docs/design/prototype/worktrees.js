/* Worktrees workspace prototype.
   Three entry points to the same state: the titlebar switcher (Ctrl+Alt+W), the sidebar panel, and the center detail pane with quick create.
   Add / Remove reuse the dialogs from dialogs.js; everything else is custom here. State lives in memory; settings come from settings-worktrees.html (localStorage). */
(function () {
  const G = window.GL, S = window.GLS, D = window.GLD, ic = G.ic, esc = G.esc;
  const REPO = 'gitwarp';
  const MAINTIP = 'Git calls it the main worktree: the original folder of this repository. It cannot be removed.';
  const cfg = () => ({
    base: S.get('wt-base', '~/develop'),
    tpl: S.get('wt-tpl', '{repo}-{branch}'),
    after: S.get('wt-after', 'switch'),
    copyEnv: S.get('wt-env', true)
  });

  /* ---------- data ---------- */
  let seq = 10;
  let WT = [
    { id: 'w1', name: 'gitwarp', path: '~/develop/gitwarp', branch: 'main', hash: 'e4f2a91', msg: 'Merge branch feat/palette', main: true, ahead: 4, behind: 0, dirty: [], last: 2 },
    { id: 'w2', name: 'gitwarp-hotfix', path: '~/develop/gitwarp-hotfix', branch: 'fix/git-detect', hash: '8d41f6e', msg: 'Detect bundled git before system git', ahead: 1, behind: 0, dirty: [['M', 'src/settings/git.rs'], ['M', 'src/settings/detect.rs'], ['M', 'Cargo.lock']], last: 38 },
    { id: 'w3', name: 'gitwarp-review', path: '~/develop/gitwarp-review', branch: 'feat/palette', hash: 'a82c4d7', msg: 'Palette: group commands by scope', locked: true, lockReason: 'review in progress', ahead: 0, behind: 2, dirty: [], last: 300 },
    { id: 'w4', name: 'gitwarp-bisect', path: '~/develop/gitwarp-bisect', branch: '', hash: '3c9e0b2', msg: 'Add idle-lane collapse', ahead: 0, behind: 0, dirty: [['U', 'bisect.log']], last: 1500 },
    { id: 'w5', name: 'gitwarp-old', path: '~/develop/gitwarp-old', branch: 'feat/idle-lanes', hash: '19d8e4f', msg: 'Collapse idle lanes in the graph', missing: true, ahead: 0, behind: 0, dirty: [], last: 20000 }
  ];
  const BRANCHES = ['main', 'feat/idle-lanes', 'feat/palette', 'feat/ssh-keys', 'fix/git-detect', 'fix/old-experiment'];
  /* branches that exist only on a remote: quick create shows "tracks {remote}" for them */
  const REMOTE_ONLY = ['feat/remote-only'];
  let cur = 'w1', sel = 'w1', view = 'ready', hi = 0, popQuery = '', creating = false;
  let quickName = '', quickTouched = false;

  const byId = (id) => WT.find((w) => w.id === id);
  const usedBy = (b) => WT.find((w) => w.branch && w.branch === b);
  const slug = (b) => b.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
  const pathFor = (branch) => {
    const c = cfg();
    const leaf = c.tpl.replace('{repo}', REPO).replace('{branch}', slug(branch));
    let p = c.base + '/' + leaf, n = 2;
    while (WT.some((w) => w.path === p)) p = c.base + '/' + leaf + '-' + n++;
    return p;
  };
  const rel = (m) => (m < 2 ? 'now' : m < 60 ? m + ' min ago' : m < 1440 ? Math.floor(m / 60) + ' h ago' : Math.floor(m / 1440) + ' d ago');
  const label = (w) => w.branch || 'detached HEAD';
  const branchErr = (n) => (!n ? '' : /\s|\.\.|~|\^|:|\\|\?|\*|\[/.test(n) || n.startsWith('-') || n.endsWith('/') || n.endsWith('.lock') ? 'This is not a valid branch name.' : '');

  /* ---------- page chrome ---------- */
  G.stagebar(`<label class="scn" style="display:inline-flex;gap:8px;align-items:center;font-size:12px">State <select id="s-state" aria-label="Worktree list state"><option value="ready">Loaded</option><option value="loading">Loading</option><option value="empty">Only the main worktree</option><option value="error">Could not read</option></select></label><span class="muted" style="font-size:12px;margin-left:12px">1200×800</span>`);
  const stage = G.h('<div class="stage"><div class="winwrap"></div></div>');
  document.body.appendChild(stage);
  document.body.appendChild(G.h(`<p class="cap-line">Switch from the titlebar (Ctrl+Alt+W) — type to filter, or type a new branch name to create a worktree on the spot. The sidebar and the detail pane work on the same list: click to inspect, double-click or Enter to switch, right-click for the menu. Quick create fills the folder from your template (Settings → Worktrees); “Add worktree…” opens the full dialog.</p>`));
  const sh = G.shell({ host: stage.firstChild, w: 1200, h: 800, view: 'worktrees', statusInfo: '' });
  const win = sh.win;
  document.title = 'GitWarp — Worktrees';
  const q = (s) => win.querySelector(s);
  const toast = (m, k) => G.toast(win, m, k || 'info');

  /* switcher button, right after the branch button */
  const brBtn = q('[data-act=branch]');
  const wtBtn = G.h(`<button class="btn btn-tool wt-btn" data-act="worktree" data-tip="Switch worktree" data-kbd="Ctrl+Alt+W" aria-haspopup="listbox" aria-expanded="false"></button>`);
  brBtn.after(wtBtn);
  const drawBtn = () => {
    const w = byId(cur);
    wtBtn.innerHTML = `${ic('worktree', 14)}<b class="tb-name">${esc(w.name)}</b>${w.dirty.length ? `<span class="wt-dot" aria-hidden="true"></span><span class="sr-only" style="position:absolute;left:-9999px">${w.dirty.length} changed files</span>` : ''}${ic('chevD', 12)}`;
    q('#tb-branch').textContent = label(w);
  };

  /* status glyphs: icon/number, never colour alone */
  const glyphs = (w, full) => [
    w.locked && `<span class="st-lock" data-tip="Locked${w.lockReason ? ': ' + esc(w.lockReason) : ''}">${ic('lock', 12)}</span>`,
    w.missing && `<span class="st-gone" data-tip="Folder is missing">${ic('warn', 12)}</span>`,
    w.dirty.length && `<span class="st-chg" data-tip="${w.dirty.length} changed files">${ic('edit', 12)}${w.dirty.length}</span>`,
    (w.ahead || w.behind) && `<span class="st-ab" data-tip="${w.ahead} ahead, ${w.behind} behind">${w.ahead ? '↑' + w.ahead : ''}${w.behind ? '↓' + w.behind : ''}</span>`
  ].filter(Boolean).join('');

  /* ---------- actions ---------- */
  function switchTo(id, how) {
    const w = byId(id); if (!w) return;
    if (w.missing) { toast('The folder of ' + w.name + ' is missing. Remove the record or restore the folder.', 'error'); return; }
    if (id === cur) { toast(w.name + ' is already the current worktree', 'info'); return; }
    cur = id; sel = id; drawBtn(); drawSide(); drawCenter();
    toast(how === 'window' ? 'Opened ' + w.name + ' in a new window' : 'Switched to ' + w.name + ' · ' + label(w), 'ok');
  }
  function create(branch, opts) {
    if (creating) return;
    const b = branch.trim(), c = cfg();
    const exists = BRANCHES.includes(b), p = pathFor(b);
    creating = true; drawCenter();
    toast('Creating worktree ' + p + '…', 'info');
    setTimeout(() => {
      creating = false;
      const w = { id: 'w' + seq++, name: p.split('/').pop(), path: p, branch: b, hash: exists ? 'a82c4d7' : 'e4f2a91', msg: exists ? 'Palette: group commands by scope' : 'Merge branch feat/palette', ahead: 0, behind: 0, dirty: [], last: 0 };
      WT.push(w); if (!exists) BRANCHES.push(b);
      quickName = ''; quickTouched = false; sel = w.id;
      if (c.after === 'switch') cur = w.id;
      drawBtn(); drawSide(); drawCenter();
      toast('Created ' + w.name + (exists ? ' on ' + b : ' on new branch ' + b) + (c.copyEnv ? ' · copied .env.local' : ''), 'ok');
    }, 700);
  }
  function removeWt(w) {
    D.worktreeRemove(win, { wt: Object.assign({}, w, { dirty: w.dirty.map((d) => d[1]) }) }, () => {
      WT = WT.filter((x) => x.id !== w.id);
      if (cur === w.id) cur = 'w1';
      sel = cur; drawBtn(); drawSide(); drawCenter();
    });
  }
  function toggleLock(w) {
    w.locked = !w.locked; w.lockReason = w.locked ? 'locked manually' : '';
    toast((w.locked ? 'Locked ' : 'Unlocked ') + w.name, 'ok'); drawSide(); drawCenter();
  }
  function prune() {
    const n = WT.filter((w) => w.missing).length;
    WT = WT.filter((w) => !w.missing); if (!byId(sel)) sel = cur;
    toast('Pruned ' + n + ' stale worktree record' + (n === 1 ? '' : 's'), 'ok'); drawSide(); drawCenter();
  }
  const copyPath = (w) => { try { navigator.clipboard && navigator.clipboard.writeText(w.path); } catch (e) {} toast('Copied ' + w.path, 'ok'); };
  const addDialog = () => D.worktreeAdd(win, { path: '' }, (r) => {
    const w = { id: 'w' + seq++, name: r.path.split('/').pop(), path: r.path, branch: r.branch, hash: 'e4f2a91', msg: 'Merge branch feat/palette', ahead: 0, behind: 0, dirty: [], last: 0 };
    WT.push(w); sel = w.id; if (cfg().after === 'switch') cur = w.id; drawBtn(); drawSide(); drawCenter();
  });
  const menuFor = (w) => [
    { label: 'Switch to this worktree', icon: 'worktree', disabled: w.id === cur || w.missing, onClick: () => switchTo(w.id) },
    { label: 'Open in new window', icon: 'sidebar', disabled: w.missing, onClick: () => switchTo(w.id, 'window') },
    { label: 'Reveal in file manager', icon: 'folder', disabled: w.missing, onClick: () => toast('Revealed ' + w.path, 'info') },
    { label: 'Copy path', icon: 'copy', onClick: () => copyPath(w) },
    { sep: 1 },
    { label: w.locked ? 'Unlock' : 'Lock', icon: 'lock', disabled: w.main || w.missing, onClick: () => toggleLock(w) },
    { label: 'Remove worktree…', icon: 'trash', danger: 1, disabled: w.main, onClick: () => removeWt(w) }
  ];

  /* ---------- switcher popover ---------- */
  let pop = null, closePop = () => {};
  function popItems() {
    const t = popQuery.trim().toLowerCase();
    const list = WT.filter((w) => !t || (w.name + ' ' + w.branch + ' ' + w.path).toLowerCase().includes(t))
      .sort((a, b) => (a.id === cur ? -1 : b.id === cur ? 1 : a.last - b.last));
    const items = list.map((w) => ({ kind: 'wt', w }));
    const name = popQuery.trim();
    if (name && !usedBy(name)) items.push({ kind: 'create', name });
    return items;
  }
  const mark = (s, t) => { const i = t ? s.toLowerCase().indexOf(t.toLowerCase()) : -1; return i < 0 ? esc(s) : esc(s.slice(0, i)) + '<mark>' + esc(s.slice(i, i + t.length)) + '</mark>' + esc(s.slice(i + t.length)); };
  function drawPop() {
    if (!pop) return;
    const items = popItems(), t = popQuery.trim();
    hi = Math.max(0, Math.min(hi, items.length - 1));
    const errMsg = t ? branchErr(t) : '';
    let n = 0;
    pop.querySelector('.wtpop-list').innerHTML = view === 'loading' ? `<div class="wtsk">${[220, 160, 200].map((w) => `<div class="skel" style="width:${w}px"></div>`).join('')}</div>` : items.length ? items.map((it, i) => {
      if (it.kind === 'create') {
        const exists = BRANCHES.includes(it.name);
        return `<div class="wtpop-sec">${errMsg ? 'Not available' : 'Create'}</div><div class="wtitem${i === hi ? ' hi' : ''}${errMsg ? ' gone' : ''}" role="option" data-i="${i}" aria-selected="${i === hi}" ${errMsg ? 'aria-disabled="true"' : ''}>${ic('plus', 16)}<div style="min-width:0"><div class="nm od-truncate">${errMsg ? esc(errMsg) : (exists ? 'Open ' : 'Create ') + '“' + esc(it.name) + '” in a new worktree'}</div><div class="sub"><span class="od-truncate mono">${errMsg ? '' : esc(pathFor(it.name))}</span></div></div><span class="tail">${errMsg ? '' : '<span class="kbd">Enter</span>'}</span></div>`;
      }
      const w = it.w; n++;
      return `${i === 0 ? '<div class="wtpop-sec">Worktrees</div>' : ''}<div class="wtitem${w.id === cur ? ' cur' : ''}${i === hi ? ' hi' : ''}${w.missing ? ' gone' : ''}" role="option" data-i="${i}" aria-selected="${w.id === cur}">${ic(w.main ? 'git' : 'worktree', 16)}<div style="min-width:0"><div class="nm-row"><span class="nm od-truncate">${mark(w.name, t)}</span>${w.main ? '<span class="chip">default</span>' : ''}</div><div class="sub"><span class="od-truncate mono">${mark(label(w), t)}</span></div></div><span class="tail">${glyphs(w)}${n <= 9 && !t ? `<span class="kbd" data-tip="Alt+${n}">Alt+${n}</span>` : ''}</span></div>`;
    }).join('') : `<div class="wtpop-empty">No worktree matches. Type a branch name to create one.</div>`;
    const stale = WT.filter((w) => w.missing).length;
    pop.querySelector('.wtpop-foot').innerHTML = `<button class="btn btn-sm btn-ghost" data-pa="add">${ic('plus', 14)}Add worktree…</button>${stale ? `<button class="btn btn-sm btn-ghost" data-pa="prune">${ic('trash', 14)}Prune ${stale} missing</button>` : ''}<span class="od-fill"></span><span class="kbd">↑↓</span><span class="kbd">Enter</span><span class="kbd">Esc</span>`;
    const h = pop.querySelector('.hi'); if (h) h.scrollIntoView({ block: 'nearest' });
  }
  function activate(i) {
    const it = popItems()[i]; if (!it) return;
    if (it.kind === 'create') { if (branchErr(it.name)) return; closePop(); create(it.name); }
    else { closePop(); switchTo(it.w.id); }
  }
  function openPop() {
    if (pop) { closePop(); return; }
    popQuery = ''; hi = 0;
    const layer = G.layer(win);
    pop = G.h(`<div class="wtpop" role="dialog" aria-label="Switch worktree"><div class="wtpop-search"><label class="input-wrap">${ic('search', 14)}<input class="input" placeholder="Filter worktrees, or type a new branch name" aria-label="Filter worktrees or type a branch name" role="combobox" aria-expanded="true"></label></div><div class="wtpop-list" role="listbox" aria-label="Worktrees"></div><div class="wtpop-foot"></div></div>`);
    layer.appendChild(pop);
    const wr = win.getBoundingClientRect(), br = wtBtn.getBoundingClientRect(), s = wr.width / win.offsetWidth || 1;
    pop.style.left = Math.max(4, Math.min((br.left - wr.left) / s, win.offsetWidth - 384)) + 'px';
    pop.style.top = ((br.bottom - wr.top) / s + 4) + 'px';
    wtBtn.classList.add('on'); wtBtn.setAttribute('aria-expanded', 'true');
    drawPop();
    const inp = pop.querySelector('input'); inp.focus();
    inp.addEventListener('input', () => { popQuery = inp.value; hi = 0; drawPop(); });
    const out = (ev) => { if (!pop.contains(ev.target) && !wtBtn.contains(ev.target)) closePop(); };
    const key = (ev) => {
      const n = popItems().length;
      if (ev.key === 'ArrowDown') { hi = (hi + 1) % Math.max(n, 1); drawPop(); ev.preventDefault(); }
      else if (ev.key === 'ArrowUp') { hi = (hi - 1 + Math.max(n, 1)) % Math.max(n, 1); drawPop(); ev.preventDefault(); }
      else if (ev.key === 'Enter') { activate(hi); ev.preventDefault(); }
      else if (ev.key === 'Escape') { closePop(); wtBtn.focus(); ev.stopPropagation(); }
      else if (ev.altKey && /^[1-9]$/.test(ev.key) && !popQuery) { const wts = popItems().filter((x) => x.kind === 'wt'); const x = wts[+ev.key - 1]; if (x) { closePop(); switchTo(x.w.id); } ev.preventDefault(); }
    };
    pop.addEventListener('keydown', key);
    pop.addEventListener('mousemove', (ev) => { const el = ev.target.closest('.wtitem'); if (el && +el.dataset.i !== hi) { hi = +el.dataset.i; pop.querySelectorAll('.wtitem').forEach((x) => x.classList.toggle('hi', +x.dataset.i === hi)); } });
    pop.addEventListener('click', (ev) => {
      const el = ev.target.closest('.wtitem'), a = ev.target.closest('[data-pa]');
      if (el) activate(+el.dataset.i);
      else if (a && a.dataset.pa === 'add') { closePop(); addDialog(); }
      else if (a && a.dataset.pa === 'prune') { prune(); drawPop(); }
    });
    setTimeout(() => document.addEventListener('mousedown', out, true), 0);
    closePop = () => { if (!pop) return; pop.remove(); pop = null; wtBtn.classList.remove('on'); wtBtn.setAttribute('aria-expanded', 'false'); document.removeEventListener('mousedown', out, true); closePop = () => {}; };
  }
  wtBtn.addEventListener('click', openPop);
  /* Esc leaves the Worktrees view (back to History) when nothing else is open */
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !pop && !win.querySelector('.layer > .scrim, .layer > .menu') && !/INPUT|SELECT|TEXTAREA/.test((document.activeElement || {}).tagName || '')) location.href = 'main-1200.html'; });
  document.addEventListener('keydown', (e) => { if (e.ctrlKey && e.altKey && e.key.toLowerCase() === 'w') { e.preventDefault(); openPop(); } });

  /* ---------- sidebar ---------- */
  const tree = q('.tree');
  const sbf = q('.sbfilter input'); const sbf2 = sbf.cloneNode(true); sbf.replaceWith(sbf2); /* drop the branch-tree filter handler; this panel filters worktrees */
  sbf2.placeholder = 'Filter worktrees…'; sbf2.setAttribute('aria-label', 'Filter worktrees');
  let sbq = '';
  sbf2.addEventListener('input', () => { sbq = sbf2.value.trim().toLowerCase(); drawSide(); });
  function drawSide() {
    if (view === 'loading') { tree.innerHTML = `<div class="wtside"><div class="wtside-none"><div class="od-stack" style="--od-gap:10px">${[180, 140, 170, 150].map((w) => `<div class="skel" style="width:${w}px"></div>`).join('')}</div></div></div>`; return; }
    if (view === 'error') { tree.innerHTML = `<div class="wtside"><div class="wtside-none">Could not read the worktree list.</div></div>`; return; }
    /* the default (main) worktree is pinned first and stays distinct from the current one; the rest sort by recency */
    const list = WT.filter((w) => !sbq || (w.name + ' ' + w.branch).toLowerCase().includes(sbq)).sort((a, b) => (a.main ? -1 : b.main ? 1 : a.last - b.last));
    tree.innerHTML = `<div class="wtside" role="listbox" aria-label="Worktrees"><div class="node sec" style="--lvl:0">Worktrees<span class="ncount">${WT.length}</span></div>${list.length ? list.map((w, i) => `${i === 1 && list[0].main ? '<div class="wt-div" role="separator"></div>' : ''}<div class="node${w.main ? ' main' : ''}${w.id === cur ? ' cur' : ''}${w.id === sel ? ' sel' : ''}${w.missing ? ' gone' : ''}" role="option" tabindex="0" data-id="${w.id}" aria-selected="${w.id === sel}"${w.id === cur ? ' aria-current="true"' : ''} style="--lvl:0"${w.main ? ' data-tip="' + MAINTIP + '"' : ''}>${ic(w.main ? 'git' : 'worktree', 14)}<div class="nbody"><span class="nl-row"><span class="nl od-truncate">${esc(w.name)}</span>${w.main ? '<span class="chip">default</span>' : ''}</span><span class="ns od-truncate mono">${esc(label(w))}</span></div><span class="tail">${glyphs(w)}</span></div>`).join('') : `<div class="wtside-none">No worktree matches “${esc(sbq)}”.</div>`}<div class="add-row"><button class="btn btn-sm" data-sa="add">${ic('plus', 14)}New worktree…</button></div></div>`;
  }
  tree.addEventListener('click', (e) => {
    if (e.target.closest('[data-sa=add]')) { focusQuick(); return; }
    const n = e.target.closest('.node[data-id]'); if (!n) return;
    sel = n.dataset.id;
    tree.querySelectorAll('.node[data-id]').forEach((x) => { const on = x.dataset.id === sel; x.classList.toggle('sel', on); x.setAttribute('aria-selected', on); });
    drawCenter();
  });
  tree.addEventListener('dblclick', (e) => { const n = e.target.closest('.node[data-id]'); if (n) switchTo(n.dataset.id); });
  tree.addEventListener('keydown', (e) => {
    const nodes = Array.from(tree.querySelectorAll('.node[data-id]')); const i = nodes.indexOf(document.activeElement);
    if (e.key === 'ArrowDown' && i < nodes.length - 1) { nodes[i + 1].focus(); nodes[i + 1].click(); e.preventDefault(); }
    else if (e.key === 'ArrowUp' && i > 0) { nodes[i - 1].focus(); nodes[i - 1].click(); e.preventDefault(); }
    else if (e.key === 'Enter' && i >= 0) switchTo(nodes[i].dataset.id);
    else if (e.key === 'Delete' && i >= 0) { const w = byId(nodes[i].dataset.id); if (w && !w.main) removeWt(w); }
  });
  tree.addEventListener('contextmenu', (e) => {
    const n = e.target.closest('.node[data-id]'); if (!n) return;
    sel = n.dataset.id; tree.querySelectorAll('.node[data-id]').forEach((x) => x.classList.toggle('sel', x.dataset.id === sel)); drawCenter();
    G.menu(win, e, menuFor(byId(sel)));
  });

  /* ---------- center ---------- */
  const center = sh.center;
  center.innerHTML = '<div class="wtwork" aria-live="polite"></div>';
  const work = center.firstChild;
  function focusQuick() { const i = work.querySelector('#quick'); if (i) { i.focus(); i.select(); } else toast('The worktree list is still loading', 'info'); }

  function quickHTML() {
    const n = quickName.trim(), err = branchErr(n), used = n && usedBy(n);
    const exists = n && (BRANCHES.includes(n) || REMOTE_ONLY.includes(n)); /* a remote-only branch is created as a tracking branch */
    const showErr = quickTouched && (err || used);
    const free = BRANCHES.filter((b) => !usedBy(b));
    let hint;
    if (showErr) hint = `<span class="quick-err">${ic('warn', 12)}${used ? `“${esc(n)}” is already checked out in <b>${esc(used.name)}</b>. ` : esc(err)}</span>${used ? `<button class="btn btn-sm btn-ghost" data-q="goto" data-id="${used.id}">Go to ${esc(used.name)}</button>` : ''}`;
    else if (n) hint = `${ic('folder', 12)}<span class="od-truncate mono">${esc(pathFor(n))}</span><span class="od-nowrap">· ${REMOTE_ONLY.includes(n) ? 'tracks origin' : exists ? 'existing branch' : byId(cur).branch ? 'new branch from ' + esc(byId(cur).branch) : 'new branch from the current commit'}</span>`;
    else hint = `Folder is filled from your template <span class="mono od-nowrap">${esc(cfg().base + '/' + cfg().tpl)}</span>`;
    return `<section class="quick" aria-label="Quick create"><div class="quick-row"><label class="input-wrap" ${showErr ? 'style="border-color:var(--error)"' : ''}>${ic('branch', 14)}<input id="quick" class="input mono" value="${esc(quickName)}" placeholder="Branch to work on — existing or new" aria-label="Branch name" ${creating ? 'disabled' : ''} ${showErr ? 'aria-invalid="true"' : ''} autocomplete="off" spellcheck="false"></label><button class="btn btn-primary" data-q="go" ${!n || err || used || creating ? 'disabled' : ''}>${creating ? ic('fetch', 14, 'spin') + 'Creating…' : ic('plus', 14) + 'Create worktree'}</button><button class="btn" data-q="dlg" ${creating ? 'disabled' : ''}>Add worktree…</button></div><div class="quick-hint">${hint}</div>${free.length ? `<div class="quick-chips"><span class="lbl">Branches without a worktree</span>${free.map((b) => `<button class="bchip" data-q="chip" data-b="${esc(b)}" aria-label="Open ${esc(b)} in a new worktree">${ic('branch', 12)}${esc(b)}</button>`).join('')}</div>` : ''}</section>`;
  }
  function detailHTML(w) {
    const isCur = w.id === cur;
    const chips = [isCur && `<span class="chip acc">${ic('check', 10)}current</span>`, w.main && `<span class="chip" data-tip="${MAINTIP}">${ic('git', 10)}default worktree</span>`, w.locked && `<span class="chip warn">${ic('lock', 10)}locked${w.lockReason ? ': ' + esc(w.lockReason) : ''}</span>`, w.missing && `<span class="chip err">${ic('warn', 10)}folder missing</span>`, !w.missing && (w.dirty.length ? `<span class="chip warn">${ic('edit', 10)}${w.dirty.length} changed</span>` : `<span class="chip ok">${ic('check', 10)}clean</span>`)].filter(Boolean).join(' ');
    const files = w.dirty.slice(0, 5).map(([s, f]) => `<div class="fl"><span class="stt ${s}" aria-label="${{ M: 'Modified', A: 'Added', D: 'Deleted', U: 'Untracked' }[s]}">${s}</span><span class="od-truncate">${esc(f)}</span></div>`).join('') + (w.dirty.length > 5 ? `<div class="muted" style="font-size:var(--fs-sm)">+${w.dirty.length - 5} more</div>` : '');
    return `<section class="detail" aria-label="Worktree ${esc(w.name)}">
      <div class="detail-top">${ic(w.main ? 'git' : 'worktree', 20, 'big')}<div class="ttl"><h3>${esc(w.name)} ${chips}</h3><div class="detail-path"><span class="od-truncate ${w.missing ? '' : ''}" ${w.missing ? 'style="text-decoration:line-through"' : ''}>${esc(w.path)}</span><button class="btn btn-icon btn-ghost btn-sm" data-d="copy" aria-label="Copy path" data-tip="Copy path">${ic('copy', 14)}</button></div></div>
        <div class="detail-acts"><button class="btn btn-primary" data-d="switch" ${isCur || w.missing ? 'disabled' : ''}>${isCur ? 'Current worktree' : 'Switch to this worktree'}</button><button class="btn" data-d="window" ${w.missing ? 'disabled' : ''}>Open in new window</button></div></div>
      ${w.missing ? `<div style="padding:12px 16px"><div class="banner warn" role="status">${ic('warn', 16)}<div><div class="b-title">The folder no longer exists</div><div>git still lists this worktree. Prune the record, or restore the folder and it will reappear as clean.</div></div></div></div>` : ''}
      <div class="detail-grid"><div><span class="k">Branch</span><span class="v">${ic('branch', 14)}<span class="od-truncate mono">${esc(label(w))}</span></span></div><div><span class="k">HEAD</span><span class="v"><span class="mono">${w.hash}</span><span class="od-truncate muted" style="font-size:var(--fs-sm)">${esc(w.msg)}</span></span></div><div><span class="k">Remote</span><span class="v">${w.ahead || w.behind ? `<span class="st-ab" style="font-size:var(--fs-base);color:var(--text)">${w.ahead ? '↑ ' + w.ahead + ' ahead' : ''}${w.ahead && w.behind ? ' · ' : ''}${w.behind ? '↓ ' + w.behind + ' behind' : ''}</span>` : `<span class="muted">${w.branch ? 'Up to date' : 'No upstream (detached)'}</span>`}</span></div></div>
      ${w.missing ? '' : `<div class="detail-sec"><h4>Local changes</h4>${w.dirty.length ? files : `<div class="muted" style="font-size:var(--fs-base)">Nothing to commit — safe to remove or switch away.</div>`}</div>`}
      <div class="detail-foot"><button class="btn btn-ghost btn-sm" data-d="reveal" ${w.missing ? 'disabled' : ''}>${ic('folder', 14)}Reveal in file manager</button><button class="btn btn-ghost btn-sm" data-d="lock" ${w.main || w.missing ? 'disabled' : ''}>${ic('lock', 14)}${w.locked ? 'Unlock' : 'Lock'}</button><span class="od-fill"></span>${w.main ? '<span class="hint">The main worktree cannot be removed.</span>' : ''}<button class="btn btn-sm" data-d="remove" ${w.main ? 'disabled' : ''} style="${w.main ? '' : 'color:var(--error);border-color:var(--error)'}">${ic('trash', 14)}Remove…</button></div></section>`;
  }
  function drawCenter() {
    const keep = document.activeElement && document.activeElement.id === 'quick' ? document.activeElement.selectionStart : null;
    if (view === 'loading') { work.innerHTML = `<div class="wthead"><h2>Worktrees</h2></div><div class="wtsk">${[360, 520, 280, 440].map((w) => `<div class="skel" style="width:${w}px;height:14px"></div>`).join('')}</div>`; return; }
    if (view === 'error') { work.innerHTML = `<div class="wthead"><h2>Worktrees</h2></div><div class="banner error" role="alert">${ic('err', 16)}<div style="min-width:0"><div class="b-title">Could not read the worktrees</div><div class="mono" style="font-size:var(--fs-sm)">fatal: not a git repository (or any of the parent directories): .git</div></div></div><div><button class="btn" data-d="retry">${ic('fetch', 14)}Try again</button></div>`; return; }
    const w = byId(sel) || byId(cur), stale = WT.filter((x) => x.missing).length;
    work.innerHTML = `<div class="wthead"><h2>Worktrees <span class="muted" style="font-weight:400;font-size:var(--fs-base)">${WT.length}</span></h2><span class="od-fill"></span>${stale ? `<button class="btn" data-d="prune">${ic('trash', 14)}Prune ${stale} missing</button>` : ''}</div>
      ${quickHTML()}
      ${WT.length === 1 ? `<div class="banner info" role="status">${ic('info', 16)}<div><div class="b-title">This is your only worktree</div><div>A worktree is a second working folder for this repository. Start a hotfix or a review without stashing — type a branch name above.</div></div></div>` : ''}
      ${detailHTML(w)}
      <div class="kbhint"><span><span class="kbd">Ctrl+Alt+W</span> switcher</span><span><span class="kbd">Alt+1–9</span> jump in the switcher</span><span><span class="kbd">Enter</span> switch in the sidebar</span><span><span class="kbd">Del</span> remove</span></div>`;
    if (keep != null) { const i = work.querySelector('#quick'); if (i) { i.focus(); i.setSelectionRange(keep, keep); } }
  }
  work.addEventListener('input', (e) => { if (e.target.id !== 'quick') return; quickName = e.target.value; quickTouched = true; drawCenter(); });
  work.addEventListener('keydown', (e) => { if (e.target.id === 'quick' && e.key === 'Enter') { const n = quickName.trim(); if (n && !branchErr(n) && !usedBy(n)) create(n); e.preventDefault(); } });
  work.addEventListener('click', (e) => {
    const b = e.target.closest('[data-q],[data-d]'); if (!b || b.disabled) return;
    const w = byId(sel);
    if (b.dataset.q === 'go') create(quickName);
    else if (b.dataset.q === 'dlg') addDialog();
    else if (b.dataset.q === 'chip') create(b.dataset.b);
    else if (b.dataset.q === 'goto') { sel = b.dataset.id; drawSide(); drawCenter(); }
    else if (b.dataset.d === 'switch') switchTo(w.id);
    else if (b.dataset.d === 'window') switchTo(w.id, 'window');
    else if (b.dataset.d === 'copy') copyPath(w);
    else if (b.dataset.d === 'reveal') toast('Revealed ' + w.path, 'info');
    else if (b.dataset.d === 'lock') toggleLock(w);
    else if (b.dataset.d === 'remove') removeWt(w);
    else if (b.dataset.d === 'prune') prune();
    else if (b.dataset.d === 'retry') { document.getElementById('s-state').value = 'ready'; setState('ready'); }
  });

  /* ---------- scenario switcher ---------- */
  const FULL = WT.slice();
  function setState(s) {
    view = s; closePop();
    if (s === 'empty') { WT = [FULL[0]]; cur = 'w1'; sel = 'w1'; }
    else if (s === 'ready' && WT.length === 1 && FULL.length > 1 && !WT.some((w) => w.id !== 'w1')) { WT = FULL.slice(); }
    drawBtn(); drawSide(); drawCenter();
  }
  document.getElementById('s-state').addEventListener('change', (e) => setState(e.target.value));

  G.initTips(win);
  drawBtn(); drawSide(); drawCenter();
})();
