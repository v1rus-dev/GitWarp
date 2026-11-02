# Releasing GitWarp

## One-time setup

- Add the repository secret `SENTRY_DSN` (Settings -> Secrets and variables -> Actions). It is compiled into the
  release binaries as `GITWARP_SENTRY_DSN`; without it the builds ship with crash reporting disabled.
- The repository must be public for the release downloads to be visible to everyone.

## Cutting a release

1. Merge everything for the release into `main`.
2. On an up-to-date, clean `main` run `scripts/release.sh 0.2.0`. It bumps `[workspace.package] version`, refreshes
   `Cargo.lock`, commits `Release v0.2.0` and creates the annotated tag `v0.2.0`. It never pushes.
3. Push the commit and the tag with the command the script prints: `git push origin main v0.2.0`.
4. Watch Actions -> Release. When all jobs are green, the GitHub Release `GitWarp 0.2.0` appears with three archives
   (Linux, Windows, macOS) and `SHA256SUMS.txt`.

The workflow fails if the tag does not match the version in `Cargo.toml`.

## Pre-releases

A version with a suffix, e.g. `scripts/release.sh 0.2.0-rc.1`, is published marked as a pre-release.

## Dry run

Actions -> Release -> Run workflow (on any branch). It builds and packages all three targets and uploads them as
workflow artifacts; no GitHub Release is created. The same dry run starts on its own for pull requests that touch
`packaging/`, `scripts/release.sh` or the workflow. (GitHub only offers "Run workflow" once the workflow is on `main`.)

## When a release job fails

Nothing is published until every build job succeeds. Delete the tag locally and on origin, fix the problem on `main`,
and cut the release again:

```sh
git tag -d v0.2.0
git push origin :refs/tags/v0.2.0
```

If the release commit itself needs to change, also revert or amend it on `main` before re-running `scripts/release.sh`.
If the failure was only in the workflow, a dry run is the cheapest way to verify the fix.

## Unsigned builds

The Windows and macOS builds are not signed with a developer certificate. Users will see a SmartScreen (Windows) or
Gatekeeper (macOS) warning on first launch; see `packaging/README.md`.
