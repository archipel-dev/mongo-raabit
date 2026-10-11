//! `cargo xtask dist`: release builds, archives and native installers.
//!
//! Every run stages `dist/jetthing-<version>-<os>-<arch>/` and packs it into the
//! archive the self-updater consumes (`.tar.gz`, `.zip` on Windows). With
//! `--installer` it also builds the native installer for the target OS:
//!
//! | OS | Installer | Tooling |
//! |---|---|---|
//! | Windows | `jetthing-<version>-windows-<arch>.msi` | WiX Toolset v5 (`wix`) |
//! | macOS | `jetthing-<version>-macos-<arch>.dmg` with `Jetthing.app` | `codesign`, `hdiutil` |
//! | Linux | `jetthing_<version>_<debarch>.deb` | `dpkg-deb` |
//! | Linux | `jetthing-<version>-linux-<arch>.AppImage` | `appimagetool` (`APPIMAGETOOL`) |
//!
//! Each artifact gets a `<artifact>.sha256` in `sha256sum` format. Paths of all
//! artifacts are printed one per line (forward slashes) for CI scripts.
//! See `docs/release.md`.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUNDLE_ID: &str = "io.github.henryschwerdt.jetthing";
const HOMEPAGE: &str = "https://github.com/HenrySchwerdt/jetthing";
/// Marker read by `jetthing_update::InstallKind::detect`.
const INSTALL_MARKER: &str = "jetthing.install";
const ICONS: &str = "assets/icon";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Rust target triple; the host when absent.
    pub target: Option<String>,
    pub installer: bool,
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options> {
        let mut options = Options::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--installer" => options.installer = true,
                "--target" => {
                    options.target = Some(args.next().context("--target needs a triple")?)
                }
                other => bail!("unknown dist option `{other}`"),
            }
        }
        Ok(options)
    }
}

/// `std::env::consts`-style OS and architecture of a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}

impl Platform {
    pub fn host() -> Platform {
        Platform {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
        }
    }

    /// `x86_64-pc-windows-msvc` → windows / x86_64.
    pub fn from_triple(triple: &str) -> Result<Platform> {
        let arch = triple.split('-').next().unwrap_or_default();
        let os = if triple.contains("-windows") {
            "windows"
        } else if triple.contains("-apple-darwin") {
            "macos"
        } else if triple.contains("-linux") {
            "linux"
        } else {
            bail!("unsupported target `{triple}`")
        };
        if arch.is_empty() {
            bail!("unsupported target `{triple}`");
        }
        Ok(Platform {
            os: os.into(),
            arch: arch.into(),
        })
    }

    fn exe(&self) -> &'static str {
        if self.os == "windows" { ".exe" } else { "" }
    }
}

/// `1.2.3-beta.1` → `1.2.3`: MSI and `CFBundleVersion` take numbers only.
pub fn numeric_version(version: &str) -> &str {
    version.split(['-', '+']).next().unwrap_or(version)
}

/// Debian sorts `~` before the release, like SemVer pre-releases.
pub fn deb_version(version: &str) -> String {
    version.replacen('-', "~", 1)
}

