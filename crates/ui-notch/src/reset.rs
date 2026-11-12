//! Reset card (visual-spec §8): a short card next to the notch saying a quota window renewed.
//! Shown 6 s or until dismissed; several in a row are queued.

use crate::dims::*;
use crate::geom::{cubic, Pt, Rect};
use crate::glyphs::{self, Glyph};
use crate::text::tf;
use crate::win32::{self, WinOps};
use chrono::TimeZone;
use codenotch_core::config::Edge;
use codenotch_core::format::reset_card;
use codenotch_core::i18n::tr_label;
use codenotch_core::model::{ProviderKind, Snapshot};
use codenotch_ui_kit::window::{self as kwin, DeviceRect};
use codenotch_ui_kit::{Theme, UiModel};
use gpui::{
    canvas, div, point, prelude::FluentBuilder, px, size, App, AppContext, Bounds, Context, Entity,
    FontWeight, InteractiveElement, IntoElement, ParentElement, PathBuilder, Render,
    StatefulInteractiveElement, Styled, Window, WindowOptions,
};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Where the notch is on screen, published by the notch view for the reset card.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotchScreen {
    /// Visible pill (or rest pill) in device px.
    pub pill: DeviceRect,
    pub edge: Edge,
    /// Notch size scale × monitor scale.
    pub density: f32,
    /// Work area of the notch's monitor.
    pub work: DeviceRect,
    /// The pill is on screen.
    pub visible: bool,
}

static SCREEN: Mutex<Option<NotchScreen>> = Mutex::new(None);

pub(crate) fn publish_screen(s: NotchScreen) {
    if let Ok(mut g) = SCREEN.lock() {
        if g.as_ref() != Some(&s) {
            *g = Some(s);
        }
    }
}

pub(crate) fn screen() -> Option<NotchScreen> {
    SCREEN.lock().ok().and_then(|g| *g)
}

/// Text of one reset card.
#[derive(Debug, Clone, PartialEq)]
pub struct ResetContent {
    pub provider: ProviderKind,
    /// "Claude renewed"
    pub title: String,
    /// "Current session renewed"
    pub subtitle: String,
    /// "Quota available · 100%"
    pub status: String,
    /// Next reset of that window ("Resets at 4:59 AM"); empty line hidden.
    pub next: Option<String>,
}

/// Build the card text from the snapshot (pure).
pub fn reset_content<Tz: TimeZone>(
    snap: &Snapshot,
    account_id: &str,
    window_id: &str,
    now: i64,
    tz: &Tz,
    clock24: bool,
) -> ResetContent {
    let lang = snap.lang();
    let view = snap.account(account_id);
    let provider = view.map(|v| v.account.provider.clone()).unwrap_or_default();
    let w = view
        .and_then(|v| v.reading.as_ref())
        .and_then(|r| r.window(window_id));
    let label = w.map_or_else(
        || window_id.to_string(),
        |w| tr_label(lang, &w.label_key).into_owned(),
    );
    let left = w
        .and_then(|w| w.remaining_percent())
        .map_or(100.0, f64::round);
    ResetContent {
        title: tf(
            lang,
            "{provider} renewed",
            &[("provider", provider.display_name())],
        ),
        subtitle: tf(lang, "{window} renewed", &[("window", &label)]),
        status: tf(
            lang,
            "Quota available · {n}%",
            &[("n", &format!("{left:.0}"))],
        ),
        next: w
            .and_then(|w| reset_card(lang, w.resets_at.filter(|at| *at > now), now, tz, clock24)),
        provider,
    }
}

