//! Aggregation of cached buckets into the analytics report.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{Datelike, Duration, NaiveDate, TimeZone, Timelike};
use codenotch_core::analytics::{
    fact_days, AnalyticsDetail, HourCell, SessionActivity, SessionFact, SharedDetail, UsageFact,
    HEATMAP_DAYS,
};
use codenotch_core::model::{
    AnalyticsSummary, DailyUsage, ModelUsage, ProviderKind, ProviderUsage, TokenCounts,
};
use codenotch_core::{Account, Config, Settings};
use serde::Serialize;
use serde_json::Value;

use crate::cache::{Cache, Tokens};
use crate::pricing::{PriceTable, Rates};

/// Average month length used to pro-rate plan prices (365.2425 / 12 days). Same value as
/// `codenotch_core::DAYS_PER_MONTH` (the UI uses that one).
pub const DAYS_PER_MONTH: f64 = codenotch_core::analytics::DAYS_PER_MONTH;
/// Upper bound for `analyticsDays`.
pub const MAX_DAYS: u64 = 366;

/// Tokens plus their API-equivalent cost.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// input / output / cacheRead / cacheWrite (cacheWrite = all cache writes).
    pub tokens: TokenCounts,
    /// Part of `tokens.cache_write` written with a 1-hour TTL.
    pub cache_write_1h: u64,
    /// USD cost of the **priced** tokens only.
    pub cost_usd: f64,
    /// Tokens of models without a known price (not included in `cost_usd`).
    pub unpriced_tokens: u64,
}

impl Usage {
    /// Add `t` priced at `rates` (`None` = unpriced).
    pub fn add_tokens(&mut self, t: &Tokens, rates: Option<Rates>) {
        self.tokens.add(&to_counts(t));
        self.cache_write_1h += t.cache_write_1h;
        match rates {
            Some(r) => self.cost_usd += cost_of(t, &r),
            None => self.unpriced_tokens += t.total(),
        }
    }
    pub fn add(&mut self, o: &Usage) {
        self.tokens.add(&o.tokens);
        self.cache_write_1h += o.cache_write_1h;
        self.cost_usd += o.cost_usd;
        self.unpriced_tokens += o.unpriced_tokens;
    }
    /// Every token has a price.
    pub fn fully_priced(&self) -> bool {
        self.unpriced_tokens == 0
    }
    /// Cost when every token is priced, else `None` (unknown, not zero).
    pub fn complete_cost_usd(&self) -> Option<f64> {
        self.fully_priced().then_some(self.cost_usd)
    }
    /// Priced cost in EUR.
    pub fn cost_eur(&self, usd_to_eur: f64) -> f64 {
        self.cost_usd * usd_to_eur
    }
}

/// USD cost of `t` at `r` (cache reads and both cache-write TTLs at their own rates).
pub fn cost_of(t: &Tokens, r: &Rates) -> f64 {
    let w1h = t.cache_write_1h.min(t.cache_write);
    let w5m = t.cache_write - w1h;
    (t.input as f64 * r.base.input
        + t.output as f64 * r.base.output
        + t.cache_read as f64 * r.base.cache_read
        + w5m as f64 * r.base.cache_write
        + w1h as f64 * r.cache_write_1h)
        / 1e6
}

pub(crate) fn to_counts(t: &Tokens) -> TokenCounts {
    TokenCounts {
        input: t.input,
        output: t.output,
        cache_read: t.cache_read,
        cache_write: t.cache_write,
    }
}

/// Usage of one CLI (provider).
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliUsage {
    pub provider: ProviderKind,
    pub usage: Usage,
    /// Transcript files (sessions) with usage in the period.
    pub sessions: u32,
}

/// Usage of one model.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReport {
    /// Model id as written by the CLI (`-fast` appended for Claude fast-mode turns).
    pub model: String,
    pub provider: ProviderKind,
    /// `false` = no price known: `usage.cost_usd` is 0 and all tokens are unpriced.
    pub priced: bool,
    pub usage: Usage,
}

/// Usage of one project (working directory).
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectReport {
    /// Full working directory as recorded (`""` = unknown). The UI shortens it for display.
    pub project: String,
    pub usage: Usage,
    pub per_cli: Vec<CliUsage>,
}

/// One local day.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayReport {
    /// Local date `YYYY-MM-DD`.
    pub date: String,
    /// Unix seconds of local midnight.
    pub start: i64,
    pub usage: Usage,
    pub per_cli: Vec<CliUsage>,
}

/// One hour of the last 24 h.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HourReport {
    /// Unix seconds at the start of the hour.
    pub start: i64,
    pub usage: Usage,
    pub per_cli: Vec<CliUsage>,
}

/// API-equivalent cost of a provider over the period vs. its subscription price.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanComparison {
    pub provider: ProviderKind,
    /// Sum of the matching `planPricesEur` entries (EUR per month).
    pub plan_eur_month: f64,
    /// Plan price pro-rated to the period (`month × days / 30.44`).
    pub plan_eur_period: f64,
    /// API-equivalent cost of the priced tokens in the period.
    pub api_cost_usd: f64,
    pub api_cost_eur: f64,
    /// `api_cost_eur / plan_eur_period` (> 1 = the subscription saved money).
    pub ratio: Option<f64>,
    /// `api_cost_eur − plan_eur_period`.
    pub savings_eur: f64,
    /// Tokens left out of `api_cost_*` (no price) — the API figure is then a lower bound.
    pub unpriced_tokens: u64,
}

