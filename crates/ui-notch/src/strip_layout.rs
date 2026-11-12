//! Taskbar strip geometry (pure). Each cell is a notch cell turned on its side: the ring in the
//! notch's 56-unit box with the figure beside it instead of under it. Cells are packed at the
//! notch's spacing (6 px ring→figure, 14 px between cells, on its 44 px ring) and centred; given
//! less room than the row needs, the whole row shrinks — no cell is dropped. The ring box is
//! 32 px at Medium (so an outside weekly ring still clears a 48 px taskbar) and is bounded by the
//! strip height at every size.
//!
//! Also the hover card's window and tail geometry (the card opens in its own window above — or
//! below a top taskbar — so the taskbar cannot clip it).
//!
//! All lengths are logical px of the strip window unless they say "device".

use crate::dims::{CARD_W, TAIL_INSET, TAIL_LEN, TAIL_SPAN};
use crate::geom::{cubic, Pt, Rect};
use codenotch_ui_kit::window::DeviceRect;

/// Ring box units (the notch's).
pub const UNITS: f32 = 56.0;
/// Main ring outer diameter: radius 25 + half of its 5-unit stroke, ×2.
pub const TRACK_EXTENT: f32 = 55.0;
/// Outside weekly ring outer diameter: radius 31 + half of 2.4, ×2.
pub const OUTSIDE_EXTENT: f32 = 64.4;
/// Ring box at Medium, logical px.
pub const BOX: f32 = 32.0;
/// Figure size at Medium (the taskbar clock's own size), logical px.
pub const FONT: f32 = 12.0;
/// The notch's 6 px ring→figure and 14 px cell gap, in ring units.
pub const TEXT_GAP: f32 = 6.0 * UNITS / 44.0;
pub const CELL_GAP: f32 = 14.0 * UNITS / 44.0;
/// Room kept for the tallest card (content scrolls beyond), logical px at Medium.
pub const CARD_ROOM: f32 = 600.0;

/// One cell's place in the strip. `x..x+w` is its hit span: it runs to the middle of the gaps
/// on each side, so there is no dead space between cells (neighbours share an edge).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub x: f32,
    pub w: f32,
    /// Ring centre.
    pub cx: f32,
    pub cy: f32,
    /// px per ring unit.
    pub unit: f32,
    /// Left edge of the figure.
    pub text_x: f32,
    /// Figure font size.
    pub font: f32,
}

/// Ring extent in units: an outside weekly ring needs its room around every cell, so cells do
/// not shift when one account's weekly reading comes or goes.
pub fn extent(outside_weekly: bool) -> f32 {
    if outside_weekly {
        OUTSIDE_EXTENT
    } else {
        TRACK_EXTENT
    }
}

/// Unit and font of an unconstrained row for a strip `height` tall at notch size `size`.
fn natural(height: f32, size: f32, extent: f32) -> Option<(f32, f32)> {
    if !height.is_finite() || height <= 0.0 {
        return None;
    }
    let size = if size.is_finite() && size > 0.0 {
        size
    } else {
        1.0
    };
    let unit = (BOX * size / UNITS).min(height / extent);
    Some((unit, FONT * size))
}

fn cell_width(em: f32, unit: f32, font: f32, extent: f32) -> f32 {
    (extent + TEXT_GAP) * unit + em.max(0.0) * font
}

/// Half a cell gap at each end, a whole one between cells.
fn row_width(ems: &[f32], unit: f32, font: f32, extent: f32) -> f32 {
    ems.iter()
        .map(|&em| cell_width(em, unit, font, extent) + CELL_GAP * unit)
        .sum()
}

/// Width the strip needs for these cells (`ems` = each figure's width at a 1 px font), logical
/// px, rounded up; 0 for no cells or a bad height.
pub fn content_width(ems: &[f32], height: f32, size: f32, outside_weekly: bool) -> f32 {
    if ems.is_empty() {
        return 0.0;
    }
    let e = extent(outside_weekly);
    natural(height, size, e).map_or(0.0, |(unit, font)| row_width(ems, unit, font, e).ceil())
}

