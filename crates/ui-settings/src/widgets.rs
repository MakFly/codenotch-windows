//! Private controls drawn with gpui primitives in the shadcn/ui style (v4, "neutral"): Switch
//! 36 × 20, Select (outline trigger + popover list with a check mark), Button (default /
//! outline / ghost / destructive), "…" dropdown menu, cards, navigation glyphs. Colours come
//! from [`Theme`] (both appearances); the window keeps the Windows layout (caption buttons,
//! left navigation, a card per setting).
//!
//! Why not gpui-component's `Switch`/`Button`/`Select`: in 0.5.0 the switch takes no keyboard
//! focus and all of them use their own sizes/colours. These take focus (Tab), react to
//! Enter/Space (gpui turns them into clicks on a focused element) and draw the shadcn focus
//! ring (3 px, `ring` at 50 %) only after keyboard navigation
//! ([`codenotch_ui_kit::controls::focus_visible`]).

use codenotch_ui_kit::controls::focus_visible;
use codenotch_ui_kit::Theme;
use gpui::prelude::FluentBuilder;
use gpui::{
    anchored, deferred, div, px, App, AppContext, Div, ElementId, FocusHandle, FontWeight, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, RenderOnce, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Window,
};
use std::rc::Rc;

pub type OnBool = Rc<dyn Fn(bool, &mut Window, &mut App)>;
pub type OnIndex = Rc<dyn Fn(usize, &mut Window, &mut App)>;
pub type OnClick = Rc<dyn Fn(&mut Window, &mut App)>;

/// Body text size (`text-sm`).
pub const FONT: f32 = 14.0;
/// Description text size under a setting's label.
pub const DESC: f32 = 13.0;
/// Small text (statuses, captions, chart labels).
pub const CAPTION: f32 = 12.0;
/// Corner radius of controls (buttons, inputs, selects, popovers, nav items).
pub const RADIUS: f32 = 8.0;
/// Corner radius of menu / list items inside a popover.
pub const ITEM_RADIUS: f32 = 6.0;
/// Corner radius of cards (`rounded-xl`).
pub const CARD_RADIUS: f32 = 12.0;
/// Standard control height (`h-9`).
pub const CONTROL_H: f32 = 36.0;
/// Small control height (`h-8`): icon buttons, menu items.
pub const CONTROL_H_SM: f32 = 32.0;
/// Select trigger width range (it sizes to its longest choice in between).
pub const COMBO_MIN_W: f32 = 120.0;
pub const COMBO_MAX_W: f32 = 240.0;
/// Width of the focus ring (`ring-[3px]`).
const RING_W: f32 = 3.0;

/// Segoe Fluent Icons code points.
pub mod glyph {
    pub const ACCOUNTS: &str = "\u{E77B}";
    pub const APPEARANCE: &str = "\u{E790}";
    pub const NOTIFICATIONS: &str = "\u{EA8F}";
    pub const ANALYTICS: &str = "\u{E9D2}";
    pub const GENERAL: &str = "\u{E713}";
    pub const QUIT: &str = "\u{E7E8}";
    pub const CHEVRON_DOWN: &str = "\u{E70D}";
    pub const CHECK: &str = "\u{E73E}";
    pub const MORE: &str = "\u{E712}";
    pub const MINIMIZE: &str = "\u{E921}";
    pub const MAXIMIZE: &str = "\u{E922}";
    pub const RESTORE: &str = "\u{E923}";
    pub const CLOSE: &str = "\u{E8BB}";
    pub const ERROR: &str = "\u{E783}";
    pub const REFRESH: &str = "\u{E72C}";
}

