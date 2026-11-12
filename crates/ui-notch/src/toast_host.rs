//! The standalone toast window: hosts app toasts (updates…) while the settings window is closed.
//!
//! A transparent, topmost, non-activating overlay at the top centre of the notch's monitor work
//! area (just under the pill when the notch is on the top edge). Its window region is the union
//! of the toast cards, so everything around them stays click-through. It exists only while it
//! has toasts: it closes itself once its queue is empty.

use crate::reset::{self, NotchScreen};
use crate::win32::WinOps;
use codenotch_core::config::Edge;
use codenotch_ui_kit::toast::{self, ToastPosition, Toaster};
use codenotch_ui_kit::window::{self as kwin, DeviceRect};
use codenotch_ui_kit::{Theme, UiModel};
use gpui::{
    div, point, px, size, App, AppContext, Bounds, Context, Entity, Global, IntoElement,
    ParentElement, Render, Styled, Subscription, Task, WeakEntity, Window, WindowOptions,
};
use std::time::Duration;

/// Window size (logical px): room for three stacked toasts.
const WIN_W: f32 = 400.0;
const WIN_H: f32 = 240.0;
/// Distance of the stack from the window's top edge.
const MARGIN: f32 = 10.0;
/// Hit-region padding around each card (close badge, shadow).
const PAD: f32 = 10.0;
/// Gap between the work-area top (or the pill) and the window.
const GAP: f32 = 6.0;
/// How often the window checks whether it is empty.
const POLL: Duration = Duration::from_millis(400);

#[derive(Default)]
struct ToastWindow {
    open: Option<(WeakEntity<ToastHostView>, Entity<Toaster>)>,
}

impl Global for ToastWindow {}

/// The toaster for app toasts: the registered host (the settings window when open), else this
/// module's toast window, opened on demand. `None` only when no window can be opened.
pub fn toaster(model: &Entity<UiModel>, cx: &mut App) -> Option<Entity<Toaster>> {
    if let Some(t) = toast::host(cx) {
        return Some(t);
    }
    let existing = cx.default_global::<ToastWindow>().open.clone();
    if let Some((view, toaster)) = existing {
        if view.upgrade().is_some() {
            toast::set_host(&toaster, cx);
            return Some(toaster);
        }
        cx.default_global::<ToastWindow>().open = None;
    }
    open(model, cx)
}

/// Window rect (device px) for the toast window.
pub(crate) fn host_rect(screen: Option<&NotchScreen>, work: DeviceRect, scale: f32) -> DeviceRect {
    let w = (WIN_W * scale).round() as i32;
    let h = (WIN_H * scale).round() as i32;
    let gap = (GAP * scale).round() as i32;
    let (x, y) = match screen {
        Some(s) if s.visible && s.edge == Edge::Top => {
            (s.pill.x + s.pill.w / 2 - w / 2, s.pill.bottom() + gap)
        }
        _ => (work.x + (work.w - w) / 2, work.y + gap),
    };
    let x = x.clamp(work.x, (work.right() - w).max(work.x));
    let y = y.clamp(work.y, (work.bottom() - h).max(work.y));
    DeviceRect { x, y, w, h }
}

