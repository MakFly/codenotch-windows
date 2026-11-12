//! Stacked area chart in the shadcn/ui "Area Chart - Interactive" style, drawn on a gpui
//! canvas: one band per series (vertical gradient fill from 80 % to 10 % of the series colour,
//! 1.5 px stroke, monotone curve that never overshoots), faint horizontal grid lines (`border`/50,
//! no axis lines) on "nice" values with compact, subtle y labels in a left gutter, muted x labels with a minimum gap.
//! The last point can be marked partial (today): its segment is drawn lighter and dashed.
//! With few points (≤ [`DOT_MAX_POINTS`]) every point gets a small dot. Hovering shows a
//! cursor line at the nearest point and a tooltip card that follows the pointer (and flips to
//! the left near the right edge).
//!
//! The geometry (stacking, scale, nearest point, tick choice, monotone smoothing) lives in
//! pure functions below, unit tested.

use std::rc::Rc;

use gpui::{
    canvas, div, linear_color_stop, linear_gradient, point, px, relative, App, Bounds, ElementId,
    Hsla, InteractiveElement, IntoElement, MouseMoveEvent, ParentElement, PathBuilder, Pixels,
    Point, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
};

use crate::tooltip::{tooltip_card, TipContent, TipRow};
use crate::Theme;

/// Height of the plot area (without the x labels).
pub const CHART_H: f32 = 250.0;
/// Minimum distance between two x labels.
pub const MIN_TICK_GAP: f32 = 56.0;
/// Room above the highest point so the stroke and the top y label are never clipped.
const TOP_PAD: f32 = 8.0;
/// Points drawn as dots up to this many points per series.
pub const DOT_MAX_POINTS: usize = 14;
/// Y label size and the gap between the gutter and the plot.
const Y_LABEL_SIZE: f32 = 11.0;
const Y_GUTTER_GAP: f32 = 8.0;
/// Opacity factor of the partial (last) segment's fill.
const PARTIAL_FILL: f32 = 0.45;
/// Tooltip width used to decide when it flips to the left of the cursor.
const TIP_W: f32 = 200.0;

// ---------------------------------------------------------------------------------------------
// Pure geometry
// ---------------------------------------------------------------------------------------------

/// Cumulative tops of stacked series: `out[k][i]` = sum of `series[0..=k][i]` (missing values
/// count as 0; every row has the length of the longest series).
pub fn stack(series: &[&[f64]]) -> Vec<Vec<f64>> {
    let n = series.iter().map(|s| s.len()).max().unwrap_or(0);
    let mut acc = vec![0.0; n];
    series
        .iter()
        .map(|s| {
            for (i, a) in acc.iter_mut().enumerate() {
                let v = s.get(i).copied().unwrap_or(0.0);
                *a += if v.is_finite() && v > 0.0 { v } else { 0.0 };
            }
            acc.clone()
        })
        .collect()
}

/// Smallest "nice" number (1, 2, 2.5 or 5 × 10^k) ≥ `max`; 1 for an empty / zero chart.
pub fn nice_ceiling(max: f64) -> f64 {
    if !max.is_finite() || max <= 0.0 {
        return 1.0;
    }
    let mag = 10f64.powf(max.log10().floor());
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        let v = m * mag;
        if v >= max * (1.0 - 1e-9) {
            return v;
        }
    }
    10.0 * mag
}

/// Y scale of a chart whose highest stacked value is `max`: `(top, intervals)` with a "nice"
/// step (1, 2, 2.5 or 5 × 10^k) and 2 or 3 intervals (3 or 4 grid lines / labels), the
/// tightest top of the candidates. `(1, 2)` for an empty / zero chart.
pub fn nice_scale(max: f64) -> (f64, usize) {
    if !max.is_finite() || max <= 0.0 {
        return (1.0, 2);
    }
    let mut best: Option<(f64, usize)> = None;
    for target in [3.0, 4.0] {
        let step = nice_ceiling(max / target);
        let n = ((max / step) * (1.0 - 1e-9)).ceil().max(1.0) as usize;
        let top = step * n as f64;
        if n > 3 {
            continue;
        }
        let n = n.max(2);
        if best.is_none_or(|(t, _)| top < t - 1e-9) {
            best = Some((top, n));
        }
    }
    best.unwrap_or((1.0, 2))
}

