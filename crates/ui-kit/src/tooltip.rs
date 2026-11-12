//! Instant hover tooltips in the shadcn/ui `ChartTooltip` style, shared by every statistics
//! tooltip: the area chart's hover card, the heat grids (activity heatmap, weekday × hour) and
//! small hover hints (KPI deltas).
//!
//! * [`tooltip_card`]: the card itself — rounded-lg, `border`/50, `background` fill, 10 × 6 px
//!   padding, 12 px text, large shadow; a medium-weight header, then rows of
//!   `[swatch] name … value`.
//! * [`HoverGrid`]: a grid of equal cells (any element drawn inside, laid out on a fixed
//!   pitch). One keyed hover state per grid holds the hovered cell; the cell is computed from
//!   the mouse position on the grid container, so the window only re-renders when the hovered
//!   cell changes, and one overlay is drawn for it. The card appears at once (no delay),
//!   anchored above the cell (below it near the top of the window, towards the left near the
//!   right edge), never clipped by the window, and goes away as soon as the pointer leaves.
//! * [`HoverTip`]: the same card over any element.
//! * [`SideTip`]: the plain shadcn `Tooltip` bubble (inverted colours, one line) on the right of
//!   an element, instant (labels of the collapsed settings sidebar).
//!
//! Placement and hit-testing are pure functions below, unit tested.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    anchored, canvas, deferred, div, point, px, AnyElement, App, Bounds, Corner, Div, ElementId,
    FontWeight, Hsla, InteractiveElement, IntoElement, MouseMoveEvent, ParentElement, Pixels,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
};

use crate::Theme;

/// Gap between the anchor (cell / element) and the card.
pub const TIP_GAP: f32 = 6.0;
/// Margin kept between the card and the window edges.
pub const TIP_MARGIN: f32 = 8.0;
/// Header / row line height used to estimate the card height (placement only).
const LINE_H: f32 = 16.0;

// ---------------------------------------------------------------------------------------------
// Card
// ---------------------------------------------------------------------------------------------

/// One `[swatch] name … value` line of a tooltip card.
#[derive(Debug, Clone, PartialEq)]
pub struct TipRow {
    pub swatch: Option<Hsla>,
    pub name: SharedString,
    pub value: SharedString,
    /// Total line: name in the foreground colour, medium weight, hairline above.
    pub strong: bool,
}

impl TipRow {
    pub fn new(name: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self {
            swatch: None,
            name: name.into(),
            value: value.into(),
            strong: false,
        }
    }
    pub fn swatch(mut self, color: Hsla) -> Self {
        self.swatch = Some(color);
        self
    }
    pub fn strong(mut self) -> Self {
        self.strong = true;
        self
    }
}

/// What a tooltip shows: a header and its rows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TipContent {
    pub title: SharedString,
    pub rows: Vec<TipRow>,
}

impl TipContent {
    pub fn new(title: impl Into<SharedString>, rows: Vec<TipRow>) -> Self {
        Self {
            title: title.into(),
            rows,
        }
    }

    /// Rough height of the card (placement only: the window edge fit is exact anyway).
    pub fn estimated_height(&self) -> f32 {
        let lines = 1 + self.rows.len();
        12.0 + 2.0 + lines as f32 * LINE_H + (lines - 1) as f32 * 6.0
    }
}

/// The shadcn `ChartTooltip` card (not positioned).
pub fn tooltip_card(t: &Theme, content: &TipContent) -> Div {
    let row = |r: &TipRow| {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .when_some(r.swatch, |d, c| {
                d.child(div().flex_none().size(px(10.)).rounded(px(2.)).bg(c))
            })
            .child(
                div()
                    .flex_grow()
                    .text_color(if r.strong {
                        t.foreground
                    } else {
                        t.muted_foreground
                    })
                    .when(r.strong, |d| d.font_weight(FontWeight::MEDIUM))
                    .child(r.name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .pl(px(16.))
                    .text_color(t.foreground)
                    .font_weight(FontWeight::MEDIUM)
                    .child(r.value.clone()),
            )
    };
    let mut card = div()
        .min_w(px(128.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(10.))
        .py(px(6.))
        .rounded(px(8.))
        .border_1()
        .border_color(t.border.opacity(0.5))
        .bg(t.background)
        .shadow_lg()
        .font_family(t.fonts.text)
        .text_size(px(12.))
        .line_height(px(LINE_H))
        .whitespace_nowrap()
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.foreground)
                .child(content.title.clone()),
        );
    for r in &content.rows {
        card = card.child(if r.strong {
            div()
                .pt(px(4.))
                .border_t_1()
                .border_color(t.border.opacity(0.5))
                .child(row(r))
        } else {
            row(r)
        });
    }
    card
}

