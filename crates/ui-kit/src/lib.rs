//! # codenotch-ui-kit
//!
//! Shared GPUI building blocks for the notch (`codenotch-ui-notch`) and the settings window
//! (`codenotch-ui-settings`). UI crates depend on `codenotch-core` + this crate (+ `gpui`) only.
//!
//! | module | what |
//! |---|---|
//! | [`model`] | [`UiModel`] entity (snapshot + command sender) and the event pump |
//! | [`theme`] | [`Theme`] tokens, grade colours, colour ramp |
//! | [`metrics`] | geometry constants (pill, ring, card) |
//! | [`chart`] | [`AreaChart`]: stacked area chart with hover cursor and tooltip (statistics page) |
//! | [`tooltip`] | instant shadcn chart tooltip card, [`HoverGrid`] (heat grids), [`HoverTip`], [`SideTip`] (sidebar rail labels) |
//! | [`ring`] | [`ring()`] element and the arc painter ([`paint_arc`], [`arc_polygon`]) |
//! | [`controls`] | settings controls: re-exports of `gpui-component` 0.5.0 + small helpers |
//! | [`toast`] | react-hot-toast style [`toast::Toaster`] overlay (queue logic unit tested) |
//! | [`window`] | transparent non-activating overlay windows, HWND styles, moves, work area |
//!
//! Call [`init`] once inside `Application::run` before opening any window.

pub mod chart;
pub mod controls;
pub mod metrics;
pub mod model;
pub mod ring;
pub mod theme;
pub mod toast;
pub mod tooltip;
pub mod window;

pub use chart::{AreaChart, ChartSeries};
pub use model::UiModel;
pub use ring::{arc_polygon, paint_arc, ring, ArcLayer, Cap, RingSpec};
pub use theme::Theme;
pub use tooltip::{tooltip_card, HoverGrid, HoverTip, SideTip, TipContent, TipRow};

/// Re-export so UI crates use the exact same gpui-component version.
pub use gpui_component;

/// Initialise the kit: `gpui-component` (needed by every control) and its theme.
/// Call once at start-up, on the main thread.
pub fn init(cx: &mut gpui::App) {
    gpui_component::init(cx);
    theme::resolve_fonts(cx);
}
