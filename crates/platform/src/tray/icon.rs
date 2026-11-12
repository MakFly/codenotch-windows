//! Tray icon drawn in code (RGBA, straight alpha, top-down rows). No binary assets.
//!
//! * Mark (default): a dark disc carrying a green ring that sweeps three quarters of the way
//!   round from 12 o'clock, on a grey track — the notch ring, reduced to an icon.
//! * Usage (`trayShowsUsage` on and a figure is known): a dark rounded plate with the worst
//!   headline percentage as pixel-font digits coloured by grade (0–99). At 100 % the mark is
//!   drawn with a full ring in the critical colour instead (three digits are not legible at
//!   16 px).
//!
//! The dark plate keeps the colours readable on both light and dark taskbars.

use codenotch_core::format::{grade, Grade};
use codenotch_core::Snapshot;

/// Sizes Windows asks for at 100/125/150/200 % scale.
pub const SIZES: [u32; 4] = [16, 20, 24, 32];

/// What the icon shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconSpec {
    /// Percentage drawn as digits (0–100); `None` = the plain mark.
    pub figure: Option<u8>,
    pub grade: Grade,
}

impl IconSpec {
    pub const MARK: IconSpec = IconSpec {
        figure: None,
        grade: Grade::Unknown,
    };
}

/// Icon for a snapshot: the worst headline percentage when `trayShowsUsage` is on.
pub fn icon_spec(snapshot: &Snapshot) -> IconSpec {
    let s = snapshot.settings();
    if !s.tray_shows_usage {
        return IconSpec::MARK;
    }
    let Some(p) = snapshot.worst_percent().filter(|p| p.is_finite()) else {
        return IconSpec::MARK;
    };
    let p = p.clamp(0.0, 100.0);
    // Never show 100 before the limit is really reached.
    let shown = if p < 100.0 {
        p.round().min(99.0)
    } else {
        100.0
    };
    IconSpec {
        figure: Some(shown as u8),
        grade: grade(Some(p), s.warn_percent(), s.critical_percent()),
    }
}

/// Snap a system small-icon size to one we draw.
pub fn snap_size(px: i32) -> u32 {
    let px = px.max(1) as u32;
    SIZES
        .iter()
        .copied()
        .min_by_key(|s| s.abs_diff(px))
        .unwrap_or(16)
}

type Rgba = [u8; 4];

const PLATE: Rgba = [0x14, 0x14, 0x14, 0xFF];
const TRACK: Rgba = [0x4A, 0x4A, 0x4E, 0xFF];
const GREEN: Rgba = [0x00, 0xFF, 0x88, 0xFF];
const YELLOW: Rgba = [0xF2, 0xFF, 0x00, 0xFF];
const RED: Rgba = [0xFF, 0x3F, 0x00, 0xFF];
const GREY: Rgba = [0xC7, 0xC7, 0xCC, 0xFF];

/// Grade colour (dark-theme palette: the plate is always dark).
pub fn grade_rgba(g: Grade) -> Rgba {
    match g {
        Grade::Ok => GREEN,
        Grade::Warn => YELLOW,
        Grade::Critical => RED,
        Grade::Unknown => GREY,
    }
}

/// 3×5 pixel font, rows top to bottom, 3 bits per row (MSB = left).
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// Digit scale for a size: two digits (3 + 1 + 3 cells) fit with at least 1 px margin.
pub fn digit_scale(size: u32) -> u32 {
    ((size.saturating_sub(2)) / 7).max(1)
}

/// Rasterise `spec` at `size`×`size`. Returns `size*size*4` bytes.
pub fn rasterize(spec: &IconSpec, size: u32) -> Vec<u8> {
    let size = size.clamp(8, 256);
    let mut img = vec![0u8; (size * size * 4) as usize];
    match spec.figure {
        Some(f) if f < 100 => {
            draw_plate(&mut img, size);
            draw_number(&mut img, size, f, grade_rgba(spec.grade));
        }
        Some(_) => draw_mark(&mut img, size, 1.0, RED),
        None => draw_mark(&mut img, size, 0.75, GREEN),
    }
    img
}

