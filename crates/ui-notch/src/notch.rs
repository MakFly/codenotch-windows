//! The notch window: pill, cells, fillets, orb and grip, hover card, fold, drag, menu.
//!
//! Rendering is a thin layer over the pure modules (`geom`, `placement`, `fold`, `cell`, `card`).
//!
//! **Window model.** One transparent, borderless, non-activating, topmost window per notch,
//! sized like the reference (360 × ≥650 upright, ≥650 × 650 flat, × notch size). Empty parts
//! must not swallow clicks, so the window gets a **window region** (`SetWindowRgn`) equal to the
//! spec's hot area: every hot rect (pill + fillets, orb, grip when out, card + tail, notice)
//! padded by 10 px, and their bounding box; just the wake zone while folded. Windows hit-tests
//! regions itself, so clicks outside go to whatever is behind, and leaving the region produces a
//! normal `WM_MOUSELEAVE` (gpui's hover flag) that drives the close/fold grace timers. The region
//! also clips drawing, which is harmless because it always covers what is drawn. Set
//! `CODENOTCH_NOTCH_NO_REGION=1` to disable it (debugging).
//!
//! **Threading.** Window moves/regions/show are queued in [`WinOps`] and applied from a spawned
//! task (outside gpui's `App` borrow) — see `win32`.
//!
//! **Idle cost.** Frames are requested only while something animates (fold, glides, press,
//! drag, orb/grip, adaptive fade); stepped activity animations wake on their step boundaries
//! (10 fps working spin, ~11 fps waiting pulse); otherwise only a 1 s work-area check runs.

