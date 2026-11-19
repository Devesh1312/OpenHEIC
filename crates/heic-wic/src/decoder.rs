//! IWICBitmapDecoder / IWICBitmapFrameDecode implementation.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};

use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_POINTER, E_UNEXPECTED};
use windows::Win32::Foundation::{
    WINCODEC_ERR_CODECNOTHUMBNAIL, WINCODEC_ERR_NOTINITIALIZED, WINCODEC_ERR_PALETTEUNAVAILABLE,
    WINCODEC_ERR_UNSUPPORTEDOPERATION, WINCODEC_ERR_WRONGSTATE, WINCODEC_ERR_BADIMAGE,
};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{
    CoCreateInstance, IStream, CLSCTX_INPROC_SERVER, STREAM_SEEK_CUR,
};
use windows_core::{implement, Interface, GUID, Ref};

use crate::{CLSID_HEIC_DECODER, LIVE_OBJECTS};

/// Upright pixels in WIC byte order (BGR / BGRA), decoded lazily once.
struct Pixels {
    width: u32,
    height: u32,
    bpp: u32,
    data: Vec<u8>,
    icc: Option<Vec<u8>>,
}

struct Source {
    bytes: Vec<u8>,
    pixels: OnceLock<Result<Pixels, windows_core::HRESULT>>,
}

impl Source {
    fn pixels(&self) -> windows_core::Result<&Pixels> {
        let r = self.pixels.get_or_init(|| {
            catch_unwind(AssertUnwindSafe(|| decode(&self.bytes)))
                .unwrap_or(Err(E_UNEXPECTED))
        });
        r.as_ref().map_err(|e| (*e).into())
    }
}

fn decode(bytes: &[u8]) -> Result<Pixels, windows_core::HRESULT> {
    let opts = heic_core::DecodeOptions { keep_alpha: true, skip_metadata: true };
    let img = heic_core::decode_bytes(bytes, opts).map_err(|_| WINCODEC_ERR_BADIMAGE)?;
    // ICC is cheap to read separately; EXIF is intentionally not exposed so
    // viewers never re-apply an orientation that libheif already applied.
    let icc = icc_for(bytes);
    let mut data = img.pixels;
    let c = img.channels as usize;
    for px in data.chunks_exact_mut(c) {
        px.swap(0, 2); // RGB(A) -> BGR(A)
    }
    Ok(Pixels { width: img.width, height: img.height, bpp: c as u32, data, icc })
}

fn icc_for(bytes: &[u8]) -> Option<Vec<u8>> {
    let _lib = libheif_rs::LibHeif::new();
    let ctx = libheif_rs::HeifContext::read_from_bytes(bytes).ok()?;
    let h = ctx.primary_image_handle().ok()?;
    heic_core::icc_profile(&h)
}

