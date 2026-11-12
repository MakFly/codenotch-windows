//! Taskbar mode: the notch's cells as a strip over the free space of the primary taskbar, and
//! their hover card in a window of its own.
//!
//! Thin gpui layer over [`crate::strip_layout`] (geometry), [`crate::cell`] / [`crate::card`]
//! (content) and the app-installed [`crate::taskbar_probe`] (where the free space is).
//!
//! **Window model.** The strip is a transparent, borderless, non-activating, topmost tool
//! window (`ui-kit` overlay options + the notch's `WinOps`), sized and placed in device px
//! exactly on the rectangle the probe returns. It is never a child of Explorer's windows.
//! The taskbar is itself topmost and raises itself when clicked, so after every poll the strip
//! checks the z-order and, when `Shell_TrayWnd` is above it, re-inserts itself directly above the
//! taskbar (not at the very top, so Start, flyouts and other shell surfaces stay above it).
//! The hover card is a second overlay window, as tall as the tallest card, whose window region is
//! just the card and its tail (the rest is click-through).
//!
//! **Threading.** Everything runs on the UI thread. A 150 ms timer polls the probe (a cache read)
//! and the pointer (for the card's close grace); window moves, regions, visibility and z-order
//! fixes are applied from spawned tasks, outside gpui's `App` borrow (see `win32`).
//!
//! **Idle cost.** Frames only when something changes or animates (activity arcs step at 10â€“11
//! fps while an agent works or waits, the press on click).

