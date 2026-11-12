//! Settings controls.
//!
//! Decision: we use **gpui-component 0.5.0** (the last release built on crates.io gpui 0.2.x;
//! 0.6+ moved to the `gpui-pre` fork). It compiles and links here, and gives a real text input
//! (IME, selection, clipboard), select/dropdown, slider, switch and buttons, which would be
//! weeks of work to hand-write. Cost: a heavier dependency tree (tree-sitter, lsp-types …) and
//! the requirement below.
//!
//! **Requirement:** a window that hosts these controls must have a `Root` as its root view:
//! `cx.open_window(opts, |window, cx| { let view = cx.new(|cx| MyView::new(window, cx));
//! cx.new(|cx| Root::new(view, window, cx)) })`. Notch/pill/card windows that only draw rings
//! and text do not need it. Call [`crate::init`] once at start-up.
//!
//! Everything a UI crate needs is re-exported here; prefer these paths over `gpui_component::…`
//! so the version stays in one place.

use crate::theme::Theme;
use gpui::{App, ElementId, SharedString, Window};

pub use gpui_component::button::{Button, ButtonGroup, ButtonVariant, ButtonVariants};
pub use gpui_component::input::{Input, InputEvent, InputState};
pub use gpui_component::select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState};
pub use gpui_component::slider::{Slider, SliderEvent, SliderState, SliderValue};
pub use gpui_component::switch::Switch;
pub use gpui_component::{Disableable, IndexPath, Root, Selectable, Sizable, StyledExt};

/// Toggle switch. `on_change` receives the new value.
pub fn toggle(
    id: impl Into<ElementId>,
    checked: bool,
    on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> Switch {
    Switch::new(id)
        .checked(checked)
        .on_click(move |value: &bool, window, cx| on_change(*value, window, cx))
}

/// Plain button. Use `.primary()` / `.ghost()` etc. from [`ButtonVariants`] for variants.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .label(label)
        .on_click(move |_, window, cx| on_click(window, cx))
}

/// Segmented control (one choice among `labels`). `on_change` receives the clicked index.
pub fn segmented(
    id: impl Into<ElementId>,
    labels: impl IntoIterator<Item = impl Into<SharedString>>,
    selected: usize,
    on_change: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> ButtonGroup {
    let id: ElementId = id.into();
    let buttons = labels.into_iter().enumerate().map(|(i, label)| {
        Button::new(ElementId::NamedInteger("seg".into(), i as u64))
            .label(label)
            .selected(i == selected)
    });
    ButtonGroup::new(id)
        .children(buttons)
        .on_click(move |clicked: &Vec<usize>, window, cx| {
            if let Some(&i) = clicked.first() {
                on_change(i, window, cx);
            }
        })
}

/// Push our tokens into gpui-component's global theme so its text box and slider look like
/// shadcn/ui controls (8 px radius, `input` border, `ring` border when focused, primary slider
/// range with a background-coloured thumb, no shadow). Call after [`crate::init`] and whenever
/// the theme flips.
///
/// `colors.background` is both the text box fill and `Root`'s background; the settings view
/// paints its own opaque window background over `Root`, so the translucent control fill is fine.
pub fn apply_component_theme(theme: &Theme, window: Option<&mut Window>, cx: &mut App) {
    use gpui_component::theme::{Theme as CTheme, ThemeMode as CMode};
    CTheme::change(
        if theme.dark {
            CMode::Dark
        } else {
            CMode::Light
        },
        window,
        cx,
    );
    let t = CTheme::global_mut(cx);
    t.colors.primary = theme.primary;
    t.colors.primary_foreground = theme.primary_foreground;
    t.colors.background = theme.control;
    t.colors.foreground = theme.foreground;
    t.colors.muted = theme.muted;
    t.colors.muted_foreground = theme.muted_foreground;
    t.colors.border = theme.border;
    t.colors.input = theme.input;
    // Focused text box: `ring` border; the 3 px translucent ring around it is drawn by the caller.
    t.colors.ring = theme.ring;
    t.colors.caret = theme.foreground;
    t.colors.selection = theme.primary.opacity(0.2);
    t.colors.slider_bar = theme.primary;
    t.colors.slider_thumb = theme.background;
    t.radius = gpui::px(8.);
    t.shadow = false;
    t.font_family = theme.fonts.text.into();
}

// ---------------------------------------------------------------------------------------------
// Keyboard focus visual
// ---------------------------------------------------------------------------------------------

/// Whether focus visuals are shown: WinUI draws them only after keyboard navigation, never
/// after a click. gpui 0.2.2 does not track this (no `:focus-visible`), so the settings window
/// sets it from a keystroke interceptor ([`track_focus_visible`], navigation keys → on) and a
/// capture-phase mouse-down listener on its root ([`set_focus_visible`] `false`).
#[derive(Debug, Default, Clone, Copy)]
pub struct FocusVisible(pub bool);

impl gpui::Global for FocusVisible {}

/// True when the last input was keyboard navigation.
pub fn focus_visible(cx: &App) -> bool {
    cx.try_global::<FocusVisible>().is_some_and(|f| f.0)
}

/// Turn focus visuals on/off and redraw the window when it changes.
pub fn set_focus_visible(on: bool, window: &mut Window, cx: &mut App) {
    if focus_visible(cx) != on {
        cx.set_global(FocusVisible(on));
        window.refresh();
    }
}

/// Keys that move keyboard focus or a selection (they turn focus visuals on).
pub fn is_navigation_key(key: &str) -> bool {
    matches!(
        key,
        "tab" | "up" | "down" | "left" | "right" | "home" | "end" | "pageup" | "pagedown"
    )
}

/// Turn focus visuals on at the first navigation keystroke (any window). Keep the subscription.
pub fn track_focus_visible(cx: &mut App) -> gpui::Subscription {
    cx.intercept_keystrokes(|event, window, cx| {
        if is_navigation_key(&event.keystroke.key) {
            set_focus_visible(true, window, cx);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::is_navigation_key;

    #[test]
    fn navigation_keys() {
        assert!(is_navigation_key("tab") && is_navigation_key("down"));
        assert!(!is_navigation_key("a") && !is_navigation_key("enter"));
    }
}