use crate::adaptive;
use crate::card::{card_model, Bar, Block, CardCtx, CardModel, Row};
use crate::cell::{cell_model, CellModel};
use crate::dims::*;
use crate::easing::{self, progress, Bezier};
use crate::fold::{self, FoldAnim, FoldInputs};
use crate::geom::{self, hot_region, NotchLayout, Pt, Rect};
use crate::glyphs::{self, Glyph};
use crate::menu;
use crate::placement;
use crate::reset::{publish_screen, NotchScreen};
use crate::win32::{self, WinOps};
use codenotch_core::config::{Edge, Settings, ShowMode};
use codenotch_core::format::ForecastTone;
use codenotch_core::i18n::Lang;
use codenotch_core::model::{ActivityState, Command, Event, Snapshot};
use codenotch_ui_kit::metrics::{STALE_OPACITY, WEEKLY_ARC_OPACITY, WEEKLY_TRACK_OPACITY};
use codenotch_ui_kit::ring::{paint_arc, paint_disc, paint_ring, Cap};
use codenotch_ui_kit::window::{self as kwin, DeviceRect, MonitorInfo};
use codenotch_ui_kit::{RingSpec, Theme, UiModel};
use gpui::prelude::FluentBuilder;
use gpui::{
    canvas, div, img, point, px, svg, AnyElement, App, Context, CursorStyle, Entity, FillOptions,
    Font, FontFeatures, FontStyle, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, PathBuilder, PathStyle, Pixels,
    Point, Render, StatefulInteractiveElement, Styled, Task, Window,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// `--preview-card` (demo): behave as if hovered and pin the first cell's card.
pub(crate) static PREVIEW_CARD: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn preview_card() -> bool {
    PREVIEW_CARD.load(std::sync::atomic::Ordering::Relaxed)
}

/// The notch is hidden for this session (global hotkey). One place to read it.
pub(crate) fn notch_hidden(snap: &Snapshot) -> bool {
    snap.notch_hidden
}

/// The notch has something to show: not hidden (state or settings) and at least one cell.
fn wants_visible(hidden: bool, cells: usize) -> bool {
    !hidden && cells > 0
}

/// gpui renders a window only on `WM_PAINT`, which a hidden window never gets: once the notch
/// hid itself (e.g. no accounts yet), `render` cannot show it again. A snapshot that makes it
/// showable must then show the window from outside `render` (see `NotchView::unhide`).
fn must_unhide(applied_visible: Option<bool>, hidden: bool, cells: usize) -> bool {
    applied_visible == Some(false) && wants_visible(hidden, cells)
}

const PRESS_MS: f32 = 300.0;
const PRESS_SCALE: f32 = 0.93;
const PRESS_MIN_MS: f64 = 380.0;
const PRESS_MAX_MS: f64 = 6000.0;
const SPIN_MS: f32 = 950.0;
const CARD_GRACE_MS: f64 = codenotch_ui_kit::metrics::CARD_CLOSE_GRACE_MS as f64;
const CARD_GLIDE_MS: f32 = 300.0;
const WINDOW_GLIDE_MS: f32 = 360.0;
const NOTICE_MS: f64 = 6000.0;

/// Snapshot-derived data, recomputed only when the snapshot (or the clock minute) changes.
struct Derived {
    settings: Settings,
    theme: Theme,
    lang: Lang,
    scale: f32,
    cells: Vec<CellModel>,
    cards: Vec<CardModel>,
    glyphs: Vec<Glyph>,
    urls: Vec<Option<&'static str>>,
    fullscreen: bool,
    hidden: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Toggle {
    on: bool,
    since: f64,
}

impl Toggle {
    fn set(&mut self, on: bool, now: f64) -> bool {
        if on == self.on {
            return false;
        }
        self.on = on;
        self.since = now;
        true
    }
    fn t(&self, now: f64, delay: f32, dur: f32) -> f32 {
        progress((now - self.since) as f32, delay, dur)
    }
}

struct Press {
    id: String,
    start: f64,
    baseline: Option<i64>,
    saw_refreshing: bool,
    released: Option<f64>,
    scale_at_release: f32,
}

impl Press {
    fn scale(&self, now: f64) -> f32 {
        match self.released {
            None => {
                1.0 - (1.0 - PRESS_SCALE)
                    * Bezier::PRESS.at(progress((now - self.start) as f32, 0.0, PRESS_MS))
            }
            Some(r) => {
                let from = self.scale_at_release;
                from + (1.0 - from) * Bezier::PRESS.at(progress((now - r) as f32, 0.0, PRESS_MS))
            }
        }
    }
    fn spin(&self, now: f64) -> f32 {
        360.0 * Bezier::SPIN.at(progress((now - self.start) as f32, 0.0, SPIN_MS))
    }
    fn done(&self, now: f64) -> bool {
        self.released.is_some_and(|r| now - r >= PRESS_MS as f64)
            && now - self.start >= SPIN_MS as f64
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Cell(usize),
    Orb,
    Grip,
    Card,
    Pill,
    Other,
}

struct Down {
    pos: Pt,
    target: Target,
    drag_candidate: bool,
}

struct Drag {
    /// Device px between the pointer and the notch centre along the edge.
    offset: f32,
    edge: Edge,
    along: f64,
    monitors: Vec<MonitorInfo>,
    idx: usize,
}

/// Placement saved after a drag, kept until the engine republishes the config.
struct PlaceOverride {
    edge: Edge,
    along: f64,
    monitor: String,
    before: (Edge, f64, String),
}

pub(crate) struct NotchView {
    model: Entity<UiModel>,
    epoch: Instant,
    hwnd: Option<isize>,
    d: Derived,
    dirty: bool,
    reduced: bool,
    no_region: bool,
    glyph_cache: HashMap<String, Glyph>,

    monitor: Option<MonitorInfo>,
    layout: NotchLayout,
    target: Option<DeviceRect>,
    glide: Option<(DeviceRect, DeviceRect, f64)>,
    applied_rect: Option<DeviceRect>,
    applied_region: Option<Option<DeviceRect>>,
    applied_visible: Option<bool>,
    pending: WinOps,

    hovered: bool,
    left_at: Option<f64>,
    orb_hover: Toggle,
    grip_out: Toggle,
    grip_swell: Toggle,
    grip_was_out: bool,
    orb_click: Option<f64>,
    orb_turns: u32,

    fold: FoldAnim,
    started: bool,

    card_cell: Option<usize>,
    /// (cell, rect, tail_u) last drawn, and the glide start (from rect, from tail, time).
    card_drawn: Option<(usize, Rect, f32)>,
    card_glide: Option<(Rect, f32, f64)>,
    heights: HashMap<String, f32>,
    /// Card cell and when it was first asked for (measurement time-out).
    card_since: Option<(usize, f64)>,

    presses: Vec<Press>,
    down: Option<Down>,
    drag: Option<Drag>,
    over: Option<PlaceOverride>,
    menu_open: bool,

    notice: Option<(String, f64)>,
    signing_in: HashSet<String>,

    rest_light: Option<bool>,
    rest_fade: Option<(bool, f64)>,
    sampling: Option<(DeviceRect, Task<()>)>,

    wake: Option<(f64, Task<()>)>,
    /// Polls the pointer while hovered, so a lost `WM_MOUSELEAVE` still folds (see
    /// `fold::effective_hover`).
    hover_watch: Option<Task<()>>,
    /// Last fold wish logged (debug).
    logged_want: Option<(fold::Want, bool)>,
    _ticker: Task<()>,
}

impl NotchView {
    pub(crate) fn new(model: Entity<UiModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |v, _, cx| {
            v.dirty = true;
            v.unhide(cx);
            cx.notify();
        })
        .detach();
        cx.subscribe(&model, |v, _, ev: &Event, cx| {
            if let Event::Notice(text) = ev {
                let now = v.now_ms();
                v.notice = Some((text.clone(), now + NOTICE_MS));
                cx.notify();
            }
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
                cx.background_executor().timer(Duration::from_secs(1)).await;
                n = n.wrapping_add(1);
                if this
                    .update(cx, |v, cx| v.tick(n.is_multiple_of(30), cx))
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
            d: Derived {
                settings: Settings::default(),
                theme: Theme::default(),
                lang: Lang::En,
                scale: 1.0,
                cells: Vec::new(),
                cards: Vec::new(),
                glyphs: Vec::new(),
                urls: Vec::new(),
                fullscreen: false,
                hidden: true,
            },
            dirty: true,
            reduced: win32::reduced_motion(),
            no_region: std::env::var_os("CODENOTCH_NOTCH_NO_REGION").is_some_and(|v| v != "0"),
            glyph_cache: HashMap::new(),
            monitor: None,
            layout: NotchLayout::new(Edge::Right, 1, 1.0),
            target: None,
            glide: None,
            applied_rect: None,
            applied_region: None,
            applied_visible: None,
            pending: WinOps::default(),
            hovered: false,
            left_at: None,
            orb_hover: Toggle::default(),
            grip_out: Toggle::default(),
            grip_swell: Toggle::default(),
            grip_was_out: false,
            orb_click: None,
            orb_turns: 0,
            fold: FoldAnim::new(false, 1),
            started: false,
            card_cell: None,
            card_drawn: None,
            card_glide: None,
            heights: HashMap::new(),
            card_since: None,
            presses: Vec::new(),
            down: None,
            drag: None,
            over: None,
            menu_open: false,
            notice: None,
            signing_in: HashSet::new(),
            rest_light: None,
            rest_fade: None,
            sampling: None,
            wake: None,
            hover_watch: None,
            logged_want: None,
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

    // ------------------------------------------------------------------------------------
    // Snapshot → derived data
    // ------------------------------------------------------------------------------------

    fn derive(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        let model = self.model.clone();
        let app: &App = cx;
        let snap = &model.read(app).snapshot;
        let settings = snap.settings().clone();
        let lang = snap.lang();
        let now = codenotch_core::now_secs();
        let theme = Theme::for_settings(&settings, app);
        let views = snap.notch_accounts();
        let clock24 = win32::clock24(lang);
        let tz = chrono::Local;
        let user_dir = glyphs::user_dir();

        let mut cells = Vec::with_capacity(views.len());
        let mut cards = Vec::with_capacity(views.len());
        let mut glyph_list = Vec::with_capacity(views.len());
        let mut urls = Vec::with_capacity(views.len());
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
            urls.push(v.account.provider.usage_url());
        }
        // A finished sign-in clears the "Signing in..." state.
        self.signing_in.retain(|id| {
            snap.account(id)
                .and_then(|v| v.reading.as_ref())
                .is_some_and(|r| r.status == codenotch_core::model::ReadingStatus::SignedOut)
        });
        // Drop the drag override once the engine published it (or someone else moved the notch).
        if let Some(o) = &self.over {
            let p = &settings.widget_placement;
            let cur = (settings.widget_position, p.along(), p.monitor.clone());
            let landed = cur.0 == o.edge && (cur.1 - o.along).abs() < 1e-6 && cur.2 == o.monitor;
            if landed || cur != o.before {
                self.over = None;
            }
        }
        let monitor_name = self.placement().2;
        self.monitor = kwin::monitor_or_primary(&monitor_name);
        self.d = Derived {
            scale: settings.notch_size.scale(),
            // Taskbar mode: the strip replaces the floating notch.
            hidden: notch_hidden(snap)
                || settings.show_mode() == ShowMode::Hidden
                || crate::taskbar_probe::active(&settings),
            fullscreen: snap.fullscreen,
            settings,
            theme,
            lang,
            cells,
            cards,
            glyphs: glyph_list,
            urls,
        };
        self.pending.topmost = true;
    }

    /// Show the window again when a snapshot gives a self-hidden notch something to show: the
    /// resulting `WM_PAINT` runs `render`, which then places, sizes and clips it (the frame
    /// drawn while hidden is empty, so nothing stale flashes).
    fn unhide(&mut self, cx: &mut Context<Self>) {
        if self.applied_visible != Some(false) || self.hwnd.is_none() {
            return;
        }
        self.derive(cx);
        if self.monitor.is_none()
            || !must_unhide(self.applied_visible, self.d.hidden, self.d.cells.len())
        {
            return;
        }
        self.applied_visible = Some(true);
        self.pending.show = Some(true);
        self.flush_ops(cx);
    }

    fn tick(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if self.drag.is_some() {
            return;
        }
        let name = self.placement().2;
        let m = kwin::monitor_or_primary(&name);
        if m != self.monitor {
            self.monitor = m;
            cx.notify();
        }
        if refresh {
            self.reduced = win32::reduced_motion();
            self.dirty = true;
            cx.notify();
        }
    }

    /// (edge, along, monitor name) in force: drag → saved override → config.
    fn placement(&self) -> (Edge, f64, String) {
        if let Some(d) = &self.drag {
            let name = d
                .monitors
                .get(d.idx)
                .map(|m| m.name.clone())
                .unwrap_or_default();
            return (d.edge, d.along, name);
        }
        if let Some(o) = &self.over {
            return (o.edge, o.along, o.monitor.clone());
        }
        let s = &self.d.settings;
        (
            s.widget_position,
            s.widget_placement.along(),
            s.widget_placement.monitor.clone(),
        )
    }

    // ------------------------------------------------------------------------------------
    // Pointer
    // ------------------------------------------------------------------------------------

    fn handles_visible(&self) -> bool {
        !self.fold.folded()
    }

    fn hit(&self, p: Pt) -> Target {
        let l = &self.layout;
        if self.fold.folded() {
            return Target::Other;
        }
        if let Some((_, r, tail_u)) = self.card_drawn {
            if r.contains(p) || l.tail_rect(tail_u).contains(p) {
                return Target::Card;
            }
        }
        if self.grip_out.on && l.over_grip(p) {
            return Target::Grip;
        }
        if self.handles_visible() && l.over_orb(p) {
            return Target::Orb;
        }
        if let Some(i) = l.cell_at(p) {
            return Target::Cell(i);
        }
        if l.shoulders_rect().contains(p) {
            return Target::Pill;
        }
        Target::Other
    }

    /// Update hover-driven state for pointer `p`; true when something visible changed.
    fn update_hover(&mut self, p: Pt, now: f64) -> bool {
        if self.fold.folded() || self.drag.is_some() {
            return false;
        }
        let target = self.hit(p);
        let on_orb = target == Target::Orb;
        let on_grip = target == Target::Grip;
        let mut changed = self.orb_hover.set(on_orb || on_grip, now);
        changed |= self.grip_out.set(on_orb || on_grip, now);
        changed |= self.grip_swell.set(on_grip, now);
        if self.grip_out.on {
            self.grip_was_out = true;
        }
        let before = self.card_cell;
        match target {
            Target::Orb | Target::Grip => self.card_cell = None,
            Target::Cell(i) => self.card_cell = Some(i),
            Target::Card | Target::Other => {}
            Target::Pill => {
                if self.card_cell.is_none() && self.layout.in_pill(p) {
                    self.card_cell = self.layout.nearest_cell(p);
                }
            }
        }
        changed || before != self.card_cell
    }

    fn on_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let p = (f32::from(e.position.x), f32::from(e.position.y));
        let now = self.now_ms();
        let mut changed = false;
        if !self.hovered {
            self.hovered = true;
            self.left_at = None;
            changed = true;
        }
        if let Some(d) = &self.down {
            if d.drag_candidate && geom::dist(p, d.pos) > DRAG_THRESHOLD * self.d.scale {
                self.start_drag();
                cx.notify();
                return;
            }
        }
        changed |= self.update_hover(p, now);
        if changed || self.fold.folded() {
            cx.notify();
        }
    }

    fn on_down(&mut self, e: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_some() || self.fold.folded() {
            return;
        }
        let p = (f32::from(e.position.x), f32::from(e.position.y));
        let target = self.hit(p);
        if matches!(target, Target::Card | Target::Other) {
            return;
        }
        let alt = e.modifiers.alt || win32::alt_down();
        let drag_candidate =
            target == Target::Grip || (alt && matches!(target, Target::Cell(_) | Target::Pill));
        self.down = Some(Down {
            pos: p,
            target,
            drag_candidate,
        });
        cx.notify();
    }

    fn on_up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_some() {
            self.finish_drag(cx);
            cx.notify();
            return;
        }
        let Some(d) = self.down.take() else { return };
        let p = (f32::from(e.position.x), f32::from(e.position.y));
        let now = self.now_ms();
        match d.target {
            Target::Cell(i) if self.layout.cell_at(p) == Some(i) => self.click_cell(i, now, cx),
            Target::Orb if self.layout.over_orb(p) => {
                self.orb_click = Some(now);
                self.orb_turns = self.orb_turns.wrapping_add(1);
                self.send(Command::OpenSettings, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn on_right(&mut self, e: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu_open || self.drag.is_some() || self.fold.folded() {
            return;
        }
        let Some(hwnd) = self.hwnd else { return };
        let p = (f32::from(e.position.x), f32::from(e.position.y));
        let Some(i) = self
            .layout
            .cell_at(p)
            .or(self.card_cell)
            .or_else(|| self.layout.nearest_cell(p))
        else {
            return;
        };
        let url = self.d.urls.get(i).copied().flatten();
        let items = menu::items(self.d.lang, url, self.d.settings.show_mode());
        let pt = win32::cursor_pos().unwrap_or((0, 0));
        let dark = self.d.theme.dark;
        self.menu_open = true;
        self.card_cell = None;
        cx.spawn(async move |this, cx| {
            let choice = win32::popup_menu(hwnd, &items, pt.0, pt.1, dark);
            let _ = this.update(cx, |v, cx| {
                v.menu_open = false;
                if let Some(id) = choice {
                    let command = menu::command(id, url, &v.model.read(cx).snapshot.config);
                    if let Some(c) = command {
                        v.send(c, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn click_cell(&mut self, i: usize, now: f64, cx: &mut Context<Self>) {
        let Some(c) = self.d.cells.get(i) else { return };
        let id = c.account_id.clone();
        self.send(Command::Refresh(Some(id.clone())), cx);
        self.presses.retain(|p| p.id != id);
        self.presses.push(Press {
            id,
            start: now,
            baseline: c.fetched_at,
            saw_refreshing: c.refreshing,
            released: None,
            scale_at_release: 1.0,
        });
    }

    fn sign_in(&mut self, id: String, cx: &mut Context<Self>) {
        if self.signing_in.insert(id.clone()) {
            self.send(Command::SignIn(id), cx);
            self.dirty = true;
            cx.notify();
        }
    }

    // ------------------------------------------------------------------------------------
    // Drag
    // ------------------------------------------------------------------------------------

    fn start_drag(&mut self) {
        self.down = None;
        let (Some(rect), Some(pt)) = (self.applied_rect, win32::cursor_pos()) else {
            return;
        };
        let (edge, along, _) = self.placement();
        let (pos, centre) = if edge.is_upright() {
            (pt.1, rect.y + rect.h / 2)
        } else {
            (pt.0, rect.x + rect.w / 2)
        };
        let monitors = kwin::monitors();
        let current = self
            .monitor
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_default();
        let idx = monitors.iter().position(|m| m.name == current).unwrap_or(0);
        if monitors.is_empty() {
            return;
        }
        self.card_cell = None;
        self.drag = Some(Drag {
            offset: (pos - centre) as f32,
            edge,
            along,
            monitors,
            idx,
        });
    }

    /// Follow the pointer; ends the drag when the button is up.
    fn poll_drag(&mut self, cx: &mut Context<Self>) {
        if !win32::primary_button_down() {
            self.finish_drag(cx);
            return;
        }
        let Some(pt) = win32::cursor_pos() else {
            return;
        };
        let fullscreen = self.d.fullscreen;
        let Some(d) = self.drag.as_mut() else { return };
        let bounds: Vec<DeviceRect> = d.monitors.iter().map(|m| m.bounds).collect();
        let scale = d.monitors.get(d.idx).map_or(1.0, |m| m.scale);
        d.idx = placement::drag_monitor(
            d.idx,
            pt,
            &bounds,
            (DRAG_MONITOR_SLACK * scale).round() as i32,
        );
        let Some(m) = d.monitors.get(d.idx) else {
            return;
        };
        let area = if fullscreen { m.bounds } else { m.work };
        d.edge = placement::drag_edge(d.edge, pt, area, DRAG_EDGE_HYSTERESIS * m.scale);
        d.along = placement::drag_along(d.edge, pt, area, d.offset);
    }

    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        let Some(mon) = d.monitors.get(d.idx).cloned() else {
            return;
        };
        let mut config = self.model.read(cx).snapshot.config.clone();
        let s = &config.settings;
        let before = (
            s.widget_position,
            s.widget_placement.along(),
            s.widget_placement.monitor.clone(),
        );
        // Keep "" (= primary) when the notch stays on the primary monitor.
        let monitor = if before.2.is_empty() && mon.primary {
            String::new()
        } else {
            mon.name.clone()
        };
        config.settings.widget_position = d.edge;
        config.settings.widget_placement.y = d.along;
        config.settings.widget_placement.monitor = monitor.clone();
        self.over = Some(PlaceOverride {
            edge: d.edge,
            along: d.along,
            monitor,
            before,
        });
        self.monitor = Some(mon);
        self.send(Command::SaveConfig(Box::new(config)), cx);
    }

    // ------------------------------------------------------------------------------------
    // Timers and window ops
    // ------------------------------------------------------------------------------------

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

    /// While hovered: every `HOVER_RECHECK_MS`, re-render if the pointer is no longer inside
    /// the window (a `WM_MOUSELEAVE` gpui never got). `render` then sees the notch unhovered,
    /// starts the fold delay and drops this task.
    fn watch_hover(&self, cx: &mut Context<Self>) -> Option<Task<()>> {
        let hwnd = self.hwnd?;
        Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(fold::HOVER_RECHECK_MS))
                .await;
            if win32::cursor_in_window(hwnd) == Some(false)
                && this.update(cx, |_, cx| cx.notify()).is_err()
            {
                break;
            }
        }))
    }

    fn flush_ops(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_empty() {
            return;
        }
        let Some(hwnd) = self.hwnd else { return };
        let ops = std::mem::take(&mut self.pending);
        cx.spawn(async move |_, _| ops.apply(hwnd)).detach();
    }

    fn sync_sampling(&mut self, want: Option<DeviceRect>, cx: &mut Context<Self>) {
        match want {
            None => self.sampling = None,
            Some(probe) => {
                if self.sampling.as_ref().is_some_and(|(p, _)| *p == probe) {
                    return;
                }
                let Some(f) = adaptive::sampler() else {
                    self.sampling = None;
                    return;
                };
                let task = cx.spawn(async move |this, cx| {
                    let mut delay = adaptive::SETTLE_MS;
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(delay))
                            .await;
                        let f2 = f.clone();
                        let l = cx
                            .background_executor()
                            .spawn(async move { f2(probe.x, probe.y, probe.w, probe.h) })
                            .await;
                        delay = if l.is_some() {
                            adaptive::PERIOD_MS
                        } else {
                            adaptive::RETRY_MS
                        };
                        if let Some(l) = l {
                            if this.update(cx, |v, cx| v.on_luminance(l, cx)).is_err() {
                                break;
                            }
                        }
                    }
                });
                self.sampling = Some((probe, task));
            }
        }
    }

    fn on_luminance(&mut self, l: f32, cx: &mut Context<Self>) {
        let current = self.rest_light.unwrap_or(!self.d.theme.dark);
        let next = adaptive::choose_light(l, current);
        if next != current || self.rest_light.is_none() {
            if next != current {
                self.rest_fade = Some((current, self.now_ms()));
            }
            self.rest_light = Some(next);
            cx.notify();
        }
    }

    /// Rest pill (fill, outline) colours, with the adaptive tone when enabled.
    fn rest_colors(&self, now: f64) -> (Hsla, Hsla) {
        let t = &self.d.theme;
        if !self.d.settings.adaptive_pill {
            return (t.pill, t.rest_edge);
        }
        let Some(light) = self.rest_light else {
            return (t.pill, t.rest_edge);
        };
        let tone = |light: bool| {
            let th = if light { Theme::light() } else { Theme::dark() };
            (th.pill, th.rest_edge)
        };
        let to = tone(light);
        match self.rest_fade {
            Some((from_light, at)) if !self.reduced => {
                let k = Bezier::EASE.at(progress((now - at) as f32, 0.0, adaptive::FADE_MS));
                let from = tone(from_light);
                (
                    codenotch_ui_kit::theme::lerp(from.0, to.0, k),
                    codenotch_ui_kit::theme::lerp(from.1, to.1, k),
                )
            }
            _ => to,
        }
    }
}

// ----------------------------------------------------------------------------------------
// Render
// ----------------------------------------------------------------------------------------

impl Render for NotchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.now_ms();
        if self.hwnd.is_none() {
            self.hwnd = kwin::hwnd(window);
            self.pending.styles = true;
        }
        if self.dirty {
            self.derive(cx);
        }
        if self.drag.is_some() {
            self.poll_drag(cx);
        }

        // --- hover (gpui tracks WM_MOUSELEAVE for us; checked against the pointer) ---
        let preview = preview_card();
        let os_hovered = window.is_window_hovered();
        let pointer_in = if os_hovered {
            self.hwnd.and_then(win32::cursor_in_window)
        } else {
            None
        };
        let hovered = fold::effective_hover(os_hovered, pointer_in, self.drag.is_some(), preview);
        if hovered != self.hovered {
            log::debug!(
                "notch hover: {} -> {hovered} (gpui {os_hovered}, pointer in {pointer_in:?})",
                self.hovered
            );
            self.hovered = hovered;
            self.left_at = if hovered { None } else { Some(now) };
        }
        // A press whose button-up never reached us (capture lost) must not hold the notch open
        // once the pointer is gone (only then: a release outside cancels the click anyway).
        if !hovered && self.down.is_some() && !win32::primary_button_down() {
            log::debug!("notch: dropping a press whose button is already up");
            self.down = None;
        }
        if hovered && self.drag.is_none() && !preview {
            if self.hover_watch.is_none() {
                self.hover_watch = self.watch_hover(cx);
            }
        } else {
            self.hover_watch = None;
        }
        if !self.hovered {
            self.orb_hover.set(false, now);
            self.grip_out.set(false, now);
            self.grip_swell.set(false, now);
        }

        // --- layout ---
        let n = self.d.cells.len();
        let s = self.d.scale;
        let (edge, along, _) = self.placement();
        if self.layout.frame.edge != edge || self.layout.n != n.max(1) || self.layout.scale != s {
            self.layout = NotchLayout::new(edge, n.max(1), s);
            self.card_drawn = None;
            self.card_glide = None;
            if self.card_cell.is_some_and(|i| i >= n) {
                self.card_cell = None;
            }
        }
        let visible = !self.d.hidden && n > 0;
        self.fold.set_cells(n);
        if preview && visible && self.card_cell.is_none() && !self.menu_open {
            self.card_cell = Some(0);
        }

        // --- window placement ---
        let monitor = match &self.drag {
            Some(d) => d.monitors.get(d.idx).cloned(),
            None => self.monitor.clone(),
        };
        let mut rect_now = self.applied_rect;
        if let Some(mon) = &monitor {
            let area = if self.d.fullscreen {
                mon.bounds
            } else {
                mon.work
            };
            let (w, h) = (self.layout.frame.w, self.layout.frame.h);
            let (wd, hd) = (
                (w * mon.scale).round() as i32,
                (h * mon.scale).round() as i32,
            );
            let target = placement::place_window(edge, along, area, wd, hd);
            if self.target != Some(target) {
                let from = self.applied_rect;
                let animate = !self.reduced
                    && self.drag.is_none()
                    && self.applied_visible == Some(true)
                    && from.is_some_and(|f| f.w == target.w && f.h == target.h && f != target);
                self.glide = match (animate, from) {
                    (true, Some(f)) => Some((f, target, now)),
                    _ => None,
                };
                self.target = Some(target);
            }
            let r = match self.glide {
                Some((a, b, t0)) => {
                    let k = progress((now - t0) as f32, 0.0, WINDOW_GLIDE_MS);
                    if k >= 1.0 {
                        self.glide = None;
                        b
                    } else {
                        placement::lerp_rect(a, b, Bezier::FOLD.at(k))
                    }
                }
                None => target,
            };
            if self.applied_rect != Some(r) {
                self.pending.place = Some(r);
                self.applied_rect = Some(r);
            }
            rect_now = Some(r);
        }
        let mon_scale = monitor.as_ref().map_or(1.0, |m| m.scale);
        let insets = match (rect_now, &monitor) {
            (Some(r), Some(m)) => {
                placement::along_insets(edge, placement::insets(r, m.work, m.scale))
            }
            _ => geom::Insets::default(),
        };

        // --- fold ---
        let inputs = FoldInputs {
            show_mode: self.d.settings.show_mode(),
            fullscreen_fold: self.d.fullscreen && self.d.settings.fold_for_full_screen,
            hovered: self.hovered,
            hold: self.drag.is_some() || self.menu_open || self.down.is_some(),
            grip_was_out: self.grip_was_out,
        };
        let want = fold::want(inputs);
        let folded = fold::folded_target(want, self.left_at, now);
        if self.logged_want != Some((want, folded)) {
            log::debug!(
                "notch fold: {want:?} folded={folded} (mode {:?}, hovered {}, hold {}, left {:?} ms ago)",
                inputs.show_mode,
                inputs.hovered,
                inputs.hold,
                self.left_at.map(|t| (now - t).round())
            );
            self.logged_want = Some((want, folded));
        }
        if !self.started || self.reduced || !visible {
            self.fold.snap(folded);
            self.started = visible;
        } else {
            self.fold.set_target(folded, now);
        }
        if folded {
            self.card_cell = None;
            self.grip_was_out = false;
        }

        // --- card target: grace after the pointer left ---
        if let Some(t) = self.left_at {
            if now - t >= CARD_GRACE_MS {
                self.card_cell = None;
            }
        }
        if self.drag.is_some() || self.menu_open || !visible {
            self.card_cell = None;
        }

        // --- presses ---
        for p in &mut self.presses {
            if p.released.is_some() {
                continue;
            }
            let cell = self.d.cells.iter().find(|c| c.account_id == p.id);
            let refreshing = cell.is_some_and(|c| c.refreshing);
            p.saw_refreshing |= refreshing;
            let landed = cell
                .is_none_or(|c| c.fetched_at != p.baseline || (p.saw_refreshing && !c.refreshing));
            let el = now - p.start;
            if el >= PRESS_MAX_MS || (el >= PRESS_MIN_MS && landed) {
                p.scale_at_release = p.scale(now);
                p.released = Some(now);
            }
        }
        self.presses.retain(|p| !p.done(now));

        // --- notice ---
        if self.notice.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.notice = None;
        }

        let at_rest = self.fold.at_rest(now);
        let l = self.layout.clone();
        let theme = self.d.theme;

        // --- card placement & measuring ---
        let (card_w, card_max_h) = l.card_limits();
        let mut card: Option<(usize, Rect, f32, bool)> = None; // cell, rect, tail_u, measured
        if let Some(i) = self.card_cell.filter(|&i| i < n) {
            if self.card_since.is_none_or(|(c, _)| c != i) {
                self.card_since = Some((i, now));
            }
            let id = &self.d.cells[i].account_id;
            // Safety net: never wait more than 250 ms for a measurement.
            if !self.heights.contains_key(id)
                && self.card_since.is_some_and(|(_, t)| now - t > 250.0)
            {
                self.heights.insert(id.clone(), 200.0 * s);
            }
            let measured = self.heights.get(id).copied();
            let h = measured.unwrap_or(200.0 * s).min(card_max_h);
            if let Some(place) = l.place_card(i, card_w, h, insets) {
                let (mut rect, mut tail_u) = (place.card, place.tail_u);
                match self.card_drawn {
                    Some((prev, r0, t0)) if prev != i && !self.reduced => {
                        self.card_glide = Some((r0, t0, now));
                    }
                    None => self.card_glide = None,
                    _ => {}
                }
                if let Some((r0, t0, at)) = self.card_glide {
                    let k = progress((now - at) as f32, 0.0, CARD_GLIDE_MS);
                    if k >= 1.0 {
                        self.card_glide = None;
                    } else {
                        let e = Bezier::GLIDE.at(k);
                        rect = r0.lerp(&rect, e);
                        tail_u = easing::lerp(t0, tail_u, e);
                    }
                }
                card = Some((i, rect, tail_u, measured.is_some()));
            }
        } else {
            self.card_glide = None;
            self.card_since = None;
        }
        self.card_drawn = card.map(|(i, r, t, _)| (i, r, t));

        // --- region / visibility / screen ---
        let mut hot: Vec<Rect> = Vec::new();
        let region_logical = if at_rest {
            Some(l.wake_rect())
        } else {
            hot.extend(l.open_hot_rects(self.grip_out.on));
            if let Some((_, r, tu, _)) = card {
                hot.push(r);
                hot.push(l.tail_rect(tu));
            }
            if self.notice.is_some() {
                hot.push(notice_rect(&l, insets));
            }
            hot_region(&hot, HOT_PAD * s, l.bounds())
        };
        let sf = window.scale_factor();
        if !self.no_region {
            let region = region_logical.map(|r| to_device(r, sf));
            if self.applied_region != Some(region) {
                self.applied_region = Some(region);
                self.pending.region = Some(region.into_iter().collect());
            }
        }
        if self.applied_visible != Some(visible) && (rect_now.is_some() || !visible) {
            self.applied_visible = Some(visible);
            self.pending.show = Some(visible);
        }
        // In taskbar mode the strip publishes where reset cards go.
        if let (Some(r), Some(m), false) = (
            rect_now,
            &monitor,
            crate::taskbar_probe::active(&self.d.settings),
        ) {
            let pill = if at_rest {
                l.rest_rect()
            } else {
                l.pill_rect()
            };
            let pd = to_device(pill, mon_scale);
            publish_screen(NotchScreen {
                pill: DeviceRect {
                    x: r.x + pd.x,
                    y: r.y + pd.y,
                    w: pd.w,
                    h: pd.h,
                },
                edge,
                density: s * mon_scale,
                work: m.work,
                visible,
            });
        }

        // --- adaptive sampling ---
        let probe = (at_rest && visible && self.d.settings.adaptive_pill)
            .then(|| {
                rect_now.map(|r| {
                    let p = to_device(l.probe_rect(), mon_scale);
                    DeviceRect {
                        x: r.x + p.x,
                        y: r.y + p.y,
                        w: p.w,
                        h: p.h,
                    }
                })
            })
            .flatten();
        self.sync_sampling(probe, cx);

        // --- wake-ups ---
        let mut frame = self.drag.is_some() || self.glide.is_some() || self.card_glide.is_some();
        frame |= visible && self.fold.animating(now);
        frame |= !self.presses.is_empty();
        frame |= card.is_some_and(|c| !c.3);
        let orb_anim = |t: &Toggle, d: f32| now - t.since < d as f64;
        frame |= orb_anim(&self.orb_hover, 550.0)
            || orb_anim(&self.grip_out, 440.0)
            || orb_anim(&self.grip_swell, 220.0);
        frame |= self.orb_click.is_some_and(|c| now - c < 430.0);
        frame |= self
            .rest_fade
            .is_some_and(|(_, at)| now - at < adaptive::FADE_MS as f64);
        let mut next: Option<f64> = None;
        let mut at = |t: f64| next = Some(next.map_or(t, |n: f64| n.min(t)));
        if visible && !at_rest && !self.reduced {
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
        if let Some(t) = self.left_at {
            if self.card_cell.is_some() {
                at(t + CARD_GRACE_MS);
            }
            if let fold::Want::Fold { delay_ms } = want {
                if !self.fold.folded() {
                    at(t + delay_ms);
                }
            }
        }
        if let Some((_, until)) = &self.notice {
            at(*until);
        }
        for p in &self.presses {
            if p.released.is_none() {
                at(p.start + PRESS_MIN_MS);
                at(p.start + PRESS_MAX_MS);
            }
        }
        if frame {
            window.request_animation_frame();
        } else if let Some(t) = next {
            self.schedule_wake(t, cx);
        }
        self.flush_ops(cx);

        self.build(window, cx, now, at_rest, visible, card, insets, theme)
    }
}

impl NotchView {
    #[allow(clippy::too_many_arguments)]
    fn build(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
        now: f64,
        at_rest: bool,
        visible: bool,
        card: Option<(usize, Rect, f32, bool)>,
        insets: geom::Insets,
        theme: Theme,
    ) -> AnyElement {
        let l = &self.layout;
        let s = self.d.scale;
        let (fw, fh) = (l.frame.w, l.frame.h);
        let mut root = div()
            .id("notch")
            .relative()
            .w(px(fw))
            .h(px(fh))
            .font_family("Segoe UI")
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_up))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right))
            .when(self.drag.is_some(), |d| d.cursor(CursorStyle::ClosedHand));
        if !visible {
            return root.into_any_element();
        }

        // --- open pill, clipped while folding ---
        if !at_rest {
            let clip = l.fold_clip(self.fold.clip(now));
            let mut inner = div()
                .absolute()
                .left(px(-clip.x))
                .top(px(-clip.y))
                .w(px(fw))
                .h(px(fh));
            inner = inner.child(pill_body(l, theme.pill, theme.edge));
            inner = inner.child(fillets(l, theme.pill, theme.edge));
            for (i, cellm) in self.d.cells.iter().enumerate() {
                let (op, slide) = if self.reduced {
                    (1.0, 0.0)
                } else {
                    self.fold.cell(i, now)
                };
                if op <= 0.0 {
                    continue;
                }
                let press = self.presses.iter().find(|p| p.id == cellm.account_id);
                let (scale_k, spin) = press.map_or((1.0, 0.0), |p| (p.scale(now), p.spin(now)));
                let off = l.frame.toward_edge(slide * s);
                if let Some(rect) = l.cell_rect(i) {
                    let glyph = self.d.glyphs.get(i).cloned().unwrap_or(Glyph::Letters("•"));
                    inner = inner.child(cell_element(
                        cellm,
                        &glyph,
                        rect,
                        off,
                        op,
                        scale_k,
                        spin,
                        now,
                        self.reduced,
                        s,
                        &theme,
                        &self.d.settings,
                    ));
                }
            }
            root = root.child(
                div()
                    .absolute()
                    .left(px(clip.x))
                    .top(px(clip.y))
                    .w(px(clip.w))
                    .h(px(clip.h))
                    .overflow_hidden()
                    .child(inner),
            );

            // --- orb and grip ---
            let handles = if self.reduced {
                1.0
            } else {
                self.fold.handles_opacity(now)
            };
            if handles > 0.0 {
                root = root.child(self.handles(now, handles, theme));
                if self.grip_out.on {
                    let g = l.grip_centre();
                    let r = GRIP_HOT_R * s;
                    root = root.child(
                        div()
                            .absolute()
                            .left(px(g.0 - r))
                            .top(px(g.1 - r))
                            .w(px(2.0 * r))
                            .h(px(2.0 * r))
                            .cursor(if self.drag.is_some() {
                                CursorStyle::ClosedHand
                            } else {
                                CursorStyle::OpenHand
                            }),
                    );
                }
            }
        }

        // --- rest pill ---
        let rest_op = if self.reduced {
            if at_rest {
                1.0
            } else {
                0.0
            }
        } else {
            self.fold.rest_opacity(now)
        };
        if rest_op > 0.0 {
            let (fill, line) = self.rest_colors(now);
            root = root.child(rest_pill(l, fill, line).opacity(rest_op));
        }

        // --- hover card ---
        if let Some((i, rect, tail_u, measured)) = card {
            if let Some(m) = self.d.cards.get(i).cloned() {
                root = root.child(self.card_element(&m, rect, tail_u, measured, theme, cx));
            }
        } else if let Some((text, _)) = &self.notice {
            let r = notice_rect(l, insets);
            root = root.child(
                div()
                    .absolute()
                    .left(px(r.x))
                    .top(px(r.y))
                    .w(px(r.w))
                    .bg(theme.pill)
                    .border_1()
                    .border_color(theme.edge)
                    .rounded(px(CARD_RADIUS * s))
                    .p(px(12.0 * s))
                    .text_size(px(12.0 * s))
                    .line_height(px(18.0 * s))
                    .text_color(theme.muted_foreground)
                    .child(text.clone()),
            );
        }
        root.into_any_element()
    }

    /// Orb (resting quarter arc ↔ disc with gear) and grip dots.
    fn handles(&self, now: f64, opacity: f32, theme: Theme) -> impl IntoElement {
        let l = &self.layout;
        let s = self.d.scale;
        let orb = l.orb_centre();
        let arc_start = l.orb_arc_start();
        let reduced = self.reduced;
        let hov = &self.orb_hover;
        let (disc_k, arc_k) = if reduced {
            (
                if hov.on { 1.0 } else { 0.0 },
                if hov.on { 0.0 } else { 1.0 },
            )
        } else if hov.on {
            (hov.t(now, 0.0, 200.0), 1.0 - hov.t(now, 0.0, 200.0))
        } else {
            (1.0 - hov.t(now, 0.0, 200.0), hov.t(now, 0.0, 200.0))
        };
        let tr = if reduced {
            1.0
        } else {
            Bezier::ORB.at(hov.t(now, 0.0, 360.0))
        };
        let (disc_scale, arc_scale) = if hov.on {
            (1.1 - 0.1 * tr, 1.0 - 0.14 * tr)
        } else {
            (1.0, 0.86 + 0.14 * tr)
        };
        let gear_t = if reduced {
            1.0
        } else {
            Bezier::GEAR.at(hov.t(now, 0.0, 550.0))
        };
        let (mut gear_rot, mut gear_scale) = if hov.on {
            (-60.0 * (1.0 - gear_t), 0.5 + 0.5 * gear_t)
        } else {
            (0.0, 1.0)
        };
        let mut press_k = 1.0;
        if let Some(c) = self.orb_click.filter(|_| !reduced) {
            let t = (now - c) as f32;
            let down = 0.21 * 430.0;
            press_k = if t < down {
                1.0 - 0.16 * Bezier::EASE_OUT.at(t / down)
            } else {
                0.84 + 0.16 * Bezier::BOUNCE.at(progress(t, down, 430.0 - down))
            };
            gear_rot += 360.0 * Bezier::EASE.at(progress(t, 0.0, 430.0));
        }
        gear_scale *= press_k;
        let disc_scale = disc_scale * press_k;

        let spread = if reduced {
            if self.grip_out.on {
                1.0
            } else {
                0.0
            }
        } else if self.grip_out.on {
            Bezier::GRIP_OUT.at(self.grip_out.t(now, 20.0, 420.0))
        } else {
            1.0 - Bezier::GRIP_IN.at(self.grip_out.t(now, 0.0, 200.0))
        };
        let swell_t = self.grip_swell.t(now, 0.0, 220.0);
        let swell = 1.0
            + GRIP_SWELL
                * if self.grip_swell.on {
                    swell_t
                } else {
                    1.0 - swell_t
                };
        let dots = l.grip_dots(spread.max(0.0));

        let pill = theme.pill.opacity(opacity);
        let edge = theme.edge.opacity(opacity);
        let gear_c = theme.ink2.opacity(opacity * disc_k);
        canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                let o = b.origin;
                let c = pt(o, orb);
                if arc_k > 0.0 {
                    let r = ORB_ARC_R * s * arc_scale;
                    let start = arc_start + (90.0 - ORB_ARC_SWEEP) / 2.0;
                    paint_arc(
                        window,
                        c,
                        r,
                        ORB_ARC_RIM_W * s,
                        start,
                        ORB_ARC_SWEEP,
                        edge.opacity(arc_k),
                        Cap::Round,
                    );
                    paint_arc(
                        window,
                        c,
                        r,
                        ORB_ARC_FILL_W * s,
                        start,
                        ORB_ARC_SWEEP,
                        pill.opacity(arc_k),
                        Cap::Round,
                    );
                }
                if spread > 0.01 {
                    let r = GRIP_DOT_D / 2.0 * s * swell;
                    let k = spread.min(1.0);
                    for d in dots {
                        paint_disc(window, pt(o, d), r + 0.5 * s, edge.opacity(k));
                        paint_disc(window, pt(o, d), r - 0.5 * s, pill.opacity(k));
                    }
                }
                if disc_k > 0.0 {
                    let r = ORB_DISC_D / 2.0 * s * disc_scale;
                    paint_disc(window, c, r + 0.5 * s, edge.opacity(disc_k));
                    paint_disc(window, c, r - 0.5 * s, pill.opacity(disc_k));
                    let (outline, hole) =
                        geom::gear(orb, ORB_GEAR / 2.0 * s * gear_scale, 8, gear_rot);
                    let mut pb =
                        PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::even_odd()));
                    pb.add_polygon(&pts(o, &outline), true);
                    pb.add_polygon(&pts(o, &hole), true);
                    if let Ok(path) = pb.build() {
                        window.paint_path(path, gear_c);
                    }
                }
            },
        )
        .absolute()
        .left_0()
        .top_0()
        .size_full()
    }

    fn card_element(
        &self,
        m: &CardModel,
        rect: Rect,
        tail_u: f32,
        measured: bool,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let l = &self.layout;
        let s = self.d.scale;
        let tail = l.tail_polygon(tail_u, 12);
        let tail_rim = geom::grow_from_base(&tail, BORDER * s * 1.5);
        // One material with the pill: same fill, `edge` hairline around card + tail, no seam.
        let card_c = theme.pill;
        let rim_c = theme.edge;
        let glyph = self
            .d
            .cells
            .iter()
            .position(|c| c.account_id == m.account_id)
            .and_then(|i| self.d.glyphs.get(i).cloned())
            .unwrap_or(Glyph::Letters("•"));

        // measuring: the content's natural height, read back next frame
        let id = m.account_id.clone();
        let known = self.heights.get(&id).copied();
        let weak = cx.entity().downgrade();
        let (_, max_h) = l.card_limits();
        let pad = CARD_PAD * s;
        let probe = canvas(
            move |b, window, _| {
                let h = (f32::from(b.size.height) + 2.0 * pad).min(max_h);
                if known.is_none_or(|k| (k - h).abs() > 0.5) {
                    let weak = weak.clone();
                    let id = id.clone();
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
            m,
            &glyph,
            s,
            &theme,
            &self.d.settings,
            cx,
            NotchView::sign_in,
        )
        .relative()
        .child(probe);
        div()
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .when(!measured, |d| d.opacity(0.0))
            .child(
                div()
                    .absolute()
                    .left(px(rect.x - BORDER * s))
                    .top(px(rect.y - BORDER * s))
                    .w(px(rect.w + 2.0 * BORDER * s))
                    .h(px(rect.h + 2.0 * BORDER * s))
                    .bg(rim_c)
                    .rounded(px((CARD_RADIUS + BORDER) * s)),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, _| {
                        fill_poly(window, &pts(b.origin, &tail_rim), rim_c);
                        fill_poly(window, &pts(b.origin, &tail), card_c);
                    },
                )
                .absolute()
                .left_0()
                .top_0()
                .size_full(),
            )
            .child(
                div()
                    .id("card")
                    .absolute()
                    .left(px(rect.x))
                    .top(px(rect.y))
                    .w(px(rect.w))
                    .h(px(rect.h))
                    .bg(card_c)
                    .rounded(px(CARD_RADIUS * s))
                    .p(px(pad))
                    .overflow_y_scroll()
                    .child(content),
            )
    }
}

