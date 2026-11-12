//! Pure model of the statistics page: the UI-local filter (provider / model / chart metric,
//! never saved), the KPI figures, the chart series, the by-model shares and the activity
//! heatmap layout. Everything here derives from `AnalyticsDetail` client-side and is unit
//! tested; `pages_stats` only draws it.

use crate::logic::Comparison;
use chrono::{Datelike, NaiveDate};
use codenotch_core::analytics::{
    dates_between, AnalyticsDetail, SeriesBy, UsageFilter, UsageTotals, DAYS_PER_MONTH,
};
use codenotch_core::format;
use codenotch_core::model::{ProviderKind, TokenCounts};
use codenotch_core::Lang;

/// Series drawn in the chart before the rest is merged into "Others" (one per chart colour).
pub const CHART_SERIES: usize = 5;
/// Columns (weeks) of the activity heatmap at most (a year).
pub const HEAT_COLS: usize = 53;
/// Smallest heat cell: fewer weeks are shown rather than smaller cells.
pub const HEAT_MIN_CELL: f32 = 10.0;
/// Smallest primary KPI card width (see [`kpi_columns`]).
pub const KPI_MIN_W: f32 = 256.0;

// ---------------------------------------------------------------------------------------------
// Filter
// ---------------------------------------------------------------------------------------------

/// What the "Évolution" chart shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Metric {
    #[default]
    Cost,
    Tokens,
    Sessions,
}

impl Metric {
    pub const ALL: [Metric; 3] = [Metric::Cost, Metric::Tokens, Metric::Sessions];

    /// English key (translated by the page).
    pub fn label(self) -> &'static str {
        match self {
            Metric::Cost => "Cost",
            Metric::Tokens => "Tokens",
            Metric::Sessions => "Sessions",
        }
    }
}

/// The statistics page's local state: filters and chart metric. Not saved.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatsUi {
    pub provider: Option<ProviderKind>,
    /// Only with a provider.
    pub model: Option<String>,
    pub metric: Metric,
}

impl StatsUi {
    pub fn filter(&self) -> UsageFilter {
        match (&self.provider, &self.model) {
            (Some(p), Some(m)) => UsageFilter::model(p.clone(), m.clone()),
            (Some(p), None) => UsageFilter::provider(p.clone()),
            _ => UsageFilter::all(),
        }
    }

    /// Forget a provider / model the data no longer has (after a refresh or a period change).
    pub fn reconcile(&mut self, models: &[(ProviderKind, String)]) {
        if let Some(p) = &self.provider {
            if !models.iter().any(|(q, _)| q == p) {
                self.provider = None;
            }
        }
        let keep = match (&self.provider, &self.model) {
            (Some(p), Some(m)) => models.iter().any(|(q, n)| q == p && n == m),
            (None, Some(_)) => false,
            _ => true,
        };
        if !keep {
            self.model = None;
        }
    }
}

/// Distinct providers of `models`, in their order.
pub fn providers_of(models: &[(ProviderKind, String)]) -> Vec<ProviderKind> {
    let mut v: Vec<ProviderKind> = Vec::new();
    for (p, _) in models {
        if !v.contains(p) {
            v.push(p.clone());
        }
    }
    v
}

/// The (provider, model) pairs of `provider` (all of them when `None`).
pub fn models_of(
    models: &[(ProviderKind, String)],
    provider: Option<&ProviderKind>,
) -> Vec<(ProviderKind, String)> {
    models
        .iter()
        .filter(|(p, _)| provider.is_none_or(|q| q == p))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------------------------

/// Token count with the language's decimal mark and unit spacing: "12.4M" (en),
/// "12,4 M" (fr, narrow no-break space before the unit, like "89 %").
pub fn tok(lang: Lang, n: u64) -> String {
    let s = format::tokens(n);
    if lang != Lang::Fr {
        return s;
    }
    let s = s.replace('.', ",");
    match s.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => format!("{}\u{202f}{c}", &s[..i]),
        _ => s,
    }
}

/// Compact y-axis figure: "1.5M" / "1,5 M", "250k", "40" (no trailing ",0").
pub fn tok_axis(lang: Lang, v: f64) -> String {
    let v = v.max(0.0);
    let (x, unit) = if v >= 1e9 {
        (v / 1e9, "B")
    } else if v >= 1e6 {
        (v / 1e6, "M")
    } else if v >= 1e3 {
        (v / 1e3, "k")
    } else {
        (v, "")
    };
    let mut num = format!("{x:.1}");
    if num.ends_with(".0") {
        num.truncate(num.len() - 2);
    }
    match lang {
        Lang::Fr if unit.is_empty() => num.replace('.', ","),
        Lang::Fr => format!("{}\u{202f}{unit}", num.replace('.', ",")),
        Lang::En => format!("{num}{unit}"),
    }
}

/// Compact y-axis amount: "$50" / "50 $", with cents only for a fractional value ("$2.50").
pub fn money_axis(lang: Lang, v: f64) -> String {
    let v = v.max(0.0);
    if (v - v.round()).abs() < 1e-6 {
        let s = format::money(lang, v.round(), "USD");
        s.replace(".00", "").replace(",00", "")
    } else {
        format::money(lang, v, "USD")
    }
}

/// A fraction as a percent: "42 %" (fr) / "42%" (en); "<1 %" for a small non-zero share.
pub fn pct(lang: Lang, fraction: f64) -> String {
    let sep = if lang == Lang::Fr { "\u{202f}" } else { "" };
    let v = fraction * 100.0;
    if v > 0.0 && v < 1.0 {
        format!("<1{sep}%")
    } else {
        format!("{:.0}{sep}%", v)
    }
}

/// "×3,2" / "×3.2", "≥ ×3,2" for a lower bound.
pub fn ratio_text(lang: Lang, ratio: f64, lower_bound: bool) -> String {
    let x = format!("×{ratio:.1}");
    let x = if lang == Lang::Fr {
        x.replace('.', ",")
    } else {
        x
    };
    if lower_bound {
        format!("≥ {x}")
    } else {
        x
    }
}