pub fn deb_arch(arch: &str) -> &str {
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

pub fn dist(options: Options) -> Result<()> {
    let platform = match &options.target {
        Some(triple) => Platform::from_triple(triple)?,
        None => Platform::host(),
    };
    let release_dir = build(options.target.as_deref())?;
    let name = format!("jetthing-{VERSION}-{}-{}", platform.os, platform.arch);
    let dist = Path::new("dist");
    let stage = dist.join(&name);
    stage_files(&release_dir, &stage, &platform)?;
    let mut artifacts = vec![if platform.os == "windows" {
        zip_dir(&stage, &dist.join(format!("{name}.zip")))?
    } else {
        tar_gz_dir(&stage, &dist.join(format!("{name}.tar.gz")))?
    }];
    if options.installer {
        artifacts.push(match platform.os.as_str() {
            "windows" => msi(&stage, &platform, dist)?,
            "macos" => dmg(&stage, &platform, dist)?,
            "linux" => deb(&stage, &platform, dist)?,
            other => bail!("no installer for {other}"),
        });
        if platform.os == "linux" {
            artifacts.push(appimage(&stage, &platform, dist)?);
        }
    }
    for artifact in &artifacts {
        write_checksum(artifact)?;
    }
    for artifact in &artifacts {
        println!("{}", artifact.to_string_lossy().replace('\\', "/"));
    }
    Ok(())
}

fn build(target: Option<&str>) -> Result<PathBuf> {
    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    command.args([
        "build",
        "--release",
        "-p",
        "jetthing-app",
        "-p",
        "jetthing-cli",
    ]);
    if let Some(target) = target {
        command.args(["--target", target]);
    }
    run(&mut command, "cargo build")?;
    Ok(match target {
        Some(target) => Path::new("target").join(target).join("release"),
        None => Path::new("target/release").to_owned(),
    })
}

/// Lays out the portable distribution (also the payload of every installer).
fn stage_files(release_dir: &Path, out: &Path, platform: &Platform) -> Result<()> {
    if out.exists() {
        fs::remove_dir_all(out)?;
    }
    fs::create_dir_all(out.join("licenses"))?;
    let exe = platform.exe();
    for (built, shipped) in [
        ("jetthing-app", "jetthing"),
        ("jetthing-cli", "jetthing-cli"),
    ] {
        copy(
            &release_dir.join(format!("{built}{exe}")),
            &out.join(format!("{shipped}{exe}")),
        )?;
    }
    copy(Path::new("README.md"), &out.join("README.md"))?;
    copy(
        Path::new("docs/spec/data/NOTICE.md"),
        &out.join("NOTICE-intellij-data.md"),
    )?;
    for license in ["Inter-OFL.txt", "JetBrainsMono-OFL.txt"] {
        copy(
            &Path::new("crates/jetthing-ui/assets/fonts").join(license),
            &out.join("licenses").join(license),
        )?;
    }
    copy(
        &Path::new(ICONS).join("jetthing.png"),
        &out.join("jetthing.png"),
    )?;
    if platform.os == "linux" {
        fs::write(out.join("jetthing.desktop"), desktop_entry("jetthing"))?;
    }
    Ok(())
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    fs::copy(from, to).with_context(|| format!("copy {}", from.display()))?;
    Ok(())
}

fn run(command: &mut Command, what: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("run {what} (is it installed?)"))?;
    if !status.success() {
        bail!("{what} failed ({status})");
    }
    Ok(())
}

/// Freedesktop launcher; `exec` is `jetthing` (on PATH) or an absolute path.
pub fn desktop_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Jetthing\n\
         GenericName=Integrated Development Environment\n\
         Comment=Native Rust IDE\n\
         Exec={exec}\n\
         Icon=jetthing\n\
         Terminal=false\n\
         Categories=Development;IDE;TextEditor;\n\
         Keywords=IDE;Rust;Java;Editor;Code;\n\
         StartupWMClass=jetthing\n"
    )
}

/// `sha256sum`-compatible `<artifact>.sha256`, read by the self-updater.
fn write_checksum(artifact: &Path) -> Result<()> {
    let bytes = fs::read(artifact).with_context(|| format!("read {}", artifact.display()))?;
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let name = artifact
        .file_name()
        .context("artifact has no file name")?
        .to_string_lossy();
    let mut path = artifact.as_os_str().to_owned();
    path.push(".sha256");
    fs::write(&path, format!("{digest}  {name}\n"))?;
    Ok(())
}

fn tar_gz_dir(dir: &Path, archive: &Path) -> Result<PathBuf> {
    let file = fs::File::create(archive)?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::best());
    let mut builder = tar::Builder::new(encoder);
    builder.mode(tar::HeaderMode::Deterministic);
    builder.follow_symlinks(false);
    builder.append_dir_all(dir.file_name().context("archive root")?, dir)?;
    builder.into_inner()?.finish()?.flush()?;
    Ok(archive.to_owned())
}

fn zip_dir(dir: &Path, archive: &Path) -> Result<PathBuf> {
    let root = dir.file_name().context("archive root")?.to_string_lossy();
    let mut zip = zip::ZipWriter::new(fs::File::create(archive)?);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for file in files_under(dir)? {
        let relative = file.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
        zip.start_file(format!("{root}/{relative}"), options)?;
        zip.write_all(&fs::read(&file)?)?;
    }
    zip.finish()?;
    Ok(archive.to_owned())
}

