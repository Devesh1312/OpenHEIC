//! Minimal EXIF/XMP helpers: extract the TIFF block from HEIF's Exif item and
//! reset the Orientation tag, because libheif already rotated the pixels.

const TAG_ORIENTATION: u16 = 0x0112;

/// HEIF Exif items start with a 4-byte big-endian offset to the TIFF header.
pub(crate) fn tiff_from_heif_exif(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.len() < 4 {
        return None;
    }
    let off = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
    let candidate = raw.get(4 + off..);
    if let Some(c) = candidate {
        if is_tiff(c) {
            return Some(c.to_vec());
        }
    }
    // Fallback: scan for a TIFF header.
    (0..raw.len().saturating_sub(8))
        .find(|&i| is_tiff(&raw[i..]))
        .map(|i| raw[i..].to_vec())
}

fn is_tiff(b: &[u8]) -> bool {
    b.len() >= 8 && (b.starts_with(b"II*\0") || b.starts_with(b"MM\0*"))
}

struct Tiff<'a> {
    b: &'a [u8],
    le: bool,
}

impl Tiff<'_> {
    fn u16(&self, o: usize) -> Option<u16> {
        let s = self.b.get(o..o + 2)?;
        Some(if self.le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
    }
    fn u32(&self, o: usize) -> Option<u32> {
        let s = self.b.get(o..o + 4)?;
        let a = [s[0], s[1], s[2], s[3]];
        Some(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }
}

/// Offset (within the TIFF block) of the 2-byte orientation value in IFD0.
fn orientation_value_offset(tiff: &[u8]) -> Option<usize> {
    if !is_tiff(tiff) {
        return None;
    }
    let t = Tiff { b: tiff, le: tiff[0] == b'I' };
    let ifd = t.u32(4)? as usize;
    let count = t.u16(ifd)? as usize;
    for i in 0..count {
        let e = ifd + 2 + i * 12;
        if t.u16(e)? == TAG_ORIENTATION {
            // type SHORT (3), count 1 -> value stored inline.
            if t.u16(e + 2)? == 3 && t.u32(e + 4)? == 1 {
                return Some(e + 8);
            }
            return None;
        }
    }
    None
}

/// Reads the EXIF orientation (1..=8) from a TIFF block.
pub fn exif_orientation(tiff: &[u8]) -> Option<u16> {
    let o = orientation_value_offset(tiff)?;
    Tiff { b: tiff, le: tiff[0] == b'I' }.u16(o)
}

/// Sets the orientation tag to 1 ("normal") in place, if present.
pub fn normalize_exif(mut tiff: Vec<u8>) -> Vec<u8> {
    if let Some(o) = orientation_value_offset(&tiff) {
        let v: [u8; 2] = if tiff[0] == b'I' { 1u16.to_le_bytes() } else { 1u16.to_be_bytes() };
        tiff[o..o + 2].copy_from_slice(&v);
    }
    tiff
}

/// Rewrites `tiff:Orientation` in an XMP packet to 1 (attribute or element form).
pub fn normalize_xmp(xmp: &[u8]) -> Vec<u8> {
    let Ok(s) = std::str::from_utf8(xmp) else {
        return xmp.to_vec();
    };
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    const KEY: &str = "tiff:Orientation";
    while let Some(pos) = rest.find(KEY) {
        let (head, tail) = rest.split_at(pos + KEY.len());
        out.push_str(head);
        rest = tail;
        // Attribute form: tiff:Orientation="6"
        let trimmed = rest.trim_start();
        let skipped = rest.len() - trimmed.len();
        if let Some(after_eq) = trimmed.strip_prefix('=') {
            let after_eq_t = after_eq.trim_start();
            if let Some(q) = after_eq_t.chars().next().filter(|c| *c == '"' || *c == '\'') {
                if let Some(end) = after_eq_t[1..].find(q) {
                    let consumed = skipped + 1 + (after_eq.len() - after_eq_t.len()) + 1 + end + 1;
                    out.push_str(&format!("={q}1{q}"));
                    rest = &rest[consumed..];
                    continue;
                }
            }
        }
        // Element form: <tiff:Orientation>6</tiff:Orientation>
        if let Some(after_gt) = rest.strip_prefix('>') {
            if let Some(end) = after_gt.find('<') {
                out.push_str(">1");
                rest = &after_gt[end..];
                continue;
            }
        }
    }
    out.push_str(rest);
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiff_le_with_orientation(v: u16) -> Vec<u8> {
        let mut t = b"II*\0".to_vec();
        t.extend_from_slice(&8u32.to_le_bytes());
        t.extend_from_slice(&1u16.to_le_bytes()); // 1 entry
        t.extend_from_slice(&TAG_ORIENTATION.to_le_bytes());
        t.extend_from_slice(&3u16.to_le_bytes());
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&v.to_le_bytes());
        t.extend_from_slice(&[0, 0]);
        t.extend_from_slice(&0u32.to_le_bytes());
        t
    }

    #[test]
    fn patches_le_orientation() {
        let t = tiff_le_with_orientation(6);
        assert_eq!(exif_orientation(&t), Some(6));
        let n = normalize_exif(t);
        assert_eq!(exif_orientation(&n), Some(1));
    }

    #[test]
    fn heif_offset_prefix() {
        let mut raw = vec![0, 0, 0, 6];
        raw.extend_from_slice(b"Exif\0\0");
        raw.extend_from_slice(&tiff_le_with_orientation(8));
        let t = tiff_from_heif_exif(&raw).unwrap();
        assert_eq!(exif_orientation(&t), Some(8));
    }

    #[test]
    fn xmp_forms() {
        let a = normalize_xmp(br#"<x tiff:Orientation="6" a="b"/>"#);
        assert_eq!(a, br#"<x tiff:Orientation="1" a="b"/>"#);
        let b = normalize_xmp(b"<tiff:Orientation>8</tiff:Orientation>");
        assert_eq!(b, b"<tiff:Orientation>1</tiff:Orientation>");
    }
}
