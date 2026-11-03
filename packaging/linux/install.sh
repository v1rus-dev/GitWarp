#!/usr/bin/env bash
# Install GitWarp for the current user (no sudo): binary, desktop entry and
# hicolor icons; with a bundled git when one is available.
#
# Works from two layouts:
#   - repo checkout: binary from target/release/gitwarp (cargo build --release -p app)
#   - release archive: gitwarp, icons/, the desktop file and git/ next to this script
#
#   install.sh [--uninstall]      PREFIX overrides the default ~/.local
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
app_id="dev.v1rus.gitwarp"

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$prefix/bin/gitwarp"
    rm -rf "$prefix/lib/gitwarp"
    rm -f "$prefix"/share/icons/hicolor/*/apps/"$app_id".png
    rm -f "$prefix/share/icons/hicolor/scalable/apps/$app_id.svg"
    rm -f "$prefix/share/applications/$app_id.desktop"
    echo "GitWarp removed from $prefix"
    exit 0
fi

if [ -f "$script_dir/gitwarp" ]; then
    # Release archive layout.
    binary="$script_dir/gitwarp"
    icons="$script_dir/icons"
    git_bundle="$script_dir/git"
else
    # Repo checkout layout.
    repo_root="$(cd "$script_dir/../.." && pwd)"
    binary="$repo_root/target/release/gitwarp"
    icons="$repo_root/crates/app/assets/app-icon"
    git_bundle="$repo_root/target/release/git"
fi

if [ ! -f "$binary" ]; then
    echo "build first: cargo build --release -p app" >&2
    exit 1
fi

install -d "$prefix/bin"
install -m 755 "$binary" "$prefix/bin/gitwarp"

if [ -d "$git_bundle" ]; then
    # The app looks for <bin dir>/../lib/gitwarp/git/bin/git.
    rm -rf "$prefix/lib/gitwarp/git"
    install -d "$prefix/lib/gitwarp"
    cp -a "$git_bundle" "$prefix/lib/gitwarp/git"
fi

for png in "$icons"/gitwarp-*.png; do
    size="$(basename "$png" .png)"
    size="${size#gitwarp-}"
    dir="$prefix/share/icons/hicolor/${size}x${size}/apps"
    install -d "$dir"
    install -m 644 "$png" "$dir/$app_id.png"
done

install -d "$prefix/share/icons/hicolor/scalable/apps"
install -m 644 "$icons/gitwarp.svg" "$prefix/share/icons/hicolor/scalable/apps/$app_id.svg"

install -d "$prefix/share/applications"
install -m 644 "$script_dir/$app_id.desktop" "$prefix/share/applications/$app_id.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$prefix/share/applications"
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$prefix/share/icons/hicolor"
fi