/// Card and tail inside the 280 × 150 window (logical px at density 1), per edge (§8).
/// `None` tail when the notch is hidden.
pub fn reset_layout(edge: Edge, notch_visible: bool) -> (Rect, Option<Vec<Pt>>) {
    let card = |x: f32, y: f32| Rect::new(x, y, RESET_CARD_W, RESET_CARD_H);
    if !notch_visible {
        return (card(17.0, 11.0), None);
    }
    // Upright tail, tip pointing right: M0,0 C0,8 19,12 34,16 C19,20 0,24 0,32 Z
    let mut upright = cubic(
        (0.0, 0.0),
        (0.0, 8.0),
        (19.0, 12.0),
        (RESET_TAIL_LEN, 16.0),
        12,
    );
    upright.extend(
        cubic(
            (RESET_TAIL_LEN, 16.0),
            (19.0, 20.0),
            (0.0, 24.0),
            (0.0, RESET_TAIL_SPAN),
            12,
        )
        .into_iter()
        .skip(1),
    );
    let k = RESET_TAIL_FLAT_LEN / RESET_TAIL_LEN;
    let sx = 34.0 / RESET_TAIL_SPAN;
    match edge {
        Edge::Right => (
            card(0.0, 11.0),
            Some(upright.iter().map(|(x, y)| (245.0 + x, 59.0 + y)).collect()),
        ),
        Edge::Left => (
            card(34.0, 11.0),
            Some(
                upright
                    .iter()
                    .map(|(x, y)| (1.0 + RESET_TAIL_LEN - x, 59.0 + y))
                    .collect(),
            ),
        ),
        Edge::Top => (
            card(17.0, 22.0),
            Some(
                upright
                    .iter()
                    .map(|(x, y)| (123.0 + y * sx, RESET_TAIL_FLAT_LEN - x * k))
                    .collect(),
            ),
        ),
        Edge::Bottom => (
            card(17.0, 0.0),
            Some(
                upright
                    .iter()
                    .map(|(x, y)| (123.0 + y * sx, 127.0 + x * k))
                    .collect(),
            ),
        ),
    }
}

/// Window rect (device px) of a `w × h` reset card for the notch at `screen`: against the pill's
/// inner side and centred on it; 8 px from the work-area edge when the notch is hidden. Kept
/// inside the work area.
pub fn reset_window_rect(screen: &NotchScreen, w: i32, h: i32) -> DeviceRect {
    let p = screen.pill;
    let work = screen.work;
    let gap = (RESET_EDGE_GAP_HIDDEN * screen.density).round() as i32;
    let (cx, cy) = (p.x + p.w / 2, p.y + p.h / 2);
    let (x, y) = match (screen.edge, screen.visible) {
        (Edge::Right, true) => (p.x - w, cy - h / 2),
        (Edge::Left, true) => (p.right(), cy - h / 2),
        (Edge::Top, true) => (cx - w / 2, p.bottom()),
        (Edge::Bottom, true) => (cx - w / 2, p.y - h),
        (Edge::Right, false) => (work.right() - gap - w, cy - h / 2),
        (Edge::Left, false) => (work.x + gap, cy - h / 2),
        (Edge::Top, false) => (cx - w / 2, work.y + gap),
        (Edge::Bottom, false) => (cx - w / 2, work.bottom() - gap - h),
    };
    let x = x.clamp(work.x, (work.right() - w).max(work.x));
    let y = y.clamp(work.y, (work.bottom() - h).max(work.y));
    DeviceRect { x, y, w, h }
}