// ----------------------------------------------------------------------------------------
// Element helpers
// ----------------------------------------------------------------------------------------

fn to_device(r: Rect, sf: f32) -> DeviceRect {
    let x = (r.x * sf).floor() as i32;
    let y = (r.y * sf).floor() as i32;
    DeviceRect {
        x,
        y,
        w: (r.right() * sf).ceil() as i32 - x,
        h: (r.bottom() * sf).ceil() as i32 - y,
    }
}

fn pt(o: Point<Pixels>, p: Pt) -> Point<Pixels> {
    point(o.x + px(p.0), o.y + px(p.1))
}

fn pts(o: Point<Pixels>, v: &[Pt]) -> Vec<Point<Pixels>> {
    v.iter().map(|p| pt(o, *p)).collect()
}

fn fill_poly(window: &mut Window, poly: &[Point<Pixels>], color: Hsla) {
    let mut pb = PathBuilder::fill();
    pb.add_polygon(poly, true);
    if let Ok(path) = pb.build() {
        window.paint_path(path, color);
    }
}

/// Notice bubble: card-sized, beside the middle of the pill.
fn notice_rect(l: &NotchLayout, insets: geom::Insets) -> Rect {
    let mid = l.n / 2;
    let s = l.scale;
    l.place_card(mid, CARD_W * s, 72.0 * s, insets)
        .map_or(Rect::default(), |p| p.card)
}

