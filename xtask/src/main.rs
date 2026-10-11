//! `cargo xtask dist`: release builds, archives and installers for the `raabit` binary.
//!
//! Stages `dist/raabit-<version>-<os>-<arch>/` and packs it into a `.tar.gz`
//! (`.zip` on Windows). With `--installer` it also builds the native installer:
//!
//! | OS | Installer | Tooling |
//! |---|---|---|
//! | Windows | `raabit-<version>-windows-<arch>.msi` | WiX Toolset v5 (`wix`) |
//! | macOS | `raabit-<version>-macos-<arch>.dmg` | `codesign`, `hdiutil` |
//!
//! Every artifact gets a `sha256sum`-format `<file>.sha256`.

mod perf;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Cargo package that builds the `raabit` binary.
const PACKAGE: &str = "raabit";
const BUNDLE_ID: &str = "dev.archipel.raabit";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("dist") => dist(Options::parse(args)?),
        Some("perf") => perf::run(args),
        _ => bail!(
            "usage: cargo xtask dist [--installer] [--target <triple>]\n       cargo xtask perf [--runs N] [--uri URI] [--no-build] [--bin PATH] [--budgets FILE] [--json FILE]"
        ),
    }
}

struct Options {
    installer: bool,
    target: Option<String>,
}

impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut options = Options {
            installer: false,
            target: None,
        };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--installer" => options.installer = true,
                "--target" => options.target = Some(args.next().context("--target needs a value")?),
                other => bail!("unknown argument `{other}`"),
            }
        }
        Ok(options)
    }
}

struct Platform {
    os: &'static str,
    arch: String,
}

impl Platform {
    fn host() -> Result<Self> {
        let os = match std::env::consts::OS {
            "windows" => "windows",
            "macos" => "macos",
            "linux" => "linux",
            other => bail!("unsupported host OS `{other}`"),
        };
        Ok(Platform {
            os,
            arch: std::env::consts::ARCH.into(),
        })
    }

    /// `x86_64-pc-windows-msvc` → windows / x86_64.
    fn from_triple(triple: &str) -> Result<Self> {
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
            os,
            arch: arch.into(),
        })
    }

    fn exe(&self) -> &'static str {
        if self.os == "windows" { ".exe" } else { "" }
    }
}

/// `1.2.3-beta.1` → `1.2.3`: MSI and `CFBundleVersion` take numbers only.
fn numeric_version(version: &str) -> &str {
    version.split(['-', '+']).next().unwrap_or(version)
}