/// Hit region of the reset window in device px at `density`: the card (plus its 5 px entrance
/// slide) and the tail's bounding box, so the transparent corners stay click-through.
pub fn reset_region(edge: Edge, notch_visible: bool, density: f32) -> Vec<DeviceRect> {
    let (card, tail) = reset_layout(edge, notch_visible);
    let dev = |r: Rect| {
        let x = (r.x * density).floor() as i32;
        let y = (r.y * density).floor() as i32;
        DeviceRect {
            x,
            y,
            w: (r.right() * density).ceil() as i32 - x,
            h: (r.bottom() * density).ceil() as i32 - y,
        }
    };
    let mut v = vec![dev(Rect::new(card.x, card.y, card.w, card.h + 5.0))];
    if let Some(t) = tail {
        let (mut l, mut tp, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in t {
            (l, tp, r, b) = (l.min(x), tp.min(y), r.max(x), b.max(y + 5.0));
        }
        v.push(dev(Rect::ltrb(l, tp, r, b)));
    }
    v
}

// -------------------------------------------------------------------------------------------
// Queue and window
// -------------------------------------------------------------------------------------------

struct Pending {
    model: Entity<UiModel>,
    account_id: String,
    window_id: String,
}

thread_local! {
    static QUEUE: RefCell<VecDeque<Pending>> = const { RefCell::new(VecDeque::new()) };
    static SHOWING: RefCell<bool> = const { RefCell::new(false) };
}

/// Queue a card; shown at once when none is on screen.
pub(crate) fn enqueue(model: &Entity<UiModel>, account_id: &str, window_id: &str, cx: &mut App) {
    QUEUE.with(|q| {
        q.borrow_mut().push_back(Pending {
            model: model.clone(),
            account_id: account_id.to_string(),
            window_id: window_id.to_string(),
        })
    });
    if !SHOWING.with(|s| *s.borrow()) {
        show_next(cx);
    }
}

fn show_next(cx: &mut App) {
    let Some(next) = QUEUE.with(|q| q.borrow_mut().pop_front()) else {
        SHOWING.with(|s| *s.borrow_mut() = false);
        return;
    };
    SHOWING.with(|s| *s.borrow_mut() = true);
    let snap = &next.model.read(cx).snapshot;
    let settings = snap.settings().clone();
    let now = codenotch_core::now_secs();
    let lang = snap.lang();
    let content = reset_content(
        snap,
        &next.account_id,
        &next.window_id,
        now,
        &chrono::Local,
        win32::clock24(lang),
    );
    let glyph = glyphs::resolve_now(&content.provider, Some(&glyphs::user_dir()));
    let scr = screen().unwrap_or_else(|| fallback_screen(&settings));
    let notch_scale = settings.notch_size.scale();
    let mon_scale = if notch_scale > 0.0 {
        scr.density / notch_scale
    } else {
        1.0
    };
    let w = (RESET_WIN_W * scr.density).round() as i32;
    let h = (RESET_WIN_H * scr.density).round() as i32;
    let rect = reset_window_rect(&scr, w, h);
    let bounds = Bounds {
        origin: point(px(rect.x as f32 / mon_scale), px(rect.y as f32 / mon_scale)),
        size: size(px(RESET_WIN_W * notch_scale), px(RESET_WIN_H * notch_scale)),
    };
    let opts = WindowOptions {
        show: false,
        ..kwin::overlay_options(bounds)
    };
    let view = ResetCardView {
        content,
        model: next.model.clone(),
        glyph,
        scale: notch_scale,
        edge: scr.edge,
        notch_visible: scr.visible,
        rect,
        start: Instant::now(),
        placed: false,
        dismiss_hover: false,
    };
    if settings.reset_sound {
        win32::beep();
    }
    match cx.open_window(opts, |window, cx| {
        // The theme is read on each frame: a theme change in Settings or an OS light/dark
        // switch repaints the card live.
        let entity = cx.new(|cx| {
            cx.observe(&view.model, |_, _, cx| cx.notify()).detach();
            cx.observe_window_appearance(window, |_, _, cx| cx.notify())
                .detach();
            view
        });
        let weak = entity.downgrade();
        window
            .spawn(cx, async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(
                        codenotch_ui_kit::metrics::RESET_CARD_SHOW_MS,
                    ))
                    .await;
                let _ = weak.update_in(cx, |_, window, cx| close(window, cx));
            })
            .detach();
        entity
    }) {
        Ok(_) => {}
        Err(err) => {
            log::warn!("reset card: cannot open window: {err}");
            SHOWING.with(|s| *s.borrow_mut() = false);
        }
    }
}

fn close(window: &mut Window, cx: &mut App) {
    window.remove_window();
    cx.defer(show_next);
}

