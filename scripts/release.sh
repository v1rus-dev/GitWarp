#!/usr/bin/env bash
# Cut a release through a pull request: bump the workspace version on a release branch,
# push it, open a PR into main and enable auto-merge.
#
#   scripts/release.sh [--dry-run] <X.Y.Z[-pre]>
#
# Run from an up-to-date, clean `develop`. Never tags and never touches `main` locally:
# after the PR is merged, .github/workflows/tag-release.yml creates the tag and starts the release.
set -euo pipefail

die() {
    echo "release.sh: $*" >&2
    exit 1
}

dry_run=false
if [ "${1:-}" = "--dry-run" ]; then
    dry_run=true
    shift
fi

[ $# -eq 1 ] || die "usage: release.sh [--dry-run] <X.Y.Z[-pre]>"
new="$1"
[[ "$new" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || die "invalid version '$new' (expected X.Y.Z or X.Y.Z-pre)"
tag="v$new"
branch="release/$tag"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

command -v gh > /dev/null 2>&1 || die "gh is not installed"
command -v cargo > /dev/null 2>&1 || die "cargo is not installed"
gh auth status > /dev/null 2>&1 || die "gh is not authenticated (run: gh auth login)"

[ "$(git rev-parse --abbrev-ref HEAD)" = "develop" ] || die "must be on branch develop"
[ -z "$(git status --porcelain)" ] || die "working tree is not clean"

git fetch -q origin
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/develop)" ] || die "local develop differs from origin/develop"
git merge-base --is-ancestor origin/main HEAD \
    || die "develop is missing commits from main; merge main into develop first"

if git rev-parse -q --verify "refs/tags/$tag" > /dev/null; then
    die "tag $tag already exists locally"
fi
if git ls-remote --exit-code --tags origin "refs/tags/$tag" > /dev/null 2>&1; then
    die "tag $tag already exists on origin"
fi
if git rev-parse -q --verify "refs/heads/$branch" > /dev/null; then
    die "branch $branch already exists locally"
fi
if git ls-remote --exit-code --heads origin "refs/heads/$branch" > /dev/null 2>&1; then
    die "branch $branch already exists on origin"
fi

# First `version = "..."` inside the [workspace.package] section.
current="$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p;}' Cargo.toml | head -n 1)"
[ -n "$current" ] || die "cannot read [workspace.package] version from Cargo.toml"
[ "$current" != "$new" ] || die "version is already $new"

if $dry_run; then
    echo "dry run: $current -> $new; would run:"
    echo "  git switch -c $branch"
    echo "  bump [workspace.package] version in Cargo.toml to $new; cargo update --workspace; git diff --stat"
    echo "  git commit -q -am \"Release $tag\""
    echo "  git push -u origin $branch"
    echo "  gh pr create --base main --head $branch --title \"Release $tag\""
    echo "  gh pr merge <pr-url> --merge --auto"
    echo "  git switch develop"
    exit 0
fi

git switch -c "$branch"

# Replace only the version line inside [workspace.package]; other `version =` lines stay untouched.
sed -i '/^\[workspace\.package\]/,/^\[/{s/^version[[:space:]]*=[[:space:]]*"[^"]*"/version = "'"$new"'"/;}' Cargo.toml
[ "$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p;}' Cargo.toml | head -n 1)" = "$new" ] \
    || die "failed to update version in Cargo.toml"

cargo update --workspace
git diff --stat

git commit -q -am "Release $tag"
git push -u origin "$branch"

pr_url="$(gh pr create --base main --head "$branch" --title "Release $tag" \
    --body "Version bump to $new. Merging this PR publishes $tag via tag-release.yml.")"

if ! gh pr merge "$pr_url" --merge --auto; then
    echo "release.sh: warning: could not enable auto-merge (disabled in repo settings?); merge $pr_url manually" >&2
fi

git switch develop

echo
echo "Release PR: $pr_url"
echo "After merge, tag-release.yml tags $tag and starts the release; it also opens a main -> develop PR."