/// Cells packed and centred in `width × height`; shrunk together when `width` is too small.
pub fn layout(ems: &[f32], width: f32, height: f32, size: f32, outside_weekly: bool) -> Vec<Slot> {
    if ems.is_empty() || !width.is_finite() || width <= 0.0 {
        return Vec::new();
    }
    let e = extent(outside_weekly);
    let Some((mut unit, mut font)) = natural(height, size, e) else {
        return Vec::new();
    };
    let wanted = row_width(ems, unit, font, e);
    if wanted > width {
        let fit = width / wanted;
        unit *= fit;
        font *= fit;
    }
    let gap = CELL_GAP * unit;
    let mut left = (width - row_width(ems, unit, font, e)).max(0.0) / 2.0 + gap / 2.0;
    let mut lefts = Vec::with_capacity(ems.len());
    for &em in ems {
        lefts.push(left);
        left += cell_width(em, unit, font, e) + gap;
    }
    // Each boundary computed once so neighbours share it exactly.
    let mut edges: Vec<f32> = lefts.iter().map(|l| l - gap / 2.0).collect();
    edges[0] = 0.0;
    edges.push(width);
    lefts
        .iter()
        .enumerate()
        .map(|(i, &l)| Slot {
            x: edges[i],
            w: edges[i + 1] - edges[i],
            cx: l + e * unit / 2.0,
            cy: height / 2.0,
            unit,
            text_x: l + (e + TEXT_GAP) * unit,
            font,
        })
        .collect()
}

/// The cell under `x` (strip coordinates).
pub fn slot_at(slots: &[Slot], x: f32) -> Option<usize> {
    if !x.is_finite() {
        return None;
    }
    slots.iter().position(|s| x >= s.x && x < s.x + s.w)
}

/// Figure width at a 1 px font when the text system cannot tell: Segoe UI Semibold advances
/// with tabular figures (every digit as wide as a 0).
pub fn estimate_em(text: &str) -> f32 {
    text.chars()
        .map(|c| match c {
            '0'..='9' => 0.5552,
            '%' => 0.8403,
            '.' => 0.2412,
            '~' | '<' | '>' => 0.6943,
            '…' => 0.8135,
            '—' => 1.0,
            _ => 0.6,
        })
        .sum()
}

// ---------------------------------------------------------------------------------------------
// Hover card
// ---------------------------------------------------------------------------------------------

/// The hover card's window: `card width × room` device px on the strip's side of the screen,
/// centred on the hovered ring and kept 8 px inside the monitor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardFrame {
    /// Device px.
    pub window: DeviceRect,
    /// Logical size of the window.
    pub w: f32,
    pub h: f32,
    /// Card hangs below the strip (taskbar at the top); else stands above it.
    pub below: bool,
    /// Tail centre along the window's width, logical px.
    pub tail_x: f32,
}

/// Window for a card anchored on the ring at device x `anchor_x` of `strip`. `dpi` = device px
/// per logical px, `s` = notch size scale. `None` when the monitor has no room.
pub fn card_frame(
    strip: DeviceRect,
    monitor: DeviceRect,
    anchor_x: i32,
    bottom: bool,
    dpi: f32,
    s: f32,
) -> Option<CardFrame> {
    if !(dpi.is_finite() && dpi > 0.0 && s.is_finite() && s > 0.0) || strip.w <= 0 || strip.h <= 0 {
        return None;
    }
    let gap = (crate::dims::CARD_MARGIN * dpi).round() as i32;
    let tail_gap = (codenotch_ui_kit::metrics::TAIL_GAP * dpi).round().max(1.0) as i32;
    let w = ((CARD_W * s * dpi).round() as i32).min(monitor.w - 2 * gap);
    let room = if bottom {
        strip.y - tail_gap - (monitor.y + gap)
    } else {
        monitor.bottom() - gap - (strip.bottom() + tail_gap)
    };
    let h = ((CARD_ROOM + TAIL_LEN) * s * dpi).round().min(room as f32) as i32;
    if w <= 0 || h <= ((TAIL_LEN + 40.0) * s * dpi) as i32 {
        return None;
    }
    let x = (anchor_x - w / 2).clamp(
        monitor.x + gap,
        (monitor.right() - w - gap).max(monitor.x + gap),
    );
    let y = if bottom {
        strip.y - tail_gap - h
    } else {
        strip.bottom() + tail_gap
    };
    let (lw, lh) = (w as f32 / dpi, h as f32 / dpi);
    let half = TAIL_SPAN * s / 2.0;
    let (lo, hi) = (TAIL_INSET * s + half, lw - TAIL_INSET * s - half);
    let want = (anchor_x - x) as f32 / dpi;
    let tail_x = if hi < lo {
        lw / 2.0
    } else {
        want.clamp(lo, hi)
    };
    Some(CardFrame {
        window: DeviceRect { x, y, w, h },
        w: lw,
        h: lh,
        below: !bottom,
        tail_x,
    })
}

