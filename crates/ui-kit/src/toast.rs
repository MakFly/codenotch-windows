//! Toast notifications in the manner of react-hot-toast.
//!
//! * [`ToastQueue`] — pure state: newest first, at most [`MAX_VISIBLE`] shown, per-kind
//!   durations, pause / resume (hover), in-place update (promise pattern), stacking offsets
//!   with smooth reflow, enter / exit progress. Time is a plain `u64` of milliseconds so every
//!   rule is unit tested.
//! * [`Toaster`] — the gpui entity that owns a queue and renders it as an overlay layer
//!   (absolutely positioned, on top of its host's content). Put it as the **last child** of a
//!   `relative()` root: `.child(toaster.clone())`. Frames are requested only while something
//!   moves (enter / exit / reflow / icon / spinner); otherwise one timer wakes it at the next
//!   expiry.
//! * Free functions [`success`], [`error`], [`loading`], [`blank`], [`custom`], [`show`],
//!   [`update`], [`dismiss`] — go to the toaster registered with [`set_host`] (the settings
//!   window while it is open, else the standalone toast window near the notch,
//!   `codenotch_ui_notch::toast_host`, opened on demand by the app). They return `None` when
//!   no host is open.
//! * Each toast carries [`ToastExtras`]: up to two buttons, and whether a click dismisses it
//!   ([`ToastOptions::persistent`] for a toast only an update may remove).
//!
//! Look: 8 px radius, `popover` fill, 1 px `border`, soft shadow (larger in light mode), padding
//! 8 × 10 px, max width 350 px, 14 px text, a 20 px icon on the left (success: green disc and a
//! white check drawn after a pop; error: red disc and a white ×; loading: spinner). Enter: slide
//! from −20 px, scale 0.6 → 1, fade, `cubic-bezier(.21, 1.02, .73, 1)` over 350 ms. Exit: fade,
//! scale 0.9, slide up, 400 ms. Windows "Animation effects" off: fades only.

use crate::ring::{paint_arc, paint_disc, Cap};
use crate::Theme;
use gpui::prelude::FluentBuilder;
use gpui::{
    canvas, div, hsla, point, px, AnyElement, App, BoxShadow, ClickEvent, Context,
    Entity, FontWeight, Global, Hsla, InteractiveElement, IntoElement, ParentElement,
    PathBuilder, Pixels, Point, Render, SharedString, StatefulInteractiveElement, Styled, Task,
    WeakEntity, Window,
};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Toasts shown at once; older ones are dismissed when a new one arrives.
pub const MAX_VISIBLE: usize = 3;
/// Vertical gap between stacked toasts.
pub const GAP: f32 = 8.0;
/// Widest toast.
pub const MAX_WIDTH: f32 = 350.0;
/// Distance from the window edge.
pub const EDGE_MARGIN: f32 = 16.0;
/// Enter animation.
pub const ENTER_MS: u64 = 350;
/// Exit animation (then the toast is removed).
pub const EXIT_MS: u64 = 400;
/// Other toasts sliding to their new place.
pub const REFLOW_MS: u64 = 250;
/// Icon pop + check / cross drawing, from the moment the kind was set.
pub const ICON_MS: u64 = 450;
/// Size assumed before a toast was measured.
const ESTIMATE: (f32, f32) = (220.0, 38.0);

pub type ToastId = u64;

/// What the toast says, and its icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastKind {
    /// No icon.
    Blank,
    Success,
    Error,
    /// Spinner; stays until updated or dismissed.
    Loading,
    /// A custom icon: an emoji or a short text / glyph.
    Custom(SharedString),
}

impl ToastKind {
    /// react-hot-toast durations: success 2 s, error 4 s, loading until updated, others 4 s.
    pub fn default_duration(&self) -> Option<u64> {
        match self {
            ToastKind::Success => Some(2000),
            ToastKind::Error => Some(4000),
            ToastKind::Loading => None,
            ToastKind::Blank | ToastKind::Custom(_) => Some(4000),
        }
    }
}

/// Where the stack sits in its host window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastPosition {
    /// Centred at the top, newest on top.
    #[default]
    TopCenter,
    /// Bottom-right corner, newest at the bottom.
    BottomRight,
}

impl ToastPosition {
    /// Direction of "away from the edge" on screen: −1 = up (top stacks), +1 = down.
    fn dir(self) -> f32 {
        match self {
            ToastPosition::TopCenter => -1.0,
            ToastPosition::BottomRight => 1.0,
        }
    }
}

/// One toast in a [`ToastQueue`]. `A` is the action payload (the button's handler in
/// [`Toaster`]).
#[derive(Debug, Clone)]
pub struct Entry<A> {
    pub id: ToastId,
    pub kind: ToastKind,
    pub message: SharedString,
    pub action: Option<A>,
    /// Enter animation start.
    pub created_at: u64,
    /// When the current kind was set (icon animation start).
    pub kind_at: u64,
    /// Expiry (absolute ms); `None` = sticky.
    pub deadline: Option<u64>,
    /// Exit animation start.
    pub leaving_at: Option<u64>,
    /// Measured size (width, height) of the card.
    pub size: Option<(f32, f32)>,
    y_from: f32,
    y_to: f32,
    y_at: u64,
    placed: bool,
}

impl<A> Entry<A> {
    pub fn leaving(&self) -> bool {
        self.leaving_at.is_some()
    }

    fn height(&self) -> f32 {
        self.size.map_or(ESTIMATE.1, |s| s.1)
    }

    /// Distance from the anchor edge at `now` (eased toward the latest target).
    pub fn offset(&self, now: u64) -> f32 {
        let t = progress(now, self.y_at, REFLOW_MS);
        self.y_from + (self.y_to - self.y_from) * ease_out(t)
    }

    /// Target distance from the anchor edge.
    pub fn target(&self) -> f32 {
        self.y_to
    }
}

