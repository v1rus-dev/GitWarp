# Release packaging

Each artifact set is built in three steps: render the third-party licences,
produce the bundled git, then package.

```sh
cargo build --release -p app
packaging/licenses.sh <linux|windows|macos> <bundle-dir>             # writes <bundle-dir>/THIRD-PARTY-LICENSES.html
packaging/git/fetch-or-build.sh <linux|windows|macos> <bundle-dir>   # writes <bundle-dir>/git
packaging/package.sh <linux|windows|macos> <bundle-dir> <out-dir>    # writes the artifacts, prints each path
```

`licenses.sh` needs [cargo-about](https://github.com/EmbarkStudios/cargo-about)
(`cargo install --locked cargo-about --version 0.9.3 --features cli`). It reads
`about.toml` and `about.hbs` at the repository root and fails when a crate uses a
licence that is not in the `accepted` list there: adding a licence to that list is
a deliberate decision, not a fix for a red build.

`CARGO_TARGET_DIR` is honoured when the binary is not in `target/`. Each
set must be built on its own OS. Tools `package.sh` needs:

- macOS: `iconutil`, `codesign`, `hdiutil` (all built in).
- Windows (under Git Bash): `7z` or `zip`, and Inno Setup 6 (`iscc`, `ISCC.exe`
  or `C:\Program Files (x86)\Inno Setup 6\ISCC.exe`; preinstalled on
  `windows-latest`). The script fails with a clear message when it is missing.
- Linux: `dpkg-deb` and `dpkg-shlibdeps` (`dpkg-dev`, part of `build-essential`),
  `file`, `curl`, `sha256sum`. The AppImage tools are downloaded and
  checksum-verified (see below).

## Updating the AppImage pins

`linux/appimage.env` pins `appimagetool` and the type2 runtime (URL + SHA-256).
The runtime is always passed with `--runtime-file`; otherwise appimagetool would
download the moving "continuous" runtime. To update: take the new release URLs
from the AppImage/appimagetool and AppImage/type2-runtime GitHub releases,
download the files, compute `sha256sum`, and change version, URL and checksum
together. For local runs `APPIMAGETOOL=/path/to/appimagetool` skips the tool
download (the runtime is still downloaded and verified).

## Layouts

- `gitwarp-<ver>-linux-x86_64.tar.gz` — `gitwarp-<ver>-linux-x86_64/` with
  `gitwarp`, `git/`, `install.sh`, `dev.v1rus.gitwarp.desktop`, `icons/`,
  `LICENSE-MIT`, `LICENSE-APACHE`, `THIRD-PARTY-LICENSES.html`, `README.md`. `install.sh` installs into
  `~/.local` (or `$PREFIX`); `install.sh --uninstall` removes it.
- `gitwarp_<debver>_amd64.deb` — `/usr/bin/gitwarp`, `/usr/lib/gitwarp/git/`,
  `/usr/share/applications/dev.v1rus.gitwarp.desktop`, icons under
  `/usr/share/icons/hicolor/`, licences and `copyright` in `/usr/share/doc/gitwarp/`.
  `<debver>` is the Cargo version with `-` replaced by `~`, so `0.2.0~beta.1`
  sorts before `0.2.0`. `Depends` comes from `dpkg-shlibdeps` plus the
  libraries GPUI loads with dlopen (Vulkan, Wayland, xkbcommon, fontconfig,
  freetype), which shlibdeps cannot see. No maintainer scripts.
- `gitwarp-<ver>-x86_64.AppImage` — same `usr/` tree as the `.deb`, plus
  `AppRun`, the desktop file and icon at the AppDir root. System shared libraries
  are not bundled: the type2 runtime is static, and graphics and libc come from
  the host (glibc 2.35 or newer, the floor of the `ubuntu-22.04` runner).
- `gitwarp-<ver>-windows-x86_64.zip` — `gitwarp-<ver>-windows-x86_64/` with
  `gitwarp.exe`, `git/`, licences, `README.md` (portable).
- `gitwarp-<ver>-windows-x86_64-setup.exe` — Inno Setup installer
  (`windows/gitwarp.iss`) with the same files. Per-user by default, optional
  all-users install; `AppId` is fixed and must never change, since upgrades and
  uninstall depend on it.
- `gitwarp-<ver>-macos-aarch64.dmg` — a volume with `GitWarp.app` (`Contents/Info.plist`
  from `packaging/macos/Info.plist`, `Contents/MacOS/gitwarp`,
  `Contents/Resources/gitwarp.icns`, `Contents/Resources/git/`, licences) and an
  `Applications` symlink.
- `gitwarp-<ver>-macos-aarch64.zip` — `GitWarp.app/` (the same bundle as in the `.dmg`) made with
  `ditto -c -k --sequesterRsrc --keepParent`, which keeps the ad-hoc signature, symlinks and
  permissions. The in-app updater downloads and unpacks this file; the name is fixed in `updater.rs`.

`macos/install.sh` is the one-line installer for macOS users (`curl ... | sh`, see the
top-level README). It downloads the `.dmg` and `SHA256SUMS.txt` of the latest (or
`GITWARP_VERSION`) release with curl, verifies the checksum and copies `GitWarp.app` to
`/Applications` or `~/Applications`. Going through curl avoids the quarantine flag that
blocks the non-notarized app when the `.dmg` is downloaded in a browser. It depends on the
`-macos-aarch64.dmg` asset name and the `SHA256SUMS.txt` format produced by the release workflow.

`release-notes.md` holds the install instructions that the release workflow
puts in front of the generated GitHub release notes.

## Update signing

The release workflow signs `SHA256SUMS.txt` with minisign and uploads the signature as `SHA256SUMS.txt.minisig`
next to the artifacts. The in-app updater only installs a download whose checksum file verifies against
`packaging/update-key.pub`, the single place that holds the public key (the app embeds it at compile time and CI
verifies against it before publishing). Key setup and rotation are described in `RELEASING.md`.

## Signing

The macOS and Windows builds are not signed with a developer certificate. The
macOS bundle is ad-hoc signed (required to run on Apple Silicon), so users
still see a Gatekeeper warning; on Windows SmartScreen warns about an unknown
publisher.
