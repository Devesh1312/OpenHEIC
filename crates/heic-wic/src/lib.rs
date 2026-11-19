//! `heic_wic.dll`: a Windows Imaging Component (WIC) decoder for HEIC/HEIF,
//! backed by libheif + libde265 (statically linked). Once registered, every
//! WIC-based app (Windows Photo Viewer, Photos, Explorer thumbnails, Paint...)
//! can open .heic files without the paid HEVC Video Extension.

#![allow(non_snake_case)]
#![allow(linker_messages)]

mod decoder;
mod props;
mod register;

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, HINSTANCE, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_core::{implement, Interface, BOOL, GUID, HRESULT, IUnknown, Ref};

/// CLSID of our decoder.
pub const CLSID_HEIC_DECODER: GUID = GUID::from_u128(0x1f6d83f1_2506_4649_9fae_261e49e54f9e);
/// CLSID of our property handler (Explorer Details / "Date taken" / search).
pub const CLSID_HEIC_PROPERTY_HANDLER: GUID = GUID::from_u128(0x36944290_c825_49de_9b94_ad0fe94defd2);
/// Vendor GUID written to the registry.
pub const GUID_VENDOR: GUID = GUID::from_u128(0xd315746a_4961_4fc4_894a_74ddf6d81ad3);

static DLL_INSTANCE: AtomicIsize = AtomicIsize::new(0);
pub(crate) static LIVE_OBJECTS: AtomicUsize = AtomicUsize::new(0);
static LOCKS: AtomicUsize = AtomicUsize::new(0);

#[unsafe(no_mangle)]
extern "system" fn DllMain(hinst: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        DLL_INSTANCE.store(hinst.0 as isize, Ordering::SeqCst);
    }
    true.into()
}

pub(crate) fn dll_instance() -> HINSTANCE {
    HINSTANCE(DLL_INSTANCE.load(Ordering::SeqCst) as *mut c_void)
}

#[derive(Clone, Copy)]
enum Kind {
    Decoder,
    PropertyHandler,
}

#[implement(IClassFactory)]
struct ClassFactory(Kind);

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> windows_core::Result<()> {
        if ppv.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *ppv = std::ptr::null_mut() };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let unk: IUnknown = match self.0 {
            Kind::Decoder => decoder::Decoder::new().into(),
            Kind::PropertyHandler => props::PropertyHandler::new().into(),
        };
        unsafe { unk.query(riid, ppv).ok() }
    }

    fn LockServer(&self, lock: BOOL) -> windows_core::Result<()> {
        if lock.as_bool() {
            LOCKS.fetch_add(1, Ordering::SeqCst);
        } else {
            LOCKS.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if rclsid.is_null() || riid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    unsafe {
        *ppv = std::ptr::null_mut();
        let kind = if *rclsid == CLSID_HEIC_DECODER {
            Kind::Decoder
        } else if *rclsid == CLSID_HEIC_PROPERTY_HANDLER {
            Kind::PropertyHandler
        } else {
            return CLASS_E_CLASSNOTAVAILABLE;
        };
        let factory: IClassFactory = ClassFactory(kind).into();
        factory.query(riid, ppv)
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    if LIVE_OBJECTS.load(Ordering::SeqCst) == 0 && LOCKS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllRegisterServer() -> HRESULT {
    match std::panic::catch_unwind(register::register) {
        Ok(Ok(())) => S_OK,
        Ok(Err(e)) => e.code(),
        Err(_) => windows::Win32::Foundation::E_UNEXPECTED,
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllUnregisterServer() -> HRESULT {
    match std::panic::catch_unwind(register::unregister) {
        Ok(Ok(())) => S_OK,
        Ok(Err(e)) => e.code(),
        Err(_) => windows::Win32::Foundation::E_UNEXPECTED,
    }
}
