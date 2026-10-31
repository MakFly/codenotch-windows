//! Figures for the Analytics page, from the quota history and the token buckets: limits
//! reached, time blocked, pace, when tokens are spent, and their cost at API list prices.
//! No GPUI here, so all of it is unit tested.
use crate::history::Sample;
use crate::model::{Account, Reading};
use crate::token_logs::{Bucket, Tokens};
use chrono::{Datelike, Local, TimeZone, Timelike};
use serde::Deserialize;
use std::collections::HashMap;

/// Readings further apart than this are a gap: nothing is assumed about the time between.
pub const GAP: i64 = 1800;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QuotaStats {
    /// Times the window reached 100 %.
    pub limits: u32,
    /// Seconds observed at 100 %.
    pub blocked: i64,
    /// Percentage points gained per hour in which the quota went up.
    pub per_hour: Option<f64>,
}

fn period(a: &Sample, b: &Sample) -> bool {
    crate::alerts::same_period(a.resets_at, b.resets_at)
}

/// Limits, time blocked and pace of one window since `since`. A gap longer than `GAP`, or
/// a new period, is never counted as time blocked or as consumption.
pub fn quota_stats(samples: &[Sample], since: i64) -> QuotaStats {
    let mut stats = QuotaStats::default();
    let mut rise = 0.;
    let mut active_hours = std::collections::HashSet::new();
    let mut previous: Option<&Sample> = None;
    for s in samples.iter().filter(|s| s.at >= since) {
        let same_period = previous.is_some_and(|p| period(p, s) && s.at - p.at <= GAP);
        let was_full = previous.is_some_and(|p| p.used >= 100. && period(p, s));
        if s.used >= 100. && !was_full {
            stats.limits += 1;
        }
        if let (Some(p), true) = (previous, same_period) {
            if p.used >= 100. {
                stats.blocked += s.at - p.at;
            }
            if s.used > p.used {
                rise += s.used - p.used;
                active_hours.insert(s.at / 3600);
            }
        }
        previous = Some(s);
    }
    stats.per_hour = (!active_hours.is_empty()).then(|| rise / active_hours.len() as f64);
    stats
}

/// Length of a quota window from its connector label ("5 h", "Opus · 7 jours", "Mensuel"),
/// `None` when the label does not say ("Session").
pub fn period_length(label: &str) -> Option<i64> {
    let last = label.rsplit(" · ").next().unwrap_or(label).trim();
    if last == "Mensuel" {
        return Some(30 * 86_400);
    }
    let (number, unit) = last.split_once(' ')?;
    let n: i64 = number.parse().ok()?;
    let unit_secs = match unit {
        "jours" | "jour" | "j" => 86_400,
        "h" => 3600,
        "min" => 60,
        _ => return None,
    };
    Some(n * unit_secs)
}

/// Where the current period of a window stands against an even pace from its start to its
/// reset: the line a quota spent evenly would follow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    pub start: i64,
    pub end: i64,
    /// Time of the latest reading.
    pub at: i64,
    pub used: f64,
    /// Share an even pace would have used by now, in percent.
    pub expected: f64,
    /// Usage at the reset if the average pace since the start goes on (may exceed 100).
    pub projected: f64,
    /// When that pace reaches 100 %, if before the reset.
    pub exhausts_at: Option<i64>,
}

/// The budget of the period the latest reading belongs to. Needs a reset time in the future
/// and a known window length, else the start of the period's readings stands in for it.
pub fn budget(label: &str, samples: &[Sample]) -> Option<Budget> {
    let last = samples.last()?;
    let end = last.resets_at?;
    let start = match period_length(label) {
        Some(length) => end - length,
        None => samples
            .iter()
            .rev()
            .take_while(|s| crate::alerts::same_period(s.resets_at, last.resets_at))
            .last()
            .map(|s| s.at)?,
    };
    let (at, used) = (last.at, last.used);
    if at <= start || end <= at {
        return None;
    }
    let (elapsed, length) = ((at - start) as f64, (end - start) as f64);
    let projected = used * length / elapsed;
    let exhausts_at = (used > 0. && projected > 100.)
        .then(|| start + (elapsed * 100. / used) as i64)
        .filter(|t| *t < end);
    Some(Budget { start, end, at, used, expected: 100. * elapsed / length, projected, exhausts_at })
}

