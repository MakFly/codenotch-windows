//! Detailed analytics tables for the statistics page (filled by `codenotch-analytics`).
//!
//! [`AnalyticsDetail`] is a compact, sparse fact table the UI filters client-side by provider
//! and model without asking the engine to recompute anything:
//!
//! * [`UsageFact`]: one row per (local date, provider, model) with usage, over
//!   [`AnalyticsDetail::facts_from`] ..= [`AnalyticsDetail::today`] — enough for the selected
//!   period, the previous period of the same length and the 53-week heatmap.
//! * [`HourCell`]: weekday × local hour activity per (provider, model) over the selected period.
//! * [`SessionFact`]: one row per transcript file with the (date, model) pairs it was active
//!   on, for exact distinct session counts under any filter / date window.
//!
//! Dates are local `YYYY-MM-DD` strings (they compare correctly as strings). Costs cover priced
//! tokens only: when `unpriced_tokens > 0` a cost is a lower bound.
//!
//! The detail is shared behind an `Arc` ([`SharedDetail`]) so cloning or comparing snapshots
//! stays cheap.

use std::collections::{BTreeMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;

use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::model::{ProviderKind, TokenCounts};

/// Average month length used to pro-rate monthly plan prices (365.2425 / 12 days).
pub const DAYS_PER_MONTH: f64 = 30.436_875;

/// Days covered by the activity heatmap: 53 weeks ending today, so any Monday- or
/// Sunday-aligned 53-column grid fits.
pub const HEATMAP_DAYS: u32 = 371;

/// Days of facts kept for a period of `period_days`: the period, the previous period of the
/// same length, and at least the heatmap.
pub fn fact_days(period_days: u32) -> u32 {
    period_days.saturating_mul(2).max(HEATMAP_DAYS)
}

/// Usage of one provider × model on one local day.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageFact {
    /// Local date `YYYY-MM-DD`.
    pub date: String,
    pub provider: ProviderKind,
    /// Model id as written by the CLI (`-fast` appended for Claude fast-mode turns).
    pub model: String,
    /// input / output / cacheRead / cacheWrite (cacheWrite = all cache writes).
    pub tokens: TokenCounts,
    /// USD cost of the priced tokens only.
    pub cost_usd: f64,
    /// Tokens of this row without a known price (all of them for an unpriced model).
    pub unpriced_tokens: u64,
    /// Distinct transcript files with usage of this provider × model on this day.
    pub sessions: u32,
}

/// Activity of one provider × model in one (weekday, local hour) cell over the selected period.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HourCell {
    pub provider: ProviderKind,
    pub model: String,
    /// 0 = Monday … 6 = Sunday (local time).
    pub weekday: u8,
    /// Local hour 0..=23.
    pub hour: u8,
    /// All tokens (input + output + cache read + cache write).
    pub tokens: u64,
    pub cost_usd: f64,
    pub unpriced_tokens: u64,
}

/// One (local date, model) a session was active on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionActivity {
    pub date: String,
    pub model: String,
}

/// One session (transcript file) with usage in the fact range.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionFact {
    pub provider: ProviderKind,
    /// Sorted, distinct.
    pub activity: Vec<SessionActivity>,
}

/// Which rows to keep. An empty list means "all".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageFilter {
    pub providers: Vec<ProviderKind>,
    pub models: Vec<String>,
}

impl UsageFilter {
    /// Everything.
    pub fn all() -> Self {
        Self::default()
    }
    pub fn provider(p: ProviderKind) -> Self {
        Self {
            providers: vec![p],
            models: Vec::new(),
        }
    }
    pub fn model(provider: ProviderKind, model: impl Into<String>) -> Self {
        Self {
            providers: vec![provider],
            models: vec![model.into()],
        }
    }
    pub fn matches(&self, provider: &ProviderKind, model: &str) -> bool {
        (self.providers.is_empty() || self.providers.contains(provider))
            && (self.models.is_empty() || self.models.iter().any(|m| m == model))
    }
}

