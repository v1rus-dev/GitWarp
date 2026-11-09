/* GitWarp: error-fix dialog, generic confirm dialogs and notices (toasts).
   Sources: crates/app/src/error_dialog.rs (w560), confirm_dialog.rs (w440), update_check.rs, ui/src/components/toast.rs,
   repo_view.rs (show_git_error, show_job_error, show_push_result, account toasts). Strings are the en locales verbatim.
   Design-prototype code: the dialogs run live through D.open (dialogs.js). */
(function () {
  'use strict';
  const G = window.GL, D = window.GLD, ic = G.ic, esc = G.esc;
  const E = (window.GLE = {});

  /* ---------- error-fix dialog ---------- */
  /* Reused from dialogs.js (D.errorFix, kinds worktree / local / push / diverged / other): one implementation, no fork. */
  E.errorDialog = (win, init) => D.errorFix(win, { kind: (init && init.k) === 'fallback' ? 'other' : (init && init.k) || 'worktree' });

  /* ---------- generic confirm ---------- */
  /* ConfirmDialog::open(title, body, primary, action): Cancel (min w72) + danger primary (min w96), Cancel is focused, Enter does not confirm.
     The only caller is the remote-branch delete; "Confirm" is the title of the empty shell shown when the dialog state is gone. */
  E.confirmShell = function (win) {
    return D.open(win, { st: {}, width: 440, view: () => ({ title: 'Confirm', icon: 'warn', body: '<div></div>', foot: '<button type="button" class="btn" style="min-width:72px" data-do="close" data-autofocus>Cancel</button>' }) });
  };

  /* ---------- toasts (static strip, nothing auto-hides so the drawing stays put) ---------- */
  /* spec: { kind, msg, lines: [{ok, text, detail, action}], action, copy } — toast.rs: Error toasts get a Copy icon button before Dismiss;
     a toast-level action is a label button (Xs) before Dismiss; line actions sit at the end of their line. */
  const kindIcon = (k) => (k === 'error' ? 'err' : k === 'warn' ? 'warn' : k === 'ok' ? 'check' : 'info');
  const toastEl = (t) => {
    const lines = t.lines || [], has = lines.length > 0;
    const el = G.h(`<div class="toast ${t.kind}${has ? ' has-lines' : ''}${t.lines || t.action ? ' has-body' : ''}" role="status">${ic(kindIcon(t.kind), 14)}<div class="toast-col"><span class="toast-msg">${esc(t.msg)}</span>${lines.map((l, i) => `<div class="tl ${l.ok ? 'ok' : 'bad'}">${ic(l.ok ? 'check' : 'err', 12)}<span>${esc(l.text)}</span>${l.detail ? `<span class="tl-d">${esc(l.detail)}</span>` : '<span class="od-fill"></span>'}${l.action ? `<button type="button" class="btn tbtn" data-l="${i}">${esc(l.action)}</button>` : ''}</div>`).join('')}</div>${t.kind === 'error' ? `<button type="button" class="btn btn-icon btn-ghost btn-xs" data-copy aria-label="Copy" data-tip="Copy">${ic('copy', 12)}</button>` : ''}${t.action ? `<button type="button" class="btn tbtn" data-a>${esc(t.action)}</button>` : ''}<button type="button" class="btn btn-icon btn-ghost btn-xs" data-x aria-label="Dismiss" data-tip="Dismiss">${ic('x', 12)}</button></div>`);
    el.querySelector('[data-x]').onclick = () => el.remove();
    const cp = el.querySelector('[data-copy]'); if (cp) cp.onclick = () => { try { navigator.clipboard.writeText([t.msg].concat(lines.map((l) => l.text + (l.detail ? ': ' + l.detail : ''))).join('\n')); } catch (e) {} };
    const a = el.querySelector('[data-a]'); if (a) a.onclick = () => { el.remove(); };
    el.querySelectorAll('[data-l]').forEach((b) => { b.onclick = () => b.closest('.toast').remove(); });
    return el;
  };
  E.toasts = function (win, list) {
    const layer = G.layer(win); let c = layer.querySelector('.toasts'); if (c) c.remove();
    c = G.h('<div class="toasts" style="align-items:flex-end"></div>'); layer.appendChild(c);
    list.forEach((t) => c.appendChild(toastEl(t)));
  };

  E.updateNotice = (win) => E.toasts(win, [{ kind: 'info', msg: 'GitWarp v0.3.0 is available', action: 'View release' }]);
  E.errorToast = (win) => E.toasts(win, [{ kind: 'error', msg: 'The SSH key was rejected. Check that your key is loaded in ssh-agent and added to the account on the Git host.' }]);
  E.tokenRejected = (win) => E.toasts(win, [{ kind: 'error', msg: 'github.com rejected the token of mira-k', action: 'Reconnect' }]);
  E.accountToasts = (win) => E.toasts(win, [
    { kind: 'ok', msg: 'Connected mira-k-oss on github.com. origin now uses it.' },
    { kind: 'ok', msg: 'origin will use mira-k-oss in this repository' },
    { kind: 'info', msg: 'github.com has one account (mira-k); origin already uses it.' },
    { kind: 'info', msg: 'GitWarp has no account for git.corp.example. Add one in Settings → Accounts.' },
    { kind: 'info', msg: 'Each push URL keeps its own account. Edit them in Settings → Accounts.' },
    { kind: 'error', msg: 'Could not remember the account for origin. Nothing was started.' }
  ]);
  E.accountToasts2 = (win) => E.toasts(win, [
    { kind: 'info', msg: 'Opened github.com in your browser' },
    { kind: 'ok', msg: 'origin now tracks origin/feat/palette' },
    { kind: 'info', msg: 'There is no origin/feat/palette yet. Push feat/palette first to create it.' },
    { kind: 'ok', msg: 'Reconnected mira-k' }
  ]);
  /* show_push_result: reused from dialogs.js (D.toastStrip with lines) instead of a second toast builder. */
  const L = (ok, text, detail, action) => ({ ok, text, detail, action });
  E.pushAll = (win) => D.toastStrip(win, [['ok', 'Pushed main to 2 targets', { lines: [L(true, 'github.com · mira-k', '3 commits'), L(true, 'git.corp.example · mira@corp', '3 commits')] }]]);
  E.pushSome = (win) => D.toastStrip(win, [['error', 'Pushed main to 1 of 3 targets', { lines: [L(true, 'github.com · mira-k', '3 commits'), L(false, 'gitlab.com · mira-k', 'Token invalid or revoked', 'Reconnect'), L(false, 'git.corp.example · mira@corp', 'Some refs pushed, others refused: protected branch')] }]]);
  E.pushKinds = (win) => D.toastStrip(win, [
    ['ok', 'Pushed tag v0.3.0 to 2 targets', { lines: [L(true, 'github.com · mira-k', 'new tag'), L(true, 'git.corp.example · mira@corp', 'up to date')] }],
    ['ok', 'Pushed feat/palette to 2 targets', { lines: [L(true, 'github.com · mira-k', 'new branch'), L(true, 'git.corp.example · mira@corp', 'pushed')] }],
    ['ok', 'Pushed origin/feat/old-spike to 2 targets', { lines: [L(true, 'github.com · mira-k', 'deleted'), L(true, 'git.corp.example · mira@corp', 'deleted')] }]]);
})();
