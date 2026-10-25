//! The notch: rail, fold tab and usage popover, drawn to the measurements of the Tauri
//! app's `notch.css` so both builds look the same.
use crate::assets::icon_path;
use crate::desktop;
use crate::model::{Reading, Window as Quota};
use crate::store::{self, Store};
use gpui::{
    canvas, div, point, prelude::*, px, relative, rgb, rgba, svg, AnyElement, App, Bounds,
    Context, DisplayId, Entity, FontWeight, PathBuilder, Pixels, Point, Rgba, SharedString, Size,
    Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
};
use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::OnceLock;
use std::time::Duration;

// `.notch-rail` and `.provider-cell` in notch.css.
const RAIL_THICKNESS: f32 = 74.;
const RAIL_RADIUS: f32 = 33.;
const RAIL_PAD_START: f32 = 23.;
const RAIL_PAD_END: f32 = 18.;
const H_RAIL_THICKNESS: f32 = 100.;
const H_RAIL_PAD: f32 = 24.;
const H_RAIL_RADIUS: f32 = 28.;
const RING: f32 = 44.;
const CELL_LEN: f32 = 72.;
const CELL_WIDTH: f32 = 48.;
const CELL_GAP: f32 = 22.;
const H_CELL_GAP: f32 = 25.;
/// Concave corners joining a side rail to the screen edge.
const FLARE: f32 = 32.;
/// `.notch-tools` sits 23 px past the rail; the settings orb is 36 px.
const TOOLS_GAP: f32 = 23.;
const TOOLS_SIZE: f32 = 36.;
const TOOLS_WIDTH: f32 = 74.;
// `.notch-peek` hit area and `.peek-indicator`.
const PEEK_LEN: f32 = 96.;
const PEEK_THICKNESS: f32 = 24.;
const INDICATOR_LEN: f32 = 64.;
const INDICATOR_THICKNESS: f32 = 10.;
// `.usage-popover`: 310 px wide, 97 px from a side edge, 110 px from the top or bottom.
const POPOVER_WIDTH: f32 = 310.;
const POPOVER_SIDE_OFFSET: f32 = 97.;
const POPOVER_END_OFFSET: f32 = 110.;
const TAIL: f32 = 21.;

const BLACK: u32 = 0x000000;

/// The Tauri app's English strings, keyed by the French source text.
fn english() -> &'static HashMap<String, String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        serde_json::from_str(include_str!("../../src/lib/messages.en.json")).unwrap_or_default()
    })
}

/// Translates a source string the way the Tauri app's `t()` does.
pub(crate) fn t(french: bool, key: &str) -> String {
    if french {
        key.to_owned()
    } else {
        english().get(key).cloned().unwrap_or_else(|| key.to_owned())
    }
}

pub fn provider_title(provider: &str) -> &'static str {
    match provider {
        "codex" => "Codex",
        "claude" => "Claude",
        "gemini" => "Gemini",
        "grok" => "Grok",
        "opencode" => "OpenCode",
        "copilot" => "Copilot",
        "ollama" => "Ollama",
        _ => "Personnalisé",
    }
}

pub(crate) fn provider_mark(provider: &str) -> &'static str {
    match provider {
        "codex" => "Cx",
        "claude" => "Cl",
        "gemini" => "Ge",
        "grok" => "Gk",
        "opencode" => "OC",
        "copilot" => "Co",
        _ => "API",
    }
}

fn usage_color(value: Option<f64>) -> Rgba {
    match value {
        None => rgb(0x858585),
        Some(v) if v < 50. => rgb(0x28e07b),
        Some(v) if v < 80. => rgb(0xf5e400),
        Some(_) => rgb(0xff4500),
    }
}

fn headline(reading: Option<&Reading>) -> Option<&Quota> {
    reading?
        .windows
        .iter()
        .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
}

/// `isFresh` in format.ts: a reading counts as current for two poll intervals.
fn is_fresh(reading: Option<&Reading>, interval: u64, now: i64) -> bool {
    reading.is_some_and(|r| {
        r.status == "ok"
            && r.updated_at
                .is_some_and(|at| now - at <= (interval as i64 * 2 + 30).max(270))
    })
}

fn status_label(status: &str) -> &'static str {
    match status {
        "ok" => "À jour",
        "needsAuth" => "À connecter",
        "stale" => "Ancien relevé",
        "rateLimited" => "En attente",
        _ => "Indisponible",
    }
}

pub(crate) fn tr(french: bool, fr: &str, en: &str) -> String {
    if french { fr } else { en }.to_owned()
}

fn percentage(value: f64, french: bool) -> String {
    let rounded = (value * 10.).round() / 10.;
    let text = if rounded.fract() == 0. {
        format!("{rounded:.0}")
    } else if french {
        format!("{rounded:.1}").replace('.', ",")
    } else {
        format!("{rounded:.1}")
    };
    format!("{text} %")
}

