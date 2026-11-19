//! COM / WIC / shell registration written by `DllRegisterServer`
//! (invoked via `regsvr32 heic_wic.dll`, which needs admin rights).

use windows::Win32::Foundation::{E_ACCESSDENIED, E_FAIL};
use windows::Win32::Graphics::Imaging::{
    CATID_WICBitmapDecoders, GUID_ContainerFormatHeif, GUID_WICPixelFormat24bppBGR,
    GUID_WICPixelFormat32bppBGRA,
};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
use windows_core::GUID;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};
use winreg::{RegKey, RegValue};

use crate::{dll_instance, CLSID_HEIC_DECODER, CLSID_HEIC_PROPERTY_HANDLER, GUID_VENDOR};

const FRIENDLY_NAME: &str = "OpenHEIC HEIC Decoder (libheif)";
const PROPERTY_HANDLER_NAME: &str = "OpenHEIC HEIC Property Handler";
const PROPERTY_HANDLERS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\PropertySystem\PropertyHandlers";
/// Where we remember the handler that was registered before ours, to restore it on uninstall.
const PREVIOUS_HANDLER: &str = "OpenHEICPrevious";
const EXTENSIONS: [&str; 3] = [".heic", ".heif", ".hif"];
/// Brands accepted at offset 8 after "ftyp" at offset 4.
const BRANDS: [&[u8; 4]; 8] = [b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"mif1", b"msf1"];
/// Explorer's built-in WIC-based thumbnail provider and the thumbnail ShellEx slot.
const THUMB_SHELLEX: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
const WIC_THUMBNAIL_PROVIDER: &str = "{C7657C4A-9F68-40fa-A4DF-96BC08EB3551}";
/// Lets the classic "Windows Photo Viewer" appear in Open-with for .heic.
const PHOTO_VIEWER_ASSOC: &str = r"SOFTWARE\Microsoft\Windows Photo Viewer\Capabilities\FileAssociations";
const OUR_MARKER: &str = "OpenHEICRegistered";
/// Marker written by pre-release builds (named HeicTools); still recognised on uninstall.
const LEGACY_MARKER: &str = "HeicToolsRegistered";

fn g(guid: &GUID) -> String {
    format!("{{{guid:?}}}")
}

fn err(e: std::io::Error) -> windows_core::Error {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        E_ACCESSDENIED.into()
    } else {
        windows_core::Error::new(E_FAIL, e.to_string())
    }
}

fn dll_path() -> windows_core::Result<String> {
    let mut buf = vec![0u16; 32768];
    let n = unsafe { GetModuleFileNameW(Some(dll_instance().into()), &mut buf) } as usize;
    if n == 0 {
        return Err(windows_core::Error::from_thread());
    }
    Ok(String::from_utf16_lossy(&buf[..n]))
}

fn classes() -> std::io::Result<RegKey> {
    RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(r"SOFTWARE\Classes", KEY_ALL_ACCESS)
}

pub fn register() -> windows_core::Result<()> {
    let dll = dll_path()?;
    register_inner(&dll).map_err(err)?;
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    Ok(())
}

