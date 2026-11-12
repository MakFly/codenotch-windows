//! Free space on the primary Windows taskbar, for the optional "taskbar mode" (rings shown as a
//! strip in the taskbar instead of the floating notch).
//!
//! Read-only: the taskbar (`Shell_TrayWnd`) is located with `FindWindowW`, its rectangle, edge
//! and auto-hide state are read with `GetWindowRect` / `SHAppBarMessage(ABM_GETSTATE)`, and the
//! occupied spans (Start, search, widgets, task buttons, system tray) come from UI Automation
//! element rectangles plus the `TrayNotifyWnd` child rectangle. Nothing is injected into
//! Explorer, no Explorer window is subclassed, moved, resized or restyled.
//!
//! ## Threads and cost
//! * [`probe`] is what the UI calls (any thread, typically the gpui UI thread on a 250 ms
//!   timer). It **never** talks to Explorer: it reads the last measurement from a cache and runs
//!   the pure [`place_strip`] on it — a mutex lock and a few comparisons.
//! * The first [`probe`] call starts one background thread ("codenotch-taskbar"), which owns a
//!   COM multithreaded apartment and one `IUIAutomation` instance. While probes keep coming
//!   (within the last 3 s) it re-reads the cheap window state every 250 ms (taskbar window,
//!   rectangle, visibility, DPI, full-screen foreground — no cross-process messages) and the UI
//!   Automation tree plus the auto-hide state at most once per second, or at once when the
//!   taskbar window, its rectangle or its DPI changed (Explorer restart, resolution / DPI / taskbar
//!   size change). Without probes it sleeps until the next one. UI Automation calls carry a
//!   400 ms connection/transaction timeout, so a hung Explorer stalls only that thread.
//! * [`measure`] does one full synchronous measurement on the calling thread (COM initialised and
//!   released around it). For diagnostics and the ignored live test only — never on the UI thread.
//!
//! ## Assumptions and limits
//! * Only the primary taskbar, horizontal, at the bottom or top of the primary monitor.
//!   Vertical (third-party) taskbars, secondary taskbars and custom shells are not supported:
//!   the strip is hidden.
//! * Left-to-right layout. In "left" mode the strip takes the first free gap from the left end,
//!   in "before the tray" mode the last free gap before the notification area; each mode stays
//!   in its own half of the taskbar (a crowded left side never silently moves to the right).
//!   RTL taskbars are not considered.
//! * Explorer's internal layout is not an API: on a future Windows build UI Automation may expose
//!   other elements. When no element can be read the last verified layout is kept for 6 s, then
//!   the strip hides ([`Hidden::NotMeasured`]).

mod uia;
mod watch;

pub use crate::desktop::Rect;
pub use watch::{measure, probe, Measurement};

/// Where the strip goes on the taskbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// First free space from the left end of the taskbar (left half only).
    Left,
    /// Last free space before the system tray (right half only).
    BeforeTray,
}

/// Which screen edge the (horizontal) taskbar sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarEdge {
    Top,
    Bottom,
}

/// Why there is no strip right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hidden {
    /// No `Shell_TrayWnd` (Explorer not running / restarting, custom shell).
    NoTaskbar,
    /// The taskbar is not on the primary monitor.
    NotPrimary,
    /// Vertical, or not flush with the bottom or top edge of its monitor.
    NotHorizontal,
    /// Auto-hide is on, or the taskbar is slid off screen.
    AutoHide,
    /// The taskbar window is not visible.
    TaskbarHidden,
    /// A full-screen application is in front on the primary monitor.
    FullScreen,
    /// No free gap wide enough on the requested side.
    NoSpace,
    /// The occupied spans could not be read (yet, or for more than 6 s).
    NotMeasured,
}

/// The strip's place on the taskbar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Strip rectangle, physical (device) px, virtual-screen coordinates.
    pub rect: Rect,
    /// `rect` width ÷ requested width (≤ 1): how much the whole row must shrink to fit.
    pub fit: f32,
    /// Device px per logical px of the taskbar's monitor (DPI / 96).
    pub dpi: f32,
    pub edge: BarEdge,
}

