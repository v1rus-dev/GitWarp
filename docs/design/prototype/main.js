/* Main window: history + commit details. Reads data-w / data-h / data-dock / data-diff from <body>.
   Interactive: right-click sidebar and history rows (real menus), double-click to check out, Settings modal (gear / Ctrl+,),
   repository and branch dropdowns, network operations in the status bar, and scenario switchers for every state. */
(function () {
  const G = window.GL, ic = G.ic, esc = G.esc;
  const W = +document.body.dataset.w, H = +document.body.dataset.h;
  let dock = document.body.dataset.dock || 'bottom';
  let mode = document.body.dataset.diff || 'unified';
  let userPicked = false; // until the user picks a dock side it follows the window: right at >=1400, bottom below

  /* ---------- stage bar + scenario bar (prototype-only controls) ---------- */
  G.stagebar(`<label class="scn" style="display:inline-flex;gap:8px;align-items:center;font-size:12px">Window width <input type="range" id="ww" min="620" max="${W}" value="${W}" aria-label="Window width"> <span id="wv" class="mono">${W}px</span></label><span class="muted" style="font-size:12px;margin-left:12px">${W}×${H}</span>`);
  const sel = (id, label, opts) => `<label>${label} <select id="${id}" aria-label="${label}">${opts.map(([v, l]) => `<option value="${v}">${l}</option>`).join('')}</select></label>`;
  const bar = G.h(`<div class="scenbar" role="toolbar" aria-label="Scenario">
    <span class="grp"><b>State</b>${sel('s-hist', 'History', [['ok', 'Loaded'], ['loading', 'Loading (skeleton)'], ['loading-rows', 'Loading (rows + bar)'], ['empty', 'Empty repository'], ['notgit', 'Not a git repository'], ['nofolder', 'Folder not found'], ['cancelled', 'Loading cancelled'], ['strip-cancelled', 'Cancelled, with rows'], ['strip-failed', 'Failed, with rows (git)'], ['strip-failed-msg', 'Failed, with rows (message)'], ['giterr', 'git error'], ['nongit', 'Non-git failure'], ['nogit', 'git not found'], ['oldgit', 'git too old']])}${sel('s-det', 'Details', [['ok', 'Loaded'], ['select', 'Select a commit'], ['loading', 'Loading commit'], ['failed', 'Commit failed'], ['nofiles', 'No files changed'], ['d-select', 'Diff: select a file'], ['d-loading', 'Diff: loading'], ['d-failed', 'Diff: failed'], ['d-large', 'Diff: too large'], ['d-binary', 'Diff: binary'], ['d-none', 'Diff: no changes']])}${sel('s-side', 'Sidebar', [['loaded', 'Loaded'], ['loading', 'Loading refs']])}</span>
    <span class="grp"><b>Toolbar</b>${sel('s-net', 'Network job', [['', 'Idle'], ['fetch', 'Fetching'], ['pull', 'Pulling'], ['push', 'Pushing'], ['tags', 'Pushing tags'], ['delbranch', 'Deleting remote branch']])}${sel('s-br', 'Branch menu', [['loaded', 'Branches'], ['loading', 'Loading branches…'], ['none', 'No local branches']])}${sel('s-dirty', 'Working tree', [['clean', 'Clean'], ['dirty', 'Dirty (6 changes)']])}</span>
    <span class="grp"><b>Appearance</b>${sel('s-date', 'Dates', [['relative', 'Relative'], ['absolute', 'Absolute (UTC)']])}${sel('s-dens', 'Row density', [['26', 'Normal 26'], ['22', 'Compact 22'], ['30', 'Roomy 30']])}${sel('s-col', 'Graph colors', [['on', 'Colored'], ['off', 'Monochrome']])}${sel('s-lw', 'Line width', [['2', '2px'], ['1', '1px'], ['3', '3px']])}${sel('s-ln', 'Diff line numbers', [['on', 'Shown'], ['off', 'Hidden']])}</span>
  </div>`);
  document.body.querySelector('.stagebar').after(bar);
  bar.insertAdjacentHTML('beforeend', `<span class="grp"><b>Branch filter</b>${sel('s-bf', 'Show commits of', [['', 'Off (all commits)'], ['main', 'main (merges)'], ['feat/palette', 'feat/palette (fork point, merged into main)'], ['feat/idle-lanes', 'feat/idle-lanes'], ['fix/git-detect', 'fix/git-detect'], ['fix/old-experiment', 'fix/old-experiment (no commits)']])}</span>`);
  const bfSel = bar.querySelector('#s-bf');
  const stage = G.h('<div class="stage"><div class="winwrap"></div></div>');
  document.body.appendChild(stage);
  document.body.appendChild(G.h(`<p class="cap-line">Right-click a sidebar row or a history row for its menu; double-click a branch, tag or commit to check it out; click a branch or tag to reveal its commit. The gear (or Ctrl+,) opens Settings as a modal card. Fetch, Pull, Push and the repository / branch switchers work; with a network job running the three buttons are disabled. Press Ctrl+J to hide the details dock, Ctrl+K to search (Enter / F3 next, Shift+Enter previous, Esc clear). The sidebar splitter drags up to half the window and resets on double-click.</p>`));

  const sh = G.shell({ host: stage.firstChild, w: W, h: H, view: 'history' });
  const win = sh.win;
  sh.center.innerHTML = `<div class="work dock-${dock}"><div class="hist-host" style="display:flex;flex-direction:column;flex:1;min-width:0;min-height:0"></div><div class="splitter ${dock === 'bottom' ? 'h' : 'v'}" role="separator" aria-label="Resize details"></div><section class="details" aria-label="Commit details"></section></div>`;
  const work = sh.center.querySelector('.work'), histHost = work.querySelector('.hist-host'), details = work.querySelector('.details'), split = work.querySelector('.splitter');

  let cur = G.commits[0], curFile = 0, hist = null, hstate = 'ok', dstate = 'ok', row = 26, mono = false, lw = 2;

  /* ---------- details dock ---------- */
  function files(c) { const i = G.commits.indexOf(c); return G.FILES.slice(0, 2 + (i % 4)); }
  const bars = (ws, cls) => `<div class="sk-bars ${cls || ''}">${ws.map((w) => `<div class="skel" style="width:${w}px"></div>`).join('')}</div>`;
  function detailsHTML() {
    const c = cur, fl = dstate === 'nofiles' ? [] : files(c), noCommit = dstate === 'select';
    const par = c.parents.map((p) => { const pc = G.commits.find((x) => x.id === p); return `<a class="link" data-parent="${pc.id}" tabindex="0" role="link">${pc.hash}</a>`; }).join(' ');
    const f = fl[Math.min(curFile, fl.length - 1)];
    const head = `<div class="dhead"><span>Commit${noCommit ? '' : ` <span class="mono muted">${c.hash}</span>`}</span><span class="od-fill"></span>
      <div class="seg sm ico" role="group" aria-label="Details position"><button data-dock="right" aria-pressed="${dock === 'right'}" data-tip="Dock details to the right" aria-label="Dock right">${ic('panelR', 14)}</button><button data-dock="bottom" aria-pressed="${dock === 'bottom'}" data-tip="Dock details to the bottom" aria-label="Dock bottom">${ic('panelB', 14)}</button></div></div>`;
    let info;
    if (noCommit) info = '<div class="dinfo"><div class="dstate">Select a commit</div></div>';
    else if (dstate === 'loading') info = `<div class="dinfo dstate top pad18">${bars([190, 240, 150])}</div>`;
    else if (dstate === 'failed') info = `<div class="dinfo"><div class="dstate top" style="padding:0;gap:4px"><b>Could not load the commit</b><span class="errt">fatal: bad object ${c.full}</span></div></div>`;
    else info = `<div class="dinfo"><h3>${esc(c.summary)}</h3>${c.body ? `<p class="body">${esc(c.body)}</p>` : ''}
        <dl class="meta"><dt>Author</dt><dd class="who">${G.avatar(c, 20)}<span class="od-truncate">${esc(c.author)} &lt;${esc(c.email)}&gt;</span></dd><dt>Authored</dt><dd>${G.abs(c.min)} <span class="muted">(${G.rel(c.min)})</span></dd><dt>Committer</dt><dd class="who">${G.avatar(c, 20)}<span class="od-truncate">${esc(c.author)}</span></dd><dt>Hash</dt><dd class="mono" data-tip="${c.full}">${c.full.slice(0, 20)}…</dd><dt>Parents</dt><dd>${par || '<span class="muted">none (root commit)</span>'}</dd></dl></div>`;
    const sub = `<div class="sub"><b style="color:var(--text)">Changed files</b>${noCommit || dstate === 'loading' || dstate === 'failed' ? '' : `<span class="ncount">${fl.length}</span>`}<span class="od-fill"></span>${!noCommit && c.parents.length > 1 ? '<span data-tip="Combined diff against first parent">vs parent 1</span>' : ''}</div>`;
    let flist;
    if (noCommit) flist = '<div class="flist"></div>';
    else if (dstate === 'loading') flist = `<div class="dstate top" style="padding:8px">${bars([180, 180, 180, 180], 'tight')}</div>`;
    else if (dstate === 'failed') flist = '<div class="dstate top" style="padding:12px"><span class="errt">fatal: bad object ' + c.hash + '</span></div>';
    else if (!fl.length) flist = '<div class="dstate">No files changed</div>';
    else flist = `<div class="flist" tabindex="0" role="listbox" aria-label="Changed files">${fl.map((x, i) => { const d = x.p.lastIndexOf('/'); return `<div class="frow${i === curFile ? ' sel' : ''}" role="option" data-f="${i}" aria-selected="${i === curFile}"><span class="fs ${x.s}" aria-label="${{ A: 'Added', M: 'Modified', D: 'Deleted', R: 'Renamed' }[x.s]}">${x.s}</span><span class="fname" data-tip="${esc(x.p)}"><bdi><span class="fdir">${esc(x.p.slice(0, d + 1))}</span>${esc(x.p.slice(d + 1))}</bdi></span><span class="fnum"><span class="p">+${x.a}</span><span class="m">−${x.d}</span></span></div>`; }).join('')}</div>`;
    const ok = !noCommit && !['loading', 'failed', 'nofiles', 'd-select', 'd-loading', 'd-failed', 'd-large', 'd-binary', 'd-none'].includes(dstate) && f;
    let dview;
    if (ok) dview = `<div class="dscroll" tabindex="0" aria-label="Diff">${G.renderDiff(G.DIFFS[f.p], mode)}</div>`;
    else if (dstate === 'd-loading' || dstate === 'loading') dview = `<div class="dstate top" style="padding:12px">${bars([320, 260, 290])}</div>`;
    else if (dstate === 'd-failed') dview = '<div class="dstate top" style="padding:12px"><span class="errt">error: could not read the blob 3f5e1a9:src/graph/layout.rs</span></div>';
    else dview = `<div class="dstate">${{ 'd-large': 'Diff too large to display', 'd-binary': 'Binary file', 'd-none': 'No changes' }[dstate] || 'Select a file'}</div>`;
    const fname = ok || dstate === 'd-large' || dstate === 'd-binary' || dstate === 'd-none' || dstate === 'd-failed' ? (f || files(c)[0]) : null;
    const dtool = `<div class="dtool"><span class="fn">${fname ? `${esc(fname.p)}${fname.from ? ` ← ${esc(fname.from)}` : ''}` : ''}</span>
        <div class="seg sm" role="group" aria-label="Diff mode"><button data-mode="unified" aria-pressed="${mode === 'unified'}">Unified</button><button data-mode="split" aria-pressed="${mode === 'split'}">Side-by-side</button></div></div>`;
    return `${head}<div class="dbody">${info}<div class="dfiles">${sub}${flist}</div><div class="ddiff">${dtool}${dview}</div></div>`;
  }
  function drawDetails() { details.innerHTML = detailsHTML(); }
  const DOCK = { bottom: { def: 340, min: 220, max: () => 560 }, right: { def: 640, min: 220, max: () => Math.min(900, (win.offsetWidth - win.querySelector('.sidebar').offsetWidth) / 2) } };
  function setDock(d) {
    dock = d; work.className = `work dock-${dock}`; split.className = `splitter ${dock === 'bottom' ? 'h' : 'v'}`;
    details.style.width = details.style.height = ''; drawDetails();
  }
  details.addEventListener('click', (e) => {
    const d = e.target.closest('[data-dock]'), m = e.target.closest('[data-mode]'), f = e.target.closest('[data-f]'), p = e.target.closest('[data-parent]');
    if (d && d.dataset.dock !== dock) { userPicked = true; setDock(d.dataset.dock); }
    else if (m) { mode = m.dataset.mode; drawDetails(); }
    else if (f) { curFile = +f.dataset.f; drawDetails(); }
    else if (p) { hist.selectCommit(G.commits.find((x) => x.id === p.dataset.parent)); }
  });
  /* splitter: bottom 220-560 (default 340), right 220-900 (default 640, at most half of the space right of the sidebar); double-click resets */
  split.addEventListener('mousedown', (e) => {
    e.preventDefault(); split.classList.add('drag');
    const ax = dock === 'bottom' ? 'y' : 'x', p0 = ax === 'x' ? e.clientX : e.clientY, s0 = ax === 'x' ? details.offsetWidth : details.offsetHeight, sc = G.scale(split), L = DOCK[dock];
    const mv = (ev) => { const dd = ((ax === 'x' ? ev.clientX : ev.clientY) - p0) / sc * -1; details.style[ax === 'x' ? 'width' : 'height'] = Math.max(L.min, Math.min(L.max(), s0 + dd)) + 'px'; };
    const up = () => { split.classList.remove('drag'); removeEventListener('mousemove', mv); removeEventListener('mouseup', up); };
    addEventListener('mousemove', mv); addEventListener('mouseup', up);
  });
  split.addEventListener('dblclick', () => { details.style.width = details.style.height = ''; });
  /* Ctrl+J hides or shows the whole details dock */
  addEventListener('keydown', (e) => { if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'j') { e.preventDefault(); details.hidden = split.hidden = !details.hidden; } });

  /* ---------- history center: loaded list, in-list strips and full-pane states ---------- */
  const hasRows = (k) => ['ok', 'loading-rows', 'strip-cancelled', 'strip-failed', 'strip-failed-msg'].includes(k);
  function mountHist() {
    const keep = cur;
    histHost.className = 'hist-host';
    hist = G.history(histHost, G.commits, { gw: 112, sel: Math.max(0, G.commits.indexOf(keep)), onSelect: (c) => { cur = c; curFile = 0; drawDetails(); } });
    hist.setRow(row); hist.setMono(mono); hist.setLine(lw);
    if (bfSel.value) hist.setBranch(bfSel.value);
    if (sh.search.value) searchRun();
  }
  function setHist(k) {
    hstate = k;
    if (hasRows(k)) {
      if (!histHost.querySelector('.lbody')) mountHist();
      hist.strip(k === 'ok' ? '' : G.stateStrip(k === 'loading-rows' ? 'loading' : k));
    } else {
      hist = null; histHost.className = 'hist-host hist';
      histHost.innerHTML = `<div class="lhead" role="row"><div class="gh"><span>Graph</span></div><div>Summary</div><div>Author</div><div>Date</div><div>Hash</div></div>${G.stateStrip(k === 'loading' ? 'loading' : '')}<div class="hs-body"><div style="flex:1;min-height:0;overflow:auto">${G.stateBody(k)}</div></div>`;
      if (k !== 'loading') histHost.querySelector('.hs-body').style.display = 'flex';
    }
    const ss = G.stateStatus[k === 'loading-rows' ? 'loading' : k];
    sh.setStatus({ left: ss || null, info: k === 'ok' || k.startsWith('strip') ? '21 commits loaded' : k === 'loading' || k === 'loading-rows' ? '12 400 commits loaded' : '', git: k === 'nogit' ? 'none' : k === 'oldgit' ? 'old' : 'ok' });
    if (dstate === 'ok' && !hasRows(k)) { /* keep the dock as is */ }
    sh.setSearchStat('');
  }
  histHost.addEventListener('click', (e) => {
    const b = e.target.closest('[data-act]'); if (!b) return;
    ({
      retry: () => { document.getElementById('s-hist').value = 'ok'; setHist('ok'); G.toast(win, 'History loaded', 'ok'); },
      cancel: () => { document.getElementById('s-hist').value = 'strip-cancelled'; setHist('strip-cancelled'); },
      'git-settings': () => sh.openSettings('Git'),
      changes: () => { location.href = 'changes.html'; },
      gitignore: () => G.toast(win, 'Added a .gitignore', 'ok'),
      init: () => { location.href = 'welcome.html'; },
      open: () => window.GLD && window.GLD.openIn('open', win),
      'remove-recent': () => { location.href = 'welcome.html'; },
      copy: () => G.toast(win, 'Details copied', 'info')
    })[b.dataset.act]();
  });

  /* ---------- toolbar search: counter in the field, keys, highlight in rows ---------- */
  function searchStat() {
    const q = sh.search.value.trim(); if (!q) return '';
    if (!hist) return '';
    const s = hist.stat(), scanning = hstate === 'loading-rows';
    if (!s.total) return scanning ? '– / 0+' : 'No matches';
    return `${s.pos || 1} / ${s.total}${scanning ? '+' : ''}`;
  }
  function searchRun() { if (hist) hist.search(sh.search.value); sh.setSearchStat(searchStat()); }
  sh.search.addEventListener('input', searchRun);
  sh.search.addEventListener('keydown', (e) => {
    if (!hist) return;
    if (e.key === 'Enter' || e.key === 'F3') { hist.step(e.shiftKey ? -1 : 1); sh.setSearchStat(searchStat()); e.preventDefault(); }
    else if (e.key === 'Escape') { sh.search.value = ''; hist.search(''); sh.setSearchStat(''); }
    else if (e.key === 'ArrowDown') { histHost.querySelector('.lbody').focus(); e.preventDefault(); }
  });
  addEventListener('keydown', (e) => { if (e.key === 'F3' && document.activeElement !== sh.search && hist && sh.search.value) { hist.step(e.shiftKey ? -1 : 1); sh.setSearchStat(searchStat()); e.preventDefault(); } });

  /* click a sidebar leaf: reveal its commit in the history */
  sh.onReveal = (kind, name) => {
    if (!hist) return;
    const t = { branch: ['br', 'head'], rbranch: ['rm'], tag: ['tag'] }[kind]; if (!t) return;
    const c = G.commits.find((c) => c.refs.some((r) => t.includes(r.t) && r.n === name));
    if (c) hist.selectCommit(c);
  };
  document.addEventListener('gl-head', () => { if (hist) hist.refresh(); });

  /* ---------- scenario controls ---------- */
  const $ = (id) => document.getElementById(id);
  bfSel.onchange = (e) => { if (hist) hist.setBranch(e.target.value || null); };
  $('s-hist').onchange = (e) => setHist(e.target.value);
  $('s-det').onchange = (e) => { dstate = e.target.value; curFile = 0; drawDetails(); };
  $('s-side').onchange = (e) => sh.setSidebar(e.target.value);
  $('s-net').onchange = (e) => { if (e.target.value) { sh.net.clear(); sh.net.set(e.target.value, 'Receiving objects: 38% (13/34)', e.target.value === 'push' ? null : 0.38); } else sh.net.clear(); };
  $('s-br').onchange = (e) => { G.state.branches = e.target.value; };
  $('s-dirty').onchange = (e) => { G.state.dirty = e.target.value === 'dirty'; };
  $('s-date').onchange = (e) => { G.dateMode = e.target.value; if (hist) hist.refresh(); };
  $('s-dens').onchange = (e) => { row = +e.target.value; win.style.setProperty('--row', row + 'px'); if (hist) hist.setRow(row); };
  $('s-col').onchange = (e) => { mono = e.target.value === 'off'; if (hist) hist.setMono(mono); };
  $('s-lw').onchange = (e) => { lw = +e.target.value; if (hist) hist.setLine(lw); };
  $('s-ln').onchange = (e) => { details.classList.toggle('nonum', e.target.value === 'off'); };

  setHist('ok');
  drawDetails();
  G.fit(win);
  const ww = $('ww');
  ww.addEventListener('input', () => {
    win.style.width = ww.value + 'px'; $('wv').textContent = ww.value + 'px';
    if (!userPicked) { const d = +ww.value >= 1400 ? 'right' : 'bottom'; if (d !== dock) setDock(d); }
    win._fit();
  });
})();
