#!/usr/bin/env bash
# Install GitWarp for the current user (no sudo): binary, desktop entry and
# hicolor icons. Expects a release build in target/release/gitwarp.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"

prefix="${PREFIX:-$HOME/.local}"
binary="$repo_root/target/release/gitwarp"
icons="$repo_root/crates/app/assets/app-icon"

if [ ! -f "$binary" ]; then
    echo "build first: cargo build --release -p app" >&2
    exit 1
fi

install -d "$prefix/bin"
install -m 755 "$binary" "$prefix/bin/gitwarp"

for png in "$icons"/gitwarp-*.png; do
    size="$(basename "$png" .png)"
    size="${size#gitwarp-}"
    dir="$prefix/share/icons/hicolor/${size}x${size}/apps"
    install -d "$dir"
    install -m 644 "$png" "$dir/dev.v1rus.gitwarp.png"
done

install -d "$prefix/share/icons/hicolor/scalable/apps"
install -m 644 "$icons/gitwarp.svg" "$prefix/share/icons/hicolor/scalable/apps/dev.v1rus.gitwarp.svg"

install -d "$prefix/share/applications"
install -m 644 "$script_dir/dev.v1rus.gitwarp.desktop" "$prefix/share/applications/dev.v1rus.gitwarp.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$prefix/share/applications"
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$prefix/share/icons/hicolor"
fi