/// The toasts of one host, newest first. Pure: all times are milliseconds from any origin.
#[derive(Debug, Clone)]
pub struct ToastQueue<A> {
    entries: Vec<Entry<A>>,
    next_id: ToastId,
    paused_since: Option<u64>,
    max_visible: usize,
}

impl<A> Default for ToastQueue<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A> ToastQueue<A> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 1,
            paused_since: None,
            max_visible: MAX_VISIBLE,
        }
    }

    pub fn with_max_visible(mut self, n: usize) -> Self {
        self.max_visible = n.max(1);
        self
    }

    /// Newest first, leaving ones included.
    pub fn entries(&self) -> &[Entry<A>] {
        &self.entries
    }

    pub fn get(&self, id: ToastId) -> Option<&Entry<A>> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Toasts not leaving.
    pub fn visible_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.leaving()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn is_paused(&self) -> bool {
        self.paused_since.is_some()
    }

    fn deadline_for(&self, duration: Option<u64>, now: u64) -> Option<u64> {
        // While paused the clock is frozen at the pause start; `resume` shifts every deadline.
        duration.map(|d| self.paused_since.unwrap_or(now) + d)
    }

    /// Add a toast on top. `duration`: `None` = the kind's default, `Some(None)` = sticky.
    pub fn push(
        &mut self,
        kind: ToastKind,
        message: impl Into<SharedString>,
        action: Option<A>,
        duration: Option<Option<u64>>,
        now: u64,
    ) -> ToastId {
        let id = self.next_id;
        self.next_id += 1;
        let duration = duration.unwrap_or_else(|| kind.default_duration());
        let deadline = self.deadline_for(duration, now);
        self.entries.insert(
            0,
            Entry {
                id,
                kind,
                message: message.into(),
                action,
                created_at: now,
                kind_at: now,
                deadline,
                leaving_at: None,
                size: None,
                y_from: 0.0,
                y_to: 0.0,
                y_at: now,
                placed: false,
            },
        );
        // Too many: the oldest visible ones leave.
        let mut shown = 0;
        for e in self.entries.iter_mut() {
            if e.leaving() {
                continue;
            }
            shown += 1;
            if shown > self.max_visible {
                e.leaving_at = Some(now);
            }
        }
        self.relayout(now);
        id
    }

    /// Change a toast in place (loading → success / error). Restarts its timer with the new
    /// kind's duration and replays the icon animation. False when it is gone or leaving.
    pub fn update(
        &mut self,
        id: ToastId,
        kind: ToastKind,
        message: impl Into<SharedString>,
        action: Option<A>,
        now: u64,
    ) -> bool {
        self.update_full(id, kind, message, action, None, now)
    }

    /// [`ToastQueue::update`] with an explicit duration (`None` = the kind's default,
    /// `Some(None)` = sticky).
    pub fn update_full(
        &mut self,
        id: ToastId,
        kind: ToastKind,
        message: impl Into<SharedString>,
        action: Option<A>,
        duration: Option<Option<u64>>,
        now: u64,
    ) -> bool {
        let duration = duration.unwrap_or_else(|| kind.default_duration());
        let deadline = self.deadline_for(duration, now);
        let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.id == id && !e.leaving())
        else {
            return false;
        };
        if e.kind != kind {
            e.kind_at = now;
        }
        e.kind = kind;
        e.message = message.into();
        e.action = action;
        e.deadline = deadline;
        // The text may wrap differently: measure again.
        e.size = None;
        self.relayout(now);
        true
    }

    /// Restart a toast's timer with `duration` (e.g. "Saved" shown again).
    pub fn extend(&mut self, id: ToastId, duration: Option<u64>, now: u64) -> bool {
        let deadline = self.deadline_for(duration, now);
        match self
            .entries
            .iter_mut()
            .find(|e| e.id == id && !e.leaving())
        {
            Some(e) => {
                e.deadline = deadline;
                true
            }
            None => false,
        }
    }

    /// Start the exit animation. False when unknown or already leaving.
    pub fn dismiss(&mut self, id: ToastId, now: u64) -> bool {
        let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.id == id && !e.leaving())
        else {
            return false;
        };
        e.leaving_at = Some(now);
        self.relayout(now);
        true
    }

    pub fn dismiss_all(&mut self, now: u64) {
        for e in self.entries.iter_mut().filter(|e| e.leaving_at.is_none()) {
            e.leaving_at = Some(now);
        }
        self.relayout(now);
    }

    /// Freeze every timer (pointer over a toast).
    pub fn pause(&mut self, now: u64) {
        if self.paused_since.is_none() {
            self.paused_since = Some(now);
        }
    }

    /// Resume the timers where they stopped.
    pub fn resume(&mut self, now: u64) {
        if let Some(since) = self.paused_since.take() {
            let shift = now.saturating_sub(since);
            for e in self.entries.iter_mut() {
                if let Some(d) = e.deadline.as_mut() {
                    *d += shift;
                }
            }
        }
    }

    /// Expire due toasts and drop finished exits. True when something changed.
    pub fn tick(&mut self, now: u64) -> bool {
        let mut changed = false;
        if self.paused_since.is_none() {
            for e in self.entries.iter_mut() {
                if e.leaving_at.is_none() && e.deadline.is_some_and(|d| d <= now) {
                    e.leaving_at = Some(now);
                    changed = true;
                }
            }
        }
        let before = self.entries.len();
        self.entries
            .retain(|e| e.leaving_at.is_none_or(|at| now < at + EXIT_MS));
        changed |= self.entries.len() != before;
        if changed {
            self.relayout(now);
        }
        changed
    }

    /// Record a measured card size; true when it moved the stack.
    pub fn set_size(&mut self, id: ToastId, w: f32, h: f32, now: u64) -> bool {
        let Some(e) = self.entries.iter_mut().find(|e| e.id == id) else {
            return false;
        };
        let same = e
            .size
            .is_some_and(|(ow, oh)| (ow - w).abs() < 0.5 && (oh - h).abs() < 0.5);
        if same {
            return false;
        }
        e.size = Some((w, h));
        self.relayout(now);
        true
    }

    /// New targets: each visible toast below the newer ones (leaving ones keep their place).
    fn relayout(&mut self, now: u64) {
        let heights: Vec<f32> = self
            .entries
            .iter()
            .filter(|e| !e.leaving())
            .map(Entry::height)
            .collect();
        let targets = stack_offsets(&heights, GAP);
        let mut i = 0;
        for e in self.entries.iter_mut() {
            if e.leaving() {
                continue;
            }
            let target = targets[i];
            i += 1;
            if !e.placed {
                e.placed = true;
                e.y_from = target;
                e.y_to = target;
                e.y_at = now;
            } else if (target - e.y_to).abs() > 0.1 {
                e.y_from = e.offset(now);
                e.y_to = target;
                e.y_at = now;
            }
        }
    }

    /// Something moves at `now` (frames needed).
    pub fn animating(&self, now: u64) -> bool {
        self.entries.iter().any(|e| {
            e.leaving()
                || now < e.created_at + ENTER_MS
                || now < e.y_at + REFLOW_MS
                || now < e.kind_at + ICON_MS
                || e.kind == ToastKind::Loading
        })
    }

    /// Next expiry while not paused.
    pub fn next_deadline(&self) -> Option<u64> {
        if self.paused_since.is_some() {
            return None;
        }
        self.entries
            .iter()
            .filter(|e| !e.leaving())
            .filter_map(|e| e.deadline)
            .min()
    }
}

