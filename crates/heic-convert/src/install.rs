//! `install` / `uninstall`: copies binaries to Program Files, registers the WIC
//! codec (via regsvr32 -> DllRegisterServer) and the Explorer context menus.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};
use winreg::RegKey;

const APP_DIR_NAME: &str = "OpenHEIC";
const DLL_NAME: &str = "heic_wic.dll";
const EXE_NAME: &str = "heic-convert.exe";
const MENU_KEY: &str = "HeicToJpeg";
const UNINSTALL_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\OpenHEIC";
const EXTENSIONS: [&str; 3] = [".heic", ".heif", ".hif"];

fn app_dir() -> PathBuf {
    let pf = std::env::var_os("ProgramW6432")
        .or_else(|| std::env::var_os("ProgramFiles"))
        .unwrap_or_else(|| r"C:\Program Files".into());
    PathBuf::from(pf).join(APP_DIR_NAME)
}

fn is_elevated() -> bool {
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SOFTWARE\Classes", KEY_ALL_ACCESS)
        .is_ok()
}

/// Re-runs this exe elevated (UAC prompt) with the given argument and waits.
fn relaunch_elevated(arg: &str) -> Result<()> {
    let exe = std::env::current_exe()?;
    println!("Administrator rights are required; requesting elevation...");
    let ps = format!(
        "$p = Start-Process -FilePath '{}' -ArgumentList '{} --elevated' -Verb RunAs -Wait -PassThru; exit $p.ExitCode",
        exe.display().to_string().replace('\'', "''"),
        arg
    );
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .status()
        .context("failed to start elevated process")?;
    if !status.success() {
        bail!("elevated {arg} failed or was cancelled (exit code {:?})", status.code());
    }
    println!("{arg} completed.");
    Ok(())
}

fn pause_if(elevated_child: bool) {
    if elevated_child {
        use std::io::Read;
        println!("\nPress Enter to close...");
        let _ = std::io::stdin().read(&mut [0u8]);
    }
}

pub fn install(elevated_child: bool) -> Result<()> {
    if !is_elevated() {
        return relaunch_elevated("install");
    }
    let r = install_inner();
    match &r {
        Ok(()) => println!("\nInstalled. Double-click a .heic file, or right-click a file/folder > \"Show more options\" > \"Convert to JPEG\"."),
        Err(e) => eprintln!("\nInstall failed: {e:#}"),
    }
    pause_if(elevated_child);
    r
}

pub fn uninstall(elevated_child: bool) -> Result<()> {
    if !is_elevated() {
        return relaunch_elevated("uninstall");
    }
    let r = uninstall_inner();
    match &r {
        Ok(()) => println!("\nUninstalled."),
        Err(e) => eprintln!("\nUninstall failed: {e:#}"),
    }
    pause_if(elevated_child);
    r
}

/// Copies `src` to `dst`. If `dst` is in use (DLL loaded by Explorer/Photos),
/// it is renamed out of the way first, which Windows permits for loaded images.
fn copy_replacing(src: &Path, dst: &Path) -> Result<()> {
    if same_file(src, dst) {
        return Ok(());
    }
    if std::fs::copy(src, dst).is_ok() {
        return Ok(());
    }
    if dst.exists() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let old = dst.with_extension(format!("old-{stamp}"));
        std::fs::rename(dst, &old).with_context(|| format!("cannot replace {}", dst.display()))?;
    }
    std::fs::copy(src, dst).with_context(|| format!("cannot copy to {}", dst.display()))?;
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

fn regsvr32(dll: &Path, unregister: bool) -> Result<()> {
    let mut cmd = Command::new("regsvr32.exe");
    cmd.arg("/s");
    if unregister {
        cmd.arg("/u");
    }
    let status = cmd.arg(dll).status().context("failed to run regsvr32")?;
    if !status.success() {
        bail!("regsvr32 {}{} failed (exit code {:?})", if unregister { "/u " } else { "" }, dll.display(), status.code());
    }
    Ok(())
}

