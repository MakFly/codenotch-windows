//! What one notch cell shows (visual-spec §2–§3, data-spec §6–§7). Pure: colours are applied by
//! the renderer from `used` with `Theme::usage_color`.

use codenotch_core::config::{Settings, WeeklyRing};
use codenotch_core::format::cell_figure;
use codenotch_core::model::{
    AccountView, ActivityState, LimitWindow, ProviderKind, Reading, ReadingStatus,
};

/// One arc to sweep: `fraction` of the circle, coloured by `used` (always *used*).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcData {
    pub fraction: f32,
    pub used: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CellModel {
    pub account_id: String,
    pub provider: ProviderKind,
    /// "42%", "~42%", "<0.1%", "~17", "—", "…".
    pub figure: String,
    /// Main reading arc; `None` = track only.
    pub main: Option<ArcData>,
    pub weekly: Option<ArcData>,
    /// Track, arc and glyph at 0.55.
    pub stale: bool,
    /// Glyph at 0.55 (stale or limit reached).
    pub dim_glyph: bool,
    pub activity: ActivityState,
    pub refreshing: bool,
    /// `fetched_at` of the reading (to notice a new reading after a refresh click).
    pub fetched_at: Option<i64>,
}

/// Swept fraction: used, or 1 − used in remaining mode (data-spec §7; visual-spec Unknown 2 —
/// we follow the reference code, which sweeps the remaining share).
pub fn sweep(used_percent: f64, show_remaining: bool) -> f32 {
    let f = (used_percent / 100.0).clamp(0.0, 1.0) as f32;
    if show_remaining {
        1.0 - f
    } else {
        f
    }
}

/// Claude numbers count as current only when the reading is live, fresh and the window's reset
/// has not passed (data-spec §2.5). Other providers are always "current" (stale ones dim).
pub fn is_current(reading: &Reading, w: &LimitWindow, now: i64) -> bool {
    if reading.provider != ProviderKind::Claude {
        return true;
    }
    matches!(
        reading.status,
        ReadingStatus::Fresh | ReadingStatus::RateLimited { .. }
    ) && reading.fetched_at > 0
        && !reading.is_stale(now)
        && !w.reset_passed(now)
}

fn arc_for(
    reading: &Reading,
    w: Option<&LimitWindow>,
    now: i64,
    remaining: bool,
) -> Option<ArcData> {
    let w = w?;
    if w.count.is_some() || !is_current(reading, w, now) {
        return None;
    }
    let used = w.used_percent?;
    Some(ArcData {
        fraction: sweep(used, remaining),
        used,
    })
}