fn tokens(n: u64, compact: bool, french: bool) -> String {
    if compact && n >= 1000 {
        let (value, unit) = if n >= 1_000_000_000 {
            (n as f64 / 1e9, if french { " Md" } else { "B" })
        } else if n >= 1_000_000 {
            (n as f64 / 1e6, if french { " M" } else { "M" })
        } else {
            (n as f64 / 1e3, if french { " k" } else { "K" })
        };
        let text = format!("{:.1}", value).trim_end_matches(".0").to_owned();
        return format!("{}{unit}", if french { text.replace('.', ",") } else { text });
    }
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(if french { '\u{202f}' } else { ',' });
        }
        out.push(c);
    }
    out
}

fn reset_time(at: Option<i64>, now: i64, french: bool) -> String {
    let Some(at) = at else {
        return tr(french, "Date non communiquée", "Reset time unavailable");
    };
    let minutes = (at - now + 59) / 60;
    if minutes <= 0 {
        return tr(french, "Renouvellement à vérifier", "Reset needs checking");
    }
    if minutes < 60 {
        return tr(french, &format!("dans {minutes} min"), &format!("in {minutes} min"));
    }
    let hours = minutes / 60;
    if hours >= 24 {
        let (d, h) = (hours / 24, hours % 24);
        return tr(french, &format!("dans {d} j {h} h"), &format!("in {d} d {h} h"));
    }
    let m = minutes % 60;
    tr(french, &format!("dans {hours} h {m} min"), &format!("in {hours} h {m} min"))
}

fn freshness(at: Option<i64>, now: i64, french: bool) -> String {
    let Some(at) = at else {
        return tr(french, "Aucun relevé", "No reading");
    };
    let s = (now - at).max(0);
    match s {
        0..10 => tr(french, "À l’instant", "Just now"),
        10..60 => tr(french, &format!("Il y a {s} s"), &format!("{s} s ago")),
        60..3600 => tr(french, &format!("Il y a {} min", s / 60), &format!("{} min ago", s / 60)),
        _ => tr(french, &format!("Il y a {} h", s / 3600), &format!("{} h ago", s / 3600)),
    }
}

fn icon(name: &str, size: f32, color: Rgba) -> gpui::Svg {
    svg()
        .path(icon_path(name))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

fn popup_options(bounds: Bounds<Pixels>, display: Option<DisplayId>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id: display,
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Edge {
    Right,
    Left,
    Top,
    Bottom,
}

impl Edge {
    fn of(s: &Store) -> Self {
        match s.config.settings.widget_position.as_str() {
            "left" => Edge::Left,
            "top" => Edge::Top,
            "bottom" => Edge::Bottom,
            _ => Edge::Right,
        }
    }

    fn vertical(self) -> bool {
        matches!(self, Edge::Right | Edge::Left)
    }

    /// Rounds the corners facing the screen, leaving the side against the edge square.
    fn round<T: Styled>(self, element: T, radius: f32) -> T {
        match self {
            Edge::Right => element.rounded_l(px(radius)),
            Edge::Left => element.rounded_r(px(radius)),
            Edge::Top => element.rounded_b(px(radius)),
            Edge::Bottom => element.rounded_t(px(radius)),
        }
    }

    /// `.fold-button` rotates the chevrons toward the edge.
    fn fold_rotation(self) -> f32 {
        match self {
            Edge::Right => 0.,
            Edge::Left => 180.,
            Edge::Top => -90.,
            Edge::Bottom => 90.,
        }
    }
}

/// Where the parts of the open notch sit inside its window.
struct Layout {
    window: Bounds<Pixels>,
    /// The black rail, relative to the window.
    rail: Bounds<Pixels>,
    /// The tool buttons, relative to the window.
    tools: Bounds<Pixels>,
}

fn cell_count(s: &Store) -> usize {
    s.enabled().len().max(1)
}

/// `.notch-position` centered on its edge; the window adds the flares and the tools.
fn open_layout(area: Bounds<Pixels>, s: &Store) -> Layout {
    let edge = Edge::of(s);
    let n = cell_count(s) as f32;
    if edge.vertical() {
        let len = RAIL_PAD_START + n * CELL_LEN + (n - 1.) * CELL_GAP + RAIL_PAD_END;
        let after = FLARE.max(TOOLS_GAP + TOOLS_SIZE);
        let size = Size { width: px(RAIL_THICKNESS), height: px(FLARE + len + after) };
        let rail_top = area.origin.y + (area.size.height - px(len)) / 2.;
        let x = match edge {
            Edge::Right => area.origin.x + area.size.width - size.width,
            _ => area.origin.x,
        };
        Layout {
            window: Bounds { origin: point(x, rail_top - px(FLARE)), size },
            rail: Bounds {
                origin: point(px(0.), px(FLARE)),
                size: Size { width: px(RAIL_THICKNESS), height: px(len) },
            },
            tools: Bounds {
                origin: point(px(0.), px(FLARE + len + TOOLS_GAP)),
                size: Size { width: px(TOOLS_WIDTH), height: px(TOOLS_SIZE) },
            },
        }
    } else {
        let len = H_RAIL_PAD * 2. + n * CELL_WIDTH + (n - 1.) * H_CELL_GAP;
        let size = Size {
            width: px(len + 8. + TOOLS_WIDTH),
            height: px(H_RAIL_THICKNESS),
        };
        let rail_left = area.origin.x + (area.size.width - px(len)) / 2.;
        let y = match edge {
            Edge::Bottom => area.origin.y + area.size.height - size.height,
            _ => area.origin.y,
        };
        // `.horizontal .notch-tools` sits 25 px from the screen edge.
        let tools_y = match edge {
            Edge::Bottom => H_RAIL_THICKNESS - 25. - TOOLS_SIZE,
            _ => 25.,
        };
        Layout {
            window: Bounds { origin: point(rail_left, y), size },
            rail: Bounds {
                origin: point(px(0.), px(0.)),
                size: Size { width: px(len), height: px(H_RAIL_THICKNESS) },
            },
            tools: Bounds {
                origin: point(px(len + 8.), px(tools_y)),
                size: Size { width: px(TOOLS_WIDTH), height: px(TOOLS_SIZE) },
            },
        }
    }
}

/// The open rail's window, or the fold tab's.
fn notch_bounds(area: Bounds<Pixels>, s: &Store) -> Bounds<Pixels> {
    let edge = Edge::of(s);
    if !s.collapsed {
        return open_layout(area, s).window;
    }
    let size = if edge.vertical() {
        Size { width: px(PEEK_THICKNESS), height: px(PEEK_LEN) }
    } else {
        Size { width: px(PEEK_LEN), height: px(PEEK_THICKNESS) }
    };
    let center_x = area.origin.x + (area.size.width - size.width) / 2.;
    let center_y = area.origin.y + (area.size.height - size.height) / 2.;
    let origin = match edge {
        Edge::Right => point(area.origin.x + area.size.width - size.width, center_y),
        Edge::Left => point(area.origin.x, center_y),
        Edge::Top => point(center_x, area.origin.y),
        Edge::Bottom => point(center_x, area.origin.y + area.size.height - size.height),
    };
    Bounds { origin, size }
}

/// Opens the notch on the primary display.
pub fn open_rail(store: Entity<Store>, cx: &mut App) {
    let Some(display) = cx.primary_display() else {
        return;
    };
    let screen = display.bounds();
    let bounds = notch_bounds(screen, store.read(cx));
    cx.open_window(popup_options(bounds, Some(display.id())), |window, cx| {
        cx.new(|cx| Rail::new(store, screen, window, cx))
    })
    .expect("notch window");
}

/// Reveals on hover; folds once the pointer has left both the rail and the popover
/// for 240 ms, so crossing the gap between them does not fold.
pub fn set_hover(store: &Entity<Store>, which: usize, hovered: bool, cx: &mut App) {
    let pending = store.update(cx, |s, cx| {
        s.hover[which] = hovered;
        s.fold_generation += 1;
        if hovered && s.collapsed {
            s.collapsed = false;
            cx.notify();
        }
        (!s.hover.contains(&true)).then_some(s.fold_generation)
    });
    let Some(generation) = pending else {
        return;
    };
    let store = store.clone();
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(240))
            .await;
        let _ = cx.update(|cx| {
            if store.read(cx).fold_generation == generation {
                fold(&store, cx);
            }
        });
    })
    .detach();
}

