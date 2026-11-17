//! The app icon (a paper plane in front of an "M" on a navy tile), drawn in code.
//! Shared by the window icon, the exe's .ico resource (via build.rs) and the
//! README's docs/logo.png (via examples/export_logo.rs).

const SUPERSAMPLE: u32 = 4;
const TILE_MARGIN: f32 = 0.04;
const TILE_RADIUS: f32 = 0.22;
const TILE_TOP_LEFT: [f32; 3] = [0x2f as f32, 0x52 as f32, 0x96 as f32];
const TILE_BOTTOM_RIGHT: [f32; 3] = [0x1e as f32, 0x32 as f32, 0x5c as f32];
const LETTER: [f32; 3] = [0x8e as f32, 0xb4 as f32, 0xff as f32];
const UPPER_WING: [f32; 3] = [255.0, 255.0, 255.0];
const LOWER_WING: [f32; 3] = [0xd6 as f32, 0xe2 as f32, 0xff as f32];

/// A block "M" in unit coordinates (y grows downwards), traced clockwise.
const LETTER_M: [(f32, f32); 12] = [
    (0.14, 0.78),
    (0.14, 0.22),
    (0.27, 0.22),
    (0.42, 0.47),
    (0.57, 0.22),
    (0.70, 0.22),
    (0.70, 0.78),
    (0.58, 0.78),
    (0.58, 0.43),
    (0.42, 0.68),
    (0.26, 0.43),
    (0.26, 0.78),
];

// Paper plane in its own unit box, placed over the M's lower right.
const TAIL: (f32, f32) = (0.18, 0.50);
const NOSE: (f32, f32) = (0.82, 0.20);
const KEEL: (f32, f32) = (0.58, 0.82);
const FOLD: (f32, f32) = (0.46, 0.58);
const PLANE_SCALE: f32 = 0.60;
const PLANE_OFFSET: (f32, f32) = (0.36, 0.34);
/// The tile-coloured rim around the plane, so it reads as being in front of the M.
const PLANE_RIM: f32 = 1.28;

/// Unmultiplied RGBA pixels, row-major from the top.
pub fn rgba(size: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    let samples = (SUPERSAMPLE * SUPERSAMPLE) as f32;
    for py in 0..size {
        for px in 0..size {
            let mut color = [0.0f32; 3];
            let mut coverage = 0.0f32;
            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let x = (px as f32 + (sx as f32 + 0.5) / SUPERSAMPLE as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SUPERSAMPLE as f32) / size as f32;
                    if let Some(c) = shade(x, y) {
                        for i in 0..3 {
                            color[i] += c[i];
                        }
                        coverage += 1.0;
                    }
                }
            }
            if coverage == 0.0 {
                pixels.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                for c in color {
                    pixels.push((c / coverage).round() as u8);
                }
                pixels.push((coverage / samples * 255.0).round() as u8);
            }
        }
    }
    pixels
}

fn shade(x: f32, y: f32) -> Option<[f32; 3]> {
    if !in_rounded_tile(x, y) {
        return None;
    }
    let tile = {
        let t = ((x + y) / 2.0).clamp(0.0, 1.0);
        std::array::from_fn(|i| TILE_TOP_LEFT[i] + (TILE_BOTTOM_RIGHT[i] - TILE_TOP_LEFT[i]) * t)
    };
    let place = |(u, v): (f32, f32)| (PLANE_OFFSET.0 + u * PLANE_SCALE, PLANE_OFFSET.1 + v * PLANE_SCALE);
    let (tail, nose, keel, fold) = (place(TAIL), place(NOSE), place(KEEL), place(FOLD));
    if in_triangle((x, y), tail, nose, fold) {
        return Some(UPPER_WING);
    }
    if in_triangle((x, y), fold, nose, keel) {
        return Some(LOWER_WING);
    }
    let centroid = ((tail.0 + nose.0 + keel.0) / 3.0, (tail.1 + nose.1 + keel.1) / 3.0);
    let grow = |p: (f32, f32)| {
        (centroid.0 + (p.0 - centroid.0) * PLANE_RIM, centroid.1 + (p.1 - centroid.1) * PLANE_RIM)
    };
    if in_triangle((x, y), grow(tail), grow(nose), grow(keel)) {
        return Some(tile);
    }
    if in_polygon((x, y), &LETTER_M) {
        return Some(LETTER);
    }
    Some(tile)
}

fn in_rounded_tile(x: f32, y: f32) -> bool {
    let half = 0.5 - TILE_MARGIN;
    let qx = (x - 0.5).abs() - (half - TILE_RADIUS);
    let qy = (y - 0.5).abs() - (half - TILE_RADIUS);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) <= TILE_RADIUS
}

fn in_triangle(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let cross = |o: (f32, f32), u: (f32, f32), v: (f32, f32)| {
        (u.0 - o.0) * (v.1 - o.1) - (u.1 - o.1) * (v.0 - o.0)
    };
    let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
    let has_negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_negative && has_positive)
}

/// Even-odd ray casting; works for the concave letter outline.
fn in_polygon(p: (f32, f32), points: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (a, b) = (points[i], points[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
        j = i;
    }
    inside
}
