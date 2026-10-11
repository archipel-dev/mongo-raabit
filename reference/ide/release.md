# Releases, installers and updates

Jetthing ships two kinds of downloads for every release:

- **Archives** — portable directories the self-updater reads and replaces in place.
- **Native installers** — `.msi`, `.dmg` and `.deb`, for a normal install with menu
  entries and clean uninstall.

| Platform | Installer | Archive |
|---|---|---|
| Windows x86_64 | `jetthing-<version>-windows-x86_64.msi` | `jetthing-<version>-windows-x86_64.zip` |
| macOS Apple silicon | `jetthing-<version>-macos-aarch64.dmg` | `jetthing-<version>-macos-aarch64.tar.gz` |
| macOS Intel | `jetthing-<version>-macos-x86_64.dmg` | `jetthing-<version>-macos-x86_64.tar.gz` |
| Linux x86_64 | `jetthing_<version>_amd64.deb` | `jetthing-<version>-linux-x86_64.tar.gz` |
| Linux x86_64 (any distribution) | `jetthing-<version>-linux-x86_64.AppImage` | (single file) |

Every file has a `<file>.sha256` next to it in `sha256sum` format. Check a download with
`sha256sum -c <file>.sha256` (Linux), `shasum -a 256 -c <file>.sha256` (macOS) or
`Get-FileHash <file>` (PowerShell). The names are stable: the self-updater and these
docs depend on them.

## Install and uninstall

### Windows (MSI)

Double-click `jetthing-<version>-windows-x86_64.msi` and approve the administrator
prompt. It installs:

- `C:\Program Files\Jetthing\` with `jetthing.exe` and `jetthing-cli.exe`;
- a **Jetthing** Start Menu entry;
- the install folder on the system `PATH` (new terminals), so `jetthing .` and
  `jetthing-cli` work from a shell.

A newer MSI replaces an older one in place, because all versions share one upgrade
code. Installing an older MSI over a newer one is refused.

To install without prompts, for example in scripts, run
`msiexec /i jetthing-<version>-windows-x86_64.msi /qn` from an elevated shell.

To uninstall, use **Settings → Apps → Installed apps → Jetthing → Uninstall**, or run
`msiexec /x jetthing-<version>-windows-x86_64.msi /qn`. This removes the files, the
Start Menu entry and the `PATH` entry. Project data in `.jetthing/` folders inside your
projects is left alone.

**SmartScreen.** Unsigned MSIs show "Windows protected your PC". Click
**More info → Run anyway**. Signed builds (see [Signing](#optional-signing)) avoid
this once the certificate has a reputation.

### macOS (DMG)

Open `jetthing-<version>-macos-<arch>.dmg`. Use `aarch64` on Apple silicon and
`x86_64` on Intel. Then drag **Jetthing** onto **Applications**. The bundle
(`io.github.henryschwerdt.jetthing`) contains `jetthing` and `jetthing-cli` in
`Jetthing.app/Contents/MacOS/`. To use them from a terminal, run:

```bash
sudo ln -s /Applications/Jetthing.app/Contents/MacOS/jetthing /usr/local/bin/jetthing
```

To uninstall, move `/Applications/Jetthing.app` to the Bin.

**Gatekeeper.** Release builds are signed ad hoc but not notarized unless the signing
secrets below are configured. The first time you open one, macOS reports that it
"cannot verify" the app. Use either:

- **System Settings → Privacy & Security → Open Anyway** (macOS 15 and later), or
  right-click the app → **Open** on older versions.
- `xattr -dr com.apple.quarantine /Applications/Jetthing.app`

Apps launched from Finder do not inherit your shell's `PATH`. If Cargo, Gradle or Git
live in a shell-only location, start Jetthing from a terminal with
`open -a Jetthing` or the symlink above.

### Linux (.deb)

```bash
sudo apt install ./jetthing_<version>_amd64.deb   # or: sudo dpkg -i …
```

The package installs `/usr/lib/jetthing/`, adds `/usr/bin/jetthing` and
`/usr/bin/jetthing-cli` links, `jetthing.desktop` and hicolor icons. The `Depends:`
glibc version comes from the binary's symbol versions. Release builds run on Ubuntu
22.04, so they need glibc 2.35 or newer.

To uninstall, run `sudo apt remove jetthing` (or `sudo dpkg -r jetthing`).

### Linux (AppImage)

The AppImage is one self-contained file and needs no installation or root:

```bash
chmod +x jetthing-<version>-linux-x86_64.AppImage
./jetthing-<version>-linux-x86_64.AppImage .
```

It needs FUSE 2 (`libfuse2` on Debian/Ubuntu). Without FUSE, run it with
`APPIMAGE_EXTRACT_AND_RUN=1`, or unpack it once with `--appimage-extract`. To add a
menu entry, use an integration tool such as AppImageLauncher or Gear Lever. To
uninstall, delete the file.

### Package managers

Each release workflow run also renders package-manager manifests from the built
files' checksums (workflow artifact `package-manifests-<version>`, or
`cargo xtask manifests --dir <artifacts>` locally):

| Manager | File | Installs |
|---|---|---|
| Homebrew | `homebrew/jetthing.rb` (cask) | the DMG for the Mac's architecture; links `jetthing` and `jetthing-cli` |
| winget | `winget/manifests/h/HenrySchwerdt/Jetthing/<version>/` (schema 1.6.0) | the MSI, machine scope |
| Scoop | `scoop/jetthing.json` | the portable zip, with shims and a Start Menu shortcut; `checkver`/`autoupdate` included |

Nothing is published automatically. To ship them:

- **Homebrew:** open a pull request against a tap, for example
  `HenrySchwerdt/homebrew-tap`, so users can run
  `brew install --cask henryschwerdt/tap/jetthing`. Unsigned builds do not qualify
  for `homebrew/cask`.
- **winget:** run `wingetcreate submit <dir>`, or open a pull request against
  `microsoft/winget-pkgs` with the version directory.
- **Scoop:** commit `jetthing.json` to a bucket repository, so users can run
  `scoop bucket add jetthing <url>` and then `scoop install jetthing`.

Copies installed by Homebrew, winget or Scoop should be updated with that package
manager (`brew upgrade`, `winget upgrade`, `scoop update`). The IDE's own updater
treats the Homebrew copy like the DMG and the winget copy like the MSI. It treats a
Scoop copy like an archive and replaces it in place; `scoop update` then reinstalls
the newer version cleanly.

### Archives (all platforms)

Extract the archive anywhere you can write and run `jetthing` (`jetthing.exe` on
Windows). On Linux, the archive also contains `jetthing.desktop` and `jetthing.png`. To
add a menu entry, copy `jetthing.desktop` to `~/.local/share/applications/` and
`jetthing.png` to `~/.local/share/icons/hicolor/512x512/apps/`, then set `Exec=` to
the absolute path of `jetthing`. To uninstall, delete the directory.

## Updates

**Help → Check for Updates…** (also run silently at startup unless
`JETTHING_NO_UPDATE_CHECK` is set) reads the latest GitHub release. **Update and
Restart** then works according to how Jetthing was installed.
`jetthing_update::InstallKind::detect` decides this from the running executable:

| Installed from | Detected by | What the update does |
|---|---|---|
| Archive | default | Downloads the archive, verifies its `.sha256` and replaces `jetthing` and `jetthing-cli` in place (the old binary is kept as `jetthing.old`). **Restart** relaunches. |
| MSI | `jetthing.install` containing `msi` next to the exe | Downloads and verifies the new `.msi`. **Quit and Install** saves the session, starts `msiexec /i <msi>` and quits. The MSI upgrades the installed version. |
| DMG | executable inside `*.app/Contents/MacOS/` | Downloads and verifies the new `.dmg`. **Quit and Install** opens it and quits; drag Jetthing into Applications and replace the old copy. |
| .deb | `jetthing.install` containing `deb` | Downloads and verifies the new `.deb`. **Quit and Install** opens it with `xdg-open` (your software installer) and quits. |
| AppImage | `$APPIMAGE` (set by the AppImage runtime) | Downloads and verifies the new `.AppImage` and replaces the file in place (the old one is kept as `<name>.old`). **Restart** relaunches. The file name keeps the old version number. |
| `cargo run` | executable not named `jetthing` | Links to the release page only. |

Installer downloads go to `<temp>/jetthing-update/`. A release without a `.sha256` for
the needed file is refused. The installed-copy paths (Program Files, `/Applications`,
`/usr/lib`) are never written by the IDE itself.

| Variable | Effect |
|---|---|
| `JETTHING_NO_UPDATE_CHECK` | disable the startup check |
| `JETTHING_GITHUB_TOKEN` / `GITHUB_TOKEN` | token for a private release repository |
| `JETTHING_UPDATE_REPOSITORY` | `owner/repo` to update from (default `HenrySchwerdt/jetthing`) |

## Building locally

```bash
cargo run -p xtask -- dist                                  # archive for the host
cargo run -p xtask -- dist --installer                      # + native installer
cargo run -p xtask -- dist --installer --target x86_64-apple-darwin
```

The output goes to `dist/`. Each artifact path is printed on its own line.

| Installer | Needs |
|---|---|
| `.msi` | [WiX Toolset v5](https://wixtoolset.org) (`dotnet tool install --global wix --version 5.0.2`); the `WIX_EXE` variable overrides the `wix` path. Definition: `packaging/windows/jetthing.wxs`. |
| `.dmg` | Xcode command line tools (`codesign`, `hdiutil`). Signing uses `JETTHING_MACOS_SIGN_IDENTITY` when it is set, and ad hoc (`-`) otherwise. |
| `.deb` | `dpkg-deb` (and `objdump` for the glibc requirement). |
| `.AppImage` | [appimagetool](https://github.com/AppImage/appimagetool) 1.9.0 (`APPIMAGETOOL` overrides the path) and `desktop-file-validate` (`desktop-file-utils`). Built together with the `.deb`. |

The app icon is original artwork. `assets/icon/jetthing.svg` is the source, and
`python3 assets/icon/render.py` (cairosvg and Pillow) regenerates the committed `.png`,
`.ico` and `.icns` files.

## Release workflow

`.github/workflows/release.yml` builds the archives and installers on
`ubuntu-22.04`, `windows-latest` and `macos-latest`. The macOS runner builds both
`aarch64` and `x86_64`, cross-compiling the latter. Each installer is then
smoke-tested:

- The `.deb` is installed and removed again.
- The AppImage runs `--version` without FUSE, and its extracted contents (`AppRun`,
  desktop entry, icon, binaries) are checked; the desktop entry is validated.
- The MSI is installed, reinstalled to check the upgrade code, and uninstalled. The
  workflow checks the Start Menu entry, the install marker and that no files are left
  behind.
- The DMG is mounted, and the workflow checks the Info.plist, the code signature and
  the CLI.

A `manifests` job then renders the Homebrew, winget and Scoop manifests from all
checksums and uploads them as a workflow artifact. Nothing is published to package
managers.

To test packaging changes on a branch, start the workflow manually from the Actions
tab (**Run workflow** with that branch). On branches other than `main` it builds and
smoke-tests only.

Publishing GitHub release `v<version>` (`version` from `[workspace.package]` in
`Cargo.toml`):

- **Push to `main`**: the workflow publishes when the release is missing, is a draft,
  or lacks any of the expected files. If tag `v<version>` already exists on a
  different commit, it does not publish and logs a warning instead. Re-run the tag's
  workflow to repair the release, or bump the version.
- **Push of tag `v<version>`**: the workflow always publishes or repairs (re-uploads)
  the release. The tag must equal the workspace version, or the run fails.
- **Manual runs on other branches**: the workflow never publishes. Pull requests do
  not run it.

To ship a version, merge the release PR described below (or bump
`[workspace.package] version` by hand and merge that to `main`). When `CHANGELOG.md`
has a section for the version, the release body is that section; otherwise GitHub's
generated notes are used.

## Automatic versioning

`.github/workflows/release-please.yml` runs [release-please](https://github.com/googleapis/release-please-action)
on every push to `main`. It reads the [Conventional Commits](https://www.conventionalcommits.org)
since the last release and opens (and keeps updating) one **release PR** that:

- bumps `[workspace.package] version` in `Cargo.toml`;
- refreshes `Cargo.lock` (a follow-up commit running `cargo update --workspace`);
- adds the section to `CHANGELOG.md`.

Merging the release PR changes the version on `main`, and `release.yml` publishes it as
above. release-please runs with `skip-github-release`, so it creates no tag or release
itself. Config: `release-please-config.json` and `.release-please-manifest.json`. The
manifest holds the last released version and is updated by the release PR. The version is
read from the workspace table with a TOML extra-file updater (release-please's `rust`
strategy handles `[package]` versions, not `[workspace.package]`).

| Commit | Bump while `0.0.x` / `0.x` |
|---|---|
| `fix:`, `perf:`, `feat:` | patch (`0.0.7` → `0.0.8`) |
| `feat!:` / `BREAKING CHANGE:` footer | minor (`0.0.7` → `0.1.0`) |
| `docs:`, `test:`, `chore:`, `ci:`, `refactor:` | no release |

This is `bump-minor-pre-major` plus `bump-patch-for-minor-pre-major`. After 1.0.0 the
usual rules apply: `feat` is minor, breaking changes are major.

**Moving to 0.1.0 (or 1.0.0) deliberately.** Push an empty commit to `main`, or put the
footer in a squash-merge commit, with a `Release-As` footer:

```
chore: release 0.1.0