/// Distance of each toast from the anchor edge: heights stacked with `gap` between them.
pub fn stack_offsets(heights: &[f32], gap: f32) -> Vec<f32> {
    let mut acc = 0.0;
    heights
        .iter()
        .map(|h| {
            let y = acc;
            acc += h + gap;
            y
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Easing
// ---------------------------------------------------------------------------------------------

/// 0..=1 progress of `now` in an animation starting at `start`.
pub fn progress(now: u64, start: u64, duration: u64) -> f32 {
    if duration == 0 || now >= start + duration {
        1.0
    } else if now <= start {
        0.0
    } else {
        (now - start) as f32 / duration as f32
    }
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// CSS `cubic-bezier(x1, y1, x2, y2)` at time `t` (0..=1). y may overshoot.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let coord = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let deriv = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    // Solve x(s) = t: Newton, then bisection when the slope is flat.
    let mut s = t;
    for _ in 0..8 {
        let dx = coord(x1, x2, s) - t;
        if dx.abs() < 1e-5 {
            return coord(y1, y2, s);
        }
        let d = deriv(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - dx / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    s = t;
    for _ in 0..30 {
        let x = coord(x1, x2, s);
        if (x - t).abs() < 1e-5 {
            break;
        }
        if x < t {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) / 2.0;
    }
    coord(y1, y2, s)
}

/// react-hot-toast's enter curve.
pub fn enter_curve(t: f32) -> f32 {
    cubic_bezier(0.21, 1.02, 0.73, 1.0, t)
}

/// react-hot-toast's exit curve.
pub fn exit_curve(t: f32) -> f32 {
    cubic_bezier(0.06, 0.71, 0.55, 1.0, t)
}

/// Pop of the success / error disc.
fn pop_curve(t: f32) -> f32 {
    cubic_bezier(0.175, 0.885, 0.32, 1.275, t)
}

/// How a toast is drawn at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub opacity: f32,
    /// Vertical shift in px (positive = down).
    pub dy: f32,
    pub scale: f32,
}

impl Look {
    pub const REST: Look = Look {
        opacity: 1.0,
        dy: 0.0,
        scale: 1.0,
    };
}

/// Enter: from (−20 px, ×0.6, 50 % opacity) to rest. `dir`: −1 for a top stack (comes from
/// above), +1 for a bottom stack. `reduced`: fade only.
pub fn enter_look(age_ms: u64, dir: f32, reduced: bool) -> Look {
    let t = progress(age_ms, 0, ENTER_MS);
    if reduced {
        return Look {
            opacity: t,
            ..Look::REST
        };
    }
    let e = enter_curve(t);
    Look {
        opacity: (0.5 + 0.5 * t * 2.0).min(1.0),
        dy: dir * 20.0 * (1.0 - e),
        scale: 0.6 + 0.4 * e,
    }
}

/// Exit: to (−24 px, ×0.9, transparent).
pub fn exit_look(age_ms: u64, dir: f32, reduced: bool) -> Look {
    let t = progress(age_ms, 0, EXIT_MS);
    if reduced {
        return Look {
            opacity: 1.0 - t,
            ..Look::REST
        };
    }
    let e = exit_curve(t);
    Look {
        opacity: (1.0 - e).max(0.0),
        dy: dir * 24.0 * e,
        scale: 1.0 - 0.1 * e,
    }
}

/// The look of `entry` at `now`.
pub fn look_of<A>(entry: &Entry<A>, now: u64, dir: f32, reduced: bool) -> Look {
    match entry.leaving_at {
        Some(at) => exit_look(now.saturating_sub(at), dir, reduced),
        None => enter_look(now.saturating_sub(entry.created_at), dir, reduced),
    }
}

/// The first `fraction` (0..=1, by length) of a polyline.
pub fn polyline_prefix(points: &[(f32, f32)], fraction: f32) -> Vec<(f32, f32)> {
    let Some(&first) = points.first() else {
        return Vec::new();
    };
    let lens: Vec<f32> = points
        .windows(2)
        .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
        .collect();
    let total: f32 = lens.iter().sum();
    let mut left = total * fraction.clamp(0.0, 1.0);
    let mut out = vec![first];
    for (w, len) in points.windows(2).zip(lens) {
        if left <= 0.0 {
            break;
        }
        if left >= len {
            out.push(w[1]);
            left -= len;
        } else {
            let k = left / len;
            out.push((w[0].0 + (w[1].0 - w[0].0) * k, w[0].1 + (w[1].1 - w[0].1) * k));
            left = 0.0;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Toaster entity
// ---------------------------------------------------------------------------------------------

/// What a toast's action button runs.
pub type ToastHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// The optional button of a toast ("Annuler", "Redémarrer" …). The toast is dismissed after the
/// handler runs.
#[derive(Clone)]
pub struct ToastAction {
    pub label: SharedString,
    pub handler: ToastHandler,
}

impl std::fmt::Debug for ToastAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToastAction")
            .field("label", &self.label)
            .finish()
    }
}

/// The buttons and behaviour of one toast (the payload of a [`Toaster`]'s queue).
#[derive(Debug, Clone)]
pub struct ToastExtras {
    /// Buttons, left to right (at most two fit).
    pub actions: Vec<ToastAction>,
    /// A click on the card or its close badge dismisses it (false: only an update or
    /// [`Toaster::dismiss`] removes it).
    pub dismissible: bool,
}

impl Default for ToastExtras {
    fn default() -> Self {
        Self {
            actions: Vec::new(),
            dismissible: true,
        }
    }
}

/// A toast to show: kind, message, optional actions and duration.
#[derive(Debug, Clone)]
pub struct ToastOptions {
    pub kind: ToastKind,
    pub message: SharedString,
    pub actions: Vec<ToastAction>,
    /// `None` = the kind's default; `Some(None)` = until dismissed.
    pub duration: Option<Option<u64>>,
    pub dismissible: bool,
}

impl ToastOptions {
    pub fn new(kind: ToastKind, message: impl Into<SharedString>) -> Self {
        Self {
            kind,
            message: message.into(),
            actions: Vec::new(),
            duration: None,
            dismissible: true,
        }
    }

    /// Add a button (the toast is dismissed after its handler runs).
    pub fn action(
        mut self,
        label: impl Into<SharedString>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.actions.push(ToastAction {
            label: label.into(),
            handler: Rc::new(handler),
        });
        self
    }

    /// No close badge, and a click on the card does not dismiss it.
    pub fn persistent(mut self) -> Self {
        self.dismissible = false;
        self
    }

    fn extras(&self) -> ToastExtras {
        ToastExtras {
            actions: self.actions.clone(),
            dismissible: self.dismissible,
        }
    }

    pub fn duration(mut self, ms: u64) -> Self {
        self.duration = Some(Some(ms));
        self
    }

    /// Stays until dismissed or updated.
    pub fn sticky(mut self) -> Self {
        self.duration = Some(None);
        self
    }
}

/// A toast stack rendered as an overlay layer of its host window. See the module docs.
pub struct Toaster {
    queue: ToastQueue<ToastExtras>,
    base: Instant,
    position: ToastPosition,
    margin: f32,
    theme: Theme,
    reduced_motion: bool,
    hovered: Option<ToastId>,
    /// Timer to the next expiry (deadline it was set for).
    wake: Option<(u64, Task<()>)>,
}

impl Toaster {
    pub fn new(theme: Theme) -> Self {
        Self {
            queue: ToastQueue::new(),
            base: Instant::now(),
            position: ToastPosition::default(),
            margin: EDGE_MARGIN,
            theme,
            reduced_motion: crate::window::reduced_motion(),
            hovered: None,
            wake: None,
        }
    }

    pub fn with_position(mut self, position: ToastPosition) -> Self {
        self.position = position;
        self
    }

    /// Distance of the stack from its window edge (default [`EDGE_MARGIN`]).
    pub fn with_margin(mut self, margin: f32) -> Self {
        self.margin = margin;
        self
    }

    pub fn set_position(&mut self, position: ToastPosition, cx: &mut Context<Self>) {
        self.position = position;
        cx.notify();
    }

    /// The host's current theme (call each frame; cheap).
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    pub fn queue(&self) -> &ToastQueue<ToastExtras> {
        &self.queue
    }

    /// Where the stack sits and its distance from the window edge.
    pub fn placement(&self) -> (ToastPosition, f32) {
        (self.position, self.margin)
    }

    fn now(&self) -> u64 {
        self.base.elapsed().as_millis() as u64
    }

    pub fn show(&mut self, options: ToastOptions, cx: &mut Context<Self>) -> ToastId {
        let now = self.now();
        let extras = options.extras();
        let id = self.queue.push(
            options.kind,
            options.message,
            Some(extras),
            options.duration,
            now,
        );
        log::debug!("toast {id} shown");
        cx.notify();
        id
    }

    pub fn success(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) -> ToastId {
        self.show(ToastOptions::new(ToastKind::Success, msg), cx)
    }

    pub fn error(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) -> ToastId {
        self.show(ToastOptions::new(ToastKind::Error, msg), cx)
    }

    pub fn loading(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) -> ToastId {
        self.show(ToastOptions::new(ToastKind::Loading, msg), cx)
    }

    pub fn blank(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) -> ToastId {
        self.show(ToastOptions::new(ToastKind::Blank, msg), cx)
    }

    pub fn custom(
        &mut self,
        icon: impl Into<SharedString>,
        msg: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> ToastId {
        self.show(ToastOptions::new(ToastKind::Custom(icon.into()), msg), cx)
    }

    /// Change a toast in place (promise pattern). False when it is gone.
    pub fn update(
        &mut self,
        id: ToastId,
        kind: ToastKind,
        msg: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.update_with(id, kind, msg, None, cx)
    }

    /// [`Toaster::update`] with an action button.
    pub fn update_with(
        &mut self,
        id: ToastId,
        kind: ToastKind,
        msg: impl Into<SharedString>,
        action: Option<ToastAction>,
        cx: &mut Context<Self>,
    ) -> bool {
        let now = self.now();
        let extras = ToastExtras {
            actions: action.into_iter().collect(),
            dismissible: true,
        };
        let done = self.queue.update(id, kind, msg, Some(extras), now);
        cx.notify();
        done
    }

    /// Replace a toast's whole content in place (kind, text, buttons, duration). False when it
    /// is gone or leaving.
    pub fn replace(&mut self, id: ToastId, options: ToastOptions, cx: &mut Context<Self>) -> bool {
        let now = self.now();
        let extras = options.extras();
        let done = self.queue.update_full(
            id,
            options.kind,
            options.message,
            Some(extras),
            options.duration,
            now,
        );
        cx.notify();
        done
    }

    /// Update toast `id` in place, or show a new one when it is gone. Returns the id shown.
    pub fn show_or_replace(
        &mut self,
        id: Option<ToastId>,
        options: ToastOptions,
        cx: &mut Context<Self>,
    ) -> ToastId {
        match id {
            Some(id) if self.is_shown(id) && self.replace(id, options.clone(), cx) => id,
            _ => self.show(options, cx),
        }
    }

    /// Restart a toast's timer with its kind's default duration. False when it is gone.
    pub fn refresh(&mut self, id: ToastId, cx: &mut Context<Self>) -> bool {
        let now = self.now();
        let duration = self.queue.get(id).and_then(|e| e.kind.default_duration());
        let done = self.queue.extend(id, duration, now);
        cx.notify();
        done
    }

    /// The toast is still on screen (not leaving).
    pub fn is_shown(&self, id: ToastId) -> bool {
        self.queue.get(id).is_some_and(|e| !e.leaving())
    }

    pub fn dismiss(&mut self, id: ToastId, cx: &mut Context<Self>) {
        let now = self.now();
        if self.queue.dismiss(id, now) {
            if self.hovered == Some(id) {
                self.hovered = None;
                self.queue.resume(now);
            }
            cx.notify();
        }
    }

    pub fn dismiss_all(&mut self, cx: &mut Context<Self>) {
        let now = self.now();
        self.queue.dismiss_all(now);
        self.hovered = None;
        self.queue.resume(now);
        cx.notify();
    }

    fn set_hovered(&mut self, id: ToastId, hovered: bool, cx: &mut Context<Self>) {
        let now = self.now();
        if hovered {
            self.hovered = Some(id);
            self.queue.pause(now);
        } else if self.hovered == Some(id) {
            self.hovered = None;
            self.queue.resume(now);
        }
        cx.notify();
    }

    fn measured(&mut self, id: ToastId, w: f32, h: f32, cx: &mut Context<Self>) {
        let now = self.now();
        if self.queue.set_size(id, w, h, now) {
            cx.notify();
        }
    }

    /// One timer to the next expiry (none while animating: frames tick the queue).
    fn schedule(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.queue.next_deadline() else {
            self.wake = None;
            return;
        };
        if self.wake.as_ref().is_some_and(|(d, _)| *d == deadline) {
            return;
        }
        let wait = deadline.saturating_sub(self.now());
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(wait))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.wake = None;
                cx.notify();
            });
        });
        self.wake = Some((deadline, task));
    }

    fn render_card(
        &self,
        entry: &Entry<ToastExtras>,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let id = entry.id;
        let dir = self.position.dir();
        let look = look_of(entry, now, dir, self.reduced_motion);
        let (w, h) = entry.size.unwrap_or(ESTIMATE);
        let s = look.scale;
        let (ix, iy) = ((1.0 - s) * w / 2.0, (1.0 - s) * h / 2.0);
        // Text and icon fade in with the growing card (the card itself cannot be scaled).
        let content = ((s - 0.6) / 0.4).clamp(0.0, 1.0).max(if entry.leaving() {
            1.0
        } else {
            0.0
        });
        let shadow = if t.dark {
            vec![BoxShadow {
                color: hsla(0., 0., 0., 0.35),
                offset: point(px(0.), px(3.)),
                blur_radius: px(8.),
                spread_radius: px(0.),
            }]
        } else {
            vec![
                BoxShadow {
                    color: hsla(0., 0., 0., 0.10),
                    offset: point(px(0.), px(3.)),
                    blur_radius: px(10.),
                    spread_radius: px(0.),
                },
                BoxShadow {
                    color: hsla(0., 0., 0., 0.05),
                    offset: point(px(0.), px(3.)),
                    blur_radius: px(3.),
                    spread_radius: px(0.),
                },
            ]
        };
        let weak = cx.entity().downgrade();
        let hovered = self.hovered == Some(id);
        let icon = toast_icon(
            &entry.kind,
            now.saturating_sub(entry.kind_at),
            now,
            &t,
            self.reduced_motion,
        );

        let extras = entry.action.clone().unwrap_or_default();
        let dismissible = extras.dismissible;
        let actions = extras.actions.into_iter().enumerate().map(|(i, a)| {
            let weak = weak.clone();
            div()
                .id(("toast-action", id * 8 + i as u64))
                .flex_none()
                .px(px(8.))
                .py(px(3.))
                .rounded(px(6.))
                .border_1()
                .border_color(t.border)
                .bg(t.background)
                .hover(|d| d.bg(t.muted))
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.foreground)
                .child(a.label.clone())
                .on_click(move |_: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    (a.handler)(window, cx);
                    let _ = weak.update(cx, |this, cx| this.dismiss(id, cx));
                })
        });
        let close = {
            let weak = weak.clone();
            // A small round badge over the top-left corner, only while hovered (it takes no
            // room in the card, so hovering never re-wraps the text).
            div()
                .id(("toast-close", id))
                .absolute()
                .top(px(iy - 6.))
                .left(px(ix - 6.))
                .size(px(18.))
                .rounded_full()
                .border_1()
                .border_color(t.border)
                .bg(t.popover)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.))
                .text_color(t.muted_foreground)
                .hover(|d| d.bg(t.muted).text_color(t.foreground))
                .child("✕")
                .on_click(move |_: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    let _ = weak.update(cx, |this, cx| this.dismiss(id, cx));
                })
        };

        let on_hover = {
            let weak = weak.clone();
            move |hovered: &bool, _: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| this.set_hovered(id, *hovered, cx));
            }
        };
        let on_click = {
            let weak = weak.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                if dismissible {
                    let _ = weak.update(cx, |this, cx| this.dismiss(id, cx));
                }
            }
        };

        div()
            .id(("toast", id))
            .relative()
            .max_w(px(MAX_WIDTH))
            .min_h(px(36.))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(8.))
            .opacity(look.opacity.clamp(0.0, 1.0))
            .occlude()
            .on_hover(on_hover)
            .on_click(on_click)
            // The card's box, scaled about its centre.
            .child(
                div()
                    .absolute()
                    .left(px(ix))
                    .right(px(ix))
                    .top(px(iy))
                    .bottom(px(iy))
                    .rounded(px(8.))
                    .bg(t.popover)
                    .border_1()
                    .border_color(t.border)
                    .shadow(shadow),
            )
            .children(icon.map(|i| div().opacity(content).child(i)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .px(px(2.))
                    .opacity(content)
                    .text_size(px(14.))
                    .line_height(px(19.))
                    .font_family(t.fonts.text)
                    .text_color(t.foreground)
                    .child(entry.message.clone()),
            )
            .children(actions.map(|a| a.opacity(content)))
            .when(dismissible && hovered && !entry.leaving(), |d| d.child(close))
            .into_any_element()
    }
}

