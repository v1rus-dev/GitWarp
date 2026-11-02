#!/usr/bin/env bash
# Produce the git bundle GitWarp ships next to the app: <out-dir>/git.
#
#   packaging/git/fetch-or-build.sh <linux|windows|macos> <out-dir>
#
# linux, macos: build git from the pinned tarball with RUNTIME_PREFIX (the
#               tree can be moved anywhere; git finds its exec-path and
#               templates relative to its own binary).
# windows:      download and unpack the pinned MinGit (git/cmd/git.exe).
#
# Needs: bash, curl, sha256sum (shasum on macOS), tar + xz (linux/macos) or
# unzip (windows); linux/macos also make, a C compiler and the dev headers for
# libcurl, expat, zlib and OpenSSL. Nothing is installed by this script.
set -euo pipefail

usage() { echo "usage: $0 <linux|windows|macos> <out-dir>" >&2; exit 2; }
[ $# -eq 2 ] || usage
target=$1
out=$2
case "$target" in linux | windows | macos) ;; *) usage ;; esac

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=versions.env
. "$here/versions.env"

mkdir -p "$out"
out=$(cd "$out" && pwd)
dest=$out/git
work=$out/.git-work
rm -rf "$dest" "$work"
mkdir -p "$work/dl" "$work/src"
trap 'rm -rf "$work"' EXIT

sha256_of() {
  if command -v sha256sum > /dev/null; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# fetch <url> <sha256> <file-in-dl-dir>; refuses anything whose sum differs.
fetch() {
  curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
    --output "$work/dl/$3" "$1"
  local got
  got=$(sha256_of "$work/dl/$3")
  if [ "$got" != "$2" ]; then
    echo "checksum mismatch for $3: expected $2, got $got" >&2
    exit 1
  fi
}

write_source_offer() {
  cat > "$dest/SOURCE.txt" << EOT
This directory contains an unmodified build of git $1, licensed under the
GNU General Public License v2 (see COPYING). GitWarp runs it as a separate
program.

Corresponding source: $2
Built by packaging/git/fetch-or-build.sh in the GitWarp repository.
EOT
}

build_unix() {
  fetch "$GIT_TARBALL_URL" "$GIT_TARBALL_SHA256" git.tar.xz
  tar -xJf "$work/dl/git.tar.xz" -C "$work/src"
  local src=$work/src/git-$GIT_VERSION
  local jobs
  jobs=$(nproc 2> /dev/null || sysctl -n hw.ncpu 2> /dev/null || echo 2)

  # prefix=/ + RUNTIME_PREFIX: paths are resolved relative to the binary.
  # NO_PERL/NO_PYTHON drop git-svn, send-email, etc.; no Tcl/gettext keeps the
  # bundle small and free of system data files. NO_INSTALL_HARDLINKS makes the
  # built-in aliases relative symlinks that survive moving the tree.
  # EXTRA_MAKE_ARGS lets CI add platform flags (e.g. macOS OpenSSL paths).
  # shellcheck disable=SC2086
  make -C "$src" -j"$jobs" prefix=/ NO_TCLTK=1 NO_GETTEXT=1 NO_PERL=1 \
    NO_PYTHON=1 RUNTIME_PREFIX=YesPlease NO_INSTALL_HARDLINKS=1 \
    ${EXTRA_MAKE_ARGS:-} install DESTDIR="$work/stage"

  mv "$work/stage" "$dest"
  # Not needed by the app: completion scripts, gitweb, server/other front ends.
  rm -rf "$dest/share/bash-completion" "$dest/share/gitweb" "$dest/share/perl5" \
    "$dest/bin/git-shell" "$dest/bin/git-cvsserver" "$dest/bin/scalar"
  rm -f "$dest"/libexec/git-core/git-cvs* "$dest"/libexec/git-core/git-svn* \
    "$dest"/libexec/git-core/git-archimport "$dest"/libexec/git-core/git-shell \
    "$dest"/libexec/git-core/scalar
  # Strip every regular ELF/Mach-O file (symlinks are left alone).
  local f
  while IFS= read -r -d '' f; do
    case "$(file -b "$f")" in
      ELF* | Mach-O*) if [ "$target" = macos ]; then strip -x "$f"; else strip "$f"; fi ;;
    esac
  done < <(find "$dest" -type f -print0)

  cp "$src/COPYING" "$src/LGPL-2.1" "$dest/"
  write_source_offer "$GIT_VERSION" "$GIT_TARBALL_URL"
}

build_windows() {
  fetch "$MINGIT_URL" "$MINGIT_SHA256" mingit.zip
  mkdir -p "$dest"
  unzip -q "$work/dl/mingit.zip" -d "$dest"
  [ -f "$dest/cmd/git.exe" ] || { echo "MinGit layout unexpected: no cmd/git.exe" >&2; exit 1; }
  # MinGit ships its licences under etc/ and usr/ (LICENSE.txt at the root).
  write_source_offer "${MINGIT_TAG#v}" \
    "https://github.com/git-for-windows/git/releases/tag/$MINGIT_TAG (sources: https://github.com/git-for-windows/git/tree/$MINGIT_TAG)"
}

case "$target" in
  linux | macos) build_unix ;;
  windows) build_windows ;;
esac

echo "bundled git ready: $dest"
