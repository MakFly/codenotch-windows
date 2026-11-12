//! Time-to-exhaustion forecast from the recent history of one window.
//!
//! Method: take the samples of the window's series from the last [`LOOKBACK_SECS`], plus the
//! current value at `now`; keep only the part after the most recent reset (a drop of at least
//! [`RESET_DROP`] points, or a reset time that moved forward past its old value, or a sample
//! whose reset time belongs to another period than the window's); require at least
//! [`MIN_POINTS`] samples spanning [`MIN_SPAN_SECS`]; slope = Theil–Sen estimator (median of
//! pairwise slopes, robust to a single odd sample); no forecast unless the slope is clearly
//! positive.

use codenotch_core::history::{canonical_label, HistoryPoint};
use codenotch_core::model::LimitWindow;

/// How far back the slope looks.
pub const LOOKBACK_SECS: i64 = 60 * 60;
/// Minimum number of samples (current value included).
pub const MIN_POINTS: usize = 3;
/// Minimum time covered by the samples.
pub const MIN_SPAN_SECS: i64 = 5 * 60;
/// A fall of this many points between two samples means the window reset.
pub const RESET_DROP: f64 = 5.0;
/// Slopes below this (percent per minute, i.e. 0.6 %/h) count as flat.
pub const MIN_SLOPE_PER_MIN: f64 = 0.01;
/// Reset times further apart than this belong to different periods (providers jitter by seconds).
pub const PERIOD_TOLERANCE_SECS: i64 = 10 * 60;
/// Most samples used (pairwise slopes are O(n²)).
const MAX_POINTS: usize = 240;

#[derive(Debug, Clone, Copy)]
struct Sample {
    at: i64,
    used: f64,
    resets_at: Option<i64>,
}

/// See [`crate::forecast_minutes_left`].
pub fn minutes_left(window: &LimitWindow, history: &[HistoryPoint], now: i64) -> Option<i64> {
    if window.count.is_some() {
        return None;
    }
    let used = window.used_percent.filter(|u| u.is_finite())?;
    if used >= 100.0 {
        return Some(0);
    }
    let label = canonical_label(&window.label_key);
    let mut samples: Vec<Sample> = history
        .iter()
        .filter(|p| p.used_percent.is_finite())
        .filter(|p| p.at < now && p.at >= now - LOOKBACK_SECS)
        .filter(|p| p.canonical_label() == label)
        .filter(|p| match (p.resets_at, window.resets_at) {
            (Some(a), Some(b)) => (a - b).abs() <= PERIOD_TOLERANCE_SECS,
            _ => true,
        })
        .map(|p| Sample {
            at: p.at,
            used: p.used_percent,
            resets_at: p.resets_at,
        })
        .collect();
    samples.sort_by_key(|s| s.at);
    samples.push(Sample {
        at: now,
        used,
        resets_at: window.resets_at,
    });

    // Keep only what follows the most recent reset.
    let mut start = 0;
    for i in 1..samples.len() {
        let (a, b) = (samples[i - 1], samples[i]);
        let dropped = a.used - b.used >= RESET_DROP;
        let rolled = matches!((a.resets_at, b.resets_at), (Some(ra), Some(rb)) if ra <= b.at && rb - ra >= 60);
        if dropped || rolled {
            start = i;
        }
    }
    let mut series = &samples[start..];
    if series.len() > MAX_POINTS {
        series = &series[series.len() - MAX_POINTS..];
    }
    if series.len() < MIN_POINTS {
        return None;
    }
    let span = series[series.len() - 1].at - series[0].at;
    if span < MIN_SPAN_SECS {
        return None;
    }
    let slope = theil_sen_per_minute(series)?;
    if !(slope.is_finite() && slope >= MIN_SLOPE_PER_MIN) {
        return None;
    }
    let minutes = ((100.0 - used) / slope).ceil();
    if !minutes.is_finite() || minutes > 1.0e7 {
        return None;
    }
    Some(minutes.max(0.0) as i64)
}

