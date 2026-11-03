# Contributing to GitWarp

GitWarp is a desktop Git client written in Rust with [GPUI](https://www.gpui.rs) (the UI
framework of Zed). The minimum supported Rust version is **1.89** (`rust-version` in
`Cargo.toml`): `uuid` 1.29 (via GPUI) and `aes` 0.9 (via the Linux keychain store)
require it; `gix` 0.90 alone needs 1.88.

## Layout

```
crates/
  git-graph/   commit-graph lane layout: incremental, UI-agnostic, no dependencies
  git-core/    repository access: gix opens the repo; git still runs mutations, log, refs
  ui/          the UI kit: theme, components, icons, fonts and the i18n catalog (locales/)
  app/         the GPUI desktop app (binary: `gitwarp`)
```

The rule: `git-graph` and `git-core` know nothing about the UI. They are tested and
benchmarked on their own, and the UI layer can change without touching them.

`ui` and `app` are not default workspace members, so a plain `cargo test` or
`cargo build` skips the GPUI parts.

## Build and run

```sh
cargo run --release -p app -- /path/to/repo  # the app
```

System packages for the app on Linux (Debian/Ubuntu names; see `.github/workflows/ci.yml`):

```sh
sudo apt-get install -y libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libvulkan-dev libfontconfig-dev libssl-dev
```

Windows needs nothing besides the Rust toolchain (MSVC). macOS needs the full Xcode,
not only the command line tools: GPUI compiles its Metal shaders with `xcrun metal` at
build time (or enable GPUI's `runtime_shaders` feature). CI builds all three.

### Install on Linux (desktop entry)

After `cargo build --release -p app`, install the binary, desktop entry and hicolor
icons for the current user:

```sh
./packaging/linux/install.sh            # into ~/.local
PREFIX=/some/prefix ./packaging/linux/install.sh
```

The desktop entry is named `dev.v1rus.gitwarp.desktop` so its basename matches the
window's `app_id`; Wayland compositors use that to show the icon in the launcher and
taskbar.

## Checks

Run these before sending a change (`ui` and `app` are not default members, hence the
separate test commands):

```sh
cargo build -p app
cargo build -p ui --examples
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo test
cargo test -p ui
cargo test -p app
```

## Which git

Opening a repository does not spawn `git`. [`GitRunner`](crates/git-core/src/runner.rs)
discovers it with in-process [`gix`](https://github.com/GitoxideLabs/gitoxide)
([`GitEngine::Native`](crates/git-core/src/git.rs), the default). Mutations, the log,
status and anything that must keep hooks, credentials, LFS or signing still run a `git`
executable (minimum version 2.31). `GitEngine::Cli` is the old `git rev-parse` discovery,
kept so the two can be compared.

Pick the executable with `--git <auto|system|bundled|PATH>` or the `GITWARP_GIT`
environment variable (the flag wins); the default `auto` uses the bundled git if present,
else the one on `PATH`. The header shows what is in use, e.g. `git 2.56.0 (system)`.

The bundled git is looked up next to the app binary:

| OS | Path |
|---|---|
| Windows | `<app dir>/git/cmd/git.exe` (MinGit) |
| macOS | `<app dir>/../Resources/git/bin/git` |
| Linux | `<app dir>/git/bin/git`, then `<app dir>/../lib/gitwarp/git/bin/git` |

[`packaging/git/fetch-or-build.sh <linux|windows|macos> <out-dir>`](packaging/git/README.md)
produces the `git/` directory to ship at those locations (a relocatable build on
Linux/macOS, MinGit on Windows). Dev builds have no bundle and fall back to the system git.

## Benchmark

```sh
cargo run --release -p git-core --example graph_bench -- /path/to/repo --draw 30
```

Prints an ASCII graph of the first rows and the timings. On git.git (~86k commits) in a
dev container:

| | without commit-graph | with commit-graph |
|---|---|---|
| first 100 rows laid out | ~2.2 s | ~0.13 s |
| whole history | ~2.5 s | ~1.9 s |

Without a commit-graph file `git log --topo-order` has to walk the whole history before
printing anything. The app writes the file in the background on first open
(`Repo::write_commit_graph`).

## How the graph works

`GraphBuilder::push(id, parents)` takes commits in topological order and returns a `Row`:
the column of the dot plus the line segments in the row's upper half (children arriving)
and lower half (lines leaving to parents). Rows never change after they are produced, so
history is laid out page by page while it loads and any slice can be painted independently.

- The first parent always continues in the commit's own lane, so branches that share a
  base stay separate lines until the fork point.
- A merged-in parent joins a lane that already waits for it, or opens a new lane to the right.
- Freed columns are reused; memory is proportional to open lanes, not history.

## Translations

UI strings live in `crates/ui/locales/<lang>/*.json` (`en`, `ru`, `de`, `fr`, `es`, `ja`).
Add new keys to every language.

## Releases

Maintainers: see [RELEASING.md](RELEASING.md) and [packaging/README.md](packaging/README.md).

## License

By contributing you agree that your contribution is dual licensed under
`MIT OR Apache-2.0`, as described in the [README](README.md#license).
