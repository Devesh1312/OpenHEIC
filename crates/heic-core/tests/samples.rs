//! End-to-end decode -> JPEG tests.
//!
//! Always runs on the redistributable fixtures in `tests/data`. If a `sample/`
//! folder exists at the repository root (it is git-ignored: put your own
//! photos there), those files are tested too.

use std::path::{Path, PathBuf};

use heic_core::{decode_file, encode_jpeg, exif_orientation, DecodeOptions, JpegOptions};

fn heif_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("heic") || e.eq_ignore_ascii_case("heif"))
        })
        .collect();
    v.sort();
    v
}

fn fixtures() -> Vec<PathBuf> {
    heif_files(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data"))
}

fn local_samples() -> Vec<PathBuf> {
    heif_files(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sample"))
}

/// APP segments of a JPEG as (marker, payload).
fn jpeg_segments(j: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = vec![];
    let mut i = 2;
    while i + 4 <= j.len() && j[i] == 0xFF {
        let m = j[i + 1];
        if m == 0xDA {
            break;
        }
        let len = u16::from_be_bytes([j[i + 2], j[i + 3]]) as usize;
        out.push((m, &j[i + 4..i + 2 + len]));
        i += 2 + len;
    }
    out
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse: f64 = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum::<f64>()
        / a.len() as f64;
    if mse == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

fn check(path: &Path, full_pixel_check: bool) {
    let img = decode_file(path, DecodeOptions::default())
        .unwrap_or_else(|e| panic!("{path:?}: decode failed: {e}"));
    assert!(img.width > 0 && img.height > 0);
    assert_eq!(img.pixels.len(), (img.width * img.height * 3) as usize);

    let mut jpg = Vec::new();
    encode_jpeg(&img, &mut jpg, JpegOptions::default()).unwrap();
    let segs = jpeg_segments(&jpg);

    // Metadata present in the source must survive, with orientation normalised.
    let exif = segs.iter().find(|(m, p)| *m == 0xE1 && p.starts_with(b"Exif\0\0")).map(|(_, p)| &p[6..]);
    assert_eq!(exif.is_some(), img.exif.is_some(), "{path:?}: EXIF presence");
    if let Some(exif) = exif {
        assert_eq!(exif_orientation(exif).unwrap_or(1), 1, "{path:?}: orientation not normalised");
    }
    let has_icc = segs.iter().any(|(m, p)| *m == 0xE2 && p.starts_with(b"ICC_PROFILE\0"));
    assert_eq!(has_icc, img.icc.is_some(), "{path:?}: ICC presence");
    let has_xmp = segs.iter().any(|(m, p)| *m == 0xE1 && p.starts_with(b"http://ns.adobe.com/xap/1.0/"));
    assert_eq!(has_xmp, img.xmp.is_some(), "{path:?}: XMP presence");

    if full_pixel_check {
        let dec = image::load_from_memory_with_format(&jpg, image::ImageFormat::Jpeg).unwrap().to_rgb8();
        assert_eq!((dec.width(), dec.height()), (img.width, img.height));
        let p = psnr(&img.pixels, dec.as_raw());
        println!(
            "{}: {}x{} {}-bit, PSNR {p:.2} dB, icc={} exif={} xmp={}",
            path.file_name().unwrap().to_string_lossy(),
            img.width,
            img.height,
            img.source_bit_depth,
            img.icc.is_some(),
            img.exif.is_some(),
            img.xmp.is_some()
        );
        assert!(p >= 45.0, "{path:?}: PSNR {p:.2} dB too low");
    }
}

#[test]
fn fixtures_convert_faithfully() {
    let files = fixtures();
    assert!(files.len() >= 3, "missing test fixtures in tests/data");
    for f in &files {
        check(f, true);
    }
}

#[test]
fn exif_fixture_keeps_exif() {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/test.heic");
    let img = decode_file(&p, DecodeOptions::default()).unwrap();
    assert!(img.exif.is_some());
}

#[test]
fn alpha_is_kept_when_requested() {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/alpha.heif");
    let img = decode_file(&p, DecodeOptions { keep_alpha: true, skip_metadata: true }).unwrap();
    assert_eq!(img.channels, 4);
}

#[test]
fn local_samples_if_present() {
    let files = local_samples();
    if files.is_empty() {
        eprintln!("no ./sample folder; skipping local sample test");
        return;
    }
    for (i, f) in files.iter().enumerate() {
        check(f, i % 8 == 0);
    }
}
