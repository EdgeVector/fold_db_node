# Releasing fold_db_node

Releasing is one command:

```bash
git tag -a v0.3.14 -m "v0.3.14" main
git push origin v0.3.14
```

That's it. No PR, no version-bump dance, no merge queue. The tag push fires
[`release.yml`](../.github/workflows/release.yml) and
[`tauri-release.yml`](../.github/workflows/tauri-release.yml), which together
produce **six artifacts**:

1. GitHub release `vX.Y.Z` on `EdgeVector/fold_db_node` with three binary
   tarballs (`folddb-aarch64-apple-darwin.tar.gz`,
   `folddb-x86_64-apple-darwin.tar.gz`,
   `folddb-x86_64-unknown-linux-gnu.tar.gz`) plus `SHA256SUMS.txt`.
2. Mirror release on the public `EdgeVector/fold_db` repo with the same
   tarballs (this is what `install.sh` and Homebrew users download from).
3. Auto-PR `bump: folddb → vX.Y.Z` on `EdgeVector/homebrew-folddb`,
   updating the formula's version + sha256 hashes.
4. Signed + notarized macOS DMG `FoldDB_X.Y.Z_aarch64.dmg` attached to
   the GitHub release.
5. The `dmg-smoke` job verifies the DMG actually launches and reports the
   right version before the release is finalized.
6. Tag itself, immutably pinned to the squash commit on `main`.

Total wall time: ~30 min (binary builds run in parallel; macOS signing +
notarization is the long pole at ~25 min).

## Where the version comes from

The tag is the only version source. There's no `Cargo.toml` `version =
"X.Y.Z"` to keep in sync, no `tauri.conf.json` `version`, no commit-
message regex.

- `Cargo.toml`'s `version` field is purely cosmetic. [`build.rs`](../build.rs)
  reads `GITHUB_REF_NAME` at compile time and stamps `FOLDDB_BUILD_VERSION`
  into every binary, overriding `CARGO_PKG_VERSION`. The verify-versus-tag
  step in `release.yml` enforces this.
- `tauri.conf.json`'s `version` field is overridden at build time via
  `npx tauri build --config '{"version":"X.Y.Z"}'`. The DMG filename and
  Info.plist's `CFBundleShortVersionString` flow from the override, not
  from the source file.

So `Cargo.toml` and `tauri.conf.json` may show old version numbers — that's
fine, they're not load-bearing. **Don't bump them as part of releasing.**

## Pre-release: dry-run

Before tagging, you can test the entire release pipeline against any
commit without burning a version:

1. Go to **Actions → Release → Run workflow**, optionally enter a `ref`
   (default: `main`), click Run.
2. Same for **Actions → Tauri Release → Run workflow**.
3. The `release` / `bump-tap` / DMG-attach jobs are gated to skip on
   non-tag triggers, so the dry-run produces artifacts as workflow
   downloads but doesn't publish anything.

This dry-run path also runs **automatically every Sunday at 06:00 UTC**
against `main`. If something breaks the pipeline (a removed binary, a
changed Tauri bundle layout, expired Apple cert), the Sunday smoke
catches it before the next release attempt does.

## When something fails

Look at the failing job:

```bash
gh run list --repo EdgeVector/fold_db_node --workflow=release.yml --limit 5
gh run view <run-id> --log-failed
```

Common failure modes and where they surface:

| Failure | Job | Symptom |
|---|---|---|
| Removed binary still listed in `release.yml` | `Release / build` | `error: no bin target named X` |
| Tauri bundle binary name changed | `Tauri Release / dmg-smoke` | `Contents/MacOS/Y: No such file or directory` |
| `secrets.GH_PAT` expired | Any `Configure git for private dependencies` | `failed to authenticate when downloading repository` |
| Apple notarization rate-limited | `Tauri Release / Notarize app` | Apple-API HTTP 429 / queue timeout |
| `dmg-smoke` health probe times out | `Tauri Release / dmg-smoke` | "App process exited before /api/health came up" |

The Sunday smoke + the daily
[`gh-pat-health.yml`](../.github/workflows/gh-pat-health.yml) probe should
catch most of these before a real release attempt.

If a tag has already been pushed and the release fails:

- **Don't try to recover the orphan tag.** Bump the patch version and tag
  again. Orphan tags are cheap; chasing a broken release is expensive.
  See `v0.3.11` and `v0.3.12` in this repo's tag list — both are orphans
  from failed release attempts that v0.3.13 superseded cleanly.

## Rolling back a release

If a release ships and you need to retract it:

1. Mark the GitHub release as a "draft" or delete it via
   `gh release delete vX.Y.Z --repo EdgeVector/fold_db_node` (also do
   `--repo EdgeVector/fold_db` for the mirror).
2. Close the auto-bump PR on `homebrew-folddb` if it hasn't merged yet.
   If it has merged, open a revert PR there.
3. Delete the tag: `git push origin :refs/tags/vX.Y.Z`. Note that anyone
   who already pulled the tag locally still has it.
4. Tag a fix forward (vX.Y.Z+1) rather than republishing vX.Y.Z under a
   different commit.

## Auth setup

All workflow auth uses `secrets.GH_PAT` (org-level Actions secret). The
older `PRIVATE_DEPS_TOKEN` was retired 2026-04-30 after a silent expiration
broke every PR's CI; do not reintroduce it. See
`feedback_never_use_private_deps_token` in agent memory.

The daily probe at `gh-pat-health.yml` pings Discord on failure. If GH_PAT
expires, you'll see the alert before CI breaks at the wrong time.
