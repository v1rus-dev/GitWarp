# Releasing GitWarp

## One-time setup

- Add the repository secret `SENTRY_DSN` (Settings -> Secrets and variables -> Actions). It is compiled into the
  release binaries as `GITWARP_SENTRY_DSN`; without it the builds ship with crash reporting disabled.
- Set up update signing (see "Update signing" below): secret `MINISIGN_SECRET_KEY` and the real
  `packaging/update-key.pub`. Without them the `publish` job fails on purpose.
- The repository must be public for the release downloads to be visible to everyone.

## Cutting a release

`main` is protected (pull requests only, required checks), so a release goes through a PR.

1. Merge everything for the release into `develop`.
2. On an up-to-date, clean `develop` run `scripts/release.sh 0.2.0` (add `--dry-run` to only run the checks and
   print the steps). It creates `release/v0.2.0`, bumps `[workspace.package] version`, refreshes `Cargo.lock`,
   commits `Release v0.2.0`, pushes the branch, opens a PR into `main` and enables auto-merge, then returns to
   `develop`. It never tags and never touches `main` locally. `develop` must already contain `main`.
3. The PR merges automatically once the required checks are green (if auto-merge is disabled in the repo settings,
   the script warns and you merge the PR manually).
4. The merge triggers `.github/workflows/tag-release.yml`: it creates the annotated tag `v0.2.0` on the merge
   commit, starts the Release workflow on that tag, and opens a PR `main -> develop` with the bump. Merge that PR
   afterwards so `develop` gets the release commit (CI does not run on that PR; it runs on push after the merge).
5. Watch Actions -> Release. When all jobs are green, the GitHub Release `GitWarp 0.2.0` appears with three archives
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

Nothing is published until every build job succeeds. The tag already exists at that point, and `tag-release.yml`
skips versions whose tag exists, so do not reuse the version without deleting the tag. Delete the tag locally and
on origin, and fix the problem on `develop`:

```sh
git tag -d v0.2.0
git push origin :refs/tags/v0.2.0
```

Then cut a new patch release (`scripts/release.sh 0.2.1`). `main` already carries the `v0.2.0` bump, so the next
release is a normal version bump on top of it. To reuse `0.2.0` exactly, the version bump on `main` would have to be
changed through a PR first, and `scripts/release.sh` refuses to run while branch `release/v0.2.0` (locally or on
origin) or tag `v0.2.0` exists, so delete them first:

```sh
git branch -D release/v0.2.0
git push origin --delete release/v0.2.0
```

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