/// Border widths and radii of a box flush with the screen edge: no border on the screen side,
/// radius on the two inner corners.
fn edge_box(d: gpui::Div, edge: Edge, border: f32, radius: f32) -> gpui::Div {
    let mut d = d;
    {
        let st = d.style();
        let b = Some(gpui::AbsoluteLength::Pixels(px(border)));
        let r = Some(gpui::AbsoluteLength::Pixels(px(radius)));
        let bw = &mut st.border_widths;
        let cr = &mut st.corner_radii;
        match edge {
            Edge::Right => {
                (bw.top, bw.left, bw.bottom) = (b, b, b);
                (cr.top_left, cr.bottom_left) = (r, r);
            }
            Edge::Left => {
                (bw.top, bw.right, bw.bottom) = (b, b, b);
                (cr.top_right, cr.bottom_right) = (r, r);
            }
            Edge::Top => {
                (bw.left, bw.right, bw.bottom) = (b, b, b);
                (cr.bottom_left, cr.bottom_right) = (r, r);
            }
            Edge::Bottom => {
                (bw.left, bw.right, bw.top) = (b, b, b);
                (cr.top_left, cr.top_right) = (r, r);
            }
        }
    }
    d
}

fn pill_body(l: &NotchLayout, fill: Hsla, line: Hsla) -> gpui::Div {
    let r = l.pill_rect();
    let s = l.scale;
    let radius = PILL_RADIUS.min(l.pill.h / s / 2.0).min(l.pill.w / s / 2.0) * s;
    let d = div()
        .absolute()
        .left(px(r.x))
        .top(px(r.y))
        .w(px(r.w))
        .h(px(r.h))
        .bg(fill)
        .border_color(line);
    edge_box(d, l.frame.edge, BORDER * s, radius)
}