/// Breathing room kept from occupied spans and the taskbar ends, logical px.
pub const GAP: f32 = 8.0;
/// Strip inset from the taskbar's top and bottom, logical px.
pub const INSET: f32 = 3.0;
/// Narrowest strip worth showing, logical px (a narrower request is honoured as is).
pub const MIN_WIDTH: f32 = 100.0;
/// Edge tolerance when deciding the taskbar is flush with a monitor edge, device px.
const EDGE_SLACK: i32 = 2;
/// Thinnest taskbar treated as shown (an auto-hidden one leaves a 2 px sliver).
const MIN_BAR_HEIGHT: i32 = 20;

fn width(r: &Rect) -> i32 {
    r.right - r.left
}
fn height(r: &Rect) -> i32 {
    r.bottom - r.top
}

/// The edge a taskbar `bar` occupies on `monitor`, or `None` when it is vertical or floating.
pub fn bar_edge(bar: Rect, monitor: Rect) -> Option<BarEdge> {
    if width(&bar) <= height(&bar) || height(&bar) <= 0 {
        return None;
    }
    let spans = bar.left <= monitor.left + EDGE_SLACK && bar.right >= monitor.right - EDGE_SLACK;
    if !spans {
        return None;
    }
    if (bar.bottom - monitor.bottom).abs() <= EDGE_SLACK {
        Some(BarEdge::Bottom)
    } else if (bar.top - monitor.top).abs() <= EDGE_SLACK {
        Some(BarEdge::Top)
    } else {
        None
    }
}

/// Pure: where a strip `wanted` device px wide goes on `bar`, given the `occupied` rectangles
/// (all device px) and the monitor scale `dpi`. Returns the strip rectangle and the fit factor,
/// or `None` when no gap on `side` is at least `min(wanted, 100 logical px)` wide.
///
/// Rules: occupied rects that do not cross the bar, or that are at least half the bar wide
/// (containers, not buttons), are ignored; every other one is padded by 8 logical px and so are
/// the bar ends. `Left` takes the first gap from the left clipped to the left half, aligned left;
/// `BeforeTray` the last gap clipped to the right half, aligned right. The strip is
/// `min(wanted, gap)` wide (so the row shrinks rather than dropping a cell) and spans the bar
/// height less 3 logical px top and bottom.
pub fn place_strip(
    bar: Rect,
    occupied: &[Rect],
    wanted: i32,
    side: Side,
    dpi: f32,
) -> Option<(Rect, f32)> {
    let dpi = if dpi.is_finite() && dpi > 0.0 {
        dpi
    } else {
        1.0
    };
    if wanted <= 0 || width(&bar) <= height(&bar) || height(&bar) <= 0 {
        return None;
    }
    let gap = (GAP * dpi).round() as i32;
    let inset = (INSET * dpi).round() as i32;
    let minimum = wanted.min((MIN_WIDTH * dpi).round() as i32).max(1);
    let mut blocks: Vec<(i32, i32)> = occupied
        .iter()
        .filter(|r| {
            r.bottom > bar.top && r.top < bar.bottom && r.right > bar.left && r.left < bar.right
        })
        .filter(|r| width(r) > 0 && height(r) > 0 && width(r) < width(&bar) / 2)
        .map(|r| ((r.left - gap).max(bar.left), (r.right + gap).min(bar.right)))
        .collect();
    blocks.sort_unstable();
    let mut gaps: Vec<(i32, i32)> = Vec::new();
    let mut edge = bar.left + gap;
    for (left, right) in blocks {
        if left > edge {
            gaps.push((edge, left));
        }
        edge = edge.max(right);
    }
    if bar.right - gap > edge {
        gaps.push((edge, bar.right - gap));
    }
    let middle = bar.left + width(&bar) / 2;
    let clipped = gaps.into_iter().map(|(l, r)| match side {
        Side::Left => (l, r.min(middle)),
        Side::BeforeTray => (l.max(middle), r),
    });
    let mut fitting: Vec<(i32, i32)> = clipped.filter(|(l, r)| r - l >= minimum).collect();
    if side == Side::BeforeTray {
        fitting.reverse();
    }
    let (left, right) = *fitting.first()?;
    let w = wanted.min(right - left);
    let x = match side {
        Side::Left => left,
        Side::BeforeTray => right - w,
    };
    let rect = Rect {
        left: x,
        top: bar.top + inset,
        right: x + w,
        bottom: bar.bottom - inset,
    };
    if height(&rect) <= 0 {
        return None;
    }
    Some((rect, w as f32 / wanted as f32))
}

