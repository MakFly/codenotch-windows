use crate::model::{Reading, Window as Quota};
use crate::desktop;
use crate::store::{self, Store};
use gpui::{
    canvas, div, point, prelude::*, px, relative, rgb, rgba, svg, AnyElement, App, Bounds, Context,
    DisplayId,
    Entity, FontWeight, PathBuilder, Pixels, Point, Rgba, SharedString, Size, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
};
use std::f32::consts::{FRAC_PI_2, TAU};
use std::time::Duration;

const RAIL_WIDTH: f32 = 76.;
const CELL: f32 = 84.;
const PAD: f32 = 12.;
const POPOVER_WIDTH: f32 = 300.;
const POPOVER_GAP: f32 = 8.;
const PEEK_WIDTH: f32 = 12.;
const PEEK_HEIGHT: f32 = 72.;
const TOOLS: f32 = 36.;

pub fn provider_title(provider: &str) -> &'static str {
    match provider {
        "codex" => "Codex",
        "claude" => "Claude",
        "gemini" => "Gemini",
        "grok" => "Grok",
        "opencode" => "OpenCode",
        "copilot" => "Copilot",
        _ => "API",
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

pub(crate) fn tr(french: bool, fr: &str, en: &str) -> String {
    if french { fr } else { en }.to_owned()
}

fn percentage(value: f64) -> String {
    let rounded = (value * 10.).round() / 10.;
    if rounded.fract() == 0. {
        format!("{rounded:.0} %")
    } else {
        format!("{rounded:.1} %")
    }
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

/// The rail when open, or the peek tab when folded, on the right edge and vertically centered.
fn rail_bounds(screen: Bounds<Pixels>, s: &Store) -> Bounds<Pixels> {
    let size = if s.collapsed {
        Size {
            width: px(PEEK_WIDTH),
            height: px(PEEK_HEIGHT),
        }
    } else {
        let count = s.enabled().len().max(1) as f32;
        let tools = if s.config.settings.auto_hide { TOOLS } else { 0. };
        Size {
            width: px(RAIL_WIDTH),
            height: px(PAD * 2. + CELL * count + tools),
        }
    };
    let origin = point(
        screen.origin.x + screen.size.width - size.width,
        screen.origin.y + (screen.size.height - size.height) / 2.,
    );
    Bounds { origin, size }
}

/// Opens the notch on the primary display.
pub fn open_rail(store: Entity<Store>, cx: &mut App) {
    let Some(display) = cx.primary_display() else {
        return;
    };
    let screen = display.bounds();
    let bounds = rail_bounds(screen, store.read(cx));
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

fn paint_arc(window: &mut Window, center: Point<Pixels>, radius: f32, to: f32, color: Rgba) {
    if to <= 0. {
        return;
    }
    let steps = (to * 96.).ceil().max(2.) as usize;
    let mut builder = PathBuilder::stroke(px(4.));
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
            .text_color(rgb(0xf5f5f5))
            .into_any_element(),
        None => div()
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .child(provider_mark(provider))
            .into_any_element(),
    }
}

fn ring(value: Option<f64>, provider: &str, stale: bool) -> impl IntoElement {
    let color = usage_color(value);
    div()
        .relative()
        .size(px(48.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let center = bounds.center();
                    paint_arc(window, center, 20., 1., rgb(0x2b2b2b));
                    if let Some(v) = value {
                        paint_arc(window, center, 20., (v / 100.).clamp(0., 1.) as f32, color);
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .child(provider_icon(provider, 22.))
        .when(stale, |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(2.))
                    .right(px(2.))
                    .size(px(8.))
                    .rounded_full()
                    .bg(rgb(0xefa44f)),
            )
        })
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
            screen,
            hovered: false,
            applied: None,
        };
        rail.apply(window, cx);
        rail
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.store.read(cx);
        let next = (rail_bounds(self.screen, s), s.visible());
        if self.applied == Some(next) {
            return;
        }
        self.applied = Some(next);
        desktop::place(window, next.0, next.1, cx);
        if !next.1 && s.popover.is_some() {
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
        if s.collapsed {
            let store = self.store.clone();
            return div()
                .id("peek")
                .size_full()
                .bg(rgb(0x000000))
                .rounded_l(px(6.))
                .border_1()
                .border_color(rgba(0xffffff28))
                .cursor_pointer()
                .on_click(move |_, _, cx| set_hover(&store, 0, true, cx));
        }
        let auto_hide = s.config.settings.auto_hide;
        let fold_store = self.store.clone();
        let accounts = s.enabled();
        let cells: Vec<_> = accounts
            .iter()
            .enumerate()
            .map(|(index, account)| {
                let reading = s.readings.get(&account.id);
                let value = headline(reading).map(|w| w.used_percent);
                let stale = reading.is_some_and(|r| r.status != "ok");
                let selected = s.selected.as_deref() == Some(account.id.as_str());
                let store = self.store.clone();
                let id = account.id.clone();
                div()
                    .id(SharedString::from(account.id.clone()))
                    .h(px(CELL))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_xl()
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(0xffffff10)))
                    .when(selected, |d| d.bg(rgba(0xffffff18)))
                    .child(ring(value, &account.provider, stale))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child(value.map_or("—".into(), |v| format!("{v:.0}%"))),
                    )
                    .on_hover({
                        let (store, id) = (store.clone(), id.clone());
                        move |hovered, window, cx| {
                            if *hovered {
                                open_popover(&store, id.clone(), index, window.bounds(), cx)
                            }
                        }
                    })
                    .on_click(move |_, window, cx| {
                        open_popover(&store, id.clone(), index, window.bounds(), cx)
                    })
            })
            .collect();
        div()
            .id("rail")
            .size_full()
            .flex()
            .flex_col()
            .py(px(PAD))
            .px(px(6.))
            .bg(rgb(0x000000))
            .rounded_l(px(24.))
            .border_1()
            .border_color(rgba(0xffffff1c))
            .text_color(rgb(0xf5f5f5))
            .children(cells)
            .when(accounts.is_empty(), |d| {
                d.child(
                    div()
                        .h(px(CELL))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(0x858585))
                        .child("—"),
                )
            })
            .when(auto_hide, |d| {
                d.child(
                    div()
                        .id("fold")
                        .h(px(TOOLS))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_lg()
                        .cursor_pointer()
                        .text_color(rgb(0xa3a3a8))
                        .hover(|d| d.bg(rgba(0xffffff10)).text_color(rgb(0xf5f5f5)))
                        .child("»")
                        .on_click(move |_, _, cx| fold(&fold_store, cx)),
                )
            })
    }
}

