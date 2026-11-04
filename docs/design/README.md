# GitWarp design

This folder is a snapshot of the GitWarp UI design prototype, made in an external design
tool (OpenDesign project «gitlane Git Client Design») and copied here so the design can be
reviewed without that tool. It is the source of truth for UI acceptance: where the
implementation and the prototype disagree, the prototype wins. On 2026-11-02 the prototype was
re-synced with the app as shipped through stage 10, so it
now also shows the dialogs, states and menus the app gained without a design page, and the
decisions listed below are drawn in it. The product name is GitWarp; the OpenDesign project is
still named "gitlane". To view it, open `prototype/index.html` in a browser —
the pages are self-contained and link to each other by relative path — and use the theme
button at the top right to switch between dark and light. Static screenshots of every page in
both themes are in `screens/` (`<page>.dark.png` and `<page>.light.png`).

| Page file | What it shows | Implementation stage |
| --- | --- | --- |
| `tokens.html` | Design tokens: colour (dark + light, AA-checked live), type scale, spacing, radii, sizes. | 1 — UI foundation |
| `components.html` | Component set: buttons, inputs, tabs, tree, rows, menus, tooltips, dialogs, toasts — with states. | 1 — UI foundation |
| `main-1200.html` | Main window 1200×800: history + details docked bottom, unified diff. Interactive. | 2 — window shell, welcome, states |
| `main-1600.html` | Main window 1600×1000: details docked right, side-by-side diff. Interactive. | 2 — window shell, welcome, states |
| `welcome.html` | Welcome / repo picker: recent repos, Open, Clone, Init dialogs. | 2 — window shell, welcome, states |
| `history-states.html` | History states: loading, empty repo, not a repo, git missing / too old, generic git error. | 2 — window shell, welcome, states |
| `dialogs-repo.html` | Dialogs: clone / open — clone from URL (HTTPS/SSH, progress, errors) and open from folder, with all states. | 2 — Open tab; 8 — Clone tab |
| `graph.html` | Commit graph spec: geometry, golden-angle lanes on both themes, clipping, widening, idle-lane collapse. | 3 — history |
| `changes.html` | Local changes: stage by file, hunk, line; commit editor with amend and character guides. | 6 — local changes |
| `dialogs-ref.html` | Dialogs: branch / tag here — create branch here and create tag here, with validation states. | 7 — ref operations |
| `dialogs-worktree.html` | Worktrees: worktree list, add and remove dialogs, locked / dirty / missing states. | 9 — worktrees |
| `worktrees.html` | Worktrees workspace: titlebar switcher (Ctrl+Alt+W) with create-on-type, sidebar worktree panel, detail pane, quick create from a branch name. | post-10 — W3–W5 |
| `settings-worktrees.html` | Settings → Worktrees: base folder, folder template, behaviour after creating, confirmations. | post-10 — W2 |
| `settings-general.html` | Settings → General: language, startup, updates, background fetch, confirmations, privacy. | 10 — settings |
| `settings-appearance.html` | Settings → Appearance: theme, accent, density, fonts, graph style, live preview. | 10 — settings |
| `settings-git.html` | Settings → Git: Auto / Bundled / System / Custom, with all warning and error states (scenario switcher). | 10 — settings |
| `settings-keybindings.html` | Settings → Keybindings: searchable shortcut table, recording, conflicts, reset and export. | 10 — settings |
| `settings-diff.html` | Settings → Diff & merge: layout, whitespace, context, merge tool with command validation, live preview. | 10 — settings |
| `settings-identity.html` | Settings → Identity: author, signing key, validation states; hosting accounts moved to Settings → Accounts. | 10 — settings |
| `settings-accounts.html` | Settings → Accounts: any number of hosting accounts (several of one type, self-hosted), statuses, import from gh / glab. | H3 — hosting accounts |
| `dialogs-account.html` | Add / Reconnect account: service → instance (probe, TLS trust) → token or browser sign-in (device code, redirect), results. | H4–H5 — hosting accounts |
| `remotes-accounts.html` | Sidebar remotes with service + account, account picker per remote, Add account for host, remote with several push URLs. | H6 — remotes ↔ accounts |
| `hosting-features.html` | Open on host, Clone From account, PR badges and popover, CI status and checks, Create PR, PR panel. | H7–H10 — hosting features |
| `settings-ssh.html` | Settings → SSH keys: key list, generate / import / delete, connection test, trusted hosts. | 10 — settings |
| `palette.html` | Command palette: Ctrl+P quick launcher for commands, branches, commits, files. | 11 — command palette |
| `dialogs-checkout.html` | Checkout with uncommitted changes: stash / bring changes / discard, for branch, remote branch, tag and commit. | 7 — ref operations |
| `dialogs-merge.html` | Dialogs: merge / rebase — merge dialog with conflict preview and merge types, rebase dialog, new branch-menu items; the Merge / Rebase in progress banner and Conflicts group are states of `changes.html` (Operation switcher, `?op=`). | 12 — merge / rebase |
| `conflicts.html` | Conflict editor: resolve a conflicted file chunk by chunk — two side panes (optional base), per-chunk checkboxes, live editable result (picked / both / edited / auto), binary and modify/delete cards, rebase side labelling. Opened from Changes (Resolve…). | 12 — conflict editor |
| `dialogs-network.html` | Network prompts: HTTPS credentials, SSH host key (first connection, key changed), result toasts. | 8 — network |
| `activity.html` | Proposal, not scheduled: commit-activity heatmap; placement A (Welcome) vs B (strip above History, recommended), all states. | — |
| `icons.html` | Icon set: the app's own icons next to their Primer Octicons replacement (adopted in the prototype). | 1 — UI foundation |
| `index.html` | Overview and keyboard model. | Overview |