fn install_inner() -> Result<()> {
    let src_dir = std::env::current_exe()?.parent().context("exe has no parent dir")?.to_path_buf();
    let src_dll = src_dir.join(DLL_NAME);
    if !src_dll.exists() {
        bail!("{} not found next to heic-convert.exe", src_dll.display());
    }
    let dir = app_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    // Clean up DLLs renamed during previous upgrades (ignore failures: may still be loaded).
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().contains(".old-") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let exe = dir.join(EXE_NAME);
    let dll = dir.join(DLL_NAME);
    copy_replacing(&std::env::current_exe()?, &exe)?;
    copy_replacing(&src_dll, &dll)?;
    println!("Copied files to {}", dir.display());

    regsvr32(&dll, false)?;
    println!("Registered WIC HEIC decoder (Photos, Photo Viewer, Explorer thumbnails).");

    register_menus(&exe)?;
    println!("Added Explorer context menu \"Convert to JPEG\".");

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let (u, _) = hklm.create_subkey(UNINSTALL_KEY)?;
    u.set_value("DisplayName", &"OpenHEIC (HEIC viewer codec & JPEG converter)")?;
    u.set_value("DisplayVersion", &env!("CARGO_PKG_VERSION"))?;
    u.set_value("Publisher", &"OpenHEIC contributors")?;
    u.set_value("URLInfoAbout", &"https://github.com/Devesh1312/OpenHEIC")?;
    u.set_value("InstallLocation", &dir.display().to_string())?;
    u.set_value("DisplayIcon", &format!("{},0", exe.display()))?;
    u.set_value("UninstallString", &format!("\"{}\" uninstall", exe.display()))?;
    u.set_value("NoModify", &1u32)?;
    u.set_value("NoRepair", &1u32)?;
    Ok(())
}

fn register_menus(exe: &Path) -> Result<()> {
    let classes = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(r"SOFTWARE\Classes", KEY_ALL_ACCESS)?;
    let exe = exe.display().to_string();
    let icon = "imageres.dll,-5205";
    let mut entries: Vec<(String, &str, &str)> = EXTENSIONS
        .iter()
        .map(|e| (format!(r"SystemFileAssociations\{e}\shell\{MENU_KEY}"), "Convert to JPEG", "%1"))
        .collect();
    entries.push((format!(r"Directory\shell\{MENU_KEY}"), "Convert HEIC images to JPEG", "%1"));
    entries.push((format!(r"Directory\Background\shell\{MENU_KEY}"), "Convert HEIC images here to JPEG", "%V"));
    for (path, label, arg) in entries {
        let (k, _) = classes.create_subkey(&path)?;
        k.set_value("", &label)?;
        k.set_value("Icon", &icon)?;
        if path.starts_with("SystemFileAssociations") {
            // One process per selected file is fine; Player model passes many files.
            k.set_value("MultiSelectModel", &"Player")?;
        }
        let (c, _) = k.create_subkey("command")?;
        c.set_value("", &format!("\"{exe}\" convert --pause \"{arg}\""))?;
    }
    Ok(())
}

fn uninstall_inner() -> Result<()> {
    let dir = app_dir();
    let dll = dir.join(DLL_NAME);
    if dll.exists() {
        regsvr32(&dll, true)?;
        println!("Unregistered WIC HEIC decoder.");
    }
    let classes = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(r"SOFTWARE\Classes", KEY_ALL_ACCESS)?;
    let mut keys: Vec<String> = EXTENSIONS
        .iter()
        .map(|e| format!(r"SystemFileAssociations\{e}\shell\{MENU_KEY}"))
        .collect();
    keys.push(format!(r"Directory\shell\{MENU_KEY}"));
    keys.push(format!(r"Directory\Background\shell\{MENU_KEY}"));
    for k in keys {
        let _ = classes.delete_subkey_all(&k);
    }
    println!("Removed Explorer context menus.");
    let _ = RegKey::predef(HKEY_LOCAL_MACHINE).delete_subkey_all(UNINSTALL_KEY);

    // Remove files. The running exe / a loaded DLL can't be deleted right now,
    // so hand the folder removal to a delayed background cmd.
    let running_from_dir = std::env::current_exe().map(|e| e.starts_with(&dir)).unwrap_or(false);
    let _ = std::fs::remove_file(&dll);
    if !running_from_dir {
        let _ = std::fs::remove_file(dir.join(EXE_NAME));
        let _ = std::fs::remove_dir(&dir);
    }
    if dir.exists() {
        let _ = Command::new("cmd.exe")
            .args(["/C", &format!("ping -n 3 127.0.0.1 >nul & rmdir /S /Q \"{}\"", dir.display())])
            .spawn();
        println!(
            "{} will be removed in a moment (if the DLL is still loaded by Explorer/Photos, delete it after sign-out).",
            dir.display()
        );
    }
    Ok(())
}
