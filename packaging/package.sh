#!/usr/bin/env bash
# Build the release artifacts for GitWarp (archives and native installers).
#
#   packaging/package.sh <linux|windows|macos> <bundle-dir> <out-dir>
#
# <bundle-dir> is the directory that CONTAINS git/ (the <out-dir> of
# packaging/git/fetch-or-build.sh) and THIRD-PARTY-LICENSES.html (the <out-dir>
# of packaging/licenses.sh). The binary is taken from target/release/
# (or $CARGO_TARGET_DIR/release/). Prints the path of every produced artifact,
# one per line:
#   linux:   .tar.gz, .deb, .AppImage
#   windows: .zip (portable), -setup.exe (Inno Setup)
#   macos:   .dmg
set -euo pipefail

die() {
    echo "package.sh: $*" >&2
    exit 1
}

[ $# -eq 3 ] || die "usage: package.sh <linux|windows|macos> <bundle-dir> <out-dir>"
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

copy_licences() {
    cp "$repo_root/LICENSE-MIT" "$repo_root/LICENSE-APACHE" "$bundle_dir/THIRD-PARTY-LICENSES.html" "$1/"
}

# Download a pinned file and verify its SHA-256; dies on mismatch.
fetch_pinned() {
    local url="$1" sha="$2" dest="$3"
    curl -fsSL --retry 3 -o "$dest" "$url" || die "download failed: $url"
    echo "$sha  $dest" | sha256sum -c --quiet - || die "checksum mismatch for $url"
}

# Lay out the files shared by the .deb and the AppImage under <root>/usr:
# binary, bundled git, desktop file, icons and licences. The app finds the
# bundled git at <exe dir>/../lib/gitwarp/git, i.e. usr/lib/gitwarp/git.
populate_usr_tree() {
    local root="$1" f size
    install -d -m 755 "$root/usr/bin" "$root/usr/lib/gitwarp" "$root/usr/share/applications" \
        "$root/usr/share/doc/gitwarp" "$root/usr/share/icons/hicolor/scalable/apps"
    install -m 755 "$binary" "$root/usr/bin/gitwarp"
    cp -a "$bundle_dir/git" "$root/usr/lib/gitwarp/git"
    install -m 644 "$script_dir/linux/dev.v1rus.gitwarp.desktop" "$root/usr/share/applications/"
    for f in "$icons"/gitwarp-*.png; do
        size="${f##*gitwarp-}"
        size="${size%.png}"
        install -d -m 755 "$root/usr/share/icons/hicolor/${size}x${size}/apps"
        install -m 644 "$f" "$root/usr/share/icons/hicolor/${size}x${size}/apps/dev.v1rus.gitwarp.png"
    done
    install -m 644 "$icons/gitwarp.svg" "$root/usr/share/icons/hicolor/scalable/apps/dev.v1rus.gitwarp.svg"
    copy_licences "$root/usr/share/doc/gitwarp"
    chmod -R go-w "$root"
}

# Depends for the .deb: whatever dpkg-shlibdeps finds for the binary and the
# bundled git, plus libraries GPUI loads with dlopen. Those are invisible to
# shlibdeps (not in DT_NEEDED), but the app does not start without them.
deb_depends() {
    local root="$1" tmp f deps
    local -a elfs=()
    tmp="$(mktemp -d)"
    while IFS= read -r -d '' f; do
        case "$(file -b "$f")" in
            ELF*) elfs+=("$f") ;;
        esac
    done < <(find "$root/usr/bin" "$root/usr/lib/gitwarp/git" -type f -print0)
    mkdir "$tmp/debian"
    : >"$tmp/debian/control"
    # dpkg-shlibdeps wants a debian/control in the cwd; -O prints the substvar
    # ("shlibs:Depends=a (>= 1), b") instead of writing debian/substvars.
    deps="$(cd "$tmp" && dpkg-shlibdeps -O --ignore-missing-info "${elfs[@]}")" \
        || { rm -rf "$tmp"; die "dpkg-shlibdeps failed"; }
    rm -rf "$tmp"
    deps="${deps#shlibs:Depends=}"
    printf '%s\n' "$deps, libvulkan1, libwayland-client0, libxkbcommon0, libxkbcommon-x11-0, libfontconfig1, libfreetype6" \
        | tr ',' '\n' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' | awk 'NF && !seen[$1]++' | paste -sd, - | sed 's/,/, /g'
}