pub fn fold(store: &Entity<Store>, cx: &mut App) {
    close_popover(store, cx);
    store.update(cx, |s, cx| {
        s.collapsed = s.config.settings.auto_hide;
        cx.notify();
    });
}

/// Strokes an arc of a circle, from 12 o'clock clockwise over `to` of a full turn.
fn paint_arc(window: &mut Window, center: Point<Pixels>, radius: f32, width: f32, to: f32, color: Rgba) {
    if to <= 0. {
        return;
    }
    let steps = (to * 96.).ceil().max(2.) as usize;
    let mut builder = PathBuilder::stroke(px(width));
    for i in 0..=steps {
        let angle = to * TAU * i as f32 / steps as f32 - FRAC_PI_2;
        let p = point(
            center.x + px(radius * angle.cos()),
            center.y + px(radius * angle.sin()),
        );
        if i == 0 {
            builder.move_to(p);
        } else {
            builder.line_to(p);
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// The provider's logo tinted with the current text color, or its short mark.
pub(crate) fn provider_icon(provider: &str, size: f32) -> AnyElement {
    match crate::assets::logo(provider) {
        Some(path) => svg()
            .path(path)
            .size(px(size))
            .flex_none()
            .text_color(rgb(0xf7f7f7))
            .into_any_element(),
        None => icon("sliders-horizontal", size, rgb(0xf7f7f7)).into_any_element(),
    }
}

/// `.quota-ring`: a 44 px ring (viewBox 48, r 20) with a 5.5 track and a 3 px value arc.
fn ring(value: Option<f64>, provider: &str, stale: bool) -> impl IntoElement {
    let color = usage_color(value);
    let scale = RING / 48.;
    div()
        .relative()
        .size(px(RING))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .when(stale, |d| d.opacity(0.5))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let center = bounds.center();
                    paint_arc(window, center, 20. * scale, 5.5 * scale, 1., rgb(0x333333));
                    if let Some(v) = value {
                        let share = (v / 100.).clamp(0., 1.) as f32;
                        paint_arc(window, center, 20. * scale, 3. * scale, share, color);
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .child(provider_icon(provider, 20.))
        .when(stale, |d| {
            d.child(
                div()
                    .absolute()
                    .right_0()
                    .bottom_0()
                    .size(px(10.))
                    .rounded_full()
                    .bg(rgb(BLACK))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(px(6.)).rounded_full().bg(rgb(0xefa44f))),
            )
        })
}

/// The black concave corners that join a side rail to the screen edge, like a notch.
fn paint_flares(window: &mut Window, bounds: Bounds<Pixels>, edge: Edge, rail: Bounds<Pixels>) {
    // u: distance from the screen edge; v: position along it (window coordinates).
    let map = |u: f32, v: f32| -> Point<Pixels> {
        match edge {
            Edge::Right => point(bounds.origin.x + bounds.size.width - px(u), bounds.origin.y + px(v)),
            _ => point(bounds.origin.x + px(u), bounds.origin.y + px(v)),
        }
    };
    let start = f32::from(rail.origin.y);
    let end = f32::from(rail.origin.y + rail.size.height);
    for (base, dir) in [(start, -1.), (end, 1.)] {
        let mut builder = PathBuilder::fill();
        builder.move_to(map(0., base));
        builder.line_to(map(FLARE, base));
        for i in 0..=24 {
            let phi = FRAC_PI_2 * i as f32 / 24.;
            let u = FLARE - FLARE * phi.sin();
            let v = base + dir * (FLARE - FLARE * phi.cos());
            builder.line_to(map(u, v));
        }
        builder.close();
        if let Ok(path) = builder.build() {
            window.paint_path(path, rgb(BLACK));
        }
    }
}

pub struct Rail {
    store: Entity<Store>,
    screen: Bounds<Pixels>,
    hovered: bool,
    /// Last (bounds, visible) given to Windows, to skip redundant calls.
    applied: Option<(Bounds<Pixels>, bool)>,
}

impl Rail {
    fn new(
        store: Entity<Store>,
        screen: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe_in(&store, window, |this: &mut Self, _, window, cx| {
            this.apply(window, cx);
            cx.notify();
        })
        .detach();
        let mut rail = Self {
            store,
            // Keep clear of the taskbar, whichever edge it is on.
            screen: desktop::work_area(window.scale_factor()).unwrap_or(screen),
            hovered: false,
            applied: None,
        };
        rail.apply(window, cx);
        rail
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.store.read(cx);
        let next = (notch_bounds(self.screen, s), s.visible());
        if self.applied == Some(next) {
            return;
        }
        let moved = self
            .applied
            .is_some_and(|(before, _)| before.origin != next.0.origin && !s.collapsed);
        self.applied = Some(next);
        desktop::place(window, next.0, next.1, cx);
        // A popover opened beside the old position would be left floating.
        if (!next.1 || moved) && s.popover.is_some() {
            let store = self.store.clone();
            cx.defer(move |cx| close_popover(&store, cx));
        }
    }
}

impl Render for Rail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // GPUI refreshes the window when the pointer enters or leaves it.
        let hovered = window.is_window_hovered();
        if hovered != self.hovered {
            self.hovered = hovered;
            let store = self.store.clone();
            cx.defer(move |cx| set_hover(&store, 0, hovered, cx));
        }
        let s = self.store.read(cx);
        let edge = Edge::of(s);
        let vertical = edge.vertical();
        if s.collapsed {
            // `.peek-indicator`: 10 × 64, open on the edge side, inside a 24 × 96 hit area.
            let store = self.store.clone();
            let indicator = edge
                .round(div(), 10.)
                .when(vertical, |d| d.w(px(INDICATOR_THICKNESS)).h(px(INDICATOR_LEN)))
                .when(!vertical, |d| d.w(px(INDICATOR_LEN)).h(px(INDICATOR_THICKNESS)))
                .bg(rgb(BLACK))
                .border_color(rgba(0xffffff28))
                .map(|d| match edge {
                    Edge::Right => d.border_l_1().border_t_1().border_b_1(),
                    Edge::Left => d.border_r_1().border_t_1().border_b_1(),
                    Edge::Top => d.border_b_1().border_l_1().border_r_1(),
                    Edge::Bottom => d.border_t_1().border_l_1().border_r_1(),
                });
            return div()
                .id("peek")
                .size_full()
                .flex()
                .items_center()
                .map(|d| match edge {
                    Edge::Right => d.justify_end(),
                    Edge::Left => d.justify_start(),
                    Edge::Top => d.flex_col().justify_start(),
                    Edge::Bottom => d.flex_col().justify_end(),
                })
                .cursor_pointer()
                .child(indicator)
                .on_click(move |_, _, cx| set_hover(&store, 0, true, cx))
                .into_any_element();
        }

        let layout = open_layout(self.screen, s);
        let interval = s.config.settings.poll_seconds;
        let now = chrono::Utc::now().timestamp();
        let french = s.french();
        let area = self.screen;
        let rail_bounds = layout.rail;
        let accounts = s.enabled();
        let cells: Vec<AnyElement> = accounts
            .iter()
            .map(|account| {
                let reading = s.readings.get(&account.id);
                let value = headline(reading).map(|w| w.used_percent);
                let stale = reading.is_some() && !is_fresh(reading, interval, now);
                let store = self.store.clone();
                let id = account.id.clone();
                div()
                    .id(SharedString::from(account.id.clone()))
                    .flex_none()
                    .w(px(CELL_WIDTH))
                    .h(px(CELL_LEN))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(9.))
                    .cursor_pointer()
                    .child(ring(value, &account.provider, stale))
                    .child(
                        div()
                            .text_size(px(15.))
                            .line_height(px(19.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(value.map_or("—".into(), |v| {
                                percentage(v, french).replace(" %", "%")
                            })),
                    )
                    .on_hover({
                        let (store, id) = (store.clone(), id.clone());
                        move |hovered, window, cx| {
                            if *hovered {
                                open_popover(&store, id.clone(), window.bounds(), rail_bounds, area, cx)
                            }
                        }
                    })
                    .on_click(move |_, window, cx| {
                        open_popover(&store, id.clone(), window.bounds(), rail_bounds, area, cx)
                    })
                    .into_any_element()
            })
            .collect();
        let empty = accounts.is_empty().then(|| {
            let store = self.store.clone();
            div()
                .id("empty-cell")
                .w(px(CELL_WIDTH))
                .h(px(CELL_LEN))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(9.))
                .cursor_pointer()
                .child(
                    div()
                        .size(px(RING))
                        .rounded_full()
                        .border_2()
                        .border_color(rgb(0x3a3a3a))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("plus", 22., rgb(0xd4d4d4))),
                )
                .child(div().text_size(px(11.)).child(t(french, "Ajouter")))
                .on_click(move |_, _, cx| crate::settings::open(store.clone(), cx))
        });

        let rail = edge
            .round(div(), if vertical { RAIL_RADIUS } else { H_RAIL_RADIUS })
            .absolute()
            .left(rail_bounds.origin.x)
            .top(rail_bounds.origin.y)
            .w(rail_bounds.size.width)
            .h(rail_bounds.size.height)
            .bg(rgb(BLACK))
            .flex()
            .when(vertical, |d| {
                d.flex_col()
                    .items_center()
                    .gap(px(CELL_GAP))
                    .pt(px(RAIL_PAD_START))
                    .pb(px(RAIL_PAD_END))
            })
            .when(!vertical, |d| {
                d.flex_row()
                    .gap(px(H_CELL_GAP))
                    .px(px(H_RAIL_PAD))
                    .map(|d| if edge == Edge::Top { d.pt(px(13.)) } else { d.pt(px(15.)) })
            })
            .children(cells)
            .children(empty);

        // `.notch-tools`: fold (with auto-hide), settings orb and grip, all on black.
        let auto_hide = s.config.settings.auto_hide;
        let fold_store = self.store.clone();
        let settings_store = self.store.clone();
        let tools = div()
            .absolute()
            .left(layout.tools.origin.x)
            .top(layout.tools.origin.y)
            .w(layout.tools.size.width)
            .h(layout.tools.size.height)
            .flex()
            .items_center()
            .justify_center()
            .when(auto_hide, |d| {
                d.child(
                    div()
                        .id("fold")
                        .w(px(22.))
                        .h(px(32.))
                        .rounded(px(16.))
                        .bg(rgb(BLACK))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(rgb(0xaaaaaa))
                        .hover(|d| d.text_color(rgb(0xffffff)))
                        .child(
                            icon("chevrons-right", 16., rgb(0xaaaaaa))
                                .with_transformation(gpui::Transformation::rotate(gpui::radians(
                                    edge.fold_rotation().to_radians(),
                                ))),
                        )
                        .on_click(move |_, _, cx| fold(&fold_store, cx)),
                )
            })
            .child(
                div()
                    .id("settings-orb")
                    .size(px(if auto_hide { 32. } else { 36. }))
                    .rounded_full()
                    .bg(rgb(BLACK))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|d| d.bg(rgb(0x161616)))
                    .child(icon("settings-2", 17., rgb(0xbbbbbb)))
                    .on_click(move |_, _, cx| crate::settings::open(settings_store.clone(), cx)),
            )
            .child(
                div()
                    .w(px(if auto_hide { 20. } else { 18. }))
                    .h(px(32.))
                    .rounded(px(10.))
                    .bg(rgb(BLACK))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("grip-vertical", 14., rgb(0xd0d0d0))),
            );

        div()
            .id("notch")
            .size_full()
            .relative()
            .text_color(rgb(0xf9f9f9))
            .font_family("Segoe UI")
            .when(vertical, |d| {
                d.child(
                    canvas(|_, _, _| {}, move |bounds, _, window, _| {
                        paint_flares(window, bounds, edge, rail_bounds)
                    })
                    .absolute()
                    .size_full(),
                )
            })
            .child(rail)
            .child(tools)
            .into_any_element()
    }
}

