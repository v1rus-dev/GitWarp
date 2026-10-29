/* Main window: history + commit details. Reads data-w / data-h / data-dock / data-diff from <body>. */
(function () {
  const G = window.GL, ic = G.ic, esc = G.esc;
  const W = +document.body.dataset.w, H = +document.body.dataset.h;
  let dock = document.body.dataset.dock || 'bottom';
  let mode = document.body.dataset.diff || 'unified';
  G.stagebar(`<label class="scn" style="display:inline-flex;gap:8px;align-items:center;font-size:12px">Window width <input type="range" id="ww" min="620" max="${W}" value="${W}" aria-label="Window width"> <span id="wv" class="mono">${W}px</span></label><span class="muted" style="font-size:12px;margin-left:12px">${W}×${H} · dock: ${dock}</span>`);
  const stage = G.h('<div class="stage"><div class="winwrap"></div></div>');
  document.body.appendChild(stage);
  const sh = G.shell({ host: stage.firstChild, w: W, h: H, view: 'history' });
  const win = sh.win;
  sh.center.innerHTML = `<div class="work dock-${dock}"><div class="hist-host" style="display:flex;flex:1;min-width:0;min-height:0"></div><div class="splitter ${dock === 'bottom' ? 'h' : 'v'}" role="separator" aria-label="Resize details"></div><section class="details" aria-label="Commit details"></section></div>`;
  const work = sh.center.querySelector('.work'), histHost = work.querySelector('.hist-host'), details = work.querySelector('.details'), split = work.querySelector('.splitter');

  let cur = G.commits[0], curFile = 0, collapseOn = false;

  function files(c) { const i = G.commits.indexOf(c); return G.FILES.slice(0, 2 + (i % 4)); }
  function detailsHTML() {
    const c = cur, fl = files(c);
    const par = c.parents.map((p) => { const pc = G.commits.find((x) => x.id === p); return `<a class="link" data-parent="${pc.id}" tabindex="0" role="link">${pc.hash}</a>`; }).join(' ');
    const f = fl[Math.min(curFile, fl.length - 1)];
    return `<div class="dhead"><span>Commit <span class="mono muted">${c.hash}</span></span><span class="od-fill"></span>
      <div class="seg sm ico" role="group" aria-label="Details position"><button data-dock="right" aria-pressed="${dock === 'right'}" data-tip="Dock details to the right" aria-label="Dock right">${ic('panelR', 14)}</button><button data-dock="bottom" aria-pressed="${dock === 'bottom'}" data-tip="Dock details to the bottom" aria-label="Dock bottom">${ic('panelB', 14)}</button></div></div>
    <div class="dbody">
      <div class="dinfo"><h3>${esc(c.summary)}</h3>${c.body ? `<p class="body">${esc(c.body)}</p>` : ''}
        <dl class="meta"><dt>Author</dt><dd class="who">${G.avatar(c, 20)}<span class="od-truncate">${esc(c.author)} &lt;${esc(c.email)}&gt;</span></dd><dt>Authored</dt><dd>${G.abs(c.min)} <span class="muted">(${G.rel(c.min)})</span></dd><dt>Committer</dt><dd class="who">${G.avatar(c, 20)}<span class="od-truncate">${esc(c.author)}</span></dd><dt>Hash</dt><dd class="mono" title="${c.full}">${c.full.slice(0, 20)}…</dd><dt>Parents</dt><dd>${par || '<span class="muted">none (root commit)</span>'}</dd></dl></div>
      <div class="dfiles"><div class="sub"><b style="color:var(--text)">Changed files</b><span class="ncount">${fl.length}</span><span class="od-fill"></span>${c.parents.length > 1 ? '<span title="Combined diff against first parent">vs parent 1</span>' : ''}</div>
        <div class="flist" tabindex="0" role="listbox" aria-label="Changed files">${fl.map((x, i) => { const d = x.p.lastIndexOf('/'); return `<div class="frow${i === curFile ? ' sel' : ''}" role="option" data-f="${i}" aria-selected="${i === curFile}"><span class="fs ${x.s}" aria-label="${{ A: 'Added', M: 'Modified', D: 'Deleted', R: 'Renamed' }[x.s]}">${x.s}</span><span class="fname" title="${esc(x.p)}"><bdi><span class="fdir">${esc(x.p.slice(0, d + 1))}</span>${esc(x.p.slice(d + 1))}</bdi></span><span class="fnum"><span class="p">+${x.a}</span><span class="m">−${x.d}</span></span></div>`; }).join('')}</div></div>
      <div class="ddiff"><div class="dtool"><span class="fn">${esc(f.p)}${f.from ? ` ← ${esc(f.from)}` : ''}</span>
        <div class="seg sm" role="group" aria-label="Diff mode"><button data-mode="unified" aria-pressed="${mode === 'unified'}">${ic('unified', 13)}Unified</button><button data-mode="split" aria-pressed="${mode === 'split'}">${ic('split', 13)}Side-by-side</button></div></div>
        <div class="dscroll" tabindex="0" aria-label="Diff">${G.renderDiff(G.DIFFS[f.p], mode)}</div></div>
    </div>`;
  }
  function drawDetails() { details.innerHTML = detailsHTML(); }
  details.addEventListener('click', (e) => {
    const d = e.target.closest('[data-dock]'), m = e.target.closest('[data-mode]'), f = e.target.closest('[data-f]'), p = e.target.closest('[data-parent]');
    if (d && d.dataset.dock !== dock) {
      dock = d.dataset.dock; work.className = `work dock-${dock}`; split.className = `splitter ${dock === 'bottom' ? 'h' : 'v'}`;
      details.style.width = details.style.height = ''; drawDetails();
    } else if (m) { mode = m.dataset.mode; drawDetails(); }
    else if (f) { curFile = +f.dataset.f; drawDetails(); }
    else if (p) { hist.select(G.commits.findIndex((x) => x.id === p.dataset.parent)); }
  });
  // resizer works for both docks: it reads the current axis on each drag
  split.addEventListener('mousedown', (e) => {
    e.preventDefault(); split.classList.add('drag');
    const ax = dock === 'bottom' ? 'y' : 'x', p0 = ax === 'x' ? e.clientX : e.clientY, s0 = ax === 'x' ? details.offsetWidth : details.offsetHeight, sc = G.scale(split);
    const mv = (ev) => { const dd = ((ax === 'x' ? ev.clientX : ev.clientY) - p0) / sc * -1; details.style[ax === 'x' ? 'width' : 'height'] = Math.max(220, Math.min(ax === 'x' ? 900 : 560, s0 + dd)) + 'px'; };
    const up = () => { split.classList.remove('drag'); removeEventListener('mousemove', mv); removeEventListener('mouseup', up); };
    addEventListener('mousemove', mv); addEventListener('mouseup', up);
  });

  const hist = G.history(histHost, G.commits, { gw: 112, onSelect: (c) => { cur = c; curFile = 0; drawDetails(); } });
  // history header actions: collapse idle lanes toggle lives in the graph header menu
  histHost.querySelector('.gh').addEventListener('contextmenu', (e) => G.menu(win, e, [{ label: collapseOn ? '✓ Collapse idle lanes' : 'Collapse idle lanes', icon: 'lanes', onClick: () => { collapseOn = !collapseOn; hist.setCollapse(collapseOn); } }, { label: 'Fit graph to content', icon: 'split', onClick: () => hist.setWidth(Math.min(420, hist.geo().width + 8)) }]));
  G.fit(win);
  const ww = document.getElementById('ww');
  ww.addEventListener('input', () => { win.style.width = ww.value + 'px'; document.getElementById('wv').textContent = ww.value + 'px'; win._fit(); });
})();
