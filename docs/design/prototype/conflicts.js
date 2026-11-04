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
  const sh = G.shell({ host: stage.querySelector('.winwrap'), w: 1200, h: 800, view: 'changes', statusInfo: '4 conflicted · 0 staged' });
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
    { path: 'docs/old.md', kind: 'moddel' }
  ];

  /* ---------- state ---------- */
  const S = { files: [], idx: 0, showBase: false, rebase: false, narrow: false, cancelAsk: false, focus: 0, allDone: false, key: 'unresolved' };
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
    const tool = `<button class="btn btn-sm btn-ghost" data-do="tool">${ic('external', 12)}Open in merge tool</button>`;
    const ovf = `<button class="btn btn-icon btn-ghost btn-sm" data-do="hmenu" data-tip="More" aria-label="More: files and merge tool">${ic('chevD', 14)}</button>`;
    if (!nTxt) {
      const kind = f.kind === 'binary' ? 'Binary file' : 'Modify / delete conflict';
      return `${back}${path}<span class="cfe-count">${kind}</span>${S.narrow ? ovf : tool + '<span class="vr"></span>' + fsw}`;
    }
    const n = chunksOf(f).length, left = leftOf(f);
    const count = `<span class="cfe-count${left ? '' : ' ok'}">${left ? `<b>${plural(n, 'conflict')}</b> · ${left} left` : `<b>${plural(n, 'conflict')}</b> · none left`}</span>`;
    const nav = `<button class="btn btn-icon btn-sm" data-do="prevC" data-tip="Previous conflict" data-kbd="Alt+↑" aria-label="Previous conflict">${ic('up', 14)}</button><button class="btn btn-icon btn-sm" data-do="nextC" data-tip="Next conflict" data-kbd="Alt+↓" aria-label="Next conflict">${ic('down', 14)}</button>`;
    const base = `<span class="sw switch${S.showBase ? ' on' : ''}" role="switch" aria-checked="${S.showBase}" tabindex="0" data-do="base"><span class="track"></span><span>Show base</span></span>`;
    return `${back}${path}${count}<span class="vr"></span>${nav}<span class="vr"></span>${base}${S.narrow ? ovf : tool + '<span class="vr"></span>' + fsw}`;
  }
  const noteHTML = () => (S.rebase && cur().kind === 'text' ? `<div class="banner info cfe-note" role="status">${ic('info', 16)}<div style="min-width:0">During a rebase, the left side is the branch you rebase onto and the right side is your commit.</div></div>` : '');

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
      else if (st.picks.length === 2) { chip = `<span class="chip acc">Both</span>`; extra = `<span class="muted">${esc(sd[st.picks[0]].name)} first</span><button class="btn btn-icon btn-ghost btn-xs swapbtn" data-do="swap" data-c="${c.id}" data-tip="Swap the order" aria-label="Swap the order of the two blocks">${ic('undo', 12)}</button>`; }
      else chip = `<span class="chip ${st.auto ? 'ok' : 'acc'}">${st.auto ? 'Auto · ' : ''}${esc(sd[st.picks[0]].name)}</span>`;
      const reset = state === 'custom' ? '<button class="reset" data-do="reset" data-c="' + c.id + '">Reset to picks</button>' : '';
      const nn = Math.max(1, lines.length);
      h += `<div class="cfe-rc ${state === 'open' ? '' : 'done '}${state === 'custom' ? 'custom ' : ''}${foc}" data-rc="${c.id}" data-c="${c.id}"><div class="cfe-rch"><span class="n">Conflict ${c.n}</span>${chip}${extra}<span class="od-fill"></span>${reset}</div><div class="cfe-rcb"><div class="cfe-gut" data-start="${start}">${Array.from({ length: nn }, (_, i) => start + i).join('\n')}</div><div class="cfe-edit" contenteditable="true" spellcheck="false" role="textbox" aria-multiline="true" aria-label="Result of conflict ${c.n}" data-ed="${c.id}" data-ph="Pick a side above, or type here">${esc(lines.join('\n'))}</div></div></div>`;
      ln += lines.length;
    });
    return h;
  }
  const resHead = () => { const f = cur(); const total = f.segs.reduce((a, s) => a + (s[0] === 'x' ? 0 : s.length - 1), 0) + chunksOf(f).reduce((a, c) => a + linesOf(f, c).length, 0); return `<b>Result</b><span class="muted">Edit it directly. The picks above rebuild it · ${plural(total, 'line')}</span>`; };

  /* ---------- footer ---------- */
  function footHTML() {
    const f = cur();
    if (S.cancelAsk) return `<div class="ask">${ic('warn', 16)}<span>Discard your ${f.kind === 'text' ? 'picks and edits' : 'choice'} for this file and go back?</span></div><span class="od-fill"></span><button class="btn" data-do="keep" data-autofocus>Keep editing</button><button class="btn btn-danger" data-do="discard">Discard and go back</button>`;
    const sd = sides();
    if (f.kind !== 'text') {
      const ok = !!f.choice;
      return `<span class="st ${ok ? 'ok' : 'warn'}">${ok ? ic('check', 14) + 'Ready to save' : ic('warn', 14) + 'Choose a version'}</span><span class="od-fill"></span><button class="btn btn-ghost" data-do="cancel">Cancel</button>${ok ? '' : '<span data-tip="Choose a version first">'}<button class="btn btn-primary" data-do="save"${ok ? '' : ' disabled'}>Save and mark resolved</button>${ok ? '' : '</span>'}`;
    }
    const left = leftOf(f);
    return `<span class="st ${left ? 'warn' : 'ok'}">${left ? ic('warn', 14) + plural(left, 'conflict') + ' left' : ic('check', 14) + 'All conflicts resolved'}</span><span class="od-fill"></span><button class="btn btn-icon btn-ghost" data-do="fmenu" data-tip="Take one side for the whole file" aria-label="More: take one side for all conflicts" aria-haspopup="menu">${ic('chevD', 14)}</button><button class="btn btn-ghost" data-do="cancel">Cancel</button>${left ? '<span data-tip="Resolve every conflict first">' : ''}<button class="btn btn-primary" data-do="save"${left ? ' disabled' : ''} data-tip="${left ? '' : 'Write the file and stage it (git add)'}">Save and mark resolved</button>${left ? '</span>' : ''}`;
  }

  /* ---------- non-text conflicts ---------- */
  function choiceHTML() {
    const f = cur(), sd = sides(), rb = S.rebase;
    const lName = rb ? 'feat/palette (new base)' : 'main', rName = rb ? `your commit ${REPLAYED.hash}` : 'feat/palette';
    const card = (side, title, thumb, meta) => `<button type="button" class="cfe-opt" role="radio" aria-checked="${f.choice === side}" data-do="choose" data-v="${side}"><span class="t"><span class="radio-dot" aria-hidden="true"></span>${esc(title)}</span>${thumb}<span class="mt">${meta}</span></button>`;
    if (f.kind === 'binary') {
      return `<div class="cfe-card-wrap"><div class="cfe-choice"><h2>${esc(f.path)} was changed on both sides</h2><p>Binary files can’t be merged line by line. Keep one version: GitWarp writes it to the working tree and marks the file resolved.</p>
        <div class="cfe-cards" role="radiogroup" aria-label="Version to keep">
        ${card('L', `Keep ${lName}’s version`, `<span class="cfe-thumb L" aria-hidden="true">PNG · 512×512</span>`, `<span>48 KB · 512×512</span><span>Last changed in <span class="mono">2f9b8a0</span> “chore: refresh logo”</span><span>Mira Okonkwo, 3 days ago</span>`)}
        ${card('R', `Use ${rName}’s version`, `<span class="cfe-thumb R" aria-hidden="true">PNG · 512×512</span>`, `<span>62 KB · 512×512</span><span>Last changed in <span class="mono">${rb ? REPLAYED.hash : FEAT.hash}</span> “${esc(rb ? REPLAYED.subj : 'feat(palette): add palette icon')}”</span><span>Jonas Berg, 2 days ago</span>`)}
        </div><div><button class="btn btn-sm" data-do="tool">${ic('external', 12)}Open in merge tool</button></div></div></div>`;
    }
    return `<div class="cfe-card-wrap"><div class="cfe-choice"><h2>${esc(f.path)} was ${rb ? 'deleted upstream and changed in your commit' : 'deleted in feat/palette and modified in main'}</h2><p>There is no text to merge. Either keep the modified file or delete it, as the other side did. The choice is written to the working tree and the file is marked resolved.</p>
      <div class="cfe-cards" role="radiogroup" aria-label="What to do with the file">
      ${card('L', rb ? 'Delete the file (as in feat/palette)' : `Keep ${lName}’s version`, `<span class="cfe-thumb ${rb ? 'del' : 'L'}" aria-hidden="true">${rb ? 'File deleted' : 'docs/old.md · 12 lines changed'}</span>`, rb ? `<span>feat/palette removed it in <span class="mono">${UPSTREAM.hash}</span> “docs: remove the old architecture notes”</span><span>Jonas Berg, 2 days ago</span>` : `<span>Modified in <span class="mono">${MAIN.hash}</span> “${esc(MAIN.subj)}”</span><span>1.4 KB · 12 lines changed · Mira Okonkwo, 3 days ago</span>`)}
      ${card('R', rb ? `Keep your commit’s version` : 'Delete the file (as in feat/palette)', `<span class="cfe-thumb ${rb ? 'R' : 'del'}" aria-hidden="true">${rb ? 'docs/old.md · 12 lines changed' : 'File deleted'}</span>`, rb ? `<span>Changed in <span class="mono">${REPLAYED.hash}</span> “${esc(REPLAYED.subj)}”</span><span>1.4 KB · 12 lines changed</span>` : `<span>feat/palette removed it in <span class="mono">${FEAT.hash}</span> “docs: remove the old architecture notes”</span><span>Jonas Berg, 2 days ago</span>`)}
      </div><div><button class="btn btn-sm" data-do="tool">${ic('external', 12)}Open in merge tool</button></div></div></div>`;
  }
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
    root.innerHTML = `<div class="cfe-head" data-r="head"></div>${noteHTML()}${f.kind === 'text' ? '<div class="cfe-top" data-r="top" tabindex="0" aria-label="Conflict panes"></div><div class="cfe-res"><div class="cfe-resh" data-r="resh"></div><div class="cfe-rs" data-r="res"></div></div>' : choiceHTML()}<div class="cfe-foot" data-r="foot"></div>`;
    mountHead(); mountFoot();
    if (f.kind === 'text') { mountTop(); mountRes(); }
    const af = root.querySelector('[data-autofocus]'); if (af) af.focus({ preventScroll: true });
  }
  const refresh = () => keepFocus(() => { mountHead(); mountFoot(); if (cur().kind === 'text') { mountTop(); mountRes(); } });

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
  const takeAll = (side) => { const f = cur(); chunksOf(f).forEach((c) => { f.st[c.id] = { picks: [side], custom: null, auto: false }; }); refresh(); toast(`Took ${sides()[side].name} for every conflict in ${f.path}`, 'info'); };
  const navTo = (path) => { location.href = path; };
  const backHref = () => 'changes.html?op=' + (S.rebase ? 'rebase' : 'merge');
  const goBack = () => { if (dirty(cur())) { S.cancelAsk = true; mountFoot(); const k = root.querySelector('[data-autofocus]'); if (k) k.focus({ preventScroll: true }); } else navTo(backHref()); };
  function stepFile(d) { S.idx = (S.idx + d + S.files.length) % S.files.length; S.focus = 0; S.cancelAsk = false; render(); }
  function save() {
    const f = cur(); if (f.kind === 'text' ? leftOf(f) > 0 : !f.choice) return;
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
    if (a === 'keep') { S.cancelAsk = false; return mountFoot(); }
    if (a === 'discard') return navTo(backHref());
    if (a === 'prevC') return setFocus(S.focus - 1, true);
    if (a === 'nextC') return setFocus(S.focus + 1, true);
    if (a === 'base') { S.showBase = !S.showBase; return keepFocus(() => { mountHead(); mountTop(); }); }
    if (a === 'tool') return toast(`Opened ${f.path} in your merge tool`, 'info');
    if (a === 'prevF') return stepFile(-1);
    if (a === 'nextF') return stepFile(1);
    if (a === 'hmenu') return G.menu(win, at(el), [{ label: `File ${S.idx + 1} of ${S.files.length}`, disabled: true }, { label: 'Previous file', icon: 'chevR', onClick: () => stepFile(-1) }, { label: 'Next file', icon: 'chevR', onClick: () => stepFile(1) }, { sep: 1 }, { label: 'Open in merge tool', icon: 'external', onClick: () => doAct('tool') }]);
    if (a === 'fmenu') { const sd = sides(); return G.menu(win, at(el), [{ label: `Take ${sd.L.name} for all`, icon: 'check', onClick: () => takeAll('L') }, { label: `Take ${sd.R.name} for all`, icon: 'check', onClick: () => takeAll('R') }, { sep: 1 }, { label: 'Reset all picks', icon: 'undo', onClick: () => { initFile(f); refresh(); } }]); }
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
    ['Other', [['binary', 'Binary'], ['modifydelete', 'Modify / delete'], ['rebase', 'Rebase'], ['cancel', 'Cancel confirm'], ['narrow', 'Narrow 1000×700']]]
  ];
  const scen = document.getElementById('scen');
  scen.innerHTML = groups.map(([l, items]) => `<div class="grp-s"><span class="l">${l}</span><div class="seg sm" role="group" aria-label="${l}">${items.map(([k, t]) => `<button data-s="${k}" aria-pressed="false">${t}</button>`).join('')}</div></div>`).join('');
  const setPicks = (f, id, picks) => { f.st[id] = { picks, custom: null, auto: false }; };
  /* the chunk each text scenario is about: it is focused and scrolled to the top of every pane on load */
  const FOCUS = { unresolved: 0, picking: 0, resolved: 0, base: 0, rebase: 0, narrow: 0, custom: 1 };
  function apply(key, o) {
    o = o || {};
    S.key = key; S.rebase = key === 'rebase' || !!o.rebase; S.narrow = key === 'narrow'; S.showBase = key === 'base'; S.cancelAsk = false; S.allDone = false; S.focus = FOCUS[key] || 0; S.idx = 0;
    S.files = freshFiles(); S.files.forEach(initFile);
    const g = S.files[0];
    if (key === 'picking' || key === 'base' || key === 'cancel' || key === 'narrow') { setPicks(g, 'c1', ['L', 'R']); setPicks(g, 'c3', ['R']); } /* chunk 1 ticked on both sides (main first), chunk 2 left open right below it, chunk 3 ticked */
    if (key === 'resolved') { setPicks(g, 'c2', ['L']); setPicks(g, 'c3', ['L', 'R']); }
    if (key === 'custom') { g.st.c2 = { picks: [], custom: ['    pub fn lane_color(col: usize, palette: &Palette) -> Hsla {', '        let hue = (col as f32 * 137.508) % 360.0;', '        let l = palette.graph_lightness().clamp(0.40, 0.70);'].join('\n'), auto: false }; setPicks(g, 'c3', ['R']); S.focus = 1; }
    if (key === 'cancel') { g.st.c3 = { picks: ['L'], custom: null, auto: false }; S.cancelAsk = true; } /* dirty: the file differs from the state git wrote */
    if (key === 'binary') S.idx = 2;
    if (key === 'modifydelete') S.idx = 3;
    if (o.file != null) { const i = S.files.findIndex((f) => f.path === o.file); if (i >= 0) S.idx = i; }
    win.style.width = S.narrow ? '1000px' : '1200px'; win.style.height = S.narrow ? '700px' : '800px'; if (win._fit) win._fit();
    scen.querySelectorAll('button[data-s]').forEach((b) => { const on = b.dataset.s === key; b.classList.toggle('on', on); b.setAttribute('aria-pressed', on); });
    render();
    if (key === 'custom') { const ed = root.querySelector('[data-ed="c2"]'); if (ed) { ed.focus({ preventScroll: true }); const r = document.createRange(); r.selectNodeContents(ed); r.collapse(false); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(r); } }
    if (key in FOCUS && cur().kind === 'text') setFocus(S.focus, true); /* synchronous: the scroll is in place on the first frame */
  }
  scen.addEventListener('click', (e) => { const b = e.target.closest('[data-s]'); if (b) apply(b.dataset.s); });

  document.getElementById('notes').innerHTML = [
    ['Layout', 'The editor replaces the file lists and the diff pane of Changes. Header, two side panes (left main / HEAD, right feat/palette), the result pane (about 44% of the height) and a footer. Panes share one scroller, so chunk boxes line up row by row; missing lines are hatched.'],
    ['Checkboxes and order', '“Use these lines” sits in each side’s chunk header. One tick takes that side; two ticks keep both blocks in the order they were ticked (the numbered badge shows it, the swap button reverses it). Unticking removes that side. Alt+1 / Alt+2 toggle the left / right side of the focused chunk.'],
    ['Custom chunks', 'Typing in a result chunk makes it Custom: an Edited chip appears, the side checkboxes above are disabled and “Reset to picks” brings the last picks back. A Custom chunk counts as resolved, even when it is empty.'],
    ['Auto-resolve', 'When one side equals the base for a chunk, the other side is pre-ticked and the chunk carries an Auto chip. It stays editable: tick or untick as usual.'],
    ['Save', '“Save and mark resolved” writes the merged file and runs git add. It is disabled while any chunk is unresolved (tooltip: Resolve every conflict first). After saving, the editor opens the next conflicted file; after the last one it offers Back to Changes. Cancel asks before discarding picks or edits.'],
    ['Non-text conflicts', 'Binary files and modify/delete conflicts have no chunks: two big option cards (Keep main’s version / Use feat/palette’s version, or Delete the file as in feat/palette), with size and last commit. Open in merge tool stays available.'],
    ['Rebase labelling', 'Sides are named by branch or commit, never ours/theirs. In a rebase the left side is the branch you rebase onto (feat/palette, upstream) and the right side is the commit being replayed (8d41f6e); a note under the header says so.'],
    ['Keyboard', 'Alt+↑ / Alt+↓ previous / next conflict · Alt+1 / Alt+2 toggle main / feat/palette on the focused chunk · Ctrl+S save · Esc back to Changes (asks first when something changed). The focused chunk has the focus ring in every pane and in the result.']
  ].map(([t, b]) => `<div class="n"><b>${t}</b>${b}</div>`).join('');

  const first = Q.get('s') && groups.some(([, it]) => it.some(([k]) => k === Q.get('s'))) ? Q.get('s') : 'unresolved';
  apply(first, { file: Q.get('file'), rebase: Q.get('op') === 'rebase' });
  G.fit(win);
})();
