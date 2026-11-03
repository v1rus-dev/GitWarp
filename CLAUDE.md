# GitWarp — instructions for Claude

## Division of labour

Claude plans and verifies; OpenCode implements.

- **Claude** (main session): breaks work into steps, writes the brief for each step,
  reviews the result (diff review, build, tests, comparison with the design) and decides
  whether the step is accepted. Claude does not write product code itself, beyond the
  trivial-edit carve-out of the global rules.
- **OpenCode** with `opencode-go/deepseek-v4.1-flash` writes the code. This replaces the
  global "implementation subagents run on Sonnet" rule for this project.
  The `opencode/…` provider of the same model has no funds — do not use it.
- **Fallback executor: Claude Sonnet.** If OpenCode stops responding (terminal stays busy with no
  progress, crashes, hangs), or reports exhausted limits / quota / billing errors, do not wait and do
  not retry in a loop: close its terminal and hand the same brief to an Agent subagent with
  `model: "sonnet"` (default subagent type). The brief, verification and per-step commits stay the same.
  Go back to OpenCode for the next step only if the failure was transient (not a limit).

### Delegating a step

Launch OpenCode through Orca, so the user sees it as an agent tab in the current worktree
(a plain background `opencode run` is invisible in Orca — do not use it):

```sh
orca agent launch --agent opencode2 \
  --surface terminal --prompt "<short pointer: read swarm-report/<stage>/<step>-brief.md and do it>" --json
```

The Orca agent is `opencode2` (`opencode` is disabled) and it rejects `--model`. The model comes from an
untracked `opencode.json` in the worktree root (listed in `.git/info/exclude`):
`{"$schema": "https://opencode.ai/config.json", "model": "opencode-go/deepseek-v4.1-flash"}`.
Create it in every new worktree before launching — the global OpenCode config defaults to another model.
Check the TUI footer says "DeepSeek V4.1 Flash OpenCode Go" right after launch.

Then wait with `orca terminal wait --terminal <handle> --for tui-idle --timeout-ms <ms> --json`
and read the result with `orca terminal read --terminal <handle> --json`.
Fix rounds are sent to the same terminal with `orca terminal send --terminal <handle> --text "…" --enter --json`.

- The brief must say exactly which files may change and which commands to run, and forbid
  commits, pushes, dependency additions not listed in the brief, and edits outside the repo.
- The brief lives in `swarm-report/<stage>/<step>-brief.md` and is self-contained: goal,
  files to touch, the relevant design excerpt (tokens/spec values copied in, not "see the
  design"), API notes for GPUI (gpui-pre, see Cargo.toml) found during planning, acceptance criteria, and the
  check commands to run before finishing.
- One OpenCode session per step: fix rounds of that step go to the same terminal (the model keeps its
  context); once the step is accepted, close that terminal (`orca terminal close --terminal <handle>`) and
  launch a fresh agent for the next step, so its context does not grow across steps. Briefs are
  self-contained, so nothing is lost.

### Verifying a step

Claude never accepts a step on OpenCode's own report. Before acceptance:

1. Read the full `git diff` of the step: scope matches the brief, no stray files,
   no new dependencies beyond the approved ones.
2. `cargo build -p app`, `cargo build -p ui --examples`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo fmt --all -- --check`, `cargo test`, `cargo test -p ui` and `cargo test -p app` (`ui` and `app` are not
   default members, so plain `cargo test` skips them).
3. UI steps: run the app and compare against the design snapshot in `docs/design`.
4. Findings go back to OpenCode as a fix round; after two failed rounds on the same issue
   Claude stops and reports to the user.

Commits are made by Claude after acceptance, on a feature branch, one per accepted step.

## Design source

The design is the OpenDesign project «gitlane Git Client Design»; the in-repo snapshot
under `docs/design` is the source of truth for acceptance. The product name is GitWarp —
the design still says "gitlane" in places; never carry that name into code.

The stage roadmap and the page → stage mapping live in `docs/design/README.md`.