fn rest_pill(l: &NotchLayout, fill: Hsla, line: Hsla) -> gpui::Div {
    let r = l.rest_rect();
    let s = l.scale;
    let d = div()
        .absolute()
        .left(px(r.x))
        .top(px(r.y))
        .w(px(r.w))
        .h(px(r.h))
        .bg(fill)
        .border_color(line);
    edge_box(d, l.frame.edge, BORDER * s, REST_RADIUS * s)
}

/// Both inverse-rounded shoulders: the edge-coloured wedge (radius F−1) under the pill-coloured
/// one (radius F) leaves the 1 px rim.
fn fillets(l: &NotchLayout, fill: Hsla, line: Hsla) -> impl IntoElement {
    let s = l.scale;
    let f = FILLET_R * s;
    let polys = [
        (l.fillet_polygon(false, f - BORDER * s, 32), line),
        (l.fillet_polygon(false, f, 32), fill),
        (l.fillet_polygon(true, f - BORDER * s, 32), line),
        (l.fillet_polygon(true, f, 32), fill),
    ];
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            for (poly, color) in &polys {
                fill_poly(window, &pts(b.origin, poly), *color);
            }
        },
    )
    .absolute()
    .left_0()
    .top_0()
    .size_full()
}

/// Percent figure font: Segoe UI semibold with tabular numerals (built once).
static FIGURE_FONT: std::sync::LazyLock<Font> = std::sync::LazyLock::new(|| Font {
    features: FontFeatures(Arc::new(vec![("tnum".into(), 1)])),
    weight: FontWeight::SEMIBOLD,
    style: FontStyle::Normal,
    ..gpui::font("Segoe UI")
});