/// Focus handle kept across frames for an element id.
fn focus_for(id: &ElementId, window: &mut Window, cx: &mut App) -> FocusHandle {
    window
        .use_keyed_state(id.clone(), cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone()
}

/// Keyboard focus visual shown for this handle (focused + last input was keyboard).
fn shows_focus(focus: &FocusHandle, window: &Window, cx: &App) -> bool {
    focus.is_focused(window) && focus_visible(cx)
}

/// Make an id from a prefix and a string key.
pub fn eid(prefix: &str, key: impl std::fmt::Display) -> ElementId {
    ElementId::Name(SharedString::from(format!("{prefix}:{key}")))
}

fn child_id(parent: &ElementId, name: impl Into<SharedString>) -> ElementId {
    ElementId::NamedChild(Box::new(parent.clone()), name.into())
}

/// The shadcn focus ring: 3 px of `ring` at 50 % just outside an element with corner `radius`.
/// An absolutely positioned overlay, so it never changes the element's size (a margin-based
/// ring once made taffy measure flex rows holding such a control as 0 px wide).
pub fn focus_ring(t: &Theme, radius: f32) -> Div {
    div()
        .absolute()
        .top(px(-RING_W))
        .left(px(-RING_W))
        .right(px(-RING_W))
        .bottom(px(-RING_W))
        .border(px(RING_W))
        .rounded(px(radius + RING_W))
        .border_color(t.ring.opacity(0.5))
}

/// Wrap `control` so the focus ring can be drawn around it when `shown`.
fn framed(
    outer: Stateful<Div>,
    t: &Theme,
    shown: bool,
    radius: f32,
    control: impl IntoElement,
) -> Stateful<Div> {
    outer
        .relative()
        .flex()
        .flex_shrink_0()
        .child(control)
        .when(shown, |d| d.child(focus_ring(t, radius)))
}

/// A popover surface (dropdown lists, menus): popover fill, 1 px border, 8 px radius,
/// `shadow-md`, 4 px padding.
fn popover(t: &Theme, id: ElementId) -> Stateful<Div> {
    div()
        .id(id)
        .occlude()
        .flex()
        .flex_col()
        .p(px(4.))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(t.border)
        .bg(t.popover)
        .shadow_md()
        .font_family(t.fonts.text)
        .text_color(t.foreground)
}

/// One item of a popover list: 32 px, 6 px radius, accent fill on hover.
fn popover_item(t: &Theme, id: ElementId) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .flex()
        .items_center()
        .h(px(CONTROL_H_SM))
        .pl(px(8.))
        .rounded(px(ITEM_RADIUS))
        .text_size(px(FONT))
        .whitespace_nowrap()
        .hover(|s| s.bg(t.accent))
}

// ---------------------------------------------------------------------------------------------
// Switch
// ---------------------------------------------------------------------------------------------

/// shadcn Switch: 36 × 20 fully rounded track, 16 px thumb 2 px in. Checked: `primary` track,
/// thumb in the background colour. Unchecked: `input` track, thumb in the background colour
/// (light) or the foreground (dark). No On/Off text.
#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    on: bool,
    disabled: bool,
    theme: Theme,
    on_change: Option<OnBool>,
}

impl Toggle {
    pub fn new(id: impl Into<ElementId>, on: bool, theme: Theme) -> Self {
        Self {
            id: id.into(),
            on,
            disabled: false,
            theme,
            on_change: None,
        }
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn on_change(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

/// Track and thumb colours of a switch.
fn switch_colors(t: &Theme, on: bool) -> (Hsla, Hsla) {
    match (on, t.dark) {
        (true, _) => (t.primary, t.background),
        (false, false) => (t.input, t.background),
        (false, true) => (t.input, t.foreground),
    }
}

impl RenderOnce for Toggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = self.theme;
        let on = self.on;
        let enabled = !self.disabled;
        let focus = focus_for(&self.id, window, cx);
        let shown = enabled && shows_focus(&focus, window, cx);
        let (track_c, thumb_c) = switch_colors(&t, on);
        let track = div()
            .w(px(36.))
            .h(px(20.))
            .rounded_full()
            .relative()
            .flex_shrink_0()
            .bg(track_c)
            .when(t.card_shadow, |d| d.shadow_xs())
            .child(
                div()
                    .absolute()
                    .top(px(2.))
                    .left(px(if on { 18. } else { 2. }))
                    .size(px(16.))
                    .rounded_full()
                    .bg(thumb_c)
                    .when(!t.dark || on, |d| d.shadow_xs()),
            );
        framed(div().id(self.id.clone()), &t, shown, 10., track)
            .when(self.disabled, |d| d.opacity(0.5))
            .when(enabled, |d| {
                d.track_focus(&focus.tab_stop(true)).cursor_pointer()
            })
            .when_some(self.on_change.filter(|_| enabled), |d, f| {
                d.on_click(move |_, window, cx| f(!on, window, cx))
            })
    }
}

// ---------------------------------------------------------------------------------------------
// Select
// ---------------------------------------------------------------------------------------------

/// shadcn Select: an outline trigger (36 px, 8 px radius, `input` border, chevron) with the
/// current choice; the list opens below in a popover, the chosen item marked with a check.
/// Up/Down/Home/End change the choice while it is focused (open or closed); Enter/Space
/// open/close it; Escape closes it.
#[derive(IntoElement)]
pub struct ComboBox {
    id: ElementId,
    labels: Vec<SharedString>,
    selected: Option<usize>,
    disabled: bool,
    theme: Theme,
    on_change: Option<OnIndex>,
}

impl ComboBox {
    pub fn new(
        id: impl Into<ElementId>,
        labels: impl IntoIterator<Item = impl Into<SharedString>>,
        selected: Option<usize>,
        theme: Theme,
    ) -> Self {
        Self {
            id: id.into(),
            labels: labels.into_iter().map(Into::into).collect(),
            selected,
            disabled: false,
            theme,
            on_change: None,
        }
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn on_change(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

/// Whether clicking entry `i` is a change (re-picking the selected entry is not).
pub(crate) fn combo_picks(i: usize, selected: Option<usize>) -> bool {
    selected != Some(i)
}

/// Next index for a navigation key in a list of `n` choices (`None` = key not handled).
pub(crate) fn combo_step(key: &str, current: Option<usize>, n: usize) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let last = n - 1;
    match key {
        "up" | "left" => Some(current.map_or(0, |i| i.saturating_sub(1))),
        "down" | "right" => Some(current.map_or(0, |i| (i + 1).min(last))),
        "home" | "pageup" => Some(0),
        "end" | "pagedown" => Some(last),
        _ => None,
    }
}

impl RenderOnce for ComboBox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = self.theme;
        let enabled = !self.disabled;
        let focus = focus_for(&self.id, window, cx);
        let shown = enabled && shows_focus(&focus, window, cx);
        let open_state = window.use_keyed_state(child_id(&self.id, "open"), cx, |_, _| false);
        let hover_state = window.use_keyed_state(child_id(&self.id, "hover"), cx, |_, _| false);
        let open = enabled && *open_state.read(cx);
        let handler = self.on_change.clone().filter(|_| enabled);
        let current = self
            .selected
            .and_then(|i| self.labels.get(i).cloned())
            .unwrap_or_default();

        let field = div()
            .id(child_id(&self.id, "face"))
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(CONTROL_H))
            // As wide as its longest choice, within 120..240 px.
            .min_w(px(COMBO_MIN_W))
            .max_w(px(COMBO_MAX_W))
            .pl(px(12.))
            .pr(px(10.))
            .rounded(px(RADIUS))
            .border_1()
            .border_color(if shown || open { t.ring } else { t.input })
            .bg(t.control)
            .when(t.card_shadow, |d| d.shadow_xs())
            .text_size(px(FONT))
            .text_color(t.foreground)
            .whitespace_nowrap()
            .when(enabled, |d| d.hover(|s| s.bg(t.control_hover)))
            .child(
                // Grows (basis = widest label) and may shrink under the max width, then
                // the shown label is cut with an ellipsis.
                div().flex_grow().min_w(px(0.)).overflow_hidden().child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().truncate().child(current))
                        .children(
                            self.labels
                                .iter()
                                .map(|l| div().h(px(0.)).overflow_hidden().child(l.clone())),
                        ),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(t.fonts.icons)
                    .text_size(px(12.))
                    .text_color(t.muted_foreground)
                    .child(glyph::CHEVRON_DOWN),
            );