/// Source-over blend of `c` with coverage `a` (0–1) into pixel (x, y).
fn blend(img: &mut [u8], size: u32, x: u32, y: u32, c: Rgba, a: f32) {
    if a <= 0.0 || x >= size || y >= size {
        return;
    }
    let i = ((y * size + x) * 4) as usize;
    let sa = (f32::from(c[3]) / 255.0) * a.min(1.0);
    let da = f32::from(img[i + 3]) / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return;
    }
    for k in 0..3 {
        let sc = f32::from(c[k]);
        let dc = f32::from(img[i + k]);
        img[i + k] = ((sc * sa + dc * da * (1.0 - sa)) / oa)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    img[i + 3] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// 4×4 supersampled coverage of a shape given as an inside test in pixel coordinates.
fn coverage(x: u32, y: u32, inside: impl Fn(f32, f32) -> bool) -> f32 {
    const N: u32 = 4;
    let mut hits = 0;
    for sy in 0..N {
        for sx in 0..N {
            let px = x as f32 + (sx as f32 + 0.5) / N as f32;
            let py = y as f32 + (sy as f32 + 0.5) / N as f32;
            if inside(px, py) {
                hits += 1;
            }
        }
    }
    hits as f32 / (N * N) as f32
}

fn draw_plate(img: &mut [u8], size: u32) {
    let s = size as f32;
    let r = (s * 0.22).max(2.0);
    let inside = |px: f32, py: f32| {
        let cx = px.clamp(r, s - r);
        let cy = py.clamp(r, s - r);
        let (dx, dy) = (px - cx, py - cy);
        dx * dx + dy * dy <= r * r
    };
    for y in 0..size {
        for x in 0..size {
            blend(img, size, x, y, PLATE, coverage(x, y, inside));
        }
    }
}

fn draw_number(img: &mut [u8], size: u32, value: u8, color: Rgba) {
    let text = value.to_string();
    let s = digit_scale(size);
    let n = text.len() as u32;
    let width = n * 3 * s + (n - 1) * s;
    let height = 5 * s;
    let x0 = (size - width.min(size)) / 2;
    let y0 = (size - height.min(size)) / 2;
    for (i, ch) in text.bytes().enumerate() {
        let glyph = DIGITS[(ch - b'0') as usize];
        let gx = x0 + i as u32 * 4 * s;
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..3u32 {
                if bits & (0b100 >> col) == 0 {
                    continue;
                }
                for dy in 0..s {
                    for dx in 0..s {
                        blend(
                            img,
                            size,
                            gx + col * s + dx,
                            y0 + row as u32 * s + dy,
                            color,
                            1.0,
                        );
                    }
                }
            }
        }
    }
}