use crate::card::{card_model, CardCtx, CardModel};
use crate::cell::{cell_model, CellModel};
use crate::easing;
use crate::geom::Rect;
use crate::glyphs::{self, Glyph};
use crate::notch::{card_content, figure_font, glyph_element};
use crate::reset::{publish_screen, NotchScreen};
use crate::strip_layout::{self as sl, CardFrame, Slot};
use crate::taskbar_probe::{self, TaskbarQuery, TaskbarSide, TaskbarStrip};
use crate::win32::{self, WinOps};
use codenotch_core::config::{Edge, Settings, WeeklyRing};
use codenotch_core::model::{ActivityState, Command};
use codenotch_ui_kit::metrics::{
    ring_units, CARD_PAD, CARD_RADIUS, RING_BOX, STALE_OPACITY, WEEKLY_ARC_OPACITY,
    WEEKLY_TRACK_OPACITY,
};
use codenotch_ui_kit::ring::paint_ring;
use codenotch_ui_kit::window::{self as kwin, DeviceRect};
use codenotch_ui_kit::{RingSpec, Theme, UiModel};
use gpui::prelude::FluentBuilder;
use gpui::{
    canvas, div, hsla, point, px, size, AnyElement, App, AppContext, Bounds, Context, Entity, Hsla,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, PathBuilder, Render, SharedString, StatefulInteractiveElement, Styled, Task,
    TextRun, WeakEntity, Window, WindowHandle, WindowOptions,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const POLL_MS: u64 = 150;
const CARD_GRACE_MS: f64 = codenotch_ui_kit::metrics::CARD_CLOSE_GRACE_MS as f64;
const PRESS_MS: f64 = 300.0;
const PRESS_MAX_MS: f64 = 6000.0;
const PRESS_SCALE: f32 = 0.93;
/// Strip height assumed before the first measurement (a 48 px taskbar less its insets).
const DEFAULT_HEIGHT: f32 = 42.0;

// ---------------------------------------------------------------------------------------------
// Controller: open / close the strip as `taskbarMode` changes
// ---------------------------------------------------------------------------------------------

thread_local! {
    static STRIP: RefCell<Option<WindowHandle<StripView>>> = const { RefCell::new(None) };
}

/// Follow the settings: open the strip window when taskbar mode becomes active, close it when
/// it stops. Call once from `open`.
pub(crate) fn install(model: &Entity<UiModel>, cx: &mut App) {
    sync(model, cx);
    cx.observe(model, |model, cx| sync(&model, cx)).detach();
}

fn sync(model: &Entity<UiModel>, cx: &mut App) {
    let want = taskbar_probe::active(model.read(cx).snapshot.settings());
    let open = STRIP.with(|s| *s.borrow());
    match (want, open) {
        (true, None) => {
            let bounds = Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(200.), px(DEFAULT_HEIGHT)),
            };
            let opts = WindowOptions {
                show: false,
                ..kwin::overlay_options(bounds)
            };
            let m = model.clone();
            match cx.open_window(opts, move |window, cx| {
                cx.new(|cx| StripView::new(m, window, cx))
            }) {
                Ok(handle) => STRIP.with(|s| *s.borrow_mut() = Some(handle)),
                Err(err) => log::error!("taskbar strip: cannot open its window: {err}"),
            }
        }
        (false, Some(handle)) => {
            STRIP.with(|s| *s.borrow_mut() = None);
            let _ = handle.update(cx, |view, window, cx| {
                view.close_card(cx);
                window.remove_window();
            });
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// Strip
// ---------------------------------------------------------------------------------------------

struct Derived {
    settings: Settings,
    /// Strip colours always follow the taskbar's own light/dark mode (whatever the app theme
    /// setting), falling back to the app theme only when the taskbar mode is unknown.
    theme: Theme,
    /// The card follows the app theme, like the notch and its card.
    card_theme: Theme,
    size: f32,
    side: Option<TaskbarSide>,
    hidden: bool,
    outside: bool,
    cells: Vec<CellModel>,
    cards: Vec<CardModel>,
    glyphs: Vec<Glyph>,
    /// Primary monitor (bounds, work area, scale), refreshed with the snapshot.
    monitor: Option<(DeviceRect, DeviceRect, f32)>,
}

pub(crate) struct StripView {
    model: Entity<UiModel>,
    epoch: Instant,
    hwnd: Option<isize>,
    dirty: bool,
    d: Derived,
    reduced: bool,
    light_taskbar: Option<bool>,
    glyph_cache: HashMap<String, Glyph>,
    /// Figure â†’ width at a 1 px font, measured by the text system.
    ems: HashMap<String, f32>,
    place: Option<TaskbarStrip>,
    applied: Option<DeviceRect>,
    applied_visible: Option<bool>,
    pending: WinOps,
    slots: Vec<Slot>,
    hover: Option<usize>,
    left_at: Option<f64>,
    down: Option<usize>,
    presses: Vec<(String, f64)>,
    signing_in: HashSet<String>,
    card: Option<(WindowHandle<CardView>, Entity<CardView>)>,
    card_opening: bool,
    wake: Option<(f64, Task<()>)>,
    _ticker: Task<()>,
}

impl StripView {
    fn new(model: Entity<UiModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |v, _, cx| {
            v.dirty = true;
            v.poll(cx);
            cx.notify();
        })
        .detach();
        cx.observe_window_appearance(window, |v, _, cx| {
            v.dirty = true;
            cx.notify();
        })
        .detach();
        let ticker = cx.spawn(async move |this, cx| {
            let mut n: u32 = 0;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(POLL_MS))
                    .await;
                n = n.wrapping_add(1);
                // Every ~10 s: taskbar theme, monitors, reduced motion, clock-dependent text.
                let refresh = n.is_multiple_of(66);
                if this
                    .update(cx, |v, cx| {
                        if refresh {
                            v.dirty = true;
                            v.reduced = win32::reduced_motion();
                            v.light_taskbar = taskbar_light();
                        }
                        v.poll(cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut view = Self {
            model,
            epoch: Instant::now(),
            hwnd: None,
            dirty: true,
            d: Derived {
                settings: Settings::default(),
                theme: Theme::default(),
                card_theme: Theme::default(),
                size: 1.0,
                side: None,
                hidden: true,
                outside: false,
                cells: Vec::new(),
                cards: Vec::new(),
                glyphs: Vec::new(),
                monitor: None,
            },
            reduced: win32::reduced_motion(),
            light_taskbar: taskbar_light(),
            glyph_cache: HashMap::new(),
            ems: HashMap::new(),
            place: None,
            applied: None,
            applied_visible: None,
            pending: WinOps::default(),
            slots: Vec::new(),
            hover: None,
            left_at: None,
            down: None,
            presses: Vec::new(),
            signing_in: HashSet::new(),
            card: None,
            card_opening: false,
            wake: None,
            _ticker: ticker,
        };
        view.derive(cx);
        view
    }

    fn now_ms(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64() * 1000.0
    }

    fn send(&self, command: Command, cx: &App) {
        self.model.read(cx).send(command);
    }

    fn derive(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        let model = self.model.clone();
        let app: &App = cx;
        let snap = &model.read(app).snapshot;
        let settings = snap.settings().clone();
        let lang = snap.lang();
        let now = codenotch_core::now_secs();
        let card_theme = Theme::for_settings(&settings, app);
        let theme = strip_theme(self.light_taskbar, card_theme);
        let views = snap.notch_accounts();
        let clock24 = win32::clock24(lang);
        let tz = chrono::Local;
        let user_dir = glyphs::user_dir();
        let mut cells = Vec::with_capacity(views.len());
        let mut cards = Vec::with_capacity(views.len());
        let mut glyph_list = Vec::with_capacity(views.len());
        for v in &views {
            let multi = views
                .iter()
                .filter(|o| o.account.provider == v.account.provider)
                .count()
                > 1;
            let signing_in = self.signing_in.contains(&v.account.id);
            cells.push(cell_model(v, &settings, now));
            let ctx = CardCtx {
                lang,
                settings: &settings,
                now,
                tz: &tz,
                clock24,
                multi,
                signing_in,
            };
            cards.push(card_model(v, &ctx));
            let key = v.account.provider.id().to_string();
            let g = self
                .glyph_cache
                .entry(key)
                .or_insert_with(|| glyphs::resolve_now(&v.account.provider, Some(&user_dir)))
                .clone();
            glyph_list.push(g);
        }
        self.signing_in.retain(|id| {
            snap.account(id)
                .and_then(|v| v.reading.as_ref())
                .is_some_and(|r| r.status == codenotch_core::model::ReadingStatus::SignedOut)
        });
        let outside =
            settings.weekly_ring == WeeklyRing::Outside && cells.iter().any(|c| c.weekly.is_some());
        let monitor = kwin::monitors()
            .into_iter()
            .find(|m| m.primary)
            .map(|m| (m.bounds, m.work, m.scale));
        self.d = Derived {
            size: settings.notch_size.scale(),
            side: taskbar_probe::side(&settings),
            hidden: crate::notch::notch_hidden(snap),
            outside,
            theme,
            card_theme,
            cells,
            cards,
            glyphs: glyph_list,
            monitor,
            settings,
        };
        if self.hover.is_some_and(|i| i >= self.d.cells.len()) {
            self.hover = None;
        }
        self.push_card(cx);
    }

    fn em(&self, text: &str) -> f32 {
        self.ems
            .get(text)
            .copied()
            .unwrap_or_else(|| sl::estimate_em(text))
    }

    /// Ask the probe where the strip goes. The cells' width depends on the strip height, which
    /// the probe tells: a second query corrects the first guess when they disagree.
    fn query(&self) -> Option<TaskbarStrip> {
        if self.d.hidden || self.d.cells.is_empty() {
            return None;
        }
        let side = self.d.side?;
        let probe = taskbar_probe::probe()?;
        let ems: Vec<f32> = self.d.cells.iter().map(|c| self.em(&c.figure)).collect();
        let (size, outside) = (self.d.size, self.d.outside);
        let want =
            |dpi: f32, h: f32| (sl::content_width(&ems, h, size, outside) * dpi).ceil() as i32;
        let dpi0 = self
            .place
            .map(|p| p.dpi)
            .or(self.d.monitor.map(|m| m.2))
            .unwrap_or(1.0);
        let h0 = self
            .place
            .map_or(DEFAULT_HEIGHT, |p| p.h as f32 / p.dpi.max(0.1));
        let w0 = want(dpi0, h0);
        let first = probe(TaskbarQuery { side, width: w0 })?;
        let w1 = want(first.dpi, first.h as f32 / first.dpi.max(0.1));
        if w1 != w0 && w1 > 0 {
            probe(TaskbarQuery { side, width: w1 })
        } else {
            Some(first)
        }
    }

    /// Timer and snapshot step: placement, visibility, z-order, the card's close grace.
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.dirty {
            self.derive(cx);
        }
        let now = self.now_ms();
        let next = self.query();
        if next != self.place {
            self.place = next;
            cx.notify();
        }
        match self.place {
            Some(p) => {
                let r = DeviceRect {
                    x: p.x,
                    y: p.y,
                    w: p.w,
                    h: p.h,
                };
                if self.applied != Some(r) {
                    self.pending.place = Some(r);
                    self.applied = Some(r);
                    // The card's anchor moved with the strip.
                    self.push_card(cx);
                }
                if self.applied_visible != Some(true) {
                    self.pending.show = Some(true);
                    self.applied_visible = Some(true);
                }
                if let Some((_, work, _)) = self.d.monitor {
                    publish_screen(NotchScreen {
                        pill: r,
                        edge: if p.bottom { Edge::Bottom } else { Edge::Top },
                        density: self.d.size * p.dpi,
                        work,
                        visible: true,
                    });
                }
            }
            None => {
                if self.applied_visible != Some(false) {
                    self.pending.show = Some(false);
                    self.applied_visible = Some(false);
                    // Reset cards fall back to the work-area edge on the taskbar's side.
                    if let Some((bounds, work, scale)) = self.d.monitor {
                        let r = self.applied.unwrap_or(DeviceRect {
                            x: work.x + work.w / 2,
                            y: work.bottom(),
                            w: 0,
                            h: 0,
                        });
                        let top = r.y < bounds.y + bounds.h / 2;
                        publish_screen(NotchScreen {
                            pill: r,
                            edge: if top { Edge::Top } else { Edge::Bottom },
                            density: self.d.size * scale,
                            work,
                            visible: false,
                        });
                    }
                }
                if self.hover.take().is_some() {
                    self.push_card(cx);
                }
            }
        }
        self.check_card(now, cx);
        self.flush(cx);
    }

    /// Apply queued window operations, then keep the strip directly above the taskbar.
    fn flush(&mut self, cx: &mut Context<Self>) {
        let Some(hwnd) = self.hwnd else { return };
        let ops = std::mem::take(&mut self.pending);
        let above = self.applied_visible == Some(true);
        if ops.is_empty() && !above {
            return;
        }
        cx.spawn(async move |_, _| {
            ops.apply(hwnd);
            if above {
                keep_above_taskbar(hwnd);
            }
        })
        .detach();
    }

    fn schedule_wake(&mut self, at: f64, cx: &mut Context<Self>) {
        let now = self.now_ms();
        if let Some((t, _)) = &self.wake {
            if *t <= at && *t > now {
                return;
            }
        }
        let delay = (at - now).max(1.0).ceil() as u64;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay))
                .await;
            let _ = this.update(cx, |v, cx| {
                v.wake = None;
                cx.notify();
            });
        });
        self.wake = Some((at, task));
    }

    // -----------------------------------------------------------------------------------------
    // Pointer
    // -----------------------------------------------------------------------------------------

    fn on_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let i = sl::slot_at(&self.slots, f32::from(e.position.x));
        self.left_at = None;
        if i.is_some() && i != self.hover {
            self.hover = i;
            self.push_card(cx);
        }
    }

    fn on_down(&mut self, e: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
        self.down = sl::slot_at(&self.slots, f32::from(e.position.x));
    }

    fn on_up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.down.take() else { return };
        if sl::slot_at(&self.slots, f32::from(e.position.x)) != Some(i) {
            return;
        }
        let Some(c) = self.d.cells.get(i) else { return };
        let id = c.account_id.clone();
        self.send(Command::Refresh(Some(id.clone())), cx);
        let now = self.now_ms();
        self.presses.retain(|(p, _)| *p != id);
        self.presses.push((id, now));
        cx.notify();
    }

    fn on_right(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.hover = None;
        self.push_card(cx);
        self.send(Command::OpenSettings, cx);
    }

    pub(crate) fn sign_in(&mut self, id: String, cx: &mut Context<Self>) {
        if self.signing_in.insert(id.clone()) {
            self.send(Command::SignIn(id), cx);
            self.dirty = true;
            self.derive(cx);
            cx.notify();
        }
    }

    // -----------------------------------------------------------------------------------------
    // Hover card
    // -----------------------------------------------------------------------------------------

    /// Close the card once the pointer has been off both the strip and the card for the grace.
    fn check_card(&mut self, now: f64, cx: &mut Context<Self>) {
        if self.hover.is_none() {
            self.left_at = None;
            return;
        }
        let Some((x, y)) = win32::cursor_pos() else {
            return;
        };
        let mut hot: Vec<DeviceRect> = self.applied.into_iter().collect();
        if let Some((_, card)) = &self.card {
            hot.extend(card.read(cx).hot_screen());
        }
        if hot.iter().any(|r| r.contains(x, y)) {
            self.left_at = None;
            return;
        }
        let since = *self.left_at.get_or_insert(now);
        if now - since >= CARD_GRACE_MS {
            self.hover = None;
            self.left_at = None;
            self.push_card(cx);
        }
    }

    /// What the card window should show now (`None` = hidden).
    fn card_state(&self) -> Option<CardState> {
        let i = self.hover?;
        let p = self.place?;
        let slot = self.slots.get(i)?;
        let (bounds, _, _) = self.d.monitor?;
        let strip = DeviceRect {
            x: p.x,
            y: p.y,
            w: p.w,
            h: p.h,
        };
        let anchor = p.x + (slot.cx * p.dpi).round() as i32;
        let frame = sl::card_frame(strip, bounds, anchor, p.bottom, p.dpi, self.d.size)?;
        Some(CardState {
            model: self.d.cards.get(i)?.clone(),
            glyph: self
                .d
                .glyphs
                .get(i)
                .cloned()
                .unwrap_or(Glyph::Letters("â€¢")),
            theme: self.d.card_theme,
            settings: self.d.settings.clone(),
            s: self.d.size,
            dpi: p.dpi,
            frame,
        })
    }

    fn push_card(&mut self, cx: &mut Context<Self>) {
        let state = self.card_state();
        if let Some((_, card)) = &self.card {
            card.update(cx, |c, cx| c.set(state, cx));
            return;
        }
        if state.is_none() || self.card_opening {
            return;
        }
        // First card: open its window outside this update.
        self.card_opening = true;
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |v, cx| v.open_card(cx));
        })
        .detach();
    }

    fn open_card(&mut self, cx: &mut Context<Self>) {
        self.card_opening = false;
        if self.card.is_some() {
            return;
        }
        let Some(state) = self.card_state() else {
            return;
        };
        let f = state.frame.window;
        let bounds = Bounds {
            origin: point(px(f.x as f32 / state.dpi), px(f.y as f32 / state.dpi)),
            size: size(px(state.frame.w), px(state.frame.h)),
        };
        let opts = WindowOptions {
            show: false,
            ..kwin::overlay_options(bounds)
        };
        let strip = cx.entity().downgrade();
        let mut entity = None;
        let opened = cx.open_window(opts, |_, cx| {
            let e = cx.new(|_| CardView::new(strip));
            entity = Some(e.clone());
            e
        });
        match (opened, entity) {
            (Ok(handle), Some(e)) => {
                e.update(cx, |c, cx| c.set(Some(state), cx));
                self.card = Some((handle, e));
            }
            (Err(err), _) => log::warn!("taskbar strip: cannot open the card window: {err}"),
            _ => {}
        }
    }

    fn close_card(&mut self, cx: &mut Context<Self>) {
        if let Some((handle, _)) = self.card.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    // -----------------------------------------------------------------------------------------
    // Render
    // -----------------------------------------------------------------------------------------

    /// Figure widths at a 1 px font from the text system (cached per figure).
    fn measure_figures(&mut self, window: &Window) {
        for c in &self.d.cells {
            if self.ems.contains_key(&c.figure) || c.figure.is_empty() {
                continue;
            }
            let run = TextRun {
                len: c.figure.len(),
                font: figure_font(),
                color: self.d.theme.ink,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(
                SharedString::from(c.figure.clone()),
                px(100.0),
                &[run],
                None,
            );
            let em = f32::from(line.width) / 100.0;
            if em.is_finite() && em > 0.0 {
                self.ems.insert(c.figure.clone(), em);
            }
        }
        if self.ems.len() > 512 {
            self.ems.clear();
        }
    }
}

impl Render for StripView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.now_ms();
        if self.hwnd.is_none() {
            self.hwnd = kwin::hwnd(window);
            self.pending.styles = true;
        }
        if self.dirty {
            self.derive(cx);
        }
        let before: Vec<f32> = self.d.cells.iter().map(|c| self.em(&c.figure)).collect();
        self.measure_figures(window);
        let ems: Vec<f32> = self.d.cells.iter().map(|c| self.em(&c.figure)).collect();
        if ems != before {
            // Exact widths differ from the estimate: re-query the strip size.
            self.poll(cx);
        }
        let vp = window.viewport_size();
        let (w, h) = (f32::from(vp.width), f32::from(vp.height));
        self.slots = sl::layout(&ems, w, h, self.d.size, self.d.outside);

        // presses: held while the click lasts and while that account refreshes
        self.presses.retain(|(id, start)| {
            let refreshing = self
                .d
                .cells
                .iter()
                .any(|c| &c.account_id == id && c.refreshing);
            let age = now - start;
            age < PRESS_MS || (refreshing && age < PRESS_MAX_MS)
        });

        let visible = self.place.is_some();
        let mut root = div()
            .id("strip")
            .relative()
            .size_full()
            .font_family("Segoe UI")
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_up))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right));
        if visible {
            let (track, hole) = strip_tones(&self.d.theme);
            for (i, (c, slot)) in self.d.cells.iter().zip(self.slots.iter()).enumerate() {
                let pressed = self.presses.iter().any(|(id, _)| *id == c.account_id);
                let glyph = self
                    .d
                    .glyphs
                    .get(i)
                    .cloned()
                    .unwrap_or(Glyph::Letters("â€¢"));
                let look = Look {
                    theme: &self.d.theme,
                    track,
                    hole,
                    settings: &self.d.settings,
                };
                root = root.child(strip_cell(
                    c,
                    &glyph,
                    *slot,
                    pressed,
                    now,
                    self.reduced,
                    &look,
                ));
            }
        }

        // wake-ups: activity steps and press ends
        let mut next: Option<f64> = None;
        let mut at = |t: f64| next = Some(next.map_or(t, |n: f64| n.min(t)));
        if visible && !self.reduced {
            if self
                .d
                .cells
                .iter()
                .any(|c| c.activity == ActivityState::Working)
            {
                at(now
                    + easing::ms_to_next_step(
                        now,
                        codenotch_ui_kit::metrics::WORKING_SPIN_MS as f64,
                        12,
                    ));
            }
            if self
                .d
                .cells
                .iter()
                .any(|c| c.activity == ActivityState::Waiting)
            {
                at(now
                    + easing::ms_to_next_step(
                        now,
                        codenotch_ui_kit::metrics::WAITING_PULSE_MS as f64,
                        12,
                    ));
            }
        }
        for (_, start) in &self.presses {
            at(start + PRESS_MS);
            at(start + PRESS_MAX_MS);
        }
        if let Some(t) = next {
            self.schedule_wake(t, cx);
        }
        self.flush(cx);
        root
    }
}