        let menu = open.then(|| {
            let close = open_state.clone();
            let hovered = hover_state.clone();
            let mut list = popover(&t, child_id(&self.id, "menu"))
                .min_w(px(COMBO_MIN_W))
                .on_mouse_down_out(move |_, _, cx| {
                    // A press on the field itself is handled by its click (toggle).
                    if !*hovered.read(cx) {
                        close.update(cx, |open, cx| {
                            *open = false;
                            cx.notify();
                        });
                    }
                });
            let selected_now = self.selected;
            for (i, label) in self.labels.iter().cloned().enumerate() {
                let selected = self.selected == Some(i);
                let close = open_state.clone();
                let handler = handler.clone();
                list = list.child(
                    popover_item(&t, child_id(&self.id, format!("opt{i}")))
                        // Room for the check mark on the right (~ the trigger's width).
                        .pr(px(32.))
                        .cursor_pointer()
                        .when(selected, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .right(px(8.))
                                    .top_0()
                                    .bottom_0()
                                    .flex()
                                    .items_center()
                                    .font_family(t.fonts.icons)
                                    .text_size(px(12.))
                                    .child(glyph::CHECK),
                            )
                        })
                        .on_click(move |_, window, cx| {
                            close.update(cx, |open, cx| {
                                *open = false;
                                cx.notify();
                            });
                            // Re-picking the current entry only closes the menu (no edit,
                            // no SaveConfig, no RefreshAnalytics from the period box).
                            if let Some(f) =
                                handler.as_ref().filter(|_| combo_picks(i, selected_now))
                            {
                                f(i, window, cx);
                            }
                        })
                        .child(label),
                );
            }
            // Left-aligned with the field, 4 px below it.
            div().absolute().left(px(0.)).top(px(CONTROL_H + 4.)).child(
                deferred(anchored().snap_to_window_with_margin(px(8.)).child(list))
                    .with_priority(1),
            )
        });

        let n = self.labels.len();
        let selected = self.selected;
        let toggle = open_state.clone();
        let keys = open_state.clone();
        let key_handler = handler.clone();
        framed(div().id(self.id.clone()), &t, shown, RADIUS, field)
            .relative()
            .when(self.disabled, |d| d.opacity(0.5))
            .when(enabled, |d| {
                let hovered = hover_state.clone();
                d.track_focus(&focus.tab_stop(true))
                    .cursor_pointer()
                    .on_hover(move |h, _, cx| {
                        let h = *h;
                        hovered.update(cx, |v, _| *v = h);
                    })
                    .on_click(move |_, _, cx| {
                        toggle.update(cx, |open, cx| {
                            *open = !*open;
                            cx.notify();
                        });
                    })
                    .on_key_down(move |e: &KeyDownEvent, window, cx| {
                        let key = e.keystroke.key.as_str();
                        if key == "escape" {
                            if *keys.read(cx) {
                                keys.update(cx, |open, cx| {
                                    *open = false;
                                    cx.notify();
                                });
                                cx.stop_propagation();
                            }
                            return;
                        }
                        if e.keystroke.modifiers.modified() {
                            return;
                        }
                        if let Some(next) = combo_step(key, selected, n) {
                            cx.stop_propagation();
                            if Some(next) != selected {
                                if let Some(f) = &key_handler {
                                    f(next, window, cx);
                                }
                            }
                        }
                    })
            })
            .children(menu)
    }
}

