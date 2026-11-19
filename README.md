# OpenHEIC

**View HEIC photos natively on Windows 11 and convert them to JPEG, for free.**

[![CI](https://github.com/Devesh1312/OpenHEIC/actions/workflows/ci.yml/badge.svg)](https://github.com/Devesh1312/OpenHEIC/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

iPhones and many Android phones save photos as `.heic`. Out of the box, Windows needs Microsoft's
*paid* "HEVC Video Extensions" to show them. OpenHEIC is an open-source alternative written in Rust
and built on [libheif](https://github.com/strukturag/libheif) + [libde265](https://github.com/strukturag/libde265):

- **Native viewing.** A Windows Imaging Component (WIC) codec. Once installed, **Photos**, **Windows Photo Viewer**,
  **Explorer thumbnails**, Paint and any other WIC-based app open `.heic` / `.heif` / `.hif` files.
- **Converter.** Convert single files or whole folder trees to JPEG, from the command line or
  by **right-clicking** in Explorer.
- **Self-contained.** No codecs or runtimes to install; libheif and the C runtime are linked statically.

## Install

1. Download the latest `OpenHEIC-vX.Y.Z-windows-x64.zip` from [Releases](https://github.com/Devesh1312/OpenHEIC/releases) and extract it.
2. Open a terminal in that folder and run:
   ```powershell
   .\heic-convert.exe install
   ```
   Approve the UAC prompt. This copies the files to `C:\Program Files\OpenHEIC`, registers the codec and
   adds the right-click menu. It also adds an *OpenHEIC* entry in **Settings › Apps** that you can use to uninstall.

> [!IMPORTANT]
> **If you have Microsoft's "HEIF Image Extension" but not the paid "HEVC Video Extension"**, Windows
> keeps choosing Microsoft's codec, which then fails. Remove it so OpenHEIC is used:
> ```powershell
> Get-AppxPackage *HEIFImageExtension* | Remove-AppxPackage
> ```
> You can reinstall it from the Microsoft Store at any time.

**Default app:** to make `.heic` open in the same app as your JPEGs, right-click a `.heic` file ›
*Open with* › *Choose another app* › pick **Photos** › **Always**. Windows only lets you change
defaults yourself, so OpenHEIC never changes them.

To uninstall: run `heic-convert uninstall`, or use Settings › Apps › OpenHEIC.

## Convert HEIC to JPEG

**From Explorer:** right-click a `.heic` file, a folder, or the empty space inside a folder, then choose
**Show more options › Convert to JPEG**.

**From the command line:**
```text
heic-convert convert <PATH>... [options]

  PATH                 HEIC files and/or folders (folders are scanned recursively)
  -o, --out <DIR>      write into DIR, mirroring the folder structure (default: next to each source)
      --overwrite      replace JPEGs that already exist (default: skip them)
  -q, --quality <N>    1-100 (default 100)
      --strip-metadata don't copy EXIF / XMP / ICC
  -j, --jobs <N>       parallel conversions (default: all CPU cores)
      --no-recursive   only the top level of folder arguments
```
```powershell
heic-convert convert D:\Photos                    # every HEIC under D:\Photos -> .jpg next to it
heic-convert convert a.heic b.HEIC --out D:\Jpegs
```
Exit codes: `0` success · `1` usage error · `2` some files failed (listed at the end).

### Quality
JPEG is always lossy, so OpenHEIC uses the highest-fidelity settings:

- **Quality 100 with 4:4:4 chroma** (no colour subsampling). Typical PSNR against the decoded HEIC is 57–59 dB, which is visually lossless.
- **Colour profile kept.** An embedded ICC profile (e.g. Display P3 on iPhones) is copied. HEICs that only carry an
  nclx colour description get a matching ICC profile generated for them.
- **EXIF and XMP kept** (date taken, camera, GPS...). The orientation tag is reset to *normal* because the pixels are
  written already rotated, so no viewer rotates the photo twice.
- File created and modified dates are copied from the original.
- 10/12-bit HDR sources are reduced to 8-bit (a JPEG limitation).

## Build from source

Requirements: Windows 10/11 x64, [Rust](https://rustup.rs) (MSVC toolchain), Visual Studio 2022+ with the
*Desktop development with C++* workload, and git.

```powershell
git clone https://github.com/Devesh1312/OpenHEIC.git
cd OpenHEIC
powershell -ExecutionPolicy Bypass -File scripts\setup-deps.ps1   # once: vcpkg + static libheif (~3 min)
cargo build --release
cargo test --release
```
The build produces `target\release\heic-convert.exe` and `target\release\heic_wic.dll`. Keep them in the same folder for `install`.

Optional: put your own HEIC photos in a `sample\` folder (git-ignored). The tests will then check them too.

### Project layout
| Crate | Output | Purpose |
|---|---|---|
| `crates/heic-core` | library | libheif decode → upright 8-bit RGB, metadata handling, JPEG encoding |
| `crates/heic-convert` | `heic-convert.exe` | CLI, parallel batch conversion, install/uninstall, Explorer menus |
| `crates/heic-wic` | `heic_wic.dll` | COM server implementing `IWICBitmapDecoder` / `IWICBitmapFrameDecode` |

Test the codec without installing it:
```powershell
cargo run --release -p heic-wic --example wic_probe -- inproc crates\heic-core\tests\data\test.heic
# After install: shows which decoder Windows actually picks
cargo run --release -p heic-wic --example wic_probe -- system crates\heic-core\tests\data\test.heic
```

## Known limitations
- The viewer codec doesn't pass EXIF to Windows yet, so Explorer's *Details* pane shows no "Date taken" for HEIC.
  Converted JPEGs keep all EXIF.
- Only HEVC-coded HEIF is supported. AV1 (AVIF) inside HEIF isn't.
- Only the primary image is decoded. Bursts, depth maps and Live Photo motion are ignored.
- The right-click entries appear under *Show more options* on Windows 11.
- Binaries are not code-signed yet, so SmartScreen may warn on first run.

## Contributing
Issues and pull requests are welcome. Please run `cargo test --release` before submitting.

## License
OpenHEIC is dual-licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Release binaries statically include libheif and libde265 (LGPL-3.0). See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md),
which also covers how to relink and the HEVC patent notice.
