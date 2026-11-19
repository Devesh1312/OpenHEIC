//! Shared HEIC decoding and max-quality JPEG encoding used by both the
//! `heic-convert` CLI and the `heic_wic` Windows Imaging Component codec.

mod exif;
mod photo_info;
mod icc;

use std::io::Write;
use std::path::Path;

use libheif_rs::{
    ColorPrimaries, ColorSpace, HeifContext, ImageHandle, LibHeif, RgbChroma,
    TransferCharacteristics,
};

pub use exif::{exif_orientation, normalize_exif, normalize_xmp};
pub use photo_info::{parse_exif, read_photo_info, ExifDateTime, ExifSummary, Gps, PhotoInfo};

/// Default JPEG quality. 100 + 4:4:4 chroma = visually lossless.
pub const DEFAULT_QUALITY: u8 = 100;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HEIF decode error")]
    Heif(#[from] libheif_rs::HeifError),
    #[error("I/O error")]
    Io(#[from] std::io::Error),
    #[error("JPEG encode error")]
    Jpeg(#[from] jpeg_encoder::EncodingError),
    #[error("image is too large for JPEG ({0}x{1}, max 65535x65535)")]
    TooLarge(u32, u32),
    #[error("decoded image has no interleaved pixel plane")]
    NoPixels,
}

pub type Result<T> = std::result::Result<T, Error>;

/// A decoded, already upright (rotation/mirror/crop applied) 8-bit image.
#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// 3 (RGB) or 4 (RGBA, straight alpha).
    pub channels: u8,
    /// Tightly packed pixels: `width * channels` bytes per row.
    pub pixels: Vec<u8>,
    /// ICC profile (embedded, or synthesised from an nclx colour description).
    pub icc: Option<Vec<u8>>,
    /// Raw TIFF-structured EXIF (starts with `II*\0` / `MM\0*`), orientation
    /// already normalised to 1 because the pixels are upright.
    pub exif: Option<Vec<u8>>,
    /// XMP packet with any orientation normalised to 1.
    pub xmp: Option<Vec<u8>>,
    /// Bit depth of the source luma plane (8, 10, 12...).
    pub source_bit_depth: u8,
}

impl DecodedImage {
    pub fn has_alpha(&self) -> bool {
        self.channels == 4
    }

    /// Returns tightly packed RGB, compositing any alpha over white.
    pub fn to_rgb(&self) -> std::borrow::Cow<'_, [u8]> {
        if self.channels == 3 {
            return std::borrow::Cow::Borrowed(&self.pixels);
        }
        let mut out = Vec::with_capacity(self.width as usize * self.height as usize * 3);
        for px in self.pixels.chunks_exact(4) {
            let a = px[3] as u32;
            for &c in &px[..3] {
                out.push(((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8);
            }
        }
        std::borrow::Cow::Owned(out)
    }
}

/// What to decode.
#[derive(Debug, Clone, Copy, Default)]
pub struct DecodeOptions {
    /// Keep an alpha channel when the image has one (otherwise RGB only).
    pub keep_alpha: bool,
    /// Skip reading EXIF/XMP/ICC metadata (faster for viewers).
    pub skip_metadata: bool,
}

/// Decodes the primary image of a HEIC/HEIF file on disk.
pub fn decode_file(path: impl AsRef<Path>, opts: DecodeOptions) -> Result<DecodedImage> {
    let bytes = std::fs::read(path)?;
    decode_bytes(&bytes, opts)
}

/// Decodes the primary image of an in-memory HEIC/HEIF file.
pub fn decode_bytes(bytes: &[u8], opts: DecodeOptions) -> Result<DecodedImage> {
    let lib = LibHeif::new();
    let ctx = HeifContext::read_from_bytes(bytes)?;
    let handle = ctx.primary_image_handle()?;
    decode_handle(&lib, &handle, opts)
}

/// Basic information without decoding pixels.
#[derive(Debug, Clone, Copy)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub has_alpha: bool,
    pub bit_depth: u8,
}

pub fn probe_bytes(bytes: &[u8]) -> Result<ImageInfo> {
    let _lib = LibHeif::new();
    let ctx = HeifContext::read_from_bytes(bytes)?;
    let h = ctx.primary_image_handle()?;
    Ok(ImageInfo {
        width: h.width(),
        height: h.height(),
        has_alpha: h.has_alpha_channel(),
        bit_depth: h.luma_bits_per_pixel(),
    })
}

/// Returns true if `head` (first >= 12 bytes of a file) looks like HEIF/HEIC.
pub fn looks_like_heif(head: &[u8]) -> bool {
    if head.len() < 12 || &head[4..8] != b"ftyp" {
        return false;
    }
    matches!(
        &head[8..12],
        b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"mif1" | b"msf1" | b"heif"
    )
}

fn decode_handle(lib: &LibHeif, handle: &ImageHandle, opts: DecodeOptions) -> Result<DecodedImage> {
    let alpha = opts.keep_alpha && handle.has_alpha_channel();
    let chroma = if alpha { RgbChroma::Rgba } else { RgbChroma::Rgb };
    // Interleaved 8-bit RGB(A). libheif applies irot/imir/clap transforms and
    // converts higher bit depths down to 8 bits.
    let image = lib.decode(handle, ColorSpace::Rgb(chroma), None)?;
    let width = image.width();
    let height = image.height();
    let channels: u8 = if alpha { 4 } else { 3 };

    let planes = image.planes();
    let plane = planes.interleaved.ok_or(Error::NoPixels)?;
    let row_len = width as usize * channels as usize;
    let mut pixels = Vec::with_capacity(row_len * height as usize);
    for row in plane.data.chunks(plane.stride).take(height as usize) {
        pixels.extend_from_slice(&row[..row_len]);
    }

    let (icc, exif, xmp) = if opts.skip_metadata {
        (None, None, None)
    } else {
        read_metadata(handle)
    };

    Ok(DecodedImage {
        width,
        height,
        channels,
        pixels,
        icc,
        exif,
        xmp,
        source_bit_depth: handle.luma_bits_per_pixel(),
    })
}

/// Returns the ICC profile for the image, if it is not plain sRGB.
pub fn icc_profile(handle: &ImageHandle) -> Option<Vec<u8>> {
    if let Some(raw) = handle.color_profile_raw() {
        if !raw.data.is_empty() {
            return Some(raw.data);
        }
    }
    let nclx = handle.color_profile_nclx()?;
    let prim = nclx.color_primaries();
    let trc = nclx.transfer_characteristics();
    let is_srgb_trc = matches!(
        trc,
        TransferCharacteristics::IEC_61966_2_1
            | TransferCharacteristics::Unspecified
            | TransferCharacteristics::ITU_R_BT_709_5
    );
    if matches!(prim, ColorPrimaries::ITU_R_BT_709_5 | ColorPrimaries::Unspecified) && is_srgb_trc {
        return None; // sRGB: no profile needed.
    }
    if !is_srgb_trc {
        // PQ/HLG etc. can't be represented by a simple matrix/TRC profile; the
        // 8-bit output is tone-mapped by libheif, so tag nothing.
        return None;
    }
    Some(icc::matrix_trc_profile(
        [
            (nclx.color_primary_red_x(), nclx.color_primary_red_y()),
            (nclx.color_primary_green_x(), nclx.color_primary_green_y()),
            (nclx.color_primary_blue_x(), nclx.color_primary_blue_y()),
        ],
        (nclx.color_primary_white_x(), nclx.color_primary_white_y()),
    ))
}

fn read_metadata(handle: &ImageHandle) -> (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>) {
    let icc = icc_profile(handle);
    let mut exif = None;
    let mut xmp = None;
    for m in handle.all_metadata() {
        let ty: [u8; 4] = m.item_type.0;
        if &ty == b"Exif" && exif.is_none() {
            exif = exif::tiff_from_heif_exif(&m.raw_data).map(normalize_exif);
        } else if &ty == b"mime" && m.content_type.contains("rdf+xml") && xmp.is_none() {
            xmp = Some(normalize_xmp(&m.raw_data));
        }
    }
    (icc, exif, xmp)
}

/// JPEG encoding settings.
#[derive(Debug, Clone, Copy)]
pub struct JpegOptions {
    /// 1..=100
    pub quality: u8,
    /// Copy ICC / EXIF / XMP into the JPEG.
    pub keep_metadata: bool,
}

impl Default for JpegOptions {
    fn default() -> Self {
        Self { quality: DEFAULT_QUALITY, keep_metadata: true }
    }
}

const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Encodes `img` as a baseline JPEG with 4:4:4 chroma (no subsampling).
pub fn encode_jpeg<W: Write>(img: &DecodedImage, out: W, opts: JpegOptions) -> Result<()> {
    if img.width > u16::MAX as u32 || img.height > u16::MAX as u32 {
        return Err(Error::TooLarge(img.width, img.height));
    }
    let mut enc = jpeg_encoder::Encoder::new(out, opts.quality.clamp(1, 100));
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_4_4);
    enc.set_optimized_huffman_tables(true);

    if opts.keep_metadata {
        if let Some(exif) = &img.exif {
            // APP1 payload max is 65533 bytes including the 6-byte "Exif\0\0".
            if exif.len() <= 65533 - 6 {
                enc.add_exif_metadata(exif)?;
            }
        }
        if let Some(xmp) = &img.xmp {
            if xmp.len() + XMP_HEADER.len() <= 65533 {
                let mut seg = XMP_HEADER.to_vec();
                seg.extend_from_slice(xmp);
                enc.add_app_segment(1, seg)?;
            }
        }
        if let Some(icc) = &img.icc {
            enc.add_icc_profile(icc)?;
        }
    }

    let rgb = img.to_rgb();
    enc.encode(&rgb, img.width as u16, img.height as u16, jpeg_encoder::ColorType::Rgb)?;
    Ok(())
}

