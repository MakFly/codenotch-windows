//! The budget chart of a quota window: its whole period from start to reset, the even pace
//! as a dashed diagonal, the readings as a smooth line, and the average pace projected to
//! the reset, with a tooltip that follows the pointer over the readings.
use crate::analytics::Budget;
use gpui::{
    canvas, div, fill, linear_color_stop, linear_gradient, point, prelude::*, px, relative, rgb, rgba,
    size, AnyElement, App, Bounds, FontWeight, MouseMoveEvent, PathBuilder, Pixels, Rgba,
    SharedString, Window,
};
use std::{cell::Cell, rc::Rc};

const HEIGHT: f32 = 92.;
/// The settings surface behind the chart, to ring the dots.
const SURFACE: u32 = 0x0e0e10;

/// One reading of the current period.
#[derive(Clone, Copy)]
pub struct Point {
    pub at: i64,
    pub value: f64,
}

pub type OnHover = Rc<dyn Fn(Option<usize>, &mut Window, &mut App)>;

/// Chart tones for the usage bands of `usage_color`: same meanings, softened for large
/// areas on a dark surface (pure yellow turns olive once it is translucent).
pub fn tone(value: Option<f64>) -> Rgba {
    match value {
        None => rgb(0x8a8a93),
        Some(v) if v < 50. => rgb(0x34d399),
        Some(v) if v < 80. => rgb(0xf5b83d),
        Some(_) => rgb(0xf9735b),
    }
}

pub struct BudgetChart {
    pub id: SharedString,
    pub budget: Budget,
    /// Usage expected at the reset, and when it reaches 100 % if before.
    pub expected: f64,
    pub exhausts_at: Option<i64>,
    /// Lowest and highest usage at the reset, drawn as a bar at the reset.
    pub range: Option<(f64, f64)>,
    pub points: Vec<Point>,
    pub color: Rgba,
    /// Series name in the tooltip.
    pub label: String,
    pub hovered: Option<usize>,
    pub on_hover: OnHover,
    pub value_text: Rc<dyn Fn(f64) -> String>,
    pub time_text: Rc<dyn Fn(i64) -> String>,
    /// Captions under the start and the end of the axis.
    pub start_caption: String,
    pub end_caption: String,
}

fn with_alpha(color: Rgba, a: f32) -> Rgba {
    Rgba { a, ..color }
}

/// A smooth curve through the points that never overshoots them: each step is a cubic whose
/// control points sit level with its ends, halfway across (d3's monotone-x look).
fn smooth(builder: &mut PathBuilder, points: &[gpui::Point<Pixels>]) {
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let half = (b.x - a.x) / 2.;
        builder.cubic_bezier_to(b, point(a.x + half, a.y), point(b.x - half, b.y));
    }
}