/// Matches the stacked blocks of `.usage-popover`, so its window fits the content.
fn popover_height(s: &Store, id: &str) -> f32 {
    let reading = s.readings.get(id);
    let now = chrono::Utc::now().timestamp();
    let fresh = is_fresh(reading, s.config.settings.poll_seconds, now);
    let windows = reading.map_or(0, |r| r.windows.len()) as f32;
    let forecasts = reading.map_or(0, |r| {
        r.windows
            .iter()
            .filter(|w| r.exhausts_at.contains_key(&w.label))
            .count()
    }) as f32;
    let mut h = 19. + 28. + 12. + 15. + 18. + 17. + 10. + 25. + 14.;
    h += if windows > 0. {
        windows * 51. + (windows - 1.) * 19. + forecasts * 22.
    } else {
        42.
    };
    if reading.is_some() && !fresh {
        h += 36.;
    }
    if let Some(r) = reading {
        let rows = r.tokens_today.is_some() as u8 + r.lifetime_tokens.is_some() as u8;
        if rows > 0 {
            h += 17. + 12. + rows as f32 * 15. + (rows as f32 - 1.) * 9.;
        }
        if !r.models.is_empty() {
            h += r.models.len() as f32 * 21.;
        }
    }
    h
}

fn close_popover(store: &Entity<Store>, cx: &mut App) {
    let handle = store.update(cx, |s, cx| {
        s.selected = None;
        s.hover[1] = false;
        cx.notify();
        s.popover.take()
    });
    if let Some(handle) = handle {
        handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

/// Where the popover box goes, before adding the tail toward the rail.
#[derive(Clone, Copy)]
struct Anchor {
    edge: Edge,
    /// Rail center along its edge, in screen coordinates.
    center: Pixels,
    area: Bounds<Pixels>,
}

impl Anchor {
    /// Window bounds for a popover of `height`, tail included.
    fn bounds(self, height: f32) -> Bounds<Pixels> {
        let (w, h) = (px(POPOVER_WIDTH), px(height));
        let a = self.area;
        let clamp = |v: Pixels, lo: Pixels, len: Pixels, size: Pixels| v.max(lo).min(lo + len - size);
        match self.edge {
            Edge::Right | Edge::Left => {
                let size = Size { width: w + px(TAIL), height: h };
                let y = clamp(self.center - h / 2., a.origin.y, a.size.height, h);
                let x = if self.edge == Edge::Right {
                    a.origin.x + a.size.width - px(POPOVER_SIDE_OFFSET) - w
                } else {
                    a.origin.x + px(POPOVER_SIDE_OFFSET) - px(TAIL)
                };
                Bounds { origin: point(x, y), size }
            }
            Edge::Top | Edge::Bottom => {
                let size = Size { width: w, height: h + px(TAIL) };
                let x = clamp(self.center - w / 2., a.origin.x, a.size.width, w);
                let y = if self.edge == Edge::Top {
                    a.origin.y + px(POPOVER_END_OFFSET) - px(TAIL)
                } else {
                    a.origin.y + a.size.height - px(POPOVER_END_OFFSET) - h
                };
                Bounds { origin: point(x, y), size }
            }
        }
    }
}

fn open_popover(
    store: &Entity<Store>,
    id: String,
    notch: Bounds<Pixels>,
    rail: Bounds<Pixels>,
    area: Bounds<Pixels>,
    cx: &mut App,
) {
    if store.read(cx).selected.as_deref() == Some(id.as_str()) {
        return;
    }
    let edge = Edge::of(store.read(cx));
    // Like `.vertical .usage-popover { top: 50% }`: centered on the rail, not the cell.
    let center = if edge.vertical() {
        notch.origin.y + rail.origin.y + rail.size.height / 2.
    } else {
        notch.origin.x + rail.origin.x + rail.size.width / 2.
    };
    let anchor = Anchor { edge, center, area };
    // Another cell of the same rail: reuse the window, only the account changes.
    if let Some(handle) = store.read(cx).popover {
        if handle
            .update(cx, |popover, _, cx| {
                popover.account_id = id.clone();
                cx.notify();
            })
            .is_ok()
        {
            store.update(cx, |s, cx| {
                s.selected = Some(id);
                cx.notify();
            });
            return;
        }
    }
    close_popover(store, cx);
    let bounds = anchor.bounds(popover_height(store.read(cx), &id));
    let handle = cx
        .open_window(popup_options(bounds, None), |window, cx| {
            cx.new(|cx| Popover::new(store.clone(), id.clone(), anchor, window, cx))
        })
        .ok();
    store.update(cx, |s, cx| {
        s.selected = Some(id);
        s.popover = handle;
        cx.notify();
    });
}

pub struct Popover {
    store: Entity<Store>,
    account_id: String,
    anchor: Anchor,
    bounds: Bounds<Pixels>,
    hovered: bool,
}

impl Popover {
    fn new(
        store: Entity<Store>,
        account_id: String,
        anchor: Anchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let bounds = anchor.bounds(popover_height(store.read(cx), &account_id));
        desktop::place(window, bounds, true, cx);
        // The window follows the content: see the probe at the end of the body.
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        // Keeps "2 min ago" and reset countdowns current between readings.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(15)).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        })
        .detach();
        Self {
            store,
            account_id,
            anchor,
            bounds,
            hovered: false,
        }
    }
}

impl Popover {
    /// A zero-height marker after the last block: where it lands is where the content
    /// ends, so the window can be sized to the text GPUI actually laid out.
    fn size_probe(&self, edge: Edge, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let anchor = self.anchor;
        canvas(
            move |bounds, window, cx| {
                let end = f32::from(bounds.origin.y) + 14.;
                let height = if edge == Edge::Top { end - TAIL } else { end };
                let Some(popover) = this.upgrade() else {
                    return;
                };
                let next = anchor.bounds(height);
                let current = popover.read(cx).bounds;
                if (f32::from(next.size.height) - f32::from(current.size.height)).abs() > 0.5
                    || next.origin != current.origin
                {
                    popover.update(cx, |p, _| p.bounds = next);
                    desktop::place(window, next, true, cx);
                }
            },
            |_, _, _, _| {},
        )
        .w_full()
        .h(px(0.))
    }
}

/// The triangle pointing from the popover to the rail (`.usage-popover:after`).
fn paint_tail(window: &mut Window, bounds: Bounds<Pixels>, edge: Edge) {
    let o = bounds.origin;
    let (w, h) = (bounds.size.width, bounds.size.height);
    let points = match edge {
        Edge::Right => {
            let x = o.x + w - px(TAIL) - px(1.);
            let cy = o.y + h / 2.;
            [point(x, cy - px(13.)), point(x + px(22.), cy), point(x, cy + px(13.))]
        }
        Edge::Left => {
            let x = o.x + px(TAIL) + px(1.);
            let cy = o.y + h / 2.;
            [point(x, cy - px(13.)), point(x - px(22.), cy), point(x, cy + px(13.))]
        }
        Edge::Top => {
            let y = o.y + px(TAIL) + px(1.);
            let cx = o.x + w / 2.;
            [point(cx - px(13.), y), point(cx, y - px(22.)), point(cx + px(13.), y)]
        }
        Edge::Bottom => {
            let y = o.y + h - px(TAIL) - px(1.);
            let cx = o.x + w / 2.;
            [point(cx - px(13.), y), point(cx, y + px(22.)), point(cx + px(13.), y)]
        }
    };
    let mut builder = PathBuilder::fill();
    builder.move_to(points[0]);
    builder.line_to(points[1]);
    builder.line_to(points[2]);
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(BLACK));
    }
}

