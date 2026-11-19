//! libheif-sys links heif.lib and libde265.lib, but its build script does not
//! re-run when the vcpkg install changes, and with the dav1d (AV1) feature
//! heif.lib also needs dav1d.lib. Link it explicitly from the same vcpkg tree.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
    println!("cargo:rerun-if-env-changed=VCPKGRS_TRIPLET");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Ok(root) = std::env::var("VCPKG_ROOT") else { return };
    let triplet = std::env::var("VCPKGRS_TRIPLET").unwrap_or_else(|_| "x64-windows-static".into());
    let lib_dir = PathBuf::from(root).join("installed").join(&triplet).join("lib");
    if !lib_dir.join("dav1d.lib").exists() {
        panic!(
            "dav1d.lib not found in {}. Run scripts/setup-deps.ps1 to build libheif with AV1 support.",
            lib_dir.display()
        );
    }
    println!("cargo:rerun-if-changed={}", lib_dir.join("dav1d.lib").display());
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=dav1d");
}