// ---------------------------------------------------------------------------------------------
// Pure placement & hit-testing
// ---------------------------------------------------------------------------------------------

/// Where the card goes for an anchor rectangle `(x, y, w, h)` in a `viewport` (window
/// coordinates): the point and the card corner put on it. Above the anchor, unless a card
/// `card_h` tall would leave the window at the top (then below); starting at the anchor's left
/// edge in the left half of the window, ending at its right edge in the right half.
pub fn tip_anchor(
    anchor: (f32, f32, f32, f32),
    viewport: (f32, f32),
    card_h: f32,
) -> (f32, f32, Corner) {
    let (x, y, w, h) = anchor;
    let above = y - TIP_GAP - card_h >= TIP_MARGIN;
    let left = x + w / 2.0 <= viewport.0 / 2.0;
    let px_ = if left { x } else { x + w };
    let (py_, corner) = match (above, left) {
        (true, true) => (y - TIP_GAP, Corner::BottomLeft),
        (true, false) => (y - TIP_GAP, Corner::BottomRight),
        (false, true) => (y + h + TIP_GAP, Corner::TopLeft),
        (false, false) => (y + h + TIP_GAP, Corner::TopRight),
    };
    (px_, py_, corner)
}

/// Cell `(col, row)` under the local point `(x, y)` of a grid of `cols` × `rows` cells of size
/// `cell`, one every `pitch.0` across and `pitch.1` down; `None` outside the grid. The gap after
/// a cell belongs to it (moving across gaps never hides and re-shows the card).
pub fn cell_at(
    x: f32,
    y: f32,
    cell: f32,
    pitch: (f32, f32),
    cols: usize,
    rows: usize,
) -> Option<(usize, usize)> {
    if x < 0.0 || y < 0.0 || pitch.0 <= 0.0 || pitch.1 <= 0.0 {
        return None;
    }
    let (c, r) = (
        (x / pitch.0).floor() as usize,
        (y / pitch.1).floor() as usize,
    );
    if c >= cols || r >= rows {
        return None;
    }
    // Past the last cell's own size (the container may be wider than the cells).
    if (c + 1 == cols && x - c as f32 * pitch.0 > cell)
        || (r + 1 == rows && y - r as f32 * pitch.1 > cell)
    {
        return None;
    }
    Some((c, r))
}

/// The card, drawn on top of everything, anchored to `anchor` (window coordinates) and kept
/// inside the window.
pub fn tip_overlay(
    t: &Theme,
    content: &TipContent,
    anchor: Bounds<Pixels>,
    window: &Window,
) -> AnyElement {
    let vp = window.viewport_size();
    let (x, y, corner) = tip_anchor(
        (
            f32::from(anchor.origin.x),
            f32::from(anchor.origin.y),
            f32::from(anchor.size.width),
            f32::from(anchor.size.height),
        ),
        (f32::from(vp.width), f32::from(vp.height)),
        content.estimated_height(),
    );
    deferred(
        anchored()
            .position(point(px(x), px(y)))
            .anchor(corner)
            .snap_to_window_with_margin(px(TIP_MARGIN))
            .child(tooltip_card(t, content)),
    )
    .with_priority(2)
    .into_any_element()
}

// ---------------------------------------------------------------------------------------------
// HoverGrid
// ---------------------------------------------------------------------------------------------

/// Tooltip of the cell `(col, row)`; `None` = no card (an empty cell).
pub type CellTip = Rc<dyn Fn(usize, usize) -> Option<TipContent>>;

#[derive(Default)]
struct GridState {
    hover: Option<(usize, usize)>,
    /// Grid bounds of the last paint (window coordinates).
    bounds: Option<Bounds<Pixels>>,
}

/// A grid of `cols` × `rows` cells (size `cell`, every `cell + gap` across and
/// `cell + row_gap` down — `row_gap` defaults to `gap` — from the top-left corner of `content`)
/// with an instant tooltip per cell. See the module docs.
#[derive(IntoElement)]
pub struct HoverGrid {
    id: ElementId,
    theme: Theme,
    cols: usize,
    rows: usize,
    cell: f32,
    gap: f32,
    row_gap: f32,
    content: AnyElement,
    tip: CellTip,
    forced: Option<(usize, usize)>,
}

impl HoverGrid {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<ElementId>,
        theme: Theme,
        cols: usize,
        rows: usize,
        cell: f32,
        gap: f32,
        content: impl IntoElement,
        tip: impl Fn(usize, usize) -> Option<TipContent> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            cols,
            rows,
            cell,
            gap,
            row_gap: gap,
            content: content.into_any_element(),
            tip: Rc::new(tip),
            forced: None,
        }
    }

    /// Vertical gap between rows when it differs from the horizontal one.
    pub fn row_gap(mut self, gap: f32) -> Self {
        self.row_gap = gap;
        self
    }

    /// Show this cell's card without a pointer (dev captures).
    pub fn forced(mut self, cell: Option<(usize, usize)>) -> Self {
        self.forced = cell;
        self
    }
}