impl Render for Popover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hovered = window.is_window_hovered();
        if hovered != self.hovered {
            self.hovered = hovered;
            let store = self.store.clone();
            cx.defer(move |cx| set_hover(&store, 1, hovered, cx));
        }
        let s = self.store.read(cx);
        let french = s.french();
        let now = chrono::Utc::now().timestamp();
        let interval = s.config.settings.poll_seconds;
        let account = s
            .config
            .accounts
            .iter()
            .find(|a| a.id == self.account_id)
            .cloned();
        let reading = s.readings.get(&self.account_id).cloned();
        let refreshing = s.refreshing;
        let Some(account) = account else {
            return div().into_any_element();
        };
        let fresh = is_fresh(reading.as_ref(), interval, now);
        let edge = self.anchor.edge;

        let close_store = self.store.clone();
        let header = div()
            .flex()
            .items_center()
            .gap(px(9.))
            .h(px(28.))
            .child(provider_icon(&account.provider, 22.))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .text_size(px(16.))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(t(french, provider_title(&account.provider))),
                    )
                    .child(div().child(t(french, "Usage"))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("close")
                    .w(px(27.))
                    .h(px(28.))
                    .rounded(px(5.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|d| d.bg(rgb(0x252525)))
                    .child(icon("x", 15., rgb(0x939393)))
                    .on_click(move |_, _, cx| {
                        close_popover(&close_store, cx);
                        // The pointer is no longer over a popover: fold unless it is on the rail.
                        set_hover(&close_store, 1, false, cx);
                    }),
            );

        let identity = div()
            .mt(px(12.))
            .mb(px(18.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .text_size(px(11.))
            .text_color(rgb(0xa4a4a4))
            .child(account.name.clone())
            .when_some(reading.as_ref().and_then(|r| r.plan.clone()), |d, plan| {
                d.child(
                    div()
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .bg(rgb(0x202020))
                        .text_size(px(10.))
                        .child(plan),
                )
            });

        let warning = reading.as_ref().filter(|_| !fresh).map(|r| {
            let status = if r.status == "ok" {
                t(french, "Dernier relevé ancien")
            } else {
                t(french, status_label(&r.status))
            };
            div()
                .mb(px(15.))
                .flex()
                .gap(px(7.))
                .text_size(px(11.))
                .text_color(rgb(0xeab778))
                .child(icon("circle-alert", 14., rgb(0xeab778)))
                .child(div().flex_1().min_w_0().child(format!("{status}. {}", t(french, &r.message))))
        });

        let windows: Vec<AnyElement> = reading
            .as_ref()
            .map(|r| r.windows.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|w| {
                let forecast = reading
                    .as_ref()
                    .and_then(|r| r.exhausts_at.get(&w.label).copied());
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_end()
                            .gap(px(9.))
                            .mb(px(9.))
                            .text_size(px(12.))
                            .child(t(french, &w.label))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(0xa5a5a5))
                                    .child(reset_time(w.resets_at, now, french)),
                            ),
                    )
                    .child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .rounded(px(4.))
                            .bg(rgb(0x2f2f2f))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative((w.used_percent / 100.).clamp(0., 1.) as f32))
                                    .rounded(px(4.))
                                    .bg(usage_color(Some(w.used_percent))),
                            ),
                    )
                    .child(
                        div()
                            .mt(px(8.))
                            .flex()
                            .gap_1()
                            .text_size(px(12.))
                            .text_color(rgb(0xf2f2f2))
                            .child(percentage(w.used_percent, french))
                            .child(div().text_color(rgb(0xc9c9c9)).child(t(french, "utilisés"))),
                    )
                    .when_some(forecast, |d, at| {
                        let text = t(french, "Plafond estimé {time} au rythme actuel")
                            .replace("{time}", &reset_time(Some(at), now, french));
                        d.child(
                            div()
                                .mt(px(8.))
                                .text_size(px(11.))
                                .text_color(rgb(0xeab778))
                                .child(text),
                        )
                    })
                    .into_any_element()
            })
            .collect();
        let missing = windows.is_empty().then(|| {
            let text = if account.provider == "ollama" {
                t(french, "Pas de quota pour ce serveur local.")
            } else if reading.is_some() && !fresh {
                t(french, "Les quotas seront affichés après la prochaine connexion réussie.")
            } else if let Some(message) = reading.as_ref().map(|r| r.message.clone()).filter(|m| !m.is_empty()) {
                t(french, &message)
            } else {
                t(french, "En attente du premier relevé. Configurez la connexion dans les réglages.")
            };
            div()
                .text_size(px(12.))
                .line_height(px(20.))
                .text_color(rgb(0xb6b6b6))
                .child(text)
        });

        let token_rows = reading.as_ref().and_then(|r| {
            let mut rows = vec![];
            if let Some(n) = r.tokens_today {
                rows.push(
                    div()
                        .flex()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .gap_1()
                                .items_end()
                                .text_color(rgb(0xa9a9a9))
                                .child(t(french, "Tokens aujourd’hui"))
                                .child(div().text_size(px(9.)).text_color(rgb(0x888888)).child("UTC")),
                        )
                        .child(div().font_weight(FontWeight::MEDIUM).child(tokens(n, false, french))),
                );
            }
            if let Some(n) = r.lifetime_tokens {
                rows.push(
                    div()
                        .flex()
                        .justify_between()
                        .child(div().text_color(rgb(0xa9a9a9)).child(t(french, "Total communiqué")))
                        .child(div().font_weight(FontWeight::MEDIUM).child(tokens(n, true, french))),
                );
            }
            (!rows.is_empty()).then(|| {
                div()
                    .mt(px(17.))
                    .pt(px(12.))
                    .border_t_1()
                    .border_color(rgba(0xffffff16))
                    .flex()
                    .flex_col()
                    .gap(px(9.))
                    .text_size(px(11.))
                    .children(rows)
            })
        });
        let models = reading
            .as_ref()
            .filter(|r| !r.models.is_empty())
            .map(|r| {
                div()
                    .mt(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .text_size(px(11.))
                    .children(r.models.clone())
            });

        let store = self.store.clone();
        let footer = div()
            .mt(px(17.))
            .pt(px(10.))
            .border_t_1()
            .border_color(rgba(0xffffff16))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(10.))
            .text_color(rgb(0x9e9e9e))
            .child(freshness(reading.as_ref().and_then(|r| r.updated_at), now, french))
            .child(
                div()
                    .id("refresh")
                    .min_h(px(25.))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .cursor_pointer()
                    .text_color(rgb(0xd0d0d0))
                    .hover(|d| d.text_color(rgb(0xffffff)))
                    .child(icon("refresh-cw", 12., rgb(0xd0d0d0)).when(refreshing, |i| i.opacity(0.5)))
                    .child(t(french, "Actualiser"))
                    .on_click(move |_, _, cx| {
                        let store = store.clone();
                        cx.spawn(async move |cx| store::poll(&store, true, cx).await)
                            .detach();
                    }),
            );

        let body = div()
            .flex()
            .flex_col()
            .w(px(POPOVER_WIDTH))
            .h_full()
            .pt(px(19.))
            .px(px(18.))
            .pb(px(14.))
            .rounded(px(18.))
            .bg(rgb(BLACK))
            .text_color(rgb(0xf4f4f4))
            .child(header)
            .child(identity)
            .children(warning)
            .child(div().flex().flex_col().gap(px(19.)).children(windows).children(missing))
            .children(token_rows)
            .children(models)
            .child(footer)
            .child(self.size_probe(edge, cx));

        // The window holds the box plus a strip on the rail side for the tail.
        let tail_space = div().flex_none().size(px(TAIL));
        let body = body.when(!edge.vertical(), |d| d.flex_1().min_h_0());
        div()
            .size_full()
            .flex()
            .font_family("Segoe UI")
            .map(|d| match edge {
                Edge::Right => d.flex_row().child(body).child(tail_space),
                Edge::Left => d.flex_row().child(tail_space).child(body),
                Edge::Top => d.flex_col().child(tail_space).child(body),
                Edge::Bottom => d.flex_col().child(body).child(tail_space),
            })
            .child(
                canvas(|_, _, _| {}, move |bounds, _, window, _| paint_tail(window, bounds, edge))
                    .absolute()
                    .size_full(),
            )
            .into_any_element()
    }
}
