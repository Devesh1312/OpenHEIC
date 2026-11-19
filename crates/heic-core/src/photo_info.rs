//! Fast, pixel-free metadata summary of a HEIC/HEIF file, used by the Windows
//! property handler (Explorer Details pane, "Date taken", search index).

use libheif_rs::{HeifContext, LibHeif};

use crate::exif::{is_tiff, tiff_from_heif_exif, Tiff};

/// A calendar date/time exactly as written in EXIF (camera local time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExifDateTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

/// GPS position. Degrees/minutes/seconds as stored, plus the hemisphere refs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gps {
    pub latitude: Option<[f64; 3]>,
    pub latitude_ref: Option<String>,
    pub longitude: Option<[f64; 3]>,
    pub longitude_ref: Option<String>,
    pub altitude: Option<f64>,
    /// 0 = above sea level, 1 = below.
    pub altitude_ref: Option<u8>,
}

impl Gps {
    pub fn latitude_decimal(&self) -> Option<f64> {
        dms_to_decimal(self.latitude?, self.latitude_ref.as_deref() == Some("S"))
    }
    pub fn longitude_decimal(&self) -> Option<f64> {
        dms_to_decimal(self.longitude?, self.longitude_ref.as_deref() == Some("W"))
    }
}

fn dms_to_decimal(d: [f64; 3], negative: bool) -> Option<f64> {
    let v = d[0] + d[1] / 60.0 + d[2] / 3600.0;
    v.is_finite().then_some(if negative { -v } else { v })
}

/// Camera / capture metadata extracted from EXIF.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExifSummary {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens_make: Option<String>,
    pub lens_model: Option<String>,
    pub software: Option<String>,
    /// DateTimeOriginal, falling back to DateTimeDigitized, then DateTime.
    pub date_taken: Option<ExifDateTime>,
    /// Seconds.
    pub exposure_time: Option<f64>,
    pub f_number: Option<f64>,
    pub iso: Option<u32>,
    /// EV.
    pub exposure_bias: Option<f64>,
    /// Millimetres.
    pub focal_length: Option<f64>,
    pub focal_length_35mm: Option<u16>,
    pub flash: Option<u16>,
    pub metering_mode: Option<u16>,
    pub white_balance: Option<u16>,
    pub exposure_program: Option<u16>,
    pub gps: Option<Gps>,
}

/// Everything the property handler needs, read without decoding pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoInfo {
    /// Upright (displayed) size of the primary image.
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub has_alpha: bool,
    /// Number of top-level images (bursts / collections have more than one).
    pub image_count: usize,
    pub exif: Option<ExifSummary>,
}

pub fn read_photo_info(bytes: &[u8]) -> crate::Result<PhotoInfo> {
    let _lib = LibHeif::new();
    let ctx = HeifContext::read_from_bytes(bytes)?;
    let h = ctx.primary_image_handle()?;
    let exif = h
        .all_metadata()
        .into_iter()
        .find(|m| &m.item_type.0 == b"Exif")
        .and_then(|m| tiff_from_heif_exif(&m.raw_data))
        .and_then(|t| parse_exif(&t));
    Ok(PhotoInfo {
        width: h.width(),
        height: h.height(),
        bit_depth: h.luma_bits_per_pixel(),
        has_alpha: h.has_alpha_channel(),
        image_count: ctx.image_ids().len().max(1),
        exif,
    })
}

// --- TIFF/EXIF parsing ------------------------------------------------------

const IFD0_MAKE: u16 = 0x010F;
const IFD0_MODEL: u16 = 0x0110;
const IFD0_SOFTWARE: u16 = 0x0131;
const IFD0_DATETIME: u16 = 0x0132;
const PTR_EXIF: u16 = 0x8769;
const PTR_GPS: u16 = 0x8825;
const EXIF_EXPOSURE_TIME: u16 = 0x829A;
const EXIF_FNUMBER: u16 = 0x829D;
const EXIF_EXPOSURE_PROGRAM: u16 = 0x8822;
const EXIF_ISO: u16 = 0x8827;
const EXIF_DATETIME_ORIGINAL: u16 = 0x9003;
const EXIF_DATETIME_DIGITIZED: u16 = 0x9004;
const EXIF_EXPOSURE_BIAS: u16 = 0x9204;
const EXIF_METERING_MODE: u16 = 0x9207;
const EXIF_FLASH: u16 = 0x9209;
const EXIF_FOCAL_LENGTH: u16 = 0x920A;
const EXIF_WHITE_BALANCE: u16 = 0xA403;
const EXIF_FOCAL_35MM: u16 = 0xA405;
const EXIF_LENS_MAKE: u16 = 0xA433;
const EXIF_LENS_MODEL: u16 = 0xA434;

/// One IFD entry: (type, count, absolute offset of the value bytes).
#[derive(Clone, Copy)]
struct Entry {
    typ: u16,
    count: u32,
    at: usize,
}

