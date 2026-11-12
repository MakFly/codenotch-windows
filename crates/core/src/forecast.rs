//! When will a quota window run out? A forecast over the **whole current period** of one
//! window, from its history and (optionally) the user's usual hour-of-week activity.
//!
//! # Method
//!
//! A quota window is a counter that starts at 0 % when its period starts
//! (`resets_at − window_seconds`) and only grows until `resets_at`. So:
//!
//! 1. **Period average.** `used / elapsed` is the exact average burn rate of the period so far,
//!    history or not. Time is measured in *activity units*: with an [`ActivityProfile`] one hour
//!    counts for that hour-of-week's weight (mean 1, nights/weekends small); without one every
//!    hour counts 1. Nights and weekends then burn little in the projection.
//! 2. **Last day** (periods older than 36 h with history covering the last 24 h): the rate
//!    over the last 24 h, averaged with (1) so a change of habit mid-week shows.
//! 3. **Right now**: Theil–Sen slope (median of pairwise slopes, robust to integer-percent
//!    steps and one odd sample) over the last [`RECENT_SECS`] of the period, when there are at
//!    least [`MIN_POINTS`] samples spanning [`MIN_SPAN_SECS`]. It drives the next
//!    [`NEAR_SECS`] of the projection (a session in progress keeps going for a while; an idle
//!    user stays idle for a while), then (1)/(2) take over hour by hour until the reset.
//!
//! The projection walks local hours from the reading's time to `resets_at`, accumulating
//! burn, and records when 100 % is crossed. Resets inside the history are detected like the
//! short-horizon alert forecast does (another `resets_at`, or a drop of [`RESET_DROP`] points).
//!
//! **Basis**: [`Basis::Profile`] when a profile shaped the projection, else
//! [`Basis::Trajectory`] when history gave a recent or last-day rate, else [`Basis::Pace`] (the
//! plain period average). **Confidence** is Low for Pace, for a period less than 10 % old and
//! for a reading over an hour old; High when the history covers at least half of the elapsed
//! period with 6+ samples; Medium otherwise.
//!
//! No forecast for count-only windows, unknown percent, unknown `resets_at` or
//! `window_seconds`, a reset already due, or a reading more than [`MAX_STALE_SECS`] old.

use crate::history::{canonical_label, HistoryPoint};
use crate::model::LimitWindow;
use chrono::{Datelike, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

/// Samples whose reset time differs by more than this belong to another period.
pub const PERIOD_TOLERANCE_SECS: i64 = 10 * 60;
/// A fall of this many points between two samples means the window reset.
pub const RESET_DROP: f64 = 5.0;
/// Minimum samples (current reading included) for a recent slope.
pub const MIN_POINTS: usize = 3;
/// Minimum time the recent samples must span (integer-percent steps are too coarse below).
pub const MIN_SPAN_SECS: i64 = 30 * 60;
/// Look-back of the recent slope.
pub const RECENT_SECS: i64 = 3 * 3600;
/// How long the recent slope drives the projection before the period rate takes over.
pub const NEAR_SECS: i64 = 90 * 60;
/// No forecast from a reading older than this.
pub const MAX_STALE_SECS: i64 = 6 * 3600;
/// A reading older than this gives a Low-confidence forecast.
pub const STALE_LOW_SECS: i64 = 3600;
/// Longest period considered (monthly windows); also the history retention cap.
pub const MAX_PERIOD_SECS: i64 = 32 * 86_400;
/// Elapsed activity below this (in units ≈ hours) is treated as this much (no division blow-up
/// a few minutes into a period).
const MIN_UNITS: f64 = 0.25;
/// Most samples used by the slope (pairwise slopes are O(n²)).
const MAX_POINTS: usize = 200;

/// How much to trust a forecast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

/// What the projection is based on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Basis {
    /// Measured burn rates from the window's history (recent slope / last day).
    Trajectory,
    /// History rates spread over the user's usual hour-of-week activity.
    Profile,
    /// Period average only (`used / elapsed`): too little history.
    Pace,
}