/// Sum of facts under a filter and date window.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UsageTotals {
    pub tokens: TokenCounts,
    /// USD cost of the priced tokens only (lower bound when `unpriced_tokens > 0`).
    pub cost_usd: f64,
    pub unpriced_tokens: u64,
    /// Distinct sessions active in the window (filled by [`AnalyticsDetail::totals`] and
    /// [`AnalyticsDetail::daily`]; 0 in series values).
    pub sessions: u32,
}

impl UsageTotals {
    pub fn add_fact(&mut self, f: &UsageFact) {
        self.tokens.add(&f.tokens);
        self.cost_usd += f.cost_usd;
        self.unpriced_tokens += f.unpriced_tokens;
    }
    pub fn fully_priced(&self) -> bool {
        self.unpriced_tokens == 0
    }
}

/// How [`AnalyticsDetail::series`] splits the stacked chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesBy {
    Provider,
    Model,
}

/// One stacked-chart series: `values[i]` is the day `dates[i]` of the requested window.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Series {
    pub provider: ProviderKind,
    /// `None` when split by provider.
    pub model: Option<String>,
    /// Totals over the window (sessions not filled).
    pub total: UsageTotals,
    pub values: Vec<UsageTotals>,
}

/// The statistics page's tables. See the module docs.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnalyticsDetail {
    /// Local date of the run (last day of every window).
    pub today: String,
    /// First day of the selected period (`today − days + 1`).
    pub period_from: String,
    /// The previous period of the same length: `previous_from ..= previous_to`
    /// (`previous_to` = the day before `period_from`).
    pub previous_from: String,
    pub previous_to: String,
    /// First day of the heatmap (`today − HEATMAP_DAYS + 1`), independent of the period.
    pub heatmap_from: String,
    /// Oldest day covered by `facts` / `sessions` (`today − fact_days(days) + 1`).
    pub facts_from: String,
    /// Sparse (no zero rows); sorted by date, provider (canonical order), model.
    pub facts: Vec<UsageFact>,
    /// Over the selected period only; sparse; sorted by provider, model, weekday, hour.
    pub hours: Vec<HourCell>,
    /// Every session with usage in the fact range.
    pub sessions: Vec<SessionFact>,
    /// Model ids seen in the selected period without a price.
    pub unpriced_models: Vec<String>,
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// Every date `from ..= to` (`YYYY-MM-DD`), oldest first; empty when invalid or reversed.
pub fn dates_between(from: &str, to: &str) -> Vec<String> {
    let (Some(a), Some(b)) = (parse_date(from), parse_date(to)) else {
        return Vec::new();
    };
    let n = (b - a).num_days();
    if n < 0 {
        return Vec::new();
    }
    (0..=n)
        .map(|i| (a + Duration::days(i)).format("%Y-%m-%d").to_string())
        .collect()
}

fn provider_rank(p: &ProviderKind) -> usize {
    ProviderKind::CANONICAL_ORDER
        .iter()
        .position(|k| k == p)
        .unwrap_or(usize::MAX)
}

