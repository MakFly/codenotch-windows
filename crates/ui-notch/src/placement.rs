//! Where the notch window goes on screen, and how a drag maps to a new placement
//! (data-spec §11). Pure functions over device pixels.

use crate::geom::Insets;
use codenotch_core::config::Edge;
use codenotch_ui_kit::window::DeviceRect;

/// Window rect for `edge` at fraction `along` (0..1, the window centre along the edge) inside
/// `area` (work area, or the whole monitor when a full-screen app is in front):
/// `y = wy + clamp(round(wh·r − H/2), 0, wh − H)` (upright), `x` likewise on flat edges.
pub fn place_window(edge: Edge, along: f64, area: DeviceRect, w: i32, h: i32) -> DeviceRect {
    let r = if along.is_finite() {
        along.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let slide = |span: i32, len: i32| -> i32 {
        let max = (span - len).max(0);
        ((span as f64 * r - len as f64 / 2.0).round() as i32).clamp(0, max)
    };
    match edge {
        Edge::Right => DeviceRect {
            x: area.right() - w,
            y: area.y + slide(area.h, h),
            w,
            h,
        },
        Edge::Left => DeviceRect {
            x: area.x,
            y: area.y + slide(area.h, h),
            w,
            h,
        },
        Edge::Top => DeviceRect {
            x: area.x + slide(area.w, w),
            y: area.y,
            w,
            h,
        },
        Edge::Bottom => DeviceRect {
            x: area.x + slide(area.w, w),
            y: area.bottom() - h,
            w,
            h,
        },
    }
}

/// How much of the window lies outside `work` on each side, logical px (top, right, bottom, left).
pub fn insets(win: DeviceRect, work: DeviceRect, scale: f32) -> (f32, f32, f32, f32) {
    let s = if scale > 0.0 { scale } else { 1.0 };
    let f = |d: i32| d.max(0) as f32 / s;
    (
        f(work.y - win.y),
        f(win.right() - work.right()),
        f(win.bottom() - work.bottom()),
        f(work.x - win.x),
    )
}

/// The along-edge part of [`insets`].
pub fn along_insets(edge: Edge, ins: (f32, f32, f32, f32)) -> Insets {
    let (t, r, b, l) = ins;
    if edge.is_upright() {
        Insets { lo: t, hi: b }
    } else {
        Insets { lo: l, hi: r }
    }
}

pub fn lerp_rect(a: DeviceRect, b: DeviceRect, t: f32) -> DeviceRect {
    let f = |x: i32, y: i32| (x as f32 + (y - x) as f32 * t).round() as i32;
    DeviceRect {
        x: f(a.x, b.x),
        y: f(a.y, b.y),
        w: f(a.w, b.w),
        h: f(a.h, b.h),
    }
}

/// Edge the dragged notch should hug: the nearest one, but the current edge is kept unless
/// another is at least `hysteresis` device px nearer.
pub fn drag_edge(current: Edge, p: (i32, i32), area: DeviceRect, hysteresis: f32) -> Edge {
    let d = |e: Edge| -> f32 {
        (match e {
            Edge::Left => p.0 - area.x,
            Edge::Right => area.right() - p.0,
            Edge::Top => p.1 - area.y,
            Edge::Bottom => area.bottom() - p.1,
        })
        .max(0) as f32
    };
    let best = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom]
        .into_iter()
        .min_by(|a, b| d(*a).total_cmp(&d(*b)))
        .unwrap_or(current);
    if best != current && d(best) + hysteresis <= d(current) {
        best
    } else {
        current
    }
}

/// Fraction along `edge` for a pointer at `p`, keeping the grip `offset` (device px between the
/// pointer and the notch centre along the edge, measured when the drag started).
pub fn drag_along(edge: Edge, p: (i32, i32), area: DeviceRect, offset: f32) -> f64 {
    let (pos, start, span) = if edge.is_upright() {
        (p.1, area.y, area.h)
    } else {
        (p.0, area.x, area.w)
    };
    if span <= 0 {
        return 0.5;
    }
    (((pos - start) as f32 - offset) as f64 / span as f64).clamp(0.0, 1.0)
}