/// Forecast for one window, computed by the engine and carried on [`LimitWindow::forecast`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Forecast {
    /// When the window reaches 100 % at this pace (unix secs). `None` = not before the reset.
    /// For a window already at 100 %: when it got there (≤ the reading time).
    pub exhaust_at: Option<i64>,
    /// Projected percent used at the reset (may exceed 100: what would have been used).
    pub at_reset_percent: f64,
    /// The window's reset time the forecast was made against.
    pub resets_at: i64,
    pub confidence: Confidence,
    pub basis: Basis,
    /// When it was computed (unix secs).
    pub computed_at: i64,
}

impl Forecast {
    /// Runs out before the reset.
    pub fn runs_out(&self) -> bool {
        self.exhaust_at.is_some_and(|t| t < self.resets_at)
    }
    /// Time between running out and the reset (seconds without quota). `None` = won't run out.
    pub fn margin_secs(&self) -> Option<i64> {
        self.exhaust_at
            .filter(|t| *t < self.resets_at)
            .map(|t| self.resets_at - t)
    }
    /// Projected percent at reset for display: 0–100 (above 100 means it runs out).
    pub fn at_reset_display(&self) -> f64 {
        if self.at_reset_percent.is_finite() {
            self.at_reset_percent.clamp(0.0, 100.0)
        } else {
            100.0
        }
    }
}

/// Usual activity per local (weekday, hour), Monday first. Weights average 1 over the week.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityProfile {
    weights: [[f64; 24]; 7],
}

impl ActivityProfile {
    /// Fewest non-zero cells for a profile worth using (a single afternoon says little).
    pub const MIN_ACTIVE_CELLS: usize = 12;
    /// Share of the weight spread evenly: quiet hours still burn a little.
    pub const FLOOR: f64 = 0.1;

    /// Every hour alike (the projection without a profile).
    pub fn uniform() -> Self {
        Self {
            weights: [[1.0; 24]; 7],
        }
    }

    /// From tokens per `[weekday][hour]` (e.g. `AnalyticsDetail::week_hour_tokens`). `None`
    /// when there is too little activity to say anything.
    pub fn from_week_hour(grid: &[[u64; 24]; 7]) -> Option<Self> {
        let total: f64 = grid.iter().flatten().map(|v| *v as f64).sum();
        let active = grid.iter().flatten().filter(|v| **v > 0).count();
        if total <= 0.0 || active < Self::MIN_ACTIVE_CELLS {
            return None;
        }
        let mean = total / 168.0;
        let mut weights = [[0.0; 24]; 7];
        for (d, row) in grid.iter().enumerate() {
            for (h, v) in row.iter().enumerate() {
                weights[d][h] = Self::FLOOR + (1.0 - Self::FLOOR) * (*v as f64 / mean);
            }
        }
        Some(Self { weights })
    }

    /// Weight of one local hour (`weekday` 0 = Monday).
    pub fn weight(&self, weekday: usize, hour: usize) -> f64 {
        self.weights[weekday.min(6)][hour.min(23)]
    }

    /// Activity units (weighted hours) between two instants, local hours of `tz`.
    pub fn units<Tz: TimeZone>(&self, from: i64, to: i64, tz: &Tz) -> f64 {
        let mut sum = 0.0;
        for_each_hour(from, to, tz, |a, b, d, h| {
            sum += self.weight(d, h) * (b - a) as f64 / 3600.0;
        });
        sum
    }
}

/// Split `[from, to)` at local hour boundaries; `f(start, end, weekday_from_monday, hour)`.
/// Each instant is mapped through `tz` on its own, so DST changes fall where they belong.
fn for_each_hour<Tz: TimeZone>(
    from: i64,
    to: i64,
    tz: &Tz,
    mut f: impl FnMut(i64, i64, usize, usize),
) {
    let mut t = from;
    while t < to {
        let Some(local) = tz.timestamp_opt(t, 0).single() else {
            // Never for UTC → local, but stay total: treat the rest as one Monday-0h block.
            f(t, to, 0, 0);
            return;
        };
        let into = i64::from(local.minute()) * 60 + i64::from(local.second());
        let next = (t + 3600 - into).min(to).max(t + 1);
        f(
            t,
            next,
            local.weekday().num_days_from_monday() as usize,
            local.hour() as usize,
        );
        t = next;
    }
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    at: i64,
    used: f64,
}

