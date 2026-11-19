//! Windows property handler for HEIC/HEIF: feeds Explorer's Details pane,
//! columns ("Date taken", "Camera model"...), sorting/grouping and Windows
//! Search from the file's EXIF. Read-only.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use windows::Win32::Foundation::{
    E_FAIL, E_POINTER, E_UNEXPECTED, FILETIME, PROPERTYKEY, S_FALSE, STG_E_ACCESSDENIED, SYSTEMTIME,
};
use windows::Win32::Storage::EnhancedStorage::*;
use windows::Win32::System::Com::IStream;
use windows::Win32::System::Com::StructuredStorage::{
    InitPropVariantFromDoubleVector, InitPropVariantFromFileTime, PROPVARIANT,
};
use windows::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};
use windows::Win32::UI::Shell::PropertiesSystem::{
    IInitializeWithStream, IInitializeWithStream_Impl, IPropertyStore, IPropertyStoreCapabilities,
    IPropertyStoreCapabilities_Impl, IPropertyStore_Impl, PSCoerceToCanonicalValue, PSCreateMemoryPropertyStore,
};
use windows_core::{implement, Interface, HRESULT, Ref};

use crate::LIVE_OBJECTS;

/// HRESULT_FROM_WIN32(ERROR_ALREADY_INITIALIZED)
const E_ALREADY_INITIALIZED: HRESULT = HRESULT(0x800704DF_u32 as i32);

#[implement(IInitializeWithStream, IPropertyStore, IPropertyStoreCapabilities)]
pub struct PropertyHandler {
    store: Mutex<Option<IPropertyStore>>,
}

impl PropertyHandler {
    pub fn new() -> Self {
        LIVE_OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self { store: Mutex::new(None) }
    }

    fn store(&self) -> windows_core::Result<IPropertyStore> {
        self.store
            .lock()
            .map_err(|_| windows_core::Error::from(E_FAIL))?
            .clone()
            .ok_or_else(|| E_UNEXPECTED.into())
    }
}

