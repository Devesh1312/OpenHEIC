//! Test harness for the property handler (Explorer Details pane metadata).
//!
//!   props_probe inproc <file.heic>   call heic_wic's handler directly (no registration needed)
//!   props_probe system <file.heic>   ask Windows for the file's properties (after install)
//!
//! Prints every property as Explorer would display it, and checks the key ones.

use std::ffi::c_void;

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::EnhancedStorage::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::PropertiesSystem::*;
use windows::Win32::UI::Shell::SHCreateStreamOnFileEx;
use windows_core::{Interface, HSTRING};

fn main() -> windows_core::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (mode, file) = (args[1].as_str(), args[2].as_str());
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };

    let store: IPropertyStore = unsafe {
        match mode {
            "inproc" => {
                let mut cf: *mut c_void = std::ptr::null_mut();
                heic_wic::DllGetClassObject(&heic_wic::CLSID_HEIC_PROPERTY_HANDLER, &IClassFactory::IID, &mut cf)
                    .ok()?;
                let cf = IClassFactory::from_raw(cf);
                let init: IInitializeWithStream = cf.CreateInstance(None)?;
                let stream = SHCreateStreamOnFileEx(&HSTRING::from(file), STGM_READ.0, 0, false, None)?;
                init.Initialize(&stream, STGM_READ.0)?;
                // A second Initialize must be refused.
                assert!(init.Initialize(&stream, STGM_READ.0).is_err(), "double Initialize accepted");
                let s: IPropertyStore = init.cast()?;
                assert!(s.SetValue(&PKEY_Photo_CameraModel, &"x".into()).is_err(), "store must be read-only");
                s
            }
            "system" => {
                let full = std::fs::canonicalize(file).unwrap();
                let full = full.to_string_lossy().trim_start_matches(r"\\?\").to_string();
                SHGetPropertyStoreFromParsingName(&HSTRING::from(full), None, GPS_DEFAULT)?
            }
            _ => panic!("mode must be inproc or system"),
        }
    };

    unsafe {
        let n = store.GetCount()?;
        let mut found = std::collections::HashMap::new();
        for i in 0..n {
            let mut key = PROPERTYKEY::default();
            store.GetAt(i, &mut key)?;
            let value = store.GetValue(&key)?;
            let name = PSGetNameFromPropertyKey(&key).map(|p| p.to_string().unwrap_or_default()).unwrap_or_default();
            let shown = PSFormatForDisplayAlloc(&key, &value, PDFF_DEFAULT)
                .map(|p| p.to_string().unwrap_or_default())
                .unwrap_or_else(|_| format!("{value:?}"));
            if name.starts_with("System.Photo") || name.starts_with("System.GPS") || name.starts_with("System.Image")
                || name == "System.ApplicationName"
            {
                println!("{name:<36} {shown}");
            }
            found.insert(name, shown);
        }
        for must in ["System.Photo.DateTaken", "System.Photo.CameraModel", "System.Image.Dimensions"] {
            assert!(found.contains_key(must), "{must} missing");
        }
    }
    println!("OK");
    Ok(())
}