/// Forecast for `window` as read at `observed_at`, from `history` (any account/labels: only the
/// samples of this window's canonical label are used, so pass one account's points), at `now`.
/// `profile`: the provider's usual activity, local hours of `tz`.
pub fn forecast<Tz: TimeZone>(
    window: &LimitWindow,
    observed_at: i64,
    history: &[HistoryPoint],
    now: i64,
    profile: Option<&ActivityProfile>,
    tz: &Tz,
) -> Option<Forecast> {
    if window.count.is_some() {
        return None;
    }
    let used = window.used_percent.filter(|u| u.is_finite())?;
    let reset = window.resets_at?;
    let len = window.window_seconds.filter(|l| *l > 0)?;
    let obs = observed_at.min(now);
    if reset <= now || now - obs > MAX_STALE_SECS {
        return None;
    }
    let start = reset - len;
    if obs < start {
        // Reading from before this period started: nothing to say about it.
        return None;
    }

    // Samples of this period, oldest first, plus the reading itself.
    let label = canonical_label(&window.label_key);
    let mut samples: Vec<Sample> = history
        .iter()
        .filter(|p| p.used_percent.is_finite())
        .filter(|p| p.at >= start && p.at < obs)
        .filter(|p| p.canonical_label() == label)
        .filter(|p| {
            p.resets_at
                .is_none_or(|r| (r - reset).abs() <= PERIOD_TOLERANCE_SECS)
        })
        .map(|p| Sample {
            at: p.at,
            used: p.used_percent,
        })
        .collect();
    samples.sort_by_key(|s| s.at);
    samples.dedup_by_key(|s| s.at);
    samples.push(Sample { at: obs, used });
    let mut first = 0;
    for i in 1..samples.len() {
        if samples[i - 1].used - samples[i].used >= RESET_DROP {
            first = i;
        }
    }
    let series = &samples[first..];

    let make = |exhaust_at: Option<i64>, at_reset: f64, confidence, basis| Forecast {
        exhaust_at,
        at_reset_percent: at_reset,
        resets_at: reset,
        confidence,
        basis,
        computed_at: now,
    };

    if used >= 100.0 {
        let reached = series
            .iter()
            .find(|s| s.used >= 100.0)
            .map_or(obs, |s| s.at);
        return Some(make(
            Some(reached.min(obs)),
            100.0,
            Confidence::High,
            Basis::Trajectory,
        ));
    }

    let uniform = ActivityProfile::uniform();
    let shape = profile.unwrap_or(&uniform);

    // (1) period average, per activity unit.
    let elapsed_units = shape.units(start, obs, tz).max(MIN_UNITS);
    let avg = used / elapsed_units;

    // (2) last day, when the period is old enough and history covers it.
    let day_ago = obs - 86_400;
    let day_rate = (obs - start >= 36 * 3600 && series.len() >= MIN_POINTS)
        .then(|| value_at(series, day_ago))
        .flatten()
        .map(|then| (used - then).max(0.0) / shape.units(day_ago, obs, tz).max(MIN_UNITS));
    let long = match day_rate {
        Some(d) => (avg + d) / 2.0,
        None => avg,
    };

    // (3) right now, percent per wall-clock hour.
    let recent = recent_rate(series, obs);

    // Projection from the reading to the reset.
    let near_end = (obs + NEAR_SECS).min(reset);
    let mut level = used;
    let mut exhaust_at = None;
    let mut step = |a: i64, b: i64, per_hour: f64| {
        let gain = per_hour * (b - a) as f64 / 3600.0;
        if exhaust_at.is_none() && per_hour > 0.0 && level + gain >= 100.0 {
            let secs = ((100.0 - level) / per_hour * 3600.0).ceil() as i64;
            exhaust_at = Some(a + secs.clamp(0, b - a));
        }
        level += gain;
    };
    match recent {
        Some(r) => {
            step(obs, near_end, r);
            for_each_hour(near_end, reset, tz, |a, b, d, h| {
                step(a, b, long * shape.weight(d, h))
            });
        }
        None => for_each_hour(obs, reset, tz, |a, b, d, h| {
            step(a, b, long * shape.weight(d, h))
        }),
    }
    // A stale reading may project a time already gone: it runs out "now".
    let exhaust_at = exhaust_at.map(|t| t.max(now)).filter(|t| *t < reset);

    let measured = recent.is_some() || day_rate.is_some();
    let basis = match (profile.is_some(), measured) {
        (true, _) => Basis::Profile,
        (false, true) => Basis::Trajectory,
        (false, false) => Basis::Pace,
    };
    let elapsed_share = (obs - start) as f64 / len as f64;
    let coverage = (obs - series[0].at) as f64 / (obs - start).max(1) as f64;
    let confidence = if !measured || elapsed_share < 0.1 || now - obs > STALE_LOW_SECS {
        Confidence::Low
    } else if coverage >= 0.5 && series.len() >= 6 {
        Confidence::High
    } else {
        Confidence::Medium
    };
    Some(make(exhaust_at, level, confidence, basis))
}