/// Monitor to use while dragging: switch only when the pointer is at least `slack` device px
/// beyond the current monitor's bounds **and** over another monitor.
pub fn drag_monitor(current: usize, p: (i32, i32), monitors: &[DeviceRect], slack: i32) -> usize {
    let Some(cur) = monitors.get(current) else {
        return monitors
            .iter()
            .position(|m| m.contains(p.0, p.1))
            .unwrap_or(0);
    };
    let grown = DeviceRect {
        x: cur.x - slack,
        y: cur.y - slack,
        w: cur.w + 2 * slack,
        h: cur.h + 2 * slack,
    };
    if grown.contains(p.0, p.1) {
        return current;
    }
    monitors
        .iter()
        .position(|m| m.contains(p.0, p.1))
        .unwrap_or(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: DeviceRect = DeviceRect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1040,
    };

    #[test]
    fn placement_per_edge() {
        let r = place_window(Edge::Right, 0.5, AREA, 360, 650);
        assert_eq!(
            r,
            DeviceRect {
                x: 1560,
                y: 195,
                w: 360,
                h: 650
            }
        );
        let l = place_window(Edge::Left, 0.0, AREA, 360, 650);
        assert_eq!((l.x, l.y), (0, 0));
        let b = place_window(Edge::Right, 1.0, AREA, 360, 650);
        assert_eq!(b.bottom(), 1040);
        let t = place_window(Edge::Top, 0.5, AREA, 650, 650);
        assert_eq!((t.x, t.y), (635, 0));
        let bo = place_window(Edge::Bottom, 0.25, AREA, 650, 650);
        assert_eq!((bo.x, bo.bottom()), (155, 1040));
        // window larger than the area: pinned to the start
        let big = place_window(
            Edge::Right,
            0.5,
            DeviceRect {
                x: 100,
                y: 50,
                w: 800,
                h: 400,
            },
            360,
            650,
        );
        assert_eq!(big.y, 50);
        assert_eq!(place_window(Edge::Right, f64::NAN, AREA, 360, 650).y, 195);
    }

    #[test]
    fn insets_and_glide() {
        let work = DeviceRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1032,
        };
        let win = DeviceRect {
            x: 1560,
            y: 600,
            w: 360,
            h: 650,
        };
        let ins = insets(win, work, 1.0);
        assert_eq!(ins, (0.0, 0.0, 218.0, 0.0));
        assert_eq!(
            along_insets(Edge::Right, ins),
            Insets { lo: 0.0, hi: 218.0 }
        );
        assert_eq!(along_insets(Edge::Top, ins), Insets { lo: 0.0, hi: 0.0 });
        assert_eq!(insets(win, work, 2.0).2, 109.0);
        let a = DeviceRect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        };
        let b = DeviceRect {
            x: 100,
            y: 50,
            w: 10,
            h: 30,
        };
        assert_eq!(
            lerp_rect(a, b, 0.5),
            DeviceRect {
                x: 50,
                y: 25,
                w: 10,
                h: 20
            }
        );
    }

    #[test]
    fn drag_switches_edge_with_hysteresis() {
        // near the right edge
        assert_eq!(drag_edge(Edge::Right, (1900, 500), AREA, 40.0), Edge::Right);
        // nearer the top by 30 px only: stays right
        assert_eq!(drag_edge(Edge::Right, (1880, 10), AREA, 40.0), Edge::Right);
        // nearer the top by 40+ px: switches
        assert_eq!(drag_edge(Edge::Right, (1860, 5), AREA, 40.0), Edge::Top);
        // middle of the left half
        assert_eq!(drag_edge(Edge::Right, (20, 520), AREA, 40.0), Edge::Left);
    }

    #[test]
    fn drag_along_keeps_grip_offset() {
        assert!((drag_along(Edge::Right, (1900, 520), AREA, 0.0) - 0.5).abs() < 1e-9);
        assert!((drag_along(Edge::Right, (1900, 620), AREA, 100.0) - 0.5).abs() < 1e-9);
        assert_eq!(drag_along(Edge::Top, (-50, 0), AREA, 0.0), 0.0);
        assert_eq!(drag_along(Edge::Bottom, (5000, 0), AREA, 0.0), 1.0);
    }

    #[test]
    fn drag_changes_monitor_only_well_past_the_border() {
        let mons = [
            AREA,
            DeviceRect {
                x: 1920,
                y: 0,
                w: 2560,
                h: 1440,
            },
        ];
        assert_eq!(drag_monitor(0, (1950, 300), &mons, 150), 0);
        assert_eq!(drag_monitor(0, (2100, 300), &mons, 150), 1);
        assert_eq!(drag_monitor(0, (-500, 300), &mons, 150), 0); // nowhere: stay
        assert_eq!(drag_monitor(9, (2100, 300), &mons, 150), 1);
    }
}