build_deb() {
    local root="$1" debver="$2" out="$3" installed_size depends
    populate_usr_tree "$root"
    cat >"$root/usr/share/doc/gitwarp/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: GitWarp
Source: https://github.com/v1rus-dev/GitWarp

Files: *
Copyright: GitWarp contributors
License: MIT or Apache-2.0
 Licensed under either of the MIT license or the Apache License, Version 2.0,
 at your option. The full texts are in LICENSE-MIT and LICENSE-APACHE in this
 directory. The licences of the bundled Rust crates are listed in
 THIRD-PARTY-LICENSES.html.

Files: usr/lib/gitwarp/git/*
Copyright: Linus Torvalds and the Git contributors
License: GPL-2
 The bundled git is licensed under the GNU General Public License, version 2,
 and runs as a separate program. See /usr/share/common-licenses/GPL-2.
EOF
    chmod 644 "$root/usr/share/doc/gitwarp/copyright"
    depends="$(deb_depends "$root")"
    installed_size="$(du -sk "$root" | cut -f1)"
    install -d -m 755 "$root/DEBIAN"
    cat >"$root/DEBIAN/control" <<EOF
Package: gitwarp
Version: $debver
Architecture: amd64
Maintainer: v1rus-dev <v1rus-dev@users.noreply.github.com>
Section: vcs
Priority: optional
Homepage: https://github.com/v1rus-dev/GitWarp
Installed-Size: $installed_size
Depends: $depends
Description: Fast, native Git client
 GitWarp is a native Git client built with Rust and GPUI, with a bundled git.
EOF
    chmod 644 "$root/DEBIAN/control"
    rm -f "$out"
    dpkg-deb --root-owner-group --build "$root" "$out" >/dev/null
}

build_appimage() {
    local appdir="$1" out="$2" tooldir="$3" tool runtime
    # shellcheck source=linux/appimage.env
    . "$script_dir/linux/appimage.env"
    if [ -n "${APPIMAGETOOL:-}" ]; then
        # Local override: a pre-downloaded appimagetool, not checksum-verified.
        tool="$APPIMAGETOOL"
        [ -x "$tool" ] || die "APPIMAGETOOL is not executable: $tool"
    else
        tool="$tooldir/appimagetool"
        fetch_pinned "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA256" "$tool"
        chmod +x "$tool"
    fi
    # Pinned runtime: without --runtime-file appimagetool downloads the moving
    # "continuous" runtime, which would make the build irreproducible.
    runtime="$tooldir/runtime-x86_64"
    fetch_pinned "$APPIMAGE_RUNTIME_URL" "$APPIMAGE_RUNTIME_SHA256" "$runtime"

    populate_usr_tree "$appdir"
    install -m 644 "$script_dir/linux/dev.v1rus.gitwarp.desktop" "$appdir/"
    install -m 644 "$icons/gitwarp-256.png" "$appdir/dev.v1rus.gitwarp.png"
    install -m 644 "$icons/gitwarp-256.png" "$appdir/.DirIcon"
    install -m 755 "$script_dir/linux/AppRun" "$appdir/AppRun"
    rm -f "$out"
    # APPIMAGE_EXTRACT_AND_RUN: runners have no FUSE. System libraries are
    # deliberately not bundled (see packaging/README.md).
    ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --runtime-file "$runtime" "$appdir" "$out" >/dev/null
}

case "$os" in
    linux | macos) binary="$release_dir/gitwarp" ;;
    windows) binary="$release_dir/gitwarp.exe" ;;
    *) die "unknown target '$os' (expected linux, windows or macos)" ;;
esac
[ -f "$binary" ] || die "binary not found: $binary (run: cargo build --release -p app)"
[ -d "$bundle_dir/git" ] || die "git bundle not found: $bundle_dir/git (run packaging/git/fetch-or-build.sh)"
[ -f "$bundle_dir/THIRD-PARTY-LICENSES.html" ] \
    || die "third-party licences not found: $bundle_dir/THIRD-PARTY-LICENSES.html (run packaging/licenses.sh)"

mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

artifacts=()

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
        artifacts+=("$archive")

        # `~` instead of `-` so a pre-release (0.2.0~beta.1) sorts before 0.2.0 in dpkg.
        debver="${version//-/\~}"
        deb="$out_dir/gitwarp_${debver}_amd64.deb"
        build_deb "$stage/deb-root" "$debver" "$deb"
        artifacts+=("$deb")

        mkdir "$stage/appimage-tools"
        appimage="$out_dir/gitwarp-$version-x86_64.AppImage"
        build_appimage "$stage/GitWarp.AppDir" "$appimage" "$stage/appimage-tools"
        artifacts+=("$appimage")
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
        artifacts+=("$archive")

        # Inno Setup compiler: PATH first, then the default install location.
        if command -v iscc >/dev/null 2>&1; then
            iscc="$(command -v iscc)"
        elif command -v ISCC.exe >/dev/null 2>&1; then
            iscc="$(command -v ISCC.exe)"
        elif [ -x "/c/Program Files (x86)/Inno Setup 6/ISCC.exe" ]; then
            iscc="/c/Program Files (x86)/Inno Setup 6/ISCC.exe"
        else
            die "Inno Setup compiler not found (iscc, ISCC.exe or C:\\Program Files (x86)\\Inno Setup 6); install Inno Setup 6"
        fi
        winpath() {
            if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf '%s\n' "$1"; fi
        }
        # MSYS_NO_PATHCONV stops Git Bash from rewriting the "/D..." and "/Q"
        # switches into Windows paths.
        MSYS_NO_PATHCONV=1 "$iscc" /Q \
            "/DAppVersion=$version" \
            "/DNumericVersion=${version%%-*}" \
            "/DSourceDir=$(winpath "$top")" \
            "/DOutputDir=$(winpath "$out_dir")" \
            "/DOutputBaseFilename=$name-setup" \
            "/DRepoRoot=$(winpath "$repo_root")" \
            "$(winpath "$script_dir/windows/gitwarp.iss")" >&2
        setup="$out_dir/$name-setup.exe"
        [ -f "$setup" ] || die "Inno Setup did not produce $setup"
        artifacts+=("$setup")
        ;;
    macos)
        name="gitwarp-$version-macos-aarch64"
        app="$stage/GitWarp.app"
        mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
        # CFBundleVersion must be numeric (dot-separated), so a pre-release suffix is dropped there.
        # LC_ALL=C: BSD sed rejects the UTF-8 "©" in the template under some locales.
        LC_ALL=C sed -e "s/@VERSION@/$version/g" -e "s/@BUILD_VERSION@/${version%%-*}/g" \
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

        # Drag-to-install image: the app next to an Applications symlink. cp -R
        # keeps symlinks, so the ad-hoc signature stays valid.
        dmg_stage="$stage/dmg"
        mkdir -p "$dmg_stage"
        cp -R "$app" "$dmg_stage/GitWarp.app"
        ln -s /Applications "$dmg_stage/Applications"
        archive="$out_dir/$name.dmg"
        rm -f "$archive"
        hdiutil create -volname "GitWarp" -srcfolder "$dmg_stage" -format UDZO -ov "$archive" >/dev/null
        artifacts+=("$archive")

        # The in-app updater downloads this zip. ditto (not zip) keeps the ad-hoc signature,
        # symlinks, permissions and extended attributes; the top-level entry is GitWarp.app/.
        zip_archive="$out_dir/$name.zip"
        rm -f "$zip_archive"
        ditto -c -k --sequesterRsrc --keepParent "$app" "$zip_archive"
        [ -f "$zip_archive" ] || die "ditto did not produce $zip_archive"
        artifacts+=("$zip_archive")
        ;;
esac

printf '%s\n' "${artifacts[@]}"