/// Linear interpolation of the series at `at`; `None` before the first sample.
fn value_at(series: &[Sample], at: i64) -> Option<f64> {
    if series.first()?.at > at {
        return None;
    }
    let i = series.partition_point(|s| s.at <= at);
    let a = series[i - 1];
    match series.get(i) {
        Some(b) if b.at > a.at => {
            Some(a.used + (b.used - a.used) * (at - a.at) as f64 / (b.at - a.at) as f64)
        }
        _ => Some(a.used),
    }
}

/// Percent per hour over the last [`RECENT_SECS`] (≥ 0), or `None` when too few/too close
/// samples. Bounded by `(net rise + 1 point) / span` so a single integer step over a short span
/// does not read as a steep climb.
fn recent_rate(series: &[Sample], obs: i64) -> Option<f64> {
    let from = series.partition_point(|s| s.at < obs - RECENT_SECS);
    let mut part = &series[from..];
    if part.len() > MAX_POINTS {
        part = &part[part.len() - MAX_POINTS..];
    }
    if part.len() < MIN_POINTS {
        return None;
    }
    let span = part[part.len() - 1].at - part[0].at;
    if span < MIN_SPAN_SECS {
        return None;
    }
    let slope = theil_sen_per_hour(part)?;
    let net = (part[part.len() - 1].used - part[0].used).max(0.0);
    let cap = (net + 1.0) / (span as f64 / 3600.0);
    Some(slope.clamp(0.0, cap))
}

