# Merge, rebase and branch-menu additions (from the Fork review)

Status: Step 0 (design) and Step 1 (merge: dialog, menus, Changes banner and Conflicts group,
squash) done. Next: "Open in merge tool" for conflicted files, then Step 2. Before Step 3, generalise
the Changes operation state (merge-only today) and move conflict side labels into git-core.

## Where we are

- Sidebar menu of a local branch already has "Merge {name} into {head}" and
  "Rebase {head} onto {name}"; a remote branch has "Merge into". All of them are
  shown-but-disabled (`crates/app/src/sidebar.rs`, `sidebar_menu_with_links`,
  comment "merge/rebase (stage 12)"). Strings: `crates/ui/locales/*/sidebar.json`
  (`sidebar.menu.merge_into`, `sidebar.menu.rebase_onto`).
- The History row menu and the ref-badge menu have no Merge / Rebase at all
  (`crates/app/src/history_view.rs`, `RowMenu::entries`).
- `crates/git-core/src/ops.rs` has checkout, branch, tag, remote, cherry-pick, revert,
  reset (`ResetMode`) and stash. No merge, rebase, abort or continue.
- Conflicted files already show in Changes (`status.rs` `conflicted`, `changes_view.rs`).
  There is no conflict editor.
- Pull is `--ff-only` (`git-core/src/network.rs`); a diverged branch tells the user to use
  a terminal (`network.friendly.diverged`).
- `docs/design/README.md` item 12 says merge/rebase + conflict editor, interactive rebase,
  blame, file history are "not designed yet".

## Step 0 — design (needs the OpenDesign project)

Add `dialogs-merge.html` to the prototype and a snapshot to `docs/design` (prototype +
`screens/<page>.dark.png` / `.light.png`), and add its row to the table in
`docs/design/README.md`. Style: same as `dialogs-ref.html`. Reference: Fork's Merge dialog.

Merge dialog:

- Rows: `Merge:` branch being merged, `Into:` current branch (HEAD), `Merge Type:` select.
- Merge types: No Fast-Forward (always create a merge commit), Fast-forward if possible,
  Fast-forward only, Squash.
- A status line, computed when the dialog opens, in four states:
  - clean: green check, "Merge can be done without conflicts", primary button **Merge**;
  - conflicts: warning with the conflicting file list (collapsed if long), primary button
    **Merge Conflicts**; it runs the merge and switches to the Changes tab, where the
    files are already under Conflicts;
  - up to date: "Nothing to merge", button disabled;
  - checking / check failed: spinner / error text with the Merge button still usable.
- Fast-forward only and a diverged branch: line says it cannot fast-forward, button disabled.
- Also draw: the "Merge in progress" banner (Abort, Commit) and the "Rebase in progress"
  banner (Continue, Skip, Abort) shown above Changes, and the Rebase dialog (same shape;
  options: autostash, rebase merges).
- Menus: where the new items sit in the branch menus and in the History row / badge menus.

## Step 1 — merge (acceptance: the design from Step 0)

git-core (`ops.rs`, exposed in `lib.rs`):

- `merge(branch, MergeType)`; the result distinguishes done / conflicts / nothing to merge.
- `merge_preview(branch) -> clean | conflicts(Vec<path>) | up_to_date | can_fast_forward`.
  Planned implementation: `git merge-tree --write-tree HEAD <branch>`; exit 0 is clean,
  exit 1 is conflicts with the file list in the output. It does not touch the work tree.
  Needs git 2.38+; the app lets the user pick Auto/Bundled/System git, so check the
  version and, for older git, fall back to a status line without a prediction.
  This is a hypothesis from the docs — verify with a real run on a scratch repo first.
- `merge_abort`, and a way to ask "is a merge / rebase in progress" (`MERGE_HEAD`,
  `rebase-merge/`, `rebase-apply/` in the git dir).

app:

- Merge dialog (new file next to `ref_dialog.rs`), strings in all six locales
  (en, ru, es, fr, ja, de).
- Enable the sidebar items (local branch, remote branch) and add "Merge into {head}…" to
  the History row menu (for a branch on that commit) and to the ref-badge menu. All entry
  points open the same dialog. Disabled for HEAD itself and when HEAD is detached.
- "Merge in progress" banner with Abort; Changes keeps working as is.
- Command palette entry if the other ref commands have one.

Tests: preview states against a scratch repo (clean, conflicts, up to date, fast-forward);
menu model tests in `sidebar.rs` / `history_view.rs` in the existing style.

## Step 2 — fast-forward and push from the branch menu

- Local branch with an upstream that is ahead: "Fast-Forward to 'origin/X'"
  (`git merge --ff-only`; for a non-current branch `git fetch . origin/X:X`).
- "Push to 'origin'…" in the local branch menu (tags already have push).

## Step 3 — rebase

- `rebase(onto, options)`, `rebase_continue`, `rebase_skip`, `rebase_abort`.
- Rebase dialog and the "Rebase in progress" banner from Step 0.
- Enable "Rebase {head} onto {name}" in the sidebar and add it to the History menus.

## Step 4 — cheap extras (backend mostly exists)

- "Checkout as Worktree…" in the branch menu (worktrees exist since stage 9).
- Reset to here with Soft / Mixed / Hard choice (`ResetMode` exists).
- Compare to Local Changes / compare two commits (diff engine exists).
- Save as Patch… (`git format-patch -1`).
- Pin / hide / filter a branch in the sidebar.

## Later, needs its own design

Interactive rebase, blame, file history. The conflict editor is now designed
(`docs/design/prototype/conflicts.html`) and is the next step after merge. Fork's AI submenu is deliberately not taken.

## Checks per step (from CLAUDE.md)

`cargo build -p app`, `cargo build -p ui --examples`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`,
`cargo test`, `cargo test -p ui`, `cargo test -p app`. UI steps: run the app and compare with
the snapshot in `docs/design`. One commit per accepted step.