impl Render for Toaster {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.now();
        self.queue.tick(now);
        if self.queue.animating(now) {
            window.request_animation_frame();
        }
        self.schedule(cx);

        let dir = self.position.dir();
        let margin = self.margin;
        let position = self.position;
        let weak = cx.entity().downgrade();
        let mut layer = div().absolute().top_0().left_0().right_0().bottom_0();
        // Oldest first: newer toasts paint over leaving ones.
        let entries: Vec<Entry<ToastExtras>> = self.queue.entries().iter().rev().cloned().collect();
        for entry in &entries {
            let look = look_of(entry, now, dir, self.reduced_motion);
            let offset = entry.offset(now);
            let card = self.render_card(entry, now, cx);
            let id = entry.id;
            let weak = weak.clone();
            let row = div()
                .absolute()
                .flex()
                .on_children_prepainted(move |bounds, _, cx| {
                    if let Some(b) = bounds.first() {
                        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
                        let _ = weak.update(cx, |this, cx| this.measured(id, w, h, cx));
                    }
                })
                .child(card);
            let row = match position {
                ToastPosition::TopCenter => row
                    .left_0()
                    .right_0()
                    .justify_center()
                    .top(px(margin + offset + look.dy)),
                ToastPosition::BottomRight => row
                    .right(px(margin))
                    .bottom(px(margin + offset - look.dy)),
            };
            layer = layer.child(row);
        }
        layer
    }
}

