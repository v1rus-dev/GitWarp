# Releasing GitWarp

## One-time setup

- Add the repository secret `SENTRY_DSN` (Settings -> Secrets and variables -> Actions). It is compiled into the
  release binaries as `GITWARP_SENTRY_DSN`; without it the builds ship with crash reporting disabled.
- Set up update signing (see "Update signing" below): secret `MINISIGN_SECRET_KEY` and the real
  `packaging/update-key.pub`. Without them the `publish` job fails on purpose.
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
Cut a pre-release first whenever the publish path itself changed (and for the very first release): the publish job
only runs for tags, so pull-request and manual dry runs never exercise it.

The workflow refuses tags whose commit is not on `main`.

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

## Update signing

The in-app updater installs a download only if `SHA256SUMS.txt` has a valid
[minisign](https://jedisct1.github.io/minisign/) signature (`SHA256SUMS.txt.minisig`) made with the key
compiled into the app. That key lives in one file, `packaging/update-key.pub`; the app embeds it at build
time and the release workflow verifies the fresh signature against it before publishing.

**Current state: `packaging/update-key.pub` holds the real release key (id `798F1D95D6984FDD`).** The `publish`
job needs the repository secret `MINISIGN_SECRET_KEY` (and `MINISIGN_PASSWORD` if the key has a password). It fails
at "Sign checksums" while the secret is unset and at "Verify signature" if the secret does not belong to this key,
so nothing unsigned or unverifiable is ever released.

One-time setup (do this on your own machine; the secret key never goes into the repo or chat):

1. Install minisign and generate the pair:
   `minisign -G -p packaging/update-key.pub -s ~/.minisign/gitwarp.key`
   You may protect the key with a password. minisign writes the `.pub` with its own comment line; keep the file
   at exactly two lines (comment line + base64 key line).
2. Add the repository secret `MINISIGN_SECRET_KEY` with the full contents of `~/.minisign/gitwarp.key`
   (both lines). If you set a password, also add `MINISIGN_PASSWORD`; leave it unset for an unencrypted key.
3. Commit the new `packaging/update-key.pub` to `main` and release as usual. Apps built from that commit
   trust the new key.
4. Back up the secret key (and its password) offline, e.g. in a password manager. Without it you cannot sign.

Key rotation: every installed app embeds exactly one key. To rotate, first ship a release that contains the NEW
public key but is still signed with the OLD secret key (so installed apps accept it), wait until users have
updated, and only then switch the CI secrets to the new key. Switching earlier strands installed apps.

If the secret key is lost or leaked, installed apps cannot be moved to a new key by an update: they stop
accepting updates signed with anything else, so users must reinstall once from a fresh download. After a leak,
revoke by publishing a new key this way as soon as possible and treat old releases as untrusted.

## Unsigned builds

The Windows and macOS builds are not signed with a developer certificate. Users will see a SmartScreen (Windows) or
Gatekeeper (macOS) warning on first launch; see `packaging/README.md`.
