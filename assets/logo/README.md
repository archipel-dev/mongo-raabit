# Mongo Raabit logo

`mongo-raabit.png` is the original artwork (1279×1230, transparent background), supplied by
the project owner. Everything in `icon/` is generated from it; don't edit those files by hand.

| File | Used by |
|---|---|
| `icon/raabit-<size>.png` (16, 32, 48, 64, 128, 256, 512, 1024) | `raabit-256.png` is the window icon (`crates/raabit/src/main.rs`); `raabit-1024.png` is the master the others are scaled from |
| `icon/raabit.ico` (16 to 256, PNG entries) | the MSI: Add/Remove Programs and the Start Menu shortcut |
| `icon/raabit.icns` (`icp4`..`icp6`, `ic07`..`ic14`, PNG entries) | `Contents/Resources/raabit.icns` of the macOS app (`CFBundleIconFile`) |

## Regenerate

```bash
cargo xtask icons            # needs ImageMagick; MAGICK=magick for ImageMagick 7
```

The logo is trimmed to its visible pixels and fitted, centered, into the 824 px safe area of a
transparent 1024 px square (100 px margin, as on the macOS icon grid). Smaller sizes are
Lanczos downscales of that master. The `.ico` and `.icns` containers are written by the xtask
from the PNGs. Metadata is stripped, so running it twice gives identical bytes. The xtask
tests check that the committed `.ico` and `.icns` parse and match the PNGs.