fn register_inner(dll: &str) -> std::io::Result<()> {
    let classes = classes()?;
    let clsid = g(&CLSID_HEIC_DECODER);

    // --- COM class -----------------------------------------------------------
    let (k, _) = classes.create_subkey(format!(r"CLSID\{clsid}"))?;
    k.set_value("", &FRIENDLY_NAME)?;
    k.set_value("FriendlyName", &FRIENDLY_NAME)?;
    k.set_value("Author", &"OpenHEIC contributors")?;
    k.set_value("Description", &"HEIC/HEIF image decoder")?;
    k.set_value("Vendor", &g(&GUID_VENDOR))?;
    k.set_value("Version", &"1.0.0.0")?;
    k.set_value("SpecVersion", &"1.0.0.0")?;
    k.set_value("ContainerFormat", &g(&GUID_ContainerFormatHeif))?;
    k.set_value("MimeTypes", &"image/heic,image/heif")?;
    k.set_value("FileExtensions", &EXTENSIONS.join(","))?;
    for v in ["SupportAnimation", "SupportChromakey", "SupportLossless", "SupportMultiframe"] {
        k.set_value(v, &0u32)?;
    }
    let (inproc, _) = k.create_subkey("InprocServer32")?;
    inproc.set_value("", &dll)?;
    inproc.set_value("ThreadingModel", &"Both")?;

    let (formats, _) = k.create_subkey("Formats")?;
    for f in [GUID_WICPixelFormat24bppBGR, GUID_WICPixelFormat32bppBGRA] {
        formats.create_subkey(g(&f))?;
    }

    let _ = k.delete_subkey_all("Patterns");
    let (patterns, _) = k.create_subkey("Patterns")?;
    for (i, brand) in BRANDS.iter().enumerate() {
        let (p, _) = patterns.create_subkey(i.to_string())?;
        let mut pattern = b"ftyp".to_vec();
        pattern.extend_from_slice(*brand);
        p.set_value("Position", &4u32)?;
        p.set_value("Length", &(pattern.len() as u32))?;
        p.set_raw_value("Pattern", &RegValue { bytes: pattern.into(), vtype: winreg::enums::REG_BINARY })?;
        p.set_raw_value("Mask", &RegValue { bytes: vec![0xFFu8; 8].into(), vtype: winreg::enums::REG_BINARY })?;
    }

    // --- WIC decoder category instance --------------------------------------
    let (inst, _) = classes.create_subkey(format!(r"CLSID\{}\Instance\{clsid}", g(&CATID_WICBitmapDecoders)))?;
    inst.set_value("CLSID", &clsid)?;
    inst.set_value("FriendlyName", &FRIENDLY_NAME)?;

    // --- File types: perceived type, MIME, Explorer thumbnails ----------------
    for ext in EXTENSIONS {
        let (e, _) = classes.create_subkey(ext)?;
        let mime = if ext == ".heic" { "image/heic" } else { "image/heif" };
        if e.get_value::<String, _>("Content Type").is_err() {
            e.set_value("Content Type", &mime)?;
        }
        if e.get_value::<String, _>("PerceivedType").is_err() {
            e.set_value("PerceivedType", &"image")?;
        }
        e.set_value(OUR_MARKER, &1u32)?;
        let (t, _) = e.create_subkey(format!(r"ShellEx\{THUMB_SHELLEX}"))?;
        t.set_value("", &WIC_THUMBNAIL_PROVIDER)?;
        // Offer Windows Photo Viewer in "Open with".
        let (owp, _) = e.create_subkey("OpenWithProgids")?;
        owp.set_raw_value("PhotoViewer.FileAssoc.Tiff", &RegValue { bytes: Vec::<u8>::new().into(), vtype: winreg::enums::REG_NONE })?;
    }

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(pv) = hklm.open_subkey_with_flags(PHOTO_VIEWER_ASSOC, KEY_ALL_ACCESS) {
        for ext in EXTENSIONS {
            pv.set_value(ext, &"PhotoViewer.FileAssoc.Tiff")?;
        }
    }
    let (kind, _) = hklm.create_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\KindMap")?;
    for ext in EXTENSIONS {
        kind.set_value(ext, &"picture")?;
    }

    register_property_handler(&classes, dll)
}