/// When each new period started: the first reading after `resets_at` changed.
pub fn resets(samples: &[Sample], since: i64) -> Vec<i64> {
    samples
        .windows(2)
        .filter(|w| w[1].at >= since && !period(&w[0], &w[1]))
        .map(|w| w[1].at)
        .collect()
}

const DAY: i64 = 86_400;
/// Weight of each past period against the next more recent one.
const DECAY: f64 = 0.75;
/// Past periods a forecast from them needs.
const MIN_PERIODS: usize = 2;
/// A limit the recent pace reaches this soon outweighs the long-run forecast.
const SOON: i64 = 7200;
/// Share of a day's usage every hour gets anyway in the active-hours profile.
const FLOOR: f64 = 0.1 / 24.;

/// How the usage at the reset was estimated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Basis {
    /// The pace of the last hour: windows shorter than a day.
    RecentPace,
    /// What the same stretch of the past periods added, the most recent counting most.
    PastPeriods(usize),
    /// The average pace since the start, spent over the hours of the day the quota is
    /// usually used in.
    ActiveHours,
    /// The average pace since the start of the period.
    AveragePace,
}

/// Where a window is likely to stand at its reset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outlook {
    pub budget: Budget,
    /// Usage expected at the reset, in percent (may exceed 100).
    pub expected: f64,
    /// Lowest and highest usage at the reset the past periods lead to.
    pub range: Option<(f64, f64)>,
    /// When usage is expected to reach 100 %, if before the reset.
    pub exhausts_at: Option<i64>,
    pub basis: Basis,
    /// `exhausts_at` comes from the pace of the last hour, which reaches the limit soon.
    pub soon: bool,
}

/// A window of a day or more, forecast from the history rather than the last hour.
pub fn long_window(label: &str) -> bool {
    period_length(label).is_some_and(|l| l >= DAY)
}

/// The usage at the reset of the window's current period and when it reaches 100 %:
/// windows under a day go on at the pace of the last hour; longer ones add what the same
/// stretch of the past periods added, else spread the average pace over the hours of the
/// day the quota is usually used in. A limit the last hour's pace reaches within two hours
/// wins over the long-run forecast.
pub fn outlook(label: &str, samples: &[Sample], now: i64) -> Option<Outlook> {
    outlook_with(label, samples, now, &|t| {
        Local.timestamp_opt(t, 0).single().map_or(0, |d| d.hour() as usize)
    })
}

fn outlook_with(label: &str, samples: &[Sample], now: i64, hour_of: &dyn Fn(i64) -> usize) -> Option<Outlook> {
    let budget = budget(label, samples)?;
    let periods = split_periods(samples);
    let current: Vec<(i64, f64)> = periods.last()?.iter().map(|s| (s.at, s.used)).collect();
    let recent = crate::alerts::forecast(&current, now).filter(|t| *t < budget.end);
    let average = Outlook {
        budget,
        expected: budget.projected,
        range: None,
        exhausts_at: budget.exhausts_at,
        basis: Basis::AveragePace,
        soon: false,
    };
    if !long_window(label) {
        return Some(match crate::alerts::pace(&current, now) {
            Some(slope) => Outlook {
                expected: budget.used + slope.max(0.) * (budget.end - budget.at) as f64,
                exhausts_at: recent,
                basis: Basis::RecentPace,
                ..average
            },
            None => average,
        });
    }
    let length = period_length(label)?;
    let mut outlook = past_periods(budget, &periods, length)
        .or_else(|| active_hours(budget, samples, hour_of))
        .unwrap_or(average);
    if let Some(at) = recent.filter(|t| t - budget.at <= SOON) {
        if outlook.exhausts_at.is_none_or(|planned| at < planned) {
            outlook.exhausts_at = Some(at);
            outlook.soon = true;
        }
    }
    Some(outlook)
}

