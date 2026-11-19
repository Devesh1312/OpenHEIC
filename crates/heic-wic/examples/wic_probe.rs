//! Test harness for the WIC codec.
//!
//!   wic_probe inproc <file.heic>           decode through heic_wic's COM objects directly (no registration needed)
//!   wic_probe system <file.heic> [out.png] decode via Windows' WIC (requires `heic-convert install`)
//!
//! Both modes convert through WICConvertBitmapSource (as Photos/Explorer do) and
//! compare the pixels against heic-core's own decode.

use std::ffi::c_void;

use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::SHCreateStreamOnFileEx;
use windows_core::{Interface, GUID, HSTRING, PCWSTR};

fn main() -> windows_core::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (mode, file) = (args[1].as_str(), args[2].as_str());
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
    let factory: IWICImagingFactory =
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };

    let decoder: IWICBitmapDecoder = match mode {
        "inproc" => unsafe {
            let mut cf: *mut c_void = std::ptr::null_mut();
            heic_wic::DllGetClassObject(&heic_wic::CLSID_HEIC_DECODER, &IClassFactory::IID, &mut cf).ok()?;
            let cf = IClassFactory::from_raw(cf);
            let d: IWICBitmapDecoder = cf.CreateInstance(None)?;
            let stream = SHCreateStreamOnFileEx(&HSTRING::from(file), STGM_READ.0, 0, false, None)?;
            let cap = d.QueryCapability(&stream)?;
            println!("QueryCapability = {cap:#x}");
            d.Initialize(&stream, WICDecodeMetadataCacheOnDemand)?;
            d
        },
        "system" => unsafe {
            let d = factory.CreateDecoderFromFilename(
                &HSTRING::from(file),
                None,
                windows::Win32::Foundation::GENERIC_ACCESS_RIGHTS(GENERIC_READ),
                WICDecodeMetadataCacheOnDemand,
            )?;
            let info = d.GetDecoderInfo()?;
            let clsid: GUID = info.GetCLSID()?;
            let mut name = [0u16; 256];
            let mut n = 0;
            info.GetFriendlyName(&mut name, &mut n)?;
            println!(
                "Windows chose decoder {clsid:?} \"{}\" (ours: {})",
                String::from_utf16_lossy(&name[..n.saturating_sub(1) as usize]),
                clsid == heic_wic::CLSID_HEIC_DECODER
            );
            d
        },
        _ => panic!("mode must be inproc or system"),
    };

    unsafe {
        println!("container = {:?}, frames = {}", decoder.GetContainerFormat()?, decoder.GetFrameCount()?);
        let frame = decoder.GetFrame(0)?;
        let (mut w, mut h) = (0, 0);
        frame.GetSize(&mut w, &mut h)?;
        println!("size = {w}x{h}, pixel format = {:?}", frame.GetPixelFormat()?);

        let ctx = factory.CreateColorContext()?;
        let mut contexts = [Some(ctx)];
        let mut actual = 0;
        frame.GetColorContexts(&mut contexts, &mut actual)?;
        if actual > 0 {
            let c = contexts[0].as_ref().unwrap();
            let mut sz = 0;
            let mut icc = vec![0u8; 1 << 20];
            c.GetProfileBytes(&mut icc, &mut sz)?;
            println!("color contexts = {actual}, ICC bytes = {sz}");
        } else {
            println!("color contexts = 0");
        }

        // Convert like viewers do, then compare with heic-core.
        let conv = WICConvertBitmapSource(&GUID_WICPixelFormat32bppBGRA, &frame)?;
        let stride = w * 4;
        let mut buf = vec![0u8; (stride * h) as usize];
        conv.CopyPixels(std::ptr::null(), stride, &mut buf)?;

        // A sub-rectangle copy, to exercise the rect path.
        let rect = WICRect { X: 10, Y: 20, Width: 7, Height: 5 };
        let mut sub = vec![0u8; 7 * 4 * 5];
        conv.CopyPixels(&rect, 7 * 4, &mut sub)?;
        for row in 0..5usize {
            let full = &buf[((20 + row) * stride as usize + 10 * 4)..][..7 * 4];
            assert_eq!(&sub[row * 28..row * 28 + 28], full, "sub-rect mismatch");
        }

        let reference = heic_core::decode_file(file, heic_core::DecodeOptions::default()).unwrap();
        assert_eq!((reference.width, reference.height), (w, h));
        let mut max_diff = 0u8;
        for (bgra, rgb) in buf.chunks_exact(4).zip(reference.pixels.chunks_exact(3)) {
            max_diff = max_diff
                .max(bgra[2].abs_diff(rgb[0]))
                .max(bgra[1].abs_diff(rgb[1]))
                .max(bgra[0].abs_diff(rgb[2]));
        }
        println!("max pixel difference vs heic-core = {max_diff}");
        assert_eq!(max_diff, 0, "WIC pixels differ from reference decode");

        if let Some(out) = args.get(3) {
            write_png(&factory, &conv, out)?;
            println!("wrote {out}");
        }
    }
    println!("OK");
    Ok(())
}

unsafe fn write_png(factory: &IWICImagingFactory, src: &IWICBitmapSource, out: &str) -> windows_core::Result<()> {
    unsafe {
        let stream = factory.CreateStream()?;
        stream.InitializeFromFilename(PCWSTR(HSTRING::from(out).as_ptr()), GENERIC_WRITE)?;
        let enc = factory.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.unwrap();
        frame.Initialize(None)?;
        frame.WriteSource(src, std::ptr::null())?;
        frame.Commit()?;
        enc.Commit()?;
    }
    Ok(())
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
