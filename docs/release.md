# Releasing

Releases are automated with [release-please](https://github.com/googleapis/release-please).
Never bump versions or edit `CHANGELOG.md` by hand.

1. **Conventional Commits.** PR titles are linted (`pr-title.yml`) and squash-merged, so
   each title is a commit on `main`: `feat:` → minor, `fix:`/`perf:` → patch, a `!` or
   `BREAKING CHANGE` → major. While the version is `0.x`, breaking changes bump the minor
   and features the minor too (`bump-minor-pre-major`); the first `feat` yields `v0.1.0`.
2. **Release PR.** `release-please.yml` runs on every push to `main` and opens or updates
   a "chore(main): release x.y.z" PR. It bumps `[workspace.package] version` in
   `Cargo.toml` (`release-please-config.json`, `.release-please-manifest.json`), updates
   `CHANGELOG.md`, and a follow-up step refreshes `Cargo.lock`.
3. **Merge** the release PR. release-please tags `vX.Y.Z` and creates the GitHub release
   with the changelog as notes, then starts `release.yml` for that tag.
4. **`release.yml`** builds the `raabit` binary (`cargo xtask dist --installer`) on every
   platform and attaches the files plus a `.sha256` for each to the release:

   | Platform | Files |
   |---|---|
   | Linux x86_64 / aarch64 | `raabit-<v>-linux-<arch>.tar.gz` |
   | macOS arm64 / x86_64 | `raabit-<v>-macos-<arch>.tar.gz`, `.dmg` |
   | Windows x86_64 | `raabit-<v>-windows-x86_64.zip`, `.msi` (WiX v5) |

## One-time setup

- **Allow Actions to create PRs.** release-please can only open the release PR once
  *Settings → Actions → General → Workflow permissions → "Allow GitHub Actions to create
  and approve pull requests"* is enabled for the organization and the repository
  (organization setting first). Until then `release-please.yml` fails with "not permitted
  to create or approve pull requests".
- **Optional `RELEASE_PLEASE_TOKEN`** (PAT or GitHub App token with contents and
  pull-requests write). PRs and tags made with `GITHUB_TOKEN` do not trigger other
  workflows, so without it CI does not run on the release PR (close and reopen it, or
  push to it, to get checks) and `release-please.yml` starts `release.yml` itself via
  `workflow_dispatch`.
- **Optional signing secrets**, skipped when unset: `MACOS_CERTIFICATE`,
  `MACOS_CERTIFICATE_PASSWORD`, `MACOS_SIGN_IDENTITY`, `APPLE_ID`, `APPLE_TEAM_ID`,
  `APPLE_APP_PASSWORD` (sign and notarize the DMG; otherwise it is ad-hoc signed) and
  `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD` (sign the MSI). Unsigned builds
  trigger SmartScreen/Gatekeeper warnings.
- Linux aarch64 uses the `ubuntu-22.04-arm` runner, available for public repositories.

## Packaging locally

`cargo xtask dist [--installer] [--target <triple>]` writes `dist/` for the host (or
target). It requires the `raabit` package (`crates/raabit`) and fails with a clear message
otherwise. `--installer` needs WiX v5 on Windows (`wix`, or `WIX_EXE`) and
`codesign`/`hdiutil` on macOS. Test packaging changes without publishing by running
`release.yml` via *Run workflow* on a branch with no tag.

Repairing a failed release: re-run the failed `release.yml` run (assets are replaced).
The MSI `UpgradeCode` in `xtask/packaging/windows/raabit.wxs` must never change.