/// The samples cut into periods, oldest first.
fn split_periods(samples: &[Sample]) -> Vec<&[Sample]> {
    let mut out = vec![];
    let mut start = 0;
    for i in 1..=samples.len() {
        if i == samples.len() || !period(&samples[i - 1], &samples[i]) {
            out.push(&samples[start..i]);
            start = i;
        }
    }
    out
}

/// Usage at `t` between readings, `None` when the readings around `t` are further apart
/// than `tolerance`. Past the last reading, the last value.
fn usage_at(points: &[(i64, f64)], t: i64, tolerance: i64) -> Option<f64> {
    let after = points.partition_point(|p| p.0 < t);
    if after == points.len() {
        return points.last().map(|p| p.1);
    }
    let b = points[after];
    if b.0 == t || after == 0 {
        return (b.0 == t).then_some(b.1);
    }
    let a = points[after - 1];
    (b.0 - a.0 <= tolerance).then(|| a.1 + (b.1 - a.1) * (t - a.0) as f64 / (b.0 - a.0) as f64)
}

/// The forecast from what the same stretch of the past periods added: from the point as far
/// into each of them as now is into the current one, to its reset.
fn past_periods(budget: Budget, periods: &[&[Sample]], length: i64) -> Option<Outlook> {
    let offset = budget.at - budget.start;
    let remaining = budget.end - budget.at;
    // Readings this far apart still say how usage went between them; a whole day missing
    // from a week does not.
    let tolerance = length / 14;
    // (readings with the period's 0 % start, usage at the matching point), recent first.
    let mut past: Vec<(Vec<(i64, f64)>, i64, f64)> = vec![];
    for period in periods.iter().rev() {
        let Some(end) = period.last().and_then(|s| s.resets_at) else {
            continue;
        };
        // The current period, or one that does not end before it starts.
        if end > budget.start + 300 {
            continue;
        }
        let start = end - length;
        let mut points = vec![(start, 0.)];
        points.extend(period.iter().filter(|s| s.at > start && s.at <= end).map(|s| (s.at, s.used)));
        // Its end was not seen: what it added after the last reading is unknown.
        if points.len() < 2 || end - points[points.len() - 1].0 > tolerance {
            continue;
        }
        let from = start + offset;
        if let Some(at_from) = usage_at(&points, from, tolerance) {
            past.push((points, from, at_from));
        }
    }
    if past.len() < MIN_PERIODS {
        return None;
    }
    let weights: Vec<f64> = (0..past.len()).map(|i| DECAY.powi(i as i32)).collect();
    let total: f64 = weights.iter().sum();
    // What the past periods added `tau` seconds past the matching point, weighed.
    let added = |tau: i64| -> f64 {
        past.iter()
            .zip(&weights)
            .map(|((points, from, at_from), w)| {
                let now = usage_at(points, from + tau, i64::MAX).unwrap_or(*at_from);
                w * (now - at_from).max(0.)
            })
            .sum::<f64>()
            / total
    };
    let finals: Vec<f64> = past
        .iter()
        .map(|(points, _, at_from)| (points[points.len() - 1].1 - at_from).max(0.))
        .collect();
    let lowest = finals.iter().copied().fold(f64::INFINITY, f64::min);
    let highest = finals.iter().copied().fold(0., f64::max);
    let expected = budget.used + added(remaining);
    let exhausts_at = (budget.used < 100. && expected >= 100.)
        .then(|| {
            let step = (remaining / 336).max(60);
            (1..)
                .map(|i| (i * step).min(remaining))
                .find(|tau| budget.used + added(*tau) >= 100. || *tau == remaining)
                .map(|tau| budget.at + tau)
        })
        .flatten()
        .filter(|at| *at < budget.end);
    Some(Outlook {
        budget,
        expected,
        range: Some((budget.used + lowest, budget.used + highest)),
        exhausts_at,
        basis: Basis::PastPeriods(past.len()),
        soon: false,
    })
}