impl Drop for PropertyHandler {
    fn drop(&mut self) {
        LIVE_OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IInitializeWithStream_Impl for PropertyHandler_Impl {
    fn Initialize(&self, stream: Ref<IStream>, _mode: u32) -> windows_core::Result<()> {
        let stream = stream.ok()?;
        let mut guard = self.store.lock().map_err(|_| windows_core::Error::from(E_FAIL))?;
        if guard.is_some() {
            return Err(E_ALREADY_INITIALIZED.into());
        }
        let bytes = crate::decoder::read_all(stream)?;
        let info = catch_unwind(AssertUnwindSafe(|| heic_core::read_photo_info(&bytes)))
            .map_err(|_| windows_core::Error::from(E_UNEXPECTED))?
            .map_err(|_| windows_core::Error::from(E_FAIL))?;
        *guard = Some(build_store(&info)?);
        Ok(())
    }
}

impl IPropertyStore_Impl for PropertyHandler_Impl {
    fn GetCount(&self) -> windows_core::Result<u32> {
        unsafe { self.store()?.GetCount() }
    }

    fn GetAt(&self, index: u32, key: *mut PROPERTYKEY) -> windows_core::Result<()> {
        if key.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { self.store()?.GetAt(index, key) }
    }

    fn GetValue(&self, key: *const PROPERTYKEY) -> windows_core::Result<PROPVARIANT> {
        if key.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { self.store()?.GetValue(key) }
    }

    fn SetValue(&self, _key: *const PROPERTYKEY, _value: *const PROPVARIANT) -> windows_core::Result<()> {
        Err(STG_E_ACCESSDENIED.into())
    }

    fn Commit(&self) -> windows_core::Result<()> {
        Err(STG_E_ACCESSDENIED.into())
    }
}

impl IPropertyStoreCapabilities_Impl for PropertyHandler_Impl {
    fn IsPropertyWritable(&self, _key: *const PROPERTYKEY) -> HRESULT {
        S_FALSE
    }
}

// ---------------------------------------------------------------------------

/// Converts an EXIF wall-clock time (camera local time) to UTC the same way
/// Windows' JPEG handler does: interpret it in this PC's time zone. A HEIC and
/// its converted JPEG therefore show the same "Date taken".
fn exif_time_to_filetime(dt: &heic_core::ExifDateTime) -> Option<FILETIME> {
    let local = SYSTEMTIME {
        wYear: dt.year,
        wMonth: dt.month,
        wDayOfWeek: 0,
        wDay: dt.day,
        wHour: dt.hour,
        wMinute: dt.minute,
        wSecond: dt.second.min(59),
        wMilliseconds: 0,
    };
    let mut utc = SYSTEMTIME::default();
    let mut ft = FILETIME::default();
    unsafe {
        TzSpecificLocalTimeToSystemTime(None, &local, &mut utc).ok()?;
        SystemTimeToFileTime(&utc, &mut ft).ok()?;
    }
    Some(ft)
}

struct Builder {
    store: IPropertyStore,
}

impl Builder {
    /// Stores `value` after coercing it to the property's canonical type;
    /// values the schema rejects are skipped rather than failing the file.
    fn set(&self, key: &PROPERTYKEY, mut value: PROPVARIANT) {
        unsafe {
            if PSCoerceToCanonicalValue(key, &mut value).is_ok() {
                let _ = self.store.SetValue(key, &value);
            }
        }
    }
    fn str(&self, key: &PROPERTYKEY, v: &Option<String>) {
        if let Some(s) = v {
            self.set(key, PROPVARIANT::from(s.as_str()));
        }
    }
    fn f64(&self, key: &PROPERTYKEY, v: Option<f64>) {
        if let Some(x) = v.filter(|x| x.is_finite()) {
            self.set(key, PROPVARIANT::from(x));
        }
    }
    fn u16(&self, key: &PROPERTYKEY, v: Option<u16>) {
        if let Some(x) = v {
            self.set(key, PROPVARIANT::from(x));
        }
    }
    fn u32(&self, key: &PROPERTYKEY, v: Option<u32>) {
        if let Some(x) = v {
            self.set(key, PROPVARIANT::from(x));
        }
    }
    fn dms(&self, key: &PROPERTYKEY, v: Option<[f64; 3]>) {
        if let Some(d) = v {
            if let Ok(pv) = unsafe { InitPropVariantFromDoubleVector(Some(&d)) } {
                self.set(key, pv);
            }
        }
    }
}

fn build_store(info: &heic_core::PhotoInfo) -> windows_core::Result<IPropertyStore> {
    let store: IPropertyStore = unsafe {
        let mut p = std::ptr::null_mut();
        PSCreateMemoryPropertyStore(&IPropertyStore::IID, &mut p)?;
        IPropertyStore::from_raw(p)
    };
    let b = Builder { store: store.clone() };

    b.u32(&PKEY_Image_HorizontalSize, Some(info.width));
    b.u32(&PKEY_Image_VerticalSize, Some(info.height));
    b.str(&PKEY_Image_Dimensions, &Some(format!("{} x {}", info.width, info.height)));
    let channels = if info.has_alpha { 4 } else { 3 };
    b.u32(&PKEY_Image_BitDepth, Some(info.bit_depth as u32 * channels));
    // Pixels are delivered upright by our codec, so orientation is always "normal".
    b.u16(&PKEY_Photo_Orientation, Some(1));

    if let Some(e) = &info.exif {
        if let Some(ft) = e.date_taken.as_ref().and_then(exif_time_to_filetime) {
            if let Ok(pv) = unsafe { InitPropVariantFromFileTime(&ft) } {
                b.set(&PKEY_Photo_DateTaken, pv);
            }
        }
        b.str(&PKEY_Photo_CameraManufacturer, &e.make);
        b.str(&PKEY_Photo_CameraModel, &e.model);
        b.str(&PKEY_Photo_LensManufacturer, &e.lens_make);
        b.str(&PKEY_Photo_LensModel, &e.lens_model);
        b.str(&PKEY_ApplicationName, &e.software);
        b.f64(&PKEY_Photo_ExposureTime, e.exposure_time);
        b.f64(&PKEY_Photo_FNumber, e.f_number);
        b.u16(&PKEY_Photo_ISOSpeed, e.iso.map(|v| v.min(u16::MAX as u32) as u16));
        b.f64(&PKEY_Photo_ExposureBias, e.exposure_bias);
        b.f64(&PKEY_Photo_FocalLength, e.focal_length);
        b.u16(&PKEY_Photo_FocalLengthInFilm, e.focal_length_35mm);
        if let Some(f) = e.flash {
            b.set(&PKEY_Photo_Flash, PROPVARIANT::from((f & 0xFF) as u8));
        }
        b.u16(&PKEY_Photo_MeteringMode, e.metering_mode);
        b.u32(&PKEY_Photo_WhiteBalance, e.white_balance.map(u32::from));
        b.u32(&PKEY_Photo_ExposureProgram, e.exposure_program.map(u32::from));

        if let Some(g) = &e.gps {
            b.dms(&PKEY_GPS_Latitude, g.latitude);
            b.str(&PKEY_GPS_LatitudeRef, &g.latitude_ref);
            b.dms(&PKEY_GPS_Longitude, g.longitude);
            b.str(&PKEY_GPS_LongitudeRef, &g.longitude_ref);
            b.f64(&PKEY_GPS_LatitudeDecimal, g.latitude_decimal());
            b.f64(&PKEY_GPS_LongitudeDecimal, g.longitude_decimal());
            b.f64(&PKEY_GPS_Altitude, g.altitude);
            if let Some(r) = g.altitude_ref {
                b.set(&PKEY_GPS_AltitudeRef, PROPVARIANT::from(r));
            }
        }
    }
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::StructuredStorage::{PropVariantGetElementCount, PropVariantToDoubleVector};

    #[test]
    fn gps_and_date_reach_the_store() {
        let info = heic_core::PhotoInfo {
            width: 4032,
            height: 3024,
            bit_depth: 8,
            has_alpha: false,
            image_count: 1,
            exif: Some(heic_core::ExifSummary {
                model: Some("iPhone 15".into()),
                date_taken: Some(heic_core::ExifDateTime { year: 2024, month: 7, day: 15, hour: 18, minute: 30, second: 5 }),
                gps: Some(heic_core::Gps {
                    latitude: Some([12.0, 30.0, 0.0]),
                    latitude_ref: Some("S".into()),
                    longitude: Some([77.0, 15.0, 36.0]),
                    longitude_ref: Some("E".into()),
                    altitude: Some(920.5),
                    altitude_ref: Some(0),
                }),
                ..Default::default()
            }),
        };
        let s = build_store(&info).unwrap();
        unsafe {
            let lat = s.GetValue(&PKEY_GPS_Latitude).unwrap();
            assert_eq!(PropVariantGetElementCount(&lat), 3);
            let mut v = [0f64; 3];
            let mut got = 0u32;
            PropVariantToDoubleVector(&lat, &mut v, &mut got).unwrap();
            assert_eq!(got, 3);
            assert_eq!(v, [12.0, 30.0, 0.0]);
            let dec = f64::try_from(&s.GetValue(&PKEY_GPS_LatitudeDecimal).unwrap()).unwrap_or(f64::NAN);
            assert!((dec + 12.5).abs() < 1e-9, "southern latitude must be negative, got {dec}");
            assert!(!s.GetValue(&PKEY_Photo_DateTaken).unwrap().is_empty());
            assert!(!s.GetValue(&PKEY_GPS_Altitude).unwrap().is_empty());
        }
    }
}