/// `previous` is still a valid strip: inside the bar's padded ends, same vertical span as a new
/// placement would get, and clear of every (padded) occupied rect.
fn still_free(bar: Rect, occupied: &[Rect], previous: Rect, dpi: f32) -> bool {
    let gap = (GAP * dpi).round() as i32;
    let inset = (INSET * dpi).round() as i32;
    previous.left >= bar.left + gap
        && previous.right <= bar.right - gap
        && previous.top == bar.top + inset
        && previous.bottom == bar.bottom - inset
        && !occupied.iter().any(|r| {
            width(r) < width(&bar) / 2
                && r.bottom > bar.top
                && r.top < bar.bottom
                && r.right + gap > previous.left
                && r.left - gap < previous.right
        })
}

/// [`place_strip`] that keeps `previous` when the new candidate has the same width and
/// `previous` is still free — so a button appearing elsewhere, or Explorer reporting a
/// transitional layout, does not make the strip hop between equally good gaps.
pub fn place_strip_stable(
    bar: Rect,
    occupied: &[Rect],
    wanted: i32,
    side: Side,
    dpi: f32,
    previous: Option<Rect>,
) -> Option<(Rect, f32)> {
    let dpi = if dpi.is_finite() && dpi > 0.0 {
        dpi
    } else {
        1.0
    };
    let candidate = place_strip(bar, occupied, wanted, side, dpi)?;
    if let Some(old) = previous {
        if width(&old) == width(&candidate.0)
            && old != candidate.0
            && still_free(bar, occupied, old, dpi)
        {
            return Some((old, candidate.1));
        }
    }
    Some(candidate)
}