/// Indexing counters of the run that produced a report (no paths).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportStats {
    pub files: u32,
    pub files_read: u32,
    pub files_restarted: u32,
    pub bytes_read: u64,
    pub malformed_lines: u64,
    pub read_errors: u32,
    pub elapsed_ms: u64,
    /// `false` when the cache could not be written (next run re-reads everything).
    pub cache_saved: bool,
}

/// Everything the analytics view shows. `summary` is the core-facing subset.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsReport {
    pub summary: AnalyticsSummary,
    pub days: u32,
    /// Local midnight starting the period (unix secs).
    pub period_start: i64,
    pub generated_at: i64,
    pub usd_to_eur: f64,
    pub total: Usage,
    pub per_cli: Vec<CliUsage>,
    /// Most expensive first; unpriced models after priced ones, by tokens.
    pub per_model: Vec<ModelReport>,
    /// Most expensive first.
    pub per_project: Vec<ProjectReport>,
    /// Oldest first, every day of the period (zero-filled).
    pub per_day: Vec<DayReport>,
    /// Oldest first, the last 24 hours including the current one (zero-filled).
    pub per_hour: Vec<HourReport>,
    /// Providers with a plan price in `planPricesEur`.
    pub plans: Vec<PlanComparison>,
    /// Model ids seen in the period without a price.
    pub unpriced_models: Vec<String>,
    pub stats: ReportStats,
}

impl AnalyticsReport {
    /// Total cost in EUR (priced tokens).
    pub fn total_cost_eur(&self) -> f64 {
        self.total.cost_eur(self.usd_to_eur)
    }
}

pub(crate) fn provider_of(cli: &str) -> ProviderKind {
    ProviderKind::from_id(cli)
}

fn add_cli(list: &mut Vec<CliUsage>, provider: &ProviderKind, t: &Tokens, rates: Option<Rates>) {
    match list.iter_mut().find(|c| &c.provider == provider) {
        Some(c) => c.usage.add_tokens(t, rates),
        None => {
            let mut c = CliUsage {
                provider: provider.clone(),
                ..Default::default()
            };
            c.usage.add_tokens(t, rates);
            list.push(c);
        }
    }
}

fn cli_rank(p: &ProviderKind) -> usize {
    ProviderKind::CANONICAL_ORDER
        .iter()
        .position(|k| k == p)
        .unwrap_or(usize::MAX)
}

fn sort_cli(list: &mut [CliUsage]) {
    list.sort_by_key(|c| cli_rank(&c.provider));
}

fn cmp_usage(a: &Usage, b: &Usage) -> std::cmp::Ordering {
    b.cost_usd
        .total_cmp(&a.cost_usd)
        .then(b.tokens.total().cmp(&a.tokens.total()))
}

/// A plan price value: a number, or a numeric string (`"21.60"`, `"21,60"`, `"21.60 €"`).
fn price_value(v: &Value) -> Option<f64> {
    let f = match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let cleaned: String = s
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
                .collect();
            cleaned.replace(',', ".").parse::<f64>().ok()
        }
        _ => None,
    }?;
    (f.is_finite() && f >= 0.0).then_some(f)
}

fn lookup(settings: &Settings, key: &str) -> Option<f64> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    settings
        .plan_prices_eur
        .get(key)
        .or_else(|| {
            settings
                .plan_prices_eur
                .iter()
                .find(|(k, _)| k.trim().eq_ignore_ascii_case(key))
                .map(|(_, v)| v)
        })
        .and_then(price_value)
}

/// Monthly plan price (EUR) of `account` from `planPricesEur`. The written format is
/// `codenotch_core::config::plan_price_key` (`"Claude Max"`, or `"Claude"` without a plan), as
/// saved by the settings window. Keys tried in order: the account id, the account name,
/// `"<Provider> <plan>"`, the bare plan name (e.g. `"Max 20x"`, from the provider reading),
/// `"<Provider>"` (display name), the provider id (`"claude"`, `"codex"`). Case-insensitive.
pub fn plan_price_eur(
    settings: &Settings,
    account: &Account,
    plan_name: Option<&str>,
) -> Option<f64> {
    use codenotch_core::config::plan_price_key;
    lookup(settings, &account.id)
        .or_else(|| lookup(settings, &account.name))
        .or_else(|| {
            plan_name.and_then(|p| lookup(settings, &plan_price_key(&account.provider, Some(p))))
        })
        .or_else(|| plan_name.and_then(|p| lookup(settings, p)))
        .or_else(|| lookup(settings, &plan_price_key(&account.provider, None)))
        .or_else(|| lookup(settings, account.provider.id()))
}

/// Sum of the monthly plan prices of the enabled accounts of `provider`; when none is
/// configured for an account, the provider-id key alone (once).
pub fn provider_plan_eur(
    config: &Config,
    provider: &ProviderKind,
    plan_names: &HashMap<String, String>,
) -> Option<f64> {
    let mut sum = None;
    for a in config
        .enabled_accounts()
        .filter(|a| &a.provider == provider)
    {
        if let Some(p) = plan_price_eur(
            &config.settings,
            a,
            plan_names.get(&a.id).map(String::as_str),
        ) {
            *sum.get_or_insert(0.0) += p;
        }
    }
    sum.or_else(|| {
        lookup(
            &config.settings,
            &codenotch_core::config::plan_price_key(provider, None),
        )
    })
    .or_else(|| lookup(&config.settings, provider.id()))
}