fn fallback_screen(settings: &codenotch_core::config::Settings) -> NotchScreen {
    let mon = kwin::monitor_or_primary(&settings.widget_placement.monitor);
    let (work, scale) = mon.map_or(
        (
            DeviceRect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1040,
            },
            1.0,
        ),
        |m| (m.work, m.scale),
    );
    NotchScreen {
        pill: DeviceRect {
            x: work.right(),
            y: work.y + work.h / 2,
            w: 0,
            h: 0,
        },
        edge: settings.widget_position,
        density: settings.notch_size.scale() * scale,
        work,
        visible: false,
    }
}

struct ResetCardView {
    content: ResetContent,
    /// Source of the settings: the card follows the app theme ([`Theme::for_settings`]).
    model: Entity<UiModel>,
    glyph: Glyph,
    scale: f32,
    edge: Edge,
    notch_visible: bool,
    rect: DeviceRect,
    start: Instant,
    placed: bool,
    dismiss_hover: bool,
}

const IN_MS: f32 = 180.0;

impl Render for ResetCardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.placed {
            if let Some(hwnd) = kwin::hwnd(window) {
                self.placed = true;
                let region = reset_region(
                    self.edge,
                    self.notch_visible,
                    self.rect.w as f32 / RESET_WIN_W,
                );
                let ops = WinOps {
                    place: Some(self.rect),
                    region: Some(region),
                    show: Some(true),
                    styles: true,
                    ..WinOps::default()
                };
                cx.spawn(async move |_, _| ops.apply(hwnd)).detach();
            }
        }
        let s = self.scale;
        let t = Theme::for_settings(self.model.read(cx).snapshot.settings(), cx);
        let elapsed = self.start.elapsed().as_secs_f32() * 1000.0;
        let reduced = win32::reduced_motion();
        let k = if reduced {
            1.0
        } else {
            crate::easing::Bezier::EASE_OUT.at((elapsed / IN_MS).clamp(0.0, 1.0))
        };
        if k < 1.0 {
            window.request_animation_frame();
        }
        let (card, tail) = reset_layout(self.edge, self.notch_visible);
        let dy = (1.0 - k) * 5.0 * s;
        let muted = t.muted_foreground;
        // Same severity palette as the notch's rings.
        let ample = t.ample;
        let hover_bg = t.accent;
        let tail_pts: Option<Vec<Pt>> =
            tail.map(|v| v.into_iter().map(|(x, y)| (x * s, y * s + dy)).collect());
        // The pill's material: same fill, `edge` hairline around card + tail, no seam.
        let card_color = t.pill;
        let line = t.edge;

        let glyph_el = crate::notch::glyph_element(&self.glyph, 21.0 * s, t.foreground, 15.0 * s);
        let x_color = if self.dismiss_hover { t.foreground } else { muted };
        let body = div()
            .flex()
            .flex_col()
            .size_full()
            .px(px(15.0 * s))
            .py(px(13.0 * s))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(9.0 * s))
                    .child(glyph_el)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(14.0 * s))
                                    .line_height(px(19.0 * s))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(t.foreground)
                                    .truncate()
                                    .child(self.content.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0 * s))
                                    .line_height(px(15.0 * s))
                                    .text_color(muted)
                                    .truncate()
                                    .child(self.content.subtitle.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("dismiss")
                            .flex_none()
                            .w(px(30.0 * s))
                            .h(px(30.0 * s))
                            .mt(px(-4.0 * s))
                            .mr(px(-6.0 * s))
                            .rounded(px(7.0 * s))
                            .when(self.dismiss_hover, |d| d.bg(hover_bg))
                            .on_hover(cx.listener(|v, hovered: &bool, _, cx| {
                                v.dismiss_hover = *hovered;
                                cx.notify();
                            }))
                            .on_click(cx.listener(|_, _, window, cx| close(window, cx)))
                            .child(cross(13.0 * s, 1.8 * s, x_color)),
                    ),
            )
            .child(
                div()
                    .mt(px(13.0 * s))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0 * s))
                    .text_size(px(12.0 * s))
                    .line_height(px(16.0 * s))
                    .text_color(ample)
                    .child(
                        div()
                            .flex_none()
                            .w(px(8.0 * s))
                            .h(px(8.0 * s))
                            .rounded_full()
                            .bg(ample),
                    )
                    .child(self.content.status.clone()),
            )
            .when_some(self.content.next.clone(), |d, next| {
                d.child(
                    div()
                        .mt(px(8.0 * s))
                        .ml(px(16.0 * s))
                        .text_size(px(11.0 * s))
                        .line_height(px(15.0 * s))
                        .text_color(muted)
                        .truncate()
                        .child(next),
                )
            });

        div()
            .size_full()
            .relative()
            .font_family("Segoe UI")
            .text_size(px(13.0 * s))
            .opacity(k)
            .child(
                div()
                    .absolute()
                    .left(px(card.x * s))
                    .top(px(card.y * s + dy))
                    .w(px(card.w * s))
                    .h(px(card.h * s))
                    .bg(card_color)
                    .border_1()
                    .border_color(line)
                    .rounded(px(16.0 * s))
                    .child(body),
            )
            // The tail goes over the card so it covers the card's hairline across its base.
            .when_some(tail_pts, |d, pts| {
                let centre = (
                    (card.x + card.w / 2.0) * s,
                    (card.y + card.h / 2.0) * s + dy,
                );
                d.child(
                    canvas(
                        |_, _, _| {},
                        move |b, _, window, _| {
                            let o = b.origin;
                            let rim = crate::geom::grow_from_base(&pts, 1.5 * s);
                            let fill = sink_base(&pts, centre, 2.0 * s);
                            for (poly, color) in [(&rim, line), (&fill, card_color)] {
                                let mut pb = PathBuilder::fill();
                                let pp: Vec<_> = poly
                                    .iter()
                                    .map(|(x, y)| point(o.x + px(*x), o.y + px(*y)))
                                    .collect();
                                pb.add_polygon(&pp, true);
                                if let Ok(path) = pb.build() {
                                    window.paint_path(path, color);
                                }
                            }
                        },
                    )
                    .absolute()
                    .left_0()
                    .top_0()
                    .size_full(),
                )
            })
    }
}