/// X of point `i` of `n` across `width` (first at 0, last at `width`; a single point in the
/// middle).
pub fn x_at(i: usize, n: usize, width: f32) -> f32 {
    if n <= 1 {
        width / 2.0
    } else {
        width * i as f32 / (n - 1) as f32
    }
}

/// Index of the point nearest to `x` (clamped into the chart).
pub fn nearest_index(x: f32, n: usize, width: f32) -> Option<usize> {
    if n == 0 {
        return None;
    }
    if n == 1 || width <= 0.0 {
        return Some(0);
    }
    let step = width / (n - 1) as f32;
    let i = (x / step).round();
    Some((i.max(0.0) as usize).min(n - 1))
}

/// Indices that get an x label: the last point always, then every `step` points backwards,
/// `step` chosen so two labels are at least `min_gap` apart.
pub fn tick_indices(n: usize, width: f32, min_gap: f32) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![0];
    }
    let spacing = (width / (n - 1) as f32).max(0.001);
    let step = ((min_gap / spacing).ceil() as usize).max(1);
    let mut v: Vec<usize> = (0..n).rev().step_by(step).collect();
    v.reverse();
    v
}

/// Control points of a monotone cubic through `pts` (d3 `curveMonotoneX`, Fritsch–Carlson):
/// one `(control_a, control_b)` pair per segment `pts[k] → pts[k + 1]`. Never overshoots, so
/// a series never dips below zero or above its neighbours. `pts` must have increasing x.
pub fn monotone_controls(pts: &[(f32, f32)]) -> Vec<((f32, f32), (f32, f32))> {
    let n = pts.len();
    if n < 2 {
        return Vec::new();
    }
    let h: Vec<f32> = pts.windows(2).map(|w| w[1].0 - w[0].0).collect();
    let s: Vec<f32> = pts
        .windows(2)
        .zip(&h)
        .map(|(w, h)| {
            if *h == 0.0 {
                0.0
            } else {
                (w[1].1 - w[0].1) / h
            }
        })
        .collect();
    let sign = |v: f32| {
        if v > 0.0 {
            1.0
        } else if v < 0.0 {
            -1.0
        } else {
            0.0
        }
    };
    let mut t = vec![0.0f32; n];
    for k in 1..n - 1 {
        let (h0, h1, s0, s1) = (h[k - 1], h[k], s[k - 1], s[k]);
        let p = if h0 + h1 == 0.0 {
            0.0
        } else {
            (s0 * h1 + s1 * h0) / (h0 + h1)
        };
        t[k] = (sign(s0) + sign(s1)) * s0.abs().min(s1.abs()).min(0.5 * p.abs());
    }
    // Ends: one-sided three-point estimate (d3 `slope2`), or the secant for two points.
    if n == 2 {
        t[0] = s[0];
        t[1] = s[0];
    } else {
        let end = |hk: f32, sk: f32, tk: f32| {
            if hk == 0.0 {
                tk
            } else {
                (3.0 * sk - tk) / 2.0
            }
        };
        t[0] = end(h[0], s[0], t[1]);
        t[n - 1] = end(h[n - 2], s[n - 2], t[n - 2]);
    }
    (0..n - 1)
        .map(|k| {
            let (x0, y0) = pts[k];
            let (x1, y1) = pts[k + 1];
            let d = (x1 - x0) / 3.0;
            ((x0 + d, y0 + d * t[k]), (x1 - d, y1 - d * t[k + 1]))
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Element
// ---------------------------------------------------------------------------------------------

/// One stacked band.
#[derive(Clone)]
pub struct ChartSeries {
    pub name: SharedString,
    pub color: Hsla,
    /// One value per x point (≥ 0).
    pub values: Vec<f64>,
}

/// Value formatter (tooltip figures).
pub type ChartFormat = Rc<dyn Fn(f64) -> String>;

/// Hover state kept across frames (keyed by the chart id).
#[derive(Default)]
struct ChartState {
    hover: Option<usize>,
    /// Plot bounds of the last paint (window coordinates).
    bounds: Option<Bounds<Pixels>>,
}

/// Stacked area chart. `series[0]` is drawn at the bottom; the tooltip lists them top first.
#[derive(IntoElement)]
pub struct AreaChart {
    id: ElementId,
    theme: Theme,
    series: Vec<ChartSeries>,
    /// Short x label per point ("4 nov.").
    labels: Vec<SharedString>,
    /// Tooltip header per point (defaults to the label).
    titles: Vec<SharedString>,
    format: ChartFormat,
    /// Label of the tooltip's total row (shown with more than one series).
    total_label: Option<SharedString>,
    height: f32,
    /// Plot width assumed before the first paint (tick density).
    width_hint: f32,
    /// Y label formatter (no y labels when `None`).
    axis_format: Option<ChartFormat>,
    /// The last point is still accumulating (today): draw its segment lighter and dashed.
    partial_last: bool,
}

impl AreaChart {
    pub fn new(
        id: impl Into<ElementId>,
        theme: Theme,
        labels: Vec<SharedString>,
        series: Vec<ChartSeries>,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            series,
            titles: labels.clone(),
            labels,
            format: Rc::new(|v| format!("{v}")),
            total_label: None,
            height: CHART_H,
            width_hint: 600.0,
            axis_format: None,
            partial_last: false,
        }
    }
    /// Compact formatter of the y labels ("$50", "10M", "12").
    pub fn axis_format(mut self, f: impl Fn(f64) -> String + 'static) -> Self {
        self.axis_format = Some(Rc::new(f));
        self
    }
    /// Mark the last point as partial (today, still accumulating).
    pub fn partial_last(mut self, partial: bool) -> Self {
        self.partial_last = partial;
        self
    }
    pub fn titles(mut self, titles: Vec<SharedString>) -> Self {
        self.titles = titles;
        self
    }
    pub fn format(mut self, f: impl Fn(f64) -> String + 'static) -> Self {
        self.format = Rc::new(f);
        self
    }
    pub fn total_label(mut self, label: impl Into<SharedString>) -> Self {
        self.total_label = Some(label.into());
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    /// Expected width of the whole chart (y gutter included), used for the x labels until the
    /// chart has been painted once.
    pub fn width_hint(mut self, w: f32) -> Self {
        self.width_hint = w;
        self
    }
}

fn pt(origin: Point<Pixels>, x: f32, y: f32) -> Point<Pixels> {
    point(origin.x + px(x), origin.y + px(y))
}

/// The two control points of one cubic segment.
type Controls = ((f32, f32), (f32, f32));
/// A band top to stroke: its points, their controls and the colour.
type Stroke = (Vec<(f32, f32)>, Vec<Controls>, Hsla);

/// Append segments `range` of a curve through `pts` with the precomputed `ctrl` (the path is
/// already at `pts[range.start]`).
fn curve_segments(
    b: &mut PathBuilder,
    o: Point<Pixels>,
    pts: &[(f32, f32)],
    ctrl: &[Controls],
    range: std::ops::Range<usize>,
) {
    for k in range {
        let (ca, cb) = ctrl[k];
        let to = pts[k + 1];
        b.cubic_bezier_to(pt(o, to.0, to.1), pt(o, ca.0, ca.1), pt(o, cb.0, cb.1));
    }
}

/// Same, walked backwards (from `pts[range.end]` down to `pts[range.start]`).
fn curve_segments_back(
    b: &mut PathBuilder,
    o: Point<Pixels>,
    pts: &[(f32, f32)],
    ctrl: &[Controls],
    range: std::ops::Range<usize>,
) {
    for k in range.rev() {
        let (ca, cb) = ctrl[k];
        let to = pts[k];
        b.cubic_bezier_to(pt(o, to.0, to.1), pt(o, cb.0, cb.1), pt(o, ca.0, ca.1));
    }
}

/// Everything `paint_chart` needs.
struct Plot<'a> {
    series: &'a [ChartSeries],
    top: f64,
    intervals: usize,
    partial_last: bool,
}

/// Paint grid, bands and strokes inside `bounds`.
fn paint_chart(bounds: Bounds<Pixels>, t: &Theme, p: &Plot, window: &mut Window) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let o = bounds.origin;
    let plot_h = (h - TOP_PAD).max(1.0);
    // Grid: one line per tick, the last one on the baseline.
    for g in 0..=p.intervals {
        let y = TOP_PAD + plot_h * g as f32 / p.intervals.max(1) as f32;
        let y = y.min(h - 1.0);
        window.paint_quad(gpui::fill(
            Bounds::new(pt(o, 0.0, y), gpui::size(px(w), px(1.0))),
            // shadcn chart grid: `stroke-border/50`.
            t.border.opacity(0.5),
        ));
    }
    let values: Vec<&[f64]> = p.series.iter().map(|s| s.values.as_slice()).collect();
    let tops = stack(&values);
    let n = tops.first().map_or(0, |v| v.len());
    if n == 0 {
        return;
    }
    let max = p.top;
    let y_of = |v: f64| TOP_PAD + plot_h * (1.0 - (v / max) as f32);
    // A single day spans the whole width.
    let xs: Vec<f32> = if n == 1 {
        vec![0.0, w]
    } else {
        (0..n).map(|i| x_at(i, n, w)).collect()
    };
    let line = |vals: &[f64]| -> Vec<(f32, f32)> {
        if n == 1 {
            vec![(xs[0], y_of(vals[0])), (xs[1], y_of(vals[0]))]
        } else {
            xs.iter().zip(vals).map(|(x, v)| (*x, y_of(*v))).collect()
        }
    };
    let segs = xs.len() - 1;
    // Segments drawn solid; the last one is partial when asked (and there is more than one).
    let solid = if p.partial_last && n >= 3 {
        segs - 1
    } else {
        segs
    };
    let zero = vec![0.0; n];
    let mut strokes: Vec<Stroke> = Vec::new();
    for (k, s) in p.series.iter().enumerate() {
        let top = line(&tops[k]);
        let bottom = line(if k == 0 { &zero } else { &tops[k - 1] });
        if top
            .iter()
            .zip(&bottom)
            .all(|(a, b)| (a.1 - b.1).abs() < 0.01)
        {
            continue;
        }
        let top_c = monotone_controls(&top);
        let bottom_c = monotone_controls(&bottom);
        let band = |range: std::ops::Range<usize>, alpha: f32, window: &mut Window| {
            let mut b = PathBuilder::fill();
            b.move_to(pt(o, top[range.start].0, top[range.start].1));
            curve_segments(&mut b, o, &top, &top_c, range.clone());
            b.line_to(pt(o, bottom[range.end].0, bottom[range.end].1));
            curve_segments_back(&mut b, o, &bottom, &bottom_c, range);
            b.close();
            if let Ok(path) = b.build() {
                window.paint_path(
                    path,
                    linear_gradient(
                        180.0,
                        linear_color_stop(s.color.opacity(0.8 * alpha), 0.0),
                        linear_color_stop(s.color.opacity(0.1 * alpha), 1.0),
                    ),
                );
            }
        };
        if solid > 0 {
            band(0..solid, 1.0, window);
        }
        if solid < segs {
            band(solid..segs, PARTIAL_FILL, window);
        }
        strokes.push((top, top_c, s.color));
    }
    // Point dots when there are few points (painted here, with the current bounds).
    if n > 1 && n <= DOT_MAX_POINTS {
        for (k, s) in p.series.iter().enumerate() {
            for i in 0..n {
                let v = s.values.get(i).copied().unwrap_or(0.0);
                if v <= 0.0 && p.series.len() > 1 {
                    continue;
                }
                let (x, y) = (xs[i], y_of(tops[k][i]));
                window.paint_quad(
                    gpui::fill(
                        Bounds::new(pt(o, x - 2.5, y - 2.5), gpui::size(px(5.), px(5.))),
                        s.color,
                    )
                    .corner_radii(px(2.5)),
                );
            }
        }
    }
    for (top, ctrl, color) in strokes {
        if solid > 0 {
            let mut b = PathBuilder::stroke(px(1.5));
            b.move_to(pt(o, top[0].0, top[0].1));
            curve_segments(&mut b, o, &top, &ctrl, 0..solid);
            if let Ok(path) = b.build() {
                window.paint_path(path, color);
            }
        }
        if solid < segs {
            let mut b = PathBuilder::stroke(px(1.5)).dash_array(&[px(4.), px(3.)]);
            b.move_to(pt(o, top[solid].0, top[solid].1));
            curve_segments(&mut b, o, &top, &ctrl, solid..segs);
            if let Ok(path) = b.build() {
                window.paint_path(path, color.opacity(0.8));
            }
        }
    }
}

impl RenderOnce for AreaChart {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = self.theme;
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| ChartState::default());
        let (hover, bounds) = {
            let s = state.read(cx);
            (s.hover, s.bounds)
        };
        let n = self
            .series
            .iter()
            .map(|s| s.values.len())
            .max()
            .unwrap_or(0);
        let hover = hover.filter(|i| *i < n);
        let plot_h = (self.height - TOP_PAD).max(1.0);
        let values: Vec<&[f64]> = self.series.iter().map(|s| s.values.as_slice()).collect();
        let tops = stack(&values);
        let (max, intervals) = nice_scale(
            tops.last()
                .map_or(0.0, |v| v.iter().cloned().fold(0.0, f64::max)),
        );
        // Y labels (top first) and the gutter they need.
        let y_labels: Vec<String> = self.axis_format.as_ref().map_or_else(Vec::new, |f| {
            (0..=intervals)
                .map(|g| f(max * (intervals - g) as f64 / intervals as f64))
                .collect()
        });
        let gutter = y_labels
            .iter()
            .map(|l| l.chars().count() as f32 * Y_LABEL_SIZE * 0.6)
            .fold(0.0, f32::max)
            .ceil();
        let gutter = if y_labels.is_empty() {
            0.0
        } else {
            gutter + Y_GUTTER_GAP
        };
        let width = bounds.map_or(self.width_hint - gutter, |b| f32::from(b.size.width));

        let series = self.series.clone();
        let partial_last = self.partial_last;
        let paint_state = state.clone();
        let plot = canvas(
            move |b, _, cx| {
                paint_state.update(cx, |s, _| s.bounds = Some(b));
            },
            move |b, _, window, _| {
                let p = Plot {
                    series: &series,
                    top: max,
                    intervals,
                    partial_last,
                };
                paint_chart(b, &t, &p, window)
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let move_state = state.clone();
        let leave_state = state.clone();
        let mut area = div()
            .id(self.id.clone())
            .relative()
            .w_full()
            .h(px(self.height))
            .child(plot)
            .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
                let Some(b) = move_state.read(cx).bounds else {
                    return;
                };
                let x = f32::from(e.position.x - b.origin.x);
                let i = nearest_index(x, n, f32::from(b.size.width));
                if move_state.read(cx).hover != i {
                    move_state.update(cx, |s, cx| {
                        s.hover = i;
                        cx.notify();
                    });
                }
            })
            .on_hover(move |h, _, cx| {
                if !*h && leave_state.read(cx).hover.is_some() {
                    leave_state.update(cx, |s, cx| {
                        s.hover = None;
                        cx.notify();
                    });
                }
            });

        if let Some(i) = hover {
            let x = x_at(i, n, width);
            // Cursor line and one dot per series on its band top.
            area = area.child(
                div()
                    .absolute()
                    .top(px(TOP_PAD))
                    .bottom_0()
                    .left(px(x - 0.5))
                    .w(px(1.))
                    .bg(t.muted_foreground.opacity(0.5)),
            );
            for (k, s) in self.series.iter().enumerate() {
                if s.values.get(i).copied().unwrap_or(0.0) <= 0.0 {
                    continue;
                }
                let y = TOP_PAD + plot_h * (1.0 - (tops[k][i] / max) as f32);
                area = area.child(
                    div()
                        .absolute()
                        .left(px(x - 4.))
                        .top(px(y - 4.))
                        .size(px(8.))
                        .rounded_full()
                        .border_2()
                        .border_color(t.card)
                        .bg(s.color),
                );
            }
            area = area.child(self.tooltip(i, x, width));
        }

        // X labels: the last point always, then every step backwards.
        let ticks = tick_indices(n, width, MIN_TICK_GAP);
        let mut axis = div().relative().w_full().h(px(20.)).mt(px(8.));
        for i in ticks {
            let label = self.labels.get(i).cloned().unwrap_or_default();
            let cell = div()
                .absolute()
                .top_0()
                .w(px(MIN_TICK_GAP))
                .text_size(px(12.))
                .text_color(t.muted_foreground)
                .whitespace_nowrap();
            let cell = if n <= 1 {
                cell.left(relative(0.5))
                    .ml(px(-MIN_TICK_GAP / 2.))
                    .flex()
                    .justify_center()
            } else if i == 0 {
                cell.left_0()
            } else if i == n - 1 {
                cell.right_0().flex().justify_end()
            } else {
                cell.left(relative(i as f32 / (n - 1) as f32))
                    .ml(px(-MIN_TICK_GAP / 2.))
                    .flex()
                    .justify_center()
            };
            axis = axis.child(cell.child(label));
        }

        let mut gutter_col = div()
            .flex_none()
            .relative()
            .w(px(gutter))
            .h(px(self.height));
        for (g, label) in y_labels.into_iter().enumerate() {
            let y = TOP_PAD + plot_h * g as f32 / intervals as f32;
            gutter_col = gutter_col.child(
                div()
                    .absolute()
                    .left_0()
                    .w(px(gutter - Y_GUTTER_GAP))
                    .top(px(y - 8.))
                    .h(px(16.))
                    .flex()
                    .items_center()
                    .justify_end()
                    .text_size(px(Y_LABEL_SIZE))
                    // Subtle: the x labels and the tooltip carry the figures.
                    .text_color(t.muted_foreground.opacity(0.75))
                    .whitespace_nowrap()
                    .child(label),
            );
        }
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(
                div()
                    .flex()
                    .w_full()
                    .child(gutter_col)
                    .child(div().flex_1().min_w(px(0.)).child(area)),
            )
            .child(div().pl(px(gutter)).child(axis))
    }
}

