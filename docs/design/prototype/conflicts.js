/* GitWarp conflict editor (merger). Design prototype: every state machine here is a spec for the GPUI implementation.
   Resolve chunk by chunk: two side panes (optional base), a live result pane, checkbox picks in tick order, hand edits ("Custom"),
   auto-resolve when one side equals the base, side-picking cards for binary and modify/delete conflicts, and a rebase labelling variant. */
(function () {
  'use strict';
  const G = GL, ic = G.ic, esc = G.esc;
  const Q = new URLSearchParams(location.search);
  if (Q.get('t') && G.THEMES[Q.get('t')]) G.setTheme(Q.get('t'));
  G.stagebar('<span class="muted" style="font-size:12px">resolve a conflicted file chunk by chunk · pick a state above the window · Alt+↑/↓, Alt+1/2, Ctrl+S</span>');
  const stage = G.h('<div class="stage"><div class="scen" id="scen"></div><div class="winwrap"></div><div class="notes" id="notes"></div></div>'); document.body.appendChild(stage);
  const sh = G.shell({ host: stage.querySelector('.winwrap'), w: 1200, h: 800, view: 'changes', statusInfo: '9 conflicted · 0 staged' });
  const win = sh.win;
  const sbBtn = win.querySelector('[data-act=sidebar]'); if (sbBtn) sbBtn.click(); /* the editor needs the width: the sidebar starts collapsed */
  const root = G.h('<div class="cfe" role="region" aria-label="Conflict editor"></div>'); sh.center.appendChild(root);
  const toast = (m, k) => G.toast(win, m, k || 'info');

  /* ---------- sample data ---------- */
  const MAIN = { name: 'main', tag: 'HEAD', hash: 'e4f2a91', subj: "Merge branch 'feat/idle-lanes'" };
  const FEAT = { name: 'feat/palette', tag: 'incoming', hash: 'a82c4d7', subj: 'feat(palette): fuzzy match with hash-prefix lookup' };
  const BASEC = { name: 'Base', tag: 'merge-base', hash: '3f5e1a9', subj: 'fix(staging): line staging was off by one on CRLF files' };
  const UPSTREAM = { name: 'feat/palette', tag: 'new base · upstream', hash: 'a82c4d7', subj: FEAT.subj };
  const REPLAYED = { name: '8d41f6e', tag: 'your commit', hash: '8d41f6e', subj: 'fix(graph): keep lane colour when a branch is collapsed' };

  /* segments: c = context, ma = non-conflicting change already merged from main, mb = from feat/palette, x = conflict chunk {base, a: main, b: feat/palette} */
  const GRAPH = [
    ['c', '//! Commit graph layout and lane colours.', 'use gpui::{hsla, px, Hsla, Pixels};', '', 'use crate::history::{Commit, CommitId};'],
    ['mb', 'use crate::theme::Palette;'],
    ['c', '', 'const LANE_W: f32 = 14.0;', 'const DOT_R: f32 = 4.5;', 'const LINE_W: f32 = 2.0;'],
    ['ma', '/// Runs of idle lanes shorter than this are never collapsed.', 'const IDLE_RUN: usize = 3;'],
    ['c', '', 'pub struct GraphLayout {', '    pub rows: Vec<GraphRow>,', '    pub lanes: usize,'],
    ['ma', '    collapse: Option<CollapseMap>,'],
    ['c', '}', '', 'impl GraphLayout {', '    pub fn lane_x(&self, col: usize) -> f32 {'],
    ['x', 'c1', { base: ['        col as f32 * LANE_W + LANE_W / 2.0'], a: ['        match self.collapse.as_ref() {', '            Some(map) => map.x_for(col),', '            None => col as f32 * LANE_W + LANE_W / 2.0,', '        }'], b: ['        col as f32 * LANE_W + LANE_W / 2.0'] }],
    ['c', '    }', '', '    /// Golden-angle hue rotation keeps neighbouring lanes distinct.'],
    ['x', 'c2', { base: ['    pub fn lane_color(col: usize, l: f32) -> Hsla {', '        let hue = (col as f32 * 137.508) % 360.0;'], a: ['    pub fn lane_color(col: usize, theme: Theme) -> Hsla {', '        let hue = (col as f32 * 137.508) % 360.0;', '        let l = if theme.is_dark() { 0.62 } else { 0.48 };'], b: ['    pub fn lane_color(col: usize, palette: &Palette) -> Hsla {', '        let hue = (col as f32 * 137.508) % 360.0;', '        let l = palette.graph_lightness();'] }],
    ['c', '        hsla(hue / 360.0, 0.65, l, 1.0)', '    }', '}', '', 'pub struct GraphRow {', '    pub id: CommitId,', '    pub col: usize,', '    pub merge: bool,', '}', '', 'fn paint_row(cx: &mut Paint, row: &GraphRow, layout: &GraphLayout) {', '    for edge in row.edges() {', '        let (x0, x1) = (layout.lane_x(edge.from), layout.lane_x(edge.to));'],
    ['x', 'c3', { base: ['        paint_line(cx, x0, x1, 2.0);'], a: ['        if x0 > clip_right && x1 > clip_right {', '            continue; // fully clipped, skip the path', '        }', '        paint_curve(cx, x0, x1, LINE_W);'], b: ['        paint_curve(cx, x0, x1, LINE_W);', '        paint_dot(cx, x1, DOT_R);'] }],
    ['c', '    }', '}']
  ];
  const SIDEBAR = [
    ['c', 'use gpui::*;', '', 'pub struct Sidebar {', '    filter: Model<String>,'],
    ['mb', '    tags: Vec<TagItem>,'],
    ['c', '    sections: Vec<Section>,', '    width: Pixels,', '}', '', 'impl Sidebar {'],
    ['x', 'c1', { base: ['    fn row_height() -> Pixels { px(24.0) }'], a: ['    fn row_height() -> Pixels { px(26.0) }'], b: ['    fn row_height() -> Pixels { px(28.0) }'] }],
    ['c', '', '    fn is_collapsed(&self) -> bool {', '        self.width == Pixels::ZERO', '    }', '}']
  ];
  const freshFiles = () => [
    { path: 'src/history/graph.rs', kind: 'text', segs: GRAPH },
    { path: 'src/ui/sidebar.rs', kind: 'text', segs: SIDEBAR },
    { path: 'assets/logo.png', kind: 'binary' },
    { path: 'docs/old.md', kind: 'moddel' },
    { path: 'scripts/current-link', kind: 'symlink' },
    { path: 'vendor/libgit2', kind: 'submodule' },
    { path: 'assets/palette.dat', kind: 'nottext' },
    { path: 'docs/palette.md', kind: 'added' },
    { path: 'docs/legacy.md', kind: 'delboth' }
  ];
  const PLANNED = '<span class="planned-tag">Planned</span>';
  const WHOLE = { binary: 'assets/logo.png', modifydelete: 'docs/old.md', symlink: 'scripts/current-link', submodule: 'vendor/libgit2', nottext: 'assets/palette.dat', added: 'docs/palette.md', delboth: 'docs/legacy.md' };

  /* ---------- state ---------- */
  /* cancelAsk = "Discard your picks for {path}?"; ask = 'takeall' ("Replace your edits too?") | 'disk' ("Replace the changes on disk?"); diskChanged = the file changed after the merge wrote it; load = 'loading' | 'failed' | null */
  const S = { files: [], idx: 0, showBase: false, rebase: false, narrow: false, cancelAsk: false, ask: null, askSide: null, diskChanged: false, diskOk: false, load: null, focus: 0, allDone: false, key: 'unresolved' };
  const cur = () => S.files[S.idx];
  const sides = () => (S.rebase ? { L: UPSTREAM, R: REPLAYED } : { L: MAIN, R: FEAT });
  const same = (x, y) => x.length === y.length && x.every((v, i) => v === y[i]);
  /* oriented chunks of a file; in a rebase the left pane is feat/palette (new base) and the right pane is the replayed commit */
  function chunksOf(f) {
    if (f._ch) return f._ch;
    f._ch = f.segs.filter((s) => s[0] === 'x').map((s, i) => { const d = s[2]; return { id: s[1], n: i + 1, L: S.rebase ? d.b : d.a, R: S.rebase ? d.a : d.b, B: d.base }; });
    return f._ch;
  }
  function initFile(f) {
    if (f.kind !== 'text') { f.choice = null; f.done = false; return; }
    f.st = {};
    chunksOf(f).forEach((c) => {
      const st = { picks: [], custom: null, auto: false };
      if (same(c.L, c.B) && !same(c.R, c.B)) { st.picks = ['R']; st.auto = true; } /* auto-resolve: the left side did not change, take the right */
      else if (same(c.R, c.B) && !same(c.L, c.B)) { st.picks = ['L']; st.auto = true; }
      f.st[c.id] = st;
    });
    f.done = false; f.init = snap(f);
  }
  const snap = (f) => (f.kind === 'text' ? JSON.stringify(f.st) : JSON.stringify(f.choice));
  const dirty = (f) => f.kind === 'text' ? snap(f) !== f.init : f.choice !== null;
  const stateOf = (f, c) => { const st = f.st[c.id]; return st.custom !== null ? 'custom' : st.picks.length ? 'picked' : 'open'; };
  const leftOf = (f) => chunksOf(f).filter((c) => stateOf(f, c) === 'open').length;
  const linesOf = (f, c) => { const st = f.st[c.id]; return st.custom !== null ? (st.custom === '' ? [] : st.custom.split('\n')) : st.picks.flatMap((p) => (p === 'L' ? c.L : c.R)); };
  const plural = (n, w) => `${n} ${w}${n === 1 ? '' : 's'}`;

  /* ---------- header ---------- */
  function headHTML() {
    const f = cur(), total = S.files.length, nTxt = f.kind === 'text';
    const fsw = `<span class="fsw"><span>${S.idx + 1} of ${total} files</span><button class="btn btn-icon btn-ghost btn-sm" data-do="prevF" data-tip="Previous file" aria-label="Previous file">${ic('chevR', 14, 'rot180')}</button><button class="btn btn-icon btn-ghost btn-sm" data-do="nextF" data-tip="Next file" aria-label="Next file">${ic('chevR', 14)}</button></span>`;
    const back = `<button class="btn btn-sm btn-ghost" data-do="back" data-tip="Back to Changes" data-kbd="Esc" aria-label="Back to Changes">${ic('chevR', 12, 'rot180')}Changes</button>`;
    const path = `<span class="cfe-path" title="${esc(f.path)}">${esc(f.path)}</span>`;
    /* "Open in merge tool" is not shipped (the app header has no such button): kept as Planned */
    const tool = `<button class="btn btn-sm btn-ghost" data-do="tool">${ic('external', 12)}Open in merge tool ${PLANNED}</button>`;
    const ovf = `<button class="btn btn-icon btn-ghost btn-sm" data-do="hmenu" data-tip="More" aria-label="More: files and merge tool">${ic('chevD', 14)}</button>`;
    if (!nTxt) return `${back}${path}${S.narrow ? ovf : tool + '<span class="vr"></span>' + fsw}`; /* whole-file cards: no count, no conflict nav, no Show base */
    const n = chunksOf(f).length, left = leftOf(f);
    const count = `<span class="cfe-count${left ? '' : ' ok'}">${left ? `<b>${plural(n, 'conflict')}</b> · ${left} left` : `<b>${plural(n, 'conflict')}</b> · none left`}</span>`;
    const nav = `<button class="btn btn-icon btn-sm" data-do="prevC" data-tip="Previous conflict" data-kbd="Alt+↑" aria-label="Previous conflict">${ic('up', 14)}</button><button class="btn btn-icon btn-sm" data-do="nextC" data-tip="Next conflict" data-kbd="Alt+↓" aria-label="Next conflict">${ic('down', 14)}</button>`;
    const base = `<span class="sw switch${S.showBase ? ' on' : ''}" role="switch" aria-checked="${S.showBase}" tabindex="0" data-do="base"><span class="track"></span><span>Show base</span></span>`;
    return `${back}${path}${count}<span class="vr"></span>${nav}<span class="vr"></span>${base}${S.narrow ? ovf : tool + '<span class="vr"></span>' + fsw}`;
  }
  const noteHTML = () => (S.rebase && cur().kind === 'text' ? `<div class="banner info cfe-note" role="status">${ic('info', 16)}<div style="min-width:0">During a rebase, the left side is the branch you rebase onto and the right side is your commit. ${PLANNED}</div></div>` : '')
    + (S.diskChanged && cur().kind === 'text' ? `<div class="cfe-disk" id="cfe-disk-banner">${ic('warn', 16)}<span>This file was changed after the merge (by rerere or another editor). Saving replaces those changes.</span></div>` : '');

  /* ---------- side panes ---------- */
  const paneHead = (cls, p) => `<div class="cfe-ph ${cls}"><span class="sdot" aria-hidden="true"></span><span class="nm">${cls === 'B' ? '' : ic('branch', 12)}${esc(p.name)}</span><span class="tg">${p.tag === 'merge-base' ? 'merge-base' : '(' + esc(p.tag) + ')'}</span><span class="hash">${p.hash}</span>${cls === 'B' ? '' : `<span class="subj" title="${esc(p.subj)}">${esc(p.subj)}</span>`}</div>`;
  const cell = (cls, ln, text, extra) => `<div class="cfe-cell ${cls}"${extra || ''}><span class="ln">${ln == null ? '' : ln}</span><span class="c"${text ? ` title="${esc(text.trim())}"` : ''}>${text == null ? '' : esc(text)}</span></div>`;
  const checkHTML = (f, c, side, o) => {
    const st = f.st[c.id], on = st.picks.includes(side), dis = st.custom !== null, ord = st.picks.length === 2 && on ? st.picks.indexOf(side) + 1 : 0;
    return `<span class="check${on ? ' on' : ''}${dis ? ' dis' : ''}" role="checkbox" aria-checked="${on}" ${dis ? 'aria-disabled="true"' : 'tabindex="0"'} data-pick="${c.id}" data-side="${side}" aria-label="Use these lines from ${esc(sides()[side].name)}"><span class="box">${on ? ic('check', 10) : ''}</span>Use these lines</span><span class="lab">${plural(o.n, 'line')}</span>${ord ? `<span class="ord" data-tip="Ticked ${ord === 1 ? 'first' : 'second'}: ${ord === 1 ? 'goes first' : 'goes after the first'} in the result" aria-label="Order ${ord}">${ord}</span>` : ''}`;
  };
  function topHTML() {
    const f = cur(), sd = sides(), p3 = S.showBase, cols = p3 ? ['L', 'B', 'R'] : ['L', 'R'];
    let h = `<div class="cfe-grid ${p3 ? 'p3' : 'p2'}">` + cols.map((k) => paneHead(k, k === 'B' ? BASEC : sd[k])).join('');
    const num = { L: 0, R: 0, B: 0 };
    const sideOf = (t) => (t === 'ma' ? (S.rebase ? 'R' : 'L') : (S.rebase ? 'L' : 'R'));
    f.segs.forEach((s) => {
      if (s[0] === 'c') s.slice(1).forEach((t) => { num.L++; num.R++; num.B++; h += `<div class="cfe-row">${cols.map((k) => cell('', num[k], t)).join('')}</div>`; });
      else if (s[0] === 'ma' || s[0] === 'mb') s.slice(1).forEach((t) => { const own = sideOf(s[0]); num[own]++; h += `<div class="cfe-row">${cols.map((k) => (k === own ? cell(own === 'L' ? 'mA' : 'mB', num[k], t) : cell('fill', null, null))).join('')}</div>`; });
      else {
        const c = chunksOf(f).find((x) => x.id === s[1]), st = f.st[c.id], state = stateOf(f, c), done = state !== 'open', foc = chunksOf(f).indexOf(c) === S.focus ? ' focus' : '';
        const n = Math.max(1, c.L.length, c.R.length, c.B.length), xc = `x${done ? ' done' : ''}${foc}`;
        const chip = st.custom !== null ? '<span class="chip acc">Edited</span>' : st.auto && st.picks.length === 1 ? '<span class="chip ok" data-tip="One side equals the base, so the other side was taken automatically">Auto</span>' : state === 'open' ? '<span class="chip warn">Unresolved</span>' : '';
        const hc = (k) => {
          const extra = ` data-c="${c.id}"`;
          if (k === 'B') return `<div class="cfe-cell xh baseh ${xc} top"${extra}>Base<span class="lab">${plural(c.B.length, 'line')}</span></div>`;
          const cn = k === cols[0] ? `<b>Conflict ${c.n}</b>` : '';
          const reset = st.custom !== null && k === 'R' ? `<button class="reset" data-do="reset" data-c="${c.id}">Reset to picks</button>` : '';
          const note = st.custom !== null && k === 'L' ? '<span class="lab">Edited in the result</span>' : '';
          return `<div class="cfe-cell xh ${xc} top"${extra}>${cn}${checkHTML(f, c, k, { n: (k === 'L' ? c.L : c.R).length })}${note}${k === 'R' ? chip : ''}${reset}</div>`;
        };
        h += `<div class="cfe-row">${cols.map(hc).join('')}</div>`;
        const lines = { L: c.L, R: c.R, B: c.B };
        for (let i = 0; i < n; i++) {
          h += `<div class="cfe-row">${cols.map((k) => { const t = lines[k][i]; const bot = i === n - 1 ? ' bot' : ''; if (t === undefined) return cell(`fill x ${xc}${bot}`, null, null, ` data-c="${c.id}"`); num[k]++; return cell(`x${k} ${xc}${bot}`, num[k], t, ` data-c="${c.id}"`); }).join('')}</div>`;
        }
      }
    });
    return h + '</div>';
  }

  /* ---------- result ---------- */
  function resultHTML() {
    const f = cur(), sd = sides(); let ln = 0, h = '';
    const plain = (t) => { ln++; return `<div class="cfe-rl"><span class="ln">${ln}</span><span class="c">${esc(t)}</span></div>`; };
    f.segs.forEach((s) => {
      if (s[0] !== 'x') { s.slice(1).forEach((t) => { h += plain(t); }); return; }
      const c = chunksOf(f).find((x) => x.id === s[1]), st = f.st[c.id], state = stateOf(f, c), lines = linesOf(f, c), start = ln + 1, foc = chunksOf(f).indexOf(c) === S.focus ? ' focus' : '';
      let chip, extra = '';
      if (state === 'custom') chip = '<span class="chip acc">Edited</span>';
      else if (state === 'open') chip = '<span class="chip warn">Unresolved</span>';
      else if (st.picks.length === 2) { chip = `<span class="chip acc">Both</span>`; extra = `<span class="muted">${esc(sd[st.picks[0]].name)} first</span><button class="btn btn-icon btn-ghost btn-xs swapbtn" data-do="swap" data-c="${c.id}" data-tip="Swap order" aria-label="Swap order">${ic('undo', 12)}</button>`; }
      else chip = `<span class="chip ${st.auto ? 'ok' : 'acc'}">${st.auto ? 'Auto · ' : ''}${esc(sd[st.picks[0]].name)}</span>`;
      const reset = state === 'custom' ? '<button class="reset" data-do="reset" data-c="' + c.id + '">Reset to picks</button>' : '';
      const editBtn = `<button class="btn btn-xs btn-ghost" data-do="edit" data-c="${c.id}" data-tip="Edit the result by hand" data-kbd="F2" aria-label="Edit the result by hand">${ic('edit', 12)}</button>`;
      const nn = Math.max(1, lines.length);
      h += `<div class="cfe-rc ${state === 'open' ? '' : 'done '}${state === 'custom' ? 'custom ' : ''}${foc}" data-rc="${c.id}" data-c="${c.id}"><div class="cfe-rch"><span class="n">Conflict ${c.n}</span>${chip}${extra}<span class="od-fill"></span>${reset}${editBtn}</div><div class="cfe-rcb"><div class="cfe-gut" data-start="${start}">${Array.from({ length: nn }, (_, i) => start + i).join('\n')}</div><div class="cfe-edit" contenteditable="true" spellcheck="false" role="textbox" aria-multiline="true" aria-label="Result of conflict ${c.n}" data-ed="${c.id}" data-ph="${state === 'open' ? 'Pick a side above, or type here' : 'Type the lines for this conflict'}">${esc(lines.join('\n'))}</div></div></div>`;
      ln += lines.length;
    });
    return h;
  }
  const resHead = () => { const f = cur(); const total = f.segs.reduce((a, s) => a + (s[0] === 'x' ? 0 : s.length - 1), 0) + chunksOf(f).reduce((a, c) => a + linesOf(f, c).length, 0); return `<b>Result</b><span class="muted">Edit it directly. The picks above rebuild it · ${plural(total, 'line')}</span>`; };

  /* ---------- footer ---------- */
  function footHTML() {
    const f = cur();
    if (S.cancelAsk) return `<div class="ask">${ic('warn', 16)}<span>Discard your picks for ${esc(f.path)}?</span></div><span class="od-fill"></span><button class="btn btn-danger" data-do="discard">Discard</button><button class="btn" data-do="keep" data-autofocus>Keep editing</button>`;
    if (S.ask) return `<div class="ask">${ic('warn', 16)}<span>${S.ask === 'disk' ? 'Replace the changes on disk?' : 'Replace your edits too?'}</span></div><span class="od-fill"></span><button class="btn btn-danger" data-do="replace">Replace</button><button class="btn" data-do="keep" data-autofocus>Keep editing</button>`;
    const sd = sides();
    if (f.kind !== 'text') {
      const ok = !!f.choice;
      return `<span class="st ${ok ? 'ok' : 'warn'}">${ok ? ic('check', 14) + 'All conflicts resolved' : ic('warn', 14) + 'Choose a version'}</span><span class="od-fill"></span><button class="btn btn-ghost" data-do="cancel">Cancel</button>${ok ? '' : '<span data-tip="Resolve every conflict first">'}<button class="btn btn-primary" data-do="save"${ok ? '' : ' disabled'}>Save and mark resolved</button>${ok ? '' : '</span>'}`;
    }
    const left = leftOf(f);
    return `<span class="st ${left ? 'warn' : 'ok'}">${left ? ic('warn', 14) + plural(left, 'conflict') + ' left' : ic('check', 14) + 'All conflicts resolved'}</span><span class="od-fill"></span><button class="btn btn-icon btn-ghost" data-do="fmenu" data-tip="More resolve options" aria-label="More resolve options" aria-haspopup="menu">${ic('chevD', 14)}</button><button class="btn btn-ghost" data-do="cancel">Cancel</button>${left ? '<span data-tip="Resolve every conflict first">' : ''}<button class="btn btn-primary" data-do="save"${left ? ' disabled' : ''}>Save and mark resolved</button>${left ? '</span>' : ''}`;
  }

  /* ---------- non-text conflicts ---------- */
  /* whole-file card (conflict_editor.rs render_whole): title, note, one or two radio cards (label + one muted line). No previews: the app draws none. */
  function choiceHTML() {
    const f = cur(), sd = sides(), kind = f.kind, absent = kind === 'moddel' ? (S.rebase ? 'L' : 'R') : kind === 'added' ? (S.rebase ? 'R' : 'L') : null; /* the side without the file */
    const nm = (k) => sd[k].name, branchOf = (k) => (S.rebase && k === 'L' ? 'feat/palette (new base)' : nm(k));
    const title = kind === 'moddel' ? `${f.path} was deleted in ${branchOf(absent)}` : kind === 'added' ? `${f.path} was added only in ${branchOf(absent === 'L' ? 'R' : 'L')}` : kind === 'delboth' ? `${f.path} was deleted on both sides` : `${f.path} was changed on both sides`;
    const NOTE = { binary: "Binary files can't be merged line by line. Keep one version: GitWarp writes it to the working tree and marks the file resolved.", symlink: "A symbolic link can't be merged line by line. Keep one version: GitWarp writes it to the working tree and marks the file resolved.", submodule: "A submodule can't be merged line by line. Keep one version: GitWarp records it and marks the file resolved.", nottext: "This file is not UTF-8 text, so GitWarp can't merge it line by line. Keep one version: GitWarp writes it to the working tree and marks the file resolved.", moddel: 'One side deleted the file and the other changed it. Choose which one stays: GitWarp applies it and marks the file resolved.', added: 'Only one side added this file. Choose whether it stays: GitWarp applies it and marks the file resolved.', delboth: 'Both sides deleted this file. Confirm the deletion: GitWarp removes it and marks the file resolved.' };
    const KIND = { binary: 'Binary file', symlink: 'Symbolic link', submodule: 'Submodule', nottext: 'Not UTF-8 text' };
    const line = (k) => (kind === 'delboth' || absent === k && kind === 'moddel' || absent === k && kind === 'added' ? 'The file is removed' : KIND[kind] || `The file stays as ${branchOf(k)} left it`);
    const label = (k) => (kind === 'delboth' || (kind === 'added' && absent === k) ? 'Delete the file' : kind === 'added' ? 'Keep the file' : kind === 'moddel' && absent === k ? `Delete the file (as in ${branchOf(k)})` : k === 'L' ? `Keep ${branchOf(k)}'s version` : `Use ${branchOf(k)}'s version`);
    const card = (k) => `<button type="button" class="cfe-opt" role="radio" aria-checked="${f.choice === k}" data-do="choose" data-v="${k}"><span class="t"><span class="radio-dot" aria-hidden="true"></span>${esc(label(k))}</span><span class="mt"><span>${esc(line(k))}</span></span></button>`;
    return `<div class="cfe-card-wrap"><div class="cfe-choice"><h2>${esc(title)}</h2><p>${esc(NOTE[kind])}</p><div class="cfe-cards${kind === 'delboth' ? ' one' : ''}" role="radiogroup" aria-label="Version to keep">${card('L')}${kind === 'delboth' ? '' : card('R')}</div></div></div>`;
  }
  const failedHTML = () => `<div class="cfe-failed"><div class="msg">Could not read the conflict: failed to read stage 2 of ${esc(cur().path)} from the index</div><div class="od-row" style="gap:8px"><button class="btn" data-do="tool-none">Take ${esc(sides().L.name)}</button><button class="btn" data-do="tool-none">Take ${esc(sides().R.name)}</button></div></div>`;
  const loadingHTML = () => `<div class="cfe-loading" role="status" aria-label="Loading the conflict…"><div class="skel" style="width:320px"></div><div class="skel" style="width:260px"></div><div class="skel" style="width:290px"></div></div>`;
  const doneHTML = () => `<div class="cfe-done"><span style="color:var(--success)">${ic('check', 28)}</span><b>All conflicted files are resolved</b><span>Every file was written and staged. Commit to finish the ${S.rebase ? 'rebase step' : 'merge'}.</span><button class="btn btn-primary" data-do="home">Back to Changes</button></div>`;

  /* ---------- render ---------- */
  const part = (n) => root.querySelector(`[data-r="${n}"]`);
  const keepFocus = (fn) => { const a = document.activeElement; let sel = null; if (a && root.contains(a)) { if (a.dataset.pick) sel = `[data-pick="${a.dataset.pick}"][data-side="${a.dataset.side}"]`; else if (a.dataset.do) sel = `[data-do="${a.dataset.do}"]${a.dataset.c ? `[data-c="${a.dataset.c}"]` : ''}`; } fn(); if (sel) { const el = root.querySelector(sel); if (el && !el.classList.contains('dis')) el.focus({ preventScroll: true }); } };
  const mountHead = () => { const el = part('head'); if (el) el.innerHTML = headHTML(); };
  const mountFoot = () => { const el = part('foot'); if (el) el.innerHTML = footHTML(); };
  const mountTop = () => { const el = part('top'); if (el) el.innerHTML = topHTML(); };
  const mountRes = () => { const el = part('res'); if (el) { el.innerHTML = resultHTML(); fixEditable(); } const h = part('resh'); if (h) h.innerHTML = resHead(); };
  const fixEditable = () => root.querySelectorAll('.cfe-edit').forEach((e) => { e.contentEditable = 'plaintext-only'; if (e.contentEditable !== 'plaintext-only') e.contentEditable = 'true'; });
  function render() {
    const f = cur();
    if (S.allDone) { root.innerHTML = `<div class="cfe-head" data-r="head"></div>${doneHTML()}`; part('head').innerHTML = `<button class="btn btn-sm btn-ghost" data-do="home">${ic('chevR', 12, 'rot180')}Changes</button><span class="cfe-path">All conflicts resolved</span>`; return; }
    root.innerHTML = `<div class="cfe-head" data-r="head"></div>${noteHTML()}${S.load === 'loading' ? loadingHTML() : S.load === 'failed' ? failedHTML() : f.kind === 'text' ? '<div class="cfe-top" data-r="top" tabindex="0" aria-label="Conflict panes"></div><div class="cfe-res"><div class="cfe-resh" data-r="resh"></div><div class="cfe-rs" data-r="res"></div></div>' : choiceHTML()}<div class="cfe-foot" data-r="foot"></div>`;
    mountHead(); mountFoot();
    if (f.kind === 'text' && !S.load) { mountTop(); mountRes(); }
    const af = root.querySelector('[data-autofocus]'); if (af) af.focus({ preventScroll: true });
  }
  const refresh = () => keepFocus(() => { mountHead(); mountFoot(); if (cur().kind === 'text' && !S.load) { mountTop(); mountRes(); } });

  /* ---------- actions ---------- */
  const nChunks = () => chunksOf(cur()).length;
  function setFocus(i, scroll) {
    if (cur().kind !== 'text' || !nChunks()) return;
    S.focus = (i + nChunks()) % nChunks(); const id = chunksOf(cur())[S.focus].id;
    root.querySelectorAll('[data-c]').forEach((e) => e.classList.toggle('focus', e.dataset.c === id));
    if (!scroll) return;
    const top = root.querySelector('.cfe-top'), hdr = top && top.querySelector(`.cfe-cell.xh[data-c="${id}"]`); if (hdr) top.scrollTop = Math.max(0, hdr.offsetTop - 34 - 3 * 22); /* below the 34px sticky pane header, 3 context lines above the chunk box */
    const rs = root.querySelector('.cfe-rs'), rc = rs && rs.querySelector(`[data-rc="${id}"]`); if (rc) rs.scrollTop = Math.max(0, rc.offsetTop - 3 * 22);
  }
  function toggle(id, side) {
    const f = cur(), st = f.st[id]; if (st.custom !== null) return;
    const i = st.picks.indexOf(side); if (i >= 0) st.picks.splice(i, 1); else st.picks.push(side);
    st.auto = false; S.focus = chunksOf(f).findIndex((c) => c.id === id); refresh(); setFocus(S.focus, false);
  }
  const hasCustom = (f) => chunksOf(f).some((c) => f.st[c.id].custom !== null);
  const requestTakeAll = (side) => { const f = cur(); if (hasCustom(f)) { S.ask = 'takeall'; S.askSide = side; mountFoot(); const k = root.querySelector('[data-autofocus]'); if (k) k.focus({ preventScroll: true }); } else takeAll(side); };
  const takeAll = (side) => { const f = cur(); chunksOf(f).forEach((c) => { f.st[c.id] = { picks: [side], custom: null, auto: false }; }); refresh(); toast(`Took ${sides()[side].name} for every conflict in ${f.path}`, 'info'); };
  /* menu helper: G.menu plus the Planned tag on items flagged planned:true */
  const menuP = (pt, items) => { const close = G.menu(win, pt, items); win.querySelectorAll('.menu .menu-item').forEach((el) => { const it = items[+el.dataset.i]; if (it && it.planned && !el.querySelector('.planned-tag')) el.insertAdjacentHTML('beforeend', PLANNED); }); return close; };
  const navTo = (path) => { location.href = path; };
  const backHref = () => 'changes.html?op=' + (S.rebase ? 'rebase' : 'merge');
  const goBack = () => { if (dirty(cur())) { S.cancelAsk = true; mountFoot(); const k = root.querySelector('[data-autofocus]'); if (k) k.focus({ preventScroll: true }); } else navTo(backHref()); };
  function stepFile(d) { S.idx = (S.idx + d + S.files.length) % S.files.length; S.focus = 0; S.cancelAsk = false; render(); }
  function save() {
    const f = cur(); if (f.kind === 'text' ? leftOf(f) > 0 : !f.choice) return;
    if (f.kind === 'text' && S.diskChanged && !S.diskOk) { S.ask = 'disk'; mountFoot(); const k = root.querySelector('[data-autofocus]'); if (k) k.focus({ preventScroll: true }); return; }
    f.done = true; S.cancelAsk = false; toast(`Resolved ${f.path}`, 'ok');
    const next = S.files.findIndex((x, i) => !x.done && i > S.idx) >= 0 ? S.files.findIndex((x, i) => !x.done && i > S.idx) : S.files.findIndex((x) => !x.done);
    if (next < 0) S.allDone = true; else { S.idx = next; S.focus = 0; }
    render();
  }
  const at = (el) => { const r = el.getBoundingClientRect(); return { clientX: r.left, clientY: r.bottom + 2 }; };
  const doAct = (a, el, e) => {
    const f = cur();
    if (a === 'back' || a === 'cancel') return goBack();
    if (a === 'home') return navTo(backHref());
    if (a === 'keep') { S.cancelAsk = false; S.ask = null; return mountFoot(); }
    if (a === 'replace') { const k = S.ask, sd = S.askSide; S.ask = null; if (k === 'disk') { S.diskOk = true; return save(); } takeAll(sd); return mountFoot(); }
    if (a === 'edit') { const ed = root.querySelector(`[data-ed="${el.dataset.c}"]`); if (ed) { ed.focus(); const r = document.createRange(); r.selectNodeContents(ed); r.collapse(false); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(r); } return; }
    if (a === 'tool-none') return;
    if (a === 'discard') return navTo(backHref());
    if (a === 'prevC') return setFocus(S.focus - 1, true);
    if (a === 'nextC') return setFocus(S.focus + 1, true);
    if (a === 'base') { S.showBase = !S.showBase; return keepFocus(() => { mountHead(); mountTop(); }); }
    if (a === 'tool') return toast(`Opened ${f.path} in your merge tool`, 'info');
    if (a === 'prevF') return stepFile(-1);
    if (a === 'nextF') return stepFile(1);
    if (a === 'hmenu') return menuP(at(el), [{ label: `File ${S.idx + 1} of ${S.files.length}`, disabled: true }, { label: 'Previous file', icon: 'chevR', onClick: () => stepFile(-1) }, { label: 'Next file', icon: 'chevR', onClick: () => stepFile(1) }, { sep: 1 }, { label: 'Open in merge tool', icon: 'external', planned: true, onClick: () => doAct('tool') }]);
    if (a === 'fmenu') { const sd = sides(); return menuP(at(el), [{ label: `Take ${sd.L.name} for all`, icon: 'check', onClick: () => requestTakeAll('L') }, { label: `Take ${sd.R.name} for all`, icon: 'check', onClick: () => requestTakeAll('R') }]); }
    if (a === 'save') return save();
    if (a === 'choose') { f.choice = el.dataset.v; return keepFocus(() => { mountFoot(); const c = root.querySelector('.cfe-card-wrap'); if (c) { const y = c.scrollTop; c.outerHTML = choiceHTML(); root.querySelector('.cfe-card-wrap').scrollTop = y; } }); }
    if (a === 'swap') { f.st[el.dataset.c].picks.reverse(); return refresh(); }
    if (a === 'reset') { f.st[el.dataset.c].custom = null; return refresh(); }
  };
  root.addEventListener('click', (e) => {
    const pk = e.target.closest('[data-pick]'); if (pk) return toggle(pk.dataset.pick, pk.dataset.side);
    const d = e.target.closest('[data-do]'); if (d && !d.disabled) return doAct(d.dataset.do, d, e);
    const cc = e.target.closest('[data-c]'); if (cc) { const i = chunksOf(cur()).findIndex((c) => c.id === cc.dataset.c); if (i >= 0 && i !== S.focus) setFocus(i, false); }
  });
  root.addEventListener('keydown', (e) => {
    const t = e.target; if ((e.key === ' ' || e.key === 'Enter') && t.matches && t.matches('[role=checkbox],[role=switch]')) { e.preventDefault(); t.click(); }
  });
  root.addEventListener('focusin', (e) => { const ed = e.target.closest && e.target.closest('[data-ed]'); if (ed) { const i = chunksOf(cur()).findIndex((c) => c.id === ed.dataset.ed); if (i >= 0 && i !== S.focus) setFocus(i, false); } });
  /* typing in the result marks the chunk Custom: only chips, counters and the checkbox state refresh, the caret stays put */
  root.addEventListener('input', (e) => {
    const ed = e.target.closest('[data-ed]'); if (!ed) return;
    const f = cur(), st = f.st[ed.dataset.ed], text = ed.innerText.replace(/\n$/, '');
    st.custom = text; st.auto = false;
    const g = ed.parentNode.querySelector('.cfe-gut'), n = Math.max(1, text === '' ? 1 : text.split('\n').length), s0 = +g.dataset.start; g.textContent = Array.from({ length: n }, (_, i) => s0 + i).join('\n');
    const rc = ed.closest('.cfe-rc'); rc.classList.add('done', 'custom');
    const chunk = chunksOf(f).find((c) => c.id === ed.dataset.ed);
    const chipEl = rc.querySelector('.chip'); if (chipEl) { chipEl.className = 'chip acc'; chipEl.textContent = 'Edited'; }
    rc.querySelectorAll('.muted,.swapbtn').forEach((x) => x.remove());
    if (!rc.querySelector('.reset')) rc.querySelector('.cfe-rch').insertAdjacentHTML('beforeend', `<button class="reset" data-do="reset" data-c="${chunk.id}">Reset to picks</button>`);
    mountHead(); mountFoot(); const top = root.querySelector('.cfe-top'), y = top.scrollTop; mountTop(); top.scrollTop = y; const rh = part('resh'); if (rh) rh.innerHTML = resHead();
  });
  document.addEventListener('keydown', (e) => {
    if (win.querySelector('.menu')) return;
    if ((e.key === 'F2' || e.key === 'Enter') && !S.allDone && cur().kind === 'text' && !S.load && !(e.target.closest && e.target.closest('[data-ed],button,[role=checkbox],[role=switch],input,textarea'))) { /* F2 / Enter edits the focused conflict's result */
      e.preventDefault(); const c = chunksOf(cur())[S.focus], ed = c && root.querySelector(`[data-ed="${c.id}"]`); if (ed) { ed.focus(); const r = document.createRange(); r.selectNodeContents(ed); r.collapse(false); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(r); } return;
    }
    if (e.key === 'Escape') { e.preventDefault(); if (S.cancelAsk) { S.cancelAsk = false; return mountFoot(); } return S.allDone ? navTo(backHref()) : goBack(); }
    if (S.allDone) return;
    if (e.ctrlKey && (e.key === 's' || e.key === 'S')) { e.preventDefault(); return save(); }
    if (!e.altKey) return;
    if (e.key === 'ArrowUp') { e.preventDefault(); return setFocus(S.focus - 1, true); }
    if (e.key === 'ArrowDown') { e.preventDefault(); return setFocus(S.focus + 1, true); }
    if ((e.key === '1' || e.key === '2') && cur().kind === 'text' && nChunks()) { e.preventDefault(); toggle(chunksOf(cur())[S.focus].id, e.key === '1' ? 'L' : 'R'); }
  });

  /* ---------- scenarios ---------- */
  const groups = [
    ['Text file', [['unresolved', 'Unresolved'], ['picking', 'Picking'], ['resolved', 'Resolved'], ['custom', 'Custom edit'], ['base', 'Show base']]],
    ['Whole file', [['binary', 'Binary'], ['symlink', 'Symlink'], ['submodule', 'Submodule'], ['nottext', 'Not UTF-8'], ['modifydelete', 'Deleted in a branch'], ['added', 'Added only in a branch'], ['delboth', 'Deleted on both sides']]],
    ['Confirms', [['cancel', 'Discard picks'], ['takeall', 'Replace your edits'], ['disk', 'Replace on disk']]],
    ['Load', [['loading', 'Loading'], ['failed', 'Could not read']]],
    ['Notices', [['n-disk', 'Changed on disk'], ['n-conflict', 'Not the same conflict'], ['n-mode', 'File mode kept']]],
    ['Other', [['rebase', 'Rebase <span class="planned-tag">Planned</span>'], ['narrow', 'Narrow 1000×700']]]
  ];
  const scen = document.getElementById('scen');
  scen.innerHTML = groups.map(([l, items]) => `<div class="grp-s"><span class="l">${l}</span><div class="seg sm" role="group" aria-label="${l}">${items.map(([k, t]) => `<button data-s="${k}" aria-pressed="false">${t}</button>`).join('')}</div></div>`).join('');
  const setPicks = (f, id, picks) => { f.st[id] = { picks, custom: null, auto: false }; };
  /* the chunk each text scenario is about: it is focused and scrolled to the top of every pane on load */
  const FOCUS = { unresolved: 0, picking: 0, resolved: 0, base: 0, rebase: 0, narrow: 0, custom: 1, takeall: 1, disk: 0 };
  function apply(key, o) {
    o = o || {};
    S.key = key; S.rebase = key === 'rebase' || !!o.rebase; S.narrow = key === 'narrow'; S.showBase = key === 'base'; S.cancelAsk = false; S.ask = null; S.askSide = null; S.diskChanged = key === 'disk'; S.diskOk = false; S.load = key === 'loading' ? 'loading' : key === 'failed' ? 'failed' : null; S.allDone = false; S.focus = FOCUS[key] || 0; S.idx = 0;
    S.files = freshFiles(); S.files.forEach(initFile);
    const g = S.files[0];
    if (key === 'picking' || key === 'base' || key === 'cancel' || key === 'narrow') { setPicks(g, 'c1', ['L', 'R']); setPicks(g, 'c3', ['R']); } /* chunk 1 ticked on both sides (main first), chunk 2 left open right below it, chunk 3 ticked */
    if (key === 'resolved') { setPicks(g, 'c2', ['L']); setPicks(g, 'c3', ['L', 'R']); }
    if (key === 'custom') { g.st.c2 = { picks: [], custom: ['    pub fn lane_color(col: usize, palette: &Palette) -> Hsla {', '        let hue = (col as f32 * 137.508) % 360.0;', '        let l = palette.graph_lightness().clamp(0.40, 0.70);'].join('\n'), auto: false }; setPicks(g, 'c3', ['R']); S.focus = 1; }
    if (key === 'cancel') { g.st.c3 = { picks: ['L'], custom: null, auto: false }; S.cancelAsk = true; } /* dirty: the file differs from the state git wrote */
    if (key === 'takeall') { g.st.c2 = { picks: [], custom: ['    pub fn lane_color(col: usize, palette: &Palette) -> Hsla {', '        let hue = (col as f32 * 137.508) % 360.0;'].join('\n'), auto: false }; setPicks(g, 'c1', ['L']); setPicks(g, 'c3', ['R']); S.ask = 'takeall'; S.askSide = 'R'; S.focus = 1; }
    if (key === 'disk') { setPicks(g, 'c1', ['L']); setPicks(g, 'c2', ['R']); setPicks(g, 'c3', ['L', 'R']); S.ask = 'disk'; }
    if (key === 'loading' || key === 'failed') { S.idx = 1; }
    if (WHOLE[key]) S.idx = S.files.findIndex((x) => x.path === WHOLE[key]);
    if (o.file != null) { const i = S.files.findIndex((f) => f.path === o.file); if (i >= 0) S.idx = i; }
    win.style.width = S.narrow ? '1000px' : '1200px'; win.style.height = S.narrow ? '700px' : '800px'; if (win._fit) win._fit();
    scen.querySelectorAll('button[data-s]').forEach((b) => { const on = b.dataset.s === key; b.classList.toggle('on', on); b.setAttribute('aria-pressed', on); });
    render();
    if (key === 'custom') { const ed = root.querySelector('[data-ed="c2"]'); if (ed) { ed.focus({ preventScroll: true }); const r = document.createRange(); r.selectNodeContents(ed); r.collapse(false); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(r); } }
    if (key in FOCUS && cur().kind === 'text' && !S.load) setFocus(S.focus, true); /* synchronous: the scroll is in place on the first frame */
  }
  scen.addEventListener('click', (e) => {
    const b = e.target.closest('[data-s]'); if (!b) return; const k = b.dataset.s, p = cur().path;
    if (k === 'n-disk') return toast(`${p} changed on disk — reopen it to see the changes`, 'warn');
    if (k === 'n-conflict') return toast(`${p} is no longer in the same conflict — reloaded`, 'warn');
    if (k === 'n-mode') return toast(`Resolved ${p}. Both sides changed the file mode; the current branch's mode was kept.`, 'ok');
    apply(k);
  });

  document.getElementById('notes').innerHTML = [
    ['Layout', 'The editor replaces the file lists and the diff pane of Changes. Header, two side panes (left main / HEAD, right feat/palette), the result pane (about 44% of the height) and a footer. Panes share one scroller, so chunk boxes line up row by row; missing lines are hatched.'],
    ['Checkboxes and order', '“Use these lines” sits in each side’s chunk header. One tick takes that side; two ticks keep both blocks in the order they were ticked (the numbered badge shows it, the swap button reverses it). Unticking removes that side. Alt+1 / Alt+2 toggle the left / right side of the focused chunk.'],
    ['Custom chunks', 'Typing in a result chunk makes it Custom: an Edited chip appears, the side checkboxes above are disabled and “Reset to picks” brings the last picks back. A Custom chunk counts as resolved, even when it is empty.'],
    ['Auto-resolve', 'When one side equals the base for a chunk, the other side is pre-ticked and the chunk carries an Auto chip. It stays editable: tick or untick as usual.'],
    ['Save', '“Save and mark resolved” writes the merged file and runs git add. It is disabled while any chunk is unresolved (tooltip: Resolve every conflict first). After saving, the editor opens the next conflicted file; after the last one it offers Back to Changes. Cancel asks before discarding picks or edits.'],
    ['Whole-file conflicts', 'Binary, symbolic link, submodule, not UTF-8 text, deleted in one branch, added only in one branch and deleted on both sides have no chunks: a title (“{path} was changed on both sides”, “…was deleted in {branch}”, “…was added only in {branch}”, “…was deleted on both sides”), a note and one or two radio cards (Keep main\'s version / Use feat/palette\'s version, Delete the file (as in {branch}), Keep the file / Delete the file) with one muted line each. The footer reads “Choose a version” until a card is picked, then “All conflicts resolved”.'],
    ['Confirms and notices', 'The footer swaps to a confirm: “Discard your picks for {path}?” (Discard / Keep editing), “Replace your edits too?” when Take {branch} for all would overwrite a hand edit, “Replace the changes on disk?” when the file changed after the merge wrote it (a warning bar under the header says so). Notices: “{path} changed on disk — reopen it to see the changes”, “{path} is no longer in the same conflict — reloaded”, and the saved notice with “Both sides changed the file mode; the current branch\'s mode was kept.”'],
    ['Editing a region', 'Each result region has an Edit button (tooltip “Edit the result by hand”, F2). F2 or Enter on the focused conflict starts the edit; typing makes the region Edited (placeholder “Type the lines for this conflict”).'],
    ['Planned', 'Not shipped, kept for design work: the rebase labelling and its note, and Open in merge tool in the header. The app has no rebase and no merge-tool button here.'],
    ['Rebase labelling', 'Sides are named by branch or commit, never ours/theirs. In a rebase the left side is the branch you rebase onto (feat/palette, upstream) and the right side is the commit being replayed (8d41f6e); a note under the header says so.'],
    ['Keyboard', 'Alt+↑ / Alt+↓ previous / next conflict · Alt+1 / Alt+2 toggle main / feat/palette on the focused chunk · Ctrl+S save · F2 / Enter edit the focused result · Esc back to Changes (asks first when something changed). The focused chunk has the focus ring in every pane and in the result.']
  ].map(([t, b]) => `<div class="n"><b>${t}</b>${b}</div>`).join('');

  const first = Q.get('s') && groups.some(([, it]) => it.some(([k]) => k === Q.get('s'))) ? Q.get('s') : 'unresolved';
  apply(first, { file: Q.get('file'), rebase: Q.get('op') === 'rebase' });
  G.fit(win);
})();