/// The tail with its two base points pushed `by` px toward the card centre, so the fill
/// overlaps the card's border.
fn sink_base(tail: &[Pt], centre: Pt, by: f32) -> Vec<Pt> {
    let (Some(&a), Some(&b)) = (tail.first(), tail.last()) else {
        return Vec::new();
    };
    let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let (dx, dy) = (centre.0 - mid.0, centre.1 - mid.1);
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    let (ux, uy) = (dx / len * by, dy / len * by);
    let mut out = Vec::with_capacity(tail.len() + 2);
    out.push((a.0 + ux, a.1 + uy));
    out.extend_from_slice(tail);
    out.push((b.0 + ux, b.1 + uy));
    out
}

/// "×" drawn as two round-capped strokes.
fn cross(size_px: f32, stroke: f32, color: gpui::Hsla) -> impl IntoElement {
    let box_px = 30.0 * size_px / 13.0;
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let c = b.center();
            let h = size_px / 2.0 - stroke / 2.0;
            for (dx, dy) in [(1.0f32, 1.0f32), (1.0, -1.0)] {
                let a = (f32::from(c.x) - h * dx, f32::from(c.y) - h * dy);
                let z = (f32::from(c.x) + h * dx, f32::from(c.y) + h * dy);
                let mut pb = PathBuilder::stroke(px(stroke));
                pb.move_to(point(px(a.0), px(a.1)));
                pb.line_to(point(px(z.0), px(z.1)));
                if let Ok(path) = pb.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .w(px(box_px))
    .h(px(box_px))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;
    use codenotch_core::demo::ids;

    #[test]
    fn content_from_demo() {
        let now = 1_800_000_000;
        let s = Snapshot::demo_at(now);
        let utc = FixedOffset::east_opt(0).unwrap();
        let c = reset_content(&s, ids::CLAUDE, "session", now, &utc, false);
        assert_eq!(c.title, "Claude renewed");
        assert_eq!(c.subtitle, "Current session renewed");
        assert_eq!(c.status, "Quota available · 39%");
        assert!(c.next.as_deref().unwrap().starts_with("Resets at "));
        // unknown window: label falls back to the id, 100 % available
        let c = reset_content(&s, ids::CLAUDE, "gone", now, &utc, false);
        assert_eq!(c.subtitle, "gone renewed");
        assert_eq!(c.status, "Quota available · 100%");
        assert!(c.next.is_none());
        let mut fr = s.clone();
        fr.config.settings.language = "fr".into();
        let c = reset_content(&fr, ids::CODEX, "secondary", now, &utc, true);
        assert_eq!(c.title, "Codex renouvelé");
        assert_eq!(c.subtitle, "Limite hebdomadaire renouvelée");
        assert_eq!(c.status, "Quota disponible · 43 %");
    }

    #[test]
    fn layout_per_edge() {
        let (card, tail) = reset_layout(Edge::Right, true);
        assert_eq!((card.x, card.y, card.w, card.h), (0.0, 11.0, 246.0, 128.0));
        let tail = tail.unwrap();
        let tip = tail.iter().map(|p| p.0).fold(f32::MIN, f32::max);
        assert!((tip - 279.0).abs() < 1e-3);
        let (card, tail) = reset_layout(Edge::Left, true);
        assert_eq!(card.x, 34.0);
        assert!((tail.unwrap().iter().map(|p| p.0).fold(f32::MAX, f32::min) - 1.0).abs() < 1e-3);
        let (card, tail) = reset_layout(Edge::Top, true);
        assert_eq!((card.x, card.y), (17.0, 22.0));
        assert!(tail
            .unwrap()
            .iter()
            .all(|p| p.1 >= -1e-3 && p.1 <= 23.0 + 1e-3));
        let (card, tail) = reset_layout(Edge::Bottom, true);
        assert_eq!((card.x, card.y), (17.0, 0.0));
        assert!(tail
            .unwrap()
            .iter()
            .all(|p| p.1 >= 127.0 - 1e-3 && p.1 <= 150.0 + 1e-3));
        let (card, tail) = reset_layout(Edge::Right, false);
        assert_eq!((card.x, card.y), (17.0, 11.0));
        assert!(tail.is_none());
    }

    #[test]
    fn region_covers_card_and_tail_only() {
        let r = reset_region(Edge::Right, true, 1.5);
        assert_eq!(r.len(), 2);
        assert_eq!(
            r[0],
            DeviceRect {
                x: 0,
                y: 16,
                w: 369,
                h: 200
            }
        );
        assert!(r[1].x >= 367 && r[1].right() <= 420);
        assert_eq!(reset_region(Edge::Top, false, 1.0).len(), 1);
    }

    #[test]
    fn window_next_to_the_pill() {
        let work = DeviceRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1040,
        };
        let scr = NotchScreen {
            pill: DeviceRect {
                x: 1850,
                y: 400,
                w: 70,
                h: 200,
            },
            edge: Edge::Right,
            density: 1.0,
            work,
            visible: true,
        };
        let r = reset_window_rect(&scr, 280, 150);
        assert_eq!((r.right(), r.y + r.h / 2), (1850, 500));
        let top = NotchScreen {
            edge: Edge::Top,
            pill: DeviceRect {
                x: 800,
                y: 0,
                w: 300,
                h: 95,
            },
            ..scr
        };
        let r = reset_window_rect(&top, 280, 150);
        assert_eq!((r.y, r.x + r.w / 2), (95, 950));
        let hidden = NotchScreen {
            visible: false,
            ..scr
        };
        let r = reset_window_rect(&hidden, 280, 150);
        assert_eq!(r.right(), 1912);
        // clamped inside the work area
        let low = NotchScreen {
            pill: DeviceRect {
                x: 1850,
                y: 1000,
                w: 70,
                h: 40,
            },
            ..scr
        };
        assert_eq!(reset_window_rect(&low, 280, 150).bottom(), 1040);
    }
}
