/* Local changes (Changes tab). Behaviour mirrors crates/app/src/changes_view.rs; state switchers above the window show every state. */
(function () {
  const G = GL, ic = G.ic, esc = G.esc;
  G.stagebar('<span class="muted" style="font-size:12px">stage / unstage per file, hunk and line · pick a state above the window</span>');
  const stage = G.h('<div class="stage"><div class="scen" id="scen"></div><div class="winwrap"></div><div class="notes" id="notes"></div></div>'); document.body.appendChild(stage);
  const sh = G.shell({ host: stage.querySelector('.winwrap'), w: 1200, h: 800, view: 'changes', statusInfo: '4 unstaged · 3 staged' });
  const win = sh.win;
  const BRANCH = 'main', FALLBACK = 'src/history/graph.rs';
  const initial = () => ({
    unstaged: [{ st: 'M', path: 'src/history/graph.rs', a: 9, d: 3 }, { st: 'M', path: 'src/ui/toolbar.rs', a: 4, d: 1 }, { st: 'q', path: 'docs/graph.md', a: 38, d: 0 }, { st: 'D', path: 'scripts/old-release.sh', a: 0, d: 22 }],
    staged: [{ st: 'M', path: 'src/ui/sidebar.rs', a: 6, d: 4 }, { st: 'A', path: 'src/settings/git_exec.rs', a: 11, d: 0 }, { st: 'R', path: 'src/ui/tree.rs', old: 'src/ui/tree_view.rs', a: 2, d: 2 }]
  });
  const LBL = { A: 'Added', M: 'Modified', D: 'Deleted', R: 'Renamed', q: 'Untracked' };
  const S = Object.assign(initial(), { sel: { g: 'u', path: 'src/history/graph.rs' }, status: 'ok', diffState: null, mode: 'unified', discard: null, picked: new Set(), amend: false, noCommits: false, msg: 'feat(graph): clip lanes past the column edge\n\nSkip paths that are fully outside the clipped column so wide graphs\nstay cheap to paint.', busy: false });

  /* local toast: errors stay until dismissed, everything else auto-hides after 5 s (G.toast hides every kind) */
  const toast = (msg, kind) => {
    const layer = G.layer(win); let c = layer.querySelector('.toasts');
    if (!c) { c = G.h('<div class="toasts"></div>'); layer.appendChild(c); }
    const t = G.h(`<div class="toast ${kind}" role="status">${ic(kind === 'error' ? 'err' : kind === 'warn' ? 'warn' : kind === 'ok' ? 'check' : 'info', 14)}<span class="toast-msg">${esc(msg)}</span><button class="btn btn-xs btn-ghost" aria-label="Dismiss">${ic('x', 12)}</button></div>`);
    t.querySelector('button').onclick = () => t.remove(); c.appendChild(t);
    if (kind !== 'error') setTimeout(() => t.remove(), 5000);
  };
  const list = (g) => (g === 'u' ? S.unstaged : S.staged);
  const cur = () => { const f = list(S.sel.g).find((x) => x.path === S.sel.path); return f ? { f, g: S.sel.g } : null; };
  const all = () => [...S.unstaged.map((f) => ({ f, g: 'u' })), ...S.staged.map((f) => ({ f, g: 's' }))];
  const plural = (n, w) => `${n} ${w}${n === 1 ? '' : 's'}`;
  const hunksFor = (f) => G.DIFFS[f.path] || G.DIFFS[FALLBACK];
  const allowedFile = (c) => (c.g === 'u' ? 'AMDq'.includes(c.f.st) : c.f.st === 'M'); // line picking: unstaged A/M/D, staged only M
  const hunkAllowed = (c) => 'AMDq'.includes(c.f.st) && !(c.g === 's' && c.f.st === 'A');

  const rowHTML = (f, g) => {
    const d = f.path.lastIndexOf('/'), sel = S.sel.g === g && S.sel.path === f.path;
    const tipS = (g === 'u' ? 'Stage file' : 'Unstage file') + ' · Space';
    return `<div class="frow${sel ? ' sel' : ''}" role="option" aria-selected="${sel}" tabindex="-1" data-g="${g}" data-p="${esc(f.path)}"><span class="fs ${f.st}" aria-label="${LBL[f.st]}">${f.st === 'q' ? '?' : f.st}</span><span class="fname" title="${esc(f.path)}"><bdi><span class="fdir">${esc(f.path.slice(0, d + 1))}</span>${esc(f.path.slice(d + 1))}</bdi></span><span class="fnum"><span class="p">+${f.a}</span><span class="m">−${f.d}</span></span><span class="acts"><button class="btn btn-xs btn-ghost" data-act="${g === 'u' ? 'stage' : 'unstage'}" data-tip="${tipS}" aria-label="${tipS}"${S.busy ? ' disabled' : ''}>${ic(g === 'u' ? 'plus' : 'minus', 12)}</button>${g === 'u' ? `<button class="btn btn-xs btn-ghost" data-act="discard" data-tip="Discard changes" aria-label="Discard changes"${S.busy ? ' disabled' : ''}>${ic('undo', 12)}</button>` : ''}</span></div>`;
  };
  const groupHTML = (g, title, allLabel) => {
    const items = list(g), empty = g === 'u' ? 'No unstaged changes' : 'Nothing staged', loading = S.status !== 'ok';
    const shown = loading ? [] : items;
    return `<section class="grp" aria-label="${title}"><div class="grp-h"><span>${title}</span><span class="ncount">${shown.length}</span><span class="od-fill"></span><button class="btn btn-sm" data-act="${g === 'u' ? 'stageAll' : 'unstageAll'}"${shown.length === 0 || S.busy ? ' disabled' : ''}>${allLabel}</button></div><div class="flist" role="listbox" tabindex="0" aria-label="${title} files" data-g="${g}">${shown.length ? shown.map((f) => rowHTML(f, g)).join('') : `<div class="none">${empty}</div>`}</div></section>`;
  };

  const hunkLabel = (c, n) => (n ? `${c.g === 'u' ? 'Stage' : 'Unstage'} ${plural(n, 'line')}` : `${c.g === 'u' ? 'Stage' : 'Unstage'} hunk`);
  function diffHTML() {
    const c = cur();
    if (S.diffState === 'loading') return `<div class="dstate load"><div class="skel" style="width:320px"></div><div class="skel" style="width:260px"></div><div class="skel" style="width:290px"></div></div>`;
    if (S.diffState === 'error') return `<div class="dstate err">fatal: bad object 9f3c1a2e — the file could not be read</div>`;
    if (S.diffState === 'binary') return `<div class="dstate">Binary file</div>`;
    if (S.diffState === 'empty') return `<div class="dstate">No changes</div>`;
    if (!c || S.diffState === 'none') return `<div class="dstate">Select a file</div>`;
    return `<div class="dscroll" id="diff" tabindex="0" aria-label="Diff"></div>`;
  }

  function render() {
    const c = cur();
    const hunks = c ? hunksFor(c.f) : [];
    const noSel = !c || S.diffState === 'none';
    const side = c && c.g === 'u' ? 'unstaged' : 'staged';
    const path = c ? (c.f.old ? `${c.f.path} ← ${c.f.old}` : c.f.path) : '';
    const nStaged = S.staged.length, canCommit = !S.busy && (nStaged > 0 || S.amend);
    const commitLabel = S.amend ? 'Amend commit' : `Commit ${plural(nStaged, 'file')} to ${BRANCH}`;
    const subj = S.msg.split('\n')[0].length, wide = Math.max(0, ...S.msg.split('\n').slice(1).map((l) => l.length));
    sh.center.innerHTML = `<div class="chg-wrap">
      <div class="lists" tabindex="-1">${S.discard ? `<div class="discardbar" role="alertdialog" aria-label="Confirm discard"><span class="msg" title="${esc(S.discard)}">Discard changes to ${esc(S.discard)}?</span><button class="btn btn-sm btn-danger" data-act="doDiscard">Discard</button><button class="btn btn-sm" data-act="cancelDiscard">Cancel</button></div>` : ''}${groupHTML('u', 'Unstaged', 'Stage all')}${groupHTML('s', 'Staged', 'Unstage all')}</div>
      <div class="dview"><div class="ddiff" style="flex:1;min-height:0"><div class="dtool"><span class="fn">${esc(path)}</span>${noSel ? '' : `<span class="side">${side} · ${plural(hunks.length, 'hunk')}</span>`}
        <div class="seg sm" role="group" aria-label="Diff mode"><button class="${S.mode === 'unified' ? 'on' : ''}" data-mode="unified">${ic('unified', 13)}Unified</button><button class="${S.mode === 'split' ? 'on' : ''}" data-mode="split">${ic('split', 13)}Side-by-side</button></div></div>
        ${diffHTML()}</div></div>
      <div class="commitbox">
        <div><label class="field-label" for="msg">Commit message — subject on the first line, body after a blank line</label>
          <div class="msgwrap"><textarea class="input" id="msg" spellcheck="true" placeholder="feat(graph): …">${esc(S.msg)}</textarea><span class="guide g50"></span><span class="guide g72"></span></div>
          <div class="od-row" style="margin-top:6px;gap:16px"><span class="cnt${subj > 50 ? ' over' : ''}" id="subj">Subject ${subj} / 50</span><span class="cnt${wide > 72 ? ' over' : ''}" id="wide">Longest body line ${wide} / 72</span><span class="od-fill"></span><span class="cnt">Ctrl+Enter to commit</span></div></div>
        <div class="cb-side">
          <div class="od-stack" style="--od-gap:4px"><div class="switch${S.amend ? ' on' : ''}${S.noCommits ? ' dis' : ''}" id="amend" role="switch" aria-checked="${S.amend}" aria-disabled="${S.noCommits}" tabindex="${S.noCommits ? -1 : 0}"><span class="track"></span><span>Amend previous commit</span></div><div class="field-help" style="margin:0">${S.noCommits ? 'There is no commit to amend yet.' : 'Amend reuses the message of e4f2a91 and rewrites it.'}</div></div>
          <div class="od-stack" style="--od-gap:8px"><button class="btn btn-primary" id="commit"${canCommit ? '' : ' disabled'}>${commitLabel}</button><button class="btn" id="commitPush"${canCommit ? '' : ' disabled'}>Commit &amp; Push</button></div>
        </div>
      </div></div>`;
    // diff body
    const d = sh.center.querySelector('#diff');
    if (d && c) {
      d.innerHTML = G.renderDiff(hunks, S.mode, { hunkAction: '' });
      d.insertAdjacentHTML('beforeend', `<div class="dnote">\\ No newline at end of file</div>`);
      decorateDiff(d, c, hunks);
    }
    // status bar: loading / failure replace the left slot; counts stay on the right
    const sb = win.querySelector('.statusbar'), op = sb && sb.querySelector('.op'), info = sb && sb.querySelector('.od-fill + span');
    if (op) { if (op._orig == null) op._orig = op.innerHTML; op.innerHTML = S.status === 'loading' ? `${ic('fetch', 12, 'spin')}Loading changes…` : S.status === 'failure' ? `<span style="color:var(--error);max-width:520px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;display:inline-block">fatal: Unable to create '.git/index.lock': File exists.</span>` : op._orig; }
    if (info) info.textContent = `${S.unstaged.length} unstaged · ${S.staged.length} staged`;
    const tc = win.querySelector('.titlebar .seg .counter.accent'); if (tc) tc.textContent = S.unstaged.length + S.staged.length;
    G.initTips(win);
  }

  /* hunk header buttons + line picking (unified only; side-by-side has neither) */
  function decorateDiff(d, c, hunks) {
    const heads = [...d.querySelectorAll('.dh')];
    const lines = [...d.querySelectorAll('.dl')];
    // assign each diff row to its hunk
    let h = -1; const byHunk = heads.map(() => []);
    [...d.children].forEach((el) => { if (el.classList.contains('dh')) h++; else if (el.classList.contains('dl') && h >= 0) byHunk[h].push(el); });
    const pickable = S.mode === 'unified' && allowedFile(c);
    byHunk.forEach((rows, hi) => rows.forEach((r, ri) => { r.dataset.h = hi; r.dataset.r = ri; if (pickable && (r.classList.contains('add') || r.classList.contains('del'))) { r.classList.add('pick'); if (S.picked.has(hi + ':' + ri)) r.classList.add('picked'); } }));
    heads.forEach((hd, hi) => {
      if (S.mode !== 'unified') return; // side-by-side: plain header row, no Stage/Unstage hunk button
      const n = [...S.picked].filter((k) => k.startsWith(hi + ':')).length;
      const b = G.h(`<button class="btn btn-sm" data-hunk="${hi}"${hunkAllowed(c) && !S.busy ? '' : ' disabled'}>${hunkLabel(c, n)}</button>`);
      hd.appendChild(b);
    });
  }

  /* ---- interactions ---- */
  const move = (path, from, to) => {
    const f = list(from).find((x) => x.path === path); if (!f) return;
    list(from).splice(list(from).indexOf(f), 1); list(to).push(f); S.picked.clear();
  };
  const select = (g, path) => { S.sel = { g, path }; S.diffState = null; S.picked.clear(); S.discard = null; };
  sh.center.addEventListener('click', (e) => {
    const t = e.target, act = t.closest('[data-act]'), row = t.closest('.frow');
    if (act && row) {
      const g = row.dataset.g, p = row.dataset.p, a = act.dataset.act;
      if (a === 'stage') { move(p, 'u', 's'); S.sel = { g: 's', path: p }; toast(`Staged ${p}`, 'ok'); }
      else if (a === 'unstage') { move(p, 's', 'u'); S.sel = { g: 'u', path: p }; toast(`Unstaged ${p}`, 'ok'); }
      else if (a === 'discard') { S.discard = p; select('u', p); S.discard = p; }
      return render();
    }
    if (act) {
      const a = act.dataset.act;
      if (a === 'stageAll') { const n = S.unstaged.length; S.staged.push(...S.unstaged.splice(0)); toast(`Staged ${plural(n, 'file')}`, 'ok'); }
      else if (a === 'unstageAll') { const n = S.staged.length; S.unstaged.push(...S.staged.splice(0)); toast(`Unstaged ${plural(n, 'file')}`, 'ok'); }
      else if (a === 'cancelDiscard') S.discard = null;
      else if (a === 'doDiscard') { const p = S.discard; S.unstaged = S.unstaged.filter((f) => f.path !== p); S.discard = null; toast(`Discarded changes to ${p}`, 'ok'); }
      return render();
    }
    if (row) { select(row.dataset.g, row.dataset.p); return render(); }
    const hb = t.closest('[data-hunk]');
    if (hb) {
      const c = cur(), hi = +hb.dataset.hunk, n = [...S.picked].filter((k) => k.startsWith(hi + ':')).length, verb = c.g === 'u' ? 'Staged' : 'Unstaged';
      toast(n ? `${verb} ${plural(n, 'line')}` : `${verb} hunk ${hi + 1} of ${c.f.path}`, 'ok'); S.picked.clear(); return render();
    }
    const m = t.closest('[data-mode]'); if (m) { S.mode = m.dataset.mode; S.picked.clear(); return render(); }
    const ln = t.closest('.dl.pick .ln');
    if (ln) {
      const r = ln.parentElement, hi = r.dataset.h, key = hi + ':' + r.dataset.r;
      if (e.shiftKey && S.last && S.last.split(':')[0] === hi) { // range from the last picked line, same hunk only
        const a = +S.last.split(':')[1], b = +r.dataset.r;
        r.parentElement.querySelectorAll(`.dl.pick[data-h="${hi}"]`).forEach((x) => { const i = +x.dataset.r; if (i >= Math.min(a, b) && i <= Math.max(a, b)) S.picked.add(hi + ':' + i); });
      } else if (S.picked.has(key)) S.picked.delete(key); else S.picked.add(key);
      S.last = key; render();
    }
    const am = t.closest('#amend');
    if (am && !S.noCommits) toggleAmend();
  });
  sh.center.addEventListener('keydown', (e) => {
    if (e.target.id === 'amend' && (e.key === ' ' || e.key === 'Enter') && !S.noCommits) { e.preventDefault(); toggleAmend(); return; }
    if (e.target.id === 'msg' && e.key === 'Enter' && e.ctrlKey) { e.preventDefault(); doCommit(false); return; }
    const l = e.target.closest && e.target.closest('.flist');
    if (l && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) { // one sequence through both lists
      e.preventDefault(); const seq = all(), i = seq.findIndex((x) => x.g === S.sel.g && x.f.path === S.sel.path), n = seq[Math.max(0, Math.min(seq.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))];
      if (n) { select(n.g, n.f.path); render(); const nl = sh.center.querySelector(`.flist[data-g="${n.g}"]`); if (nl) nl.focus(); }
    }
  });
  sh.center.addEventListener('input', (e) => {
    if (e.target.id !== 'msg') return; S.msg = e.target.value;
    const ls = S.msg.split('\n'), s = ls[0].length, w = Math.max(0, ...ls.slice(1).map((x) => x.length)), a = sh.center.querySelector('#subj'), b = sh.center.querySelector('#wide');
    a.textContent = `Subject ${s} / 50`; a.classList.toggle('over', s > 50); b.textContent = `Longest body line ${w} / 72`; b.classList.toggle('over', w > 72);
  });
  const HEAD_MSG = 'fix(history): keep the selection after a refresh';
  let draft = '';
  function toggleAmend() { if (!S.amend) { draft = S.msg; S.msg = HEAD_MSG; } else S.msg = draft; S.amend = !S.amend; render(); }
  function doCommit() {
    if (!S.msg.trim()) { toast('Write a commit message first', 'warn'); return; }
    if (!(S.staged.length || S.amend)) return;
    toast(S.amend ? 'Amended the previous commit' : `Committed ${plural(S.staged.length, 'file')} to ${BRANCH}`, 'ok');
    S.staged = []; S.msg = ''; S.amend = false; S.picked.clear(); if (S.sel.g === 's') S.sel = { g: 'u', path: S.unstaged[0] ? S.unstaged[0].path : '' }; render();
  }
  sh.center.addEventListener('click', (e) => { if (e.target.closest('#commit') || e.target.closest('#commitPush')) { if (!e.target.closest('button').disabled) doCommit(); } });

  /* ---- scenario switchers (presentation only) ---- */
  const scen = document.getElementById('scen');
  const groups = [
    ['Lists & status', [['work', 'Working tree'], ['discard', 'Discard confirm'], ['clean', 'Clean tree'], ['loading', 'Loading status'], ['failure', 'Status failed']]],
    ['Diff pane', [['loaded', 'Loaded'], ['none', 'No selection'], ['dloading', 'Loading'], ['derror', 'Error'], ['binary', 'Binary'], ['empty', 'No changes']]],
    ['Commit box', [['cb', 'Default'], ['amend', 'Amend on'], ['nocommit', 'No commits yet'], ['nostage', 'Nothing staged'], ['nomsg', 'Empty message']]],
    ['Toasts', [['t-ok', 'Ok'], ['t-warn', 'Warn'], ['t-err', 'Error']]]
  ];
  scen.innerHTML = groups.map(([l, items]) => `<div class="grp-s"><span class="l">${l}</span><div class="seg sm" role="group" aria-label="${l}">${items.map(([k, t]) => `<button data-s="${k}">${t}</button>`).join('')}</div></div>`).join('');
  const reset = () => { Object.assign(S, initial(), { sel: { g: 'u', path: 'src/history/graph.rs' }, status: 'ok', diffState: null, discard: null, picked: new Set(), amend: false, noCommits: false, msg: 'feat(graph): clip lanes past the column edge\n\nSkip paths that are fully outside the clipped column so wide graphs\nstay cheap to paint.' }); };
  scen.addEventListener('click', (e) => {
    const b = e.target.closest('[data-s]'); if (!b) return; const k = b.dataset.s;
    if (k === 'work') reset();
    else if (k === 'discard') { reset(); S.discard = 'docs/graph.md'; S.sel = { g: 'u', path: 'docs/graph.md' }; }
    else if (k === 'clean') { reset(); S.unstaged = []; S.staged = []; S.msg = ''; S.diffState = 'none'; }
    else if (k === 'loading') { S.status = 'loading'; } else if (k === 'failure') { S.status = 'failure'; }
    else if (k === 'loaded') { S.diffState = null; S.status = 'ok'; if (!cur()) S.sel = { g: 'u', path: 'src/history/graph.rs' }; }
    else if (k === 'none') S.diffState = 'none'; else if (k === 'dloading') S.diffState = 'loading'; else if (k === 'derror') S.diffState = 'error';
    else if (k === 'binary') S.diffState = 'binary'; else if (k === 'empty') S.diffState = 'empty';
    else if (k === 'cb') { S.amend = false; S.noCommits = false; S.msg = initialMsg(); if (!S.staged.length) S.staged = initial().staged; }
    else if (k === 'amend') { S.noCommits = false; if (!S.amend) { draft = S.msg; S.msg = HEAD_MSG; S.amend = true; } }
    else if (k === 'nocommit') { S.noCommits = true; S.amend = false; }
    else if (k === 'nostage') { S.unstaged.push(...S.staged.splice(0)); S.amend = false; }
    else if (k === 'nomsg') { S.msg = ''; if (!S.staged.length) S.staged = initial().staged; S.amend = false; }
    else if (k === 't-ok') return toast('Committed 3 files to main', 'ok');
    else if (k === 't-warn') return toast('Write a commit message first', 'warn');
    else if (k === 't-err') return toast('error: pathspec \'src/old.rs\' did not match any file(s) known to git', 'error');
    if (k !== 'discard' && k !== 'work') S.discard = null;
    scen.querySelectorAll('button').forEach((x) => x.classList.toggle('on', x === b && !['t-ok', 't-warn', 't-err'].includes(k)));
    render();
  });
  const initialMsg = () => 'feat(graph): clip lanes past the column edge\n\nSkip paths that are fully outside the clipped column so wide graphs\nstay cheap to paint.';

  document.getElementById('notes').innerHTML = [
    ['Stage and discard', 'Unstaged rows have Stage file and Discard changes; staged rows only Unstage file. Buttons are Xs ghost icons, disabled while an action runs; the stage/unstage tooltip carries its key (Space); Discard is Ctrl+Shift+Z. Discard asks first (Settings → General "confirm discard", on by default); with the setting off it runs immediately. Untracked files are discarded by deletion.'],
    ['Selection and keys', 'The first unstaged file is selected automatically (else the first staged one); the selection follows a file that moves to the other side. ↑ / ↓ walk both lists as one sequence; Ctrl+Enter commits (rebindable, the hint stays).'],
    ['Hunks and lines', 'Hunk button: Stage hunk / Unstage hunk, or Stage n lines / Unstage n lines when lines are picked. Disabled while busy, for renamed or conflicted files, and for a staged added file. Click a line-number gutter to pick a line, Shift+click for a range within the hunk; picks reset on selection change. Unstaged: added / modified / deleted files; staged: modified only.'],
    ['Side-by-side', 'The toggle is saved to settings (shared with Diff settings). In side-by-side there is no hunk button and the gutters are not clickable: no hunk or line staging.'],
    ['Diff rendering', 'No syntax highlighting: text colour plus changed-word emphasis. No gutter tint on context lines, no row hover, hunk headers are not sticky, an empty split side is a flat hover fill. With line numbers off in Diff settings there are no gutters and so no picking.'],
    ['Commit box', 'Primary is disabled while busy, or with nothing staged and amend off. Amend saves the typed draft and fills in HEAD\'s message; turning it off restores the draft; after a commit the message clears and amend switches off. With no commits yet, amend is disabled. Commit & Push is available alongside Commit.']
  ].map(([t, b]) => `<div class="n"><b>${t}</b>${b}</div>`).join('');

  scen.querySelector('[data-s="work"]').classList.add('on'); scen.querySelector('[data-s="loaded"]').classList.add('on'); scen.querySelector('[data-s="cb"]').classList.add('on');
  render();
  G.fit(win);
})();