fn type_size(t: u16) -> Option<usize> {
    Some(match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

impl Tiff<'_> {
    fn entries(&self, ifd: usize) -> Vec<(u16, Entry)> {
        let mut out = Vec::new();
        let Some(n) = self.u16(ifd) else { return out };
        for i in 0..n.min(512) as usize {
            let e = ifd + 2 + i * 12;
            let (Some(tag), Some(typ), Some(count)) = (self.u16(e), self.u16(e + 2), self.u32(e + 4)) else {
                break;
            };
            let Some(sz) = type_size(typ) else { continue };
            let total = sz.saturating_mul(count as usize);
            let at = if total <= 4 {
                e + 8
            } else {
                match self.u32(e + 8) {
                    Some(o) => o as usize,
                    None => continue,
                }
            };
            if at.checked_add(total).is_none_or(|end| end > self.b.len()) {
                continue;
            }
            out.push((tag, Entry { typ, count, at }));
        }
        out
    }

    fn uint(&self, e: Entry, i: usize) -> Option<u32> {
        match e.typ {
            1 | 7 => self.b.get(e.at + i).map(|&v| v as u32),
            3 => self.u16(e.at + 2 * i).map(u32::from),
            4 => self.u32(e.at + 4 * i),
            _ => None,
        }
    }

    fn rational(&self, e: Entry, i: usize) -> Option<f64> {
        let o = e.at + 8 * i;
        match e.typ {
            5 => {
                let (n, d) = (self.u32(o)?, self.u32(o + 4)?);
                (d != 0).then(|| n as f64 / d as f64)
            }
            10 => {
                let (n, d) = (self.u32(o)? as i32, self.u32(o + 4)? as i32);
                (d != 0).then(|| n as f64 / d as f64)
            }
            _ => self.uint(e, i).map(f64::from),
        }
    }

    fn ascii(&self, e: Entry) -> Option<String> {
        if e.typ != 2 && e.typ != 7 {
            return None;
        }
        let raw = self.b.get(e.at..e.at + e.count as usize)?;
        let raw = raw.split(|&c| c == 0).next().unwrap_or(raw);
        let s = String::from_utf8_lossy(raw).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

fn find(entries: &[(u16, Entry)], tag: u16) -> Option<Entry> {
    entries.iter().find(|(t, _)| *t == tag).map(|(_, e)| *e)
}

fn parse_datetime(s: &str) -> Option<ExifDateTime> {
    // "YYYY:MM:DD HH:MM:SS"
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<u16>().ok();
    let dt = ExifDateTime {
        year: num(0..4)?,
        month: num(5..7)?,
        day: num(8..10)?,
        hour: num(11..13)?,
        minute: num(14..16)?,
        second: num(17..19)?,
    };
    let valid = (1..=9999).contains(&dt.year)
        && (1..=12).contains(&dt.month)
        && (1..=31).contains(&dt.day)
        && dt.hour < 24
        && dt.minute < 60
        && dt.second < 61;
    valid.then_some(dt)
}

/// Parses the fields we expose from a TIFF-structured EXIF block.
pub fn parse_exif(tiff: &[u8]) -> Option<ExifSummary> {
    if !is_tiff(tiff) {
        return None;
    }
    let t = Tiff { b: tiff, le: tiff[0] == b'I' };
    let ifd0 = t.entries(t.u32(4)? as usize);
    let sub = |tag| find(&ifd0, tag).and_then(|e| t.uint(e, 0)).map(|o| t.entries(o as usize)).unwrap_or_default();
    let exif = sub(PTR_EXIF);
    let gps_ifd = sub(PTR_GPS);

    let s0 = |tag| find(&ifd0, tag).and_then(|e| t.ascii(e));
    let se = |tag| find(&exif, tag).and_then(|e| t.ascii(e));
    let ue = |tag| find(&exif, tag).and_then(|e| t.uint(e, 0));
    let re = |tag| find(&exif, tag).and_then(|e| t.rational(e, 0));

    let date_taken = se(EXIF_DATETIME_ORIGINAL)
        .or_else(|| se(EXIF_DATETIME_DIGITIZED))
        .or_else(|| s0(IFD0_DATETIME))
        .and_then(|s| parse_datetime(&s));

    let gps = if gps_ifd.is_empty() {
        None
    } else {
        let tri = |tag| {
            let e = find(&gps_ifd, tag)?;
            (e.count >= 3).then_some([t.rational(e, 0)?, t.rational(e, 1)?, t.rational(e, 2)?])
        };
        let ascii = |tag| find(&gps_ifd, tag).and_then(|e| t.ascii(e));
        let g = Gps {
            latitude_ref: ascii(1),
            latitude: tri(2),
            longitude_ref: ascii(3),
            longitude: tri(4),
            altitude_ref: find(&gps_ifd, 5).and_then(|e| t.uint(e, 0)).map(|v| v as u8),
            altitude: find(&gps_ifd, 6).and_then(|e| t.rational(e, 0)),
        };
        (g != Gps::default()).then_some(g)
    };

    let s = ExifSummary {
        make: s0(IFD0_MAKE),
        model: s0(IFD0_MODEL),
        software: s0(IFD0_SOFTWARE),
        lens_make: se(EXIF_LENS_MAKE),
        lens_model: se(EXIF_LENS_MODEL),
        date_taken,
        exposure_time: re(EXIF_EXPOSURE_TIME),
        f_number: re(EXIF_FNUMBER),
        iso: ue(EXIF_ISO),
        exposure_bias: re(EXIF_EXPOSURE_BIAS),
        focal_length: re(EXIF_FOCAL_LENGTH),
        focal_length_35mm: ue(EXIF_FOCAL_35MM).map(|v| v as u16),
        flash: ue(EXIF_FLASH).map(|v| v as u16),
        metering_mode: ue(EXIF_METERING_MODE).map(|v| v as u16),
        white_balance: ue(EXIF_WHITE_BALANCE).map(|v| v as u16),
        exposure_program: ue(EXIF_EXPOSURE_PROGRAM).map(|v| v as u16),
        gps,
    };
    (s != ExifSummary::default()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a little-endian TIFF with IFD0 {Make, Model, ExifPtr, GpsPtr},
    /// Exif IFD {DateTimeOriginal, FNumber, ISO} and GPS IFD {lat/lon}.
    fn sample_tiff() -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[..4].copy_from_slice(b"II*\0");
        b[4..8].copy_from_slice(&8u32.to_le_bytes());
        let w16 = |b: &mut Vec<u8>, o: usize, v: u16| b[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let w32 = |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        let entry = |b: &mut Vec<u8>, at: usize, tag: u16, typ: u16, count: u32, val: u32| {
            w16(b, at, tag);
            w16(b, at + 2, typ);
            w32(b, at + 4, count);
            w32(b, at + 8, val);
        };
        // IFD0 at 8 with 4 entries.
        w16(&mut b, 8, 4);
        entry(&mut b, 10, IFD0_MAKE, 2, 6, 200); // "Apple\0"
        entry(&mut b, 22, IFD0_MODEL, 2, 10, 210); // "iPhone 15\0"
        entry(&mut b, 34, PTR_EXIF, 4, 1, 100);
        entry(&mut b, 46, PTR_GPS, 4, 1, 150);
        b[200..206].copy_from_slice(b"Apple\0");
        b[210..220].copy_from_slice(b"iPhone 15\0");
        // Exif IFD at 100 with 3 entries.
        w16(&mut b, 100, 3);
        entry(&mut b, 102, EXIF_DATETIME_ORIGINAL, 2, 20, 230);
        entry(&mut b, 114, EXIF_FNUMBER, 5, 1, 260);
        entry(&mut b, 126, EXIF_ISO, 3, 1, 125);
        b[230..250].copy_from_slice(b"2024:07:15 18:30:05\0");
        w32(&mut b, 260, 178);
        w32(&mut b, 264, 100);
        // GPS IFD at 150: N 12deg 30' 0", E 77deg 15' 36".
        w16(&mut b, 150, 4);
        entry(&mut b, 152, 1, 2, 2, u32::from_le_bytes(*b"N\0\0\0"));
        entry(&mut b, 164, 2, 5, 3, 300);
        entry(&mut b, 176, 3, 2, 2, u32::from_le_bytes(*b"E\0\0\0"));
        entry(&mut b, 188, 4, 5, 3, 330);
        for (i, (n, d)) in [(12, 1), (30, 1), (0, 1)].iter().enumerate() {
            w32(&mut b, 300 + i * 8, *n);
            w32(&mut b, 304 + i * 8, *d);
        }
        for (i, (n, d)) in [(77, 1), (15, 1), (3600, 100)].iter().enumerate() {
            w32(&mut b, 330 + i * 8, *n);
            w32(&mut b, 334 + i * 8, *d);
        }
        b
    }

    #[test]
    fn parses_camera_date_and_gps() {
        let s = parse_exif(&sample_tiff()).unwrap();
        assert_eq!(s.make.as_deref(), Some("Apple"));
        assert_eq!(s.model.as_deref(), Some("iPhone 15"));
        assert_eq!(
            s.date_taken,
            Some(ExifDateTime { year: 2024, month: 7, day: 15, hour: 18, minute: 30, second: 5 })
        );
        assert_eq!(s.f_number, Some(1.78));
        assert_eq!(s.iso, Some(125));
        let g = s.gps.unwrap();
        assert!((g.latitude_decimal().unwrap() - 12.5).abs() < 1e-9);
        assert!((g.longitude_decimal().unwrap() - 77.26).abs() < 1e-9);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_exif(b"not a tiff at all").is_none());
        // Truncated / out-of-range offsets must not panic.
        let mut t = sample_tiff();
        t.truncate(120);
        let _ = parse_exif(&t);
        assert!(parse_datetime("0000:00:00 00:00:00").is_none());
        assert!(parse_datetime("    :  :     :  :  ").is_none());
    }
}