pub fn cell_model(view: &AccountView, settings: &Settings, now: i64) -> CellModel {
    let remaining = settings.show_remaining;
    let reading = view.reading.as_ref().filter(|r| r.shows_figures());
    let main = reading.and_then(|r| arc_for(r, r.headline(), now, remaining));
    let weekly = match settings.weekly_ring {
        WeeklyRing::Off => None,
        WeeklyRing::Inside if view.activity == ActivityState::Working => None,
        _ => reading.and_then(|r| arc_for(r, r.weekly(), now, remaining)),
    };
    let stale = view.is_stale(now);
    let limit_reached = main.is_some_and(|a| a.used >= 100.0);
    CellModel {
        account_id: view.account.id.clone(),
        provider: view.account.provider.clone(),
        figure: cell_figure(view, remaining),
        main,
        weekly,
        stale,
        dim_glyph: stale || limit_reached,
        activity: view.activity,
        refreshing: view.refreshing,
        fetched_at: view.reading.as_ref().map(|r| r.fetched_at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::demo::ids;
    use codenotch_core::model::{Snapshot, WindowKind};

    const NOW: i64 = 1_800_000_000;

    fn demo() -> Snapshot {
        Snapshot::demo_at(NOW)
    }

    fn cell(s: &Snapshot, id: &str) -> CellModel {
        cell_model(s.account(id).unwrap(), s.settings(), NOW)
    }

    #[test]
    fn demo_states() {
        let s = demo();
        let c = cell(&s, ids::CLAUDE);
        assert_eq!(c.figure, "61%");
        assert!((c.main.unwrap().fraction - 0.61).abs() < 1e-6);
        assert_eq!(c.weekly.unwrap().used, 34.0); // demo: weekly ring outside
        assert_eq!(c.activity, ActivityState::Working);
        assert!(!c.stale && !c.dim_glyph);

        // stale Codex: arcs kept but dimmed
        let c = cell(&s, ids::CODEX);
        assert!(c.stale && c.dim_glyph);
        assert_eq!(c.main.unwrap().used, 23.0);

        // signed out: dash, no arc
        let c = cell(&s, ids::GROK);
        assert_eq!(c.figure, "—");
        assert!(c.main.is_none() && c.weekly.is_none());

        // first reading in flight
        let c = cell(&s, ids::OPENCODE);
        assert_eq!(c.figure, "…");
        assert!(c.refreshing && c.main.is_none());

        // derived figure; count window draws no arc
        let c = cell(&s, ids::CUSTOM);
        assert_eq!(c.figure, "~9%");
        assert!(c.main.is_some());

        // rate-limited Cursor keeps its critical numbers
        let c = cell(&s, ids::CURSOR);
        assert_eq!(c.main.unwrap().used, 97.0);
        assert_eq!(cell(&s, ids::GLM).figure, "—");
    }

    #[test]
    fn remaining_mode_sweeps_the_rest_but_colours_by_used() {
        let mut s = demo();
        s.config.settings.show_remaining = true;
        let c = cell(&s, ids::CLAUDE);
        assert_eq!(c.figure, "39%");
        let a = c.main.unwrap();
        assert!((a.fraction - 0.39).abs() < 1e-6);
        assert_eq!(a.used, 61.0);
    }

    #[test]
    fn weekly_ring_rules() {
        let mut s = demo();
        s.config.settings.weekly_ring = WeeklyRing::Off;
        assert!(cell(&s, ids::CLAUDE).weekly.is_none());
        s.config.settings.weekly_ring = WeeklyRing::Inside;
        // inside ring stands down while the working arc shows
        assert!(cell(&s, ids::CLAUDE).weekly.is_none());
        assert!(cell(&s, ids::CLAUDE_WORK).weekly.is_some());
    }

    #[test]
    fn claude_without_session_shows_most_used_weekly() {
        let mut s = demo();
        if let Some(r) = s.account_mut(ids::CLAUDE).and_then(|v| v.reading.as_mut()) {
            r.windows = vec![
                LimitWindow::new("seven_day", "Weekly limit", WindowKind::Weekly)
                    .used(71.0)
                    .resets_at(NOW + 3600),
                LimitWindow::new("weekly_scoped", "Weekly limit (Fable)", WindowKind::Weekly)
                    .used(73.0)
                    .resets_at(NOW + 3600),
            ];
        }
        let c = cell(&s, ids::CLAUDE);
        assert_eq!(c.figure, "73%");
        assert_eq!(c.main.unwrap().used, 73.0);
        assert!(c.weekly.is_none());
    }

    #[test]
    fn claude_not_current_hides_arcs() {
        let mut s = demo();
        // reset passed → historical
        if let Some(v) = s.account_mut(ids::CLAUDE) {
            if let Some(r) = v.reading.as_mut() {
                r.windows[0].resets_at = Some(NOW - 10);
            }
        }
        let c = cell(&s, ids::CLAUDE);
        assert!(c.main.is_none());
        assert_eq!(c.figure, "61%");
        // stale Claude: no arc either
        let mut s = demo();
        if let Some(r) = s.account_mut(ids::CLAUDE).and_then(|v| v.reading.as_mut()) {
            r.fetched_at = NOW - 3600;
        }
        assert!(cell(&s, ids::CLAUDE).main.is_none());
    }

    #[test]
    fn limit_reached_dims_glyph_and_zero_draws_nothing() {
        let mut s = demo();
        if let Some(r) = s.account_mut(ids::COPILOT).and_then(|v| v.reading.as_mut()) {
            r.windows[0].used_percent = Some(100.0);
        }
        let c = cell(&s, ids::COPILOT);
        assert!(c.dim_glyph && !c.stale);
        assert_eq!(c.main.unwrap().fraction, 1.0);
        let w = LimitWindow::new("x", "X", WindowKind::Other).used(0.0);
        assert_eq!(sweep(w.used_percent.unwrap(), false), 0.0);
        assert_eq!(sweep(0.0, true), 1.0);
    }
}
