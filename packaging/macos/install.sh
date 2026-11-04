#!/bin/sh
# GitWarp installer for macOS (Apple Silicon).
#
#   curl -fsSL https://raw.githubusercontent.com/v1rus-dev/GitWarp/main/packaging/macos/install.sh | sh
#
# Downloads the latest release .dmg, verifies it against SHA256SUMS.txt, and copies
# GitWarp.app into /Applications (or ~/Applications when /Applications is not writable).
# Never uses sudo.
#
# Why curl: release builds are not notarized (no paid Apple developer account). A .dmg
# downloaded in a browser gets the quarantine flag and macOS 15+ refuses to launch it.
# Files fetched with curl get no quarantine flag, so the app starts without Gatekeeper prompts.
#
# Environment:
#   GITWARP_VERSION=X.Y.Z   install that release (tag vX.Y.Z) instead of the latest
#   GITWARP_NO_LAUNCH=1     do not open the app after installing
#
# All logic lives in functions and main runs on the last line, so a truncated
# download executes nothing.

set -eu

REPO="v1rus-dev/GitWarp"
BUNDLE_ID="dev.v1rus.gitwarp"
APP_NAME="GitWarp.app"

TMP=""
MNT=""

log() {
    printf '%s\n' "$*" >&2
}

die() {
    log "error: $*"
    exit 1
}

cleanup() {
    if [ -n "$MNT" ] && [ -d "$MNT" ]; then
        hdiutil detach -quiet "$MNT" >/dev/null 2>&1 ||
            hdiutil detach -quiet -force "$MNT" >/dev/null 2>&1 || true
    fi
    if [ -n "$TMP" ] && [ -d "$TMP" ]; then
        rm -rf "$TMP"
    fi
}

check_platform() {
    [ "$(uname -s)" = "Darwin" ] || die "this installer is for macOS only"
    if [ "$(uname -m)" != "arm64" ]; then
        # Under Rosetta uname -m reports x86_64 even on Apple Silicon.
        if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" != "1" ]; then
            die "Intel Macs are not supported: GitWarp is built for Apple Silicon only"
        fi
    fi
}

base_url() {
    if [ -n "${GITWARP_VERSION:-}" ]; then
        printf 'https://github.com/%s/releases/download/v%s' "$REPO" "$GITWARP_VERSION"
    else
        printf 'https://github.com/%s/releases/latest/download' "$REPO"
    fi
}

fetch() {
    # fetch <url> <output file>
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 --connect-timeout 30 -o "$2" "$1"
}

# Prints "<sha256> <file name>" for the single macOS dmg listed in a SHA256SUMS.txt.
pick_dmg_line() {
    awk '$2 ~ /-macos-aarch64\.dmg$/ { print $1, $2; n++ } END { exit n == 1 ? 0 : 1 }' "$1"
}

quit_running_app() {
    pgrep -x gitwarp >/dev/null 2>&1 || return 0
    log "GitWarp is running, asking it to quit..."
    osascript -e "quit app id \"$BUNDLE_ID\"" >/dev/null 2>&1 || true
    i=0
    while [ "$i" -lt 10 ]; do
        pgrep -x gitwarp >/dev/null 2>&1 || return 0
        sleep 1
        i=$((i + 1))
    done
    die "GitWarp is still running; quit it and run the installer again"
}

choose_target() {
    if [ -d /Applications ] && [ -w /Applications ]; then
        printf '/Applications'
    else
        mkdir -p "$HOME/Applications"
        printf '%s/Applications' "$HOME"
    fi
}

main() {
    check_platform

    TMP="$(mktemp -d "${TMPDIR:-/tmp}/gitwarp-install.XXXXXX")"
    trap cleanup EXIT
    trap 'exit 1' INT TERM

    url="$(base_url)"

    log "Downloading checksums..."
    fetch "$url/SHA256SUMS.txt" "$TMP/SHA256SUMS.txt"
    line="$(pick_dmg_line "$TMP/SHA256SUMS.txt")" || die "no single macOS .dmg listed in SHA256SUMS.txt"
    expected="${line%% *}"
    dmg_name="${line#* }"

    log "Downloading $dmg_name..."
    fetch "$url/$dmg_name" "$TMP/$dmg_name"

    actual="$(shasum -a 256 "$TMP/$dmg_name" | awk '{ print $1 }')"
    [ "$actual" = "$expected" ] || die "checksum mismatch for $dmg_name (expected $expected, got $actual)"
    log "Checksum OK."

    MNT="$TMP/mnt"
    mkdir -p "$MNT"
    hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$MNT" "$TMP/$dmg_name" >/dev/null ||
        die "could not mount $dmg_name"
    [ -d "$MNT/$APP_NAME" ] || die "$APP_NAME not found in the disk image"

    target="$(choose_target)"
    app="$target/$APP_NAME"

    quit_running_app

    log "Installing to $app..."
    new="$target/.GitWarp-install-$$.app"
    rm -rf "$new"
    ditto "$MNT/$APP_NAME" "$new" || { rm -rf "$new"; die "copy to $target failed"; }
    rm -rf "$app"
    mv "$new" "$app"

    # Defensive: curl downloads are not quarantined, but clear the flag in case it is set.
    xattr -dr com.apple.quarantine "$app" 2>/dev/null || true

    log "GitWarp installed: $app"

    if [ "${GITWARP_NO_LAUNCH:-}" != "1" ]; then
        open "$app"
    fi
}

main "$@"
