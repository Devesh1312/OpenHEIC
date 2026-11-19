# Test images

These HEIF/HEIC files come from the test data of
[libheif-rs](https://github.com/Cykooz/libheif-rs) (`data/` directory, crate v3.0.0)
and are licensed under **CC BY-SA 4.0** (see `LICENSE-CC-BY-SA-4.0`).
They are used here unmodified, only for automated tests.

| File | Codec | Covers |
|---|---|---|
| `test.heic` | HEVC | 8-bit, iPhone 14 Pro EXIF, embedded thumbnail |
| `test_nclx.heif` | HEVC | 12-bit HDR (BT.2020 / PQ nclx) |
| `alpha.heif` | HEVC | alpha channel |
| `test.heif` | AV1 | 8-bit, embedded ICC profile + EXIF |
| `grid.heif` | AV1 | grid (tiled) image, sRGB nclx |
| `collection.heif` | AV1 | 4 top-level images (multi-image file) |
