//! # codenotch-ui-settings (owner: agent H)
//!
//! Settings window (Accounts / Appearance / Notifications / Analytics / General), first-run
//! onboarding and the analytics view. Spec: visual-spec.md §9, §12; config keys:
//! `codenotch_core::config::Settings`.
//!
//! Inputs: the [`UiModel`] entity. Outputs: `UiModel::send(Command)`. The window never mutates
//! config directly: it keeps a local edited copy of the snapshot's `Config` and sends
//! `Command::SaveConfig(Box::new(edited))` after each change (text fields and sliders are
//! debounced); new snapshots are merged back with the rule in [`logic::Sync`]. Secrets typed
//! in fields go out through `Command::SetSecret` and the field is cleared.
//!
//! Layout of the crate:
//! * `logic` — pure: edits (reducers), validation, re-sync rule, page models (unit tested).
//! * `widgets` — Windows 11 (WinUI 3) toggle switch, combo box, buttons and settings cards
//!   drawn with gpui primitives.
//! * `view` — window state, editing model, chrome. `pages` — page bodies (`pages_stats`: the
//!   statistics page body, over the pure `stats` model). `i18n` — French.
//!
//! The window is a normal, activating, taskbar-visible window with a custom-drawn 32 px title
//! bar (drag region) whose minimise / maximise / close caption buttons are native hit-test
//! regions; it is resizable down to 800 × 560. Only one exists:
//! [`open`] focuses it when it is already open. Its root view is a gpui-component `Root`.

mod i18n;
mod logic;
mod pages;
mod pages_stats;
mod stats;
mod view;
mod widgets;

use codenotch_ui_kit::controls::Root;
use codenotch_ui_kit::{metrics, UiModel};
use gpui::{
    px, size, App, AppContext, Bounds, Entity, Global, Pixels, TitlebarOptions, WeakEntity,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions,
};
use view::SettingsView;

/// The settings page's hotkey validator ("ctrl + alt + n" → "Ctrl+Alt+N"); exported so the app can
/// check it against `codenotch_platform::hotkey::parse`.
pub use logic::{normalize_hotkey, FieldError};

/// Open (or focus) the settings window. Opens on the last page shown in this session
/// (Accounts the first time). Focusing an open window keeps its current page.
pub fn open(model: Entity<UiModel>, cx: &mut App) {
    open_page(model, None, cx);
}

/// Open (or focus) the settings window on `page` (the `--settings-page=<name>` start-up flag).
pub fn open_at(model: Entity<UiModel>, page: Page, cx: &mut App) {
    open_page(model, Some(page), cx);
}

/// Open the first-run onboarding (inside the settings window). Call it at start-up when
/// `Config::needs_welcome()` is true. Finishing or skipping it saves `onboardingDone = true`
/// and shows the Accounts page.
pub fn open_onboarding(model: Entity<UiModel>, cx: &mut App) {
    open_page(model, Some(Page::Onboarding), cx);
}

/// The settings window is open and active (the user is working in it): an automatic update
/// waits until this is false.
pub fn is_in_use(cx: &mut App) -> bool {
    let Some((handle, _)) = cx.default_global::<SettingsWindow>().open.clone() else {
        return false;
    };
    handle
        .update(cx, |_, window, _| window.is_window_active())
        .unwrap_or(false)
}

/// Pages of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Onboarding,
    Accounts,
    Appearance,
    Notifications,
    General,
    Analytics,
}

/// The single settings window of the process, and the last page shown.
#[derive(Default)]
struct SettingsWindow {
    open: Option<(WindowHandle<Root>, WeakEntity<SettingsView>)>,
    last_page: Option<Page>,
}

impl Global for SettingsWindow {}

pub(crate) fn remember_page(page: Page, cx: &mut App) {
    if page != Page::Onboarding {
        cx.default_global::<SettingsWindow>().last_page = Some(page);
    }
}

/// Default rectangle of the settings window: [`metrics::SETTINGS_W`] x [`metrics::SETTINGS_H`],
/// fitted to the primary monitor's work area and centered there.
fn default_bounds(cx: &App) -> Bounds<Pixels> {
    let want = (metrics::SETTINGS_W, metrics::SETTINGS_H);
    match codenotch_ui_kit::window::monitor_or_primary("") {
        Some(m) => codenotch_ui_kit::window::fit_centered(
            m.work,
            m.scale,
            want,
            (metrics::SETTINGS_MIN_W, metrics::SETTINGS_MIN_H),
            metrics::SETTINGS_MARGIN,
        ),
        None => Bounds::centered(None, size(px(want.0), px(want.1)), cx),
    }
}

fn open_page(model: Entity<UiModel>, page: Option<Page>, cx: &mut App) {
    let existing = cx.default_global::<SettingsWindow>().open.clone();
    if let Some((handle, weak)) = existing {
        if let Some(view) = weak.upgrade() {
            let focused = handle
                .update(cx, |_, window, cx| {
                    // Restored if minimised, raised and activated despite the foreground lock
                    // (gpui's `activate_window` only flashes the taskbar button from here).
                    codenotch_ui_kit::window::bring_window_to_front(window);
                    // Brought back: same page unless one is asked for, never a field focused.
                    view.update(cx, |v, cx| match page {
                        Some(page) => v.go(page, window, cx),
                        None => v.release_input_focus(window, cx),
                    });
                })
                .is_ok();
            if focused {
                cx.activate(true);
                return;
            }
        }
    }

    let page = page
        .or(cx.default_global::<SettingsWindow>().last_page)
        .unwrap_or(Page::Accounts);
    let lang = model.read(cx).snapshot.lang();
    let bounds = default_bounds(cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(i18n::t(lang, "CodeNotch Settings").to_string().into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_movable: true,
        // Resizable like Windows Settings (gpui hit-tests the resize borders of a window with a
        // hidden title bar itself; the caption buttons stay native hit-test regions).
        is_resizable: true,
        is_minimizable: true,
        window_min_size: Some(size(
            px(metrics::SETTINGS_MIN_W),
            px(metrics::SETTINGS_MIN_H),
        )),
        window_background: WindowBackgroundAppearance::Opaque,
        ..Default::default()
    };
    let mut created: Option<WeakEntity<SettingsView>> = None;
    let result = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| SettingsView::new(model, page, window, cx));
        created = Some(view.downgrade());
        cx.new(|cx| Root::new(view, window, cx))
    });
    match (result, created) {
        (Ok(handle), Some(weak)) => {
            // gpui shows the new window without forcing it to the front: it lands behind the
            // foreground app when the click came from the non-activating notch.
            let _ = handle.update(cx, |_, window, _| {
                codenotch_ui_kit::window::bring_window_to_front(window);
            });
            cx.default_global::<SettingsWindow>().open = Some((handle, weak));
            remember_page(page, cx);
        }
        (Ok(_), None) => log::error!("cannot open settings: view not created"),
        (Err(err), _) => log::error!("cannot open settings: {err}"),
    }
    cx.activate(true);
}
