// GamePrep's icon, drawn in code: a rounded square with a violet->blue
// gradient and a white lightning bolt. One source for the window icon
// (RGBA), the .exe resource (build.rs includes this file) and the MSI's
// Add/Remove Programs icon (`gameprep --write-icon`). No image assets,
// no image crates.
//
// Plain `//` comments on purpose: build.rs pulls this in with
// `#[path] mod`, where an inner `//!` doc comment wouldn't be allowed.

/// Lightning bolt outline in unit coordinates (0..1, y down).
const BOLT: [(f32, f32); 7] = [
    (0.60, 0.10),
    (0.27, 0.56),
    (0.48, 0.56),
    (0.40, 0.90),
    (0.75, 0.42),
    (0.54, 0.42),
    (0.64, 0.10),
];

fn inside_polygon(x: f32, y: f32, poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn inside_rounded_square(x: f32, y: f32, r: f32) -> bool {
    let (m, n) = (0.04, 0.96);
    if !(m..=n).contains(&x) || !(m..=n).contains(&y) {
        return false;
    }
    let cx = x.clamp(m + r, n - r);
    let cy = y.clamp(m + r, n - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// `size`x`size` RGBA8, row-major, top row first. 4x4 supersampled.
pub fn rgba(size: u32) -> Vec<u8> {
    const SS: u32 = 4;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (mut bg, mut bolt) = (0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    if inside_rounded_square(x, y, 0.22) {
                        bg += 1;
                        if inside_polygon(x, y, &BOLT) {
                            bolt += 1;
                        }
                    }
                }
            }
            let total = (SS * SS) as f32;
            let t = (px + py) as f32 / (2 * size.max(1)) as f32;
            // violet (124, 58, 237) -> blue (37, 99, 235)
            let grad = [
                124.0 + (37.0 - 124.0) * t,
                58.0 + (99.0 - 58.0) * t,
                237.0 - 2.0 * t,
            ];
            let w = if bg > 0 { bolt as f32 / bg as f32 } else { 0.0 };
            for c in grad {
                out.push((c * (1.0 - w) + 255.0 * w).round() as u8);
            }
            out.push((255.0 * bg as f32 / total).round() as u8);
        }
    }
    out
}

/// A multi-size .ico with 32-bit BMP entries (valid on every Windows).
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| bmp_entry(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&s, img) in sizes.iter().zip(&images) {
        let dim = if s >= 256 { 0 } else { s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

fn bmp_entry(size: u32) -> Vec<u8> {
    let px = rgba(size);
    let mut out = Vec::new();
    // BITMAPINFOHEADER; height is doubled (XOR image + AND mask).
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    // Pixels bottom-up, BGRA.
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            out.extend_from_slice(&[px[i + 2], px[i + 1], px[i], px[i + 3]]);
        }
    }
    // AND mask: all zero (alpha channel does the work); rows padded to 4 bytes.
    let row = size.div_ceil(32) * 4;
    out.extend(std::iter::repeat_n(0u8, (row * size) as usize));
    out
}