fn stroke(window: &mut Window, width: f32, dash: Option<&[Pixels]>, from: gpui::Point<Pixels>, to: gpui::Point<Pixels>, color: Rgba) {
    let mut builder = PathBuilder::stroke(px(width));
    if let Some(dash) = dash {
        builder = builder.dash_array(dash);
    }
    builder.move_to(from);
    builder.line_to(to);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn dot(window: &mut Window, center: gpui::Point<Pixels>, color: Rgba) {
    for (radius, fill_color) in [(5.5, rgb(SURFACE)), (3.5, color)] {
        window.paint_quad(
            fill(
                Bounds {
                    origin: point(center.x - px(radius), center.y - px(radius)),
                    size: size(px(radius * 2.), px(radius * 2.)),
                },
                fill_color,
            )
            .corner_radii(px(radius)),
        );
    }
}

impl BudgetChart {
    fn x(&self, at: i64) -> f32 {
        let b = &self.budget;
        ((at - b.start) as f32 / (b.end - b.start).max(1) as f32).clamp(0., 1.)
    }

    pub fn render(self) -> AnyElement {
        let bounds_cell: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let xs: Vec<f32> = self.points.iter().map(|p| self.x(p.at)).collect();
        let values: Vec<(f32, f32)> = self
            .points
            .iter()
            .zip(&xs)
            .map(|(p, x)| (*x, (p.value / 100.).clamp(0., 1.) as f32))
            .collect();
        let budget = self.budget;
        let now_x = self.x(budget.at);
        // The forecast ends at the reset, or where it reaches 100 %.
        let exhausts_at = self.exhausts_at;
        let projection_end = match exhausts_at {
            Some(at) => (self.x(at), 1.),
            None => (1., (self.expected / 100.).clamp(0., 1.) as f32),
        };
        let range = self
            .range
            .filter(|_| exhausts_at.is_none())
            .map(|(low, high)| ((low / 100.).clamp(0., 1.) as f32, (high / 100.).clamp(0., 1.) as f32));
        let color = self.color;
        let hovered = self.hovered.filter(|i| *i < self.points.len());

        let painter = {
            let bounds_cell = bounds_cell.clone();
            canvas(
                move |bounds, _, _| bounds_cell.set(Some(bounds)),
                move |bounds, _, window, _| {
                    let (w, h) = (bounds.size.width, bounds.size.height);
                    let at = |x: f32, y: f32| point(bounds.origin.x + w * x, bounds.origin.y + h * (1. - y));
                    // The limit, the floor, and the even pace between them.
                    stroke(window, 1., Some(&[px(2.), px(4.)]), at(0., 1.), at(1., 1.), rgba(0xffffff24));
                    stroke(window, 1., None, at(0., 0.), at(1., 0.), rgba(0xffffff1a));
                    stroke(window, 1., Some(&[px(5.), px(5.)]), at(0., 0.), at(1., 1.), rgba(0xffffff38));
                    // Elapsed part of the period, faintly lit.
                    window.paint_quad(fill(
                        Bounds { origin: at(0., 1.), size: size(w * now_x, h) },
                        rgba(0xffffff05),
                    ));
                    stroke(window, 1., None, at(now_x, 1.), at(now_x, 0.), rgba(0xffffff1f));

                    let measured: Vec<_> = values.iter().map(|(x, y)| at(*x, *y)).collect();
                    let Some(&first) = measured.first() else {
                        return;
                    };
                    let last = measured[measured.len() - 1];
                    // Before the first reading CodeNotch only knows the period started at 0 %:
                    // that stretch is dotted, the area under it fainter.
                    let mut area = PathBuilder::fill();
                    area.move_to(at(0., 0.));
                    area.line_to(first);
                    smooth(&mut area, &measured);
                    area.line_to(point(last.x, bounds.origin.y + h));
                    area.close();
                    if let Ok(path) = area.build() {
                        window.paint_path(
                            path,
                            linear_gradient(
                                180.,
                                linear_color_stop(with_alpha(color, 0.22), 0.),
                                linear_color_stop(with_alpha(color, 0.), 1.),
                            ),
                        );
                    }
                    stroke(window, 1.5, Some(&[px(1.5), px(3.5)]), at(0., 0.), first, with_alpha(color, 0.55));
                    if measured.len() >= 2 {
                        let mut line = PathBuilder::stroke(px(2.));
                        line.move_to(first);
                        smooth(&mut line, &measured);
                        if let Ok(path) = line.build() {
                            window.paint_path(path, color);
                        }
                    }
                    // Where the average pace leads by the reset.
                    let (px_end, py_end) = projection_end;
                    let target = at(px_end, py_end);
                    stroke(window, 1.5, Some(&[px(4.), px(4.)]), last, target, with_alpha(color, 0.7));
                    if exhausts_at.is_some() {
                        let arm = px(4.);
                        let red = rgb(0xf9735b);
                        stroke(window, 1.5, None, point(target.x - arm, target.y - arm), point(target.x + arm, target.y + arm), red);
                        stroke(window, 1.5, None, point(target.x - arm, target.y + arm), point(target.x + arm, target.y - arm), red);
                    }
                    // The range the past periods give, as a bar just inside the reset.
                    if let Some((low, high)) = range {
                        let x = bounds.origin.x + w - px(4.);
                        let (top, bottom) = (at(1., high).y, at(1., low).y);
                        let soft = with_alpha(color, 0.7);
                        stroke(window, 1.5, None, point(x, top), point(x, bottom), soft);
                        stroke(window, 1.5, None, point(x - px(3.), top), point(x + px(3.), top), soft);
                        stroke(window, 1.5, None, point(x - px(3.), bottom), point(x + px(3.), bottom), soft);
                    }
                    match hovered.and_then(|i| measured.get(i)) {
                        Some(&p) => {
                            stroke(window, 1., None, point(p.x, bounds.origin.y), point(p.x, bounds.origin.y + h), rgba(0xffffff33));
                            dot(window, p, color);
                        }
                        None => dot(window, last, color),
                    }
                },
            )
            .absolute()
            .size_full()
        };

        let tooltip = hovered.map(|i| {
            let p = self.points[i];
            let x = xs[i];
            let card = div()
                .absolute()
                .bottom(px(HEIGHT - 6.))
                .min_w(px(132.))
                .px(px(10.))
                .py(px(7.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .rounded(px(8.))
                .border_1()
                .border_color(rgba(0xffffff14))
                .bg(rgb(0x1a1a1d))
                .shadow_lg()
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(0xe4e4e7))
                        .child((self.time_text)(p.at)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_size(px(11.))
                        .child(div().w(px(3.)).h(px(12.)).rounded(px(2.)).bg(color))
                        .child(div().flex_1().text_color(rgb(0xa1a1aa)).child(self.label.clone()))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xfafafa))
                                .child((self.value_text)(p.value)),
                        ),
                );
            if x > 0.6 {
                card.right(relative(1. - x + 0.02))
            } else {
                card.left(relative(x + 0.02))
            }
        });

        let on_hover = self.on_hover.clone();
        let on_leave = self.on_hover.clone();
        let times: Vec<i64> = self.points.iter().map(|p| p.at).collect();
        let (start, end) = (budget.start, budget.end);
        let plot = div()
            .id(self.id.clone())
            .relative()
            .w_full()
            .h(px(HEIGHT))
            .child(painter)
            .children(tooltip)
            .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                let Some(bounds) = bounds_cell.get() else {
                    return;
                };
                if !bounds.contains(&event.position) || times.is_empty() {
                    return;
                }
                let share = f32::from(event.position.x - bounds.origin.x) / f32::from(bounds.size.width);
                let at = start + ((end - start) as f32 * share) as i64;
                // Only readings answer the pointer, not the projected or unknown stretches.
                let nearest = times
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, t)| (**t - at).abs())
                    .filter(|(_, t)| (**t - at).abs() < (end - start) / 12)
                    .map(|(i, _)| i);
                if nearest != hovered {
                    on_hover(nearest, window, cx);
                }
            })
            .on_hover(move |inside, window, cx| {
                if !inside {
                    on_leave(None, window, cx);
                }
            });

        div()
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(plot)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(rgb(0x8a8a93))
                    .child(self.start_caption)
                    .child(self.end_caption),
            )
            .into_any_element()
    }
}