pub(crate) fn figure_font() -> Font {
    FIGURE_FONT.clone()
}

/// A provider mark: SVG tinted `color`, PNG override, or fallback letters.
pub(crate) fn glyph_element(g: &Glyph, size: f32, color: Hsla, letters_px: f32) -> AnyElement {
    match g {
        Glyph::Svg(path) => svg()
            .path(path.clone())
            .w(px(size))
            .h(px(size))
            .flex_none()
            .text_color(color)
            .into_any_element(),
        Glyph::Png(p) => img(p.clone())
            .w(px(size))
            .h(px(size))
            .flex_none()
            .opacity(color.a)
            .into_any_element(),
        Glyph::Letters(text) => {
            let fs = if text.chars().count() > 1 {
                letters_px * 0.75
            } else {
                letters_px
            };
            div()
                .w(px(size))
                .h(px(size))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .whitespace_nowrap()
                .text_size(px(fs))
                .line_height(px(size))
                .font_weight(FontWeight::BOLD)
                .text_color(color)
                .child(*text)
                .into_any_element()
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cell_element(
    c: &CellModel,
    glyph: &Glyph,
    rect: Rect,
    off: Pt,
    opacity: f32,
    press: f32,
    spin: f32,
    now: f64,
    reduced: bool,
    s: f32,
    theme: &Theme,
    settings: &Settings,
) -> impl IntoElement {
    let fade = |col: Hsla| {
        if c.stale {
            col.opacity(STALE_OPACITY)
        } else {
            col
        }
    };
    let k = s * press;
    let track = fade(theme.track);
    let main_c = fade(theme.usage_color(c.main.map(|a| a.used), settings));
    // No hole disc: the glyph sits on the plain pill; a 0 % / unknown cell keeps only the track.
    let mut spec = RingSpec::notch(k).main(c.main.map_or(0.0, |a| a.fraction), main_c, track);
    if let Some(w) = c.weekly {
        let wc = fade(
            theme
                .usage_color(Some(w.used), settings)
                .opacity(WEEKLY_ARC_OPACITY),
        );
        let wt = fade(theme.track.opacity(WEEKLY_TRACK_OPACITY));
        spec = spec.weekly(
            settings.weekly_ring,
            w.fraction,
            wc,
            wt,
            settings.weekly_ring_dashed,
        );
    }
    for layer in spec.layers.iter_mut() {
        layer.start_deg += spin;
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
    let ring_box = RING_BOX * s;
    let glyph_px =
        codenotch_ui_kit::metrics::ring_units::GLYPH * codenotch_ui_kit::metrics::RING_K * k;
    let glyph_c = if c.dim_glyph {
        theme.ink2.opacity(STALE_OPACITY)
    } else {
        theme.ink2
    };
    let letters = 20.0 * codenotch_ui_kit::metrics::RING_K * k;
    let text_w = 70.0 * s;
    div()
        .absolute()
        .left(px(rect.x + off.0))
        .top(px(rect.y + off.1))
        .w(px(rect.w))
        .h(px(rect.h))
        .opacity(opacity)
        .child(
            canvas(
                |_, _, _| {},
                move |b, _, window, _| paint_ring(window, b.center(), &spec),
            )
            .absolute()
            .left_0()
            .top_0()
            .w(px(ring_box))
            .h(px(ring_box)),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w(px(ring_box))
                .h(px(ring_box))
                .flex()
                .items_center()
                .justify_center()
                .child(glyph_element(glyph, glyph_px, glyph_c, letters)),
        )
        .child(
            div()
                .absolute()
                .left(px((ring_box - text_w) / 2.0))
                .top(px(ring_box + CELL_TEXT_GAP * s))
                .w(px(text_w))
                .h(px(PERCENT_LINE * s))
                .flex()
                .justify_center()
                .whitespace_nowrap()
                .font(figure_font())
                .font_weight(FontWeight::BOLD)
                .text_size(px(PERCENT_FONT * s))
                .line_height(px(PERCENT_LINE * s))
                .text_color(theme.ink)
                .child(c.figure.clone()),
        )
}

fn dot_color(theme: &Theme, state: ActivityState) -> Hsla {
    match state {
        ActivityState::Working => theme.ink,
        ActivityState::Waiting => theme.watch,
        ActivityState::Done => theme.ample,
        ActivityState::Idle => theme.track,
    }
}

fn card_row(r: &Row, s: f32, theme: &Theme, settings: &Settings) -> gpui::Div {
    let mut d = div().flex().flex_col().mt(px(12.0 * s)).child(
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .justify_between()
            .items_baseline()
            .gap_x(px(8.0 * s))
            .child(
                div()
                    .whitespace_nowrap()
                    .text_size(px(13.0 * s))
                    .line_height(px(17.0 * s))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(r.label.clone()),
            )
            .when_some(r.reset.clone(), |d, reset| {
                d.child(
                    div()
                        .whitespace_nowrap()
                        .text_size(px(12.0 * s))
                        .line_height(px(16.0 * s))
                        .text_color(theme.muted_foreground)
                        .child(reset),
                )
            }),
    );
    if let Some(Bar { fraction, used }) = &r.bar {
        // Same severity palette as the rings.
        let fill = theme.usage_color(*used, settings);
        let h = codenotch_ui_kit::metrics::CARD_BAR_H * s;
        let f = fraction.clamp(0.0, 1.0);
        d = d.child(
            div()
                .mt(px(7.0 * s))
                .mb(px(5.0 * s))
                .h(px(h))
                .w_full()
                .rounded(px(h / 2.0))
                .bg(theme.track)
                .when(f > 0.0, |d| {
                    // At least a round dot, like the rings' round caps.
                    d.child(
                        div()
                            .h_full()
                            .min_w(px(h))
                            .w(gpui::relative(f))
                            .rounded(px(h / 2.0))
                            .bg(fill),
                    )
                }),
        );
    } else {
        d = d.child(div().h(px(4.0 * s)));
    }
    let small = |text: String| {
        div()
            .text_size(px(12.0 * s))
            .line_height(px(16.0 * s))
            .text_color(theme.muted_foreground)
            .child(text)
    };
    d = d.child(small(r.line.clone()));
    if let Some((text, tone)) = &r.forecast {
        let color = match tone {
            ForecastTone::Muted => theme.muted_foreground,
            ForecastTone::Warn => theme.watch,
            ForecastTone::Critical => theme.crit,
        };
        d = d.child(small(text.clone()).text_color(color));
    }
    if let Some(detail) = &r.detail {
        d = d.child(small(detail.clone()));
    }
    d
}

/// Hover card body (shared by the notch and the taskbar strip's card window). `sign_in` is
/// called from the "Sign in" button with the account id.
pub(crate) fn card_content<V: 'static>(
    m: &CardModel,
    glyph: &Glyph,
    s: f32,
    theme: &Theme,
    settings: &Settings,
    cx: &mut Context<V>,
    sign_in: fn(&mut V, String, &mut Context<V>),
) -> gpui::Div {
    let note = |text: String| {
        div()
            .mt(px(6.0 * s))
            .text_size(px(12.0 * s))
            .line_height(px(18.0 * s))
            .text_color(theme.muted_foreground)
            .child(text)
    };
    let mut d = div().flex().flex_col().w_full().flex_none().child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0 * s))
            .mb(px(2.0 * s))
            .child(glyph_element(glyph, 18.0 * s, theme.foreground, 13.0 * s))
            .child(
                div()
                    .text_size(px(16.0 * s))
                    .line_height(px(21.0 * s))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
                    .child(m.title.clone()),
            ),
    );
    if let Some(line) = &m.account_line {
        d = d.child(
            div()
                .mt(px(-6.0 * s))
                .mb(px(10.0 * s))
                .text_size(px(11.0 * s))
                .line_height(px(15.0 * s))
                .text_color(theme.muted_foreground)
                .child(line.clone()),
        );
    }
    if let Some(line) = &m.stale_line {
        d = d.child(
            div()
                .text_size(px(11.0 * s))
                .line_height(px(15.0 * s))
                .text_color(theme.muted_foreground)
                .child(line.clone()),
        );
    }
    if let Some(line) = &m.not_current {
        d = d.child(note(line.clone()));
    }
    for b in &m.blocks {
        match b {
            Block::Row(r) => d = d.child(card_row(r, s, theme, settings)),
            Block::Group { heading, rows } => {
                d = d.child(
                    div()
                        .mt(px(12.0 * s))
                        .text_size(px(12.0 * s))
                        .line_height(px(16.0 * s))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child(heading.clone()),
                );
                let mut boxed = div()
                    .mt(px(6.0 * s))
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(10.0 * s))
                    .px(px(10.0 * s))
                    .pb(px(8.0 * s));
                for r in rows {
                    boxed = boxed.child(card_row(r, s, theme, settings));
                }
                d = d.child(boxed);
            }
        }
    }
    for n in &m.notes {
        d = d.child(note(n.clone()));
    }
    if let Some(sign) = &m.sign_in {
        let id = m.account_id.clone();
        let busy = sign.busy;
        d = d.child(
            div()
                .flex()
                .flex_row()
                .gap(px(6.0 * s))
                .mt(px(10.0 * s))
                .child(
                    div()
                        .id("sign-in")
                        .text_size(px(11.0 * s))
                        .line_height(px(14.0 * s))
                        .px(px(9.0 * s))
                        .py(px(6.0 * s))
                        .rounded(px(7.0 * s))
                        .bg(theme.control)
                        .border_1()
                        .border_color(theme.input)
                        .text_color(theme.foreground)
                        .when(busy, |d| d.opacity(0.5))
                        .when(!busy, |d| {
                            let (h, hl) = (theme.control_hover, theme.input);
                            d.hover(move |st| st.bg(h).border_color(hl)).on_click(
                                cx.listener(move |v, _, _, cx| sign_in(v, id.clone(), cx)),
                            )
                        })
                        .child(sign.label.clone()),
                ),
        );
    }
    if !m.sessions.is_empty() {
        let mut list = div()
            .mt(px(12.0 * s))
            .pt(px(8.0 * s))
            .border_t_1()
            .border_color(theme.border)
            .flex()
            .flex_col();
        for row in &m.sessions {
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0 * s))
                    .py(px(2.0 * s))
                    .text_size(px(11.0 * s))
                    .line_height(px(15.0 * s))
                    .text_color(theme.foreground)
                    .child(
                        div()
                            .flex_none()
                            .w(px(6.0 * s))
                            .h(px(6.0 * s))
                            .rounded_full()
                            .bg(dot_color(theme, row.state)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(row.left.clone()))
                    .when_some(row.right.clone(), |d, r| {
                        d.child(
                            div()
                                .flex_none()
                                .whitespace_nowrap()
                                .text_color(theme.muted_foreground)
                                .child(r),
                        )
                    }),
            );
        }
        if let Some(more) = &m.more {
            list = list.child(
                div()
                    .text_size(px(11.0 * s))
                    .line_height(px(15.0 * s))
                    .text_color(theme.muted_foreground)
                    .child(more.clone()),
            );
        }
        d = d.child(list);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::{must_unhide, wants_visible};

    #[test]
    fn visible_only_when_not_hidden_with_cells() {
        assert!(wants_visible(false, 1));
        assert!(!wants_visible(false, 0));
        assert!(!wants_visible(true, 5));
    }

    /// Regression: `--demo` (or a first launch) starts with no accounts, so the notch hides
    /// itself; the later snapshot with accounts must show it again from outside `render`.
    #[test]
    fn accounts_arriving_after_an_empty_start_unhide_the_notch() {
        assert!(must_unhide(Some(false), false, 5));
        // Still nothing to show, or hidden by state/settings: stay hidden.
        assert!(!must_unhide(Some(false), false, 0));
        assert!(!must_unhide(Some(false), true, 5));
        // Already shown, or never rendered yet: `render` handles it.
        assert!(!must_unhide(Some(true), false, 5));
        assert!(!must_unhide(None, false, 5));
    }
}