/// The card inside its window for content `height` tall (logical px; capped by the room).
pub fn card_rect(f: &CardFrame, height: f32, s: f32) -> Rect {
    let tail = TAIL_LEN * s;
    let h = height.clamp(1.0, (f.h - tail).max(1.0));
    if f.below {
        Rect::new(0.0, tail, f.w, h)
    } else {
        Rect::new(0.0, f.h - tail - h, f.w, h)
    }
}

/// Tail wedge from the card edge toward the strip, centred on `f.tail_x` (the notch's tail
/// path; its base overlaps the card by 1 px so the two read as one shape).
pub fn tail_polygon(f: &CardFrame, card: Rect, s: f32, segments: usize) -> Vec<Pt> {
    let p = |x: f32, y: f32| (x * s, y * s);
    // x = toward the tip, y = along the edge.
    let mut local = Vec::with_capacity(2 * segments + 4);
    local.push((-1.0, 0.0));
    local.push(p(0.0, 0.0));
    local.extend(
        cubic(
            p(0.0, 0.0),
            p(0.0, 8.18),
            p(16.36, 12.43),
            p(TAIL_LEN, 16.36),
            segments,
        )
        .into_iter()
        .skip(1),
    );
    local.extend(
        cubic(
            p(TAIL_LEN, 16.36),
            p(16.36, 20.29),
            p(0.0, 24.54),
            p(0.0, TAIL_SPAN),
            segments,
        )
        .into_iter()
        .skip(1),
    );
    local.push((-1.0, TAIL_SPAN * s));
    let left = f.tail_x - TAIL_SPAN * s / 2.0;
    local
        .into_iter()
        .map(|(x, y)| {
            if f.below {
                (left + y, card.y - x)
            } else {
                (left + y, card.bottom() + x)
            }
        })
        .collect()
}

/// Bounding box of the tail (for hit testing and the window region).
pub fn tail_rect(f: &CardFrame, card: Rect, s: f32) -> Rect {
    let left = f.tail_x - TAIL_SPAN * s / 2.0;
    let len = TAIL_LEN * s + 1.0;
    if f.below {
        Rect::new(left, card.y - TAIL_LEN * s, TAIL_SPAN * s, len)
    } else {
        Rect::new(left, card.bottom() - 1.0, TAIL_SPAN * s, len)
    }
}