/// Median of the pairwise slopes (percent per hour). Pairs closer than 30 s are skipped.
fn theil_sen_per_hour(series: &[Sample]) -> Option<f64> {
    let mut slopes = Vec::with_capacity(series.len() * series.len() / 2);
    for i in 0..series.len() {
        for j in (i + 1)..series.len() {
            let dt = series[j].at - series[i].at;
            if dt >= 30 {
                slopes.push((series[j].used - series[i].used) / (dt as f64 / 3600.0));
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
    use crate::model::WindowKind;
    use chrono::FixedOffset;

    /// Monday 2027-01-04 00:00:00 UTC.
    const MON: i64 = 1_799_020_800;
    const H: i64 = 3600;
    const DAY: i64 = 86_400;

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }
    fn pt(label: &str, at: i64, used: f64, reset: Option<i64>) -> HistoryPoint {
        HistoryPoint {
            account_id: "a".into(),
            label: label.into(),
            at,
            used_percent: used,
            resets_at: reset,
        }
    }
    fn session(used: f64, reset: i64) -> LimitWindow {
        LimitWindow::new("session", "Current session", WindowKind::Session)
            .used(used)
            .resets_at(reset)
            .window_seconds(5 * H)
    }
    fn weekly(used: f64, reset: i64) -> LimitWindow {
        LimitWindow::new("seven_day", "Weekly limit", WindowKind::Weekly)
            .used(used)
            .resets_at(reset)
            .window_seconds(7 * DAY)
    }

    #[test]
    fn monday_constant_is_a_monday() {
        let d = utc().timestamp_opt(MON, 0).unwrap();
        assert_eq!(d.weekday(), chrono::Weekday::Mon);
        assert_eq!((d.hour(), d.minute()), (0, 0));
    }

    #[test]
    fn linear_burn_runs_out_before_reset() {
        // Session started 2 h ago at a steady 30 %/h: 60 % now, 40 % left → 80 min, while the
        // reset is 3 h away.
        let now = MON + 12 * H;
        let reset = now + 3 * H;
        let start = reset - 5 * H;
        let h: Vec<_> = (0..24)
            .map(|i| {
                let at = start + i * 5 * 60;
                pt(
                    "Current session",
                    at,
                    (at - start) as f64 / 120.0,
                    Some(reset),
                )
            })
            .collect();
        let f = forecast(&session(60.0, reset), now, &h, now, None, &utc()).unwrap();
        assert_eq!(f.basis, Basis::Trajectory);
        assert_eq!(f.confidence, Confidence::High);
        let t = f.exhaust_at.unwrap();
        assert!((t - (now + 80 * 60)).abs() <= 60, "{}", (t - now) / 60);
        assert!(f.runs_out());
        assert_eq!(f.margin_secs(), Some(reset - t));
        assert!(f.at_reset_percent > 100.0);
        assert_eq!(f.at_reset_display(), 100.0);
    }

    #[test]
    fn linear_burn_that_fits_gives_percent_at_reset() {
        // 10 %/h for 2 h → 20 %; 3 h left → 50 %.
        let now = MON + 12 * H;
        let reset = now + 3 * H;
        let start = reset - 5 * H;
        let h: Vec<_> = (0..24)
            .map(|i| {
                let at = start + i * 5 * 60;
                pt(
                    "Current session",
                    at,
                    (at - start) as f64 / 360.0,
                    Some(reset),
                )
            })
            .collect();
        let f = forecast(&session(20.0, reset), now, &h, now, None, &utc()).unwrap();
        assert_eq!(f.exhaust_at, None);
        assert!(!f.runs_out());
        assert_eq!(f.margin_secs(), None);
        assert!(
            (f.at_reset_percent - 50.0).abs() < 0.5,
            "{}",
            f.at_reset_percent
        );
    }

    #[test]
    fn too_few_samples_fall_back_to_pace_with_low_confidence() {
        // Weekly: 3.5 days in, 40 % used → pace says 80 % at reset.
        let now = MON + 3 * DAY + 12 * H;
        let reset = MON + 7 * DAY;
        let h = vec![pt("Weekly limit", now - 10 * 60, 40.0, Some(reset))];
        let f = forecast(&weekly(40.0, reset), now, &h, now, None, &utc()).unwrap();
        assert_eq!(f.basis, Basis::Pace);
        assert_eq!(f.confidence, Confidence::Low);
        assert_eq!(f.exhaust_at, None);
        assert!(
            (f.at_reset_percent - 80.0).abs() < 0.01,
            "{}",
            f.at_reset_percent
        );
        // Span under 30 min: still pace.
        let h: Vec<_> = (1..5)
            .map(|i| pt("Weekly limit", now - i * 5 * 60, 40.0, Some(reset)))
            .collect();
        let f = forecast(&weekly(40.0, reset), now, &h, now, None, &utc()).unwrap();
        assert_eq!(f.basis, Basis::Pace);
        assert_eq!(f.confidence, Confidence::Low);
    }

    #[test]
    fn profile_makes_nights_and_weekends_burn_nothing() {
        // Works Mon–Fri 09–17 only. Weekly window Mon 00:00 → next Mon 00:00.
        let mut grid = [[0u64; 24]; 7];
        for row in grid.iter_mut().take(5) {
            for cell in row.iter_mut().take(17).skip(9) {
                *cell = 1000;
            }
        }
        let p = ActivityProfile::from_week_hour(&grid).unwrap();
        // Night hours are nearly zero, work hours large; mean 1.
        assert!(p.weight(0, 3) < 0.11 && p.weight(0, 10) > 3.0);
        let mean: f64 = (0..7)
            .flat_map(|d| (0..24).map(move |h| (d, h)))
            .map(|(d, h)| p.weight(d, h))
            .sum::<f64>()
            / 168.0;
        assert!((mean - 1.0).abs() < 1e-9);

        let reset = MON + 7 * DAY;
        // Wednesday 18:00: 3 working days done, 45 % used (15 %/working day).
        let now = MON + 2 * DAY + 18 * H;
        let f = forecast(&weekly(45.0, reset), now, &[], now, Some(&p), &utc()).unwrap();
        assert_eq!(f.basis, Basis::Profile);
        // Two working days left (+ the evening/weekend floor): ≈ 75 %, well under the
        // uniform-time pace (45 / 2.75 d × 7 d ≈ 115 %).
        assert_eq!(f.exhaust_at, None);
        assert!(
            (74.0..80.0).contains(&f.at_reset_percent),
            "{}",
            f.at_reset_percent
        );
        let pace = forecast(&weekly(45.0, reset), now, &[], now, None, &utc()).unwrap();
        assert!(pace.runs_out());

        // Friday 23:00 at 90 %: the weekend only burns through the small floor weight.
        let now = MON + 4 * DAY + 23 * H;
        let f = forecast(&weekly(90.0, reset), now, &[], now, Some(&p), &utc()).unwrap();
        assert_eq!(f.exhaust_at, None, "{}", f.at_reset_percent);
        // 49 quiet hours × 0.1 = 4.9 units at 90 / 163.1 per unit ≈ +2.7.
        assert!(
            (92.0..93.5).contains(&f.at_reset_percent),
            "{}",
            f.at_reset_percent
        );
    }

    #[test]
    fn idle_now_with_profile_projects_the_next_working_hours() {
        // Same profile, Thursday 08:00 and a flat recent history (idle overnight): the recent
        // slope is 0 for 90 min, then working hours burn at the period rate.
        let mut grid = [[0u64; 24]; 7];
        for row in grid.iter_mut().take(5) {
            for cell in row.iter_mut().take(17).skip(9) {
                *cell = 1000;
            }
        }
        let p = ActivityProfile::from_week_hour(&grid).unwrap();
        let reset = MON + 7 * DAY;
        let now = MON + 3 * DAY + 8 * H;
        let h: Vec<_> = (1..=12)
            .map(|i| pt("Weekly limit", now - i * 15 * 60, 66.0, Some(reset)))
            .collect();
        let f = forecast(&weekly(66.0, reset), now, &h, now, Some(&p), &utc()).unwrap();
        // 66 % over 3 working days (22 %/day), nothing until 09:30, then Thursday's hours
        // → about 86 % on Friday morning → runs out Friday afternoon.
        let t = f.exhaust_at.expect("runs out");
        let d = utc().timestamp_opt(t, 0).unwrap();
        assert_eq!(d.weekday(), chrono::Weekday::Fri, "{d}");
        assert!((9..=17).contains(&d.hour()), "{d}");
        assert_eq!(f.confidence, Confidence::Medium);
    }

    #[test]
    fn reset_inside_the_history_uses_only_the_current_period() {
        let now = MON + 12 * H;
        let reset = now + 4 * H; // period started 1 h ago, 20 %/h since
        let start = reset - 5 * H;
        let line = |at: i64| (at - start) as f64 / 180.0;
        // Late samples of the previous session (95 %, its own reset time), then the new one.
        let old_reset = now - 30 * 60;
        let mut h: Vec<_> = (40..=58)
            .step_by(3)
            .map(|m| pt("Current session", now - m * 60, 95.0, Some(old_reset)))
            .collect();
        h.extend((1..=6).map(|i| {
            let at = now - i * 6 * 60;
            pt("Current session", at, line(at), Some(reset))
        }));
        let f = forecast(&session(20.0, reset), now, &h, now, None, &utc()).unwrap();
        // 80 % left at 20 %/h → 4 h → right at the reset.
        assert!(
            f.at_reset_percent > 97.0 && f.at_reset_percent < 103.0,
            "{}",
            f.at_reset_percent
        );
        assert_eq!(f.basis, Basis::Trajectory);
        // Unknown reset times on the samples: the 95 → 8 drop splits the series instead.
        let h2: Vec<_> = h
            .iter()
            .cloned()
            .map(|mut p| {
                p.resets_at = None;
                p
            })
            .collect();
        let g = forecast(&session(20.0, reset), now, &h2, now, None, &utc()).unwrap();
        assert!(
            (g.at_reset_percent - f.at_reset_percent).abs() < 1.0,
            "{} vs {}",
            g.at_reset_percent,
            f.at_reset_percent
        );
    }

    #[test]
    fn integer_steps_do_not_fake_a_steep_climb() {
        // 40 % for a while, one step to 41 % right before now: recent slope is bounded.
        let now = MON + 3 * DAY + 12 * H;
        let reset = MON + 7 * DAY;
        let mut h: Vec<_> = (2..=8)
            .map(|i| pt("Weekly limit", now - i * 5 * 60, 40.0, Some(reset)))
            .collect();
        h.push(pt("Weekly limit", now - 60, 41.0, Some(reset)));
        let f = forecast(&weekly(41.0, reset), now, &h, now, None, &utc()).unwrap();
        // Bound: (1 + 1) points / 40 min = 3 %/h for 90 min = +4.5 at most, then ≈ pace.
        let pace = 41.0 / (3.5 * 24.0) * (3.5 * 24.0 - 1.5);
        assert!(
            f.at_reset_percent < 41.0 + 4.5 + pace,
            "{}",
            f.at_reset_percent
        );
        assert_eq!(f.exhaust_at, None);
    }

    #[test]
    fn full_window_reports_when_it_got_there() {
        let now = MON + 12 * H;
        let reset = now + 2 * H;
        let h = vec![
            pt("Current session", now - 40 * 60, 90.0, Some(reset)),
            pt("Current session", now - 20 * 60, 100.0, Some(reset)),
        ];
        let f = forecast(&session(100.0, reset), now, &h, now, None, &utc()).unwrap();
        assert_eq!(f.exhaust_at, Some(now - 20 * 60));
        assert_eq!(f.at_reset_percent, 100.0);
        assert!(f.runs_out());
    }

    #[test]
    fn no_forecast_without_reset_length_figure_or_time() {
        let now = MON + 12 * H;
        let reset = now + 2 * H;
        let mut w = session(50.0, reset);
        w.window_seconds = None;
        assert!(forecast(&w, now, &[], now, None, &utc()).is_none());
        let mut w = session(50.0, reset);
        w.resets_at = None;
        assert!(forecast(&w, now, &[], now, None, &utc()).is_none());
        let mut w = session(50.0, reset);
        w.used_percent = None;
        assert!(forecast(&w, now, &[], now, None, &utc()).is_none());
        let mut w = session(50.0, reset);
        w.count = Some(3);
        assert!(forecast(&w, now, &[], now, None, &utc()).is_none());
        // reset already due
        assert!(forecast(&session(50.0, now), now, &[], now, None, &utc()).is_none());
        // reading too old
        assert!(forecast(&session(50.0, reset), now - 7 * H, &[], now, None, &utc()).is_none());
    }

    #[test]
    fn stale_reading_is_low_confidence() {
        let now = MON + 12 * H;
        let reset = now + 2 * H;
        let start = reset - 5 * H;
        let obs = now - 2 * H;
        let h: Vec<_> = (0..12)
            .map(|i| {
                let at = start + i * 5 * 60;
                pt(
                    "Current session",
                    at,
                    (at - start) as f64 / 360.0,
                    Some(reset),
                )
            })
            .collect();
        let f = forecast(&session(10.0, reset), obs, &h, now, None, &utc()).unwrap();
        assert_eq!(f.confidence, Confidence::Low);
    }

    #[test]
    fn other_labels_and_periods_are_ignored() {
        let now = MON + 3 * DAY + 12 * H;
        let reset = MON + 7 * DAY;
        let h: Vec<_> = (1..=20)
            .flat_map(|i| {
                [
                    pt("Current session", now - i * 600, 99.0, Some(now + H)),
                    pt("Weekly limit", now - i * 600, 50.0, Some(reset + 7 * DAY)),
                ]
            })
            .collect();
        let f = forecast(&weekly(35.0, reset), now, &h, now, None, &utc()).unwrap();
        assert_eq!(f.basis, Basis::Pace);
    }

    #[test]
    fn last_day_rate_changes_the_long_term_rate() {
        // 3.5 days in at 35 %: first 2.5 days idle-ish (5 %), last day +30 %.
        let now = MON + 3 * DAY + 12 * H;
        let reset = MON + 7 * DAY;
        let mut h = Vec::new();
        let mut t = MON + H;
        while t < now {
            let used = if t < now - DAY {
                5.0
            } else {
                5.0 + 30.0 * (t - (now - DAY)) as f64 / DAY as f64
            };
            h.push(pt("Weekly limit", t, used.floor(), Some(reset)));
            t += 30 * 60;
        }
        let f = forecast(&weekly(35.0, reset), now, &h, now, None, &utc()).unwrap();
        assert_eq!(f.basis, Basis::Trajectory);
        assert_eq!(f.confidence, Confidence::High);
        // Pace alone: 35 / 3.5 × 7 = 70 %. The last day (30 %/day) pushes it over 100.
        assert!(f.runs_out(), "{}", f.at_reset_percent);
    }

    #[test]
    fn local_hours_follow_the_offset() {
        // Only Monday 09:00 local is active. With +05:30 local time, Monday 09:00 local is
        // Monday 03:30 UTC.
        let mut grid = [[0u64; 24]; 7];
        for row in grid.iter_mut() {
            row[0] = 1; // keep it a valid profile (enough active cells)
            row[1] = 1;
        }
        grid[0][9] = 1_000_000;
        let p = ActivityProfile::from_week_hour(&grid).unwrap();
        let ist = FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
        let busy = p.weight(0, 9);
        let u = p.units(MON + 3 * H + 1800, MON + 4 * H + 1800, &ist);
        assert!((u - busy).abs() < 1e-9, "{u} vs {busy}");
        // Same instant in UTC is Monday 03:30–04:30: quiet.
        assert!(p.units(MON + 3 * H + 1800, MON + 4 * H + 1800, &utc()) < 1.0);
        // Half an hour straddling a local boundary splits into two weights.
        let half = p.units(MON + 3 * H, MON + 4 * H, &ist);
        assert!((half - (busy / 2.0 + p.weight(0, 8) / 2.0)).abs() < 1e-9);
        // A switch of offset (like DST) between two instants is honoured per instant.
        let summer = FixedOffset::east_opt(2 * 3600).unwrap();
        let winter = FixedOffset::east_opt(3600).unwrap();
        assert!(
            (p.units(MON + 7 * H, MON + 8 * H, &summer) - busy).abs() < 1e-9,
            "07:00 UTC = 09:00 at +2"
        );
        assert!(
            (p.units(MON + 8 * H, MON + 9 * H, &winter) - busy).abs() < 1e-9,
            "08:00 UTC = 09:00 at +1"
        );
        // Uniform profile = plain hours.
        assert!(
            (ActivityProfile::uniform().units(MON, MON + 5 * H + 900, &ist) - 5.25).abs() < 1e-9
        );
    }

    #[test]
    fn sparse_or_empty_grids_give_no_profile() {
        assert!(ActivityProfile::from_week_hour(&[[0; 24]; 7]).is_none());
        let mut g = [[0u64; 24]; 7];
        g[0][9] = 5;
        assert!(ActivityProfile::from_week_hour(&g).is_none());
    }

    #[test]
    fn serde_round_trip() {
        let f = Forecast {
            exhaust_at: Some(5),
            at_reset_percent: 120.0,
            resets_at: 9,
            confidence: Confidence::Medium,
            basis: Basis::Profile,
            computed_at: 1,
        };
        let s = serde_json::to_string(&f).unwrap();
        assert!(s.contains("\"basis\":\"profile\""));
        assert_eq!(serde_json::from_str::<Forecast>(&s).unwrap(), f);
    }
}