/// Regular files below `dir`, sorted for reproducible archives.
fn files_under(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

// ---------------------------------------------------------------- Windows

/// Builds the MSI from `packaging/windows/jetthing.wxs` with WiX v5.
fn msi(stage: &Path, platform: &Platform, dist: &Path) -> Result<PathBuf> {
    fs::write(stage.join(INSTALL_MARKER), "msi\n")?;
    let arch = match platform.arch.as_str() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => bail!("no MSI architecture for {other}"),
    };
    let out = dist.join(format!("jetthing-{VERSION}-windows-{}.msi", platform.arch));
    let wix = std::env::var("WIX_EXE").unwrap_or_else(|_| "wix".into());
    run(
        Command::new(wix)
            .args(["build", "-nologo", "-arch", arch])
            .arg("-d")
            .arg(format!("Version={}", numeric_version(VERSION)))
            .arg("-d")
            .arg(format!("Stage={}", absolute(stage)?.display()))
            .arg("-d")
            .arg(format!(
                "Icon={}",
                absolute(&Path::new(ICONS).join("jetthing.ico"))?.display()
            ))
            .arg("-o")
            .arg(&out)
            .arg("packaging/windows/jetthing.wxs"),
        "wix build",
    )?;
    // The marker belongs to the MSI only; the archive stays updatable in place.
    fs::remove_file(stage.join(INSTALL_MARKER))?;
    Ok(out)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(std::env::current_dir()?.join(path))
}

// ---------------------------------------------------------------- macOS

pub fn info_plist(version: &str) -> String {
    let short = numeric_version(version);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>Jetthing</string>
  <key>CFBundleExecutable</key><string>jetthing</string>
  <key>CFBundleIconFile</key><string>jetthing</string>
  <key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>Jetthing</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{short}</string>
  <key>CFBundleVersion</key><string>{short}</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>Jetthing {version}. Apache-2.0 OR MIT.</string>
</dict>
</plist>
"#
    )
}