/// Dark disc + track ring + an arc of `sweep` (0–1) from 12 o'clock clockwise.
fn draw_mark(img: &mut [u8], size: u32, sweep: f32, color: Rgba) {
    let s = size as f32;
    let c = s / 2.0;
    let disc_r = s / 2.0 - 0.25;
    let ring_r = s * 0.31;
    let stroke = (s * 0.14).max(1.6);
    let (inner, outer) = (ring_r - stroke / 2.0, ring_r + stroke / 2.0);
    let in_disc = |px: f32, py: f32| {
        let (dx, dy) = (px - c, py - c);
        dx * dx + dy * dy <= disc_r * disc_r
    };
    let in_ring = |px: f32, py: f32| {
        let (dx, dy) = (px - c, py - c);
        let d2 = dx * dx + dy * dy;
        d2 >= inner * inner && d2 <= outer * outer
    };
    let in_arc = |px: f32, py: f32| {
        if !in_ring(px, py) {
            return false;
        }
        // Angle clockwise from 12 o'clock, 0–1.
        let a = (px - c).atan2(-(py - c));
        let t = if a < 0.0 {
            a + std::f32::consts::TAU
        } else {
            a
        } / std::f32::consts::TAU;
        t <= sweep
    };
    for y in 0..size {
        for x in 0..size {
            blend(img, size, x, y, PLATE, coverage(x, y, in_disc));
            blend(img, size, x, y, TRACK, coverage(x, y, in_ring));
            blend(img, size, x, y, color, coverage(x, y, in_arc));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &[u8], size: u32, x: u32, y: u32) -> Rgba {
        let i = ((y * size + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    fn upper_left_on_ring(size: u32) -> (u32, u32) {
        let c = size as f32 / 2.0;
        let d = size as f32 * 0.31 * std::f32::consts::FRAC_1_SQRT_2;
        let v = (c - d) as u32;
        (v, v)
    }

    #[test]
    fn buffer_sizes() {
        for size in SIZES {
            assert_eq!(
                rasterize(&IconSpec::MARK, size).len(),
                (size * size * 4) as usize
            );
            let spec = IconSpec {
                figure: Some(42),
                grade: Grade::Ok,
            };
            assert_eq!(rasterize(&spec, size).len(), (size * size * 4) as usize);
        }
    }

    #[test]
    fn mark_pixels() {
        for size in SIZES {
            let img = rasterize(&IconSpec::MARK, size);
            // Corners are transparent (round disc).
            assert_eq!(px(&img, size, 0, 0)[3], 0, "size {size}");
            assert_eq!(px(&img, size, size - 1, size - 1)[3], 0);
            // Centre is the dark disc.
            assert_eq!(px(&img, size, size / 2, size / 2), PLATE);
            // Right of centre on the ring (3 o'clock, inside the 75 % sweep) is green.
            let ring_x = (size as f32 / 2.0 + size as f32 * 0.31) as u32;
            let p = px(&img, size, ring_x, size / 2);
            assert!(p[1] > 200 && p[0] < 60, "size {size}: {p:?}");
            // Upper left (about 10:30, outside the sweep) is the grey track.
            let (tx, ty) = upper_left_on_ring(size);
            let p = px(&img, size, tx, ty);
            assert!(p[0] < 120 && p[1] < 120 && p[0] > 40, "size {size}: {p:?}");
        }
    }

    #[test]
    fn full_ring_is_red_at_100() {
        let size = 32;
        let img = rasterize(
            &IconSpec {
                figure: Some(100),
                grade: Grade::Critical,
            },
            size,
        );
        let (tx, ty) = upper_left_on_ring(size);
        let p = px(&img, size, tx, ty);
        assert!(p[0] > 200 && p[1] < 120, "{p:?}");
    }

    #[test]
    fn digits_are_drawn_in_the_grade_colour() {
        // 16 px, scale 2: "61" occupies x 1..15, y 3..13.
        let size = 16;
        assert_eq!(digit_scale(size), 2);
        let img = rasterize(
            &IconSpec {
                figure: Some(61),
                grade: Grade::Warn,
            },
            size,
        );
        // Top-left cell of the "6" (row 0, col 0) at (1,3).
        assert_eq!(px(&img, size, 1, 3), YELLOW);
        // "6" row 1 col 1 is empty → plate.
        assert_eq!(px(&img, size, 3, 5), PLATE);
        // "1" row 0, col 1 is set: x = 1 + 8 + 2 = 11, y = 3.
        assert_eq!(px(&img, size, 11, 3), YELLOW);
        // "1" row 0, col 0 is empty.
        assert_eq!(px(&img, size, 9, 3), PLATE);
        // Rounded plate corners are (nearly) transparent; edge midpoints are opaque.
        assert!(px(&img, size, 0, 0)[3] < 128);
        assert_eq!(px(&img, size, 0, 8), PLATE);

        let img = rasterize(
            &IconSpec {
                figure: Some(7),
                grade: Grade::Critical,
            },
            32,
        );
        // One digit, scale 4, width 12: x0 = 10, y0 = 6. "7" row 0 col 0 set.
        assert_eq!(px(&img, 32, 10, 6), RED);
        assert_eq!(px(&img, 32, 21, 25), RED); // row 4, col 2 (x 18..21, y 22..25)
        assert_eq!(px(&img, 32, 10, 25), PLATE); // row 4, col 0 empty
    }

    #[test]
    fn digit_scales() {
        assert_eq!(digit_scale(16), 2);
        assert_eq!(digit_scale(20), 2);
        assert_eq!(digit_scale(24), 3);
        assert_eq!(digit_scale(32), 4);
    }

    #[test]
    fn spec_from_snapshot() {
        let mut snap = Snapshot::demo_at(1_763_000_000);
        // Demo worst headline: Cursor 97% (rate limited, still shows figures).
        let spec = icon_spec(&snap);
        assert_eq!(spec.figure, Some(97));
        assert_eq!(spec.grade, Grade::Critical);
        snap.config.settings.tray_shows_usage = false;
        assert_eq!(icon_spec(&snap), IconSpec::MARK);
        let empty = Snapshot::from_config(Default::default(), 0);
        assert_eq!(icon_spec(&empty), IconSpec::MARK);
    }

    #[test]
    fn size_snapping() {
        assert_eq!(snap_size(16), 16);
        assert_eq!(snap_size(18), 16);
        assert_eq!(snap_size(20), 20);
        assert_eq!(snap_size(24), 24);
        assert_eq!(snap_size(30), 32);
        assert_eq!(snap_size(48), 32);
        assert_eq!(snap_size(0), 16);
    }
}