/// Compare `usage` with a monthly plan price pro-rated to `days`.
pub fn compare_plan(
    provider: ProviderKind,
    usage: &Usage,
    plan_eur_month: f64,
    days: u32,
    usd_to_eur: f64,
) -> PlanComparison {
    let plan_eur_period = plan_eur_month * f64::from(days) / DAYS_PER_MONTH;
    let api_cost_eur = usage.cost_eur(usd_to_eur);
    PlanComparison {
        provider,
        plan_eur_month,
        plan_eur_period,
        api_cost_usd: usage.cost_usd,
        api_cost_eur,
        ratio: (plan_eur_period > 0.0).then(|| api_cost_eur / plan_eur_period),
        savings_eur: api_cost_eur - plan_eur_period,
        unpriced_tokens: usage.unpriced_tokens,
    }
}

/// Days in the period: `analyticsDays` clamped to 1..=366.
pub fn period_days(settings: &Settings) -> u32 {
    settings.analytics_days.clamp(1, MAX_DAYS) as u32
}

/// `usdToEur`, or the 0.92 default when the setting is not a positive finite number.
pub fn fx_rate(settings: &Settings) -> f64 {
    let v = settings.usd_to_eur;
    if v.is_finite() && v > 0.0 {
        v
    } else {
        0.92
    }
}

fn local_midnight<Tz: TimeZone>(tz: &Tz, date: NaiveDate) -> i64 {
    let naive = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    tz.from_local_datetime(&naive)
        .earliest()
        // Midnight skipped by a DST jump: take 01:00.
        .or_else(|| {
            tz.from_local_datetime(&(naive + Duration::hours(1)))
                .earliest()
        })
        .map(|t| t.timestamp())
        .unwrap_or_else(|| naive.and_utc().timestamp())
}

fn local_date<Tz: TimeZone>(tz: &Tz, ts: i64) -> NaiveDate {
    tz.timestamp_opt(ts, 0)
        .earliest()
        .map(|t| t.date_naive())
        .unwrap_or_default()
}

/// Start of the period: local midnight `days − 1` days before today.
pub fn period_start<Tz: TimeZone>(tz: &Tz, now: i64, days: u32) -> i64 {
    let today = local_date(tz, now);
    local_midnight(
        tz,
        today - Duration::days(i64::from(days.saturating_sub(1))),
    )
}