/// Pure: the whole decision from one [`Measurement`]: hide conditions first, then the free gap.
pub fn layout(
    m: &Measurement,
    side: Side,
    wanted: i32,
    previous: Option<Rect>,
) -> Result<Placement, Hidden> {
    let Some(bar) = m.bar else {
        return Err(Hidden::NoTaskbar);
    };
    if !m.primary {
        return Err(Hidden::NotPrimary);
    }
    if !m.visible {
        return Err(Hidden::TaskbarHidden);
    }
    // A slid-away auto-hide bar is no longer flush and would read "not horizontal": check the
    // auto-hide state and the on-screen part first.
    let on_screen =
        bar.top >= m.monitor.top - EDGE_SLACK && bar.bottom <= m.monitor.bottom + EDGE_SLACK;
    if m.auto_hide || !on_screen || height(&bar) < MIN_BAR_HEIGHT {
        return Err(Hidden::AutoHide);
    }
    let edge = bar_edge(bar, m.monitor).ok_or(Hidden::NotHorizontal)?;
    if m.fullscreen {
        return Err(Hidden::FullScreen);
    }
    let occupied = m.occupied.as_deref().ok_or(Hidden::NotMeasured)?;
    let (rect, fit) =
        place_strip_stable(bar, occupied, wanted, side, m.dpi, previous).ok_or(Hidden::NoSpace)?;
    Ok(Placement {
        rect,
        fit,
        dpi: m.dpi,
        edge,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    const BAR: Rect = Rect {
        left: 0,
        top: 1032,
        right: 1920,
        bottom: 1080,
    };

    fn r(left: i32, right: i32) -> Rect {
        Rect { left, right, ..BAR }
    }

    fn measurement(occupied: Vec<Rect>) -> Measurement {
        Measurement {
            bar: Some(BAR),
            monitor: MON,
            primary: true,
            visible: true,
            auto_hide: false,
            dpi: 1.0,
            fullscreen: false,
            occupied: Some(occupied),
        }
    }

    /// Windows 11 default: centred Start/search/apps, widgets at the far left, tray on the right.
    fn centred() -> Vec<Rect> {
        vec![
            r(12, 160),
            r(760, 800),
            r(800, 960),
            r(960, 1160),
            r(1600, 1920),
        ]
    }

    #[test]
    fn centred_taskbar_left_mode_sits_after_widgets() {
        let (s, fit) = place_strip(BAR, &centred(), 200, Side::Left, 1.0).unwrap();
        assert_eq!((s.left, s.right, s.top, s.bottom), (168, 368, 1035, 1077));
        assert_eq!(fit, 1.0);
    }

    #[test]
    fn centred_taskbar_tray_mode_ends_before_the_tray() {
        let (s, _) = place_strip(BAR, &centred(), 200, Side::BeforeTray, 1.0).unwrap();
        assert_eq!((s.left, s.right), (1392, 1592));
    }

    #[test]
    fn left_aligned_taskbar() {
        // Start, search, task view, 6 apps from the left end; tray on the right.
        let occupied = vec![r(0, 48), r(48, 96), r(96, 144), r(144, 432), r(1600, 1920)];
        let (left, _) = place_strip(BAR, &occupied, 200, Side::Left, 1.0).unwrap();
        assert_eq!((left.left, left.right), (440, 640));
        let (right, _) = place_strip(BAR, &occupied, 200, Side::BeforeTray, 1.0).unwrap();
        assert_eq!((right.left, right.right), (1392, 1592));
    }

    #[test]
    fn a_narrow_gap_shrinks_the_strip_instead_of_hiding() {
        // Left gap 168..(760-8)=752 clipped to the middle (960): 584 px for a 700 px row.
        let (s, fit) = place_strip(BAR, &centred(), 700, Side::Left, 1.0).unwrap();
        assert_eq!((s.left, s.right), (168, 752));
        assert!((fit - 584.0 / 700.0).abs() < 1e-6);
    }

    #[test]
    fn insufficient_space_hides() {
        assert!(place_strip(BAR, &[r(0, 900), r(900, 1920)], 200, Side::Left, 1.0).is_none());
        // 90 px free on the left: less than the 100 px minimum.
        let crowded = vec![r(0, 950), r(1048, 1920)];
        assert!(place_strip(BAR, &crowded, 300, Side::Left, 1.0).is_none());
        // A crowded left side does not move the strip to the right half.
        let left_full = vec![r(0, 400), r(400, 800), r(800, 1100), r(1700, 1920)];
        assert!(place_strip(BAR, &left_full, 200, Side::Left, 1.0).is_none());
        assert!(place_strip(BAR, &left_full, 200, Side::BeforeTray, 1.0).is_some());
        // A small request may use a gap smaller than the generic minimum.
        assert!(place_strip(BAR, &[r(0, 900), r(990, 1920)], 60, Side::Left, 1.0).is_none());
        let (s, fit) = place_strip(
            BAR,
            &[r(0, 400), r(490, 900), r(990, 1920)],
            60,
            Side::Left,
            1.0,
        )
        .unwrap();
        assert_eq!((s.left, s.right, fit), (408, 468, 1.0));
    }

    #[test]
    fn containers_and_rects_off_the_bar_are_ignored() {
        let mut occupied = centred();
        occupied.push(Rect {
            left: 0,
            top: 1032,
            right: 1500,
            bottom: 1080,
        }); // task list pane
        occupied.push(Rect {
            left: 200,
            top: 500,
            right: 400,
            bottom: 600,
        }); // not on the bar
        let (s, _) = place_strip(BAR, &occupied, 200, Side::Left, 1.0).unwrap();
        assert_eq!(s.left, 168);
    }

    #[test]
    fn dpi_scales_gaps_insets_and_minimum() {
        // 150 %: bar 72 px tall at the bottom of a 2880 × 1620 monitor.
        let bar = Rect {
            left: 0,
            top: 1548,
            right: 2880,
            bottom: 1620,
        };
        let occ = |l, rr| Rect {
            left: l,
            right: rr,
            ..bar
        };
        let (s, _) = place_strip(
            bar,
            &[occ(18, 240), occ(1140, 1740), occ(2400, 2880)],
            300,
            Side::Left,
            1.5,
        )
        .unwrap();
        assert_eq!((s.left, s.right, s.top, s.bottom), (252, 552, 1553, 1615));
        // Minimum is 150 device px at 150 %: a 140 px gap hides a wide request.
        let rest = |from| [occ(0, 1000), occ(from, 2000), occ(2000, 2880)];
        assert!(place_strip(bar, &rest(1152), 400, Side::Left, 1.5).is_none());
        let (s, fit) = place_strip(bar, &rest(1176), 400, Side::Left, 1.5).unwrap();
        assert_eq!((s.left, s.right), (1012, 1164));
        assert!((fit - 152.0 / 400.0).abs() < 1e-6);
    }

    #[test]
    fn negative_coordinates_and_top_taskbar() {
        let mon = Rect {
            left: -1920,
            top: -1080,
            right: 0,
            bottom: 0,
        };
        let bar = Rect {
            left: -1920,
            top: -1080,
            right: 0,
            bottom: -1032,
        };
        assert_eq!(bar_edge(bar, mon), Some(BarEdge::Top));
        let occupied = [
            Rect {
                left: -1700,
                right: -900,
                ..bar
            },
            Rect {
                left: -900,
                right: 0,
                ..bar
            },
        ];
        let (s, _) = place_strip(bar, &occupied, 280, Side::Left, 1.0).unwrap();
        assert_eq!(
            (s.left, s.right, s.top, s.bottom),
            (-1912, -1708, -1077, -1035)
        );
    }

    #[test]
    fn edges_and_orientation() {
        assert_eq!(bar_edge(BAR, MON), Some(BarEdge::Bottom));
        let vertical = Rect {
            left: 0,
            top: 0,
            right: 62,
            bottom: 1080,
        };
        assert_eq!(bar_edge(vertical, MON), None);
        let floating = Rect {
            left: 0,
            top: 500,
            right: 1920,
            bottom: 548,
        };
        assert_eq!(bar_edge(floating, MON), None);
        let short = Rect {
            left: 300,
            top: 1032,
            right: 1600,
            bottom: 1080,
        };
        assert_eq!(bar_edge(short, MON), None);
    }

    #[test]
    fn stable_placement_survives_unrelated_changes() {
        let prev = Rect {
            left: 300,
            top: 1035,
            right: 500,
            bottom: 1077,
        };
        let occupied = vec![r(700, 1200), r(1720, 1920)];
        assert_eq!(
            place_strip_stable(BAR, &occupied, 200, Side::Left, 1.0, Some(prev))
                .unwrap()
                .0,
            prev
        );
        let new_buttons = vec![r(300, 600), r(700, 1200), r(1720, 1920)];
        let moved = place_strip_stable(BAR, &new_buttons, 200, Side::Left, 1.0, Some(prev))
            .unwrap()
            .0;
        assert_eq!((moved.left, moved.right), (8, 208));
        // A different width is placed afresh.
        let wider = place_strip_stable(BAR, &occupied, 250, Side::Left, 1.0, Some(prev))
            .unwrap()
            .0;
        assert_eq!(wider.left, 8);
    }

    #[test]
    fn hide_conditions() {
        let ok = measurement(centred());
        assert!(layout(&ok, Side::Left, 200, None).is_ok());
        let p = layout(&ok, Side::Left, 200, None).unwrap();
        assert_eq!(p.edge, BarEdge::Bottom);
        let cases: Vec<(Measurement, Hidden)> = vec![
            (
                Measurement {
                    bar: None,
                    ..ok.clone()
                },
                Hidden::NoTaskbar,
            ),
            (
                Measurement {
                    primary: false,
                    ..ok.clone()
                },
                Hidden::NotPrimary,
            ),
            (
                Measurement {
                    visible: false,
                    ..ok.clone()
                },
                Hidden::TaskbarHidden,
            ),
            (
                Measurement {
                    auto_hide: true,
                    ..ok.clone()
                },
                Hidden::AutoHide,
            ),
            (
                Measurement {
                    bar: Some(Rect {
                        top: 1078,
                        bottom: 1126,
                        ..BAR
                    }),
                    ..ok.clone()
                },
                Hidden::AutoHide,
            ),
            (
                Measurement {
                    bar: Some(Rect {
                        left: 0,
                        top: 0,
                        right: 62,
                        bottom: 1080,
                    }),
                    ..ok.clone()
                },
                Hidden::NotHorizontal,
            ),
            (
                Measurement {
                    fullscreen: true,
                    ..ok.clone()
                },
                Hidden::FullScreen,
            ),
            (
                Measurement {
                    occupied: None,
                    ..ok.clone()
                },
                Hidden::NotMeasured,
            ),
            (
                Measurement {
                    occupied: Some(vec![r(0, 900), r(900, 1920)]),
                    ..ok.clone()
                },
                Hidden::NoSpace,
            ),
        ];
        for (m, why) in cases {
            assert_eq!(layout(&m, Side::Left, 200, None), Err(why));
        }
    }

    #[test]
    fn invalid_input_is_rejected() {
        assert!(place_strip(BAR, &[], 0, Side::Left, 1.0).is_none());
        assert!(place_strip(BAR, &[], -5, Side::Left, 1.0).is_none());
        let (s, _) = place_strip(BAR, &[], 200, Side::Left, f32::NAN).unwrap();
        assert_eq!(s.left, 8);
        let degenerate = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        assert!(place_strip(degenerate, &[], 200, Side::Left, 1.0).is_none());
    }
}