// ---------------------------------------------------------------------------------------------
// Toggle group
// ---------------------------------------------------------------------------------------------

/// shadcn ToggleGroup (`variant="outline"`, `type="single"`): one outlined `rounded-md` group
/// (`input` border, `shadow-xs` in light) of items split by 1 px borders; the selected item has
/// the `accent` fill and medium text, the others are plain with an accent hover. Tab focuses
/// the group; Left/Right/Home/End move the choice; a click on the selected item does nothing.
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    labels: Vec<SharedString>,
    selected: Option<usize>,
    small: bool,
    theme: Theme,
    on_change: Option<OnIndex>,
}

impl Segmented {
    pub fn new(
        id: impl Into<ElementId>,
        labels: impl IntoIterator<Item = impl Into<SharedString>>,
        selected: Option<usize>,
        theme: Theme,
    ) -> Self {
        Self {
            id: id.into(),
            labels: labels.into_iter().map(Into::into).collect(),
            selected,
            small: false,
            theme,
            on_change: None,
        }
    }
    /// 32 px tall (`size="sm"`) instead of 36.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }
    pub fn on_change(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = self.theme;
        let focus = focus_for(&self.id, window, cx);
        let shown = shows_focus(&focus, window, cx);
        let h = if self.small { CONTROL_H_SM } else { CONTROL_H };
        let n = self.labels.len();
        let selected = self.selected;
        let inner = RADIUS - 1.0;
        let mut group = div()
            .flex()
            .items_center()
            .h(px(h))
            .rounded(px(RADIUS))
            .border_1()
            .border_color(t.input)
            .when(t.card_shadow, |d| d.shadow_xs());
        for (i, label) in self.labels.into_iter().enumerate() {
            let on = selected == Some(i);
            let handler = self.on_change.clone();
            group = group.child(
                div()
                    .id(child_id(&self.id, format!("seg{i}")))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(if self.small { 12. } else { 16. }))
                    .text_size(px(FONT))
                    .whitespace_nowrap()
                    .text_color(t.foreground)
                    .cursor_pointer()
                    .when(i > 0, |d| d.border_l_1().border_color(t.input))
                    .when(i == 0, |d| d.rounded_l(px(inner)))
                    .when(i + 1 == n, |d| d.rounded_r(px(inner)))
                    .map(|d| {
                        if on {
                            d.bg(t.accent).font_weight(FontWeight::MEDIUM)
                        } else {
                            d.hover(|s| s.bg(t.accent.opacity(0.6)))
                        }
                    })
                    .when_some(handler.filter(|_| !on), |d, f| {
                        d.on_click(move |_, window, cx| f(i, window, cx))
                    })
                    .child(label),
            );
        }
        let key_handler = self.on_change.clone();
        framed(div().id(self.id.clone()), &t, shown, RADIUS, group)
            .track_focus(&focus.tab_stop(true))
            .on_key_down(move |e: &KeyDownEvent, window, cx| {
                if e.keystroke.modifiers.modified() {
                    return;
                }
                let key = e.keystroke.key.as_str();
                // Left/Right only: Up/Down keep scrolling the page.
                if matches!(key, "up" | "down" | "pageup" | "pagedown") {
                    return;
                }
                if let Some(next) = combo_step(key, selected, n) {
                    cx.stop_propagation();
                    if Some(next) != selected {
                        if let Some(f) = &key_handler {
                            f(next, window, cx);
                        }
                    }
                }
            })
    }
}

// ---------------------------------------------------------------------------------------------
// Button
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtnKind {
    /// `variant="outline"`: control fill, `input` border, accent on hover.
    Outline,
    /// `variant="default"`: primary fill.
    Primary,
    /// `variant="destructive"`: destructive fill, white text.
    Destructive,
    /// `variant="ghost"`: transparent, accent on hover.
    Ghost,
}

