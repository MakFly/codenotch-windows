//! Taskbar mode hook. Measuring the taskbar's free space lives in `codenotch-platform`
//! (`taskbar::probe`), which this crate may not depend on, so the app installs it with
//! [`set_taskbar_probe`] — the same pattern as `set_backdrop_sampler`. Without a probe, the
//! `taskbarMode` setting is ignored and the floating notch stays.

use codenotch_core::config::{Settings, TaskbarMode};
use std::sync::{Arc, RwLock};

/// Which part of the taskbar the strip uses (from `taskbarMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarSide {
    /// `taskbarMode: "left"` — free space from the left end.
    Left,
    /// `taskbarMode: "right"` — free space just before the system tray.
    BeforeTray,
}

/// What the strip asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskbarQuery {
    pub side: TaskbarSide,
    /// Width the cells need at full size, device px.
    pub width: i32,
}

/// Where the strip goes. Device px, virtual-screen coordinates. A strip narrower than the query
/// makes the whole row shrink to fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TaskbarStrip {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Device px per logical px of the taskbar's monitor.
    pub dpi: f32,
    /// The taskbar is at the bottom of the screen (else at the top).
    pub bottom: bool,
}

/// `query → Some(strip)` when the strip should show, `None` to hide it (no space, full-screen
/// app, hidden/auto-hidden or vertical taskbar…). Called on the UI thread every ~150 ms while
/// taskbar mode is on, so it must return at once (read a cache; never wait on Explorer).
pub type TaskbarProbe = dyn Fn(TaskbarQuery) -> Option<TaskbarStrip> + Send + Sync;

static PROBE: RwLock<Option<Arc<TaskbarProbe>>> = RwLock::new(None);

/// Install the taskbar probe (once at start-up). Typical wiring in the app:
///
/// ```ignore
/// use codenotch_platform::taskbar::{self, Side};
/// use codenotch_ui_notch::{TaskbarSide, TaskbarStrip};
/// codenotch_ui_notch::set_taskbar_probe(|q| {
///     let side = match q.side { TaskbarSide::Left => Side::Left, TaskbarSide::BeforeTray => Side::BeforeTray };
///     let p = taskbar::probe(side, q.width).ok()?;
///     Some(TaskbarStrip {
///         x: p.rect.left, y: p.rect.top, w: p.rect.right - p.rect.left, h: p.rect.bottom - p.rect.top,
///         dpi: p.dpi, bottom: p.edge == taskbar::BarEdge::Bottom,
///     })
/// });
/// ```
pub fn set_taskbar_probe(f: impl Fn(TaskbarQuery) -> Option<TaskbarStrip> + Send + Sync + 'static) {
    if let Ok(mut p) = PROBE.write() {
        *p = Some(Arc::new(f));
    }
}

pub(crate) fn probe() -> Option<Arc<TaskbarProbe>> {
    PROBE.read().ok().and_then(|p| p.clone())
}

/// The side asked for by the settings, `None` when taskbar mode is off.
pub(crate) fn side(settings: &Settings) -> Option<TaskbarSide> {
    match settings.taskbar_mode {
        TaskbarMode::Off => None,
        TaskbarMode::Left => Some(TaskbarSide::Left),
        TaskbarMode::Right => Some(TaskbarSide::BeforeTray),
    }
}

/// Taskbar mode is in force: asked for in the settings and a probe is installed. The floating
/// notch stays hidden while this holds (even when the strip itself is hidden for lack of room).
pub(crate) fn active(settings: &Settings) -> bool {
    side(settings).is_some() && probe().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_follows_the_setting() {
        let mut s = Settings::default();
        assert_eq!(side(&s), None);
        s.taskbar_mode = TaskbarMode::Left;
        assert_eq!(side(&s), Some(TaskbarSide::Left));
        s.taskbar_mode = TaskbarMode::Right;
        assert_eq!(side(&s), Some(TaskbarSide::BeforeTray));
    }

    #[test]
    fn probe_hook_round_trip() {
        set_taskbar_probe(|q| {
            (q.width > 0).then_some(TaskbarStrip {
                x: 8,
                y: 1035,
                w: q.width.min(300),
                h: 42,
                dpi: 1.0,
                bottom: true,
            })
        });
        let p = probe().unwrap();
        let strip = p(TaskbarQuery {
            side: TaskbarSide::Left,
            width: 500,
        })
        .unwrap();
        assert_eq!((strip.x, strip.w), (8, 300));
        assert!(p(TaskbarQuery {
            side: TaskbarSide::Left,
            width: 0
        })
        .is_none());
        let mut s = Settings::default();
        assert!(!active(&s));
        s.taskbar_mode = TaskbarMode::Right;
        assert!(active(&s));
    }
}