struct Look<'a> {
    theme: &'a Theme,
    track: Hsla,
    hole: Hsla,
    settings: &'a Settings,
}

/// Track and hole over the taskbar: the notch's greys sit on its black pill, here they become
/// white (dark) or black (light) at the same strength over whatever the taskbar shows.
/// The strip sits on the taskbar, so its colours follow the taskbar's light/dark mode when known
/// (the app theme may be forced the other way); `card` (the app theme) is the fallback.
fn strip_theme(light_taskbar: Option<bool>, card: Theme) -> Theme {
    match light_taskbar {
        Some(true) => Theme::light(),
        Some(false) => Theme::dark(),
        None => card,
    }
}

fn strip_tones(theme: &Theme) -> (Hsla, Hsla) {
    if theme.dark {
        (
            hsla(0.0, 0.0, 1.0, 48.0 / 255.0),
            hsla(0.0, 0.0, 1.0, 42.0 / 255.0),
        )
    } else {
        (hsla(0.0, 0.0, 0.0, 0.16), hsla(0.0, 0.0, 1.0, 1.0))
    }
}

/// One cell: the notch's ring (track, reading, weekly ring, activity arc), glyph and figure,
/// with the figure beside the ring.
fn strip_cell(
    c: &CellModel,
    glyph: &Glyph,
    slot: Slot,
    pressed: bool,
    now: f64,
    reduced: bool,
    look: &Look,
) -> AnyElement {
    let theme = look.theme;
    let settings = look.settings;
    let fade = |col: Hsla| {
        if c.stale {
            col.opacity(STALE_OPACITY)
        } else {
            col
        }
    };
    let press = if pressed { PRESS_SCALE } else { 1.0 };
    let unit = slot.unit * press;
    // RingSpec::notch(k) draws the 56-unit box at RING_BOX Ã— k px.
    let k = sl::UNITS * unit / RING_BOX;
    let track = fade(look.track);
    let main_c = fade(theme.usage_color(c.main.map(|a| a.used), settings));
    let mut spec = RingSpec::notch(k).hole(fade(look.hole)).main(
        c.main.map_or(0.0, |a| a.fraction),
        main_c,
        track,
    );
    if let Some(wk) = c.weekly {
        let wc = fade(
            theme
                .usage_color(Some(wk.used), settings)
                .opacity(WEEKLY_ARC_OPACITY),
        );
        let wt = fade(look.track.opacity(WEEKLY_TRACK_OPACITY));
        spec = spec.weekly(
            settings.weekly_ring,
            wk.fraction,
            wc,
            wt,
            settings.weekly_ring_dashed,
        );
    }
    spec = match c.activity {
        ActivityState::Working => spec.working(
            if reduced {
                0.0
            } else {
                easing::working_rotation(now)
            },
            theme.ink,
        ),
        ActivityState::Waiting => spec.waiting(theme.watch.opacity(if reduced {
            1.0
        } else {
            easing::waiting_opacity(now)
        })),
        _ => spec,
    };
    let ring_box = sl::UNITS * unit;
    let glyph_px = ring_units::GLYPH * unit;
    let glyph_c = if c.dim_glyph {
        theme.ink2.opacity(STALE_OPACITY)
    } else {
        theme.ink2
    };
    let letters = 20.0 * unit;
    let (left, top) = (slot.cx - ring_box / 2.0, slot.cy - ring_box / 2.0);
    let ring = canvas(
        |_, _, _| {},
        move |b, _, window, _| paint_ring(window, b.center(), &spec),
    )
    .absolute()
    .left(px(left))
    .top(px(top))
    .w(px(ring_box))
    .h(px(ring_box));
    let mark = div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(ring_box))
        .h(px(ring_box))
        .flex()
        .items_center()
        .justify_center()
        .child(glyph_element(glyph, glyph_px, glyph_c, letters));
    let figure = div()
        .absolute()
        .left(px(slot.text_x))
        .top_0()
        .h_full()
        .flex()
        .items_center()
        .whitespace_nowrap()
        .font(figure_font())
        .text_size(px(slot.font))
        .line_height(px(slot.font * 1.3))
        .text_color(theme.ink)
        .child(c.figure.clone());
    div()
        .absolute()
        .left_0()
        .top_0()
        .size_full()
        .child(ring)
        .child(mark)
        .child(figure)
        .into_any_element()
}

