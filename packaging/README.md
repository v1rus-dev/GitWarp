# Release packaging

Each archive is built in three steps: render the third-party licences, produce
the bundled git, then package.

```sh
cargo build --release -p app
packaging/licenses.sh <linux|windows|macos> <bundle-dir>             # writes <bundle-dir>/THIRD-PARTY-LICENSES.html
packaging/git/fetch-or-build.sh <linux|windows|macos> <bundle-dir>   # writes <bundle-dir>/git
packaging/package.sh <linux|windows|macos> <bundle-dir> <out-dir>    # writes one archive, prints its path
```

`licenses.sh` needs [cargo-about](https://github.com/EmbarkStudios/cargo-about)
(`cargo install --locked cargo-about --version 0.9.3 --features cli`). It reads
`about.toml` and `about.hbs` at the repository root and fails when a crate uses a
licence that is not in the `accepted` list there: adding a licence to that list is
a deliberate decision, not a fix for a red build.

`CARGO_TARGET_DIR` is honoured when the binary is not in `target/`. Each
archive must be built on its own OS: `iconutil`, `codesign` and `ditto` are
needed on macOS; Windows uses `7z` (or `zip`) under Git Bash.

## Layouts

- `gitwarp-<ver>-linux-x86_64.tar.gz` — `gitwarp-<ver>-linux-x86_64/` with
  `gitwarp`, `git/`, `install.sh`, `dev.v1rus.gitwarp.desktop`, `icons/`,
  `LICENSE-MIT`, `LICENSE-APACHE`, `THIRD-PARTY-LICENSES.html`, `README.md`. `install.sh` installs into
  `~/.local` (or `$PREFIX`); `install.sh --uninstall` removes it.
- `gitwarp-<ver>-windows-x86_64.zip` — `gitwarp-<ver>-windows-x86_64/` with
  `gitwarp.exe`, `git/`, licences, `README.md`.
- `gitwarp-<ver>-macos-aarch64.zip` — `GitWarp.app` (`Contents/Info.plist`
  from `packaging/macos/Info.plist`, `Contents/MacOS/gitwarp`,
  `Contents/Resources/gitwarp.icns`, `Contents/Resources/git/`, licences).

## Signing

The macOS and Windows builds are not signed with a developer certificate. The
macOS bundle is ad-hoc signed (required to run on Apple Silicon), so users
still see a Gatekeeper warning; on Windows SmartScreen warns about an unknown
publisher.