fn read_all(stream: &IStream) -> windows_core::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let mut read = 0u32;
        let hr = unsafe { stream.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read)) };
        hr.ok()?;
        if read == 0 {
            break;
        }
        out.extend_from_slice(&buf[..read as usize]);
        if out.len() > 1 << 30 {
            return Err(WINCODEC_ERR_BADIMAGE.into());
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------

#[implement(IWICBitmapDecoder)]
pub struct Decoder {
    source: Mutex<Option<Arc<Source>>>,
}

impl Decoder {
    pub fn new() -> Self {
        LIVE_OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self { source: Mutex::new(None) }
    }

    fn source(&self) -> windows_core::Result<Arc<Source>> {
        self.source
            .lock()
            .map_err(|_| windows_core::Error::from(E_FAIL))?
            .clone()
            .ok_or_else(|| WINCODEC_ERR_NOTINITIALIZED.into())
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        LIVE_OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IWICBitmapDecoder_Impl for Decoder_Impl {
    fn QueryCapability(&self, stream: Ref<IStream>) -> windows_core::Result<u32> {
        let stream = stream.ok()?;
        let mut head = [0u8; 12];
        let mut read = 0u32;
        unsafe {
            stream.Read(head.as_mut_ptr().cast(), 12, Some(&mut read)).ok()?;
            // Restore the stream position, as the WIC contract requires.
            stream.Seek(-(read as i64), STREAM_SEEK_CUR, None)?;
        }
        if heic_core::looks_like_heif(&head[..read as usize]) {
            Ok((WICBitmapDecoderCapabilityCanDecodeAllImages.0
                | WICBitmapDecoderCapabilityCanDecodeSomeImages.0) as u32)
        } else {
            Ok(0)
        }
    }

    fn Initialize(&self, stream: Ref<IStream>, _opts: WICDecodeOptions) -> windows_core::Result<()> {
        let stream = stream.ok()?;
        let mut guard = self.source.lock().map_err(|_| windows_core::Error::from(E_FAIL))?;
        if guard.is_some() {
            return Err(WINCODEC_ERR_WRONGSTATE.into());
        }
        let bytes = read_all(stream)?;
        if !heic_core::looks_like_heif(&bytes) {
            return Err(WINCODEC_ERR_BADIMAGE.into());
        }
        *guard = Some(Arc::new(Source { bytes, pixels: OnceLock::new() }));
        Ok(())
    }

    fn GetContainerFormat(&self) -> windows_core::Result<GUID> {
        Ok(GUID_ContainerFormatHeif)
    }

    fn GetDecoderInfo(&self) -> windows_core::Result<IWICBitmapDecoderInfo> {
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            factory.CreateComponentInfo(&CLSID_HEIC_DECODER)?.cast()
        }
    }

    fn CopyPalette(&self, _p: Ref<IWICPalette>) -> windows_core::Result<()> {
        Err(WINCODEC_ERR_PALETTEUNAVAILABLE.into())
    }

    fn GetMetadataQueryReader(&self) -> windows_core::Result<IWICMetadataQueryReader> {
        Err(WINCODEC_ERR_UNSUPPORTEDOPERATION.into())
    }

    fn GetPreview(&self) -> windows_core::Result<IWICBitmapSource> {
        Err(WINCODEC_ERR_UNSUPPORTEDOPERATION.into())
    }

    fn GetColorContexts(
        &self,
        _count: u32,
        _contexts: *mut Option<IWICColorContext>,
        actual: *mut u32,
    ) -> windows_core::Result<()> {
        if actual.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe { *actual = 0 };
        Ok(())
    }

    fn GetThumbnail(&self) -> windows_core::Result<IWICBitmapSource> {
        Err(WINCODEC_ERR_CODECNOTHUMBNAIL.into())
    }

    fn GetFrameCount(&self) -> windows_core::Result<u32> {
        self.source()?;
        Ok(1)
    }

    fn GetFrame(&self, index: u32) -> windows_core::Result<IWICBitmapFrameDecode> {
        if index != 0 {
            return Err(E_INVALIDARG.into());
        }
        let source = self.source()?;
        source.pixels()?; // decode now so errors surface here
        LIVE_OBJECTS.fetch_add(1, Ordering::SeqCst);
        Ok(Frame { source }.into())
    }
}

// ---------------------------------------------------------------------------

#[implement(IWICBitmapFrameDecode)]
struct Frame {
    source: Arc<Source>,
}

impl Drop for Frame {
    fn drop(&mut self) {
        LIVE_OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IWICBitmapSource_Impl for Frame_Impl {
    fn GetSize(&self, w: *mut u32, h: *mut u32) -> windows_core::Result<()> {
        if w.is_null() || h.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let p = self.source.pixels()?;
        unsafe {
            *w = p.width;
            *h = p.height;
        }
        Ok(())
    }

    fn GetPixelFormat(&self) -> windows_core::Result<GUID> {
        let p = self.source.pixels()?;
        Ok(if p.bpp == 4 { GUID_WICPixelFormat32bppBGRA } else { GUID_WICPixelFormat24bppBGR })
    }

    fn GetResolution(&self, x: *mut f64, y: *mut f64) -> windows_core::Result<()> {
        if x.is_null() || y.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe {
            *x = 72.0;
            *y = 72.0;
        }
        Ok(())
    }

    fn CopyPalette(&self, _p: Ref<IWICPalette>) -> windows_core::Result<()> {
        Err(WINCODEC_ERR_PALETTEUNAVAILABLE.into())
    }

    fn CopyPixels(
        &self,
        rc: *const WICRect,
        stride: u32,
        buf_size: u32,
        buf: *mut u8,
    ) -> windows_core::Result<()> {
        if buf.is_null() {
            return Err(E_POINTER.into());
        }
        let p = self.source.pixels()?;
        let full = WICRect { X: 0, Y: 0, Width: p.width as i32, Height: p.height as i32 };
        let r = if rc.is_null() { full } else { unsafe { *rc } };
        if r.X < 0
            || r.Y < 0
            || r.Width < 0
            || r.Height < 0
            || r.X as i64 + r.Width as i64 > p.width as i64
            || r.Y as i64 + r.Height as i64 > p.height as i64
        {
            return Err(E_INVALIDARG.into());
        }
        if r.Width == 0 || r.Height == 0 {
            return Ok(());
        }
        let row_bytes = r.Width as usize * p.bpp as usize;
        let stride = stride as usize;
        if stride < row_bytes {
            return Err(E_INVALIDARG.into());
        }
        let needed = stride * (r.Height as usize - 1) + row_bytes;
        if (buf_size as usize) < needed {
            return Err(E_INVALIDARG.into());
        }
        let out = unsafe { std::slice::from_raw_parts_mut(buf, needed) };
        let src_stride = p.width as usize * p.bpp as usize;
        for row in 0..r.Height as usize {
            let s = (r.Y as usize + row) * src_stride + r.X as usize * p.bpp as usize;
            out[row * stride..row * stride + row_bytes].copy_from_slice(&p.data[s..s + row_bytes]);
        }
        Ok(())
    }
}

impl IWICBitmapFrameDecode_Impl for Frame_Impl {
    fn GetMetadataQueryReader(&self) -> windows_core::Result<IWICMetadataQueryReader> {
        Err(WINCODEC_ERR_UNSUPPORTEDOPERATION.into())
    }

    fn GetColorContexts(
        &self,
        count: u32,
        contexts: *mut Option<IWICColorContext>,
        actual: *mut u32,
    ) -> windows_core::Result<()> {
        if actual.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let p = self.source.pixels()?;
        let Some(icc) = &p.icc else {
            unsafe { *actual = 0 };
            return Ok(());
        };
        unsafe { *actual = 1 };
        if count == 0 || contexts.is_null() {
            return Ok(());
        }
        // The caller pre-creates the colour-context objects; we initialise them.
        let ctx = unsafe { (*contexts).as_ref() }.ok_or(windows_core::Error::from(E_INVALIDARG))?;
        unsafe { ctx.InitializeFromMemory(icc) }
    }

    fn GetThumbnail(&self) -> windows_core::Result<IWICBitmapSource> {
        Err(WINCODEC_ERR_CODECNOTHUMBNAIL.into())
    }
}