/// Build the report from the cached buckets. Buckets are hour-aligned; an hour belongs to
/// the local day its start falls in.
pub fn build<Tz: TimeZone>(
    cache: &Cache,
    prices: &PriceTable,
    config: &Config,
    plan_names: &HashMap<String, String>,
    now: i64,
    tz: &Tz,
) -> AnalyticsReport {
    let days = period_days(&config.settings);
    let fx = fx_rate(&config.settings);
    let start = period_start(tz, now, days);
    let hour_now = crate::parse::hour_of(now);
    let hours_from = hour_now - 23 * 3600;

    // Day skeleton.
    let today = local_date(tz, now);
    let mut per_day: Vec<DayReport> = (0..days)
        .rev()
        .map(|back| {
            let date = today - Duration::days(i64::from(back));
            DayReport {
                date: date.format("%Y-%m-%d").to_string(),
                start: local_midnight(tz, date),
                ..Default::default()
            }
        })
        .collect();
    let day_index: HashMap<String, usize> = per_day
        .iter()
        .enumerate()
        .map(|(i, d)| (d.date.clone(), i))
        .collect();
    let mut per_hour: Vec<HourReport> = (0..24)
        .map(|i| HourReport {
            start: hours_from + i * 3600,
            ..Default::default()
        })
        .collect();

    let mut total = Usage::default();
    let mut per_cli: Vec<CliUsage> = Vec::new();
    let mut models: BTreeMap<(String, String), ModelReport> = BTreeMap::new();
    let mut projects: BTreeMap<String, ProjectReport> = BTreeMap::new();
    let mut rate_cache: HashMap<String, Option<Rates>> = HashMap::new();

    for entry in cache.files.values() {
        let mut file_clis: Vec<ProviderKind> = Vec::new();
        for b in &entry.buckets {
            if b.hour < start || b.hour > now {
                continue;
            }
            let rates = *rate_cache
                .entry(b.model.clone())
                .or_insert_with(|| prices.rates(&b.model));
            let provider = provider_of(&b.cli);
            if !file_clis.contains(&provider) {
                file_clis.push(provider.clone());
            }
            total.add_tokens(&b.tokens, rates);
            add_cli(&mut per_cli, &provider, &b.tokens, rates);

            let m = models
                .entry((b.cli.clone(), b.model.clone()))
                .or_insert_with(|| ModelReport {
                    model: b.model.clone(),
                    provider: provider.clone(),
                    priced: rates.is_some(),
                    usage: Usage::default(),
                });
            m.usage.add_tokens(&b.tokens, rates);

            let p = projects
                .entry(b.project.clone())
                .or_insert_with(|| ProjectReport {
                    project: b.project.clone(),
                    ..Default::default()
                });
            p.usage.add_tokens(&b.tokens, rates);
            add_cli(&mut p.per_cli, &provider, &b.tokens, rates);

            let date = local_date(tz, b.hour).format("%Y-%m-%d").to_string();
            if let Some(&i) = day_index.get(&date) {
                per_day[i].usage.add_tokens(&b.tokens, rates);
                add_cli(&mut per_day[i].per_cli, &provider, &b.tokens, rates);
            }
            if b.hour >= hours_from {
                let i = ((b.hour - hours_from) / 3600) as usize;
                if let Some(h) = per_hour.get_mut(i) {
                    h.usage.add_tokens(&b.tokens, rates);
                    add_cli(&mut h.per_cli, &provider, &b.tokens, rates);
                }
            }
        }
        for p in file_clis {
            if let Some(c) = per_cli.iter_mut().find(|c| c.provider == p) {
                c.sessions += 1;
            }
        }
    }

    sort_cli(&mut per_cli);
    for d in &mut per_day {
        sort_cli(&mut d.per_cli);
    }
    for h in &mut per_hour {
        sort_cli(&mut h.per_cli);
    }
    let mut per_model: Vec<ModelReport> = models.into_values().collect();
    per_model.sort_by(|a, b| b.priced.cmp(&a.priced).then(cmp_usage(&a.usage, &b.usage)));
    let mut per_project: Vec<ProjectReport> = projects.into_values().collect();
    for p in &mut per_project {
        sort_cli(&mut p.per_cli);
    }
    per_project.sort_by(|a, b| cmp_usage(&a.usage, &b.usage));
    let unpriced_models: Vec<String> = per_model
        .iter()
        .filter(|m| !m.priced)
        .map(|m| m.model.clone())
        .collect();

    // Plan comparison for every provider with a configured price.
    let mut plan_providers: Vec<ProviderKind> =
        per_cli.iter().map(|c| c.provider.clone()).collect();
    for kind in [ProviderKind::Claude, ProviderKind::Codex] {
        if !plan_providers.contains(&kind) {
            plan_providers.push(kind);
        }
    }
    let plans: Vec<PlanComparison> = plan_providers
        .into_iter()
        .filter_map(|p| {
            let month = provider_plan_eur(config, &p, plan_names)?;
            let usage = per_cli
                .iter()
                .find(|c| c.provider == p)
                .map(|c| c.usage)
                .unwrap_or_default();
            Some(compare_plan(p, &usage, month, days, fx))
        })
        .collect();

    // Core summary.
    let mut daily = Vec::new();
    for d in &per_day {
        for c in &per_cli {
            let u = d
                .per_cli
                .iter()
                .find(|x| x.provider == c.provider)
                .map(|x| x.usage)
                .unwrap_or_default();
            daily.push(DailyUsage {
                date: d.date.clone(),
                provider: c.provider.clone(),
                tokens: u.tokens,
                cost_usd: u.cost_usd,
                ..Default::default()
            });
        }
    }
    let detail = build_detail(
        cache,
        &mut rate_cache,
        prices,
        now,
        tz,
        days,
        &unpriced_models,
    );
    let summary = AnalyticsSummary {
        detail: SharedDetail::new(detail),
        days,
        generated_at: now,
        total_tokens: total.tokens,
        total_cost_usd: total.cost_usd,
        per_provider: per_cli
            .iter()
            .map(|c| ProviderUsage {
                provider: c.provider.clone(),
                tokens: c.usage.tokens,
                cost_usd: c.usage.cost_usd,
                sessions: c.sessions,
                ..Default::default()
            })
            .collect(),
        per_day: daily,
        per_model: per_model
            .iter()
            .map(|m| ModelUsage {
                model: m.model.clone(),
                provider: m.provider.clone(),
                tokens: m.usage.tokens,
                cost_usd: m.usage.cost_usd,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };

    AnalyticsReport {
        summary,
        days,
        period_start: start,
        generated_at: now,
        usd_to_eur: fx,
        total,
        per_cli,
        per_model,
        per_project,
        per_day,
        per_hour,
        plans,
        unpriced_models,
        stats: ReportStats::default(),
    }
}

/// Local (date, weekday 0 = Monday, hour) of a bucket start.
fn local_slot<Tz: TimeZone>(tz: &Tz, ts: i64) -> (String, u8, u8) {
    match tz.timestamp_opt(ts, 0).earliest() {
        Some(t) => (
            t.date_naive().format("%Y-%m-%d").to_string(),
            t.weekday().num_days_from_monday() as u8,
            t.hour() as u8,
        ),
        None => (String::new(), 0, 0),
    }
}

/// The statistics fact tables: per (local date, cli, model) over `fact_days(days)` days, the
/// weekday × hour grid over the period, and one session row per file.
fn build_detail<Tz: TimeZone>(
    cache: &Cache,
    rate_cache: &mut HashMap<String, Option<Rates>>,
    prices: &PriceTable,
    now: i64,
    tz: &Tz,
    days: u32,
    unpriced_models: &[String],
) -> AnalyticsDetail {
    let today = local_date(tz, now);
    let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
    let back = |n: u32| today - Duration::days(i64::from(n.saturating_sub(1)));
    let fdays = fact_days(days);
    let facts_from = back(fdays);
    let facts_start = local_midnight(tz, facts_from);
    let period_from = fmt(back(days));
    let previous_to = back(days) - Duration::days(1);
    let previous_from = previous_to - Duration::days(i64::from(days.saturating_sub(1)));

    let mut slots: HashMap<i64, (String, u8, u8)> = HashMap::new();
    let mut facts: BTreeMap<(String, String, String), (Usage, u32)> = BTreeMap::new();
    let mut hours: BTreeMap<(String, String, u8, u8), Usage> = BTreeMap::new();
    let mut sessions: Vec<SessionFact> = Vec::new();

    for entry in cache.files.values() {
        // cli → (date, model) pairs of this file.
        let mut active: BTreeMap<&str, BTreeSet<(String, String)>> = BTreeMap::new();
        for b in &entry.buckets {
            if b.hour < facts_start || b.hour > now {
                continue;
            }
            let rates = *rate_cache
                .entry(b.model.clone())
                .or_insert_with(|| prices.rates(&b.model));
            let (date, weekday, hour) = slots
                .entry(b.hour)
                .or_insert_with(|| local_slot(tz, b.hour))
                .clone();
            if date.is_empty() {
                continue;
            }
            let f = facts
                .entry((date.clone(), b.cli.clone(), b.model.clone()))
                .or_default();
            f.0.add_tokens(&b.tokens, rates);
            let pair = (date.clone(), b.model.clone());
            if active.entry(b.cli.as_str()).or_default().insert(pair) {
                f.1 += 1;
            }
            if date >= period_from {
                hours
                    .entry((b.cli.clone(), b.model.clone(), weekday, hour))
                    .or_default()
                    .add_tokens(&b.tokens, rates);
            }
        }
        for (cli, pairs) in active {
            sessions.push(SessionFact {
                provider: provider_of(cli),
                activity: pairs
                    .into_iter()
                    .map(|(date, model)| SessionActivity { date, model })
                    .collect(),
            });
        }
    }

    let mut facts: Vec<UsageFact> = facts
        .into_iter()
        .map(|((date, cli, model), (u, sessions))| UsageFact {
            date,
            provider: provider_of(&cli),
            model,
            tokens: u.tokens,
            cost_usd: u.cost_usd,
            unpriced_tokens: u.unpriced_tokens,
            sessions,
        })
        .collect();
    facts.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then(cli_rank(&a.provider).cmp(&cli_rank(&b.provider)))
            .then(a.model.cmp(&b.model))
    });
    let mut hours: Vec<HourCell> = hours
        .into_iter()
        .map(|((cli, model, weekday, hour), u)| HourCell {
            provider: provider_of(&cli),
            model,
            weekday,
            hour,
            tokens: u.tokens.total(),
            cost_usd: u.cost_usd,
            unpriced_tokens: u.unpriced_tokens,
        })
        .collect();
    hours.sort_by(|a, b| {
        cli_rank(&a.provider)
            .cmp(&cli_rank(&b.provider))
            .then(a.model.cmp(&b.model))
            .then(a.weekday.cmp(&b.weekday))
            .then(a.hour.cmp(&b.hour))
    });
    sessions.sort_by_key(|s| cli_rank(&s.provider));

    AnalyticsDetail {
        today: fmt(today),
        period_from,
        previous_from: fmt(previous_from),
        previous_to: fmt(previous_to),
        heatmap_from: fmt(back(HEATMAP_DAYS)),
        facts_from: fmt(facts_from),
        facts,
        hours,
        sessions,
        unpriced_models: unpriced_models.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{Bucket, FileEntry};
    use crate::parse::parse_timestamp;
    use chrono::FixedOffset;

    fn ts(s: &str) -> i64 {
        parse_timestamp(s).expect("ts")
    }

    fn bucket(hour: &str, cli: &str, model: &str, project: &str, t: Tokens) -> Bucket {
        Bucket {
            hour: ts(hour),
            source: "r".into(),
            cli: cli.into(),
            model: model.into(),
            project: project.into(),
            tokens: t,
        }
    }

    fn tok(input: u64, output: u64) -> Tokens {
        Tokens {
            input,
            output,
            ..Default::default()
        }
    }

    fn config(days: u64) -> Config {
        let mut c = Config::default();
        c.settings.analytics_days = days;
        c.settings.usd_to_eur = 0.5;
        c
    }

    #[test]
    fn cost_uses_each_rate() {
        let r = crate::pricing::builtin_rates("claude-opus-5-5").expect("priced");
        let t = Tokens {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 3_000_000,
            cache_write_1h: 1_000_000,
        };
        // 4 + 20 + 0.2 + 2 × 5 + 1 × 8
        assert!((cost_of(&t, &r) - 42.2).abs() < 1e-9);
        assert_eq!(
            crate::cost_usd("claude-opus-5-5", &to_counts(&tok(1_000_000, 0))),
            Some(4.0)
        );
    }

    #[test]
    fn buckets_by_local_day_and_hour_and_period() {
        let tz = FixedOffset::east_opt(2 * 3600).expect("tz"); // UTC+2
        let now = ts("2026-11-12T10:30:00Z"); // 12:30 local on the 12th
        let mut e = FileEntry::default();
        // 21:00Z on the 11th = 23:00 local on the 11th; 22:00Z on the 11th = 00:00 local on the 12th.
        e.buckets.push(bucket(
            "2026-11-11T21:00:00Z",
            "claude",
            "claude-opus-5-5",
            "C:\\a",
            tok(1, 0),
        ));
        e.buckets.push(bucket(
            "2026-11-11T22:00:00Z",
            "claude",
            "claude-opus-5-5",
            "C:\\a",
            tok(2, 0),
        ));
        // 21:00Z on the 10th = 23:00 local on the 10th: outside a 2-day period.
        e.buckets.push(bucket(
            "2026-11-10T21:00:00Z",
            "claude",
            "claude-opus-5-5",
            "C:\\a",
            tok(100, 0),
        ));
        // Future bucket ignored.
        e.buckets.push(bucket(
            "2026-11-12T12:00:00Z",
            "claude",
            "claude-opus-5-5",
            "C:\\a",
            tok(1000, 0),
        ));
        let mut cache = Cache::empty();
        cache.files.insert("f".into(), e);
        let r = build(
            &cache,
            &PriceTable::builtin(),
            &config(2),
            &HashMap::new(),
            now,
            &tz,
        );
        assert_eq!(r.period_start, ts("2026-11-10T22:00:00Z"));
        assert_eq!(
            r.per_day
                .iter()
                .map(|d| d.date.as_str())
                .collect::<Vec<_>>(),
            ["2026-11-11", "2026-11-12"]
        );
        assert_eq!(r.per_day[0].usage.tokens.input, 1);
        assert_eq!(r.per_day[1].usage.tokens.input, 2);
        assert_eq!(r.total.tokens.input, 3);
        assert_eq!(r.per_hour.len(), 24);
        assert_eq!(r.per_hour[23].start, ts("2026-11-12T10:00:00Z"));
        assert_eq!(r.per_hour[0].start, ts("2026-11-11T11:00:00Z"));
        let h21 = r
            .per_hour
            .iter()
            .find(|h| h.start == ts("2026-11-11T21:00:00Z"))
            .expect("hour");
        assert_eq!(h21.usage.tokens.input, 1);
        assert_eq!(r.summary.per_day.len(), 2);
        assert_eq!(r.summary.per_provider[0].sessions, 1);
    }

    #[test]
    fn unpriced_models_are_counted_not_zero_priced() {
        let tz = FixedOffset::east_opt(0).expect("tz");
        let now = ts("2026-11-12T10:30:00Z");
        let mut a = FileEntry::default();
        a.buckets.push(bucket(
            "2026-11-12T09:00:00Z",
            "claude",
            "claude-sonnet-5-5",
            "C:\\a",
            tok(1_000_000, 0),
        ));
        let mut b = FileEntry::default();
        b.buckets.push(bucket(
            "2026-11-12T09:00:00Z",
            "codex",
            "gpt-6-luna",
            "C:\\b",
            tok(500, 20),
        ));
        b.buckets.push(bucket(
            "2026-11-12T10:00:00Z",
            "codex",
            "gpt-6-luna",
            "C:\\b",
            tok(500, 0),
        ));
        let mut cache = Cache::empty();
        cache.files.insert("a".into(), a);
        cache.files.insert("b".into(), b);
        let r = build(
            &cache,
            &PriceTable::builtin(),
            &config(7),
            &HashMap::new(),
            now,
            &tz,
        );
        assert!((r.total.cost_usd - 2.0).abs() < 1e-9);
        assert_eq!(r.total.unpriced_tokens, 1020);
        assert_eq!(r.total.complete_cost_usd(), None);
        assert_eq!(r.unpriced_models, ["gpt-6-luna"]);
        assert_eq!(r.per_model[0].model, "claude-sonnet-5-5");
        assert!(!r.per_model[1].priced);
        assert_eq!(
            r.per_cli
                .iter()
                .map(|c| c.provider.id())
                .collect::<Vec<_>>(),
            ["claude", "codex"]
        );
        let codex = &r.per_cli[1];
        assert_eq!(
            (
                codex.usage.cost_usd,
                codex.usage.unpriced_tokens,
                codex.sessions
            ),
            (0.0, 1020, 1)
        );
        assert_eq!(r.per_project[0].project, "C:\\a");
        assert!((r.total_cost_eur() - 1.0).abs() < 1e-9);
        // With a pricing.json entry it becomes priced.
        let t = PriceTable::parse(
            r#"{"models":{"gpt-6-luna":{"input":1,"output":10,"cacheRead":0.1,"cacheWrite":1}}}"#,
        );
        let r = build(&cache, &t, &config(7), &HashMap::new(), now, &tz);
        assert!(r.total.fully_priced());
        assert!((r.total.cost_usd - (2.0 + 1000.0 / 1e6 + 200.0 / 1e6)).abs() < 1e-12);
    }

    #[test]
    fn plan_comparison_pro_rates_and_converts() {
        let tz = FixedOffset::east_opt(0).expect("tz");
        let now = ts("2026-11-12T10:30:00Z");
        let mut a = FileEntry::default();
        // 10 M output tokens of Opus 5.5 = 200 USD.
        a.buckets.push(bucket(
            "2026-11-12T09:00:00Z",
            "claude",
            "claude-opus-5-5",
            "",
            tok(0, 10_000_000),
        ));
        let mut cache = Cache::empty();
        cache.files.insert("a".into(), a);
        let mut cfg = config(7);
        let mut acc = Account::new(ProviderKind::Claude, "Perso");
        acc.id = "acc-1".into();
        cfg.accounts.push(acc);
        cfg.settings
            .plan_prices_eur
            .insert("max 20x".into(), Value::from(180.0));
        cfg.settings
            .plan_prices_eur
            .insert("codex".into(), Value::from("21,60 €"));
        let plans: HashMap<String, String> = [("acc-1".to_string(), "Max 20x".to_string())].into();
        let r = build(&cache, &PriceTable::builtin(), &cfg, &plans, now, &tz);
        let claude = r
            .plans
            .iter()
            .find(|p| p.provider == ProviderKind::Claude)
            .expect("claude plan");
        assert_eq!(claude.plan_eur_month, 180.0);
        assert!((claude.plan_eur_period - 180.0 * 7.0 / DAYS_PER_MONTH).abs() < 1e-9);
        assert!((claude.api_cost_eur - 100.0).abs() < 1e-9);
        assert!((claude.ratio.expect("ratio") - 100.0 / claude.plan_eur_period).abs() < 1e-9);
        let codex = r
            .plans
            .iter()
            .find(|p| p.provider == ProviderKind::Codex)
            .expect("codex plan");
        assert_eq!(codex.plan_eur_month, 21.6);
        assert_eq!(codex.api_cost_eur, 0.0);
        // Without plan names, the Claude account has no matching key → no Claude comparison.
        let r = build(
            &cache,
            &PriceTable::builtin(),
            &cfg,
            &HashMap::new(),
            now,
            &tz,
        );
        assert!(r.plans.iter().all(|p| p.provider != ProviderKind::Claude));
    }

    #[test]
    fn plan_keys_order_and_values() {
        let mut s = Settings::default();
        let mut acc = Account::new(ProviderKind::Claude, "Work");
        acc.id = "id-9".into();
        assert_eq!(plan_price_eur(&s, &acc, None), None);
        s.plan_prices_eur.insert("claude".into(), Value::from(90));
        assert_eq!(plan_price_eur(&s, &acc, None), Some(90.0));
        s.plan_prices_eur
            .insert("WORK".into(), Value::from("18.00"));
        assert_eq!(plan_price_eur(&s, &acc, Some("Max")), Some(18.0));
        s.plan_prices_eur.insert("id-9".into(), Value::from(-3));
        assert_eq!(
            plan_price_eur(&s, &acc, None),
            Some(18.0),
            "negative ignored"
        );
        s.plan_prices_eur.insert("id-9".into(), Value::Bool(true));
        assert_eq!(plan_price_eur(&s, &acc, None), Some(18.0));
    }

    #[test]
    fn plan_keys_written_by_the_settings_window_are_found() {
        use codenotch_core::config::plan_price_key;
        let mut s = Settings::default();
        let acc = Account::new(ProviderKind::Claude, "Work");
        // "<Provider> <plan>" as written by the settings page.
        s.plan_prices_eur.insert(
            plan_price_key(&ProviderKind::Claude, Some("Max")),
            Value::from(180),
        );
        assert_eq!(plan_price_eur(&s, &acc, Some("Max")), Some(180.0));
        assert_eq!(plan_price_eur(&s, &acc, Some("Pro")), None);
        // "<Provider>" alone (plan unknown), also as the provider-level fallback.
        s.plan_prices_eur.insert(
            plan_price_key(&ProviderKind::Claude, None),
            Value::from("21,60"),
        );
        assert_eq!(plan_price_eur(&s, &acc, Some("Pro")), Some(21.6));
        assert_eq!(plan_price_eur(&s, &acc, None), Some(21.6));
        // The full key wins over the provider-only key; case-insensitive.
        s.plan_prices_eur
            .insert("claude max".into(), Value::from(100));
        s.plan_prices_eur.remove("Claude Max");
        assert_eq!(plan_price_eur(&s, &acc, Some("Max")), Some(100.0));
        let config = Config {
            settings: s,
            ..Default::default()
        };
        assert_eq!(
            provider_plan_eur(&config, &ProviderKind::Claude, &HashMap::new()),
            Some(21.6)
        );
    }

    #[test]
    fn detail_buckets_by_local_date_weekday_and_hour() {
        use codenotch_core::analytics::UsageFilter;
        let tz = FixedOffset::east_opt(2 * 3600).expect("tz"); // UTC+2
        let now = ts("2026-11-12T10:30:00Z"); // Thursday 12:30 local
        let mut e = FileEntry::default();
        // 21:00Z Wed 11th = 23:00 local Wed; 22:00Z = 00:00 local Thu 12th.
        e.buckets.push(bucket(
            "2026-11-11T21:00:00Z",
            "claude",
            "claude-opus-5-5",
            "p",
            tok(1, 0),
        ));
        e.buckets.push(bucket(
            "2026-11-11T22:00:00Z",
            "claude",
            "claude-sonnet-5-5",
            "p",
            tok(2, 0),
        ));
        let mut cache = Cache::empty();
        cache.files.insert("f".into(), e);
        let r = build(
            &cache,
            &PriceTable::builtin(),
            &config(2),
            &HashMap::new(),
            now,
            &tz,
        );
        let d = &r.summary.detail;
        assert_eq!(d.today, "2026-11-12");
        assert_eq!(d.period_from, "2026-11-11");
        assert_eq!(
            (d.previous_from.as_str(), d.previous_to.as_str()),
            ("2026-11-09", "2026-11-10")
        );
        assert_eq!(d.heatmap_from, "2025-11-07");
        assert_eq!(d.facts_from, "2025-11-07");
        assert_eq!(
            d.facts
                .iter()
                .map(|f| (
                    f.date.as_str(),
                    f.model.as_str(),
                    f.tokens.input,
                    f.sessions
                ))
                .collect::<Vec<_>>(),
            [
                ("2026-11-11", "claude-opus-5-5", 1, 1),
                ("2026-11-12", "claude-sonnet-5-5", 2, 1)
            ]
        );
        let cells: Vec<(u8, u8, u64)> = d
            .hours
            .iter()
            .map(|c| (c.weekday, c.hour, c.tokens))
            .collect();
        assert!(cells.contains(&(2, 23, 1)), "{cells:?}"); // Wednesday 23:00
        assert!(cells.contains(&(3, 0, 2)), "{cells:?}"); // Thursday 00:00
        let g = d.week_hour_tokens(&UsageFilter::all());
        assert_eq!((g[2][23], g[3][0]), (1, 2));
        assert_eq!(d.sessions.len(), 1);
        assert_eq!(d.sessions[0].activity.len(), 2);
        assert_eq!(d.period_totals(&UsageFilter::all()).sessions, 1);
    }

    #[test]
    fn detail_facts_add_up_to_the_report_and_cover_previous_and_heatmap() {
        use codenotch_core::analytics::{UsageFilter, HEATMAP_DAYS};
        let tz = FixedOffset::west_opt(5 * 3600).expect("tz"); // UTC-5
        let now = ts("2026-11-12T20:00:00Z");
        let models = [
            ("claude", "claude-opus-5-5"),
            ("claude", "claude-haiku-4-5"),
            ("codex", "gpt-5-codex"),
            ("codex", "gpt-6-luna"),
        ];
        let mut cache = Cache::empty();
        // 400 days of hourly-ish activity across several files and models.
        for day in 0..400i64 {
            let mut e = FileEntry::default();
            for (i, (cli, model)) in models.iter().enumerate() {
                let hour = now - day * 86_400 - (i as i64 * 5 + day % 7) * 3600;
                let hour = hour - hour.rem_euclid(3600);
                e.buckets.push(Bucket {
                    hour,
                    source: "r".into(),
                    cli: (*cli).into(),
                    model: (*model).into(),
                    project: "p".into(),
                    tokens: Tokens {
                        input: 1000 + day as u64,
                        output: 100,
                        cache_read: 50,
                        cache_write: 20,
                        cache_write_1h: 5,
                    },
                });
            }
            cache.files.insert(format!("f{day}"), e);
        }
        let r = build(
            &cache,
            &PriceTable::builtin(),
            &config(30),
            &HashMap::new(),
            now,
            &tz,
        );
        let d = &r.summary.detail;
        let all = d.period_totals(&UsageFilter::all());
        assert_eq!(all.tokens, r.total.tokens);
        assert!((all.cost_usd - r.total.cost_usd).abs() < 1e-9);
        assert_eq!(all.unpriced_tokens, r.total.unpriced_tokens);
        assert_eq!(d.unpriced_models, r.unpriced_models);
        for c in &r.per_cli {
            let t = d.period_totals(&UsageFilter::provider(c.provider.clone()));
            assert_eq!(t.tokens, c.usage.tokens);
            assert!((t.cost_usd - c.usage.cost_usd).abs() < 1e-9);
            assert_eq!(t.sessions, c.sessions);
        }
        for m in &r.per_model {
            let t = d.period_totals(&UsageFilter::model(m.provider.clone(), m.model.clone()));
            assert_eq!(t.tokens, m.usage.tokens);
            assert!((t.cost_usd - m.usage.cost_usd).abs() < 1e-9);
            assert_eq!(t.unpriced_tokens, m.usage.unpriced_tokens);
        }
        // Daily facts match the per-day report.
        let daily = d.daily(&UsageFilter::all(), &d.period_from, &d.today);
        assert_eq!(daily.len(), r.per_day.len());
        for ((date, t), day) in daily.iter().zip(&r.per_day) {
            assert_eq!(date, &day.date);
            assert_eq!(t.tokens, day.usage.tokens);
        }
        // Week × hour grid sums to the period total.
        let grid: u64 = d
            .week_hour_tokens(&UsageFilter::all())
            .iter()
            .flatten()
            .sum();
        assert_eq!(grid, r.total.tokens.total());
        // Previous period and heatmap are covered regardless of the 30-day period.
        let prev = d.previous_totals(&UsageFilter::all());
        assert!(prev.tokens.total() > 0);
        // One file per day holding both CLIs (two session rows); some straddle two local days.
        assert!((60..=62).contains(&prev.sessions), "{}", prev.sessions);
        let heat = d.heatmap(&UsageFilter::provider(ProviderKind::Codex));
        assert_eq!(heat.len(), HEATMAP_DAYS as usize);
        assert!(
            heat.iter().all(|(_, t)| t.tokens.total() > 0),
            "every day has codex usage"
        );
        assert!(d
            .facts
            .iter()
            .all(|f| f.date >= d.facts_from && f.date <= d.today));
        // The sum of facts equals the sum over every bucket in the fact range.
        let range = d.totals(&UsageFilter::all(), &d.facts_from, &d.today);
        assert_eq!(
            range.tokens.total(),
            d.facts.iter().map(|f| f.tokens.total()).sum::<u64>()
        );
    }

    #[test]
    fn settings_are_sanitised() {
        let mut s = Settings {
            analytics_days: 0,
            usd_to_eur: f64::NAN,
            ..Default::default()
        };
        assert_eq!((period_days(&s), fx_rate(&s)), (1, 0.92));
        s.analytics_days = 10_000;
        assert_eq!(period_days(&s), 366);
    }
}