Release-As: 0.1.0
```

The release PR then proposes exactly that version.

**CI on the release PR.** Pushes made with the default `GITHUB_TOKEN` do not trigger other
workflows, so `ci.yml` does not run on the release PR unless the repository secret
`RELEASE_PLEASE_TOKEN` (a PAT or GitHub App token with contents and pull-requests write)
is set. Without it, close and reopen the PR to run CI. `main` is checked again by
`release.yml` after the merge.

**Pending label.** With `skip-github-release`, release-please never relabels merged release
PRs, and it will not open another release PR while one is `autorelease: pending`. The
workflow relabels them `autorelease: tagged` on each run.

PR titles are linted against Conventional Commits by `pr-title.yml` (non-blocking for
now). See [contributing](contributing.md).

## Optional signing

All signing is optional. The workflow skips each signing step when its secrets are
unset, so forks and branch runs build unsigned. Store certificates as base64:
`base64 -i cert.p12 | pbcopy`.

| Secret | Used for |
|---|---|
| `MACOS_CERTIFICATE`, `MACOS_CERTIFICATE_PASSWORD` | Developer ID Application certificate (`.p12`), imported into a temporary keychain |
| `MACOS_SIGN_IDENTITY` | e.g. `Developer ID Application: Name (TEAMID)`; enables the hardened runtime and a timestamp |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | `notarytool` notarization and stapling of the DMG (the checksum is regenerated after stapling) |
| `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD` | Authenticode `.pfx` used by `signtool` to sign the MSI (the checksum is regenerated afterwards) |

Never commit certificates or passwords. The repository contains no signing material.