#[derive(IntoElement)]
pub struct Btn {
    id: ElementId,
    label: SharedString,
    kind: BtnKind,
    disabled: bool,
    theme: Theme,
    on_click: Option<OnClick>,
    /// Square icon button: `label` is the accessible name shown as a tooltip.
    icon: Option<&'static str>,
    /// 32 px (`size="sm"`) instead of 36.
    small: bool,
    /// Focus handle owned by the caller (to focus the button from code), instead of a keyed one.
    focus: Option<FocusHandle>,
}

impl Btn {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, theme: Theme) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: BtnKind::Outline,
            disabled: false,
            theme,
            on_click: None,
            icon: None,
            small: false,
            focus: None,
        }
    }
    /// 32 px tall (an icon button 32 × 32), as in a header bar.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }
    /// Square 36 x 36 icon button showing `glyph` (an icon-font code point); the label becomes
    /// its tooltip.
    pub fn icon(mut self, glyph: &'static str) -> Self {
        self.icon = Some(glyph);
        self
    }
    pub fn kind(mut self, kind: BtnKind) -> Self {
        self.kind = kind;
        self
    }
    pub fn primary(self) -> Self {
        self.kind(BtnKind::Primary)
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn on_click(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    /// Use `focus` as the button's focus handle (e.g. the default button of a dialog).
    pub fn focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
    }
}

/// (fill, hover fill, text, border) of a button variant.
fn button_colors(t: &Theme, kind: BtnKind) -> (Hsla, Hsla, Hsla, Option<Hsla>) {
    let clear = gpui::transparent_black();
    match kind {
        BtnKind::Outline => (t.control, t.control_hover, t.foreground, Some(t.input)),
        BtnKind::Primary => (
            t.primary,
            t.primary.opacity(0.9),
            t.primary_foreground,
            None,
        ),
        BtnKind::Destructive => {
            let fill = if t.dark {
                t.destructive.opacity(0.6)
            } else {
                t.destructive
            };
            (fill, fill.opacity(0.9), gpui::white(), None)
        }
        BtnKind::Ghost => (clear, t.accent, t.foreground, None),
    }
}

impl RenderOnce for Btn {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = self.theme;
        let enabled = !self.disabled;
        let focus = match self.focus.clone() {
            Some(f) => f,
            None => focus_for(&self.id, window, cx),
        };
        let shown = enabled && shows_focus(&focus, window, cx);
        let (fill, hover, fg, stroke) = button_colors(&t, self.kind);
        let h = if self.small { CONTROL_H_SM } else { CONTROL_H };
        let control = div()
            .id(child_id(&self.id, "face"))
            .flex()
            .items_center()
            .justify_center()
            .h(px(h))
            .px(px(16.))
            .text_size(px(FONT))
            .font_weight(FontWeight::MEDIUM)
            .rounded(px(RADIUS))
            .bg(fill)
            .text_color(fg)
            .whitespace_nowrap()
            .when_some(stroke, |d, s| d.border_1().border_color(s))
            .when(t.card_shadow && self.kind != BtnKind::Ghost, |d| {
                d.shadow_xs()
            })
            .when(enabled, |d| d.hover(|s| s.bg(hover)));
        let control = match self.icon {
            Some(glyph) => control
                .w(px(h))
                .px(px(0.))
                .font_family(t.fonts.icons)
                .text_size(px(14.))
                .child(glyph),
            None => control.child(self.label.clone()),
        };
        let tip = self.icon.map(|_| self.label.clone());
        framed(div().id(self.id.clone()), &t, shown, RADIUS, control)
            .when_some(tip, |d, tip| {
                d.tooltip(move |_, cx| tip_view(tip.clone(), t, cx))
            })
            .when(self.disabled, |d| d.opacity(0.5))
            .when(enabled, |d| {
                d.track_focus(&focus.tab_stop(true)).cursor_pointer()
            })
            .when_some(self.on_click.filter(|_| enabled), |d, f| {
                d.on_click(move |_, window, cx| f(window, cx))
            })
    }
}

// ---------------------------------------------------------------------------------------------
// "More options" button + dropdown menu
// ---------------------------------------------------------------------------------------------

/// One entry of a [`MoreMenu`] dropdown.
pub enum MenuEntry {
    Item {
        label: SharedString,
        disabled: bool,
        on_click: OnClick,
        /// Drawn in `destructive` (delete actions).
        destructive: bool,
        /// Tooltip (why a disabled entry is disabled).
        hint: Option<SharedString>,
    },
    Separator,
}

impl MenuEntry {
    pub fn item(
        label: impl Into<SharedString>,
        disabled: bool,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        MenuEntry::Item {
            label: label.into(),
            disabled,
            on_click: Rc::new(on_click),
            destructive: false,
            hint: None,
        }
    }

    /// A destructive entry (red text); `hint` explains why it is disabled.
    pub fn destructive(
        label: impl Into<SharedString>,
        disabled: bool,
        hint: Option<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        MenuEntry::Item {
            label: label.into(),
            disabled,
            on_click: Rc::new(on_click),
            destructive: true,
            hint,
        }
    }
}

