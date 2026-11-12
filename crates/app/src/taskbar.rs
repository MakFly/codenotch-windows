//! Taskbar mode glue: `codenotch_platform::taskbar` measures the primary taskbar's free space
//! (cached, never blocks), `codenotch_ui_notch` draws the ring strip there. ui-notch may not
//! depend on platform, so the app installs the probe (the pattern of `set_backdrop_sampler`).

use codenotch_platform::taskbar::{self, BarEdge, Side};
use codenotch_ui_notch::{TaskbarQuery, TaskbarSide, TaskbarStrip};

/// One probe: ui-notch's query → platform's placement → ui-notch's strip (`None` = hidden).
pub fn strip_for(q: TaskbarQuery) -> Option<TaskbarStrip> {
    let side = match q.side {
        TaskbarSide::Left => Side::Left,
        TaskbarSide::BeforeTray => Side::BeforeTray,
    };
    let p = taskbar::probe(side, q.width).ok()?;
    Some(TaskbarStrip {
        x: p.rect.left,
        y: p.rect.top,
        w: p.rect.right - p.rect.left,
        h: p.rect.bottom - p.rect.top,
        dpi: p.dpi,
        bottom: p.edge == BarEdge::Bottom,
    })
}

/// Install the probe (once, at start-up, before the notch opens).
pub fn wire() {
    codenotch_ui_notch::set_taskbar_probe(strip_for);
}