/// The average pace since the start of the period, counted only over the hours of the day
/// the quota usually goes up in: a night weighs little, a working afternoon a lot. Learnt
/// from every rise between readings close enough together to place it in time.
fn active_hours(budget: Budget, samples: &[Sample], hour_of: &dyn Fn(i64) -> usize) -> Option<Outlook> {
    let mut rises = [0f64; 24];
    for pair in samples.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if period(&a, &b) && b.at - a.at <= GAP && b.used > a.used {
            rises[hour_of(b.at)] += b.used - a.used;
        }
    }
    let total: f64 = rises.iter().sum();
    let span = samples.last()?.at - samples.first()?.at;
    // Too little seen to know the hours.
    if total < 5. || span < DAY {
        return None;
    }
    let share: Vec<f64> = rises.iter().map(|r| r / total + FLOOR).collect();
    // The shares of the hours between two times, per hour of the day.
    let weight = |from: i64, to: i64| -> f64 {
        let mut t = from;
        let mut sum = 0.;
        while t < to {
            let next = (t - t.rem_euclid(3600) + 3600).min(to);
            sum += share[hour_of(t)] * (next - t) as f64 / 3600.;
            t = next;
        }
        sum
    };
    let elapsed = weight(budget.start, budget.at);
    if elapsed <= 0. {
        return None;
    }
    let rate = budget.used / elapsed;
    let expected = budget.used + rate * weight(budget.at, budget.end);
    let mut exhausts_at = None;
    if budget.used < 100. && expected >= 100. {
        let mut used = budget.used;
        let mut t = budget.at;
        while t < budget.end {
            let next = (t - t.rem_euclid(3600) + 3600).min(budget.end);
            let gain = rate * share[hour_of(t)] * (next - t) as f64 / 3600.;
            if used + gain >= 100. {
                exhausts_at = Some(t + ((100. - used) / gain * (next - t) as f64) as i64);
                break;
            }
            used += gain;
            t = next;
        }
    }
    Some(Outlook {
        budget,
        expected,
        range: None,
        exhausts_at: exhausts_at.filter(|at| *at < budget.end),
        basis: Basis::ActiveHours,
        soon: false,
    })
}

/// Tokens per local weekday (Monday first) and hour.
pub fn heatmap<'a>(buckets: impl Iterator<Item = &'a Bucket>) -> [[u64; 24]; 7] {
    let mut grid = [[0; 24]; 7];
    for b in buckets {
        if let Some(t) = Local.timestamp_opt(b.hour, 0).single() {
            grid[t.weekday().num_days_from_monday() as usize][t.hour() as usize] += b.tokens.total();
        }
    }
    grid
}

/// Tokens per local day, oldest first, every day of the span included.
pub fn daily<'a>(buckets: impl Iterator<Item = &'a Bucket>, days: u16, now: i64) -> Vec<(chrono::NaiveDate, Tokens)> {
    let today = Local.timestamp_opt(now, 0).single().map(|t| t.date_naive()).unwrap_or_default();
    let mut out: Vec<(chrono::NaiveDate, Tokens)> = (0..days as i64)
        .rev()
        .map(|d| (today - chrono::Duration::days(d), Tokens::default()))
        .collect();
    for b in buckets {
        let Some(day) = Local.timestamp_opt(b.hour, 0).single().map(|t| t.date_naive()) else {
            continue;
        };
        if let Some((_, t)) = out.iter_mut().find(|(d, _)| *d == day) {
            *t += b.tokens;
        }
    }
    out
}