// ---------------------------------------------------------------------------------------------
// Card window
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
struct CardState {
    model: CardModel,
    glyph: Glyph,
    theme: Theme,
    settings: Settings,
    s: f32,
    dpi: f32,
    frame: CardFrame,
}

pub(crate) struct CardView {
    strip: WeakEntity<StripView>,
    state: Option<CardState>,
    hwnd: Option<isize>,
    heights: HashMap<String, f32>,
    applied_rect: Option<DeviceRect>,
    applied_region: Option<Vec<DeviceRect>>,
    applied_visible: Option<bool>,
    pending: WinOps,
    /// Card and tail on screen (device px), for the strip's close grace.
    hot: Vec<DeviceRect>,
}

impl CardView {
    fn new(strip: WeakEntity<StripView>) -> Self {
        Self {
            strip,
            state: None,
            hwnd: None,
            heights: HashMap::new(),
            applied_rect: None,
            applied_region: None,
            applied_visible: None,
            pending: WinOps::default(),
            hot: Vec::new(),
        }
    }

    fn set(&mut self, state: Option<CardState>, cx: &mut Context<Self>) {
        self.state = state;
        cx.notify();
    }

    fn hot_screen(&self) -> Vec<DeviceRect> {
        if self.applied_visible == Some(true) {
            self.hot.clone()
        } else {
            Vec::new()
        }
    }

