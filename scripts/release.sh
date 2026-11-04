#!/usr/bin/env bash
# Cut a release: bump the workspace version, commit and tag locally.
#
#   scripts/release.sh <X.Y.Z[-pre]>
#
# Never pushes; prints the push command instead.
set -euo pipefail

die() {
    echo "release.sh: $*" >&2
    exit 1
}

[ $# -eq 1 ] || die "usage: release.sh <X.Y.Z[-pre]>"
new="$1"
[[ "$new" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || die "invalid version '$new' (expected X.Y.Z or X.Y.Z-pre)"
tag="v$new"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

[ "$(git rev-parse --abbrev-ref HEAD)" = "main" ] || die "must be on branch main"
[ -z "$(git status --porcelain)" ] || die "working tree is not clean"

git fetch -q origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "local main differs from origin/main"

if git rev-parse -q --verify "refs/tags/$tag" > /dev/null; then
    die "tag $tag already exists locally"
fi
if git ls-remote --exit-code --tags origin "refs/tags/$tag" > /dev/null 2>&1; then
    die "tag $tag already exists on origin"
fi

# First `version = "..."` inside the [workspace.package] section.
current="$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p;}' Cargo.toml | head -n 1)"
[ -n "$current" ] || die "cannot read [workspace.package] version from Cargo.toml"
[ "$current" != "$new" ] || die "version is already $new"

# Replace only the version line inside [workspace.package]; other `version =` lines stay untouched.
sed -i '/^\[workspace\.package\]/,/^\[/{s/^version[[:space:]]*=[[:space:]]*"[^"]*"/version = "'"$new"'"/;}' Cargo.toml
[ "$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p;}' Cargo.toml | head -n 1)" = "$new" ] \
    || die "failed to update version in Cargo.toml"

cargo update --workspace
git diff --stat

git commit -q -am "Release $tag"
git tag -a "$tag" -m "GitWarp $tag"

echo
echo "Released $current -> $new locally. Review, then push:"
echo "  git push origin main $tag"