/// Explorer Details pane / "Date taken" / search: our IPropertyStore replaces
/// the generic handler for HEIC (it cannot see HEIF EXIF). The previous
/// handler is remembered and restored by `unregister`.
fn register_property_handler(classes: &RegKey, dll: &str) -> std::io::Result<()> {
    let ph = g(&CLSID_HEIC_PROPERTY_HANDLER);
    let (k, _) = classes.create_subkey(format!(r"CLSID\{ph}"))?;
    k.set_value("", &PROPERTY_HANDLER_NAME)?;
    let (inproc, _) = k.create_subkey("InprocServer32")?;
    inproc.set_value("", &dll)?;
    inproc.set_value("ThreadingModel", &"Both")?;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for ext in EXTENSIONS {
        let (h, _) = hklm.create_subkey(format!(r"{PROPERTY_HANDLERS}\{ext}"))?;
        let current: Option<String> = h.get_value("").ok();
        let is_ours = current.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(&ph));
        if !is_ours {
            if let Some(prev) = current.filter(|c| !c.is_empty()) {
                if h.get_value::<String, _>(PREVIOUS_HANDLER).is_err() {
                    h.set_value(PREVIOUS_HANDLER, &prev)?;
                }
            }
            h.set_value("", &ph)?;
        }
    }
    Ok(())
}

fn unregister_property_handler(classes: &RegKey) -> std::io::Result<()> {
    let ph = g(&CLSID_HEIC_PROPERTY_HANDLER);
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for ext in EXTENSIONS {
        let path = format!(r"{PROPERTY_HANDLERS}\{ext}");
        let Ok(h) = hklm.open_subkey_with_flags(&path, KEY_ALL_ACCESS) else { continue };
        let current: Option<String> = h.get_value("").ok();
        let previous: Option<String> = h.get_value(PREVIOUS_HANDLER).ok();
        if current.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(&ph)) {
            match &previous {
                Some(prev) => h.set_value("", prev)?,
                None => {
                    drop(h);
                    ignore_missing(hklm.delete_subkey_all(&path))?;
                    continue;
                }
            }
        }
        let _ = h.delete_value(PREVIOUS_HANDLER);
    }
    ignore_missing(classes.delete_subkey_all(format!(r"CLSID\{ph}")))
}

pub fn unregister() -> windows_core::Result<()> {
    unregister_inner().map_err(err)?;
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    Ok(())
}

fn ignore_missing(r: std::io::Result<()>) -> std::io::Result<()> {
    match r {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn unregister_inner() -> std::io::Result<()> {
    let classes = classes()?;
    let clsid = g(&CLSID_HEIC_DECODER);
    ignore_missing(classes.delete_subkey_all(format!(r"CLSID\{clsid}")))?;
    ignore_missing(classes.delete_subkey_all(format!(
        r"CLSID\{}\Instance\{clsid}",
        g(&CATID_WICBitmapDecoders)
    )))?;

    for ext in EXTENSIONS {
        let Ok(e) = classes.open_subkey_with_flags(ext, KEY_ALL_ACCESS) else { continue };
        if e.get_value::<u32, _>(OUR_MARKER).is_err() && e.get_value::<u32, _>(LEGACY_MARKER).is_err() {
            continue; // not ours; leave untouched
        }
        if let Ok(t) = e.open_subkey(format!(r"ShellEx\{THUMB_SHELLEX}")) {
            if t.get_value::<String, _>("").is_ok_and(|v| v.eq_ignore_ascii_case(WIC_THUMBNAIL_PROVIDER)) {
                drop(t);
                ignore_missing(e.delete_subkey_all(format!(r"ShellEx\{THUMB_SHELLEX}")))?;
                let _ = e.delete_subkey("ShellEx"); // only succeeds if now empty
            }
        }
        if let Ok(owp) = e.open_subkey_with_flags("OpenWithProgids", KEY_ALL_ACCESS) {
            let _ = owp.delete_value("PhotoViewer.FileAssoc.Tiff");
            drop(owp);
            let _ = e.delete_subkey("OpenWithProgids");
        }
        let _ = e.delete_value(OUR_MARKER);
        let _ = e.delete_value(LEGACY_MARKER);
    }

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(pv) = hklm.open_subkey_with_flags(PHOTO_VIEWER_ASSOC, KEY_ALL_ACCESS) {
        for ext in EXTENSIONS {
            let _ = pv.delete_value(ext);
        }
    }
    unregister_property_handler(&classes)
}
