# Bundled git

GitWarp prefers a git shipped next to the app (see "Which git" in
[CONTRIBUTING.md](../../CONTRIBUTING.md) for the lookup paths). `fetch-or-build.sh` produces that `git/`
directory:

```sh
packaging/git/fetch-or-build.sh <linux|windows|macos> <out-dir>   # writes <out-dir>/git
```

Copy `<out-dir>/git` to the lookup location of the target OS
(`<app dir>/git`, `<app dir>/../lib/gitwarp/git`, or
`<App>.app/Contents/Resources/git`). The script installs nothing; it needs
`curl`, `tar`+`xz` (or `unzip` for Windows), and for Linux/macOS `make`, a C
compiler and the development headers of libcurl, expat, zlib and OpenSSL.

## What is bundled

- **Linux, macOS**: git built from the pinned upstream tarball with
  `RUNTIME_PREFIX=YesPlease prefix=/`, so the tree can be moved anywhere. No
  Perl/Python/Tcl/gettext (git-svn, send-email, gitk, git-gui and translations
  are absent), docs/man pages are not installed, binaries are stripped, the
  default templates are kept. About 27 MB. `git-remote-https` links the
  build host's shared libcurl/OpenSSL (system libraries, not bundled), so the
  target machine needs `libcurl`, `libexpat`, `libz` (see the glibc note below).
- **Windows**: the pinned MinGit 64-bit zip (non-busybox), unpacked as-is;
  `git/cmd/git.exe` is the entry point. About 93 MB unpacked.
- Every bundle contains `COPYING` (GPLv2, plus `LGPL-2.1` on Linux/macOS /
  MinGit's `LICENSE.txt`) and `SOURCE.txt` with the source URL.

## Licence

git is GPLv2. GitWarp starts it as a separate program and does not link it.
Distributing the bundle means also offering the corresponding source: the
pinned tarball URL (or the Git for Windows tag) written into `SOURCE.txt`
satisfies that as long as the pin stays reachable; keep the tarball when
releasing if in doubt.

## glibc floor (Linux)

A binary built on a host needs at least that host's glibc symbol versions
(2.38 for the 2.56.0 build made on a glibc 2.44 host). Build the release
bundle on the oldest distro the app supports (e.g. in a container), not on a
rolling-release dev machine.

## Updating the pin

1. Pick the new git version. For Linux/macOS take the tarball URL from
   <https://cdn.kernel.org/pub/software/scm/git/> and its SHA-256
   from `sha256sums.asc` in the same directory.
2. For Windows take the latest `MinGit-<ver>-64-bit.zip` (not busybox) from
   <https://github.com/git-for-windows/git/releases> and the SHA-256 listed in
   the release notes.
3. Edit `versions.env` and run the script for every target; check
   `git --version` of the result and `gitwarp --git bundled` (status bar shows
   `git X (bundled)`).

## macOS (CI recipe, not run locally)

On `macos-latest`: `packaging/git/fetch-or-build.sh macos out`, then copy
`out/git` to `GitWarp.app/Contents/Resources/git`. Git's RUNTIME_PREFIX is
supported on macOS. If the Homebrew OpenSSL/curl headers are not found, pass
them via `EXTRA_MAKE_ARGS` (e.g. `CURLDIR=$(brew --prefix curl)
OPENSSLDIR=$(brew --prefix openssl)`). Code-sign the nested binaries with the
app. Unverified: this has no runner here.