impl AreaChart {
    /// The hover card at point `i` (x = `x` in a plot `width` wide): the shared
    /// [`tooltip_card`](crate::tooltip::tooltip_card), series top first, then the total.
    fn tooltip(&self, i: usize, x: f32, width: f32) -> gpui::Div {
        let title = self
            .titles
            .get(i)
            .or_else(|| self.labels.get(i))
            .cloned()
            .unwrap_or_default();
        let mut rows = Vec::new();
        let mut total = 0.0;
        for s in self.series.iter().rev() {
            let v = s.values.get(i).copied().unwrap_or(0.0);
            total += v.max(0.0);
            rows.push(TipRow::new(s.name.clone(), (self.format)(v)).swatch(s.color));
        }
        if self.series.len() > 1 {
            if let Some(label) = &self.total_label {
                rows.push(TipRow::new(label.clone(), (self.format)(total)).strong());
            }
        }
        let card = tooltip_card(&self.theme, &TipContent::new(title, rows))
            .absolute()
            .top(px(TOP_PAD));
        if x > width - TIP_W - 12.0 && x > TIP_W {
            card.right(px(width - x + 12.))
        } else {
            card.left(px(x + 12.))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacking_adds_up_and_ignores_bad_values() {
        let a = [1.0, 2.0, 3.0];
        let b = [10.0, f64::NAN, -5.0];
        let c = [1.0];
        let s = stack(&[&a, &b, &c]);
        assert_eq!(s[0], vec![1.0, 2.0, 3.0]);
        assert_eq!(s[1], vec![11.0, 2.0, 3.0]);
        assert_eq!(s[2], vec![12.0, 2.0, 3.0]);
        assert!(stack(&[]).is_empty());
    }

    #[test]
    fn nice_ceilings() {
        assert_eq!(nice_ceiling(0.0), 1.0);
        assert_eq!(nice_ceiling(f64::NAN), 1.0);
        assert_eq!(nice_ceiling(7.0), 10.0);
        assert_eq!(nice_ceiling(10.0), 10.0);
        assert_eq!(nice_ceiling(12.0), 20.0);
        assert_eq!(nice_ceiling(21.0), 25.0);
        assert_eq!(nice_ceiling(3_400_000.0), 5_000_000.0);
        assert!((nice_ceiling(0.13) - 0.2).abs() < 1e-12);
    }

    #[test]
    fn nice_scales_have_three_or_four_ticks() {
        assert_eq!(nice_scale(0.0), (1.0, 2));
        assert_eq!(nice_scale(f64::NAN), (1.0, 2));
        assert_eq!(nice_scale(250.0), (300.0, 3));
        assert_eq!(nice_scale(90.0), (100.0, 2));
        assert_eq!(nice_scale(12_400_000.0), (15_000_000.0, 3));
        assert_eq!(nice_scale(7.0), (7.5, 3));
        for max in [0.37, 3.0, 13.0, 101.0, 999.0, 4_567.0, 39_500_000.0] {
            let (top, n) = nice_scale(max);
            assert!(top >= max && (2..=3).contains(&n), "{max}: {top} / {n}");
            // Reasonably tight: the data fills at least half the height.
            assert!(max / top >= 0.5, "{max}: {top}");
        }
    }

    #[test]
    fn x_positions_and_nearest_point() {
        assert_eq!(x_at(0, 5, 400.0), 0.0);
        assert_eq!(x_at(4, 5, 400.0), 400.0);
        assert_eq!(x_at(0, 1, 400.0), 200.0);
        assert_eq!(nearest_index(0.0, 5, 400.0), Some(0));
        assert_eq!(nearest_index(49.0, 5, 400.0), Some(0));
        assert_eq!(nearest_index(51.0, 5, 400.0), Some(1));
        assert_eq!(nearest_index(-30.0, 5, 400.0), Some(0));
        assert_eq!(nearest_index(999.0, 5, 400.0), Some(4));
        assert_eq!(nearest_index(10.0, 0, 400.0), None);
        assert_eq!(nearest_index(10.0, 1, 400.0), Some(0));
    }

    #[test]
    fn ticks_keep_the_last_point_and_a_min_gap() {
        // 7 points over 600 px: 100 px apart, every point fits.
        assert_eq!(tick_indices(7, 600.0, 56.0), vec![0, 1, 2, 3, 4, 5, 6]);
        // 30 points over 600 px: ~20.7 px apart → every 3rd, ending on the last.
        let t = tick_indices(30, 600.0, 56.0);
        assert_eq!(t.last(), Some(&29));
        assert!(t.windows(2).all(|w| w[1] - w[0] == 3));
        assert_eq!(t.first(), Some(&2));
        assert_eq!(tick_indices(1, 600.0, 56.0), vec![0]);
        assert!(tick_indices(0, 600.0, 56.0).is_empty());
    }

    #[test]
    fn monotone_curve_never_overshoots() {
        let pts = [
            (0.0, 10.0),
            (10.0, 0.0),
            (20.0, 0.0),
            (30.0, 5.0),
            (40.0, 5.0),
        ];
        let c = monotone_controls(&pts);
        assert_eq!(c.len(), 4);
        for (k, (a, b)) in c.iter().enumerate() {
            let (lo, hi) = (pts[k].1.min(pts[k + 1].1), pts[k].1.max(pts[k + 1].1));
            for y in [a.1, b.1] {
                assert!(y >= lo - 1e-4 && y <= hi + 1e-4, "segment {k}: {y}");
            }
            assert!(a.0 > pts[k].0 && b.0 < pts[k + 1].0);
        }
        // Flat runs stay flat.
        assert_eq!(c[1], ((10.0 + 10.0 / 3.0, 0.0), (20.0 - 10.0 / 3.0, 0.0)));
        // A straight line stays straight.
        let line = monotone_controls(&[(0.0, 0.0), (3.0, 3.0), (6.0, 6.0)]);
        assert_eq!(line[0], ((1.0, 1.0), (2.0, 2.0)));
        assert!(monotone_controls(&[(0.0, 1.0)]).is_empty());
        assert_eq!(
            monotone_controls(&[(0.0, 0.0), (3.0, 3.0)])[0],
            ((1.0, 1.0), (2.0, 2.0))
        );
    }
}
