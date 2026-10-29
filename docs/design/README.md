# GitWarp design

This folder is a snapshot of the GitWarp UI design prototype, made in an external design
tool and copied here unchanged so the design can be reviewed without that tool. It is the
source of truth for UI acceptance: where the implementation and the prototype disagree, the
prototype wins, except for the decisions listed below. The prototype still says "gitlane" in
places; the product name is GitWarp. To view it, open `prototype/index.html` in a browser —
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
| `settings-general.html` | Settings → General: language, startup, updates, background fetch, confirmations, privacy. | 10 — settings |
| `settings-appearance.html` | Settings → Appearance: theme, accent, density, fonts, graph style, live preview. | 10 — settings |
| `settings-git.html` | Settings → Git: Auto / Bundled / System / Custom, with all warning and error states (scenario switcher). | 10 — settings |
| `settings-keybindings.html` | Settings → Keybindings: searchable shortcut table, recording, conflicts, reset and export. | 10 — settings |
| `settings-diff.html` | Settings → Diff & merge: layout, whitespace, context, merge tool with command validation, live preview. | 10 — settings |
| `settings-identity.html` | Settings → Identity: author, signing key, GitHub / GitLab accounts, validation states. | 10 — settings |
| `settings-ssh.html` | Settings → SSH keys: key list, generate / import / delete, connection test, trusted hosts. | 10 — settings |
| `palette.html` | Command palette: Ctrl+P quick launcher for commands, branches, commits, files. | 11 — command palette |
| `index.html` | Overview and keyboard model. | Overview |

## Decisions where the prototype contradicts its own spec

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
- Row density is fixed at 26px until the Appearance settings are implemented.
- Light-theme graph lightness is 45% (the token value).

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
12. Not designed yet: merge/rebase + conflict editor, interactive rebase, blame, file history
