//! End-to-end tests of the heic-convert binary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../heic-core/tests/data")
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("openheic-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(args: &[&str], cwd: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_heic-convert")).args(args).current_dir(cwd).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
}

fn jpgs(dir: &Path) -> Vec<String> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".jpg"))
        .collect();
    v.sort();
    v
}

#[test]
fn multi_image_default_and_all_images() {
    let dir = tmp("multi");
    std::fs::copy(data_dir().join("collection.heif"), dir.join("burst.heif")).unwrap();

    let (ok, out) = run(&["convert", "burst.heif"], &dir);
    assert!(ok, "{out}");
    assert_eq!(jpgs(&dir), ["burst.jpg"]);
    assert!(out.contains("3 extra image(s)"), "should mention extra images: {out}");

    let (ok, out) = run(&["convert", "burst.heif", "--all-images"], &dir);
    assert!(ok, "{out}");
    assert_eq!(jpgs(&dir), ["burst-2.jpg", "burst-3.jpg", "burst-4.jpg", "burst.jpg"]);
    assert!(out.contains("3 converted, 1 skipped"), "main image already existed: {out}");

    // Every output is a real, decodable JPEG.
    for f in jpgs(&dir) {
        let b = std::fs::read(dir.join(&f)).unwrap();
        assert!(b.starts_with(&[0xFF, 0xD8]) && b.ends_with(&[0xFF, 0xD9]), "{f}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn whole_fixture_folder_converts() {
    let dir = tmp("all");
    let (ok, out) = run(&["convert", data_dir().to_str().unwrap(), "--out", dir.to_str().unwrap()], &dir);
    assert!(ok, "{out}");
    let names = jpgs(&dir);
    assert_eq!(names.len(), 6, "{out}");
    assert!(names.contains(&"test.jpg".to_string()) && names.contains(&"test-heif.jpg".to_string()), "{names:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