impl AnalyticsDetail {
    fn in_window<'a>(
        &'a self,
        filter: &'a UsageFilter,
        from: &'a str,
        to: &'a str,
    ) -> impl Iterator<Item = &'a UsageFact> + 'a {
        self.facts.iter().filter(move |f| {
            f.date.as_str() >= from
                && f.date.as_str() <= to
                && filter.matches(&f.provider, &f.model)
        })
    }

    /// Distinct sessions with matching activity in `from ..= to`.
    pub fn session_count(&self, filter: &UsageFilter, from: &str, to: &str) -> u32 {
        self.sessions
            .iter()
            .filter(|s| {
                s.activity.iter().any(|a| {
                    a.date.as_str() >= from
                        && a.date.as_str() <= to
                        && filter.matches(&s.provider, &a.model)
                })
            })
            .count() as u32
    }

    /// Totals of `from ..= to` under `filter` (sessions = distinct sessions).
    pub fn totals(&self, filter: &UsageFilter, from: &str, to: &str) -> UsageTotals {
        let mut t = UsageTotals::default();
        for f in self.in_window(filter, from, to) {
            t.add_fact(f);
        }
        t.sessions = self.session_count(filter, from, to);
        t
    }

    /// Totals of the selected period.
    pub fn period_totals(&self, filter: &UsageFilter) -> UsageTotals {
        self.totals(filter, &self.period_from, &self.today)
    }

    /// Totals of the previous period of the same length (for deltas). Transcripts that old may
    /// no longer exist (Claude Code prunes them), so a zero here can mean "no data".
    pub fn previous_totals(&self, filter: &UsageFilter) -> UsageTotals {
        self.totals(filter, &self.previous_from, &self.previous_to)
    }

    /// One entry per day `from ..= to` (zero-filled, oldest first), sessions = distinct
    /// sessions active that day.
    pub fn daily(&self, filter: &UsageFilter, from: &str, to: &str) -> Vec<(String, UsageTotals)> {
        let dates = dates_between(from, to);
        let mut map: BTreeMap<&str, UsageTotals> = BTreeMap::new();
        for f in self.in_window(filter, from, to) {
            map.entry(f.date.as_str()).or_default().add_fact(f);
        }
        let mut sessions: BTreeMap<&str, u32> = BTreeMap::new();
        for s in &self.sessions {
            let mut seen: HashSet<&str> = HashSet::new();
            for a in &s.activity {
                if a.date.as_str() >= from
                    && a.date.as_str() <= to
                    && filter.matches(&s.provider, &a.model)
                    && seen.insert(a.date.as_str())
                {
                    *sessions.entry(a.date.as_str()).or_default() += 1;
                }
            }
        }
        dates
            .into_iter()
            .map(|d| {
                let mut t = map.get(d.as_str()).copied().unwrap_or_default();
                t.sessions = sessions.get(d.as_str()).copied().unwrap_or(0);
                (d, t)
            })
            .collect()
    }

    /// The heatmap: every day of the last [`HEATMAP_DAYS`] days under `filter`.
    pub fn heatmap(&self, filter: &UsageFilter) -> Vec<(String, UsageTotals)> {
        self.daily(filter, &self.heatmap_from, &self.today)
    }

    /// Stacked-chart series over `from ..= to` (values aligned with
    /// `dates_between(from, to)`), most expensive first (then by tokens).
    pub fn series(&self, filter: &UsageFilter, from: &str, to: &str, by: SeriesBy) -> Vec<Series> {
        let dates = dates_between(from, to);
        let index: BTreeMap<&str, usize> = dates
            .iter()
            .enumerate()
            .map(|(i, d)| (d.as_str(), i))
            .collect();
        let mut out: Vec<Series> = Vec::new();
        for f in self.in_window(filter, from, to) {
            let model = match by {
                SeriesBy::Provider => None,
                SeriesBy::Model => Some(f.model.clone()),
            };
            let pos = match out
                .iter()
                .position(|s| s.provider == f.provider && s.model == model)
            {
                Some(p) => p,
                None => {
                    out.push(Series {
                        provider: f.provider.clone(),
                        model,
                        total: UsageTotals::default(),
                        values: vec![UsageTotals::default(); dates.len()],
                    });
                    out.len() - 1
                }
            };
            let s = &mut out[pos];
            s.total.add_fact(f);
            if let Some(&i) = index.get(f.date.as_str()) {
                s.values[i].add_fact(f);
            }
        }
        out.sort_by(|a, b| {
            b.total
                .cost_usd
                .total_cmp(&a.total.cost_usd)
                .then(b.total.tokens.total().cmp(&a.total.tokens.total()))
                .then(provider_rank(&a.provider).cmp(&provider_rank(&b.provider)))
                .then(a.model.cmp(&b.model))
        });
        out
    }

    /// Tokens per `[weekday][hour]` (Monday first, local time) over the selected period.
    pub fn week_hour_tokens(&self, filter: &UsageFilter) -> [[u64; 24]; 7] {
        let mut grid = [[0u64; 24]; 7];
        for c in &self.hours {
            if filter.matches(&c.provider, &c.model) && c.weekday < 7 && c.hour < 24 {
                grid[c.weekday as usize][c.hour as usize] += c.tokens;
            }
        }
        grid
    }

    /// Distinct (provider, model) pairs with usage in `from ..= to`, canonical provider order
    /// then model id (for filter chips).
    pub fn models_in(&self, from: &str, to: &str) -> Vec<(ProviderKind, String)> {
        let mut v: Vec<(ProviderKind, String)> = Vec::new();
        for f in self.in_window(&UsageFilter::all(), from, to) {
            if !v.iter().any(|(p, m)| p == &f.provider && m == &f.model) {
                v.push((f.provider.clone(), f.model.clone()));
            }
        }
        v.sort_by(|a, b| {
            provider_rank(&a.0)
                .cmp(&provider_rank(&b.0))
                .then(a.1.cmp(&b.1))
        });
        v
    }
}