/// Tokens grouped by `key` (a model, a project), most used first.
pub fn ranked<'a>(buckets: impl Iterator<Item = &'a Bucket>, key: impl Fn(&Bucket) -> String) -> Vec<(String, Tokens)> {
    let mut map: HashMap<String, Tokens> = HashMap::new();
    for b in buckets {
        *map.entry(key(b)).or_default() += b.tokens;
    }
    let mut out: Vec<_> = map.into_iter().collect();
    out.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then(a.0.cmp(&b.0)));
    out
}

/// The folder name of a project path, for display; the full path goes in a tooltip.
pub fn project_name(path: &str) -> String {
    let trimmed = path.trim_end_matches(['\\', '/']);
    trimmed
        .rsplit(['\\', '/'])
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or(trimmed)
        .to_owned()
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub prefix: String,
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Pricing {
    pub models: Vec<Price>,
}

impl Pricing {
    fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    /// The bundled list prices, completed or overridden, model by model, by a
    /// `pricing.json` next to the configuration.
    pub fn load() -> Self {
        let mut pricing = Self::parse(include_str!("../assets/pricing.json")).unwrap_or_default();
        let local = crate::store::config_path()
            .map(|p| p.with_file_name("pricing.json"))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| Self::parse(&t));
        if let Some(local) = local {
            pricing.merge(local);
        }
        pricing
    }

    fn merge(&mut self, other: Pricing) {
        for price in other.models {
            self.models.retain(|p| p.prefix != price.prefix);
            self.models.push(price);
        }
    }

    /// The price of the longest prefix of `model`, if any.
    pub fn find(&self, model: &str) -> Option<&Price> {
        self.models
            .iter()
            .filter(|p| model.starts_with(&p.prefix))
            .max_by_key(|p| p.prefix.len())
    }

    /// What `tokens` cost at API list prices, in dollars; `None` for an unknown model,
    /// never zero.
    pub fn cost_usd(&self, tokens: &Tokens, model: &str) -> Option<f64> {
        let p = self.find(model)?;
        let m = |n: u64, price: f64| n as f64 / 1_000_000. * price;
        Some(
            m(tokens.input, p.input)
                + m(tokens.output, p.output)
                + m(tokens.cache_write, p.cache_write)
                + m(tokens.cache_read, p.cache_read),
        )
    }
}

/// Cost of buckets in dollars, and the tokens of models the price list does not know.
pub fn cost<'a>(buckets: impl Iterator<Item = &'a Bucket>, pricing: &Pricing) -> (f64, u64) {
    let (mut usd, mut unpriced) = (0., 0);
    for b in buckets {
        match pricing.cost_usd(&b.tokens, &b.model) {
            Some(c) => usd += c,
            None => unpriced += b.tokens.total(),
        }
    }
    (usd, unpriced)
}

/// A monthly subscription price, in dollars, guessed from the plan the provider reports:
/// Codex's `planType`, or the subscription Claude Code saved with its sign-in (read
/// without the token).
pub fn suggested_plan_usd(account: &Account, reading: Option<&Reading>) -> Option<f64> {
    match account.provider.as_str() {
        "codex" => match reading?.plan.as_deref()?.to_lowercase().as_str() {
            "plus" => Some(20.),
            "pro" => Some(200.),
            _ => None,
        },
        "claude" => {
            let profile = if account.profile_path.is_empty() {
                crate::login::default_profile("claude")
            } else {
                std::path::PathBuf::from(&account.profile_path)
            };
            let text = std::fs::read_to_string(profile.join(".credentials.json")).ok()?;
            let value: serde_json::Value = serde_json::from_str(&text).ok()?;
            let oauth = &value["claudeAiOauth"];
            claude_plan_usd(oauth["subscriptionType"].as_str()?, oauth["rateLimitTier"].as_str().unwrap_or(""))
        }
        _ => None,
    }
}

