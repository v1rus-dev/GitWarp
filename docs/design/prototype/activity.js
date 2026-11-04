/* GitWarp — commit activity heatmap prototype. Two placements (A: Welcome, B: repository window) × states.
   Behaviour here is the spec for the GPUI implementation; see the captions on activity.html. */
(function () {
  'use strict';
  const G = window.GL, ic = G.ic, esc = G.esc;
  const DAY = 86400000;
  const NOW = Date.UTC(2026, 9, 29, 14, 20);             // same "now" as app.js sample data
  const TODAY = Math.floor(NOW / DAY) * DAY;              // prototype buckets by UTC day; the app uses the local day
  const MON = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  const WD = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  const ME = 'mira.okonkwo@example.dev';
  const fmtN = (n) => n.toLocaleString('en-US');
  const dow = (t) => (new Date(t).getUTCDay() + 6) % 7;   // 0 = Monday (week start follows the locale; Monday here)
  const dateLong = (t) => { const d = new Date(t); return `${WD[dow(t)]}, ${d.getUTCDate()} ${MON[d.getUTCMonth()]} ${d.getUTCFullYear()}`; };
  const key = (t) => new Date(t).toISOString().slice(0, 10);

  /* ---------- range: 52 full weeks + the current partial week (53 columns) ---------- */
  function range(year) {
    let start, end;
    if (year) { end = Math.min(TODAY, Date.UTC(year, 11, 31)); start = Date.UTC(year, 0, 1); start -= dow(start) * DAY; }
    else { end = TODAY; start = TODAY - dow(TODAY) * DAY - 52 * 7 * DAY; }
    const first = year ? Date.UTC(year, 0, 1) : start;          // days before `first` are drawn blank (year mode only)
    const last = year ? Date.UTC(year, 11, 31) : end;
    const cols = Math.floor((Math.max(end, last) - start) / DAY / 7) + 1;
    return { start, end, first, cols };
  }

  /* ---------- sample data ---------- */
  function rng(seed) { let s = seed >>> 0; return () => { s = (s * 1664525 + 1013904223) >>> 0; return s / 4294967296; }; }
  const sample = {};                                           // real sample commits from app.js → exact counts for the last 6 weeks
  G.commits.forEach((c) => { const k = G.abs(c.min).slice(0, 10); (sample[k] = sample[k] || []).push(c); });
  const SAMPLE_FROM = TODAY - 4 * DAY;                         // the last 5 days use the real sample commits exactly
  function series(seed, scale, who) {
    const r = rng(seed), out = {};
    let vac = 0;
    for (let t = TODAY - 400 * DAY; t <= TODAY; t += DAY) {
      if (t >= SAMPLE_FROM && seed === 7) { const s = sample[key(t)] || []; out[key(t)] = who === 'me' ? s.filter((c) => c.email === ME).length : s.length; continue; }
      if (vac > 0) { vac--; out[key(t)] = 0; continue; }
      if (r() < 0.012) { vac = 5 + Math.floor(r() * 9); }
      const wk = dow(t) >= 5 ? 0.25 : 1, season = 0.7 + 0.5 * Math.sin((t / DAY) / 58);
      const p = r();
      out[key(t)] = p < 0.28 * (2 - wk) ? 0 : Math.round(Math.pow(r(), 1.8) * 16 * scale * wk * season);
    }
    return out;
  }
  const DATA = {
    repo: { everyone: series(7, 1.6, 'all'), me: series(7, 0.6, 'me') },
    recent: series(11, 1.1, 'me'),
    perRepo: { gitwarp: series(7, 0.6, 'me'), 'open-design': series(21, 0.5, 'me'), 'zed-themes': series(33, 0.15, 'me'), dotfiles: series(41, 0.1, 'me'), 'tree-sitter-rust': series(55, 0.08, 'me') }
  };

  /* ---------- buckets: 0 + 4 levels by quartiles of the non-zero days in view ---------- */
  function buckets(values) {
    const nz = values.filter((v) => v > 0).sort((a, b) => a - b);
    if (!nz.length) return { level: () => 0, ranges: [] };
    const max = nz[nz.length - 1], distinct = new Set(nz).size;
    if (distinct <= 4) {                                       // few distinct values: scale linearly so 1 is never "hot" by accident
      const level = (v) => (v <= 0 ? 0 : Math.max(1, Math.ceil((4 * v) / max)));
      const ranges = [1, 2, 3, 4].map((l) => { const vs = nz.filter((v) => level(v) === l); return vs.length ? [vs[0], vs[vs.length - 1]] : null; });
      return { level, ranges };
    }
    const q = (p) => nz[Math.min(nz.length - 1, Math.ceil(p * nz.length) - 1)];
    const t = [q(0.25), q(0.5), q(0.75)];
    const level = (v) => (v <= 0 ? 0 : v <= t[0] ? 1 : v <= t[1] ? 2 : v <= t[2] ? 3 : 4);
    const ranges = [[1, t[0]], [t[0] + 1, t[1]], [t[1] + 1, t[2]], [t[2] + 1, max]].map((x) => (x[0] > x[1] ? null : x));
    return { level, ranges };
  }

  /* ---------- heatmap renderer ---------- */
  // opts: { data, year, compact, state: 'ok'|'loading'|'zero', pendingFrom (time), selected, label }
  function heatHTML(o) {
    const R = range(o.year), cells = [], vals = [];
    for (let t = R.start; t <= R.end; t += DAY) if (t >= R.first) vals.push(o.data[key(t)] || 0);
    const B = buckets(o.state === 'zero' ? [] : vals);
    // month labels: at the first column whose Monday-week contains the 1st; dropped if < 3 columns after the previous one
    let months = '', lastCol = -9;
    for (let c = 0; c < R.cols; c++) {
      for (let d = 0; d < 7; d++) {
        const t = R.start + (c * 7 + d) * DAY, dt = new Date(t);
        if (dt.getUTCDate() === 1 && t >= R.first && t <= R.end) {
          if (c - lastCol >= 3) { months += `<span style="left:calc(${c} * var(--pitch))">${MON[dt.getUTCMonth()]}</span>`; lastCol = c; }
        }
      }
      if (c === 0 && lastCol < 0) { const dt = new Date(Math.max(R.start, R.first)); if (dt.getUTCDate() <= 21) { months += `<span style="left:0">${MON[dt.getUTCMonth()]}</span>`; lastCol = 0; } }
    }
    let rows = '';
    for (let d = 0; d < 7; d++) {
      let row = `<div role="row" aria-label="${WD[d]}">`;
      for (let c = 0; c < R.cols; c++) {
        const t = R.start + (c * 7 + d) * DAY;
        if (t > R.end || t < R.first) { row += '<div class="hc future" role="gridcell" aria-hidden="true"></div>'; continue; }
        const v = o.state === 'zero' ? 0 : o.data[key(t)] || 0;
        const lv = B.level(v), pend = o.pendingFrom != null && t < o.pendingFrom;
        const lab = o.state === 'loading' || pend ? `Counting commits on ${dateLong(t)}` : tipText(v, t);
        row += `<div class="hc${!pend && o.state !== 'loading' && lv ? ' l' + lv : ''}${pend ? ' pending' : ''}${o.selected === t ? ' sel' : ''}" role="gridcell" tabindex="-1" data-t="${t}" data-v="${v}" aria-label="${lab}"${o.selected === t ? ' aria-selected="true"' : ''}></div>`;
      }
      rows += row + '</div>';
    }
    const total = vals.reduce((a, b) => a + b, 0);
    return {
      total, ranges: B.ranges,
      html: `<div class="heat${o.compact ? ' compact' : ''}${o.state === 'loading' ? ' loading' : ''}" style="--cols:${R.cols}">
        <div class="heat-months" aria-hidden="true" style="width:calc(${R.cols} * var(--pitch))">${months}</div>
        <div class="heat-wd" aria-hidden="true">${WD.map((w, i) => `<span>${i % 2 === 0 && i < 6 ? w : ''}</span>`).join('')}</div>
        <div class="heat-grid" role="grid" tabindex="0" aria-label="${esc(o.label || 'Commit activity')}" aria-readonly="true">${rows}</div></div>`
    };
  }
  function tipText(v, t) { return `${v === 0 ? 'No' : fmtN(v)} commit${v === 1 ? '' : 's'} on ${dateLong(t)}`; }
  function legendHTML(ranges) {
    const tip = (l) => { if (l === 0) return 'No commits'; const r = ranges[l - 1]; return r ? (r[0] === r[1] ? `${r[0]} commit${r[0] === 1 ? '' : 's'} a day` : `${r[0]}–${r[1]} commits a day`) : 'Not used at this scale'; };
    return `<span class="heat-legend" aria-label="Colour scale: less to more commits"><span class="t" style="margin-left:0">Less</span>${[0, 1, 2, 3, 4].map((l) => `<span class="hc${l ? ' l' + l : ''}" data-tip="${tip(l)}"></span>`).join('')}<span class="t" style="margin-right:0">More</span></span>`;
  }

  /* ---------- interaction: instant anchored tooltip, roving focus, arrows, click ---------- */
  function wire(win, root, onPick) {
    const layer = G.layer(win); let tip = null;
    const hide = () => { if (tip) { tip.remove(); tip = null; } };
    const show = (cell) => {
      hide(); if (!cell || cell.classList.contains('future')) return;
      tip = G.h(`<div class="tip" role="tooltip">${esc(cell.getAttribute('aria-label'))}</div>`);
      layer.appendChild(tip);
      const s = G.scale(win), wr = win.getBoundingClientRect(), cr = cell.getBoundingClientRect();
      const x = (cr.left - wr.left + cr.width / 2) / s, y = (cr.top - wr.top) / s;
      tip.style.left = Math.max(4, Math.min(win.offsetWidth - tip.offsetWidth - 4, x - tip.offsetWidth / 2)) + 'px';
      tip.style.top = (y - tip.offsetHeight - 6) + 'px';
    };
    root.addEventListener('mouseover', (e) => { const c = e.target.closest('.hc[data-t], .heat-legend .hc'); if (c) { if (c.dataset.tip) { c.setAttribute('aria-label', c.dataset.tip); } show(c); } });
    root.addEventListener('mouseout', (e) => { if (e.target.closest('.hc')) hide(); });
    root.addEventListener('focusout', hide);
    root.addEventListener('focusin', (e) => {
      const g = e.target.closest('.heat-grid'); if (!g) return;
      let c = e.target.closest('.hc[data-t]');
      if (!c) { c = g.querySelector('.hc.sel') || g.querySelector('.hc.kf') || g.querySelector(`.hc[data-t="${TODAY}"]`) || [...g.querySelectorAll('.hc[data-t]')].pop(); }
      move(g, c);
    });
    function move(g, c) {
      if (!c) return;
      g.querySelectorAll('.hc.kf').forEach((x) => x.classList.remove('kf'));
      c.classList.add('kf'); g.setAttribute('aria-activedescendant', ''); show(c);
    }
    root.addEventListener('keydown', (e) => {
      const g = e.target.closest('.heat-grid'); if (!g) return;
      const cur = g.querySelector('.hc.kf'); if (!cur) return;
      const t = +cur.dataset.t;
      const dlt = { ArrowUp: -1, ArrowDown: 1, ArrowLeft: -7, ArrowRight: 7 }[e.key];
      let nt = null;
      if (dlt) nt = t + dlt * DAY;
      else if (e.key === 'Home') nt = +g.querySelector('.hc[data-t]').dataset.t;
      else if (e.key === 'End') nt = +[...g.querySelectorAll('.hc[data-t]')].pop().dataset.t;
      else if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onPick(cur, e); return; }
      if (nt != null) { e.preventDefault(); const n = g.querySelector(`.hc[data-t="${nt}"]`); if (n) move(g, n); }
    });
    root.addEventListener('click', (e) => { const c = e.target.closest('.heat-grid .hc[data-t]'); if (c) { c.closest('.heat-grid').focus({ preventScroll: true }); move(c.closest('.heat-grid'), c); onPick(c, e); } });
    return { hide };
  }

  /* ---------- synthetic commits for a filtered day (prototype only; the app filters the real history) ---------- */
  const POOL = ['fix(history): keep selection when the filter changes', 'feat(activity): heatmap above history', 'refactor(git): share the log parser', 'docs: update keyboard map', 'test(graph): lane colours stay stable', 'chore: bump gpui', 'fix(diff): wrap long lines in split mode', 'feat(sidebar): show ahead/behind on remotes', 'perf(log): stream in 512-commit pages', 'fix(staging): CRLF hunks', 'feat(palette): recent commands first', 'style: rustfmt', 'fix(settings): validate identity email', 'feat(welcome): remove missing repos', 'refactor(ui): split Tree rows'];
  const AUS = [['Mira Okonkwo', ME], ['Tomás Reyes', 'tomas.reyes@example.dev'], ['Lin Zhao', 'lin.zhao@example.dev'], ['Priya Natarajan', 'priya.n@example.dev'], ['Jonas Berg', 'jonas.berg@example.dev']];
  function commitsOn(t, n, onlyMe) {
    const real = (sample[key(t)] || []).filter((c) => !onlyMe || c.email === ME);
    if (real.length && real.length === n) { const ids = new Set(real.map((c) => c.id)); return real.map((c) => ({ ...c, parents: c.parents.filter((p) => ids.has(p)), refs: c.refs.filter((r) => r.t !== 'head') })); }
    const r = rng(t / DAY);
    return Array.from({ length: n }, (_, i) => {
      const a = onlyMe ? AUS[0] : AUS[Math.floor(r() * AUS.length)], h = Math.floor(r() * 0xfffffff).toString(16).padStart(7, '0');
      const min = Math.round((NOW - t) / 60000 - 9 * 60 - (n - i) * 37);
      return { id: 'd' + i, hash: h, parents: i < n - 1 ? ['d' + (i + 1)] : [], summary: POOL[Math.floor(r() * POOL.length)], author: a[0], email: a[1], avatar: '', min, body: '', refs: [], full: (h + '9c1e7a40b2d5f8036ae1c94d7b52e08f1a35c7').slice(0, 40) };
    });
  }

  /* ---------- page state ---------- */
  const S = { variant: 'B', state: 'ok', who: 'me', year: 0, collapsed: false, day: null, scope: 'all', sel: 0 };
  // deep links for review/screenshots: ?theme=light&v=A&s=loading&day=2026-10-14
  const Q = new URLSearchParams(location.search);
  if (G.THEMES[Q.get('theme')]) { G.setTheme(Q.get('theme')); const b = document.getElementById('gl-theme'); if (b) b.textContent = 'Theme: ' + G.THEMES[G.theme]; }
  if (Q.get('v') === 'A' || Q.get('v') === 'B') S.variant = Q.get('v');
  const STATES = [['ok', 'Populated'], ['loading', 'Loading'], ['large', 'Large repo'], ['empty', 'Empty repo'], ['nomine', 'No commits by me'], ['noemail', 'No user.email'], ['error', 'Error']];
  const pageHost = document.getElementById('act-stage');
  const ctrl = document.getElementById('act-ctrls');
  if (STATES.some(([k]) => k === Q.get('s'))) { S.state = Q.get('s'); if (S.state === 'noemail') S.who = 'everyone'; }
  if (Q.get('day')) S.day = Date.parse(Q.get('day') + 'T00:00:00Z');
  if (Q.get('who') === 'everyone') S.who = 'everyone';
  if (Q.get('collapsed')) S.collapsed = true;
  ctrl.innerHTML = `<span><span class="lab">Placement</span><span class="seg" role="group" aria-label="Placement"><button data-v="A" aria-pressed="${S.variant === 'A'}">A · Welcome</button><button data-v="B" aria-pressed="${S.variant === 'B'}">B · Repository window (recommended)</button></span></span>
    <span><span class="lab">State</span><span class="seg sm" role="group" aria-label="State">${STATES.map(([k, l]) => `<button data-s="${k}" aria-pressed="${k === S.state}">${l}</button>`).join('')}</span></span>`;
  ctrl.addEventListener('click', (e) => {
    const v = e.target.closest('[data-v]'), s = e.target.closest('[data-s]');
    if (v) { S.variant = v.dataset.v; S.day = null; }
    if (s) { S.state = s.dataset.s; S.day = null; S.who = s.dataset.s === 'noemail' ? 'everyone' : 'me'; }
    ctrl.querySelectorAll('[data-v]').forEach((b) => b.setAttribute('aria-pressed', b.dataset.v === S.variant));
    ctrl.querySelectorAll('[data-s]').forEach((b) => b.setAttribute('aria-pressed', b.dataset.s === S.state));
    if (v || s) render();
  });

  let timer = null;
  function render() {
    clearInterval(timer); timer = null;
    pageHost.innerHTML = '<div class="winwrap"></div>';
    document.getElementById('act-cap').innerHTML = S.variant === 'A' ? CAP_A : CAP_B;
    (S.variant === 'A' ? variantA : variantB)(pageHost.firstChild);
    /* Settings → Appearance → "Show activity statistics" (stored by settings.js as gl-set-show-activity; default on).
       Off hides the Welcome card / History strip; the data and the Placement/State controls of this page stay. */
    let on = true; try { on = JSON.parse(localStorage.getItem('gl-set-show-activity') ?? 'true') !== false; } catch (e) {}
    if (!on) pageHost.querySelectorAll('.act-strip, .wact-sec, .dayfilter').forEach((n) => { n.hidden = true; n.style.display = 'none'; });
  }
  addEventListener('storage', (e) => { if (e.key === 'gl-set-show-activity') render(); });

  /* ---------- the strip/card body shared by A and B per state ---------- */
  function body(o) {
    // returns { html, total, ranges, head } for the given state
    const st = S.state;
    if (st === 'error') return { html: `<div class="heat-msg err" style="min-height:${o.compact ? 84 : 98}px">${ic('err', 16)}<div><div style="color:var(--text);font-weight:600">Could not count commits</div><div><span class="mono">git log</span> failed: <span class="mono">fatal: bad object refs/heads/feat/palette</span></div></div><button class="btn btn-sm" data-retry>${ic('fetch', 13)}Retry</button></div>`, head: '<span class="heat-total">Activity unavailable</span>' };
    if (st === 'loading') { const h = heatHTML({ data: {}, compact: o.compact, state: 'loading', label: o.label }); return { html: h.html, head: '<span class="skel" style="width:180px;display:inline-block"></span>', ranges: [] }; }
    const zero = st === 'empty' || (st === 'nomine' && S.who === 'me');
    const data = o.data;
    const h = heatHTML({ data, year: S.year, compact: o.compact, state: zero ? 'zero' : 'ok', selected: S.day, label: o.label, pendingFrom: o.pendingFrom });
    let head;
    const when = S.year ? `in ${S.year}` : 'in the last year';
    if (st === 'empty') head = '<span class="heat-total">No commits yet</span>';
    else if (zero) head = `<span class="heat-total">No commits by you ${when}</span>`;
    else if (o.pendingFrom != null) head = `<span class="heat-total" style="display:inline-flex;gap:6px;align-items:center">${ic('fetch', 12, 'spin')}Counting… <b>${fmtN(o.partial)}</b> commits so far</span>`;
    else head = `<span class="heat-total"><b>${fmtN(h.total)}</b> ${o.mine ? (h.total === 1 ? 'commit by you' : 'commits by you') : h.total === 1 ? 'commit' : 'commits'} ${when}</span>`;
    return { html: h.html, head, ranges: h.ranges, total: h.total, zero };
  }

  /* ================= Variant B: repository window ================= */
  function variantB(wrap) {
    const sh = G.shell({ host: wrap, w: 1200, h: 800, view: 'history', statusInfo: S.state === 'large' ? '48 210 commits loaded' : '21 commits loaded' });
    const win = sh.win;
    sh.center.innerHTML = `<div class="work dock-bottom"><div class="histcol"><section class="act-strip${S.collapsed ? ' collapsed' : ''}" aria-label="Commit activity"></section><div class="dayfilter" hidden></div><div class="hist-host" style="display:flex;flex:1;min-width:0;min-height:0"></div></div><div class="splitter h" role="separator" aria-label="Resize details"></div><section class="details" aria-label="Commit details" style="height:300px"></section></div>`;
    const strip = sh.center.querySelector('.act-strip'), filt = sh.center.querySelector('.dayfilter'), histHost = sh.center.querySelector('.hist-host'), details = sh.center.querySelector('.details');
    let hist;
    const dayCount = (t) => (S.who === 'me' ? DATA.repo.me : DATA.repo.everyone)[key(t)] || 0;
    if (S.day != null) { while (S.day > TODAY - 360 * DAY && !dayCount(S.day)) S.day -= DAY; if (!dayCount(S.day)) S.day = null; }  // deep link to an empty day → nearest earlier day with commits
    const showDetails = (c) => { details.innerHTML = `<div class="dhead"><span>Commit <span class="mono muted">${c.hash}</span></span></div><div class="dinfo" style="padding:16px 18px"><h3 style="margin:0 0 8px">${esc(c.summary)}</h3><dl class="meta"><dt>Author</dt><dd class="who">${G.avatar(c, 20)}<span class="od-truncate">${esc(c.author)} &lt;${esc(c.email)}&gt;</span></dd><dt>Authored</dt><dd>${G.abs(c.min)} <span class="muted">(${G.rel(c.min)})</span></dd><dt>Hash</dt><dd class="mono">${c.full.slice(0, 20)}…</dd></dl></div>`; };
    function drawHistory() {
      if (S.state === 'empty') { histHost.className = 'hist-host'; histHost.innerHTML = `<div class="empty"><div class="ei">${ic('git', 22)}</div><h4>No commits yet</h4><p>This repository is empty. Stage some files and make your first commit to see history here.</p></div>`; details.innerHTML = '<div class="empty"><p>Select a commit</p></div>'; return; }
      const list = S.day != null ? commitsOn(S.day, dayCount(S.day), S.who === 'me') : G.commits;
      hist = G.history(histHost, list, { gw: 112, onSelect: showDetails });
    }
    let pendingFrom = null, partial = 0;
    function drawStrip() {
      const R = range(S.year);
      const data = S.who === 'me' ? DATA.repo.me : DATA.repo.everyone;
      const b = body({ data, mine: S.who === 'me', pendingFrom, partial, label: `Commit activity, ${S.who === 'me' ? 'your commits' : 'all authors'}, ${S.year || 'last 12 months'}` });
      const meDisabled = S.state === 'noemail';
      const years = [2026, 2025, 2024, 2023];
      strip.innerHTML = `<div class="act-head">
          <button class="btn btn-icon btn-ghost btn-xs" data-toggle aria-expanded="${!S.collapsed}" aria-label="${S.collapsed ? 'Expand' : 'Collapse'} activity" data-tip="${S.collapsed ? 'Expand' : 'Collapse'} activity">${ic('chevD', 12, 'chev')}</button>
          <span class="ttl">Activity</span>${b.head}
          <span class="od-fill"></span>
          ${S.collapsed || S.state === 'error' || S.state === 'empty' ? '' : legendHTML(b.ranges || [])}
          <span class="seg sm" role="group" aria-label="Author"><button data-who="me" aria-pressed="${S.who === 'me'}"${meDisabled ? ' disabled style="opacity:.45" data-tip="Set user.email to see your own commits"' : ` data-tip="Commits where author email = ${ME}"`}>${ic('user', 12)}Me</button><button data-who="everyone" aria-pressed="${S.who === 'everyone'}">Everyone</button></span>
          <button class="select sm" data-range aria-haspopup="menu">${S.year ? S.year : 'Last 12 months'}${ic('chevD', 12)}</button>
        </div>
        <div class="act-body"><div class="heat-scroll">${b.html}</div>
          <div class="act-side">${sideHTML(b, meDisabled)}</div></div>`;
      strip.dataset.years = years.join(',');
    }
    function sideHTML(b, meDisabled) {
      if (S.state === 'error' || S.state === 'loading') return S.state === 'loading' ? '<span class="skel" style="width:140px"></span><span class="skel" style="width:110px"></span>' : '';
      if (meDisabled) return `<span>Showing everyone: <span class="mono">user.email</span> is not set for this repository.</span><a href="settings-identity.html">Set identity…</a>`;
      if (S.state === 'nomine') return `<span>No commits by <span class="mono">${ME}</span>.</span><a href="#" data-who="everyone" role="button">Show everyone’s commits</a>`;
      if (S.state === 'empty') return '<span>Make the first commit to start the graph.</span>';
      if (S.day != null) { const v = dayCount(S.day); return `<span class="kv"><b>${dateLong(S.day)}</b></span><span>${v} commit${v === 1 ? '' : 's'} · History is filtered to this day.</span>`; }
      if (b.pendingFrom != null || pendingFrom != null) return '<span>Older weeks fill in as <span class="mono">git log</span> streams; colours settle when counting ends.</span>';
      return '<span>Click a day to show its commits in History.</span><span><span class="kbd">↑</span> <span class="kbd">↓</span> day · <span class="kbd">←</span> <span class="kbd">→</span> week · <span class="kbd">Enter</span> filter</span>';
    }
    function drawFilter() {
      if (S.day == null) { filt.hidden = true; return; }
      const v = dayCount(S.day);
      filt.hidden = false;
      filt.innerHTML = `${ic('lanes', 14)}<span>Showing <b>${v}</b> ${S.who === 'me' ? 'of your commits' : 'commits'} on <b>${dateLong(S.day)}</b></span><span class="od-fill"></span><button class="btn btn-sm" data-clear>Clear filter <span class="kbd">Esc</span></button>`;
    }
    function all() { drawStrip(); drawFilter(); drawHistory(); G.initTips(win); }
    all();
    if (S.state === 'large') {                                   // progressive fill: newest week first, one week per tick
      const R = range(0); let wk = 0; pendingFrom = TODAY - dow(TODAY) * DAY;
      const sum = (from) => { let s = 0; for (let t = from; t <= TODAY; t += DAY) s += (S.who === 'me' ? DATA.repo.me : DATA.repo.everyone)[key(t)] || 0; return s; };
      partial = sum(pendingFrom) * 37; drawStrip();
      timer = setInterval(() => {
        wk += 3; pendingFrom = TODAY - dow(TODAY) * DAY - wk * 7 * DAY;
        if (pendingFrom <= R.start) { pendingFrom = null; clearInterval(timer); timer = null; }
        partial = sum(pendingFrom || R.start) * 37; drawStrip();
      }, 450);
    }
    wire(win, strip, (cell) => {
      const v = +cell.dataset.v; if (!v || S.state === 'large' && pendingFrom != null) return;
      S.day = S.day === +cell.dataset.t ? null : +cell.dataset.t; refocus();
    });
    function refocus() { const t = S.day; all(); const g = strip.querySelector('.heat-grid'); if (g) { g.focus({ preventScroll: true }); const c = t != null && g.querySelector(`.hc[data-t="${t}"]`); if (c) c.classList.add('kf'); } }
    strip.addEventListener('click', (e) => {
      const w = e.target.closest('[data-who]'), tg = e.target.closest('[data-toggle]'), rg = e.target.closest('[data-range]'), rt = e.target.closest('[data-retry]');
      if (w && w.tagName === 'A') e.preventDefault();
      if (w && !w.disabled) { S.who = w.dataset.who; if (S.state === 'nomine' && S.who === 'everyone') { /* stays in state; data now non-zero */ } S.day = null; all(); }
      else if (tg) { S.collapsed = !S.collapsed; strip.classList.toggle('collapsed', S.collapsed); drawStrip(); G.initTips(win); }
      else if (rg) { const r = rg.getBoundingClientRect(); G.menu(win, { clientX: r.left, clientY: r.bottom + 2 }, [{ label: 'Last 12 months', icon: S.year ? '' : 'check', onClick: () => { S.year = 0; S.day = null; all(); } }, { sep: 1 }].concat([2026, 2025, 2024].map((y) => ({ label: String(y), icon: S.year === y ? 'check' : '', onClick: () => { S.year = y; S.day = null; all(); } })))); }
      else if (rt) { G.toast(win, 'Counting commits again…', 'info'); }
    });
    filt.addEventListener('click', (e) => { if (e.target.closest('[data-clear]')) { S.day = null; all(); } });
    win.addEventListener('keydown', (e) => { if (e.key === 'Escape' && S.day != null && !win.querySelector('.menu')) { S.day = null; refocus(); } });
    G.fit(win);
  }

  /* ================= Variant A: Welcome ================= */
  function variantA(wrap) {
    const R = [['gitwarp', '~/develop/gitwarp', 'main', '12 min ago'], ['open-design', '~/develop/open-design', 'feat/prompt-bundle', '2 h ago'], ['zed-themes', '~/projects/zed-themes', 'main', 'yesterday'], ['dotfiles', '~/.dotfiles', 'master', '3 d ago'], ['tree-sitter-rust', '~/src/tree-sitter-rust', 'master', '2 weeks ago'], ['notes', '/mnt/data/notes', '—', '3 weeks ago']];
    const win = G.h(`<div class="win" style="width:960px;height:680px;grid-template-rows:var(--bar) minmax(0,1fr) var(--status)">
      <header class="titlebar"><span style="display:inline-flex;gap:8px;align-items:center;font-weight:600;padding-left:4px">${G.logo(16)}GitWarp</span><span class="od-fill"></span><div class="wc"><button class="btn btn-ghost" aria-label="Minimise">${ic('min', 14)}</button><button class="btn btn-ghost" aria-label="Maximise">${ic('max', 14)}</button><button class="btn btn-ghost close" aria-label="Close">${ic('x', 14)}</button></div></header>
      <div class="wl"><aside><div class="logo">${G.logo(26)}GitWarp</div><div class="muted" style="margin-bottom:16px">A fast, keyboard-first git client.</div>
        <button class="act" data-tip="Open an existing repository">${ic('folder', 16)}Open repository…<span class="kbd">Ctrl O</span></button>
        <button class="act" data-tip="Clone a repository from a URL">${ic('clone', 16)}Clone repository…</button>
        <button class="act" data-tip="Create a new repository">${ic('init', 16)}Init new repository…<span class="kbd">Ctrl N</span></button></aside>
      <main><section class="wact-sec" aria-label="Your activity"></section>
        <div class="od-row"><h2 style="margin:0;font-size:var(--fs-lg)" class="od-fill">Recent repositories</h2><label class="input-wrap" style="width:240px">${ic('search', 14)}<input class="input" placeholder="Filter recent…" aria-label="Filter recent repositories"></label></div>
        <div class="recent" role="listbox" tabindex="0" aria-label="Recent repositories">${R.map((r, i) => `<div class="rrow${i === S.sel ? ' sel' : ''}" role="option" aria-selected="${i === S.sel}" data-i="${i}"><span>${ic('folder', 16)}</span><div style="min-width:0"><div class="n">${esc(r[0])}</div><div class="p od-truncate mono">${esc(r[1])}</div></div><span class="od-truncate muted" style="font-size:12px">${r[2] === '—' ? '' : ic('branch', 12) + ' '}${esc(r[2])}</span><span class="muted" style="font-size:12px;text-align:right">${r[3]}</span></div>`).join('')}</div>
        <div class="muted" style="font-size:12px"><span class="kbd">↑</span> <span class="kbd">↓</span> select · <span class="kbd">Enter</span> open · <span class="kbd">Del</span> remove from list</div></main></div>
      <footer class="statusbar"><span class="od-fill"></span><span class="sb-btn">${ic('git', 12)}git 2.56.0 · bundled</span></footer></div>`);
    wrap.appendChild(win);
    const sec = win.querySelector('.wact-sec'), list = win.querySelector('.recent');
    function draw() {
      const repo = R[S.sel][0], unavailable = repo === 'notes';
      const data = S.scope === 'all' ? DATA.recent : DATA.perRepo[repo] || {};
      const b = (S.scope === 'repo' && unavailable && S.state === 'ok')
        ? { html: `<div class="heat-msg" style="min-height:84px">${ic('warn', 16)}<span><span class="mono">/mnt/data/notes</span> is not reachable, so its activity can’t be counted.</span></div>`, head: '<span class="heat-total">Unavailable</span>', ranges: [] }
        : body({ data, compact: true, mine: true, label: `Your commit activity, ${S.scope === 'all' ? 'all recent repositories' : repo}, last 12 months` });
      const noEmail = S.state === 'noemail';
      sec.innerHTML = `<div class="od-row" style="margin-bottom:12px"><h2 style="margin:0;font-size:var(--fs-lg)">Your activity</h2>${noEmail ? '' : b.head}<span class="od-fill"></span>
          <span class="seg sm" role="group" aria-label="Activity scope"><button data-scope="all" aria-pressed="${S.scope === 'all'}" data-tip="Your commits in the ${R.length - 1} reachable recent repositories">All recent</button><button data-scope="repo" aria-pressed="${S.scope === 'repo'}" data-tip="Follows the selected row">${esc(repo)}</button></span></div>
        <div class="wact">${noEmail
          ? `<div class="heat-msg" style="min-height:84px">${ic('user', 16)}<span>Set a global <span class="mono">user.email</span> to see your activity across repositories.</span><button class="btn btn-sm" onclick="location.href='settings-identity.html'">Set identity…</button></div>`
          : `<div class="heat-scroll">${b.html}</div>`}
          <div class="wact-foot"><span class="heat-total">${S.state === 'ok' && S.scope === 'all' ? `By <span class="mono">${ME}</span> · 5 of 6 repositories <span data-tip="/mnt/data/notes is not reachable" style="color:var(--warning)">${ic('warn', 12)}</span>` : S.state === 'nomine' ? `No commits by <span class="mono">${ME}</span>` : S.state === 'empty' ? 'This repository has no commits yet' : ''}</span><span class="od-fill"></span>${noEmail || S.state === 'error' ? '' : legendHTML(b.ranges || [])}</div></div>`;
      G.initTips(win);
    }
    draw();
    sec.addEventListener('click', (e) => { const s = e.target.closest('[data-scope]'); if (s) { S.scope = s.dataset.scope; draw(); } else if (e.target.closest('[data-retry]')) G.toast(win, 'Counting commits again…', 'info'); });
    list.addEventListener('click', (e) => { const r = e.target.closest('.rrow'); if (!r) return; S.sel = +r.dataset.i; list.querySelectorAll('.rrow').forEach((x, k) => { x.classList.toggle('sel', k === S.sel); x.setAttribute('aria-selected', k === S.sel); }); if (S.scope === 'repo') draw(); else { const b = sec.querySelector('[data-scope=repo]'); b.textContent = R[S.sel][0]; } });
    wire(win, sec, (cell, e) => {
      const v = +cell.dataset.v, t = +cell.dataset.t; if (!v) return;
      const cr = cell.getBoundingClientRect(), at = { clientX: cr.left, clientY: cr.bottom + 4 };
      if (S.scope === 'repo') { G.toast(win, `Opened ${R[S.sel][0]} · History filtered to ${dateLong(t)}`, 'ok'); return; }
      // split the day's total over the repositories that have commits on it
      const r = rng(t / DAY), names = ['gitwarp', 'open-design', 'zed-themes', 'dotfiles'];
      let left = v; const parts = [];
      names.forEach((n, i) => { const k = i === names.length - 1 ? left : Math.min(left, Math.round(r() * left)); if (k > 0) parts.push([n, k]); left -= k; });
      G.menu(win, at, [{ label: `${dateLong(t)} — open with History filtered to this day`, disabled: true }].concat(parts.sort((a, b) => b[1] - a[1]).map(([n, k]) => ({ label: `${n} · ${k} commit${k === 1 ? '' : 's'}`, icon: 'folder', onClick: () => G.toast(win, `Opened ${n} · History filtered to ${dateLong(t)}`, 'ok') }))));
    });
    G.fit(win);
  }

  const CAP_B = `<b>B · strip above History.</b> 34px header (same height as the details-dock header) + 14px month row + 7 × 12px grid; ~150px expanded, 34px collapsed. Collapsed/expanded is remembered app-wide. Click a day (or focus it and press <span class="kbd">Enter</span>) to filter History to that day — the accent bar under the strip says so; <span class="kbd">Esc</span> or “Clear filter” returns to the full graph. Clicking the selected day again also clears. Days with no commits do nothing on click. The dock and sidebar are unchanged.`;
  const CAP_A = `<b>A · Welcome.</b> “Your activity” sits above “Recent repositories”, in the same bordered panel style as the list. Author is always <i>you</i> (global <span class="mono">user.email</span>); the scope switch is <i>All recent</i> or the selected row (the second button follows the list selection). The grid uses the compact 8px cell so 53 weeks fit the 612px column. Clicking a day opens a menu of the repositories with commits that day; picking one opens it with History filtered to the day. Unreachable repositories are skipped and counted in the footer.`;

  render();
})();