fn parse(date: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

/// "4 nov." / "Nov 4" (the input when it is not a date).
pub fn short_date(lang: Lang, date: &str) -> String {
    match parse(date) {
        Some(d) => format::month_day(lang, d.month(), d.day()),
        None => date.to_string(),
    }
}

/// "4 nov. 2026" / "Nov 4, 2026".
pub fn long_date(lang: Lang, date: &str) -> String {
    match parse(date) {
        Some(d) if lang == Lang::Fr => {
            format!(
                "{} {}",
                format::month_day(lang, d.month(), d.day()),
                d.year()
            )
        }
        Some(d) => format!(
            "{}, {}",
            format::month_day(lang, d.month(), d.day()),
            d.year()
        ),
        None => date.to_string(),
    }
}

/// Tooltip header of a day: "mer. 12 nov. 2026" / "Wed, Nov 12, 2026".
pub fn day_title(lang: Lang, date: &str) -> String {
    const EN: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const FR: [&str; 7] = ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."];
    match parse(date) {
        Some(d) => {
            let wd = d.weekday().num_days_from_monday() as usize;
            if lang == Lang::Fr {
                format!("{} {}", FR[wd], long_date(lang, date))
            } else {
                format!("{}, {}", EN[wd], long_date(lang, date))
            }
        }
        None => date.to_string(),
    }
}

/// "Lun" … "Dim" / "Mon" … "Sun" (0 = Monday).
pub fn weekday_short(lang: Lang, from_monday: usize) -> &'static str {
    const EN: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const FR: [&str; 7] = ["Lun", "Mar", "Mer", "Jeu", "Ven", "Sam", "Dim"];
    let i = from_monday.min(6);
    if lang == Lang::Fr {
        FR[i]
    } else {
        EN[i]
    }
}

// ---------------------------------------------------------------------------------------------
// KPIs
// ---------------------------------------------------------------------------------------------

/// Relative change from `previous` to `current`; `None` when the previous figure is zero (no
/// data to compare with).
pub fn delta(current: f64, previous: f64) -> Option<f64> {
    (previous > 0.0 && current.is_finite()).then(|| (current - previous) / previous)
}

/// Share of the prompt served from the cache: cache read / (input + cache read + cache write).
pub fn cache_rate(t: &TokenCounts) -> Option<f64> {
    let prompt = t.input + t.cache_read + t.cache_write;
    (prompt > 0).then(|| t.cache_read as f64 / prompt as f64)
}

/// (current, best) runs of consecutive days with tokens. The current run ends today, or
/// yesterday while today has no activity yet.
pub fn streaks(days: &[(String, UsageTotals)]) -> (u32, u32) {
    let active: Vec<bool> = days.iter().map(|(_, t)| t.tokens.total() > 0).collect();
    let mut best = 0u32;
    let mut run = 0u32;
    for a in &active {
        run = if *a { run + 1 } else { 0 };
        best = best.max(run);
    }
    let mut end = active.len();
    if end > 0 && !active[end - 1] {
        end -= 1;
    }
    let current = active[..end].iter().rev().take_while(|a| **a).count() as u32;
    (current, best)
}

/// Heat level 0..=4: 0 = nothing, then quarters of the busiest value (anything > 0 is ≥ 1).
pub fn heat_level(value: u64, max: u64) -> u8 {
    if value == 0 || max == 0 {
        return 0;
    }
    let r = value as f64 / max as f64;
    if r <= 0.25 {
        1
    } else if r <= 0.5 {
        2
    } else if r <= 0.75 {
        3
    } else {
        4
    }
}

/// Busiest weekday (0 = Monday) and busiest hour of a `[weekday][hour]` grid.
pub fn peaks(grid: &[[u64; 24]; 7]) -> (Option<usize>, Option<usize>) {
    let argmax = |v: &[u64]| {
        v.iter()
            .enumerate()
            .filter(|(_, x)| **x > 0)
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(&a.0)))
            .map(|(i, _)| i)
    };
    let days: Vec<u64> = grid.iter().map(|r| r.iter().sum()).collect();
    let hours: Vec<u64> = (0..24).map(|h| grid.iter().map(|r| r[h]).sum()).collect();
    (argmax(&days), argmax(&hours))
}

/// Whether a delta vs the previous period is meaningful. Claude Code purges transcripts after
/// ~30 days, so the previous window is often (almost) empty: hide the delta when the history
/// does not reach back to `previous_from` (`earliest_usage` = first date with any usage, all
/// providers), or when the previous period has fewer than `max(1, ⌈period_days / 4⌉)` active
/// days. A zero previous value is handled by [`delta`].
pub fn delta_reliable(
    earliest_usage: Option<&str>,
    previous_from: &str,
    previous_active_days: u32,
    period_days: u32,
) -> bool {
    let Some(first) = earliest_usage else {
        return false;
    };
    first <= previous_from && previous_active_days >= period_days.div_ceil(4).max(1)
}

/// Number of days in the selected period.
pub fn period_days(d: &AnalyticsDetail) -> u32 {
    (dates_between(&d.period_from, &d.today).len() as u32).max(1)
}

/// Figures of the KPI grid under one filter.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Kpis {
    pub period_days: u32,
    pub totals: UsageTotals,
    pub previous: UsageTotals,
    pub cost_delta: Option<f64>,
    pub tokens_delta: Option<f64>,
    pub cache_rate: Option<f64>,
    pub tokens_per_session: Option<u64>,
    /// Days of the period with tokens.
    pub active_days: u32,
    pub cost_per_active_day: Option<f64>,
    /// (weekday, hour) cells of the period with tokens.
    pub active_hours: u32,
    pub tokens_per_active_hour: Option<u64>,
    /// Cost of the period scaled to an average month.
    pub monthly_projection: f64,
    /// Over the heatmap (53 weeks).
    pub streak: u32,
    pub best_streak: u32,
    /// Busiest day of the period: (date, tokens).
    pub busiest_day: Option<(String, u64)>,
    pub peak_weekday: Option<usize>,
    pub peak_hour: Option<usize>,
    /// Model with the most tokens and its share of the tokens.
    pub top_model: Option<(String, f64)>,
}

