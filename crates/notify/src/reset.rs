//! Reset detection, one state per account, decisions per window id.
//!
//! * Only fresh readings count: status `Fresh`, `fetched_at > 0`, not more than 60 s in the
//!   future and at most 5 min old. Any other reading clears that account's state entirely, so
//!   stale, restored-from-disk, signed-out and error readings never bridge two observations.
//! * A reading not newer than the last one seen is ignored.
//! * Windows with a `group` or a `count`, unknown / out-of-range percents and duplicate ids are
//!   skipped. The first sighting of a window is only a baseline.
//! * Fire when the peak since the last event is ≥ 10 %, the window's last event is ≥ 15 min
//!   old, and either the reset time rolled forward (old reset passed, new one in the future and
//!   ≥ 60 s later, and later than the reset announced by the last event) or usage dropped by
//!   ≥ 10 points (with the old reset passed or unknown).
//! * Ids absent from a reading are forgotten.

use codenotch_core::model::{Reading, ReadingStatus};
use std::collections::{HashMap, HashSet};

/// Minimum peak (percent used) before a reset is worth telling.
pub const MIN_PEAK: f64 = 10.0;
/// Usage drop (points) that counts as a reset.
pub const MIN_DROP: f64 = 10.0;
/// The reset time must move forward by at least this much.
pub const MIN_ROLL_SECS: i64 = 60;
/// No second event for the same window within this delay.
pub const COOLDOWN_SECS: i64 = 15 * 60;
/// A reading older than this is not fresh for reset detection.
pub const FRESH_MAX_AGE_SECS: i64 = 5 * 60;
/// Tolerated clock skew for `fetched_at` in the future.
pub const FUTURE_SKEW_SECS: i64 = 60;

/// Whether a reading counts for reset detection: status `Fresh`, fetched within
/// [`FRESH_MAX_AGE_SECS`] and not further in the future than [`FUTURE_SKEW_SECS`].
pub fn is_fresh(reading: &Reading, now: i64) -> bool {
    reading.status == ReadingStatus::Fresh
        && reading.fetched_at > 0
        && reading.fetched_at <= now + FUTURE_SKEW_SECS
        && now - reading.fetched_at <= FRESH_MAX_AGE_SECS
}

#[derive(Debug, Clone)]
pub(crate) struct WindowWatch {
    used: f64,
    peak: f64,
    resets_at: Option<i64>,
    last_event_at: Option<i64>,
    last_event_next_reset: Option<i64>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct AccountWatch {
    last_fetched_at: i64,
    windows: HashMap<String, WindowWatch>,
}

/// One detection step for one account. Returns `(window_id, label_key, used, next_reset)`.
pub(crate) fn observe_account(
    watch: &mut AccountWatch,
    reading: &Reading,
) -> Vec<(String, String, f64, Option<i64>)> {
    let t = reading.fetched_at;
    if watch.last_fetched_at != 0 && t <= watch.last_fetched_at {
        return Vec::new();
    }
    watch.last_fetched_at = t;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut fired = Vec::new();
    for w in &reading.windows {
        if w.group.is_some() || w.count.is_some() {
            continue;
        }
        let Some(used) = w
            .used_percent
            .filter(|u| u.is_finite() && (0.0..=100.0).contains(u))
        else {
            continue;
        };
        if !seen.insert(w.id.as_str()) {
            continue;
        }
        let Some(prev) = watch.windows.get_mut(&w.id) else {
            watch.windows.insert(
                w.id.clone(),
                WindowWatch {
                    used,
                    peak: used,
                    resets_at: w.resets_at,
                    last_event_at: None,
                    last_event_next_reset: None,
                },
            );
            continue;
        };
        let elapsed = prev.resets_at.is_some_and(|r| r <= t);
        let date_rolled = elapsed
            && match (prev.resets_at, w.resets_at) {
                (Some(old), Some(new)) => {
                    new > t
                        && new - old >= MIN_ROLL_SECS
                        && prev.last_event_next_reset.is_none_or(|last| new > last)
                }
                _ => false,
            };
        let dropped = prev.used - used >= MIN_DROP && (prev.resets_at.is_none() || elapsed);
        let outside_cooldown = prev.last_event_at.is_none_or(|at| t - at >= COOLDOWN_SECS);
        if prev.peak >= MIN_PEAK && outside_cooldown && (date_rolled || dropped) {
            fired.push((w.id.clone(), w.label_key.clone(), used, w.resets_at));
            prev.peak = used;
            prev.last_event_at = Some(t);
            prev.last_event_next_reset = w.resets_at.or(prev.last_event_next_reset);
        } else {
            prev.peak = prev.peak.max(used);
        }
        prev.used = used;
        prev.resets_at = w.resets_at;
    }
    watch.windows.retain(|id, _| seen.contains(id.as_str()));
    fired
}