/// `Jetthing.app` inside a compressed `.dmg` with an Applications link.
/// Signs ad hoc unless `JETTHING_MACOS_SIGN_IDENTITY` names a certificate.
fn dmg(stage: &Path, platform: &Platform, dist: &Path) -> Result<PathBuf> {
    let image_root = dist.join(format!("dmg-{}", platform.arch));
    if image_root.exists() {
        fs::remove_dir_all(&image_root)?;
    }
    let contents = image_root.join("Jetthing.app/Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos)?;
    fs::create_dir_all(resources.join("licenses"))?;
    fs::write(contents.join("Info.plist"), info_plist(VERSION))?;
    fs::write(contents.join("PkgInfo"), "APPL????")?;
    for binary in ["jetthing", "jetthing-cli"] {
        copy(&stage.join(binary), &macos.join(binary))?;
    }
    copy(
        &Path::new(ICONS).join("jetthing.icns"),
        &resources.join("jetthing.icns"),
    )?;
    for file in ["README.md", "NOTICE-intellij-data.md"] {
        copy(&stage.join(file), &resources.join(file))?;
    }
    for license in ["Inter-OFL.txt", "JetBrainsMono-OFL.txt"] {
        copy(
            &stage.join("licenses").join(license),
            &resources.join("licenses").join(license),
        )?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("/Applications", image_root.join("Applications"))?;

    let identity = std::env::var("JETTHING_MACOS_SIGN_IDENTITY")
        .ok()
        .filter(|identity| !identity.is_empty())
        .unwrap_or_else(|| "-".into());
    let mut codesign = Command::new("codesign");
    codesign.args(["--force", "--deep", "--sign", &identity]);
    if identity != "-" {
        // Hardened runtime and a secure timestamp are required for notarization.
        codesign.args(["--options", "runtime", "--timestamp"]);
    }
    run(codesign.arg(image_root.join("Jetthing.app")), "codesign")?;

    let out = dist.join(format!("jetthing-{VERSION}-macos-{}.dmg", platform.arch));
    let _ = fs::remove_file(&out);
    // hdiutil intermittently fails with "Resource busy" on CI runners.
    let mut attempt = 1;
    loop {
        let result = run(
            Command::new("hdiutil")
                .args(["create", "-volname", &format!("Jetthing {VERSION}")])
                .arg("-srcfolder")
                .arg(&image_root)
                .args(["-ov", "-format", "UDZO"])
                .arg(&out),
            "hdiutil create",
        );
        match result {
            Ok(()) => return Ok(out),
            Err(error) if attempt >= 3 => return Err(error),
            Err(error) => {
                eprintln!("{error}; retrying");
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        }
    }
}

// ---------------------------------------------------------------- Linux

pub fn deb_control(version: &str, arch: &str, installed_kib: u64, glibc: Option<&str>) -> String {
    let libc = glibc.map_or_else(|| "libc6".to_owned(), |v| format!("libc6 (>= {v})"));
    format!(
        "Package: jetthing\n\
         Version: {}\n\
         Section: devel\n\
         Priority: optional\n\
         Architecture: {}\n\
         Installed-Size: {installed_kib}\n\
         Depends: {libc}, libgcc-s1\n\
         Recommends: libgl1, libxkbcommon0, libwayland-client0 | libx11-6, xdg-utils, git\n\
         Maintainer: Jetthing contributors <{HOMEPAGE}/issues>\n\
         Homepage: {HOMEPAGE}\n\
         Description: Native Rust IDE\n \
         Jetthing is an experimental, clean-room, native Rust IDE platform with\n \
         a workbench modelled on the IntelliJ New UI.\n",
        deb_version(version),
        deb_arch(arch),
    )
}

/// Highest `GLIBC_x.y` symbol version the binary needs, via `objdump -T`.
pub fn required_glibc(objdump_output: &str) -> Option<String> {
    objdump_output
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .filter_map(|word| word.strip_prefix("GLIBC_"))
        .filter_map(|version| {
            let parts: Option<Vec<u32>> = version.split('.').map(|p| p.parse().ok()).collect();
            parts.map(|parts| (parts, version.to_owned()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, version)| version)
}

/// `/usr/lib/jetthing` payload, `/usr/bin` links, desktop entry and icons.
fn deb(stage: &Path, platform: &Platform, dist: &Path) -> Result<PathBuf> {
    let root = dist.join(format!("deb-{}", platform.arch));
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    let lib = root.join("usr/lib/jetthing");
    fs::create_dir_all(lib.join("licenses"))?;
    for file in files_under(stage)? {
        let relative = file.strip_prefix(stage)?;
        if relative == Path::new("jetthing.desktop") {
            continue;
        }
        copy(&file, &lib.join(relative))?;
    }
    fs::write(lib.join(INSTALL_MARKER), "deb\n")?;
    let bin = root.join("usr/bin");
    fs::create_dir_all(&bin)?;
    let applications = root.join("usr/share/applications");
    fs::create_dir_all(&applications)?;
    fs::write(
        applications.join("jetthing.desktop"),
        desktop_entry("jetthing"),
    )?;
    for (size, icon) in [("256x256", "jetthing-256.png"), ("512x512", "jetthing.png")] {
        let dir = root.join(format!("usr/share/icons/hicolor/{size}/apps"));
        fs::create_dir_all(&dir)?;
        copy(&Path::new(ICONS).join(icon), &dir.join("jetthing.png"))?;
    }
    let doc = root.join("usr/share/doc/jetthing");
    fs::create_dir_all(&doc)?;
    fs::write(
        doc.join("copyright"),
        format!(
            "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n\
             Upstream-Name: jetthing\nSource: {HOMEPAGE}\n\n\
             Files: *\nLicense: Apache-2.0 OR MIT\n\n\
             Files: usr/lib/jetthing/licenses/*\nLicense: OFL-1.1\n"
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        for binary in ["jetthing", "jetthing-cli"] {
            symlink(format!("../lib/jetthing/{binary}"), bin.join(binary))?;
        }
        // Normalise modes regardless of the builder's umask.
        for file in files_under(&root)? {
            let executable = file.parent() == Some(lib.as_path())
                && matches!(
                    file.file_name().and_then(|n| n.to_str()),
                    Some("jetthing" | "jetthing-cli")
                );
            if !fs::symlink_metadata(&file)?.file_type().is_symlink() {
                let mode = if executable { 0o755 } else { 0o644 };
                fs::set_permissions(&file, fs::Permissions::from_mode(mode))?;
            }
        }
    }
    let installed_bytes: u64 = files_under(&root)?
        .iter()
        .filter_map(|file| fs::symlink_metadata(file).ok())
        .map(|meta| meta.len())
        .sum();
    let glibc = Command::new("objdump")
        .arg("-T")
        .arg(lib.join("jetthing"))
        .output()
        .ok()
        .and_then(|output| required_glibc(&String::from_utf8_lossy(&output.stdout)));
    let debian = root.join("DEBIAN");
    fs::create_dir_all(&debian)?;
    fs::write(
        debian.join("control"),
        deb_control(
            VERSION,
            &platform.arch,
            installed_bytes.div_ceil(1024),
            glibc.as_deref(),
        ),
    )?;
    let out = dist.join(format!(
        "jetthing_{}_{}.deb",
        deb_version(VERSION),
        deb_arch(&platform.arch)
    ));
    run(
        Command::new("dpkg-deb")
            .args(["--root-owner-group", "-Zxz", "--build"])
            .arg(&root)
            .arg(&out),
        "dpkg-deb",
    )?;
    Ok(out)
}

/// Portable single-file build: an AppDir (`usr/bin`, desktop entry, icons,
/// `AppRun`) packed by `appimagetool` (path in `APPIMAGETOOL`, else on PATH).
/// The self-updater replaces the `.AppImage` file in place (`$APPIMAGE`).
fn appimage(stage: &Path, platform: &Platform, dist: &Path) -> Result<PathBuf> {
    let app_dir = dist.join(format!("AppDir-{}", platform.arch));
    if app_dir.exists() {
        fs::remove_dir_all(&app_dir)?;
    }
    let bin = app_dir.join("usr/bin");
    fs::create_dir_all(&bin)?;
    for binary in ["jetthing", "jetthing-cli"] {
        copy(&stage.join(binary), &bin.join(binary))?;
    }
    let share = app_dir.join("usr/share/jetthing");
    fs::create_dir_all(share.join("licenses"))?;
    for file in ["README.md", "NOTICE-intellij-data.md"] {
        copy(&stage.join(file), &share.join(file))?;
    }
    for license in ["Inter-OFL.txt", "JetBrainsMono-OFL.txt"] {
        copy(
            &stage.join("licenses").join(license),
            &share.join("licenses").join(license),
        )?;
    }
    let applications = app_dir.join("usr/share/applications");
    fs::create_dir_all(&applications)?;
    let desktop = desktop_entry("jetthing");
    fs::write(applications.join("jetthing.desktop"), &desktop)?;
    fs::write(app_dir.join("jetthing.desktop"), &desktop)?;
    for (size, icon) in [("256x256", "jetthing-256.png"), ("512x512", "jetthing.png")] {
        let dir = app_dir.join(format!("usr/share/icons/hicolor/{size}/apps"));
        fs::create_dir_all(&dir)?;
        copy(&Path::new(ICONS).join(icon), &dir.join("jetthing.png"))?;
    }
    copy(
        &Path::new(ICONS).join("jetthing.png"),
        &app_dir.join("jetthing.png"),
    )?;
    fs::write(app_dir.join("AppRun"), APP_RUN)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        fs::set_permissions(app_dir.join("AppRun"), fs::Permissions::from_mode(0o755))?;
        symlink("jetthing.png", app_dir.join(".DirIcon"))?;
    }
    let out = dist.join(format!(
        "jetthing-{VERSION}-linux-{}.AppImage",
        platform.arch
    ));
    let _ = fs::remove_file(&out);
    let tool = std::env::var("APPIMAGETOOL").unwrap_or_else(|_| "appimagetool".into());
    run(
        Command::new(tool)
            // Runs without FUSE (containers, CI).
            .env("APPIMAGE_EXTRACT_AND_RUN", "1")
            .env("ARCH", platform.arch.as_str())
            .arg("--no-appstream")
            .arg(&app_dir)
            .arg(&out),
        "appimagetool",
    )?;
    Ok(out)
}

/// Starts the bundled IDE; `APPDIR` is set by the AppImage runtime.
const APP_RUN: &str = r#"#!/bin/sh
HERE="${APPDIR:-$(dirname "$(readlink -f "$0")")}"
exec "$HERE/usr/bin/jetthing" "$@"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_options() {
        let parse = |args: &[&str]| Options::parse(args.iter().map(|a| a.to_string()));
        assert_eq!(parse(&[]).unwrap(), Options::default());
        assert_eq!(
            parse(&["--installer", "--target", "x86_64-apple-darwin"]).unwrap(),
            Options {
                target: Some("x86_64-apple-darwin".into()),
                installer: true
            }
        );
        assert!(parse(&["--target"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }

    #[test]
    fn maps_target_triples_to_release_names() {
        let platform = |triple| Platform::from_triple(triple).unwrap();
        assert_eq!(
            platform("x86_64-pc-windows-msvc"),
            Platform {
                os: "windows".into(),
                arch: "x86_64".into()
            }
        );
        assert_eq!(platform("aarch64-apple-darwin").os, "macos");
        assert_eq!(platform("x86_64-apple-darwin").arch, "x86_64");
        assert_eq!(platform("aarch64-unknown-linux-gnu").os, "linux");
        assert!(Platform::from_triple("wasm32-unknown-unknown").is_err());
    }

    #[test]
    fn versions_fit_each_package_format() {
        assert_eq!(numeric_version("0.0.1"), "0.0.1");
        assert_eq!(numeric_version("1.2.0-beta.1"), "1.2.0");
        assert_eq!(deb_version("1.2.0-beta.1"), "1.2.0~beta.1");
        assert_eq!(deb_arch("x86_64"), "amd64");
        assert_eq!(deb_arch("aarch64"), "arm64");
    }

    #[test]
    fn deb_control_declares_package_metadata() {
        let control = deb_control("0.0.1", "x86_64", 4242, Some("2.34"));
        assert!(control.contains("Package: jetthing\n"));
        assert!(control.contains("Version: 0.0.1\n"));
        assert!(control.contains("Architecture: amd64\n"));
        assert!(control.contains("Installed-Size: 4242\n"));
        assert!(control.contains("Depends: libc6 (>= 2.34), libgcc-s1\n"));
        // Every continuation line of the description starts with a space.
        let description = control.split("Description:").nth(1).unwrap();
        assert!(
            description
                .lines()
                .skip(1)
                .all(|line| line.starts_with(' '))
        );
        assert!(control.ends_with('\n'));
    }

    #[test]
    fn finds_the_newest_glibc_symbol_version() {
        let objdump = "0000 DF *UND* 0000 (GLIBC_2.2.5) memcpy\n\
                       0000 DF *UND* 0000 (GLIBC_2.34) __libc_start_main\n\
                       0000 DF *UND* 0000 (GLIBC_2.9) pipe2\n\
                       0000 DF *UND* 0000 (GLIBC_PRIVATE) x\n";
        assert_eq!(required_glibc(objdump).as_deref(), Some("2.34"));
        assert_eq!(required_glibc("nothing"), None);
    }

    #[test]
    fn bundle_metadata_names_the_executable_and_icon() {
        let plist = info_plist("1.2.0-beta.1");
        assert!(plist.contains("<key>CFBundleExecutable</key><string>jetthing</string>"));
        assert!(plist.contains("<key>CFBundleIconFile</key><string>jetthing</string>"));
        assert!(plist.contains(&format!("<string>{BUNDLE_ID}</string>")));
        assert!(plist.contains("<key>CFBundleVersion</key><string>1.2.0</string>"));
        let desktop = desktop_entry("jetthing");
        assert!(desktop.starts_with("[Desktop Entry]\n"));
        assert!(desktop.contains("Exec=jetthing\n"));
        assert!(desktop.contains("Icon=jetthing\n"));
    }

    #[test]
    fn checksum_files_use_sha256sum_format() {
        let dir = std::env::temp_dir().join(format!("xtask-sum-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("jetthing.tar.gz");
        fs::write(&file, b"abc").unwrap();
        write_checksum(&file).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("jetthing.tar.gz.sha256")).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  jetthing.tar.gz\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn icon_assets_exist_for_every_installer() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        for icon in [
            "jetthing.svg",
            "jetthing.png",
            "jetthing-256.png",
            "jetthing.ico",
            "jetthing.icns",
        ] {
            assert!(root.join(ICONS).join(icon).is_file(), "{icon}");
        }
        assert!(root.join("packaging/windows/jetthing.wxs").is_file());
    }
}
