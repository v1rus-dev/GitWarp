/* Ref menus (R1): cascading menus on commits and ref badges. All 10 scenarios draw the menu OPEN over the real main-window shell (static, sticky)
   and stay interactive; ?s=N picks a scenario. The item sets come from G.refMenus (app.js) — the same code the main windows use. */
(function () {
  const G = window.GL, ic = G.ic, esc = G.esc;
  document.title = 'GitWarp — ref menus';
  G.stagebar();
  const W = 1200, H = 860, ROW = 2; /* the commit under the pointer is always list row 2, so every menu opens near the top of the window */
  const R = (t, n) => ({ t, n });
  const PAIR = [R('head', 'main'), R('rm', 'origin/main')];
  const MANY = [R('head', 'main'), R('rm', 'origin/main'), R('br', 'feat/idle-lanes'), R('rm', 'upstream/main'), R('tag', 'v0.3.0')];
  const NINE = [R('head', 'main'), R('rm', 'origin/main'), R('br', 'feat/idle-lanes'), R('br', 'fix/git-detect'), R('br', 'refactor/tree'), R('rm', 'origin/refactor/tree'), R('br', 'spike/virtual-list'), R('br', 'wip/sidebar'), R('rm', 'upstream/release/0.3'), R('tag', 'v0.3.0'), R('tag', 'v0.3.1-rc1')];
  const S = [
    { t: 'One ref', refs: [R('br', 'feat/palette')], open: [0], note: 'The commit has a single ref (<b>feat/palette</b>) and it still opens through a submenu, so the menu has the same shape on every commit. <b>feat/palette</b> is behind its upstream, so the row hints <b>↓2</b> and the submenu offers “Fast-forward”.' },
    { t: 'HEAD pair', refs: PAIR, open: [0], note: 'Local <b>main</b> (HEAD) and <b>origin/main</b> on the same commit are one row, “main” with the muted “+origin” hint and an accent icon. Its submenu has two sections, each with a small header. “Switch to main” is disabled with a check and the hint “current”; “Pull” appears because main is HEAD.' },
    { t: 'Several refs', refs: MANY, open: [1], note: 'Order: HEAD branch, other local branches, remote-only branches, tags. The pointer rests on <b>feat/idle-lanes</b> (↑1 ahead of its upstream); its submenu opened after the 200 ms hover delay and the parent row keeps the “open” fill.' },
    { t: 'Tag', refs: [R('br', 'feat/palette'), R('tag', 'v0.3.0')], open: [1], note: 'Tag submenu: header with the tag name, checkout / branch / push, then rename and delete. “Compare with main” is planned.' },
    { t: 'Remote only', refs: [R('rm', 'origin/fix/git-detect'), R('rm', 'upstream/fix/git-detect')], open: [0], note: 'A remote-only ref gets a cloud icon and the remote section on its own: checkout as a local branch, merge, fetch, new branch, delete the remote branch.' },
    { t: 'Overflow (9 refs)', refs: NINE, open: [5], note: 'More than 6 refs: the first 5 rows, then <b>More refs (4)</b>. Its submenu starts with a filter field and lists the rest; every row there opens its own submenu one level further.' },
    { t: 'Badge right-click', refs: PAIR, badge: 'main', note: 'Right-click on the merged badge “main +origin”: the ref’s content opens directly as a top-level menu, with no ref-row level. A merged badge shows both sections; the menu scrolls when the window is shorter than the list.' },
    { t: 'Flip left', refs: MANY, x: 900, open: [1], note: 'Opened near the right window edge: the submenu does not fit to the right, so it mirrors to the left and still overlaps its parent by 4 px. The safe triangle is mirrored as well.' },
    { t: 'Keyboard', refs: MANY, open: [1], kbd: true, note: '↑ ↓ move the focus ring, → or Enter opens the submenu and focuses its first row, ← or Esc closes only that level, Esc on the first level closes the menu.' },
    { t: 'Before / after', ba: true, note: 'Left: today’s badge menu for “main +origin” (local and remote menus glued together). Right: the same badge in the new design.' }
  ];
  const q = new URLSearchParams(location.search); let cur = Math.max(1, Math.min(S.length, +q.get('s') || 1));
  const tabs = S.map((s, i) => `<button type="button" class="btn btn-sm" data-s="${i + 1}" aria-pressed="false">${i + 1} · ${esc(s.t)}</button>`).join('');
  document.body.insertAdjacentHTML('beforeend', `<div class="page" style="max-width:1268px"><h1>Ref menus on commits</h1>
    <p class="lead">Every ref of a commit is a row at the top of the commit menu; each opens a cascading submenu with the actions of that ref, so branch, tag and compare actions are reachable from one place.</p>
    <div class="rm-banner" role="note"><b>Planned · not in the app yet</b><span>The whole page is a new design. Items drawn with their own Planned tag are planned even inside this design.</span></div>
    <div class="rm-tabs" role="toolbar" aria-label="Scenario">${tabs}</div><p class="rm-cap" id="cap"></p><div id="stage" class="stage rm-stage"></div>
    <div class="rm-notes">
      <div><h3>Decisions</h3><ul>
        <li><b>1.</b> A local branch and its remote counterpart (“main +origin”) are <b>one row</b>; its submenu has two sections, “main” and “origin/main”, each with a small header.</li>
        <li><b>2.</b> Refs <b>always</b> open through a submenu, even when the commit has a single ref.</li>
        <li><b>3.</b> New actions: “Compare with {HEAD}”, “Hide from graph”, “Interactive rebase from here…” (Planned); a “Copy ›” submenu replaces the scattered copy items.</li></ul></div>
      <div><h3>Behaviour</h3><ul>
        <li>Opens on hover after <b>200 ms</b>, on click, or with <b>→</b> / <b>Enter</b>; <b>←</b> closes one level, <b>Esc</b> closes the submenu only.</li>
        <li>Placement: right of the parent, overlapping it by 4 px, top = row top − 6; mirrors to the left when it does not fit; clamped to the window.</li>
        <li><b>Safe triangle:</b> moving diagonally towards the submenu does not switch to the rows crossed on the way (300 ms grace).</li>
        <li>The parent row keeps the “open” fill and a bright chevron while its submenu is open.</li></ul></div>
      <div><h3>UI kit</h3><ul>
        <li><code>crates/ui/src/components/menu.rs</code> needs a <b>cascading submenu</b> (item with a submenu, chevron, hover delay, flip, safe triangle) instead of replacing the menu in place with a “Back” row.</li>
        <li>New entry kinds: <b>section header</b> with optional right-aligned ghost icon buttons, <b>filter field</b>, row <b>hint</b> text.</li>
        <li>The History row menu and the badge right-click share one item set; the sidebar menus keep their code for now.</li></ul></div>
    </div></div>`);
  const stage = document.getElementById('stage'), cap = document.getElementById('cap');

  function history(sc) {
    stage.innerHTML = '<div class="winwrap"></div>';
    const sh = G.shell({ host: stage.firstChild, w: W, h: H, view: 'history' }), win = sh.win;
    sh.center.innerHTML = '<div style="display:flex;flex-direction:column;flex:1;min-width:0;min-height:0;height:100%"><div class="hist-host" style="display:flex;flex-direction:column;flex:1;min-width:0;min-height:0"></div></div>';
    const list = G.commits.slice(); list[ROW] = Object.assign({}, G.commits[ROW], { refs: sc.refs.map((r) => Object.assign({}, r)) });
    G.history(sh.center.querySelector('.hist-host'), list, { gw: 112, sel: ROW });
    const row = win.querySelector(`.lrow[data-i="${ROW}"]`), wr = win.getBoundingClientRect(), rr = row.getBoundingClientRect();
    let pt = { clientX: wr.left + (sc.x || 520), clientY: rr.top + rr.height / 2 }, items;
    if (sc.badge) {
      const b = row.querySelector(`.ref[data-rn="${sc.badge}"]`).getBoundingClientRect(); pt = { clientX: b.left + b.width / 2, clientY: b.top + b.height / 2 };
      const ref = G.refMenus.model(list[ROW]).find((r) => r.n === sc.badge); items = G.refMenus.badge(win, ref, list[ROW]);
    } else items = G.refMenus.commit(win, list[ROW]);
    G.menu(win, null, items, { at: G.pos(win, pt), sticky: true, open: sc.open, kbd: sc.kbd });
  }

  /* 10: today's glued badge menu (reproduced from the app's local + remote menus) next to the new one */
  function beforeAfter() {
    stage.innerHTML = '<div class="rm-ba"><div class="rm-pane"><h3>Today · local and remote glued</h3><div class="rm-box" id="b0"><div class="rm-fake"></div></div></div><div class="rm-pane"><h3>New · one level, two sections</h3><div class="rm-box" id="b1"><div class="rm-fake"></div></div></div></div>';
    const b0 = document.getElementById('b0'), b1 = document.getElementById('b1'), n = 'main', rn = 'origin/main', h = G.headName();
    const flat = [
      { label: 'Checkout', icon: 'check', key: 'Enter', disabled: true }, { label: `Show all commits of ${n}`, icon: 'branch' },
      { label: `Merge ${n} into ${h}…`, icon: 'merge', disabled: true }, { label: `Rebase ${h} onto ${n}`, icon: 'branch', disabled: true }, { sep: 1 },
      { label: 'Rename…', icon: 'edit', key: 'F2' }, { label: 'Set upstream', icon: 'link' }, { label: 'Delete branch', icon: 'trash', key: 'Del', danger: 1, disabled: true }, { sep: 1 },
      { label: 'Open on GitHub', icon: 'external' }, { label: 'Create pull request…', icon: 'pr' }, { sep: 1 }, { label: 'Copy branch name', icon: 'copy' }, { sep: 1 },
      { label: 'Checkout as local branch', icon: 'check', key: 'Enter', disabled: true }, { label: `Show all commits of ${rn}`, icon: 'branch' }, { label: `Merge ${rn} into ${h}…`, icon: 'merge', disabled: true }, { sep: 1 },
      { label: 'Delete remote branch…', icon: 'trash', danger: 1 }, { sep: 1 }, { label: 'Open on GitHub', icon: 'external' }, { sep: 1 }, { label: 'Copy branch name', icon: 'copy' }];
    const win = (el) => { el.dataset.menuHost = ''; return el; };
    G.menu(win(b0), null, flat, { at: { x: 24, y: 16 }, sticky: true });
    const c = Object.assign({}, G.commits[0], { refs: PAIR.map((r) => Object.assign({}, r)) });
    G.menu(win(b1), null, G.refMenus.badge(b1, G.refMenus.model(c)[0], c), { at: { x: 24, y: 16 }, sticky: true });
    G.initTips(b1);
  }

  function show(i) {
    cur = i; const sc = S[i - 1];
    document.querySelectorAll('[data-s]').forEach((b) => b.setAttribute('aria-pressed', String(+b.dataset.s === i)));
    cap.innerHTML = `<b>${i} · ${esc(sc.t)}.</b> ${sc.note}`;
    if (sc.ba) beforeAfter(); else history(sc);
    try { history_.replaceState(null, '', '?s=' + i); } catch (e) {}
  }
  const history_ = window.history;
  document.querySelector('.rm-tabs').addEventListener('click', (e) => { const b = e.target.closest('[data-s]'); if (b) show(+b.dataset.s); });
  show(cur);
})();