    fn sign_in(&mut self, id: String, cx: &mut Context<Self>) {
        let _ = self.strip.update(cx, |s, cx| s.sign_in(id, cx));
    }
}

impl Render for CardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.hwnd.is_none() {
            self.hwnd = kwin::hwnd(window);
            self.pending.styles = true;
        }
        let mut root = div().relative().size_full().font_family("Segoe UI");
        let state = self.state.clone();
        let visible = state.is_some();
        if let Some(st) = state {
            let s = st.s;
            let id = st.model.account_id.clone();
            let measured = self.heights.get(&id).copied();
            let card = sl::card_rect(&st.frame, measured.unwrap_or(200.0 * s), s);
            let tail = sl::tail_polygon(&st.frame, card, s, 12);
            let tail_box = sl::tail_rect(&st.frame, card, s);
            if self.applied_rect != Some(st.frame.window) {
                self.applied_rect = Some(st.frame.window);
                self.pending.place = Some(st.frame.window);
            }
            let region: Vec<DeviceRect> = if measured.is_some() {
                vec![sl::to_device(card, st.dpi), sl::to_device(tail_box, st.dpi)]
            } else {
                Vec::new()
            };
            let origin = st.frame.window;
            self.hot = region
                .iter()
                .map(|r| DeviceRect {
                    x: origin.x + r.x,
                    y: origin.y + r.y,
                    w: r.w,
                    h: r.h,
                })
                .collect();
            if self.applied_region.as_ref() != Some(&region) {
                self.applied_region = Some(region.clone());
                self.pending.region = Some(region);
            }

            // measuring: the content's natural height, read back next frame
            let known = measured;
            let weak = cx.entity().downgrade();
            let pad = CARD_PAD * s;
            let max_h = (st.frame.h - crate::dims::TAIL_LEN * s).max(40.0 * s);
            let mid = id.clone();
            let probe = canvas(
                move |b, window, _| {
                    let h = (f32::from(b.size.height) + 2.0 * pad).min(max_h);
                    if known.is_none_or(|k| (k - h).abs() > 0.5) {
                        let weak = weak.clone();
                        let id = mid.clone();
                        window.on_next_frame(move |_, cx| {
                            let _ = weak.update(cx, |v, cx| {
                                v.heights.insert(id, h);
                                cx.notify();
                            });
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .left_0()
            .top_0()
            .size_full();
            let content = card_content(
                &st.model,
                &st.glyph,
                s,
                &st.theme,
                &st.settings,
                cx,
                CardView::sign_in,
            )
            .relative()
            .child(probe);
            // The notch card's material.
            let card_c = st.theme.pill;
            root = root.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .size_full()
                    .when(measured.is_none(), |d| d.opacity(0.0))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |b, _, window, _| {
                                let mut pb = PathBuilder::fill();
                                let pts: Vec<_> = tail
                                    .iter()
                                    .map(|p| point(b.origin.x + px(p.0), b.origin.y + px(p.1)))
                                    .collect();
                                pb.add_polygon(&pts, true);
                                if let Ok(path) = pb.build() {
                                    window.paint_path(path, card_c);
                                }
                            },
                        )
                        .absolute()
                        .left_0()
                        .top_0()
                        .size_full(),
                    )
                    .child(card_box(card, card_c, s, content)),
            );
        } else {
            self.hot.clear();
        }
        if self.applied_visible != Some(visible) {
            self.applied_visible = Some(visible);
            self.pending.show = Some(visible);
        }
        if visible {
            // Above everything, the taskbar included.
            self.pending.topmost = true;
        }
        if let Some(hwnd) = self.hwnd {
            if !self.pending.is_empty() {
                let ops = std::mem::take(&mut self.pending);
                cx.spawn(async move |_, _| ops.apply(hwnd)).detach();
            }
        }
        root
    }
}