fn open(model: &Entity<UiModel>, cx: &mut App) -> Option<Entity<Toaster>> {
    let settings = model.read(cx).snapshot.settings().clone();
    let screen = reset::screen();
    let monitors = kwin::monitors();
    let monitor = screen
        .and_then(|s| monitors.iter().find(|m| m.work == s.work))
        .or_else(|| {
            monitors
                .iter()
                .find(|m| m.name == settings.widget_placement.monitor)
        })
        .or_else(|| monitors.iter().find(|m| m.primary))
        .or(monitors.first())
        .cloned();
    let (work, scale) = monitor.map_or(
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
    let rect = host_rect(screen.as_ref(), work, scale);
    let bounds = Bounds {
        origin: point(px(rect.x as f32 / scale), px(rect.y as f32 / scale)),
        size: size(px(WIN_W), px(WIN_H)),
    };
    let opts = WindowOptions {
        show: false,
        ..kwin::overlay_options(bounds)
    };
    let theme = Theme::for_settings(&settings, cx);
    let toaster = cx.new(|_| {
        Toaster::new(theme)
            .with_position(ToastPosition::TopCenter)
            .with_margin(MARGIN)
    });
    let mut created: Option<WeakEntity<ToastHostView>> = None;
    let t = toaster.clone();
    let m = model.clone();
    let result = cx.open_window(opts, |window, cx| {
        let view = cx.new(|cx| ToastHostView::new(m, t, rect, window, cx));
        created = Some(view.downgrade());
        view
    });
    match (result, created) {
        (Ok(_), Some(view)) => {
            toast::set_host(&toaster, cx);
            cx.default_global::<ToastWindow>().open = Some((view, toaster.clone()));
            Some(toaster)
        }
        (Err(err), _) => {
            log::warn!("toast window: cannot open: {err}");
            None
        }
        (Ok(_), None) => None,
    }
}

/// Hit rectangles (device px, window coordinates) of the toasts of `toaster`.
fn card_rects(toaster: &Toaster, scale: f32) -> Vec<DeviceRect> {
    let (_, margin) = toaster.placement();
    toaster
        .queue()
        .entries()
        .iter()
        .map(|e| {
            let (w, h) = e.size.unwrap_or((240.0, 40.0));
            let x = (WIN_W - w) / 2.0 - PAD;
            let y = margin + e.target() - PAD;
            let r = |v: f32| (v * scale).round() as i32;
            let x0 = r(x.max(0.0));
            let y0 = r(y.max(0.0));
            DeviceRect {
                x: x0,
                y: y0,
                w: (r((x + w + 2.0 * PAD).min(WIN_W)) - x0).max(1),
                h: (r((y + h + 2.0 * PAD).min(WIN_H)) - y0).max(1),
            }
        })
        .collect()
}

struct ToastHostView {
    model: Entity<UiModel>,
    toaster: Entity<Toaster>,
    rect: DeviceRect,
    placed: bool,
    region: Option<Vec<DeviceRect>>,
    _subs: Vec<Subscription>,
    _poll: Task<()>,
}

impl ToastHostView {
    fn new(
        model: Entity<UiModel>,
        toaster: Entity<Toaster>,
        rect: DeviceRect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subs = vec![
            cx.observe(&toaster, |_, _, cx| cx.notify()),
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.observe_window_appearance(window, |_, _, cx| cx.notify()),
        ];
        let weak = cx.entity().downgrade();
        let poll = window.spawn(cx, async move |cx| loop {
            cx.background_executor().timer(POLL).await;
            let alive = weak.update_in(cx, |this, window, cx| {
                if this.toaster.read(cx).queue().is_empty() {
                    let id = this.toaster.entity_id();
                    toast::clear_host(id, cx);
                    let global = cx.default_global::<ToastWindow>();
                    if global
                        .open
                        .as_ref()
                        .is_some_and(|(_, t)| t.entity_id() == id)
                    {
                        global.open = None;
                    }
                    window.remove_window();
                    false
                } else {
                    cx.notify();
                    true
                }
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        });
        Self {
            model,
            toaster,
            rect,
            placed: false,
            region: None,
            _subs: subs,
            _poll: poll,
        }
    }
}

impl Render for ToastHostView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_settings(self.model.read(cx).snapshot.settings(), cx);
        self.toaster.update(cx, |t, _| t.set_theme(theme));
        let region = card_rects(self.toaster.read(cx), window.scale_factor());
        if let Some(hwnd) = kwin::hwnd(window) {
            let mut ops = WinOps::default();
            if !self.placed {
                self.placed = true;
                ops.place = Some(self.rect);
                ops.styles = true;
                ops.show = Some(true);
            }
            if self.region.as_ref() != Some(&region) {
                self.region = Some(region.clone());
                ops.region = Some(region);
            }
            if !ops.is_empty() {
                cx.spawn(async move |_, _| ops.apply(hwnd)).detach();
            }
        }
        div().size_full().relative().child(self.toaster.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> DeviceRect {
        DeviceRect {
            x: 0,
            y: 0,
            w: 2560,
            h: 1392,
        }
    }

    #[test]
    fn top_centre_of_the_work_area_unless_the_notch_is_on_top() {
        let r = host_rect(None, work(), 1.0);
        assert_eq!(
            r,
            DeviceRect {
                x: 1080,
                y: 6,
                w: 400,
                h: 240
            }
        );
        let r = host_rect(None, work(), 1.5);
        assert_eq!((r.w, r.h, r.y), (600, 360, 9));
        let top = NotchScreen {
            pill: DeviceRect {
                x: 1000,
                y: 0,
                w: 200,
                h: 30,
            },
            edge: Edge::Top,
            density: 1.0,
            work: work(),
            visible: true,
        };
        let r = host_rect(Some(&top), work(), 1.0);
        assert_eq!((r.x, r.y), (900, 36));
        // A right-edge notch: top centre.
        let right = NotchScreen {
            edge: Edge::Right,
            ..top
        };
        assert_eq!(host_rect(Some(&right), work(), 1.0).x, 1080);
        // Clamped into the work area.
        let corner = NotchScreen {
            pill: DeviceRect {
                x: 2500,
                y: 0,
                w: 50,
                h: 30,
            },
            ..top
        };
        assert_eq!(host_rect(Some(&corner), work(), 1.0).x, 2160);
    }
}