/// The icon of a kind (20 px box), `age` = ms since the kind was set.
fn toast_icon(
    kind: &ToastKind,
    age: u64,
    now: u64,
    t: &Theme,
    reduced: bool,
) -> Option<AnyElement> {
    const BOX: f32 = 20.0;
    let white = hsla(0., 0., 1., 1.);
    let el = match kind {
        ToastKind::Blank => return None,
        ToastKind::Custom(icon) => div()
            .flex_none()
            .size(px(BOX))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(16.))
            .child(icon.clone())
            .into_any_element(),
        ToastKind::Loading => {
            let track = t.border;
            let arc = t.foreground.opacity(0.75);
            // One turn per second.
            let start = (now % 1000) as f32 * 0.36;
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    let c = b.center();
                    paint_arc(window, c, 6.0, 2.0, 0.0, 360.0, track, Cap::Butt);
                    paint_arc(window, c, 6.0, 2.0, start, 90.0, arc, Cap::Round);
                },
            )
            .size(px(BOX))
            .flex_none()
            .into_any_element()
        }
        ToastKind::Success | ToastKind::Error => {
            let success = *kind == ToastKind::Success;
            let fill = if success { t.success } else { t.critical };
            let (pop, draw) = if reduced {
                (1.0, 1.0)
            } else {
                (
                    pop_curve(progress(age, 100, 300)),
                    ease_out(progress(age, 200, 200)),
                )
            };
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    let c = b.center();
                    paint_disc(window, c, 10.0 * pop.max(0.0), fill);
                    if draw <= 0.0 {
                        return;
                    }
                    let lines: Vec<Vec<(f32, f32)>> = if success {
                        vec![polyline_prefix(&[(-4.5, 0.2), (-1.4, 3.3), (4.6, -3.0)], draw)]
                    } else {
                        let k = 3.4;
                        vec![
                            polyline_prefix(&[(-k, -k), (k, k)], draw),
                            polyline_prefix(&[(k, -k), (-k, k)], (draw * 1.2 - 0.2).max(0.0)),
                        ]
                    };
                    for line in lines {
                        stroke(window, c, &line, 2.0, white);
                    }
                },
            )
            .size(px(BOX))
            .flex_none()
            .into_any_element()
        }
    };
    Some(el)
}