fn card_box(card: Rect, color: Hsla, s: f32, content: gpui::Div) -> impl IntoElement {
    div()
        .id("strip-card")
        .absolute()
        .left(px(card.x))
        .top(px(card.y))
        .w(px(card.w))
        .h(px(card.h))
        .bg(color)
        .rounded(px(CARD_RADIUS * s))
        .p(px(CARD_PAD * s))
        .overflow_y_scroll()
        .child(content)
}

// ---------------------------------------------------------------------------------------------
// Win32 bits (UI thread, outside gpui's borrow)
// ---------------------------------------------------------------------------------------------

/// When the taskbar is above `hwnd` in the z-order, re-insert `hwnd` directly above it. Never
/// touches the taskbar itself.
#[cfg(windows)]
fn keep_above_taskbar(hwnd: isize) {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetWindow, SetWindowPos, GW_HWNDPREV, HWND_TOPMOST, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
    };
    let me = hwnd as HWND;
    let class: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
    // SAFETY: read-only window lookups; SetWindowPos only on our own window.
    unsafe {
        let bar = FindWindowW(class.as_ptr(), std::ptr::null());
        if bar.is_null() {
            return;
        }
        let mut w = GetWindow(me, GW_HWNDPREV);
        let mut bar_above = false;
        for _ in 0..512 {
            if w.is_null() {
                break;
            }
            if w == bar {
                bar_above = true;
                break;
            }
            w = GetWindow(w, GW_HWNDPREV);
        }
        if !bar_above {
            return;
        }
        let prev = GetWindow(bar, GW_HWNDPREV);
        let after = if prev.is_null() || prev == me {
            HWND_TOPMOST
        } else {
            prev
        };
        SetWindowPos(
            me,
            after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
    }
}

