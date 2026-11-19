//! Builds a small ICC v2 matrix/TRC display profile from nclx chromaticities
//! (sRGB transfer curve), so that e.g. Display-P3 HEICs that only carry an
//! nclx box keep their colours when written to JPEG.

type M3 = [[f64; 3]; 3];

const D50: [f64; 3] = [0.9642, 1.0, 0.8249];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

fn mul_v(a: &M3, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| a[i][0] * v[0] + a[i][1] * v[1] + a[i][2] * v[2])
}

fn inv(m: &M3) -> M3 {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    let k = 1.0 / det;
    [
        [(e * i - f * h) * k, (c * h - b * i) * k, (b * f - c * e) * k],
        [(f * g - d * i) * k, (a * i - c * g) * k, (c * d - a * f) * k],
        [(d * h - e * g) * k, (b * g - a * h) * k, (a * e - b * d) * k],
    ]
}

fn xy_to_xyz(x: f64, y: f64) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// Columns = D50-adapted XYZ of the R, G, B primaries.
fn colorants(prim: [(f32, f32); 3], white: (f32, f32)) -> M3 {
    let p: Vec<[f64; 3]> = prim.iter().map(|&(x, y)| xy_to_xyz(x as f64, y as f64)).collect();
    let pm: M3 = [0, 1, 2].map(|r| [p[0][r], p[1][r], p[2][r]]);
    let w = xy_to_xyz(white.0 as f64, white.1 as f64);
    let s = mul_v(&inv(&pm), w);
    let m: M3 = [0, 1, 2].map(|r| [pm[r][0] * s[0], pm[r][1] * s[1], pm[r][2] * s[2]]);

    // Bradford chromatic adaptation white -> D50.
    let ma: M3 = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let src = mul_v(&ma, w);
    let dst = mul_v(&ma, D50);
    let d: M3 = [
        [dst[0] / src[0], 0.0, 0.0],
        [0.0, dst[1] / src[1], 0.0],
        [0.0, 0.0, dst[2] / src[2]],
    ];
    let adapt = mul(&inv(&ma), &mul(&d, &ma));
    mul(&adapt, &m)
}

fn s15(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_tag(v: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    for c in v {
        t.extend_from_slice(&s15(c));
    }
    t
}

fn srgb_curve() -> Vec<u8> {
    const N: usize = 1024;
    let mut t = b"curv\0\0\0\0".to_vec();
    t.extend_from_slice(&(N as u32).to_be_bytes());
    for i in 0..N {
        let x = i as f64 / (N - 1) as f64;
        let y = if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) };
        t.extend_from_slice(&((y * 65535.0).round() as u16).to_be_bytes());
    }
    t
}

fn desc_tag(s: &str) -> Vec<u8> {
    let mut t = b"desc\0\0\0\0".to_vec();
    t.extend_from_slice(&((s.len() + 1) as u32).to_be_bytes());
    t.extend_from_slice(s.as_bytes());
    t.push(0);
    t.extend_from_slice(&[0u8; 4 + 4]); // unicode lang + count
    t.extend_from_slice(&[0u8; 2 + 1 + 67]); // scriptcode
    t
}

fn text_tag(s: &str) -> Vec<u8> {
    let mut t = b"text\0\0\0\0".to_vec();
    t.extend_from_slice(s.as_bytes());
    t.push(0);
    t
}

pub(crate) fn matrix_trc_profile(prim: [(f32, f32); 3], white: (f32, f32)) -> Vec<u8> {
    let c = colorants(prim, white);
    let trc = srgb_curve();
    // (signature, data). TRC tags share one data block.
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc_tag("HEIC nclx RGB")),
        (b"cprt", text_tag("No copyright, use freely")),
        (b"wtpt", xyz_tag(D50)),
        (b"rXYZ", xyz_tag([c[0][0], c[1][0], c[2][0]])),
        (b"gXYZ", xyz_tag([c[0][1], c[1][1], c[2][1]])),
        (b"bXYZ", xyz_tag([c[0][2], c[1][2], c[2][2]])),
        (b"rTRC", trc.clone()),
    ];
    let shared_trc = [b"gTRC", b"bTRC"];
    let tag_count = tags.len() + shared_trc.len();
    let table_len = 4 + 12 * tag_count;

    let mut data = Vec::new();
    let mut table = (tag_count as u32).to_be_bytes().to_vec();
    let mut trc_entry = (0u32, 0u32);
    for (sig, body) in &tags {
        let off = (128 + table_len + data.len()) as u32;
        table.extend_from_slice(*sig);
        table.extend_from_slice(&off.to_be_bytes());
        table.extend_from_slice(&(body.len() as u32).to_be_bytes());
        if *sig == b"rTRC" {
            trc_entry = (off, body.len() as u32);
        }
        data.extend_from_slice(body);
        while data.len() % 4 != 0 {
            data.push(0);
        }
    }
    for sig in shared_trc {
        table.extend_from_slice(sig);
        table.extend_from_slice(&trc_entry.0.to_be_bytes());
        table.extend_from_slice(&trc_entry.1.to_be_bytes());
    }

    let total = 128 + table.len() + data.len();
    let mut h = Vec::with_capacity(total);
    h.extend_from_slice(&(total as u32).to_be_bytes());
    h.extend_from_slice(b"\0\0\0\0"); // CMM
    h.extend_from_slice(&[2, 0x10, 0, 0]); // v2.1
    h.extend_from_slice(b"mntrRGB XYZ ");
    h.extend_from_slice(&[0x07, 0xEA, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0]); // date 2026-01-01
    h.extend_from_slice(b"acspMSFT");
    h.extend_from_slice(&[0u8; 4 + 4 + 4 + 8 + 4]); // flags, manuf, model, attrs, intent
    for v in D50 {
        h.extend_from_slice(&s15(v));
    }
    h.extend_from_slice(&[0u8; 4]); // creator
    h.resize(128, 0);
    h.extend_from_slice(&table);
    h.extend_from_slice(&data);
    debug_assert_eq!(h.len(), total);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_colorants_match_reference() {
        // sRGB primaries -> well-known D50 colorants (from sRGB IEC61966-2.1 ICC).
        let c = colorants([(0.64, 0.33), (0.30, 0.60), (0.15, 0.06)], (0.3127, 0.3290));
        assert!((c[0][0] - 0.4361).abs() < 0.002, "rX {}", c[0][0]);
        assert!((c[1][1] - 0.7169).abs() < 0.002, "gY {}", c[1][1]);
        assert!((c[2][2] - 0.7141).abs() < 0.002, "bZ {}", c[2][2]);
        // Row sums must equal D50.
        for r in 0..3 {
            let s: f64 = c[r].iter().sum();
            assert!((s - D50[r]).abs() < 0.001);
        }
    }

    #[test]
    fn profile_is_well_formed() {
        let p = matrix_trc_profile([(0.68, 0.32), (0.265, 0.69), (0.15, 0.06)], (0.3127, 0.3290));
        assert_eq!(u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize, p.len());
        assert_eq!(&p[36..40], b"acsp");
    }
}