impl RenderOnce for HoverGrid {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = self.theme;
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| GridState::default());
        let (hover, bounds) = {
            let s = state.read(cx);
            (s.hover.or(self.forced), s.bounds)
        };
        let (cell, cols, rows) = (self.cell, self.cols, self.rows);
        let pitch = (cell + self.gap, cell + self.row_gap);
        let hover = hover.filter(|(c, r)| *c < cols && *r < rows);

        let paint_state = state.clone();
        let probe = canvas(
            move |b, _, cx| paint_state.update(cx, |s, _| s.bounds = Some(b)),
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let move_state = state.clone();
        let leave_state = state.clone();
        let mut grid = div()
            .id(self.id.clone())
            .relative()
            .flex_none()
            .child(probe)
            .child(self.content)
            .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
                let Some(b) = move_state.read(cx).bounds else {
                    return;
                };
                let local = e.position - b.origin;
                let at = cell_at(
                    f32::from(local.x),
                    f32::from(local.y),
                    cell,
                    pitch,
                    cols,
                    rows,
                );
                if move_state.read(cx).hover != at {
                    move_state.update(cx, |s, cx| {
                        s.hover = at;
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

        if let (Some((c, r)), Some(b)) = (hover, bounds) {
            if let Some(content) = (self.tip)(c, r) {
                let (x, y) = (c as f32 * pitch.0, r as f32 * pitch.1);
                // Outline of the hovered cell.
                grid = grid.child(
                    div()
                        .absolute()
                        .left(px(x - 1.))
                        .top(px(y - 1.))
                        .size(px(cell + 2.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(t.foreground.opacity(0.55)),
                );
                let anchor = Bounds {
                    origin: b.origin + point(px(x), px(y)),
                    size: gpui::size(px(cell), px(cell)),
                };
                grid = grid.child(tip_overlay(&t, &content, anchor, window));
            }
        }
        grid
    }
}

// ---------------------------------------------------------------------------------------------
// HoverTip
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct TipState {
    hovered: bool,
    bounds: Option<Bounds<Pixels>>,
}

/// Any element with an instant tooltip card while hovered.
#[derive(IntoElement)]
pub struct HoverTip {
    id: ElementId,
    theme: Theme,
    child: AnyElement,
    content: TipContent,
}

impl HoverTip {
    pub fn new(
        id: impl Into<ElementId>,
        theme: Theme,
        child: impl IntoElement,
        content: TipContent,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            child: child.into_any_element(),
            content,
        }
    }
}

impl RenderOnce for HoverTip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| TipState::default());
        let (hovered, bounds) = {
            let s = state.read(cx);
            (s.hovered, s.bounds)
        };
        let paint_state = state.clone();
        let hover_state = state.clone();
        div()
            .id(self.id.clone())
            .relative()
            .child(
                canvas(
                    move |b, _, cx| paint_state.update(cx, |s, _| s.bounds = Some(b)),
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(self.child)
            .on_hover(move |h, _, cx| {
                let h = *h;
                if hover_state.read(cx).hovered != h {
                    hover_state.update(cx, |s, cx| {
                        s.hovered = h;
                        cx.notify();
                    });
                }
            })
            .when_some(bounds.filter(|_| hovered), |d, b| {
                d.child(tip_overlay(&self.theme, &self.content, b, window))
            })
    }
}

// ---------------------------------------------------------------------------------------------
// SideTip
// ---------------------------------------------------------------------------------------------

/// Height of a [`plain_tip`] (one 16 px line, 6 px above and below).
pub const PLAIN_TIP_H: f32 = 28.0;
/// Gap between an element and its [`SideTip`].
pub const SIDE_TIP_GAP: f32 = 8.0;

/// The shadcn `Tooltip` bubble: inverted colours (foreground fill, background text), 12 px,
/// 6 px radius, 12 × 6 px padding, one line.
pub fn plain_tip(t: &Theme, text: SharedString) -> Div {
    div()
        .h(px(PLAIN_TIP_H))
        .flex()
        .items_center()
        .px(px(12.))
        .rounded(px(6.))
        .bg(t.foreground)
        .font_family(t.fonts.text)
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(t.background)
        .whitespace_nowrap()
        .child(text)
}

/// Top-left corner of a side tooltip `tip_h` tall for an anchor `(x, y, w, h)`: right of the
/// anchor, [`SIDE_TIP_GAP`] away, vertically centred on it.
pub fn side_anchor(anchor: (f32, f32, f32, f32), tip_h: f32) -> (f32, f32) {
    let (x, y, w, h) = anchor;
    (x + w + SIDE_TIP_GAP, y + (h - tip_h) / 2.0)
}

/// Any element with an instant [`plain_tip`] on its right while hovered (shadcn sidebar rail:
/// `TooltipContent side="right"`). Shown at once, gone as soon as the pointer leaves; drawn on
/// top of everything and kept inside the window.
#[derive(IntoElement)]
pub struct SideTip {
    id: ElementId,
    theme: Theme,
    child: AnyElement,
    text: SharedString,
    /// No tooltip (the element shows its own label).
    disabled: bool,
}

impl SideTip {
    pub fn new(
        id: impl Into<ElementId>,
        theme: Theme,
        text: impl Into<SharedString>,
        child: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            child: child.into_any_element(),
            text: text.into(),
            disabled: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for SideTip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| TipState::default());
        let (hovered, bounds) = {
            let s = state.read(cx);
            (s.hovered && !self.disabled, s.bounds)
        };
        let paint_state = state.clone();
        let hover_state = state.clone();
        let t = self.theme;
        let text = self.text.clone();
        div()
            .id(self.id.clone())
            .relative()
            .child(
                canvas(
                    move |b, _, cx| paint_state.update(cx, |s, _| s.bounds = Some(b)),
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(self.child)
            .on_hover(move |h, _, cx| {
                let h = *h;
                if hover_state.read(cx).hovered != h {
                    hover_state.update(cx, |s, cx| {
                        s.hovered = h;
                        cx.notify();
                    });
                }
            })
            .when_some(bounds.filter(|_| hovered), |d, b| {
                let (x, y) = side_anchor(
                    (
                        f32::from(b.origin.x),
                        f32::from(b.origin.y),
                        f32::from(b.size.width),
                        f32::from(b.size.height),
                    ),
                    PLAIN_TIP_H,
                );
                d.child(
                    deferred(
                        anchored()
                            .position(point(px(x), px(y)))
                            .anchor(Corner::TopLeft)
                            .snap_to_window_with_margin(px(TIP_MARGIN))
                            .child(plain_tip(&t, text)),
                    )
                    .with_priority(2),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_tips_sit_right_of_the_anchor_and_centred() {
        assert_eq!(
            side_anchor((8.0, 100.0, 32.0, 32.0), PLAIN_TIP_H),
            (40.0 + SIDE_TIP_GAP, 102.0)
        );
    }

    #[test]
    fn cells_from_local_positions() {
        // 10 px cells every 12 px across, every 20 px down; 5 × 3.
        let p = (12.0, 20.0);
        assert_eq!(cell_at(0.0, 0.0, 10.0, p, 5, 3), Some((0, 0)));
        assert_eq!(cell_at(25.0, 21.0, 10.0, p, 5, 3), Some((2, 1)));
        assert_eq!(cell_at(57.0, 49.0, 10.0, p, 5, 3), Some((4, 2)));
        // A gap belongs to the cell before it.
        assert_eq!(cell_at(11.0, 15.0, 10.0, p, 5, 3), Some((0, 0)));
        assert_eq!(
            cell_at(59.0, 0.0, 10.0, p, 5, 3),
            None,
            "past the last cell"
        );
        assert_eq!(
            cell_at(60.0, 0.0, 10.0, p, 5, 3),
            None,
            "past the last column"
        );
        assert_eq!(cell_at(0.0, 60.0, 10.0, p, 5, 3), None, "past the last row");
        assert_eq!(cell_at(-1.0, 0.0, 10.0, p, 5, 3), None);
    }

    #[test]
    fn card_goes_above_and_flips_near_edges() {
        let vp = (1000.0, 800.0);
        // Middle-left: above, from the cell's left edge.
        assert_eq!(
            tip_anchor((100.0, 400.0, 10.0, 10.0), vp, 80.0),
            (100.0, 400.0 - TIP_GAP, Corner::BottomLeft)
        );
        // Right half: ends at the cell's right edge.
        assert_eq!(
            tip_anchor((900.0, 400.0, 10.0, 10.0), vp, 80.0),
            (910.0, 400.0 - TIP_GAP, Corner::BottomRight)
        );
        // Too close to the top: below the cell.
        assert_eq!(
            tip_anchor((100.0, 40.0, 10.0, 10.0), vp, 80.0),
            (100.0, 50.0 + TIP_GAP, Corner::TopLeft)
        );
        assert_eq!(
            tip_anchor((900.0, 40.0, 10.0, 10.0), vp, 80.0).2,
            Corner::TopRight
        );
    }

    #[test]
    fn height_estimate_grows_with_rows() {
        let one = TipContent::new("a", vec![]);
        let three = TipContent::new("a", vec![TipRow::new("x", "1"), TipRow::new("y", "2")]);
        assert!(three.estimated_height() > one.estimated_height());
    }
}
