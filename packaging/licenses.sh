#!/usr/bin/env bash
# Render the licences of every crate compiled into the GitWarp binary.
#
#   packaging/licenses.sh <linux|windows|macos> <out-dir>
#
# Writes <out-dir>/THIRD-PARTY-LICENSES.html with cargo-about (about.toml and
# about.hbs at the repository root); package.sh picks it up from there.
# Install the tool with: cargo install --locked cargo-about --version 0.9.3 --features cli
set -euo pipefail

die() {
    echo "licenses.sh: $*" >&2
    exit 1
}

[ $# -eq 2 ] || die "usage: licenses.sh <linux|windows|macos> <out-dir>"
os="$1"
out_dir="$2"

# The dependency graph differs per platform (keychain, windowing, TLS crates).
case "$os" in
    linux) triple=x86_64-unknown-linux-gnu ;;
    windows) triple=x86_64-pc-windows-msvc ;;
    macos) triple=aarch64-apple-darwin ;;
    *) die "unknown target '$os' (expected linux, windows or macos)" ;;
esac

command -v cargo-about >/dev/null 2>&1 \
    || die "cargo-about not found (cargo install --locked cargo-about --version 0.9.3 --features cli)"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
mkdir -p "$out_dir"
out="$out_dir/THIRD-PARTY-LICENSES.html"

# --fail: a crate whose licence cannot be identified, or is not accepted in about.toml, fails the run.
cargo about generate --locked --fail \
    --config "$repo_root/about.toml" \
    --manifest-path "$repo_root/crates/app/Cargo.toml" \
    --target "$triple" \
    --output-file "$out" \
    "$repo_root/about.hbs"
echo "$out"
