//! `cargo xtask icons`: the app icons in `assets/logo/icon/`, generated from the logo.
//!
//! ImageMagick (`convert`, or the program named by `MAGICK`) trims the logo, scales it into
//! the 824 px safe area of a transparent 1024 px square (100 px margin, the macOS icon grid)
//! and downscales that master to every PNG size. The `.ico` and `.icns` containers are
//! written here from those PNGs. Metadata chunks are stripped, so the output is byte-for-byte
//! reproducible for a given ImageMagick version.

use anyhow::{Context, Result, bail, ensure};
use std::fs;
use std::path::Path;
use std::process::Command;

/// The original artwork.
pub const LOGO: &str = "assets/logo/mongo-raabit.png";
/// Where the generated icons live.
pub const DIR: &str = "assets/logo/icon";
/// Windows icon (MSI, Add/Remove Programs, Start Menu shortcut).
pub const ICO: &str = "assets/logo/icon/raabit.ico";
/// macOS icon (`Contents/Resources/raabit.icns`).
pub const ICNS: &str = "assets/logo/icon/raabit.icns";

/// Side of the master image.
const MASTER: u32 = 1024;
/// Side of the box the logo is fitted into, centered on the master.
const SAFE_AREA: u32 = 824;
/// PNG sizes written as `raabit-<size>.png`; the largest is the master.
pub const PNG_SIZES: [u32; 8] = [16, 32, 48, 64, 128, 256, 512, 1024];
/// Sizes in the `.ico`, all PNG-encoded (Windows Vista and later).
pub const ICO_SIZES: [u32; 6] = [16, 32, 48, 64, 128, 256];
/// `.icns` entries: OSType and pixel size, all PNG-encoded (macOS 10.7 and later).
pub const ICNS_ENTRIES: [(&[u8; 4], u32); 11] = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"icp6", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024), // 512@2x
    (b"ic11", 32),   // 16@2x
    (b"ic12", 64),   // 32@2x
    (b"ic13", 256),  // 128@2x
    (b"ic14", 512),  // 256@2x
];

pub fn png_path(size: u32) -> String {
    format!("{DIR}/raabit-{size}.png")
}

pub fn run(mut args: impl Iterator<Item = String>) -> Result<()> {
    if let Some(arg) = args.next() {
        bail!("unknown argument `{arg}`; usage: cargo xtask icons");
    }
    let magick = std::env::var("MAGICK").unwrap_or_else(|_| "convert".into());
    fs::create_dir_all(DIR)?;
    let master = png_path(MASTER);
    let safe = format!("{SAFE_AREA}x{SAFE_AREA}");
    let side = format!("{MASTER}x{MASTER}");
    magick_png(
        &magick,
        &[
            LOGO,
            "-trim",
            "+repage",
            "-filter",
            "Lanczos",
            "-resize",
            &safe,
            "-background",
            "none",
            "-gravity",
            "center",
            "-extent",
            &side,
        ],
        &master,
    )?;
    for size in PNG_SIZES.into_iter().filter(|&size| size != MASTER) {
        let side = format!("{size}x{size}");
        magick_png(
            &magick,
            &[&master, "-filter", "Lanczos", "-resize", &side],
            &png_path(size),
        )?;
    }
    let read = |size: u32| fs::read(png_path(size)).with_context(|| png_path(size));
    let ico_images = ICO_SIZES
        .into_iter()
        .map(read)
        .collect::<Result<Vec<_>>>()?;
    let ico_bytes = ico(&ico_images)?;
    ensure!(
        parse_ico(&ico_bytes)?.len() == ICO_SIZES.len(),
        "ico self-check failed"
    );
    fs::write(ICO, ico_bytes)?;
    let icns_entries = ICNS_ENTRIES
        .into_iter()
        .map(|(kind, size)| Ok((*kind, read(size)?)))
        .collect::<Result<Vec<_>>>()?;
    let icns_bytes = icns(&icns_entries)?;
    ensure!(
        parse_icns(&icns_bytes)?.len() == ICNS_ENTRIES.len(),
        "icns self-check failed"
    );
    fs::write(ICNS, icns_bytes)?;
    for size in PNG_SIZES {
        println!("{}", png_path(size));
    }
    println!("{ICO}\n{ICNS}");
    Ok(())
}

/// Runs ImageMagick with `args` and writes a stripped 8-bit RGBA PNG to `out`.
fn magick_png(magick: &str, args: &[&str], out: &str) -> Result<()> {
    let status = Command::new(magick)
        .args(args)
        .args([
            "-strip",
            "-define",
            "png:exclude-chunks=date,time",
            "-define",
            "png:compression-level=9",
        ])
        .arg(format!("PNG32:{out}"))
        .status()
        .with_context(|| format!("run {magick} (is ImageMagick installed? set MAGICK)"))?;
    ensure!(status.success(), "{magick} failed for {out} ({status})");
    Ok(())
}

/// Width and height from a PNG's IHDR chunk.
pub fn png_size(png: &[u8]) -> Result<(u32, u32)> {
    ensure!(
        png.len() >= 24 && png.starts_with(b"\x89PNG\r\n\x1a\n") && &png[12..16] == b"IHDR",
        "not a PNG"
    );
    let be = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
    Ok((be(16), be(20)))
}

/// An `.ico` with one PNG-encoded image per entry.
pub fn ico(pngs: &[Vec<u8>]) -> Result<Vec<u8>> {
    let count = u16::try_from(pngs.len()).context("too many images")?;
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]); // reserved, type 1 = icon
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = 6 + 16 * pngs.len();
    for png in pngs {
        let (width, height) = png_size(png)?;
        ensure!(
            width <= 256 && height <= 256,
            "ico images are at most 256 px"
        );
        // 256 is stored as 0.
        out.push(width as u8);
        out.push(height as u8);
        out.extend_from_slice(&[0, 0]); // palette size, reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // color planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&u32::try_from(png.len())?.to_le_bytes());
        out.extend_from_slice(&u32::try_from(offset)?.to_le_bytes());
        offset += png.len();
    }
    for png in pngs {
        out.extend_from_slice(png);
    }
    Ok(out)
}

