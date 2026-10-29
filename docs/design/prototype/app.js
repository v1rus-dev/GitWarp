/* gitwarp mock runtime: icons, sample data, graph layout/render, diff render, overlays, window shell.
   This is design-prototype code; every behaviour here is a spec for the GPUI implementation. */
(function () {
  'use strict';
  const G = (window.GL = {});

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
  G.ic = (name, size, cls) => `<svg class="od-icon${cls ? ' ' + cls : ''}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" style="--od-icon-size:${size || 14}px" aria-hidden="true">${ICONS[name] || ''}</svg>`;
  const ic = G.ic;

  /* ---------- stage bar (presentation only) ---------- */
  G.PAGES = [['index.html', 'Overview'], ['tokens.html', 'Tokens'], ['components.html', 'Components'], ['graph.html', 'Graph'], ['main-1200.html', 'Main 1200×800'], ['main-1600.html', 'Main 1600×1000'], ['changes.html', 'Local changes'], ['settings-general.html', 'Settings'], ['settings-git.html', 'Settings → Git'], ['history-states.html', 'History states'], ['welcome.html', 'Welcome'], ['dialogs-repo.html', 'Dialogs: clone / open'], ['dialogs-ref.html', 'Dialogs: branch / tag'], ['dialogs-worktree.html', 'Worktrees'], ['settings-ssh.html', 'Settings → SSH keys'], ['palette.html', 'Palette']];
  G.stagebar = function (extra) {
    const cur = location.pathname.split('/').pop() || 'index.html';
    const bar = G.h(`<header class="stagebar"><span class="brand">${ic('git', 16)} gitwarp <span class="muted" style="font-weight:400">design</span></span>${G.PAGES.map(([h, l]) => `<a class="nav" href="${h}"${h === cur ? ' aria-current="page"' : ''}>${l}</a>`).join('')}<span class="od-fill"></span>${extra || ''}<button class="btn btn-sm" id="gl-theme" aria-label="Switch theme"></button></header>`);
    document.body.prepend(bar);
    const b = bar.querySelector('#gl-theme');
    const lab = () => { b.textContent = 'Theme: ' + G.THEMES[G.theme]; };
    lab();
    b.onclick = () => { const k = Object.keys(G.THEMES); G.setTheme(k[(k.indexOf(G.theme) + 1) % k.length]); lab(); document.dispatchEvent(new Event('gl-theme')); };
    return bar;
  };
  G.fit = function (win) {
    const wrap = win.parentElement;
    const apply = () => { const W = win.offsetWidth, H = win.offsetHeight, s = Math.min(1, (document.documentElement.clientWidth - 32) / W); win.style.transform = `scale(${s})`; wrap.style.width = W * s + 'px'; wrap.style.height = H * s + 'px'; };
    win._fit = apply; apply(); addEventListener('resize', apply);
  };
  G.scale = (el) => { const w = el.closest('.win'); return w ? w.getBoundingClientRect().width / w.offsetWidth || 1 : 1; };

  /* ---------- sample data: relative time ---------- */
  const NOW = Date.UTC(2026, 9, 29, 14, 20);
  const pad = (n) => String(n).padStart(2, '0');
  G.abs = (min) => { const d = new Date(NOW - min * 60000); return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`; };
  G.rel = (m) => (m < 1 ? 'just now' : m < 60 ? `${m} min ago` : m < 1440 ? `${Math.floor(m / 60)} h ago` : m < 43200 ? `${Math.floor(m / 1440)} d ago` : `${Math.floor(m / 43200)} mo ago`);

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
    ['a09', '8d41f6e', ['a10'], 'fix(settings): list both reasons when bundled and system git fail', 'pn', 600, ['br:fix/git-detect', 'rm:origin/fix/git-detect'], ''],
    ['a07', '5c0e92b', ['a08'], 'refactor(ui): extract Tree component from the sidebar', 'lz', 780, [], ''],
    ['a10', 'b7a3d15', ['a08'], 'feat(settings): validate custom git path, require git ≥ 2.31', 'pn', 1100, [], ''],
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
  G.geometry = function (lay, collapse) {
    const n = lay.lanes, ev = new Array(n).fill(0), xs = [], grp = [];
    lay.rows.forEach((r) => { ev[r.col]++; r.cols.forEach((i) => { if (i !== r.col) ev[i]++; }); r.outs.forEach((j) => { if (j !== r.col) ev[j]++; }); });
    if (!collapse) { for (let i = 0; i < n; i++) xs.push(i * 14 + 7); return { xs, grp, width: n * 14, idle: 0 }; }
    let x = 0, i = 0, idle = 0;
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
    const pass = (i) => { const x = xs[i]; if (x > maxX) return; const k = 'v' + x; if (seen.has(k)) return; seen.add(k); p += `<path class="g-e${geo.grp[i] ? ' idle' : ''}" style="--h:${G.hue(i)}" d="M${x} 0V26"/>`; };
    row.before.forEach((v, i) => { if (v !== null && !row.cols.includes(i)) pass(i); });
    const xc = xs[row.col];
    row.cols.forEach((i) => {
      const xi = xs[i];
      if (Math.min(xi, xc) > maxX) return;
      p += i === row.col ? `<path class="g-e" style="--h:${G.hue(i)}" d="M${xi} 0V13"/>` : `<path class="g-e" style="--h:${G.hue(i)}" d="M${xi} 0C${xi} 7 ${xc} 6 ${xc} 13"/>`;
    });
    row.outs.forEach((j) => {
      const xj = xs[j];
      if (Math.min(xj, xc) > maxX) return;
      p += j === row.col ? `<path class="g-e" style="--h:${G.hue(j)}" d="M${xc} 13V26"/>` : `<path class="g-e" style="--h:${G.hue(j)}" d="M${xc} 13C${xc} 20 ${xj} 19 ${xj} 26"/>`;
    });
    if (xc <= maxX + 6) {
      if (o.head) p += `<circle class="g-h" cx="${xc}" cy="13" r="7.5"/>`;
      p += row.merge ? `<circle class="g-d m" style="--h:${G.hue(row.col)}" cx="${xc}" cy="13" r="3.5"/>` : `<circle class="g-d" style="--h:${G.hue(row.col)}" cx="${xc}" cy="13" r="4.5"/>`;
    }
    return `<svg width="${Math.min(geo.width, maxX + 20)}" height="26" viewBox="0 0 ${Math.min(geo.width, maxX + 20)} 26" aria-hidden="true">${p}</svg>`;
  };

  /* ---------- virtualised history list ---------- */
  G.history = function (host, commits, o) {
    o = o || {};
    const lay = G.layout(commits);
    let collapse = !!o.collapse, gw = o.gw || 112, geo = G.geometry(lay, collapse), sel = o.sel == null ? 0 : o.sel;
    host.classList.add('hist');
    host.innerHTML = `<div class="lhead" role="row"><div class="gh"><span>Graph</span><button class="more" hidden></button><span class="grip" title="Drag to resize the graph column · double-click to fit"></span></div><div>Summary</div><div>Author</div><div>Date</div><div>Hash</div></div><div class="lbody" tabindex="0" role="listbox" aria-label="Commit history"></div>`;
    const head = host.querySelector('.lhead'), body = host.querySelector('.lbody'), more = head.querySelector('.more'), grip = head.querySelector('.grip');
    const RH = 26, OV = 8;
    const api = { el: host, get sel() { return sel; } };
    function setGw() { head.style.setProperty('--gw', gw + 'px'); body.style.setProperty('--gw', gw + 'px'); }
    function rowHTML(i) {
      const c = commits[i], r = lay.rows[i], clipped = geo.width > gw;
      const head_ = c.refs.some((x) => x.t === 'head');
      const refs = c.refs.map((x) => x.t === 'head' ? `<span class="ref head">${ic('branch', 11)}${esc(x.n)}</span>` : x.t === 'br' ? `<span class="ref">${ic('branch', 11)}${esc(x.n)}</span>` : x.t === 'rm' ? `<span class="ref remote">${ic('cloud', 11)}${esc(x.n)}</span>` : `<span class="ref tag">${ic('tag', 11)}${esc(x.n)}</span>`).join('');
      return `<div class="lrow${head_ ? ' head' : ''}${i === sel ? ' sel' : ''}" role="option" aria-selected="${i === sel}" id="cm-${i}" data-i="${i}"><div class="gcol${clipped ? ' clip' : ''}">${G.rowSVG(r, geo, { maxX: clipped ? gw : undefined, head: head_ })}</div><div class="summary">${refs}<span class="txt" title="${esc(c.summary)}">${esc(c.summary)}</span></div><div class="au">${G.avatar(c, 16)}<span class="od-truncate" title="${esc(c.author)}">${esc(c.author)}</span></div><div class="dt" data-tip="${G.abs(c.min)}">${G.rel(c.min)}</div><div class="hash">${c.hash}</div></div>`;
    }
    function paint() {
      const st = body.scrollTop, vh = body.clientHeight || 420;
      const a = Math.max(0, Math.floor(st / RH) - OV), b = Math.min(commits.length, Math.ceil((st + vh) / RH) + OV);
      let h = `<div class="vs" style="height:${a * RH}px"></div>`;
      for (let i = a; i < b; i++) h += rowHTML(i);
      h += `<div class="vs" style="height:${(commits.length - b) * RH}px"></div>`;
      body.innerHTML = h;
    }
    function header() {
      const hidden = geo.xs.filter((x) => x > gw).length;
      more.hidden = hidden === 0;
      more.textContent = `+${hidden}`;
      more.title = `${hidden} lanes are clipped. Click to widen the column.`;
    }
    function render() { setGw(); header(); paint(); }
    function select(i, scroll) {
      sel = Math.max(0, Math.min(commits.length - 1, i));
      if (scroll !== false) { const t = sel * RH; if (t < body.scrollTop) body.scrollTop = t; else if (t + RH > body.scrollTop + body.clientHeight) body.scrollTop = t + RH - body.clientHeight; }
      paint(); body.setAttribute('aria-activedescendant', 'cm-' + sel);
      body.querySelectorAll('.lrow.sel').forEach((e) => e.classList.add('kf'));
      o.onSelect && o.onSelect(commits[sel], sel);
    }
    body.addEventListener('scroll', () => requestAnimationFrame(paint));
    body.addEventListener('mousedown', (e) => { const r = e.target.closest('.lrow'); if (r) { body.focus({ preventScroll: true }); select(+r.dataset.i, false); body.querySelectorAll('.lrow.sel').forEach((x) => x.classList.remove('kf')); } });
    body.addEventListener('contextmenu', (e) => { const r = e.target.closest('.lrow'); if (!r) return; select(+r.dataset.i, false); const win = host.closest('.win'); if (win) G.menu(win, e, [{ label: 'Copy commit hash', icon: 'copy', key: 'Ctrl+C' }, { label: 'Create branch here…', icon: 'branch', key: 'B', onClick: () => window.GLD && window.GLD.openIn('branch', win) }, { label: 'Create tag here…', icon: 'tag', onClick: () => window.GLD && window.GLD.openIn('tag', win) }, { sep: 1 }, { label: 'Cherry-pick', icon: 'undo' }, { label: 'Revert commit', icon: 'undo' }, { label: 'Reset main to here…', danger: 1 }]); });
    body.addEventListener('keydown', (e) => {
      const page = Math.floor(body.clientHeight / RH) - 1;
      const k = { ArrowDown: 1, ArrowUp: -1, PageDown: page, PageUp: -page, j: 1, k: -1 }[e.key];
      if (k) { select(sel + k); e.preventDefault(); }
      else if (e.key === 'Home') { select(0); e.preventDefault(); }
      else if (e.key === 'End') { select(commits.length - 1); e.preventDefault(); }
    });
    more.addEventListener('click', () => { gw = Math.min(geo.width + 8, 420); render(); });
    grip.addEventListener('dblclick', () => { gw = Math.max(40, Math.min(geo.width + 8, 420)); render(); });
    grip.addEventListener('mousedown', (e) => {
      e.preventDefault(); grip.classList.add('drag');
      const x0 = e.clientX, w0 = gw, s = G.scale(host);
      const mv = (ev) => { gw = Math.max(40, Math.min(480, w0 + (ev.clientX - x0) / s)); render(); };
      const up = () => { grip.classList.remove('drag'); removeEventListener('mousemove', mv); removeEventListener('mouseup', up); };
      addEventListener('mousemove', mv); addEventListener('mouseup', up);
    });
    api.select = select;
    api.setCollapse = (b) => { collapse = b; geo = G.geometry(lay, collapse); render(); };
    api.setWidth = (w) => { gw = w; render(); };
    api.geo = () => geo;
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
    const m = G.h(`<div class="menu" role="menu" tabindex="-1">${items.map((it, i) => it.sep ? '<div class="menu-sep" role="separator"></div>' : `<div class="menu-item${it.danger ? ' danger' : ''}${it.disabled ? ' disabled' : ''}" role="menuitem" data-i="${i}">${it.icon ? ic(it.icon, 14) : '<span class="menu-gap"></span>'}<span class="menu-label">${esc(it.label)}</span>${it.key ? `<span class="menu-key">${esc(it.key)}</span>` : ''}</div>`).join('')}</div>`);
    layer.appendChild(m);
    m.style.left = Math.max(4, Math.min(p.x, win.offsetWidth - m.offsetWidth - 4)) + 'px';
    m.style.top = Math.max(4, Math.min(p.y, win.offsetHeight - m.offsetHeight - 4)) + 'px';
    m.focus();
    const its = Array.from(m.querySelectorAll('.menu-item:not(.disabled)')); let k = -1;
    const hi = (n) => { its.forEach((x) => x.classList.remove('hi')); k = (n + its.length) % its.length; its[k].classList.add('hi'); };
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
  };
  G.initTips = function (win) {
    const layer = G.layer(win); let t, tip, last;
    const hide = () => { clearTimeout(t); if (tip) { tip.remove(); tip = null; } last = null; };
    win.addEventListener('mouseover', (e) => {
      const el = e.target.closest && e.target.closest('[data-tip]');
      if (el === last) return;
      hide(); if (!el) return; last = el;
      t = setTimeout(() => {
        tip = G.h(`<div class="tip" role="tooltip">${esc(el.dataset.tip)}${el.dataset.kbd ? `<span class="kbd">${esc(el.dataset.kbd)}</span>` : ''}</div>`);
        layer.appendChild(tip);
        const r = el.getBoundingClientRect(), wr = win.getBoundingClientRect(), s = wr.width / win.offsetWidth || 1;
        let x = (r.left - wr.left) / s, y = (r.bottom - wr.top) / s + 6;
        x = Math.max(4, Math.min(x, win.offsetWidth - tip.offsetWidth - 6));
        if (y + tip.offsetHeight > win.offsetHeight - 4) y = (r.top - wr.top) / s - tip.offsetHeight - 6;
        tip.style.left = x + 'px'; tip.style.top = y + 'px';
      }, 350);
    });
    win.addEventListener('mouseleave', hide); win.addEventListener('mousedown', hide);
  };
  G.toast = function (win, msg, kind) {
    const layer = G.layer(win); let c = layer.querySelector('.toasts');
    if (!c) { c = G.h('<div class="toasts"></div>'); layer.appendChild(c); }
    const t = G.h(`<div class="toast ${kind || 'info'}" role="status">${ic(kind === 'error' ? 'err' : kind === 'warn' ? 'warn' : kind === 'ok' ? 'check' : 'info', 14)}<span class="toast-msg">${esc(msg)}</span><button class="btn btn-icon btn-ghost btn-xs" aria-label="Dismiss">${ic('x', 12)}</button></div>`);
    t.querySelector('button').onclick = () => t.remove(); c.appendChild(t); setTimeout(() => t.remove(), 5000);
  };
  G.resizer = function (handle, target, axis, min, max, invert) {
    handle.addEventListener('mousedown', (e) => {
      e.preventDefault(); handle.classList.add('drag');
      const p0 = axis === 'x' ? e.clientX : e.clientY, s0 = axis === 'x' ? target.offsetWidth : target.offsetHeight, sc = G.scale(handle);
      const mv = (ev) => { const d = ((axis === 'x' ? ev.clientX : ev.clientY) - p0) / sc * (invert ? -1 : 1); const v = Math.max(min, Math.min(max, s0 + d)); target.style[axis === 'x' ? 'width' : 'height'] = v + 'px'; };
      const up = () => { handle.classList.remove('drag'); removeEventListener('mousemove', mv); removeEventListener('mouseup', up); };
      addEventListener('mousemove', mv); addEventListener('mouseup', up);
    });
  };

  /* ---------- window shell: toolbar, sidebar, status bar ---------- */
  const TREE_ICON = { sec: 'chevD' };
  function node(lvl, icon, label, o) {
    o = o || {};
    const tr = (o.ahead != null || o.behind != null) ? `<span class="ab">${o.ahead ? `<span class="up" aria-label="${o.ahead} ahead">↑${o.ahead}</span>` : ''}${o.behind ? `<span class="dn" aria-label="${o.behind} behind">↓${o.behind}</span>` : ''}</span>` : o.count != null ? `<span class="ncount">${o.count}</span>` : o.state ? `<span class="muted" style="font-size:11px">${o.state}</span>` : '';
    return `<div class="node${o.sec ? ' sec' : ''}${o.cur ? ' cur' : ''}${o.sel ? ' sel' : ''}" role="treeitem" tabindex="-1" style="--lvl:${lvl}" data-kind="${o.kind || ''}" data-name="${esc(o.name || label)}"${o.exp != null ? ` aria-expanded="${o.exp}"` : ''}${o.cur ? ' aria-current="true"' : ''}><span class="twisty">${o.exp == null ? '' : ic(o.exp ? 'chevD' : 'chevR', 12)}</span>${icon ? ic(icon, 14) : ''}<span class="nlabel od-truncate">${esc(label)}</span>${tr}</div>`;
  }
  G.sidebarHTML = function () {
    return [
      node(0, '', 'Local branches', { sec: 1, exp: true, count: 5 }),
      node(1, 'branch', 'main', { cur: 1, kind: 'branch', ahead: 4 }),
      node(1, 'folder', 'feat', { exp: true, kind: 'folder' }),
      node(2, 'branch', 'idle-lanes', { kind: 'branch', name: 'feat/idle-lanes', ahead: 1 }),
      node(2, 'branch', 'palette', { kind: 'branch', name: 'feat/palette', behind: 2 }),
      node(1, 'folder', 'fix', { exp: true, kind: 'folder' }),
      node(2, 'branch', 'git-detect', { kind: 'branch', name: 'fix/git-detect' }),
      node(0, '', 'Remotes', { sec: 1, kind: 'sec-remotes', exp: true, count: 2 }),
      node(1, 'cloud', 'origin', { exp: true, kind: 'remote' }),
      node(2, 'branch', 'main', { kind: 'rbranch', name: 'origin/main' }),
      node(2, 'branch', 'feat/idle-lanes', { kind: 'rbranch', name: 'origin/feat/idle-lanes' }),
      node(2, 'branch', 'fix/git-detect', { kind: 'rbranch', name: 'origin/fix/git-detect' }),
      node(1, 'cloud', 'upstream', { exp: false, kind: 'remote' }),
      node(0, '', 'Tags', { sec: 1, kind: 'sec-tags', exp: true, count: 2 }),
      node(1, 'tag', 'v0.3.0', { kind: 'tag' }),
      node(1, 'tag', 'v0.2.0', { kind: 'tag' }),
      node(0, '', 'Stashes', { sec: 1, exp: true, count: 1 }),
      node(1, 'stash', 'WIP on feat/idle-lanes', { kind: 'stash', name: 'stash@{0}' }),
      node(0, '', 'Worktrees', { sec: 1, kind: 'sec-wt', exp: true, count: 3 }),
      node(1, 'worktree', 'gitwarp', { cur: 1, kind: 'wt', state: 'main' }),
      node(1, 'worktree', 'gitwarp-hotfix', { kind: 'wt', state: 'fix/git-detect' }),
      node(1, 'worktree', 'gitwarp-review', { kind: 'wt', state: 'locked' }),
      node(0, '', 'Submodules', { sec: 1, exp: true, count: 1 }),
      node(1, 'box', 'vendor/tree-sitter-rust', { kind: 'sub', state: 'modified' })
    ].join('');
  };
  const MENUS = {
    branch: (n) => [{ label: 'Checkout', icon: 'check', key: 'Enter' }, { label: `Merge ${n} into main`, icon: 'branch' }, { label: `Rebase main onto ${n}`, icon: 'branch' }, { sep: 1 }, { label: 'Rename…', icon: 'edit', key: 'F2' }, { label: 'Set upstream…', icon: 'link' }, { label: 'Delete branch', icon: 'trash', danger: 1, key: 'Del' }, { sep: 1 }, { label: 'Copy branch name', icon: 'copy' }],
    rbranch: (n) => [{ label: 'Checkout as local branch', icon: 'check' }, { label: `Merge ${n} into main`, icon: 'branch' }, { sep: 1 }, { label: 'Delete remote branch…', icon: 'trash', danger: 1 }, { label: 'Copy branch name', icon: 'copy' }],
    tag: () => [{ label: 'Checkout', icon: 'check' }, { label: 'Create branch here…', icon: 'branch', onClick: () => window.GLD && window.GLD.openIn('branch') }, { label: 'Push tag to origin', icon: 'up' }, { sep: 1 }, { label: 'Delete tag', icon: 'trash', danger: 1 }],
    stash: () => [{ label: 'Apply stash', icon: 'check' }, { label: 'Pop stash', icon: 'undo' }, { label: 'View changes', icon: 'eye' }, { sep: 1 }, { label: 'Drop stash', icon: 'trash', danger: 1 }],
    'sec-wt': () => [{ label: 'Add worktree…', icon: 'plus', onClick: () => window.GLD && window.GLD.openIn('worktreeAdd') }, { sep: 1 }, { label: 'Prune missing worktrees', icon: 'trash' }, { label: 'Refresh list', icon: 'fetch' }],
    'sec-tags': () => [{ label: 'Create tag…', icon: 'plus', onClick: () => window.GLD && window.GLD.openIn('tag') }, { sep: 1 }, { label: 'Fetch tags from origin', icon: 'fetch' }, { label: 'Push all tags to origin', icon: 'up' }],
    'sec-remotes': () => [{ label: 'Add remote…', icon: 'plus', onClick: () => window.GLD && window.GLD.openIn('remoteAdd') }, { sep: 1 }, { label: 'Fetch all remotes', icon: 'fetch', key: 'Ctrl+Shift+F' }, { label: 'Prune stale remote branches', icon: 'trash' }],
    wt: (n) => [{ label: 'Open in new window', icon: 'folder' }, { label: 'Add worktree…', icon: 'plus', onClick: () => window.GLD && window.GLD.openIn('worktreeAdd') }, { sep: 1 }, { label: 'Lock', icon: 'lock' }, { label: 'Remove worktree…', icon: 'trash', danger: 1, onClick: () => window.GLD && window.GLD.openIn('worktreeRemove') }],
    remote: () => [{ label: 'Fetch', icon: 'fetch' }, { label: 'Edit remote…', icon: 'edit' }, { label: 'Remove remote', icon: 'trash', danger: 1 }],
    sub: () => [{ label: 'Update submodule', icon: 'fetch' }, { label: 'Open in new window', icon: 'folder' }]
  };
  function toggleNode(n) {
    const lvl = +n.style.getPropertyValue('--lvl'), open = n.getAttribute('aria-expanded') !== 'true';
    n.setAttribute('aria-expanded', open); n.querySelector('.twisty').innerHTML = ic(open ? 'chevD' : 'chevR', 12);
    let s = n.nextElementSibling, skip = null;
    while (s && s.classList.contains('node')) {
      const l = +s.style.getPropertyValue('--lvl');
      if (l <= lvl) break;
      if (skip !== null && l > skip) { s = s.nextElementSibling; continue; }
      skip = null; s.hidden = !open;
      if (open && s.getAttribute('aria-expanded') === 'false') skip = l;
      s = s.nextElementSibling;
    }
  }
  G.shell = function (o) {
    const repo = o.repo || 'gitwarp', branch = o.branch || 'main', view = o.view || 'history';
    const win = G.h(`<div class="win" style="width:${o.w}px;height:${o.h}px">
      <header class="titlebar">
        <button class="btn btn-icon btn-ghost" data-act="sidebar" data-tip="Toggle sidebar" data-kbd="Ctrl+B" aria-label="Toggle sidebar">${ic('sidebar', 16)}</button>
        <button class="btn btn-tool" data-tip="Switch repository" data-kbd="Ctrl+O">${ic('folder', 14)}<b class="tb-name">${esc(repo)}</b>${ic('chevD', 12)}</button>
        <button class="btn btn-tool" data-act="branch" data-tip="Switch branch" data-kbd="Ctrl+Shift+B">${ic('branch', 14)}<b class="tb-name">${esc(branch)}</b>${ic('chevD', 12)}</button>
        <span class="tb-sep"></span>
        <button class="btn btn-tool" data-tip="Fetch all remotes" data-kbd="Ctrl+Shift+F">${ic('fetch', 14)}<span class="lbl">Fetch</span></button>
        <button class="btn btn-tool" data-tip="Pull — nothing to pull" data-kbd="Ctrl+Shift+L">${ic('down', 14)}<span class="lbl">Pull</span></button>
        <button class="btn btn-tool" data-tip="Push 4 commits to origin/main" data-kbd="Ctrl+Shift+P">${ic('up', 14)}<span class="lbl">Push</span><span class="counter accent">↑4</span></button>
        <button class="btn btn-tool" data-tip="Stash local changes" data-kbd="Ctrl+Shift+S">${ic('stash', 14)}<span class="lbl">Stash</span><span class="counter">1</span></button>
        <span class="od-fill"></span>
        <div class="seg" role="tablist" aria-label="View"><button role="tab" class="${view === 'history' ? 'on' : ''}" onclick="location.href='main-1200.html'">${ic('lanes', 14)}<span class="lbl">History</span></button><button role="tab" class="${view === 'changes' ? 'on' : ''}" onclick="location.href='changes.html'">${ic('edit', 14)}<span class="lbl">Changes</span><span class="counter accent">6</span></button></div>
        <label class="input-wrap tb-search" data-tip="Search commits, authors, hashes" data-kbd="Ctrl+K">${ic('search', 14)}<input class="input" placeholder="Search commits, authors, hashes" aria-label="Search commits, authors, hashes"><span class="kbd tb-kbd">Ctrl K</span></label>
        <button class="btn btn-icon btn-ghost" data-tip="Settings" data-kbd="Ctrl+," aria-label="Settings" onclick="location.href='settings-git.html'">${ic('gear', 16)}</button>
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
        <span class="op">${ic('fetch', 12, 'spin')}Fetching origin… <span class="progress"><i style="width:62%"></i></span> 62%</span>
        <span class="od-fill"></span>
        <span>${o.statusInfo || '21 commits loaded'}</span>
        <button class="sb-btn" onclick="location.href='settings-git.html'" data-tip="Git executable — open settings">${ic('git', 12)}git 2.56.0 · bundled</button>
        <span class="muted"><span class="kbd">Ctrl P</span> commands</span>
      </footer>
    </div>`);
    o.host.appendChild(win);
    const sb = win.querySelector('.sidebar'), tree = win.querySelector('.tree');
    G.resizer(win.querySelector('.splitter.v'), sb, 'x', 160, 420);
    win.querySelector('[data-act=sidebar]').onclick = () => { sb.classList.toggle('collapsed'); win.querySelector('.splitter.v').hidden = sb.classList.contains('collapsed'); };
    addEventListener('keydown', (e) => { if (e.ctrlKey && e.key === 'b' && !e.shiftKey) { e.preventDefault(); win.querySelector('[data-act=sidebar]').click(); } });
    tree.addEventListener('click', (e) => {
      const n = e.target.closest('.node'); if (!n) return;
      if (n.hasAttribute('aria-expanded')) toggleNode(n);
      else { tree.querySelectorAll('.node.sel').forEach((x) => x.classList.remove('sel')); n.classList.add('sel'); }
    });
    tree.addEventListener('dblclick', (e) => { const n = e.target.closest('.node[data-kind=branch]'); if (n) G.toast(win, `Checked out ${n.dataset.name}`, 'ok'); });
    tree.addEventListener('contextmenu', (e) => { const n = e.target.closest('.node'); if (!n || !MENUS[n.dataset.kind]) return; G.menu(win, e, MENUS[n.dataset.kind](n.dataset.name)); });
    tree.addEventListener('keydown', (e) => {
      const vis = Array.from(tree.querySelectorAll('.node')).filter((x) => !x.hidden);
      let i = vis.findIndex((x) => x.classList.contains('kf'));
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { i = Math.max(0, Math.min(vis.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1))); vis.forEach((x) => x.classList.remove('kf')); vis[i].classList.add('kf'); vis[i].scrollIntoView({ block: 'nearest' }); e.preventDefault(); }
      else if ((e.key === 'Enter' || e.key === ' ') && i >= 0) { vis[i].click(); e.preventDefault(); }
    });
    const flt = win.querySelector('.sbfilter input');
    flt.addEventListener('input', () => {
      const q = flt.value.trim().toLowerCase();
      tree.querySelectorAll('.node:not(.sec)').forEach((n) => { n.hidden = q ? !(n.dataset.name || '').toLowerCase().includes(q) || n.dataset.kind === 'folder' : false; });
    });
    win.querySelector('[data-act=branch]').onclick = (e) => { const r = e.currentTarget.getBoundingClientRect(); G.menu(win, { clientX: r.left, clientY: r.bottom + 2 }, [{ label: 'main', icon: 'check' }, { label: 'feat/idle-lanes', icon: 'branch' }, { label: 'feat/palette', icon: 'branch' }, { label: 'fix/git-detect', icon: 'branch' }, { sep: 1 }, { label: 'New branch…', icon: 'plus', key: 'Ctrl+Shift+N' }]); };
    G.initTips(win);
    return { win, center: win.querySelector('.center') };
  };
})();