/// shadcn Tooltip: inverted colours (foreground fill, background text), 12 px, 6 px radius.
struct TipView {
    text: SharedString,
    theme: Theme,
}

impl gpui::Render for TipView {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .px(px(12.))
            .py(px(6.))
            .rounded(px(ITEM_RADIUS))
            .bg(t.foreground)
            .font_family(t.fonts.text)
            .text_size(px(CAPTION))
            .text_color(t.background)
            .child(self.text.clone())
    }
}

/// A shadcn tooltip view for `.tooltip(…)`.
pub fn tip_view(text: impl Into<SharedString>, theme: Theme, cx: &mut App) -> gpui::AnyView {
    let text = text.into();
    cx.new(|_| TipView { text, theme }).into()
}

/// Ghost 32 × 32 "More options" icon button (glyph E712) opening a dropdown menu under it,
/// right-aligned to the button. Enter/Space open/close it, Escape or a click outside closes
/// it, a click on an entry runs it and closes the menu.
#[derive(IntoElement)]
pub struct MoreMenu {
    id: ElementId,
    label: SharedString,
    entries: Vec<MenuEntry>,
    theme: Theme,
}

impl MoreMenu {
    /// `label` is the tooltip ("More options").
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        entries: Vec<MenuEntry>,
        theme: Theme,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            entries,
            theme,
        }
    }
}

impl RenderOnce for MoreMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = self.theme;
        let focus = focus_for(&self.id, window, cx);
        let shown = shows_focus(&focus, window, cx);
        let open_state = window.use_keyed_state(child_id(&self.id, "open"), cx, |_, _| false);
        let hover_state = window.use_keyed_state(child_id(&self.id, "hover"), cx, |_, _| false);
        let open = *open_state.read(cx);

        let face = div()
            .id(child_id(&self.id, "face"))
            .flex()
            .items_center()
            .justify_center()
            .size(px(CONTROL_H_SM))
            .rounded(px(RADIUS))
            .font_family(t.fonts.icons)
            .text_size(px(16.))
            .text_color(t.foreground)
            .when(open, |d| d.bg(t.accent))
            .hover(|s| s.bg(t.accent))
            .child(glyph::MORE);

        let menu = open.then(|| {
            let close = open_state.clone();
            let hovered = hover_state.clone();
            let mut list = popover(&t, child_id(&self.id, "menu"))
                .min_w(px(176.))
                .on_mouse_down_out(move |_, _, cx| {
                    // A press on the button itself is handled by its click (toggle).
                    if !*hovered.read(cx) {
                        close.update(cx, |open, cx| {
                            *open = false;
                            cx.notify();
                        });
                    }
                });
            for (i, entry) in self.entries.into_iter().enumerate() {
                match entry {
                    MenuEntry::Separator => {
                        list = list.child(div().mx(px(-4.)).my(px(4.)).h(px(1.)).bg(t.border));
                    }
                    MenuEntry::Item {
                        label,
                        disabled,
                        on_click,
                        destructive,
                        hint,
                    } => {
                        let close = open_state.clone();
                        list = list.child(
                            popover_item(&t, child_id(&self.id, format!("item{i}")))
                                .pr(px(24.))
                                .map(|d| {
                                    if disabled {
                                        d.opacity(0.5)
                                    } else {
                                        d.cursor_pointer().on_click(move |_, window, cx| {
                                            close.update(cx, |open, cx| {
                                                *open = false;
                                                cx.notify();
                                            });
                                            on_click(window, cx);
                                        })
                                    }
                                })
                                .when(destructive, |d| d.text_color(t.destructive))
                                .when_some(hint, |d, hint| {
                                    d.tooltip(move |_, cx| tip_view(hint.clone(), t, cx))
                                })
                                .child(label),
                        );
                    }
                }
            }
            div()
                .absolute()
                .right(px(0.))
                .top(px(CONTROL_H_SM + 4.))
                .child(
                    deferred(
                        anchored()
                            .anchor(gpui::Corner::TopRight)
                            .snap_to_window_with_margin(px(8.))
                            .child(list),
                    )
                    .with_priority(1),
                )
        });

        let toggle = open_state.clone();
        let keys = open_state.clone();
        let hovered = hover_state.clone();
        let tip = self.label.clone();
        framed(div().id(self.id.clone()), &t, shown, RADIUS, face)
            .relative()
            .track_focus(&focus.tab_stop(true))
            .cursor_pointer()
            .when(!open, |d| {
                d.tooltip(move |_, cx| {
                    cx.new(|_| TipView {
                        text: tip.clone(),
                        theme: t,
                    })
                    .into()
                })
            })
            .on_hover(move |h, _, cx| {
                let h = *h;
                hovered.update(cx, |v, _| *v = h);
            })
            .on_click(move |_, _, cx| {
                toggle.update(cx, |open, cx| {
                    *open = !*open;
                    cx.notify();
                });
            })
            .on_key_down(move |e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" && *keys.read(cx) {
                    keys.update(cx, |open, cx| {
                        *open = false;
                        cx.notify();
                    });
                    cx.stop_propagation();
                }
            })
            .children(menu)
    }
}

