# Third-party notices

OpenHEIC's own source code is dual-licensed under **MIT OR Apache-2.0**
(see `LICENSE-MIT` and `LICENSE-APACHE`). The binaries additionally contain
the following third-party components.

## Statically linked C/C++ libraries (LGPL-3.0)

| Component | Version | License | Source |
|---|---|---|---|
| libheif | 1.23.7 | LGPL-3.0 | https://github.com/strukturag/libheif |
| libde265 | 1.1.3 | LGPL-3.0 | https://github.com/strukturag/libde265 |

Full license texts: [`licenses/libheif-COPYING.txt`](licenses/libheif-COPYING.txt),
[`licenses/libde265-COPYING.txt`](licenses/libde265-COPYING.txt).

**LGPL relinking:** `heic-convert.exe` and `heic_wic.dll` link these libraries
statically. As the LGPL requires, you can replace them: the complete source of
OpenHEIC and its build scripts are in this repository. Point
`scripts/setup-deps.ps1` (vcpkg) at your modified libheif/libde265, for example
with a vcpkg overlay port, then run `cargo build --release` to produce
binaries linked against your version.

## Rust crates

All Rust dependencies are under permissive licenses (MIT, Apache-2.0,
Unlicense/MIT or similar). Notable ones: `libheif-rs` / `libheif-sys` (MIT),
`jpeg-encoder` (MIT OR Apache-2.0), `windows` / `windows-core` (MIT OR
Apache-2.0), `clap`, `rayon`, `walkdir`, `indicatif`, `winreg`, `anyhow`,
`thiserror` (MIT OR Apache-2.0 / MIT). Run `cargo tree` for the full list.

## Test data

`crates/heic-core/tests/data/*.heic|heif` come from
[libheif-rs](https://github.com/Cykooz/libheif-rs) and are licensed under
CC BY-SA 4.0 (see `crates/heic-core/tests/data/LICENSE-CC-BY-SA-4.0`).
They are not included in release binaries.

## Patents

HEVC (H.265) is covered by patents in some countries. OpenHEIC contains an
HEVC *decoder* (libde265). You are responsible for checking whether using it
requires a patent licence where you live.