/// Fails with a clear message when the `raabit` package is not in the workspace yet.
fn ensure_package() -> Result<()> {
    let output = Command::new(cargo())
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .context("run cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let metadata = String::from_utf8_lossy(&output.stdout);
    if !metadata.contains(&format!("\"name\":\"{PACKAGE}\"")) {
        bail!(
            "the `{PACKAGE}` package is not a member of this workspace, so there is nothing to \
             package. It is added by the standalone-app port (crates/raabit, binary `{PACKAGE}`)."
        );
    }
    Ok(())
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn dist(options: Options) -> Result<()> {
    ensure_package()?;
    let platform = match &options.target {
        Some(triple) => Platform::from_triple(triple)?,
        None => Platform::host()?,
    };
    let release_dir = build(options.target.as_deref())?;
    let name = format!("raabit-{VERSION}-{}-{}", platform.os, platform.arch);
    let dist = Path::new("dist");
    let stage = dist.join(&name);
    stage_files(&release_dir, &stage, &platform)?;
    let mut artifacts = vec![if platform.os == "windows" {
        zip_dir(&stage, &dist.join(format!("{name}.zip")))?
    } else {
        tar_gz_dir(&stage, &dist.join(format!("{name}.tar.gz")))?
    }];
    if options.installer {
        match platform.os {
            "windows" => artifacts.push(msi(&stage, &platform, dist)?),
            "macos" => artifacts.push(dmg(&stage, &platform, dist)?),
            _ => {}
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
    let mut command = Command::new(cargo());
    command.args(["build", "--release", "-p", PACKAGE]);
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
    fs::create_dir_all(out)?;
    let exe = platform.exe();
    copy(
        &release_dir.join(format!("{PACKAGE}{exe}")),
        &out.join(format!("raabit{exe}")),
    )?;
    for file in ["README.md", "LICENSE-APACHE", "LICENSE-MIT"] {
        copy(Path::new(file), &out.join(file))?;
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

/// `sha256sum`-compatible `<artifact>.sha256`.
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

/// Builds the MSI from `xtask/packaging/windows/raabit.wxs` with WiX v5.
fn msi(stage: &Path, platform: &Platform, dist: &Path) -> Result<PathBuf> {
    let arch = match platform.arch.as_str() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => bail!("no MSI architecture for {other}"),
    };
    let out = dist.join(format!("raabit-{VERSION}-windows-{}.msi", platform.arch));
    let wix = std::env::var("WIX_EXE").unwrap_or_else(|_| "wix".into());
    run(
        Command::new(wix)
            .args(["build", "-nologo", "-arch", arch])
            .arg("-d")
            .arg(format!("Version={}", numeric_version(VERSION)))
            .arg("-d")
            .arg(format!("Stage={}", absolute(stage)?.display()))
            .arg("-o")
            .arg(&out)
            .arg("xtask/packaging/windows/raabit.wxs"),
        "wix build",
    )?;
    Ok(out)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(std::env::current_dir()?.join(path))
}

fn info_plist(version: &str) -> String {
    let short = numeric_version(version);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>Mongo Raabit</string>
  <key>CFBundleExecutable</key><string>raabit</string>
  <key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>Mongo Raabit</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{short}</string>
  <key>CFBundleVersion</key><string>{short}</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>Mongo Raabit {version}. Apache-2.0 OR MIT.</string>
</dict>
</plist>
"#
    )
}

/// `Mongo Raabit.app` inside a compressed `.dmg` with an Applications link.
/// Signs ad hoc unless `RAABIT_MACOS_SIGN_IDENTITY` names a certificate.
fn dmg(stage: &Path, platform: &Platform, dist: &Path) -> Result<PathBuf> {
    let image_root = dist.join(format!("dmg-{}", platform.arch));
    if image_root.exists() {
        fs::remove_dir_all(&image_root)?;
    }
    let app = image_root.join("Mongo Raabit.app");
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos)?;
    fs::create_dir_all(&resources)?;
    fs::write(contents.join("Info.plist"), info_plist(VERSION))?;
    fs::write(contents.join("PkgInfo"), "APPL????")?;
    copy(&stage.join("raabit"), &macos.join("raabit"))?;
    for file in ["README.md", "LICENSE-APACHE", "LICENSE-MIT"] {
        copy(&stage.join(file), &resources.join(file))?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("/Applications", image_root.join("Applications"))?;

    let identity = std::env::var("RAABIT_MACOS_SIGN_IDENTITY")
        .ok()
        .filter(|identity| !identity.is_empty())
        .unwrap_or_else(|| "-".into());
    let mut codesign = Command::new("codesign");
    codesign.args(["--force", "--deep", "--sign", &identity]);
    if identity != "-" {
        // Hardened runtime and a secure timestamp are required for notarization.
        codesign.args(["--options", "runtime", "--timestamp"]);
    }
    run(codesign.arg(&app), "codesign")?;

    let out = dist.join(format!("raabit-{VERSION}-macos-{}.dmg", platform.arch));
    let _ = fs::remove_file(&out);
    // hdiutil intermittently fails with "Resource busy" on CI runners.
    let mut attempt = 1;
    loop {
        let result = run(
            Command::new("hdiutil")
                .args(["create", "-volname", &format!("Mongo Raabit {VERSION}")])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_version_drops_prerelease() {
        assert_eq!(numeric_version("1.2.3-beta.1"), "1.2.3");
        assert_eq!(numeric_version("1.2.3"), "1.2.3");
    }

    #[test]
    fn platform_from_triple() {
        let p = Platform::from_triple("aarch64-apple-darwin").unwrap();
        assert_eq!((p.os, p.arch.as_str()), ("macos", "aarch64"));
        assert!(Platform::from_triple("wasm32-unknown-unknown").is_err());
    }
}