// ---------------------------------------------------------------------------------------------
// Layout helpers (a card per setting)
// ---------------------------------------------------------------------------------------------

/// Section header above a group (14 px semibold; 32 px above, 12 below).
pub fn section_title(t: &Theme, text: impl Into<SharedString>, first: bool) -> Div {
    div()
        .mt(px(if first { 0. } else { 32. }))
        .mb(px(12.))
        .px(px(1.))
        .text_size(px(FONT))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.foreground)
        .child(text.into())
}

/// One shadcn Card: `card` fill, 1 px border, 12 px radius, `shadow-sm` in light.
pub fn card(t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .when(t.card_shadow, |d| d.shadow_sm())
}

/// A group of settings: every element is its own card, 8 px apart.
pub fn group(t: &Theme, items: Vec<gpui::AnyElement>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .children(items.into_iter().map(|item| card(t).child(item)))
}

/// One setting inside its card: label (14 px medium) and description (13 px, muted) on the
/// left, the control right-aligned and vertically centred; min height 68 px; padding 16 / 14.
/// The text column takes the remaining width and wraps; the control keeps its natural width
/// (never shrunk, never pushed past the card's right edge).
pub fn item(
    t: &Theme,
    label: impl Into<SharedString>,
    desc: Option<SharedString>,
    control: impl IntoElement,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(24.))
        .px(px(20.))
        .py(px(14.))
        .min_h(px(68.))
        .child(text_column(t, label.into(), desc))
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.))
                .child(control),
        )
}

/// Take the free width of a flex row, and only that: grow and shrink from a 0 px basis, min
/// width 0, so long text wraps instead of widening the row.
pub fn fill_rest(d: Div) -> Div {
    d.flex_grow().flex_shrink().flex_basis(px(0.)).min_w(px(0.))
}

/// Label + description column of a card (takes the free width, wraps its text).
fn text_column(t: &Theme, label: SharedString, desc: Option<SharedString>) -> Div {
    fill_rest(div().flex().flex_col().gap(px(2.)))
        .child(
            div()
                .text_size(px(FONT))
                .line_height(px(20.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.foreground)
                .child(label),
        )
        .when_some(desc, |d, desc| {
            d.child(
                div()
                    .text_size(px(DESC))
                    .line_height(px(18.))
                    .text_color(t.muted_foreground)
                    .child(desc),
            )
        })
}

/// Same as [`item`] with a disabled look.
pub fn item_disabled(
    t: &Theme,
    label: impl Into<SharedString>,
    desc: Option<SharedString>,
    control: impl IntoElement,
    disabled: bool,
) -> Div {
    item(t, label, desc, control).when(disabled, |d| d.opacity(0.5))
}

/// Caption under a group (13 px, muted).
pub fn caption(t: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .px(px(1.))
        .pt(px(10.))
        .text_size(px(DESC))
        .line_height(px(18.))
        .text_color(t.muted_foreground)
        .child(text.into())
}

/// Small coloured note (validation error, warning).
pub fn note(color: Hsla, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(CAPTION))
        .text_color(color)
        .child(text.into())
}

/// shadcn Alert (destructive) under the page title: card fill, border, destructive icon and
/// text.
pub fn error_strip(t: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_h(px(48.))
        .px(px(16.))
        .py(px(12.))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .text_color(t.destructive)
        .text_size(px(FONT))
        .child(
            div()
                .flex_shrink_0()
                .font_family(t.fonts.icons)
                .text_size(px(16.))
                .child(glyph::ERROR),
        )
        .child(div().flex_1().child(text.into()))
}

/// Navigation pane glyph: 16 px monochrome Segoe Fluent Icons glyph.
pub fn nav_icon(t: &Theme, glyph: &'static str) -> Div {
    div()
        .w(px(16.))
        .flex_shrink_0()
        .flex()
        .justify_center()
        .font_family(t.fonts.icons)
        .text_size(px(16.))
        .text_color(t.foreground)
        .child(glyph)
}

#[cfg(test)]
mod style_tests {
    use super::*;

    #[test]
    fn switch_follows_shadcn() {
        let l = Theme::light();
        assert_eq!(switch_colors(&l, true), (l.primary, l.background));
        assert_eq!(switch_colors(&l, false), (l.input, l.background));
        let d = Theme::dark();
        assert_eq!(switch_colors(&d, true), (d.primary, d.background));
        assert_eq!(switch_colors(&d, false), (d.input, d.foreground));
    }