/// `(width, height, image bytes)` of every entry of an `.ico`.
pub fn parse_ico(ico: &[u8]) -> Result<Vec<(u32, u32, &[u8])>> {
    ensure!(
        ico.len() >= 6 && ico[..4] == [0, 0, 1, 0],
        "not an icon file"
    );
    let count = usize::from(u16::from_le_bytes([ico[4], ico[5]]));
    let le = |at: usize| u32::from_le_bytes([ico[at], ico[at + 1], ico[at + 2], ico[at + 3]]);
    (0..count)
        .map(|index| {
            let entry = 6 + 16 * index;
            ensure!(ico.len() >= entry + 16, "truncated directory");
            let side = |byte: u8| if byte == 0 { 256 } else { u32::from(byte) };
            let (len, at) = (le(entry + 8) as usize, le(entry + 12) as usize);
            let data = ico.get(at..at + len).context("entry out of bounds")?;
            Ok((side(ico[entry]), side(ico[entry + 1]), data))
        })
        .collect()
}

/// An `.icns`: big-endian `icns` header, then `(OSType, length, data)` entries.
pub fn icns(entries: &[([u8; 4], Vec<u8>)]) -> Result<Vec<u8>> {
    let total = 8 + entries
        .iter()
        .map(|(_, data)| 8 + data.len())
        .sum::<usize>();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&u32::try_from(total)?.to_be_bytes());
    for (kind, data) in entries {
        out.extend_from_slice(kind);
        out.extend_from_slice(&u32::try_from(8 + data.len())?.to_be_bytes());
        out.extend_from_slice(data);
    }
    Ok(out)
}

/// `(OSType, data)` of every entry of an `.icns`.
pub fn parse_icns(icns: &[u8]) -> Result<Vec<([u8; 4], &[u8])>> {
    let be = |at: usize| u32::from_be_bytes([icns[at], icns[at + 1], icns[at + 2], icns[at + 3]]);
    ensure!(
        icns.len() >= 8 && icns.starts_with(b"icns"),
        "not an icns file"
    );
    ensure!(
        be(4) as usize == icns.len(),
        "icns length does not match the file"
    );
    let mut entries = Vec::new();
    let mut at = 8;
    while at < icns.len() {
        ensure!(icns.len() >= at + 8, "truncated entry header");
        let kind = [icns[at], icns[at + 1], icns[at + 2], icns[at + 3]];
        let len = be(at + 4) as usize;
        ensure!(len >= 8, "bad entry length");
        let data = icns.get(at + 8..at + len).context("entry out of bounds")?;
        entries.push((kind, data));
        at += len;
    }
    Ok(entries)
}

/// Fails unless `path` exists, for steps that package the icons.
pub fn require(path: &str) -> Result<&Path> {
    let path = Path::new(path);
    ensure!(
        path.is_file(),
        "{} is missing; run `cargo xtask icons`",
        path.display()
    );
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICO_FILE: &[u8] = include_bytes!("../../assets/logo/icon/raabit.ico");
    const ICNS_FILE: &[u8] = include_bytes!("../../assets/logo/icon/raabit.icns");

    fn committed_png(size: u32) -> Vec<u8> {
        fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join(png_path(size)),
        )
        .unwrap_or_else(|error| panic!("{}: {error}", png_path(size)))
    }

    #[test]
    fn committed_pngs_are_square_at_every_size() {
        for size in PNG_SIZES {
            assert_eq!(png_size(&committed_png(size)).unwrap(), (size, size));
        }
    }

    #[test]
    fn committed_ico_has_every_size_as_png() {
        let entries = parse_ico(ICO_FILE).unwrap();
        assert_eq!(entries.len(), ICO_SIZES.len());
        for ((width, height, data), size) in entries.into_iter().zip(ICO_SIZES) {
            assert_eq!((width, height), (size, size));
            assert_eq!(png_size(data).unwrap(), (size, size));
            assert_eq!(data, committed_png(size), "ico {size} px is stale");
        }
    }

    #[test]
    fn committed_icns_has_every_entry_as_png() {
        let entries = parse_icns(ICNS_FILE).unwrap();
        assert_eq!(entries.len(), ICNS_ENTRIES.len());
        for ((kind, data), (want_kind, size)) in entries.into_iter().zip(ICNS_ENTRIES) {
            assert_eq!(&kind, want_kind);
            assert_eq!(png_size(data).unwrap(), (size, size), "{kind:?}");
            assert_eq!(data, committed_png(size), "icns {kind:?} is stale");
        }
    }

    #[test]
    fn containers_round_trip_and_reject_garbage() {
        let png = committed_png(16);
        let ico_bytes = ico(&[png.clone(), committed_png(256)]).unwrap();
        let parsed = parse_ico(&ico_bytes).unwrap();
        assert_eq!((parsed[0].0, parsed[1].0), (16, 256));
        assert_eq!(parsed[0].2, png);
        let icns_bytes = icns(&[(*b"icp4", png.clone())]).unwrap();
        assert_eq!(parse_icns(&icns_bytes).unwrap(), [(*b"icp4", &png[..])]);

        assert!(ico(&[committed_png(512)]).is_err());
        assert!(parse_ico(b"nope").is_err());
        assert!(parse_icns(&icns_bytes[..icns_bytes.len() - 1]).is_err());
        assert!(png_size(b"\x89PNG").is_err());
    }
}
