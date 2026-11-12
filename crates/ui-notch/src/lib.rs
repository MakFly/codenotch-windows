//! # codenotch-ui-notch
//!
//! The notch: an always-on-top pill on a screen edge with one ring cell per account, the folded
//! rest pill, the hover card with its tail, the settings orb and drag grip, the right-click menu,
//! and the reset card window.
//! Depends on `codenotch-core` + `codenotch-ui-kit` (+ gpui) only.
//!
//! Inputs: the [`UiModel`] entity (observed for snapshots, subscribed for `ResetDetected` →
//! reset card and `Notice` → notice bubble). Outputs: `UiModel::send(Command)` only — refresh on
//! ring click, `OpenSettings` from the orb or the menu, `OpenUrl`, `SignIn`, `Quit`, and
//! `SaveConfig` after a drag or a "Keep open" toggle (config is never mutated locally).
//!
//! ## Integration
//! * Register the glyph assets when building the application:
//!   `Application::new().with_assets(codenotch_ui_notch::Assets::new())` (chain another source
//!   with [`Assets::with_fallback`]). Without it the cells fall back to letters.
//! * Connect the adaptive pill: `codenotch_ui_notch::set_backdrop_sampler(codenotch_platform::backdrop::luminance)`.
//! * Connect taskbar mode: [`set_taskbar_probe`] with `codenotch_platform::taskbar::probe` (see
//!   its doc for the exact glue). Without it `taskbarMode` is ignored and the notch stays.
//! * Call [`open`] once after `codenotch_ui_kit::init`.
//!
//! ## Taskbar mode
//! With `taskbarMode` "left"/"right" and a probe installed, the floating notch is hidden and the
//! same cells (ring, weekly ring, activity arc, glyph, figure beside the ring) are drawn as a
//! strip over the free space of the primary taskbar (`strip`, `strip_layout`). Hover shows the
//! account's card in its own window, left-click refreshes that account, right-click opens
//! Settings. The switch follows the setting while running.
//!
//! Layout and state logic live in pure modules (`geom`, `placement`, `fold`, `cell`, `card`,
//! `menu`, `easing`, `adaptive`, `reset` layout) with unit tests; `notch` and the reset view
//! are thin gpui layers on top.

mod adaptive;
mod card;
mod cell;
mod dims;
mod easing;
mod fold;
mod geom;
mod glyphs;
mod menu;
mod notch;
mod placement;
mod reset;
mod strip;
mod strip_layout;
mod taskbar_probe;
mod text;
mod toast_host;
mod win32;

pub use toast_host::toaster as toast_host;

pub use adaptive::set_backdrop_sampler;
pub use glyphs::{Assets, NOTICE as GLYPHS_NOTICE};
pub use taskbar_probe::{set_taskbar_probe, TaskbarProbe, TaskbarQuery, TaskbarSide, TaskbarStrip};
pub use text::FR;

use codenotch_core::config::Edge;
use codenotch_core::model::Event;
use codenotch_ui_kit::window as kwin;
use codenotch_ui_kit::UiModel;
use gpui::{point, px, size, App, AppContext, Bounds, Entity, WindowOptions};

/// Demo-only (`--preview-card`): keep the notch open with the first account's hover card, so the
/// card can be captured without hovering. Call before [`open`].
pub fn set_preview_card(on: bool) {
    notch::PREVIEW_CARD.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Open the notch window. Call once after `codenotch_ui_kit::init`.
pub fn open(model: Entity<UiModel>, cx: &mut App) {
    let (w, h) = geom::NotchLayout::window_size(Edge::Right, 1, 1.0);
    let bounds = Bounds {
        origin: point(px(0.), px(0.)),
        size: size(px(w), px(h)),
    };
    // Created hidden; the view places, styles, clips and shows it on its first frame.
    let opts = WindowOptions {
        show: false,
        ..kwin::overlay_options(bounds)
    };
    let m = model.clone();
    if let Err(err) = cx.open_window(opts, move |window, cx| {
        cx.new(|cx| notch::NotchView::new(m, window, cx))
    }) {
        log::error!("cannot open the notch window: {err}");
    }
    strip::install(&model, cx);
    cx.subscribe(&model, |model, event: &Event, cx| {
        if let Event::ResetDetected {
            account_id,
            window_id,
            ..
        } = event
        {
            if model.read(cx).snapshot.settings().reset_cards {
                show_reset_card(&model, account_id, window_id, cx);
            }
        }
    })
    .detach();
}

/// Show the reset card for one window (also used by `Command::PreviewResetCard`'s echo).
/// Cards arriving while one is on screen are queued and shown one after another.
pub fn show_reset_card(model: &Entity<UiModel>, account_id: &str, window_id: &str, cx: &mut App) {
    reset::enqueue(model, account_id, window_id, cx);
}