    #[test]
    fn button_variants() {
        let t = Theme::light();
        let (fill, _, fg, border) = button_colors(&t, BtnKind::Primary);
        assert_eq!((fill, fg, border), (t.primary, t.primary_foreground, None));
        let (fill, hover, _, border) = button_colors(&t, BtnKind::Outline);
        assert_eq!(
            (fill, hover, border),
            (t.control, t.control_hover, Some(t.input))
        );
        let (fill, hover, _, _) = button_colors(&t, BtnKind::Ghost);
        assert_eq!((fill.a, hover), (0.0, t.accent));
    }
}

#[cfg(test)]
mod combo_tests {
    use super::{combo_picks, combo_step};

    #[test]
    fn repicking_the_selected_entry_is_not_a_change() {
        assert!(!combo_picks(1, Some(1)));
        assert!(combo_picks(0, Some(1)));
        assert!(combo_picks(0, None));
    }

    #[test]
    fn arrow_keys_move_and_clamp() {
        assert_eq!(combo_step("down", Some(0), 3), Some(1));
        assert_eq!(combo_step("down", Some(2), 3), Some(2));
        assert_eq!(combo_step("up", Some(0), 3), Some(0));
        assert_eq!(combo_step("up", None, 3), Some(0));
        assert_eq!(combo_step("end", Some(0), 3), Some(2));
        assert_eq!(combo_step("a", Some(0), 3), None);
        assert_eq!(combo_step("down", None, 0), None);
    }
}

/// Provider logo size (SettingsCard header icon).
pub const LOGO: f32 = 24.0;

/// Provider fallback letters in the 24 px logo box (no disc, like a SettingsCard icon).
pub fn avatar(t: &Theme, letters: &'static str, dim: bool) -> Div {
    div()
        .size(px(LOGO))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .text_color(t.foreground)
        .text_size(px(if letters.len() > 2 { 9. } else { 13. }))
        .font_weight(FontWeight::SEMIBOLD)
        .when(dim, |d| d.opacity(0.5))
        .child(letters)
}

/// Asset path prefix of the provider marks served by the notch's asset source
/// (`codenotch_ui_notch::Assets`, registered application-wide by the app with
/// `Application::with_assets`): `codenotch-glyphs/<glyph id>.svg`.
pub const MARK_PREFIX: &str = "codenotch-glyphs/";

/// Candidate asset paths of a provider mark, in lookup order: the provider's glyph id, then
/// the file name the mark ships under (Antigravity → `gemini.svg`). Same rule as the notch.
pub fn mark_paths(kind: &codenotch_core::ProviderKind) -> Vec<String> {
    let mut names = vec![kind.glyph_id().to_string()];
    if *kind == codenotch_core::ProviderKind::Antigravity {
        names.push("gemini".into());
    }
    names
        .into_iter()
        .map(|n| format!("{MARK_PREFIX}{n}.svg"))
        .collect()
}

/// Which marks the registered asset source can serve (probed once per provider and process,
/// so rendering never touches the disk again).
#[derive(Default)]
struct MarkCache(std::collections::HashMap<String, Option<SharedString>>);

impl gpui::Global for MarkCache {}

/// The mark of `kind` when the app registered the glyph assets, else `None` (letters).
pub fn mark_for(kind: &codenotch_core::ProviderKind, cx: &mut App) -> Option<SharedString> {
    let id = kind.id().to_string();
    if let Some(hit) = cx.default_global::<MarkCache>().0.get(&id) {
        return hit.clone();
    }
    let found = mark_paths(kind)
        .into_iter()
        .find(|p| matches!(cx.asset_source().load(p), Ok(Some(_))))
        .map(SharedString::from);
    cx.default_global::<MarkCache>().0.insert(id, found.clone());
    found
}

/// The provider's 24 px SVG mark when available ([`avatar`] letters otherwise).
pub fn mark_avatar(t: &Theme, letters: &'static str, mark: Option<SharedString>, dim: bool) -> Div {
    match mark {
        Some(path) => div()
            .size(px(LOGO))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .when(dim, |d| d.opacity(0.5))
            .child(
                gpui::svg()
                    .path(path)
                    .size(px(LOGO))
                    .text_color(t.foreground),
            ),
        None => avatar(t, letters, dim),
    }
}

#[cfg(test)]
mod mark_tests {
    use super::*;
    use codenotch_core::ProviderKind;

    #[test]
    fn mark_paths_follow_the_notch_convention() {
        assert_eq!(
            mark_paths(&ProviderKind::Claude),
            ["codenotch-glyphs/claude.svg"]
        );
        assert_eq!(
            mark_paths(&ProviderKind::Antigravity),
            [
                format!(
                    "codenotch-glyphs/{}.svg",
                    ProviderKind::Antigravity.glyph_id()
                ),
                "codenotch-glyphs/gemini.svg".into()
            ]
        );
    }
}