pub fn kpis(d: &AnalyticsDetail, f: &UsageFilter) -> Kpis {
    let days = period_days(d);
    let totals = d.period_totals(f);
    let previous = d.previous_totals(f);
    let daily = d.daily(f, &d.period_from, &d.today);
    let active_days = daily.iter().filter(|(_, t)| t.tokens.total() > 0).count() as u32;
    let busiest_day = daily
        .iter()
        .filter(|(_, t)| t.tokens.total() > 0)
        .max_by(|a, b| {
            a.1.tokens
                .total()
                .cmp(&b.1.tokens.total())
                .then(b.0.cmp(&a.0))
        })
        .map(|(date, t)| (date.clone(), t.tokens.total()));
    let grid = d.week_hour_tokens(f);
    let active_hours = grid.iter().flatten().filter(|v| **v > 0).count() as u32;
    let hour_tokens: u64 = grid.iter().flatten().sum();
    let (peak_weekday, peak_hour) = peaks(&grid);
    let (streak, best_streak) = streaks(&d.heatmap(f));
    let total_tokens = totals.tokens.total();
    let earliest_usage = d
        .facts
        .iter()
        .filter(|x| x.tokens.total() > 0)
        .map(|x| x.date.as_str())
        .min();
    let previous_active_days = d
        .daily(f, &d.previous_from, &d.previous_to)
        .iter()
        .filter(|(_, t)| t.tokens.total() > 0)
        .count() as u32;
    let reliable = delta_reliable(earliest_usage, &d.previous_from, previous_active_days, days);
    let gated = |current: f64, prev: f64| delta(current, prev).filter(|_| reliable);
    let top_model = model_shares(d, f)
        .into_iter()
        .max_by(|a, b| a.tokens.cmp(&b.tokens))
        .map(|m| (m.model, m.share));
    Kpis {
        period_days: days,
        totals,
        previous,
        cost_delta: gated(totals.cost_usd, previous.cost_usd),
        tokens_delta: gated(total_tokens as f64, previous.tokens.total() as f64),
        cache_rate: cache_rate(&totals.tokens),
        tokens_per_session: (totals.sessions > 0)
            .then(|| total_tokens / u64::from(totals.sessions)),
        active_days,
        cost_per_active_day: (active_days > 0).then(|| totals.cost_usd / f64::from(active_days)),
        active_hours,
        tokens_per_active_hour: (active_hours > 0).then(|| hour_tokens / u64::from(active_hours)),
        monthly_projection: totals.cost_usd / f64::from(days) * DAYS_PER_MONTH,
        streak,
        best_streak,
        busiest_day,
        peak_weekday,
        peak_hour,
        top_model,
    }
}

/// The "×N" plan value of the KPI grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanRatio {
    /// API-equivalent cost / plan price over the period.
    pub ratio: f64,
    /// Some tokens have no price: the ratio is a lower bound.
    pub lower_bound: bool,
    /// Some plan price is a built-in list price (an estimate).
    pub list_price: bool,
}

/// API-equivalent / plan price for the filter: every provider with a plan price when nothing
/// is filtered, that provider's line when one is, nothing for a single model (plans are per
/// provider). Only providers with usage data have a plan price here (see
/// `logic::comparisons`).
pub fn plan_ratio(comparisons: &[Comparison], ui: &StatsUi) -> Option<PlanRatio> {
    if ui.model.is_some() {
        return None;
    }
    let lines: Vec<&Comparison> = comparisons
        .iter()
        .filter(|c| c.plan_eur.is_some_and(|p| p > 0.0))
        .filter(|c| ui.provider.as_ref().is_none_or(|p| *p == c.provider))
        .collect();
    let lower_bound = lines.iter().any(|c| c.unpriced_tokens > 0);
    let list_price = lines.iter().any(|c| c.list_price);
    let plan: f64 = lines.iter().filter_map(|c| c.plan_eur).sum();
    if lines.is_empty() || plan <= 0.0 {
        return None;
    }
    let api: f64 = lines.iter().map(|c| c.api_eur).sum();
    Some(PlanRatio {
        ratio: api / plan,
        lower_bound,
        list_price,
    })
}

// ---------------------------------------------------------------------------------------------
// Chart & shares
// ---------------------------------------------------------------------------------------------

/// What a chart series stands for (decides its colour).
#[derive(Debug, Clone, PartialEq)]
pub enum SeriesKey {
    Provider(ProviderKind),
    Model(ProviderKind, String),
    /// The tail merged into "Others".
    Others,
    /// Sessions of the whole filter (one series).
    Total,
}

/// The "Évolution" chart: one label per day, stacked series (bottom first).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChartData {
    pub dates: Vec<String>,
    /// (name, one value per day).
    pub series: Vec<(String, Vec<f64>)>,
    /// One key per series.
    pub keys: Vec<SeriesKey>,
}

fn metric_value(m: Metric, t: &UsageTotals) -> f64 {
    match m {
        Metric::Cost => t.cost_usd,
        Metric::Tokens => t.tokens.total() as f64,
        Metric::Sessions => f64::from(t.sessions),
    }
}

/// Daily series of the period for the filter: stacked by model when a provider is chosen, by
/// provider otherwise; at most [`CHART_SERIES`] (the rest merged under `others`). Sessions are
/// not additive across series (one session can use several models): one series.
pub fn chart_data(d: &AnalyticsDetail, ui: &StatsUi, sessions: &str, others: &str) -> ChartData {
    let f = ui.filter();
    let dates = dates_between(&d.period_from, &d.today);
    if ui.metric == Metric::Sessions {
        let values = d
            .daily(&f, &d.period_from, &d.today)
            .iter()
            .map(|(_, t)| f64::from(t.sessions))
            .collect();
        let key = match (&ui.provider, &ui.model) {
            (Some(p), Some(m)) => SeriesKey::Model(p.clone(), m.clone()),
            (Some(p), None) => SeriesKey::Provider(p.clone()),
            _ => SeriesKey::Total,
        };
        return ChartData {
            dates,
            series: vec![(sessions.to_string(), values)],
            keys: vec![key],
        };
    }
    let by = if ui.provider.is_some() {
        SeriesBy::Model
    } else {
        SeriesBy::Provider
    };
    let mut all = d.series(&f, &d.period_from, &d.today, by);
    if ui.metric == Metric::Tokens {
        all.sort_by_key(|s| std::cmp::Reverse(s.total.tokens.total()));
    }
    let named = |s: &codenotch_core::analytics::Series| {
        s.model
            .clone()
            .unwrap_or_else(|| s.provider.display_name().to_string())
    };
    let key = |s: &codenotch_core::analytics::Series| match &s.model {
        Some(m) => SeriesKey::Model(s.provider.clone(), m.clone()),
        None => SeriesKey::Provider(s.provider.clone()),
    };
    let mut series: Vec<(String, Vec<f64>)> = Vec::new();
    let mut keys: Vec<SeriesKey> = Vec::new();
    let keep = if all.len() > CHART_SERIES {
        CHART_SERIES - 1
    } else {
        CHART_SERIES
    };
    for s in all.iter().take(keep) {
        keys.push(key(s));
        series.push((
            named(s),
            s.values
                .iter()
                .map(|v| metric_value(ui.metric, v))
                .collect(),
        ));
    }
    if all.len() > keep {
        let mut rest = vec![0.0; dates.len()];
        for s in &all[keep..] {
            for (r, v) in rest.iter_mut().zip(&s.values) {
                *r += metric_value(ui.metric, v);
            }
        }
        series.push((others.to_string(), rest));
        keys.push(SeriesKey::Others);
    }
    ChartData {
        dates,
        series,
        keys,
    }
}