fn popover_height(s: &Store, id: &str) -> f32 {
    let reading = s.readings.get(id);
    let windows = reading.map_or(0, |r| r.windows.len()).max(1) as f32;
    let warning = reading.is_some_and(|r| r.status != "ok" && !r.message.is_empty());
    // Padding, header, identity and footer, then one 16 px gap per row.
    let rows = windows + 2. + if warning { 1. } else { 0. };
    124. + rows * 16. + windows * 62. + if warning { 20. } else { 0. }
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

fn open_popover(
    store: &Entity<Store>,
    id: String,
    index: usize,
    rail: Bounds<Pixels>,
    cx: &mut App,
) {
    if store.read(cx).selected.as_deref() == Some(id.as_str()) {
        return;
    }
    close_popover(store, cx);
    let height = popover_height(store.read(cx), &id);
    let center = rail.origin.y + px(PAD + CELL * index as f32 + CELL / 2.);
    let bounds = Bounds {
        origin: point(
            rail.origin.x - px(POPOVER_WIDTH + POPOVER_GAP),
            center - px(height / 2.),
        ),
        size: Size {
            width: px(POPOVER_WIDTH),
            height: px(height),
        },
    };
    let handle = cx
        .open_window(popup_options(bounds, None), |window, cx| {
            cx.new(|cx| Popover::new(store.clone(), id.clone(), bounds, window, cx))
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
    bounds: Bounds<Pixels>,
    hovered: bool,
}

impl Popover {
    fn new(
        store: Entity<Store>,
        account_id: String,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        desktop::place(window, bounds, true, cx);
        cx.observe_in(&store, window, |this: &mut Self, store, window, cx| {
            let height = px(popover_height(store.read(cx), &this.account_id));
            if this.bounds.size.height != height {
                this.bounds.size.height = height;
                desktop::place(window, this.bounds, true, cx);
            }
            cx.notify();
        })
        .detach();
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
            bounds,
            hovered: false,
        }
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
        let account = s
            .config
            .accounts
            .iter()
            .find(|a| a.id == self.account_id)
            .cloned();
        let reading = s.readings.get(&self.account_id).cloned();
        let refreshing = s.refreshing;
        let Some(account) = account else {
            return div();
        };
        let muted = rgb(0xa3a3a8);

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_lg()
                    .child(div().mr_1().child(provider_icon(&account.provider, 22.)))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(provider_title(&account.provider)),
                    )
                    .child(div().text_color(muted).child(tr(french, "· usage", "Usage"))),
            )
            .child(
                div()
                    .id("close")
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(muted)
                    .hover(|d| d.bg(rgba(0xffffff14)))
                    .child("×")
                    .on_click(cx.listener(|this, _, _, cx| {
                        close_popover(&this.store, cx);
                        // The pointer is no longer over a popover: fold unless it is on the rail.
                        set_hover(&this.store, 1, false, cx);
                    })),
            );

        let identity = div()
            .flex()
            .items_center()
            .justify_between()
            .text_sm()
            .text_color(muted)
            .child(account.name.clone())
            .when_some(reading.as_ref().and_then(|r| r.plan.clone()), |d, plan| {
                d.child(
                    div()
                        .px_2()
                        .rounded_md()
                        .bg(rgb(0x252525))
                        .text_xs()
                        .text_color(rgb(0xd0d0d0))
                        .child(plan),
                )
            });

        let warning = reading
            .as_ref()
            .filter(|r| r.status != "ok" && !r.message.is_empty())
            .map(|r| {
                div()
                    .text_sm()
                    .text_color(rgb(0xefa44f))
                    .child(r.message.clone())
            });

        let windows: Vec<_> = reading
            .as_ref()
            .map(|r| r.windows.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|w| {
                let color = usage_color(Some(w.used_percent));
                let forecast = reading
                    .as_ref()
                    .and_then(|r| r.exhausts_at.get(&w.label).copied());
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_sm()
                            .child(div().font_weight(FontWeight::BOLD).child(w.label.clone()))
                            .child(
                                div()
                                    .text_color(muted)
                                    .child(reset_time(w.resets_at, now, french)),
                            ),
                    )
                    .child(
                        div()
                            .h(px(6.))
                            .w_full()
                            .rounded_full()
                            .bg(rgb(0x2b2b2b))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative((w.used_percent / 100.).clamp(0., 1.) as f32))
                                    .rounded_full()
                                    .bg(color),
                            ),
                    )
                    .child(div().text_sm().child(format!(
                        "{} {}",
                        percentage(w.used_percent),
                        tr(french, "utilisés", "used")
                    )))
                    .when_some(forecast, |d, at| {
                        d.child(div().text_xs().text_color(rgb(0xefa44f)).child(tr(
                            french,
                            &format!("Limite atteinte {}", reset_time(Some(at), now, true)),
                            &format!("Limit reached {}", reset_time(Some(at), now, false)),
                        )))
                    })
            })
            .collect();
        let empty = windows.is_empty();

        let store = self.store.clone();
        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .pt_3()
            .border_t_1()
            .border_color(rgba(0xffffff16))
            .text_xs()
            .text_color(muted)
            .child(freshness(reading.as_ref().and_then(|r| r.updated_at), now, french))
            .child(
                div()
                    .id("refresh")
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(0xffffff14)).text_color(rgb(0xf5f5f5)))
                    .child(if refreshing {
                        tr(french, "Actualisation…", "Refreshing…")
                    } else {
                        tr(french, "Actualiser", "Refresh")
                    })
                    .on_click(move |_, _, cx| {
                        let store = store.clone();
                        cx.spawn(async move |cx| store::poll(&store, true, cx).await)
                            .detach();
                    }),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(rgb(0x000000))
            .rounded_2xl()
            .border_1()
            .border_color(rgba(0xffffff1c))
            .text_color(rgb(0xf5f5f5))
            .child(header)
            .child(identity)
            .children(warning)
            .children(windows)
            .when(empty, |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(tr(french, "Pas encore de relevé", "No reading yet")),
                )
            })
            .child(footer)
    }
}