/// [`AnalyticsDetail`] behind an `Arc`: cloning is a pointer copy and comparing two clones of
/// the same run is a pointer comparison. Serialises as the plain detail.
#[derive(Debug, Clone, Default)]
pub struct SharedDetail(pub Arc<AnalyticsDetail>);

impl SharedDetail {
    pub fn new(detail: AnalyticsDetail) -> Self {
        Self(Arc::new(detail))
    }
}

impl Deref for SharedDetail {
    type Target = AnalyticsDetail;
    fn deref(&self) -> &AnalyticsDetail {
        &self.0
    }
}

impl PartialEq for SharedDetail {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl Serialize for SharedDetail {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for SharedDetail {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        AnalyticsDetail::deserialize(d).map(SharedDetail::new)
    }
}

// ---------------------------------------------------------------------------------------------
// Hourly usage per scan root (Teams export)
// ---------------------------------------------------------------------------------------------

/// UTC hours covered by the hourly export: the last 45 days.
pub const HOURLY_EXPORT_DAYS: u32 = 45;

/// Usage of one UTC hour × scan root × provider × model, summed over every project and
/// transcript file of that root. Carries no project, file or message data: `source` is the scan
/// root (`…\.claude\projects`, `…\.codex\sessions`), only used on the device to find the
/// account it belongs to (see [`RootOwners`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HourlyUsage {
    /// Unix seconds at the start of the UTC hour (multiple of 3600).
    pub hour: i64,
    pub source: String,
    pub provider: ProviderKind,
    /// Normalised model id (date suffixes and prefixes removed; `-fast` kept).
    pub model: String,
    /// input / output / cacheRead / cacheWrite (cacheWrite = all cache writes).
    pub tokens: TokenCounts,
    /// Part of `tokens.cache_write` written with a 1-hour TTL.
    pub cache_write_1h: u64,
}