/// One line of the "By model" list.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelShare {
    pub provider: ProviderKind,
    pub model: String,
    pub tokens: u64,
    pub cost_usd: f64,
    pub unpriced_tokens: u64,
    /// Share of the filter's tokens, 0..=1.
    pub share: f64,
}

/// Models of the period under the filter, most expensive first, with their token share.
pub fn model_shares(d: &AnalyticsDetail, f: &UsageFilter) -> Vec<ModelShare> {
    let series = d.series(f, &d.period_from, &d.today, SeriesBy::Model);
    let total: u64 = series.iter().map(|s| s.total.tokens.total()).sum();
    series
        .into_iter()
        .map(|s| {
            let tokens = s.total.tokens.total();
            ModelShare {
                provider: s.provider,
                model: s.model.unwrap_or_default(),
                tokens,
                cost_usd: s.total.cost_usd,
                unpriced_tokens: s.total.unpriced_tokens,
                share: if total == 0 {
                    0.0
                } else {
                    tokens as f64 / total as f64
                },
            }
        })
        .collect()
}

/// Rank of every model of the period within its provider (0 = most expensive, then by tokens,
/// then by name): the stable index of its colour shade, whatever the filter or the metric.
pub fn model_ranks(d: &AnalyticsDetail) -> Vec<((ProviderKind, String), usize)> {
    let mut all = model_shares(d, &UsageFilter::all());
    all.sort_by(|a, b| {
        a.provider
            .cmp(&b.provider)
            .then(b.cost_usd.total_cmp(&a.cost_usd))
            .then(b.tokens.cmp(&a.tokens))
            .then(a.model.cmp(&b.model))
    });
    let mut out: Vec<((ProviderKind, String), usize)> = Vec::new();
    let mut rank = 0;
    for (i, m) in all.iter().enumerate() {
        rank = if i > 0 && all[i - 1].provider == m.provider {
            rank + 1
        } else {
            0
        };
        out.push(((m.provider.clone(), m.model.clone()), rank));
    }
    out
}

/// Rank of `model` in [`model_ranks`] (0 when unknown).
pub fn rank_of(ranks: &[((ProviderKind, String), usize)], p: &ProviderKind, model: &str) -> usize {
    ranks
        .iter()
        .find(|((q, m), _)| q == p && m == model)
        .map_or(0, |(_, r)| *r)
}

// ---------------------------------------------------------------------------------------------
// Heatmap
// ---------------------------------------------------------------------------------------------

/// One day of the activity heatmap.
#[derive(Debug, Clone, PartialEq)]
pub struct HeatCell {
    pub date: String,
    /// Week 0..HEAT_COLS (oldest first).
    pub col: usize,
    /// 0 = Monday … 6 = Sunday.
    pub row: usize,
    pub totals: UsageTotals,
    pub level: u8,
}

/// Lay the last `weeks` weeks of `days` (oldest first, ending today) on a Monday-first grid:
/// today is in the last column at its weekday row; later days of that week are not drawn.
/// Levels are relative to the busiest day shown.
pub fn heat_grid(days: &[(String, UsageTotals)], weeks: usize) -> Vec<HeatCell> {
    let Some(last) = days.last().and_then(|(d, _)| parse(d)) else {
        return Vec::new();
    };
    let weeks = weeks.max(1);
    let weekday = last.weekday().num_days_from_monday() as usize;
    let slots = (weeks - 1) * 7 + weekday + 1;
    let shown = &days[days.len().saturating_sub(slots)..];
    let offset = slots - shown.len();
    let max = shown
        .iter()
        .map(|(_, t)| t.tokens.total())
        .max()
        .unwrap_or(0);
    shown
        .iter()
        .enumerate()
        .map(|(k, (date, t))| {
            let pos = k + offset;
            HeatCell {
                date: date.clone(),
                col: pos / 7,
                row: pos % 7,
                totals: *t,
                level: heat_level(t.tokens.total(), max),
            }
        })
        .collect()
}

