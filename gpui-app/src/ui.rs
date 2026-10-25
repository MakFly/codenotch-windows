use crate::model::{Reading, Window as Quota};
use crate::store::{self, Store};
use gpui::{
    canvas, div, point, prelude::*, px, relative, rgb, rgba, App, Bounds, Context, DisplayId,
    Entity, FontWeight, MouseButton, PathBuilder, Pixels, Point, Rgba, SharedString, Size, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
};
use std::f32::consts::{FRAC_PI_2, TAU};
use std::time::Duration;

const RAIL_WIDTH: f32 = 76.;
const CELL: f32 = 84.;
const PAD: f32 = 12.;
const POPOVER_WIDTH: f32 = 300.;
const POPOVER_GAP: f32 = 8.;

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

fn provider_mark(provider: &str) -> &'static str {
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

fn tr(french: bool, fr: &str, en: &str) -> String {
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

/// Opens the notch on the right edge of the primary display, vertically centered.
pub fn open_rail(store: Entity<Store>, cx: &mut App) {
    let Some(display) = cx.primary_display() else {
        return;
    };
    let count = store.read(cx).enabled().len().max(1);
    let size = Size {
        width: px(RAIL_WIDTH),
        height: px(PAD * 2. + CELL * count as f32),
    };
    let screen = display.bounds();
    let origin = point(
        screen.origin.x + screen.size.width - size.width,
        screen.origin.y + (screen.size.height - size.height) / 2.,
    );
    cx.open_window(
        popup_options(Bounds { origin, size }, Some(display.id())),
        |_, cx| cx.new(|cx| Rail::new(store, cx)),
    )
    .expect("notch window");
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

fn ring(value: Option<f64>, mark: &'static str, stale: bool) -> impl IntoElement {
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
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .child(mark),
        )
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
}

impl Rail {
    fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        Self { store }
    }
}

impl Render for Rail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
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
                    .child(ring(value, provider_mark(&account.provider), stale))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child(value.map_or("—".into(), |v| format!("{v:.0}%"))),
                    )
                    .on_click(move |_, window, cx| {
                        toggle_popover(&store, id.clone(), index, window.bounds(), cx)
                    })
            })
            .collect();
        div()
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
            // Prototype only: there is no tray icon yet, so a right click quits.
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.quit())
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
    }
}

fn popover_height(s: &Store, id: &str) -> f32 {
    let reading = s.readings.get(id);
    let windows = reading.map_or(0, |r| r.windows.len()).max(1) as f32;
    let warning = reading.is_some_and(|r| r.status != "ok" && !r.message.is_empty());
    // Padding, header, identity and footer, then one 16 px gap per row.
    let rows = windows + 2. + if warning { 1. } else { 0. };
    110. + rows * 16. + windows * 62. + if warning { 20. } else { 0. }
}

fn close_popover(store: &Entity<Store>, cx: &mut App) {
    let handle = store.update(cx, |s, cx| {
        s.selected = None;
        cx.notify();
        s.popover.take()
    });
    if let Some(handle) = handle {
        handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

fn toggle_popover(
    store: &Entity<Store>,
    id: String,
    index: usize,
    rail: Bounds<Pixels>,
    cx: &mut App,
) {
    let same = store.read(cx).selected.as_deref() == Some(id.as_str());
    close_popover(store, cx);
    if same {
        return;
    }
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
            cx.new(|cx| Popover::new(store.clone(), id.clone(), window, cx))
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
}

impl Popover {
    fn new(store: Entity<Store>, account_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_in(&store, window, |this: &mut Self, store, window, cx| {
            let height = popover_height(store.read(cx), &this.account_id);
            window.resize(Size {
                width: px(POPOVER_WIDTH),
                height: px(height),
            });
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
        Self { store, account_id }
    }
}

impl Render for Popover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .gap_1()
                    .text_lg()
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
                    .on_click(cx.listener(|this, _, _, cx| close_popover(&this.store, cx))),
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