/// A scanned transcript root and the ids of the ENABLED accounts whose profile folder it is
/// (in config order). Empty = the default profile with no account configured for it; two or
/// more = shared, so its usage cannot be attributed to one account.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RootOwners {
    pub source: String,
    pub provider: ProviderKind,
    pub accounts: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(date: &str, p: ProviderKind, model: &str, input: u64, cost: f64) -> UsageFact {
        UsageFact {
            date: date.into(),
            provider: p,
            model: model.into(),
            tokens: TokenCounts {
                input,
                ..Default::default()
            },
            cost_usd: cost,
            unpriced_tokens: if cost == 0.0 { input } else { 0 },
            sessions: 1,
        }
    }

    fn act(date: &str, model: &str) -> SessionActivity {
        SessionActivity {
            date: date.into(),
            model: model.into(),
        }
    }

    fn detail() -> AnalyticsDetail {
        AnalyticsDetail {
            today: "2026-11-12".into(),
            period_from: "2026-11-11".into(),
            previous_from: "2026-11-09".into(),
            previous_to: "2026-11-10".into(),
            heatmap_from: "2025-11-07".into(),
            facts_from: "2025-11-07".into(),
            facts: vec![
                fact("2026-11-09", ProviderKind::Claude, "opus", 5, 0.5),
                fact("2026-11-11", ProviderKind::Claude, "opus", 10, 1.0),
                fact("2026-11-11", ProviderKind::Claude, "sonnet", 20, 0.4),
                fact("2026-11-12", ProviderKind::Codex, "gpt", 30, 0.0),
            ],
            hours: vec![
                HourCell {
                    provider: ProviderKind::Claude,
                    model: "opus".into(),
                    weekday: 2,
                    hour: 9,
                    tokens: 10,
                    ..Default::default()
                },
                HourCell {
                    provider: ProviderKind::Codex,
                    model: "gpt".into(),
                    weekday: 3,
                    hour: 23,
                    tokens: 30,
                    ..Default::default()
                },
            ],
            sessions: vec![
                SessionFact {
                    provider: ProviderKind::Claude,
                    activity: vec![act("2026-11-09", "opus"), act("2026-11-11", "sonnet")],
                },
                SessionFact {
                    provider: ProviderKind::Claude,
                    activity: vec![act("2026-11-11", "opus"), act("2026-11-11", "sonnet")],
                },
                SessionFact {
                    provider: ProviderKind::Codex,
                    activity: vec![act("2026-11-12", "gpt")],
                },
            ],
            unpriced_models: vec!["gpt".into()],
        }
    }

    #[test]
    fn totals_filter_and_sessions() {
        let d = detail();
        let all = d.period_totals(&UsageFilter::all());
        assert_eq!(all.tokens.input, 60);
        assert!((all.cost_usd - 1.4).abs() < 1e-12);
        assert_eq!(all.unpriced_tokens, 30);
        assert_eq!(all.sessions, 3);
        let opus = d.period_totals(&UsageFilter::model(ProviderKind::Claude, "opus"));
        assert_eq!((opus.tokens.input, opus.sessions), (10, 1));
        let sonnet = d.period_totals(&UsageFilter::model(ProviderKind::Claude, "sonnet"));
        assert_eq!(sonnet.sessions, 2);
        let prev = d.previous_totals(&UsageFilter::provider(ProviderKind::Claude));
        assert_eq!((prev.tokens.input, prev.sessions), (5, 1));
        // Per-provider totals add up to the overall totals.
        let claude = d.period_totals(&UsageFilter::provider(ProviderKind::Claude));
        let codex = d.period_totals(&UsageFilter::provider(ProviderKind::Codex));
        assert_eq!(claude.tokens.input + codex.tokens.input, all.tokens.input);
    }

    #[test]
    fn daily_heatmap_series_and_grid() {
        let d = detail();
        let days = d.daily(&UsageFilter::all(), "2026-11-10", "2026-11-12");
        assert_eq!(
            days.iter().map(|(_, t)| t.tokens.input).collect::<Vec<_>>(),
            [0, 30, 30]
        );
        assert_eq!(days[1].1.sessions, 2);
        let heat = d.heatmap(&UsageFilter::all());
        assert_eq!(heat.len(), HEATMAP_DAYS as usize);
        assert_eq!(heat.last().map(|(d, _)| d.as_str()), Some("2026-11-12"));
        let s = d.series(
            &UsageFilter::all(),
            "2026-11-11",
            "2026-11-12",
            SeriesBy::Model,
        );
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].model.as_deref(), Some("opus"));
        assert_eq!(s[0].values.len(), 2);
        let s = d.series(
            &UsageFilter::all(),
            "2026-11-11",
            "2026-11-12",
            SeriesBy::Provider,
        );
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].values[0].tokens.input, 30);
        let g = d.week_hour_tokens(&UsageFilter::provider(ProviderKind::Codex));
        assert_eq!(g[3][23], 30);
        assert_eq!(g[2][9], 0);
        assert_eq!(d.models_in(&d.period_from, &d.today).len(), 3);
    }

    #[test]
    fn dates_and_fact_days() {
        assert_eq!(
            dates_between("2026-02-27", "2026-03-01"),
            ["2026-02-27", "2026-02-28", "2026-03-01"]
        );
        assert!(dates_between("2026-03-01", "2026-02-27").is_empty());
        assert!(dates_between("x", "2026-02-27").is_empty());
        assert_eq!(fact_days(7), HEATMAP_DAYS);
        assert_eq!(fact_days(200), 400);
    }

    #[test]
    fn shared_detail_serde_and_eq() {
        let a = SharedDetail::new(detail());
        let b = a.clone();
        assert_eq!(a, b);
        let json = serde_json::to_string(&a).unwrap();
        let c: SharedDetail = serde_json::from_str(&json).unwrap();
        assert_eq!(a, c);
        let empty: AnalyticsDetail = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, AnalyticsDetail::default());
    }
}