/// Median of the pairwise slopes (percent per minute). Pairs closer than 30 s are skipped.
fn theil_sen_per_minute(series: &[Sample]) -> Option<f64> {
    let mut slopes = Vec::with_capacity(series.len() * series.len() / 2);
    for i in 0..series.len() {
        for j in (i + 1)..series.len() {
            let dt = series[j].at - series[i].at;
            if dt >= 30 {
                slopes.push((series[j].used - series[i].used) / (dt as f64 / 60.0));
            }
        }
    }
    if slopes.is_empty() {
        return None;
    }
    slopes.sort_by(|a, b| a.total_cmp(b));
    let n = slopes.len();
    Some(if n % 2 == 1 {
        slopes[n / 2]
    } else {
        (slopes[n / 2 - 1] + slopes[n / 2]) / 2.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::WindowKind;

    const NOW: i64 = 1_700_000_000;
    const RESET: i64 = NOW + 4 * 3600;

    fn pt(mins_ago: i64, used: f64, reset: Option<i64>) -> HistoryPoint {
        HistoryPoint {
            account_id: "a".into(),
            label: "Current session".into(),
            at: NOW - mins_ago * 60,
            used_percent: used,
            resets_at: reset,
        }
    }
    fn win(used: f64) -> LimitWindow {
        LimitWindow::new("session", "Current session", WindowKind::Session)
            .used(used)
            .resets_at(RESET)
    }

    #[test]
    fn linear_rise() {
        // 1 point per minute over 20 min: 50 → 70 (1 %/min); 30 % left → 30 min.
        let h: Vec<_> = (1..=20)
            .map(|m| pt(m, 70.0 - m as f64, Some(RESET)))
            .collect();
        assert_eq!(minutes_left(&win(70.0), &h, NOW), Some(30));
    }

    #[test]
    fn flat_and_falling_give_none() {
        let flat: Vec<_> = (1..=20).map(|m| pt(m, 40.0, Some(RESET))).collect();
        assert_eq!(minutes_left(&win(40.0), &flat, NOW), None);
        // falling slowly (no reset-sized drop between samples)
        let down: Vec<_> = (1..=20)
            .map(|m| pt(m, 40.0 + m as f64 * 0.5, Some(RESET)))
            .collect();
        assert_eq!(minutes_left(&win(40.0), &down, NOW), None);
    }

    #[test]
    fn unknown_count_and_full() {
        let h: Vec<_> = (1..=20)
            .map(|m| pt(m, 70.0 - m as f64, Some(RESET)))
            .collect();
        let unknown = LimitWindow::new("session", "Current session", WindowKind::Session);
        assert_eq!(minutes_left(&unknown, &h, NOW), None);
        let mut count = win(70.0);
        count.count = Some(3);
        assert_eq!(minutes_left(&count, &h, NOW), None);
        assert_eq!(minutes_left(&win(100.0), &h, NOW), Some(0));
    }

    #[test]
    fn too_few_or_too_short() {
        assert_eq!(minutes_left(&win(70.0), &[], NOW), None);
        let h = vec![pt(1, 69.0, Some(RESET))];
        assert_eq!(minutes_left(&win(70.0), &h, NOW), None, "two samples only");
        let h = vec![pt(2, 60.0, Some(RESET)), pt(1, 65.0, Some(RESET))];
        assert_eq!(minutes_left(&win(70.0), &h, NOW), None, "span under 5 min");
    }

    #[test]
    fn series_spanning_a_reset_uses_only_the_new_period() {
        // Before the reset: steep climb to 95 (old period). After: 2 %/min from 10 to 30.
        let old_reset = NOW - 11 * 60;
        let mut h: Vec<_> = (12..=30)
            .map(|m| pt(m, 95.0 - (m - 12) as f64 * 3.0, Some(old_reset)))
            .collect();
        h.extend((1..=10).map(|m| pt(m, 30.0 - m as f64 * 2.0, Some(RESET))));
        // 70 % left at 2 %/min → 35 min
        assert_eq!(minutes_left(&win(30.0), &h, NOW), Some(35));
        // Same with unknown reset times: the drop alone splits the series.
        let h2: Vec<_> = h
            .iter()
            .cloned()
            .map(|mut p| {
                p.resets_at = None;
                p
            })
            .collect();
        let mut w = win(30.0);
        w.resets_at = None;
        assert_eq!(minutes_left(&w, &h2, NOW), Some(35));
    }

    #[test]
    fn robust_to_one_outlier_and_ignores_other_labels_and_old_points() {
        let mut h: Vec<_> = (1..=20)
            .map(|m| pt(m, 70.0 - m as f64, Some(RESET)))
            .collect();
        h[5].used_percent = 68.0; // one glitch (+4 points, under the reset-drop size)
        h.push(HistoryPoint {
            label: "Weekly limit".into(),
            ..pt(3, 1.0, None)
        });
        h.push(pt(90, 0.0, Some(RESET))); // outside the look-back
        assert_eq!(minutes_left(&win(70.0), &h, NOW), Some(30));
    }

    #[test]
    fn legacy_labels_match() {
        let h: Vec<_> = (1..=20)
            .map(|m| HistoryPoint {
                label: "5 h".into(),
                ..pt(m, 70.0 - m as f64, Some(RESET))
            })
            .collect();
        assert_eq!(minutes_left(&win(70.0), &h, NOW), Some(30));
    }
}