/// Month labels above the heatmap: `(column, month 1..=12)` at each column whose first shown
/// day starts a new month, never two closer than `min_cols` columns (a label wider than its
/// month): a too-close first label (partial month) gives way to the next one, any later one is
/// skipped.
pub fn month_labels(cells: &[HeatCell], min_cols: usize) -> Vec<(usize, u32)> {
    let mut out: Vec<(usize, u32)> = Vec::new();
    let mut prev: Option<u32> = None;
    let mut replaced = false;
    for c in cells {
        let first_of_col = c.row == 0 || cells.first().is_some_and(|f| f.date == c.date);
        if !first_of_col {
            continue;
        }
        let Some(m) = parse(&c.date).map(|d| d.month()) else {
            continue;
        };
        if prev != Some(m) {
            prev = Some(m);
            match out.last() {
                Some(&(col, _)) if c.col - col < min_cols => {
                    // Only the grid's opening (partial) month gives way, once.
                    if out.len() == 1 && !replaced {
                        out[0] = (c.col, m);
                        replaced = true;
                    }
                }
                _ => out.push((c.col, m)),
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------------

/// Columns of the primary KPI cards for a content width (shadcn `SectionCards`: 4 across on a
/// wide page, 2 × 2 on a medium one, stacked on a narrow one): each card keeps at least
/// [`KPI_MIN_W`] px.
pub fn kpi_columns(content_w: f32, gap: f32) -> usize {
    [4usize, 2]
        .into_iter()
        .find(|&n| content_w + gap >= n as f32 * (KPI_MIN_W + gap))
        .unwrap_or(1)
}

/// Columns of the secondary figures card: 3 (two rows of three), 2 on a narrow page (one-line\n/// sub-lines stay whole).
pub fn habit_columns(content_w: f32) -> usize {
    if content_w >= 780.0 {
        3
    } else {
        2
    }
}

/// Direction of a change vs the previous period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    Up,
    Down,
    /// Under half a percent either way.
    Flat,
}

impl Trend {
    pub fn of(delta: f64) -> Trend {
        if delta >= 0.005 {
            Trend::Up
        } else if delta <= -0.005 {
            Trend::Down
        } else {
            Trend::Flat
        }
    }

    /// Arrow drawn with the figure ("↗" / "↘"; none when flat).
    pub fn arrow(self) -> Option<&'static str> {
        match self {
            Trend::Up => Some("\u{2197}"),
            Trend::Down => Some("\u{2198}"),
            Trend::Flat => None,
        }
    }
}

/// Signed change with one decimal for the KPI badges: "+12,5 %" / "−3,2 %" (fr), "+12.5%" /
/// "−3.2%" (en), "0 %" when flat.
pub fn signed_pct(lang: Lang, delta: f64) -> String {
    let sep = if lang == Lang::Fr { "\u{202f}" } else { "" };
    let v = (delta * 1000.0).round() / 10.0;
    if v == 0.0 {
        return format!("0{sep}%");
    }
    let sign = if v > 0.0 { "+" } else { "\u{2212}" };
    let n = format!("{:.1}", v.abs());
    let n = if lang == Lang::Fr {
        n.replace('.', ",")
    } else {
        n
    };
    format!("{sign}{n}{sep}%")
}

/// Weeks of the heatmap that fit `avail` px with cells of at least [`HEAT_MIN_CELL`] (3 px
/// gaps), at most [`HEAT_COLS`].
pub fn heat_weeks(avail: f32) -> usize {
    (((avail + 3.0) / (HEAT_MIN_CELL + 3.0)).floor() as usize).clamp(4, HEAT_COLS)
}

/// (cell, gap) of a grid of `cols` square cells fitting `avail` px: 3 px gaps for an 11 px
/// cell, up to 14 px cells when there is room, scaled down (never under 4 px) when there is
/// not.
pub fn grid_cell(avail: f32, cols: usize, max_cell: f32) -> (f32, f32) {
    let cols = cols.max(1) as f32;
    let ideal_gap = 3.0;
    let cell = (avail - (cols - 1.0) * ideal_gap) / cols;
    if cell >= 11.0 {
        (cell.min(max_cell).floor(), ideal_gap)
    } else {
        // Keep the 11:3 proportion.
        let unit = avail / (cols * 14.0 - 3.0);
        ((11.0 * unit).max(4.0), (3.0 * unit).max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::analytics::{HourCell, SessionActivity, SessionFact, UsageFact};

    fn tc(input: u64) -> TokenCounts {
        TokenCounts {
            input,
            ..Default::default()
        }
    }

    fn day(date: &str, tokens: u64) -> (String, UsageTotals) {
        (
            date.to_string(),
            UsageTotals {
                tokens: tc(tokens),
                ..Default::default()
            },
        )
    }

    fn fact(date: &str, p: ProviderKind, model: &str, input: u64, cost: f64) -> UsageFact {
        UsageFact {
            date: date.into(),
            provider: p,
            model: model.into(),
            tokens: TokenCounts {
                input,
                cache_read: input,
                ..Default::default()
            },
            cost_usd: cost,
            unpriced_tokens: 0,
            sessions: 1,
        }
    }

    fn detail() -> AnalyticsDetail {
        AnalyticsDetail {
            today: "2026-11-12".into(),
            period_from: "2026-11-10".into(),
            previous_from: "2026-11-07".into(),
            previous_to: "2026-11-09".into(),
            heatmap_from: "2025-11-07".into(),
            facts_from: "2025-11-07".into(),
            facts: vec![
                fact("2026-11-08", ProviderKind::Claude, "opus", 10, 1.0),
                fact("2026-11-10", ProviderKind::Claude, "opus", 30, 3.0),
                fact("2026-11-11", ProviderKind::Claude, "sonnet", 10, 0.5),
                fact("2026-11-11", ProviderKind::Codex, "gpt", 10, 0.5),
            ],
            hours: vec![
                HourCell {
                    provider: ProviderKind::Claude,
                    model: "opus".into(),
                    weekday: 1,
                    hour: 14,
                    tokens: 60,
                    ..Default::default()
                },
                HourCell {
                    provider: ProviderKind::Codex,
                    model: "gpt".into(),
                    weekday: 2,
                    hour: 9,
                    tokens: 20,
                    ..Default::default()
                },
            ],
            sessions: vec![
                SessionFact {
                    provider: ProviderKind::Claude,
                    activity: vec![SessionActivity {
                        date: "2026-11-10".into(),
                        model: "opus".into(),
                    }],
                },
                SessionFact {
                    provider: ProviderKind::Codex,
                    activity: vec![SessionActivity {
                        date: "2026-11-11".into(),
                        model: "gpt".into(),
                    }],
                },
            ],
            unpriced_models: Vec::new(),
        }
    }

    #[test]
    fn filter_and_reconcile() {
        let models = vec![
            (ProviderKind::Claude, "opus".to_string()),
            (ProviderKind::Claude, "sonnet".to_string()),
            (ProviderKind::Codex, "gpt".to_string()),
        ];
        assert_eq!(
            providers_of(&models),
            vec![ProviderKind::Claude, ProviderKind::Codex]
        );
        assert_eq!(
            models_of(&models, Some(&ProviderKind::Claude))
                .iter()
                .map(|(_, m)| m.as_str())
                .collect::<Vec<_>>(),
            ["opus", "sonnet"]
        );
        assert_eq!(models_of(&models, None).len(), 3);
        let mut ui = StatsUi {
            provider: Some(ProviderKind::Claude),
            model: Some("opus".into()),
            metric: Metric::Tokens,
        };
        assert_eq!(
            ui.filter(),
            UsageFilter::model(ProviderKind::Claude, "opus")
        );
        ui.reconcile(&models);
        assert_eq!(ui.model.as_deref(), Some("opus"));
        ui.reconcile(&models[1..]);
        assert_eq!(
            (ui.provider.clone(), ui.model.clone()),
            (Some(ProviderKind::Claude), None)
        );
        assert_eq!(ui.filter(), UsageFilter::provider(ProviderKind::Claude));
        ui.reconcile(&models[2..]);
        assert_eq!(ui.provider, None);
        assert_eq!(ui.filter(), UsageFilter::all());
        assert_eq!(ui.metric, Metric::Tokens, "the metric is kept");
    }

    #[test]
    fn formatting() {
        assert_eq!(tok(Lang::Fr, 12_400_000), "12,4\u{202f}M");
        assert_eq!(tok(Lang::Fr, 531_700), "531,7\u{202f}k");
        assert_eq!(tok(Lang::Fr, 950), "950");
        assert_eq!(tok(Lang::En, 12_400_000), "12.4M");
        assert_eq!(tok_axis(Lang::En, 15_000_000.0), "15M");
        assert_eq!(tok_axis(Lang::Fr, 2_500_000.0), "2,5\u{202f}M");
        assert_eq!(tok_axis(Lang::En, 250_000.0), "250k");
        assert_eq!(tok_axis(Lang::En, 0.0), "0");
        assert_eq!(money_axis(Lang::En, 50.0), "$50");
        assert_eq!(money_axis(Lang::Fr, 1500.0), "1\u{a0}500 $");
        assert_eq!(money_axis(Lang::En, 2.5), "$2.50");
        assert_eq!(money_axis(Lang::Fr, 0.0), "0 $");
        assert_eq!(pct(Lang::Fr, 0.423), "42\u{202f}%");
        assert_eq!(pct(Lang::En, 0.004), "<1%");
        assert_eq!(pct(Lang::En, 0.0), "0%");
        assert_eq!(ratio_text(Lang::Fr, 3.24, true), "≥ ×3,2");
        assert_eq!(ratio_text(Lang::En, 0.5, false), "×0.5");
        assert_eq!(short_date(Lang::Fr, "2026-11-04"), "4 nov.");
        assert_eq!(short_date(Lang::En, "2026-11-04"), "Nov 4");
        assert_eq!(long_date(Lang::Fr, "2026-11-04"), "4 nov. 2026");
        assert_eq!(long_date(Lang::En, "2026-11-04"), "Nov 4, 2026");
        assert_eq!(day_title(Lang::Fr, "2026-11-12"), "jeu. 12 nov. 2026");
        assert_eq!(day_title(Lang::En, "2026-11-11"), "Wed, Nov 11, 2026");
        assert_eq!(day_title(Lang::Fr, "x"), "x");
        assert_eq!(short_date(Lang::Fr, "bad"), "bad");
        assert_eq!(weekday_short(Lang::Fr, 2), "Mer");
        assert_eq!(weekday_short(Lang::En, 9), "Sun");
    }

    #[test]
    fn deltas_cache_and_levels() {
        assert_eq!(delta(150.0, 100.0), Some(0.5));
        assert_eq!(delta(50.0, 100.0), Some(-0.5));
        assert_eq!(delta(50.0, 0.0), None);
        let t = TokenCounts {
            input: 10,
            output: 999,
            cache_read: 30,
            cache_write: 10,
        };
        assert_eq!(cache_rate(&t), Some(0.6));
        assert_eq!(cache_rate(&TokenCounts::default()), None);
        assert_eq!(heat_level(0, 100), 0);
        assert_eq!(heat_level(1, 100), 1);
        assert_eq!(heat_level(25, 100), 1);
        assert_eq!(heat_level(26, 100), 2);
        assert_eq!(heat_level(50, 100), 2);
        assert_eq!(heat_level(75, 100), 3);
        assert_eq!(heat_level(76, 100), 4);
        assert_eq!(heat_level(100, 100), 4);
        assert_eq!(heat_level(5, 0), 0);
    }

    #[test]
    fn streak_runs() {
        let d = |v: &[u64]| -> Vec<(String, UsageTotals)> {
            v.iter()
                .enumerate()
                .map(|(i, t)| day(&format!("d{i}"), *t))
                .collect()
        };
        assert_eq!(streaks(&d(&[1, 1, 1, 0, 1, 1])), (2, 3));
        // Today not active yet: the run up to yesterday still counts.
        assert_eq!(streaks(&d(&[1, 0, 1, 1, 0])), (2, 2));
        assert_eq!(streaks(&d(&[1, 0, 0])), (0, 1));
        assert_eq!(streaks(&[]), (0, 0));
    }

    #[test]
    fn peak_weekday_and_hour() {
        let mut g = [[0u64; 24]; 7];
        assert_eq!(peaks(&g), (None, None));
        g[1][14] = 50;
        g[2][9] = 30;
        g[2][10] = 30;
        assert_eq!(peaks(&g), (Some(2), Some(14)));
    }

    #[test]
    fn kpi_figures() {
        let mut d = detail();
        // History reaching back before the previous period, so the deltas are shown.
        d.facts
            .insert(0, fact("2026-10-01", ProviderKind::Claude, "opus", 1, 0.0));
        let k = kpis(&d, &UsageFilter::all());
        assert_eq!(k.period_days, 3);
        assert_eq!(k.totals.tokens.total(), 100);
        assert!((k.totals.cost_usd - 4.0).abs() < 1e-9);
        // Previous: 1 $ → +300 %.
        assert!((k.cost_delta.unwrap() - 3.0).abs() < 1e-9);
        assert_eq!(k.tokens_delta, Some(4.0));
        assert_eq!(k.cache_rate, Some(0.5));
        assert_eq!(k.totals.sessions, 2);
        assert_eq!(k.tokens_per_session, Some(50));
        assert_eq!(k.active_days, 2);
        assert_eq!(k.cost_per_active_day, Some(2.0));
        assert_eq!((k.active_hours, k.tokens_per_active_hour), (2, Some(40)));
        assert!((k.monthly_projection - 4.0 / 3.0 * DAYS_PER_MONTH).abs() < 1e-9);
        // 11-08 (prev), gap on 11-09, then 11-10 and 11-11; 11-12 (today) empty.
        assert_eq!((k.streak, k.best_streak), (2, 2));
        assert_eq!(k.busiest_day, Some(("2026-11-10".to_string(), 60)));
        assert_eq!((k.peak_weekday, k.peak_hour), (Some(1), Some(14)));
        assert_eq!(k.top_model, Some(("opus".to_string(), 0.6)));

        let codex = kpis(&d, &UsageFilter::provider(ProviderKind::Codex));
        assert_eq!(codex.cost_delta, None, "no previous data");
        assert_eq!(codex.tokens_delta, None);
        assert_eq!(codex.top_model, Some(("gpt".to_string(), 1.0)));
        assert_eq!(codex.peak_hour, Some(9));

        let empty = kpis(&d, &UsageFilter::provider(ProviderKind::Glm));
        assert_eq!(empty.tokens_per_session, None);
        assert_eq!(empty.cost_per_active_day, None);
        assert_eq!(empty.top_model, None);
        assert_eq!(empty.busiest_day, None);
    }

    #[test]
    fn delta_reliability_rule() {
        // History covers the window, enough active days.
        assert!(delta_reliable(Some("2026-10-01"), "2026-10-14", 1, 1));
        assert!(delta_reliable(Some("2026-10-14"), "2026-10-14", 2, 7));
        assert!(delta_reliable(Some("2026-09-01"), "2026-10-01", 8, 30));
        // History starts after the previous window's first day.
        assert!(!delta_reliable(Some("2026-10-15"), "2026-10-14", 7, 7));
        // No usage at all.
        assert!(!delta_reliable(None, "2026-10-14", 0, 7));
        // Too few active days: ⌈7/4⌉ = 2, ⌈30/4⌉ = 8, at least 1.
        assert!(!delta_reliable(Some("2026-01-01"), "2026-10-14", 1, 7));
        assert!(!delta_reliable(Some("2026-01-01"), "2026-10-01", 7, 30));
        assert!(!delta_reliable(Some("2026-01-01"), "2026-11-12", 0, 1));
    }

    #[test]
    fn kpi_deltas_hidden_when_history_is_short() {
        let mut d = detail();
        // History starts on 11-08, after previous_from (11-07): both deltas hidden.
        let k = kpis(&d, &UsageFilter::all());
        assert_eq!((k.cost_delta, k.tokens_delta), (None, None));
        // An older fact under another provider still proves the history reaches back.
        d.facts
            .insert(0, fact("2026-10-01", ProviderKind::Codex, "gpt", 5, 0.1));
        let k = kpis(&d, &UsageFilter::all());
        assert!(k.cost_delta.is_some() && k.tokens_delta.is_some());
        // A 12-day period needs 3 active previous days; there is 1.
        d.period_from = "2026-11-01".into();
        d.previous_from = "2026-10-20".into();
        d.previous_to = "2026-10-31".into();
        d.facts
            .push(fact("2026-10-25", ProviderKind::Claude, "opus", 10, 1.0));
        let k = kpis(&d, &UsageFilter::all());
        assert_eq!((k.cost_delta, k.tokens_delta), (None, None));
    }

    #[test]
    fn plan_ratio_follows_the_filter() {
        let c =
            |p: ProviderKind, api: f64, plan: Option<f64>, unpriced: u64, list: bool| Comparison {
                provider: p,
                tokens: 0,
                sessions: 0,
                api_usd: api,
                api_eur: api,
                plan_eur: plan,
                list_price: list,
                unpriced_tokens: unpriced,
            };
        let lines = vec![
            c(ProviderKind::Claude, 60.0, Some(20.0), 0, false),
            c(ProviderKind::Codex, 10.0, Some(20.0), 5, true),
            c(ProviderKind::Glm, 99.0, None, 0, false),
        ];
        let r = |ratio: f64, lower_bound: bool, list_price: bool| {
            Some(PlanRatio {
                ratio,
                lower_bound,
                list_price,
            })
        };
        let mut ui = StatsUi::default();
        assert_eq!(plan_ratio(&lines, &ui), r(70.0 / 40.0, true, true));
        ui.provider = Some(ProviderKind::Claude);
        assert_eq!(plan_ratio(&lines, &ui), r(3.0, false, false));
        ui.provider = Some(ProviderKind::Codex);
        assert_eq!(plan_ratio(&lines, &ui), r(0.5, true, true));
        ui.provider = Some(ProviderKind::Glm);
        assert_eq!(plan_ratio(&lines, &ui), None);
        ui.provider = Some(ProviderKind::Claude);
        ui.model = Some("opus".into());
        assert_eq!(plan_ratio(&lines, &ui), None);
        assert_eq!(plan_ratio(&[], &StatsUi::default()), None);
    }

    #[test]
    fn chart_series_by_provider_model_and_sessions() {
        let d = detail();
        let mut ui = StatsUi::default();
        let c = chart_data(&d, &ui, "Sessions", "Others");
        assert_eq!(c.dates, ["2026-11-10", "2026-11-11", "2026-11-12"]);
        assert_eq!(c.series.len(), 2);
        assert_eq!(c.series[0].0, "Claude");
        assert_eq!(c.series[0].1, vec![3.0, 0.5, 0.0]);
        assert_eq!(
            c.keys,
            [
                SeriesKey::Provider(ProviderKind::Claude),
                SeriesKey::Provider(ProviderKind::Codex)
            ]
        );
        ui.provider = Some(ProviderKind::Claude);
        ui.metric = Metric::Tokens;
        let c = chart_data(&d, &ui, "Sessions", "Others");
        assert_eq!(
            c.series.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
            ["opus", "sonnet"]
        );
        assert_eq!(c.series[0].1, vec![60.0, 0.0, 0.0]);
        ui.metric = Metric::Sessions;
        let c = chart_data(&d, &ui, "Sessions", "Others");
        assert_eq!(
            c.series,
            vec![("Sessions".to_string(), vec![1.0, 0.0, 0.0])]
        );
        assert_eq!(c.keys, [SeriesKey::Provider(ProviderKind::Claude)]);
        let all = chart_data(
            &d,
            &StatsUi {
                metric: Metric::Sessions,
                ..Default::default()
            },
            "Sessions",
            "Others",
        );
        assert_eq!(all.keys, [SeriesKey::Total]);
    }

    #[test]
    fn chart_merges_the_tail_into_others() {
        let mut d = detail();
        d.facts = (0..7)
            .map(|i| {
                fact(
                    "2026-11-11",
                    ProviderKind::Claude,
                    &format!("m{i}"),
                    10,
                    7.0 - i as f64,
                )
            })
            .collect();
        let ui = StatsUi {
            provider: Some(ProviderKind::Claude),
            ..Default::default()
        };
        let c = chart_data(&d, &ui, "Sessions", "Others");
        assert_eq!(c.series.len(), CHART_SERIES);
        assert_eq!(c.series[0].0, "m0");
        assert_eq!(c.series[4].0, "Others");
        assert_eq!(c.keys.len(), c.series.len());
        assert_eq!(c.keys[4], SeriesKey::Others);
        assert_eq!(
            c.keys[0],
            SeriesKey::Model(ProviderKind::Claude, "m0".into())
        );
        // m4 + m5 + m6 = 3 + 2 + 1.
        assert_eq!(c.series[4].1[1], 6.0);
    }

    #[test]
    fn model_share_list() {
        let d = detail();
        let m = model_shares(&d, &UsageFilter::all());
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].model, "opus");
        assert!((m.iter().map(|x| x.share).sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(model_shares(&d, &UsageFilter::provider(ProviderKind::Glm)).is_empty());
        let ranks = model_ranks(&d);
        // Claude: opus (3 $ in the period) before sonnet (0.5 $); Codex restarts at 0.
        assert_eq!(rank_of(&ranks, &ProviderKind::Claude, "opus"), 0);
        assert_eq!(rank_of(&ranks, &ProviderKind::Claude, "sonnet"), 1);
        assert_eq!(rank_of(&ranks, &ProviderKind::Codex, "gpt"), 0);
        assert_eq!(rank_of(&ranks, &ProviderKind::Codex, "nope"), 0);
    }

    #[test]
    fn heat_grid_is_monday_first_and_ends_today() {
        let d = detail();
        let days = d.heatmap(&UsageFilter::all());
        let cells = heat_grid(&days, HEAT_COLS);
        // 2026-11-12 is a Thursday: 52 full weeks + Mon..Thu.
        assert_eq!(cells.len(), 52 * 7 + 4);
        let last = cells.last().unwrap();
        assert_eq!(
            (last.date.as_str(), last.col, last.row),
            ("2026-11-12", 52, 3)
        );
        let first = &cells[0];
        assert_eq!((first.col, first.row), (0, 0));
        assert_eq!(
            parse(&first.date).unwrap().weekday().num_days_from_monday(),
            0
        );
        let busiest = cells.iter().find(|c| c.date == "2026-11-10").unwrap();
        assert_eq!(busiest.level, 4);
        // 20 of 60 tokens: second quarter.
        let light = cells.iter().find(|c| c.date == "2026-11-08").unwrap();
        assert_eq!(light.level, 2);
        assert!(cells.iter().filter(|c| c.level > 0).count() == 3);

        // Fewer days than the grid: right-aligned on today.
        let short = heat_grid(&days[days.len() - 3..], HEAT_COLS);
        assert_eq!(short.len(), 3);
        assert_eq!((short[2].col, short[2].row), (52, 3));
        assert_eq!((short[0].col, short[0].row), (52, 1));
        assert!(heat_grid(&[], HEAT_COLS).is_empty());
        // Fewer weeks: still ending today in the last column.
        let few = heat_grid(&days, 40);
        assert_eq!(few.len(), 39 * 7 + 4);
        let last = few.last().unwrap();
        assert_eq!(
            (last.date.as_str(), last.col, last.row),
            ("2026-11-12", 39, 3)
        );
    }

    #[test]
    fn month_label_positions() {
        let d = detail();
        let cells = heat_grid(&d.heatmap(&UsageFilter::all()), HEAT_COLS);
        let labels = month_labels(&cells, 3);
        // First column starts Mon 2025-11-10: Nov, then one label per month start.
        assert_eq!(labels.first(), Some(&(0, 11)));
        assert_eq!(labels.len(), 13);
        assert_eq!(labels.last().map(|l| l.1), Some(11));
        assert!(labels.windows(2).all(|w| w[1].0 > w[0].0));
        // December 2025 starts on Monday 2025-12-01, column 3.
        assert_eq!(labels[1], (3, 12));
        // Labels wider than a month: never closer than min_cols, the first one gives way.
        let wide = month_labels(&cells, 6);
        assert!(wide.windows(2).all(|w| w[1].0 - w[0].0 >= 6));
        assert_eq!(wide.first(), Some(&(3, 12)));
    }

    #[test]
    fn layout_helpers() {
        // KPI badges and trends.
        assert_eq!(signed_pct(Lang::Fr, 0.125), "+12,5\u{202f}%");
        assert_eq!(signed_pct(Lang::En, -0.032), "\u{2212}3.2%");
        assert_eq!(signed_pct(Lang::En, 0.0004), "0%");
        assert_eq!(Trend::of(0.2), Trend::Up);
        assert_eq!(Trend::of(-0.2), Trend::Down);
        assert_eq!(Trend::of(0.004), Trend::Flat);
        assert_eq!(Trend::Flat.arrow(), None);
        // 4 across from 4 × 256 + 3 gaps, then 2 × 2, then stacked.
        assert_eq!(kpi_columns(1120.0, 16.0), 4);
        assert_eq!(kpi_columns(1072.0, 16.0), 4);
        assert_eq!(kpi_columns(1071.0, 16.0), 2);
        assert_eq!(kpi_columns(806.0, 16.0), 2);
        assert_eq!(kpi_columns(528.0, 16.0), 2);
        assert_eq!(kpi_columns(527.0, 16.0), 1);
        assert_eq!(habit_columns(806.0), 3);
        assert_eq!(habit_columns(762.0), 2);
        assert_eq!(heat_weeks(536.0), 41);
        assert_eq!(heat_weeks(5000.0), HEAT_COLS);
        assert_eq!(heat_weeks(10.0), 4);
        // Room for 53 × 11 + 52 × 3 = 739 px.
        assert_eq!(grid_cell(739.0, 53, 14.0), (11.0, 3.0));
        assert_eq!(grid_cell(2000.0, 53, 14.0), (14.0, 3.0));
        let (cell, gap) = grid_cell(369.5, 53, 14.0);
        assert!((cell - 5.5).abs() < 1e-3 && (gap - 1.5).abs() < 1e-3);
    }
}