fn claude_plan_usd(subscription: &str, tier: &str) -> Option<f64> {
    match subscription {
        "pro" => Some(20.),
        "max" if tier.contains("20x") => Some(200.),
        "max" => Some(100.),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_logs::Cli;

    fn s(at: i64, used: f64, resets_at: i64) -> Sample {
        Sample { at, used, resets_at: Some(resets_at) }
    }

    #[test]
    fn limits_blocked_time_and_pace_ignore_gaps_and_new_periods() {
        let samples = [
            s(0, 50., 10_000),
            s(600, 80., 10_000),     // +30 within the hour
            s(1200, 100., 10_000),   // limit 1, +20
            s(1800, 100., 10_000),   // blocked 600
            s(9000, 100., 10_000),   // after a 2 h gap: not counted as blocked
            s(9600, 5., 20_000),     // new period
            s(10_200, 100., 20_000), // limit 2, +95
        ];
        let stats = quota_stats(&samples, 0);
        assert_eq!(stats.limits, 2);
        assert_eq!(stats.blocked, 600);
        // 50 points over hour 0, 95 over hour 2.
        assert_eq!(stats.per_hour, Some((30. + 20. + 95.) / 2.));
        assert_eq!(resets(&samples, 0), vec![9600]);
        // A reset time drifting by a second is the same period.
        assert!(resets(&[s(0, 1., 10_000), s(60, 2., 9_999), s(120, 3., 10_000)], 0).is_empty());
        // Only what is in the span.
        assert_eq!(quota_stats(&samples, 9000).limits, 2);
        assert_eq!(quota_stats(&[], 0), QuotaStats::default());
    }

    #[test]
    fn budgets_compare_usage_with_an_even_pace() {
        assert_eq!(period_length("5 h"), Some(5 * 3600));
        assert_eq!(period_length("Opus · 7 jours"), Some(7 * 86_400));
        assert_eq!(period_length("Mensuel"), Some(30 * 86_400));
        assert_eq!(period_length("Session"), None);
        // 7-day window, 3.5 days in, 70 % used: 20 points over an even pace.
        let end = 7 * 86_400;
        let at = end / 2;
        let b = budget("7 jours", &[s(at - 600, 69., end), s(at, 70., end)]).unwrap();
        assert_eq!(b.start, 0);
        assert!((b.expected - 50.).abs() < 1e-9);
        assert!((b.projected - 140.).abs() < 1e-9);
        // 100 % at 5 days at that pace.
        assert_eq!(b.exhausts_at, Some(5 * 86_400));
        // Under the pace: no limit before the reset.
        let b = budget("7 jours", &[s(at, 30., end)]).unwrap();
        assert!((b.projected - 60.).abs() < 1e-9);
        assert_eq!(b.exhausts_at, None);
        // Unknown length: the period's first reading is its start.
        let b = budget("Session", &[s(100, 5., 9_000), s(1_000, 10., 9_000)]).unwrap();
        assert_eq!(b.start, 100);
        // A reset already past, or no reset time: nothing to compare.
        assert!(budget("5 h", &[s(10_000, 5., 9_000)]).is_none());
        assert!(budget("5 h", &[Sample { at: 1, used: 1., resets_at: None }]).is_none());
    }

    /// Hour of the day in UTC, so the tests do not depend on the machine's time zone.
    fn utc_hour(t: i64) -> usize {
        (t.rem_euclid(DAY) / 3600) as usize
    }

    /// A week of hourly readings ending at `end`: 0 to `before` over six days, then
    /// `last_day` more over the seventh.
    fn week(end: i64, before: f64, last_day: f64) -> Vec<Sample> {
        let start = end - 7 * DAY;
        (1..=168)
            .map(|h| {
                let tau = h * 3600;
                let used = if tau <= 6 * DAY {
                    before * tau as f64 / (6 * DAY) as f64
                } else {
                    before + last_day * (tau - 6 * DAY) as f64 / DAY as f64
                };
                s(start + tau, used, end)
            })
            .collect()
    }

    #[test]
    fn long_windows_add_what_the_past_weeks_added() {
        let w = 7 * DAY;
        // Three past weeks whose last day added 10, 10 then 20 points.
        let mut samples = [week(w, 60., 10.), week(2 * w, 60., 10.), week(3 * w, 60., 20.)].concat();
        // This week: 80 % after six days.
        let current: Vec<Sample> = week(4 * w, 80., 0.).into_iter().filter(|p| p.at <= 3 * w + 6 * DAY).collect();
        samples.extend(current);
        let now = 3 * w + 6 * DAY;
        let o = outlook_with("7 jours", &samples, now, &utc_hour).unwrap();
        assert_eq!(o.basis, Basis::PastPeriods(3));
        // The most recent week counts most: (20 + 0.75 × 10 + 0.5625 × 10) / 2.3125.
        assert!((o.expected - (80. + 33.125 / 2.3125)).abs() < 1e-6, "{}", o.expected);
        let (low, high) = o.range.unwrap();
        assert!((low - 90.).abs() < 1e-6 && (high - 100.).abs() < 1e-6);
        assert_eq!(o.exhausts_at, None);

        // At 95 %, the same last day crosses 100 % about a third of the way through.
        samples.retain(|p| p.at <= 3 * w);
        samples.extend(week(4 * w, 95., 0.).into_iter().filter(|p| p.at <= now));
        let o = outlook_with("7 jours", &samples, now, &utc_hour).unwrap();
        let at = o.exhausts_at.unwrap();
        let expected = now + (DAY as f64 * 5. / (33.125 / 2.3125)) as i64;
        assert!((at - expected).abs() <= 7 * DAY / 336, "{at} vs {expected}");
        assert!(!o.soon);
    }

    #[test]
    fn without_past_weeks_nights_do_not_count() {
        // Three days of a week: 24 points a day, all between 9 and 18 h.
        let end = 7 * DAY;
        let mut samples = vec![];
        let mut used = 0.;
        // A reading every half hour: closer than `GAP`, so each rise is placed in time.
        for k in 1..=(2 * 48 + 44) {
            let at = k * 1800;
            if (10..=17).contains(&utc_hour(at)) {
                used += 1.5;
            }
            samples.push(s(at, used, end));
        }
        let now = samples.last().unwrap().at;
        assert!((used - 72.).abs() < 1e-9);
        let o = outlook_with("7 jours", &samples, now, &utc_hour).unwrap();
        assert_eq!(o.basis, Basis::ActiveHours);
        // 28 points to go at 24 a working day: early in the fifth day’s work, not in the
        // middle of a night as an even pace would say.
        let at = o.exhausts_at.unwrap();
        assert!((9..=17).contains(&utc_hour(at)), "{}", utc_hour(at));
        assert_eq!(at / DAY, 4);
        let average = budget("7 jours", &samples).unwrap().exhausts_at.unwrap();
        assert!(!(9..=17).contains(&utc_hour(average)));
    }

    #[test]
    fn short_windows_and_imminent_limits_follow_the_last_hour() {
        // 5 h window: +1 point a minute over the last 20 minutes.
        let end = 5 * 3600;
        let samples: Vec<Sample> = (0..=20).map(|m| s(3600 + m * 60, 50. + m as f64, end)).collect();
        let now = 3600 + 20 * 60;
        let o = outlook_with("5 h", &samples, now, &utc_hour).unwrap();
        assert_eq!(o.basis, Basis::RecentPace);
        assert!((o.expected - (70. + (end - now) as f64 / 60.)).abs() < 1e-6);
        assert_eq!(o.exhausts_at, Some(now + 30 * 60));
        // On a week, a burst that reaches the limit within the hour wins over past weeks.
        let w = 7 * DAY;
        let mut samples = [week(w, 60., 10.), week(2 * w, 60., 10.)].concat();
        let now = 2 * w + 6 * DAY;
        samples.extend(week(3 * w, 80., 0.).into_iter().filter(|p| p.at < now - 1200));
        samples.extend((0..=20).map(|m| s(now - 1200 + m * 60, 80. + m as f64 * 0.5, 3 * w)));
        let o = outlook_with("7 jours", &samples, now, &utc_hour).unwrap();
        assert_eq!(o.basis, Basis::PastPeriods(2));
        assert!(o.soon);
        assert!(o.exhausts_at.unwrap() - now <= 3600);
    }

    fn bucket(hour: i64, model: &str, project: &str, output: u64) -> Bucket {
        Bucket {
            hour,
            source: "s".into(),
            cli: Cli::Claude,
            model: model.into(),
            project: project.into(),
            tokens: Tokens { output, ..Default::default() },
        }
    }

    #[test]
    fn heatmap_daily_and_rankings_use_local_time() {
        let at = Local.with_ymd_and_hms(2026, 10, 27, 15, 0, 0).unwrap().timestamp(); // a Tuesday
        let buckets = [bucket(at, "m1", r"C:\dev\app\", 10), bucket(at, "m2", "/home/x/api", 30)];
        let grid = heatmap(buckets.iter());
        assert_eq!(grid[1][15], 40);
        let days = daily(buckets.iter(), 7, at + 3 * 86_400);
        assert_eq!(days.len(), 7);
        assert_eq!(days[3].1.output, 40);
        let models = ranked(buckets.iter(), |b| b.model.clone());
        assert_eq!(models[0].0, "m2");
        assert_eq!(project_name(r"C:\dev\app\"), "app");
        assert_eq!(project_name("/home/x/api"), "api");
    }

    #[test]
    fn prices_match_the_longest_prefix_and_unknown_models_stay_unpriced() {
        let mut pricing = Pricing::parse(include_str!("../assets/pricing.json")).unwrap();
        let t = Tokens { input: 1_000_000, output: 1_000_000, cache_write: 0, cache_read: 1_000_000 };
        assert_eq!(pricing.find("claude-opus-4-1-20250805").unwrap().input, 15.);
        assert_eq!(pricing.find("claude-opus-4-5").unwrap().input, 5.);
        assert!((pricing.cost_usd(&t, "claude-opus-5-5").unwrap() - 24.2).abs() < 1e-9);
        assert_eq!(pricing.cost_usd(&t, "gpt-6.1-sol"), None);
        let buckets = [bucket(0, "claude-opus-5-5", "", 1_000_000), bucket(0, "gpt-6.1-sol", "", 7)];
        let (usd, unpriced) = cost(buckets.iter(), &pricing);
        assert!((usd - 20.).abs() < 1e-9);
        assert_eq!(unpriced, 7);
        // A local file adds a model and overrides another.
        pricing.merge(Pricing::parse(r#"{"models":[{"prefix":"gpt-6.1","input":2,"output":8,"cacheWrite":2,"cacheRead":0.2},{"prefix":"claude-opus-5-5","input":1,"output":1,"cacheWrite":1,"cacheRead":1}]}"#).unwrap());
        assert!(pricing.cost_usd(&t, "gpt-6.1-sol").is_some());
        assert_eq!(pricing.find("claude-opus-5-5").unwrap().input, 1.);
    }

    #[test]
    fn plans_suggest_a_price() {
        assert_eq!(claude_plan_usd("max", "default_claude_max_20x"), Some(200.));
        assert_eq!(claude_plan_usd("max", "default_claude_max_5x"), Some(100.));
        assert_eq!(claude_plan_usd("pro", ""), Some(20.));
        assert_eq!(claude_plan_usd("free", ""), None);
    }
}