## Decisions that override the original prototype spec

The prototype now draws all of these; they are kept here because they contradict captions
and specs written before the re-sync.

Both themes share one geometry and differ only in colour.

- Buttons: primary has an accent border (accent-hover on hover), danger an error@55% border, a
  disabled ghost button has no border. (The prototype's CSS cascade turns them all grey.)
- Error toasts stay until dismissed; other toasts auto-hide after 5 s. (The prototype hides all
  toasts after 5 s.)
- Splitter hover/drag shows a 2px accent line centred in the 5px hit area. (The prototype fills
  the whole 5px strip.)
- Tooltips use GPUI's built-in tooltip (500 ms delay, placed at the cursor) instead of 350 ms
  anchored below the element.
- Scrollbars are always visible: 6px thumb in border-strong. (The caption says hidden until hover.)
- No hover/press transitions; only the switch and progress bars animate.
- Font weights 400, 500, 600 and 700 are all used.
- The history header gets the same 6px side margin as the rows, so the columns line up.
- A keyboard-focused tree node and a focused segment show an inset 2px focus ring.
- Disabled checkbox, radio and switch are drawn at 45% opacity.
- Row density defaults to 26px (Normal); the Appearance settings change it.
- Light-theme graph lightness is 45% (the token value).
- Icons are Primer Octicons 19.42.0 (MIT, 16px fill set) instead of the hand-drawn stroke set;
  the Git mark, keyboard, command and window-control icons stay our own (`icons.html`).
- Settings opens as a centred modal card over the repository window, not as a separate window.

## Roadmap

0. Design snapshot (this folder)
1. UI foundation: theme, icons, fonts, actions/keymap, overlays, component kit, gallery
2. Window shell + welcome + history states
3. History to spec: columns, ref labels, graph details, idle-lane collapse, search
4. Sidebar: branches, remotes, tags, stashes, worktrees, submodules, context menus
5. Commit details + diff (unified / side-by-side, syntax highlighting)
6. Local changes: stage by file / hunk / line, commit, amend
7. Ref operations and dialogs: checkout, branch/tag/remote, cherry-pick, revert, reset, stash
8. Network: fetch / pull / push, clone, SSH host keys, background fetch
9. Worktrees
10. Settings pages
11. Command palette
12. Merge / rebase (`dialogs-merge.html`, Changes operation states). Conflict editor (`conflicts.html`). Not designed yet: interactive rebase, blame, file history

Hosting services (GitHub, GitLab, Gitea / Forgejo, Bitbucket, Azure DevOps, self-hosted) are planned
separately as steps H1–H10; spec in `docs/specs/hosting-services-spec.md`.