#[cfg(not(windows))]
fn keep_above_taskbar(_: isize) {}

/// The taskbar's own colour mode (`SystemUsesLightTheme`), which can differ from the apps'.
#[cfg(windows)]
fn taskbar_light() -> Option<bool> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let val: Vec<u16> = "SystemUsesLightTheme\0".encode_utf16().collect();
    let mut data: u32 = 0;
    let mut len = std::mem::size_of::<u32>() as u32;
    // SAFETY: read-only registry query into a u32 of `len` bytes.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut data as *mut u32 as *mut core::ffi::c_void,
            &mut len,
        )
    };
    (rc == 0).then_some(data != 0)
}

#[cfg(not(windows))]
fn taskbar_light() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_follow_the_theme() {
        let (track, hole) = strip_tones(&Theme::dark());
        assert!(track.l > 0.9 && track.a < 0.25 && hole.a < 0.25);
        let (track, hole) = strip_tones(&Theme::light());
        assert!(track.l < 0.1 && (hole.a - 1.0).abs() < 1e-6);
    }

    #[test]
    fn strip_follows_the_taskbar_not_the_app_theme() {
        assert_eq!(strip_theme(Some(true), Theme::dark()), Theme::light());
        assert_eq!(strip_theme(Some(false), Theme::light()), Theme::dark());
        assert_eq!(strip_theme(None, Theme::dark()), Theme::dark());
        assert_eq!(strip_theme(None, Theme::light()), Theme::light());
    }

    #[test]
    fn live_reads_do_not_panic() {
        // Read-only: registry value and z-order lookups with an invalid handle.
        let _ = taskbar_light();
        keep_above_taskbar(0);
    }
}