/// Logical rect → device rect (window coordinates) at `dpi`, rounded outwards.
pub fn to_device(r: Rect, dpi: f32) -> DeviceRect {
    let x = (r.x * dpi).floor() as i32;
    let y = (r.y * dpi).floor() as i32;
    DeviceRect {
        x,
        y,
        w: (r.right() * dpi).ceil() as i32 - x,
        h: (r.bottom() * dpi).ceil() as i32 - y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ems(texts: &[&str]) -> Vec<f32> {
        texts.iter().map(|t| estimate_em(t)).collect()
    }

    #[test]
    fn the_strip_is_as_wide_as_its_cells() {
        let one = content_width(&ems(&["8%"]), 42.0, 1.0, false);
        let two = content_width(&ems(&["8%", "~1"]), 42.0, 1.0, false);
        assert!(one > 40.0 && one < 70.0, "one cell: {one}");
        assert!(two > one * 1.8 && two < one * 2.2);
        // Tabular figures: 31% and 47% need the same room.
        assert_eq!(
            content_width(&ems(&["31%"]), 42.0, 1.0, false),
            content_width(&ems(&["47%"]), 42.0, 1.0, false)
        );
        assert_eq!(content_width(&[], 42.0, 1.0, false), 0.0);
    }

    #[test]
    fn cells_are_packed_centred_and_share_edges() {
        let e = ems(&["8%", "100%", "~1"]);
        let needed = content_width(&e, 42.0, 1.0, false);
        for width in [needed, needed + 60.0] {
            let slots = layout(&e, width, 42.0, 1.0, false);
            assert_eq!(slots.len(), 3);
            assert_eq!(slots[0].x, 0.0);
            assert_eq!(slots[2].x + slots[2].w, width);
            for pair in slots.windows(2) {
                assert_eq!(pair[0].x + pair[0].w, pair[1].x);
                let gap = pair[1].cx - pair[0].text_x;
                assert!(gap > 0.0 && gap < 60.0, "packed: {gap}");
            }
            let margin_left = slots[0].cx - slots[0].unit * TRACK_EXTENT / 2.0;
            let margin_right = width - (slots[2].text_x + e[2] * slots[2].font);
            assert!(
                (margin_left - margin_right).abs() < 0.5,
                "centred: {margin_left} vs {margin_right}"
            );
        }
        let slots = layout(&e, needed, 42.0, 1.0, false);
        assert_eq!(slot_at(&slots, 0.0), Some(0));
        assert_eq!(slot_at(&slots, slots[1].x), Some(1));
        assert_eq!(slot_at(&slots, needed - 0.01), Some(2));
        assert_eq!(slot_at(&slots, needed), None);
        assert_eq!(slot_at(&slots, f32::NAN), None);
    }

    #[test]
    fn the_ring_fits_the_strip_height_at_every_size() {
        for outside in [false, true] {
            let mut previous = 0.0;
            for size in [0.8, 1.0, 1.25] {
                for height in [26.0, 42.0, 66.0] {
                    let s = layout(&ems(&["8%"]), 1000.0, height, size, outside)[0];
                    assert!(
                        s.unit * extent(outside) <= height + 1e-3,
                        "{outside} {size} {height}"
                    );
                    assert_eq!(s.cy, height / 2.0);
                }
                let s = layout(&ems(&["8%"]), 1000.0, 42.0, size, outside)[0];
                assert!(s.unit >= previous);
                previous = s.unit;
            }
        }
        // Medium on a 42 px strip: the 32 px box.
        let s = layout(&ems(&["8%"]), 1000.0, 42.0, 1.0, false)[0];
        assert!((s.unit * UNITS - BOX).abs() < 1e-3);
        // Large is bounded by the height.
        let s = layout(&ems(&["8%"]), 1000.0, 42.0, 1.25, true)[0];
        assert!((s.unit - 42.0 / OUTSIDE_EXTENT).abs() < 1e-4);
    }

    #[test]
    fn a_narrow_strip_shrinks_the_row_instead_of_dropping_cells() {
        let e = ems(&["8%", "10%", "~1", "—", "…"]);
        let natural = layout(&e, 1000.0, 42.0, 1.0, false)[0].unit;
        let slots = layout(&e, 120.0, 42.0, 1.0, false);
        assert_eq!(slots.len(), 5);
        assert!(slots[0].unit < natural);
        let last = slots[4];
        assert!(last.text_x + e[4] * last.font <= 120.0 + 0.01);
    }

    #[test]
    fn invalid_geometry_has_no_slots() {
        let e = ems(&["8%"]);
        for w in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(layout(&e, w, 42.0, 1.0, false).is_empty());
        }
        assert!(layout(&e, 100.0, f32::NAN, 1.0, false).is_empty());
        assert!(layout(&[], 100.0, 42.0, 1.0, false).is_empty());
        assert_eq!(
            layout(&e, 100.0, 42.0, f32::NAN, false),
            layout(&e, 100.0, 42.0, 1.0, false)
        );
    }

    const MON: DeviceRect = DeviceRect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };

    #[test]
    fn card_stands_above_a_bottom_strip_centred_on_the_ring() {
        let strip = DeviceRect {
            x: 800,
            y: 1035,
            w: 280,
            h: 42,
        };
        let f = card_frame(strip, MON, 900, true, 1.0, 1.0).unwrap();
        assert!(!f.below);
        assert_eq!(f.window.w, 246);
        assert_eq!(f.window.x, 900 - 123);
        // tail gap 10.5 → 11 px (rounded) above the strip
        assert_eq!(f.window.bottom(), 1035 - 11);
        assert!((f.tail_x - 123.0).abs() < 1e-3);
        let card = card_rect(&f, 180.0, 1.0);
        assert_eq!(card.bottom(), f.h - TAIL_LEN);
        assert_eq!(card.h, 180.0);
        let tail = tail_rect(&f, card, 1.0);
        assert!((tail.bottom() - f.h).abs() < 1.01);
        let poly = tail_polygon(&f, card, 1.0, 8);
        let tip = poly.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        assert!(
            (tip - f.h).abs() < 0.01,
            "the tail points down at the strip"
        );
    }

    #[test]
    fn card_clamps_at_monitor_edges_and_hangs_below_a_top_strip() {
        let left = card_frame(
            DeviceRect {
                x: 8,
                y: 3,
                w: 280,
                h: 42,
            },
            MON,
            20,
            false,
            1.0,
            1.0,
        )
        .unwrap();
        assert!(left.below);
        assert_eq!((left.window.x, left.window.y), (8, 45 + 11));
        // The tail stays inside the card's rounded corners.
        assert!(left.tail_x >= TAIL_INSET + TAIL_SPAN / 2.0 - 1e-3);
        let card = card_rect(&left, 100.0, 1.0);
        assert_eq!(card.y, TAIL_LEN);
        let tip = tail_polygon(&left, card, 1.0, 8)
            .iter()
            .map(|p| p.1)
            .fold(f32::MAX, f32::min);
        assert!(tip.abs() < 0.01, "the tail points up at the strip");
        let right = card_frame(
            DeviceRect {
                x: 1632,
                y: 1035,
                w: 280,
                h: 42,
            },
            MON,
            1910,
            true,
            1.0,
            1.0,
        )
        .unwrap();
        assert_eq!(right.window.right(), 1912);
    }

    #[test]
    fn card_follows_dpi_and_negative_monitors() {
        let mon = DeviceRect {
            x: -2880,
            y: -200,
            w: 2880,
            h: 1620,
        };
        let strip = DeviceRect {
            x: -1500,
            y: 1360,
            w: 420,
            h: 54,
        };
        let f = card_frame(strip, mon, -1428, true, 1.5, 1.0).unwrap();
        assert_eq!(f.window.w, 369);
        assert_eq!(f.window.x, -1428 - 184);
        assert_eq!(f.window.bottom(), 1360 - 16);
        assert!((f.w - 246.0).abs() < 0.01);
        assert!((f.tail_x - 184.0 / 1.5).abs() < 0.01);
        // Content taller than the room is capped (it scrolls).
        let card = card_rect(&f, 5000.0, 1.0);
        assert!((card.y).abs() < 0.01);
    }

    #[test]
    fn card_rejects_bad_input_and_tiny_monitors() {
        let strip = DeviceRect {
            x: 8,
            y: 214,
            w: 264,
            h: 34,
        };
        assert!(card_frame(strip, MON, 50, true, f32::NAN, 1.0).is_none());
        assert!(card_frame(strip, MON, 50, true, 1.0, 0.0).is_none());
        assert!(card_frame(DeviceRect { w: 0, ..strip }, MON, 50, true, 1.0, 1.0).is_none());
        // 214 px above the strip: room for a short, scrolling card.
        let f = card_frame(strip, MON, 50, true, 1.0, 1.0).unwrap();
        assert_eq!(f.window.y, 8);
        assert!(card_frame(DeviceRect { y: 60, ..strip }, MON, 50, true, 1.0, 1.0).is_none());
    }

    #[test]
    fn device_rects_round_outwards() {
        let d = to_device(Rect::new(0.5, 1.2, 10.0, 10.0), 1.5);
        assert_eq!((d.x, d.y, d.w, d.h), (0, 1, 16, 16));
    }
}