/// A polyline stroke with round ends and joints (dots), relative to `c`.
fn stroke(window: &mut Window, c: Point<Pixels>, pts: &[(f32, f32)], width: f32, color: Hsla) {
    if pts.len() < 2 {
        return;
    }
    let at = |p: (f32, f32)| point(c.x + px(p.0), c.y + px(p.1));
    let mut b = PathBuilder::stroke(px(width));
    b.move_to(at(pts[0]));
    for p in &pts[1..] {
        b.line_to(at(*p));
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
    for p in pts {
        paint_disc(window, at(*p), width / 2.0, color);
    }
}

// ---------------------------------------------------------------------------------------------
// App-level host
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct ToastHost(Option<WeakEntity<Toaster>>);

impl Global for ToastHost {}

/// Make `toaster` the target of the free functions (the window that shows app toasts).
pub fn set_host(toaster: &Entity<Toaster>, cx: &mut App) {
    cx.set_global(ToastHost(Some(toaster.downgrade())));
}

/// Forget `toaster` as the host (its window closes). No-op when another one is the host.
pub fn clear_host(toaster: gpui::EntityId, cx: &mut App) {
    let host = cx.default_global::<ToastHost>();
    if host.0.as_ref().is_some_and(|w| w.entity_id() == toaster) {
        host.0 = None;
    }
}

/// The current host, if its window is open.
pub fn host(cx: &mut App) -> Option<Entity<Toaster>> {
    cx.default_global::<ToastHost>().0.as_ref()?.upgrade()
}

fn with_host<R>(cx: &mut App, f: impl FnOnce(&mut Toaster, &mut Context<Toaster>) -> R) -> Option<R> {
    let toaster = host(cx)?;
    Some(toaster.update(cx, f))
}

pub fn show(options: ToastOptions, cx: &mut App) -> Option<ToastId> {
    with_host(cx, |t, cx| t.show(options, cx))
}

pub fn success(msg: impl Into<SharedString>, cx: &mut App) -> Option<ToastId> {
    with_host(cx, |t, cx| t.success(msg, cx))
}

pub fn error(msg: impl Into<SharedString>, cx: &mut App) -> Option<ToastId> {
    with_host(cx, |t, cx| t.error(msg, cx))
}

pub fn loading(msg: impl Into<SharedString>, cx: &mut App) -> Option<ToastId> {
    with_host(cx, |t, cx| t.loading(msg, cx))
}

pub fn blank(msg: impl Into<SharedString>, cx: &mut App) -> Option<ToastId> {
    with_host(cx, |t, cx| t.blank(msg, cx))
}

pub fn custom(
    icon: impl Into<SharedString>,
    msg: impl Into<SharedString>,
    cx: &mut App,
) -> Option<ToastId> {
    with_host(cx, |t, cx| t.custom(icon, msg, cx))
}

pub fn update(id: ToastId, kind: ToastKind, msg: impl Into<SharedString>, cx: &mut App) -> bool {
    with_host(cx, |t, cx| t.update(id, kind, msg, cx)).unwrap_or(false)
}

pub fn dismiss(id: ToastId, cx: &mut App) {
    with_host(cx, |t, cx| t.dismiss(id, cx));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q() -> ToastQueue<()> {
        ToastQueue::new()
    }

    #[test]
    fn durations_per_kind() {
        assert_eq!(ToastKind::Success.default_duration(), Some(2000));
        assert_eq!(ToastKind::Error.default_duration(), Some(4000));
        assert_eq!(ToastKind::Loading.default_duration(), None);
        assert_eq!(ToastKind::Blank.default_duration(), Some(4000));
        assert_eq!(ToastKind::Custom("🔥".into()).default_duration(), Some(4000));
    }

    #[test]
    fn newest_first_and_expiry() {
        let mut q = q();
        let a = q.push(ToastKind::Success, "a", None, None, 0);
        let b = q.push(ToastKind::Error, "b", None, None, 100);
        assert_eq!(q.entries()[0].id, b);
        assert_eq!(q.next_deadline(), Some(2000));
        assert!(!q.tick(1999));
        assert!(q.tick(2000));
        assert!(q.get(a).unwrap().leaving());
        // Removed once the exit animation ran.
        q.tick(2000 + EXIT_MS);
        assert!(q.get(a).is_none());
        assert!(q.get(b).is_some());
        q.tick(4100);
        q.tick(4100 + EXIT_MS);
        assert!(q.is_empty());
    }

    #[test]
    fn loading_is_sticky_until_updated() {
        let mut q = q();
        let id = q.push(ToastKind::Loading, "…", None, None, 0);
        assert_eq!(q.next_deadline(), None);
        q.tick(60_000);
        assert!(!q.get(id).unwrap().leaving());
        assert!(q.update(id, ToastKind::Success, "done", None, 60_000));
        let e = q.get(id).unwrap();
        assert_eq!(e.kind, ToastKind::Success);
        assert_eq!(e.message.as_ref(), "done");
        assert_eq!(e.kind_at, 60_000);
        assert_eq!(q.next_deadline(), Some(62_000));
        // A leaving toast cannot be updated.
        q.dismiss(id, 61_000);
        assert!(!q.update(id, ToastKind::Error, "x", None, 61_000));
    }

    #[test]
    fn update_full_takes_an_explicit_duration() {
        let mut q = q();
        let id = q.push(ToastKind::Loading, "…", None, None, 0);
        assert!(q.update_full(id, ToastKind::Blank, "offer", None, Some(None), 10));
        assert_eq!(q.get(id).unwrap().deadline, None, "sticky");
        assert!(q.update_full(id, ToastKind::Error, "x", None, Some(Some(500)), 20));
        assert_eq!(q.get(id).unwrap().deadline, Some(520));
        assert!(q.update_full(id, ToastKind::Success, "y", None, None, 30));
        assert_eq!(q.get(id).unwrap().deadline, Some(2030), "kind default");
    }

    #[test]
    fn options_buttons_and_persistence() {
        let o = ToastOptions::new(ToastKind::Blank, "x")
            .action("A", |_, _| {})
            .action("B", |_, _| {})
            .persistent()
            .sticky();
        let e = o.extras();
        assert_eq!(e.actions.len(), 2);
        assert_eq!(e.actions[1].label.as_ref(), "B");
        assert!(!e.dismissible);
        assert_eq!(o.duration, Some(None));
        assert!(ToastExtras::default().dismissible);
    }

    #[test]
    fn explicit_and_sticky_durations() {
        let mut q = q();
        let a = q.push(ToastKind::Success, "a", None, Some(Some(6000)), 0);
        let b = q.push(ToastKind::Error, "b", None, Some(None), 0);
        assert_eq!(q.get(a).unwrap().deadline, Some(6000));
        assert_eq!(q.get(b).unwrap().deadline, None);
        assert!(q.extend(a, Some(2000), 5000));
        assert_eq!(q.get(a).unwrap().deadline, Some(7000));
    }

    #[test]
    fn max_visible_dismisses_the_oldest() {
        let mut q = q();
        let ids: Vec<_> = (0..4)
            .map(|i| q.push(ToastKind::Blank, format!("{i}"), None, None, i * 10))
            .collect();
        assert_eq!(q.visible_count(), MAX_VISIBLE);
        assert!(q.get(ids[0]).unwrap().leaving());
        assert!(!q.get(ids[1]).unwrap().leaving());
    }

    #[test]
    fn pause_freezes_timers() {
        let mut q = q();
        let id = q.push(ToastKind::Success, "a", None, None, 0);
        q.pause(1000);
        assert!(q.is_paused());
        assert_eq!(q.next_deadline(), None);
        assert!(!q.tick(5000));
        // A toast added during the pause gets its full duration after the resume.
        let b = q.push(ToastKind::Success, "b", None, None, 3000);
        q.resume(6000);
        assert_eq!(q.get(id).unwrap().deadline, Some(7000));
        assert_eq!(q.get(b).unwrap().deadline, Some(8000));
        assert!(!q.tick(6999));
        assert!(q.tick(7000));
        assert!(q.get(id).unwrap().leaving());
    }

    #[test]
    fn dismiss_and_reflow() {
        let mut q = q();
        let a = q.push(ToastKind::Blank, "a", None, None, 0);
        q.set_size(a, 200.0, 40.0, 0);
        let b = q.push(ToastKind::Blank, "b", None, None, 10);
        q.set_size(b, 200.0, 50.0, 10);
        // b on top, a below it.
        assert_eq!(q.get(b).unwrap().target(), 0.0);
        assert_eq!(q.get(a).unwrap().target(), 50.0 + GAP);
        // a slides there smoothly (it started at 0).
        let mid = q.get(a).unwrap().offset(10 + REFLOW_MS / 2);
        assert!(mid > 0.0 && mid < 58.0, "{mid}");
        assert_eq!(q.get(a).unwrap().offset(10 + REFLOW_MS), 58.0);
        // b leaves: a moves back up, b keeps its place while fading.
        assert!(q.dismiss(b, 1000));
        assert!(!q.dismiss(b, 1001));
        assert_eq!(q.get(a).unwrap().target(), 0.0);
        assert_eq!(q.get(b).unwrap().target(), 0.0);
        assert!(q.animating(1100));
    }

    #[test]
    fn animating_only_while_moving() {
        let mut q = q();
        assert!(!q.animating(0));
        let id = q.push(ToastKind::Success, "a", None, None, 0);
        assert!(q.animating(100));
        assert!(!q.animating(ICON_MS.max(ENTER_MS) + 1));
        q.update(id, ToastKind::Loading, "l", None, 1000);
        assert!(q.animating(5000)); // spinner
    }

    #[test]
    fn offsets_stack_with_gap() {
        assert_eq!(stack_offsets(&[40.0, 30.0, 20.0], 8.0), vec![0.0, 48.0, 86.0]);
        assert!(stack_offsets(&[], 8.0).is_empty());
    }

    #[test]
    fn bezier_endpoints_and_overshoot() {
        assert_eq!(enter_curve(0.0), 0.0);
        assert_eq!(enter_curve(1.0), 1.0);
        let max = (1..100)
            .map(|i| enter_curve(i as f32 / 100.0))
            .fold(0.0f32, f32::max);
        assert!(max > 1.0 && max < 1.05, "{max}");
        // Linear control points give the identity.
        let v = cubic_bezier(1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0, 0.3);
        assert!((v - 0.3).abs() < 1e-3);
        // Monotonic exit curve.
        let mut last = 0.0;
        for i in 0..=20 {
            let v = exit_curve(i as f32 / 20.0);
            assert!(v >= last - 1e-4);
            last = v;
        }
    }

    #[test]
    fn looks() {
        let start = enter_look(0, -1.0, false);
        assert_eq!(start.scale, 0.6);
        assert_eq!(start.dy, -20.0);
        assert_eq!(start.opacity, 0.5);
        assert_eq!(enter_look(ENTER_MS, -1.0, false), Look::REST);
        let end = exit_look(EXIT_MS, -1.0, false);
        assert_eq!(end.opacity, 0.0);
        assert!((end.scale - 0.9).abs() < 1e-4);
        assert!((end.dy + 24.0).abs() < 1e-3);
        // Bottom stacks come from below.
        assert_eq!(enter_look(0, 1.0, false).dy, 20.0);
        // Reduced motion: fade only.
        let r = enter_look(ENTER_MS / 2, -1.0, true);
        assert_eq!((r.dy, r.scale), (0.0, 1.0));
        assert!(r.opacity > 0.4 && r.opacity < 0.6);
        assert_eq!(exit_look(0, -1.0, true).opacity, 1.0);
    }

    #[test]
    fn polyline_prefix_by_length() {
        let pts = [(0.0, 0.0), (3.0, 0.0), (3.0, 4.0)]; // lengths 3 + 4
        assert_eq!(polyline_prefix(&pts, 0.0), vec![(0.0, 0.0)]);
        assert_eq!(polyline_prefix(&pts, 1.0), pts.to_vec());
        let half = polyline_prefix(&pts, 0.5); // 3.5 along
        assert_eq!(half.len(), 3);
        assert!((half[2].1 - 0.5).abs() < 1e-4);
        assert!(polyline_prefix(&[], 0.5).is_empty());
    }

    #[test]
    fn progress_bounds() {
        assert_eq!(progress(0, 100, 200), 0.0);
        assert_eq!(progress(200, 100, 200), 0.5);
        assert_eq!(progress(400, 100, 200), 1.0);
        assert_eq!(progress(5, 0, 0), 1.0);
    }
}
