#!/usr/bin/env bash
# Build one release archive for GitWarp.
#
#   packaging/package.sh <linux|windows|macos> <git-bundle-dir> <out-dir>
#
# <git-bundle-dir> is the directory that CONTAINS git/ (the <out-dir> of
# packaging/git/fetch-or-build.sh). The binary is taken from target/release/
# (or $CARGO_TARGET_DIR/release/). Prints the path of the produced archive.
set -euo pipefail

die() {
    echo "package.sh: $*" >&2
    exit 1
}

[ $# -eq 3 ] || die "usage: package.sh <linux|windows|macos> <git-bundle-dir> <out-dir>"
os="$1"
bundle_dir="$2"
out_dir="$3"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
icons="$repo_root/crates/app/assets/app-icon"
release_dir="${CARGO_TARGET_DIR:-$repo_root/target}/release"

# First `version = "..."` inside the [workspace.package] section.
version="$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p;}' "$repo_root/Cargo.toml" | head -n 1)"
[ -n "$version" ] || die "cannot read [workspace.package] version from Cargo.toml"

case "$os" in
    linux | macos) binary="$release_dir/gitwarp" ;;
    windows) binary="$release_dir/gitwarp.exe" ;;
    *) die "unknown target '$os' (expected linux, windows or macos)" ;;
esac
[ -f "$binary" ] || die "binary not found: $binary (run: cargo build --release -p app)"
[ -d "$bundle_dir/git" ] || die "git bundle not found: $bundle_dir/git (run packaging/git/fetch-or-build.sh)"

mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

copy_licences() {
    cp "$repo_root/LICENSE-MIT" "$repo_root/LICENSE-APACHE" "$1/"
}

case "$os" in
    linux)
        name="gitwarp-$version-linux-x86_64"
        top="$stage/$name"
        mkdir -p "$top/icons"
        install -m 755 "$binary" "$top/gitwarp"
        cp -a "$bundle_dir/git" "$top/git"
        install -m 755 "$script_dir/linux/install.sh" "$top/install.sh"
        install -m 644 "$script_dir/linux/dev.v1rus.gitwarp.desktop" "$top/"
        cp "$icons"/gitwarp-*.png "$icons/gitwarp.svg" "$top/icons/"
        copy_licences "$top"
        cp "$repo_root/README.md" "$top/README.md"
        archive="$out_dir/$name.tar.gz"
        rm -f "$archive"
        tar -C "$stage" -czf "$archive" "$name"
        ;;
    windows)
        name="gitwarp-$version-windows-x86_64"
        top="$stage/$name"
        mkdir -p "$top"
        cp "$binary" "$top/gitwarp.exe"
        cp -a "$bundle_dir/git" "$top/git"
        copy_licences "$top"
        cp "$repo_root/README.md" "$top/README.md"
        archive="$out_dir/$name.zip"
        rm -f "$archive"
        if command -v 7z >/dev/null 2>&1; then
            (cd "$stage" && 7z a -tzip "$archive" "$name" >/dev/null)
        else
            (cd "$stage" && zip -qr "$archive" "$name")
        fi
        ;;
    macos)
        name="gitwarp-$version-macos-aarch64"
        app="$stage/GitWarp.app"
        mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
        # CFBundleVersion must be numeric (dot-separated), so a pre-release suffix is dropped there.
        sed -e "s/@VERSION@/$version/g" -e "s/@BUILD_VERSION@/${version%%-*}/g" \
            "$script_dir/macos/Info.plist" >"$app/Contents/Info.plist"
        install -m 755 "$binary" "$app/Contents/MacOS/gitwarp"
        cp -a "$bundle_dir/git" "$app/Contents/Resources/git"
        copy_licences "$app/Contents/Resources"

        iconset="$stage/gitwarp.iconset"
        mkdir -p "$iconset"
        cp "$icons/gitwarp-16.png" "$iconset/icon_16x16.png"
        cp "$icons/gitwarp-32.png" "$iconset/icon_16x16@2x.png"
        cp "$icons/gitwarp-32.png" "$iconset/icon_32x32.png"
        cp "$icons/gitwarp-64.png" "$iconset/icon_32x32@2x.png"
        cp "$icons/gitwarp-128.png" "$iconset/icon_128x128.png"
        cp "$icons/gitwarp-256.png" "$iconset/icon_128x128@2x.png"
        cp "$icons/gitwarp-256.png" "$iconset/icon_256x256.png"
        cp "$icons/gitwarp-512.png" "$iconset/icon_256x256@2x.png"
        cp "$icons/gitwarp-512.png" "$iconset/icon_512x512.png"
        cp "$icons/gitwarp-1024.png" "$iconset/icon_512x512@2x.png"
        iconutil -c icns "$iconset" -o "$app/Contents/Resources/gitwarp.icns"

        # Apple Silicon refuses to run unsigned arm64 code; ad-hoc signing needs no certificate.
        # --deep does not treat Mach-O files under Resources/ as code, so the bundled git
        # binaries are signed one by one before the bundle itself.
        while IFS= read -r -d '' f; do
            case "$(file -b "$f")" in
                Mach-O*) codesign --force --sign - "$f" ;;
            esac
        done < <(find "$app/Contents/Resources/git" -type f -print0)
        codesign --force --deep --sign - "$app"
        archive="$out_dir/$name.zip"
        rm -f "$archive"
        # ditto keeps symlinks and the signature intact (plain zip does not).
        ditto -c -k --keepParent "$app" "$archive"
        ;;
esac

echo "$archive"