/// Convenience: decode a HEIC file and write it as a JPEG file.
pub fn convert_file(src: &Path, dst: &Path, opts: JpegOptions) -> Result<()> {
    let img = decode_file(src, DecodeOptions { keep_alpha: false, skip_metadata: !opts.keep_metadata })?;
    let f = std::fs::File::create(dst)?;
    let mut w = std::io::BufWriter::with_capacity(1 << 20, f);
    encode_jpeg(&img, &mut w, opts)?;
    w.flush()?;
    Ok(())
}

/// Bilinear-free box downscale to fit within `max_side` (used for thumbnails).
pub fn downscale(img: &DecodedImage, max_side: u32) -> DecodedImage {
    let (w, h) = (img.width, img.height);
    if w <= max_side && h <= max_side {
        return img.clone();
    }
    let scale = max_side as f64 / w.max(h) as f64;
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    let c = img.channels as usize;
    let mut out = vec![0u8; nw as usize * nh as usize * c];
    for y in 0..nh {
        let sy0 = (y as u64 * h as u64 / nh as u64) as u32;
        let sy1 = (((y + 1) as u64 * h as u64 / nh as u64) as u32).max(sy0 + 1);
        for x in 0..nw {
            let sx0 = (x as u64 * w as u64 / nw as u64) as u32;
            let sx1 = (((x + 1) as u64 * w as u64 / nw as u64) as u32).max(sx0 + 1);
            let mut acc = [0u32; 4];
            for sy in sy0..sy1 {
                let row = &img.pixels[(sy as usize * w as usize) * c..];
                for sx in sx0..sx1 {
                    let p = &row[sx as usize * c..sx as usize * c + c];
                    for i in 0..c {
                        acc[i] += p[i] as u32;
                    }
                }
            }
            let n = (sy1 - sy0) * (sx1 - sx0);
            let o = (y as usize * nw as usize + x as usize) * c;
            for i in 0..c {
                out[o + i] = ((acc[i] + n / 2) / n) as u8;
            }
        }
    }
    DecodedImage { width: nw, height: nh, pixels: out, ..img.clone() }
}
