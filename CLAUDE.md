# GitWarp — instructions for Claude

## Division of labour

Claude plans and verifies; a Claude Sonnet subagent implements.

- **Claude** (main session): breaks work into steps, writes the brief for each step,
  reviews the result (diff review, build, tests, comparison with the design) and decides
  whether the step is accepted. Claude does not write product code itself, beyond the
  trivial-edit carve-out of the global rules.
- **Claude Sonnet** (Agent subagent, `model: "sonnet"`, default subagent type, run in the
  background) writes the code. OpenCode is no longer used.

### Delegating a step

- The brief must say exactly which files may change and which commands to run, and forbid
  commits, pushes, `git stash`, dependency additions not listed in the brief, and edits outside the repo.
- The brief lives in `swarm-report/<stage>/<step>-brief.md` and is self-contained: goal,
  files to touch, the relevant design excerpt (tokens/spec values copied in, not "see the
  design"), API notes for GPUI (gpui-pre, see Cargo.toml) found during planning, acceptance criteria, and the
  check commands to run before finishing. The agent prompt is a short pointer to the brief.
- One agent per step: fix rounds of that step go to the same agent via SendMessage (it keeps its
  context); once the step is accepted, the next step gets a fresh agent, so context does not grow
  across steps. Briefs are self-contained, so nothing is lost.

### Verifying a step

Claude never accepts a step on the agent's own report. Before acceptance:

1. Read the full `git diff` of the step: scope matches the brief, no stray files,
   no new dependencies beyond the approved ones.
2. `cargo build -p app`, `cargo build -p ui --examples`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo fmt --all -- --check`, `cargo test`, `cargo test -p ui` and `cargo test -p app` (`ui` and `app` are not
   default members, so plain `cargo test` skips them).
3. UI steps: run the app and compare against the design snapshot in `docs/design`.
4. Findings go back to the agent as a fix round; after two failed rounds on the same issue
   Claude stops and reports to the user.

Commits are made by Claude after acceptance, on a feature branch, one per accepted step.

## Design source

The design is the OpenDesign project «gitlane Git Client Design»; the in-repo snapshot
under `docs/design` is the source of truth for acceptance. The product name is GitWarp —
the design still says "gitlane" in places; never carry that name into code.

The stage roadmap and the page → stage mapping live in `docs/design/README.md`.
