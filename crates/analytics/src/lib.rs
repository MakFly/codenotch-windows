//! # codenotch-analytics (owner: agent D)
//!
//! Token and cost analytics from local transcripts:
//! * Claude Code: `<profile>\projects\**\*.jsonl` (assistant `message.usage` + `message.model`),
//!   for the default profile (`CLAUDE_CONFIG_DIR` or `~\.claude`) and every enabled Claude
//!   account's profile folder.
//! * Codex: `<profile>\sessions\**\rollout-*.jsonl` and `<profile>\archived_sessions\…`
//!   `token_count` events (cumulative totals → deltas), for `CODEX_HOME` or `~\.codex` and every
//!   enabled Codex account's profile folder.
//!
//! Output: [`compute`] → `codenotch_core::AnalyticsSummary`; [`compute_report`] →
//! [`AnalyticsReport`] (the richer view: unpriced tokens, EUR, per project / day / hour, plan
//! comparison) over `settings.analyticsDays` local days, plus the statistics fact tables
//! (`AnalyticsSummary::detail`, see `codenotch_core::analytics`) over
//! `fact_days(analyticsDays)` days (the period, the previous period and the 53-week heatmap).
//!
//! ## Incremental index
//! `Paths::analytics_cache()` (`analytics-cache.json`, format in [`cache`]) keeps, per
//! transcript, the bytes already parsed and hourly token buckets. A refresh stats every file,
//! skips unchanged ones, resumes grown ones at the stored offset and restarts a file that shrank
//! or whose first bytes changed. Files not modified within `fact_days` days are neither read
//! nor kept. An unreadable cache is treated as empty and overwritten. Only usage numbers, model ids,
//! working directories and timestamps are read — never message content.
//!
//! ## Prices
//! Built-in USD list prices per model (see [`pricing`]) plus optional overrides in
//! `<data dir>\pricing.json`. A model without a price is never priced at 0: its tokens are
//! reported in `unpriced_tokens` and left out of the cost.
//!
//! Threading: [`compute`] is blocking and may take seconds on a first run (well under a second
//! afterwards); the engine calls it on a worker thread at start-up, on
//! `Command::RefreshAnalytics`, and at most every 10 minutes. Concurrent calls are serialised.

pub mod cache;
pub mod index;
pub mod parse;
pub mod pricing;
pub mod report;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use codenotch_core::model::{AnalyticsSummary, TokenCounts};
use codenotch_core::{Config, Env, Paths, Result};

pub use pricing::{normalize_model, PriceTable, Rates};
pub use report::{
    compare_plan, plan_price_eur, AnalyticsReport, CliUsage, DayReport, HourReport, ModelReport,
    PlanComparison, ProjectReport, ReportStats, Usage,
};

/// USD price per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    /// 5-minute cache write (1-hour writes: see [`Rates::cache_write_1h`]).
    pub cache_write: f64,
}

/// Built-in price for a model id (normalised exact match: date suffixes and provider prefixes
/// are ignored, e.g. `claude-sonnet-4-20250514` → `claude-sonnet-4`). `None` = unknown (cost then
/// stays unknown for those tokens; never priced at 0). Does not read `pricing.json`; use
/// [`PriceTable::load`] for the effective table.
pub fn price_for(model: &str) -> Option<ModelPrice> {
    pricing::builtin_rates(model).map(|r| r.base)
}

/// Cost in USD of `tokens` on `model` (built-in prices; cache writes at the 5-minute rate);
/// `None` when the model has no price.
pub fn cost_usd(model: &str, tokens: &TokenCounts) -> Option<f64> {
    let p = price_for(model)?;
    Some(
        (tokens.input as f64 * p.input
            + tokens.output as f64 * p.output
            + tokens.cache_read as f64 * p.cache_read
            + tokens.cache_write as f64 * p.cache_write)
            / 1e6,
    )
}

/// `<data dir>\pricing.json`.
pub fn pricing_path(paths: &Paths) -> std::path::PathBuf {
    paths.root().join("pricing.json")
}

static RUN_LOCK: Mutex<()> = Mutex::new(());

/// Scan transcripts of the configured accounts and summarise the last `config.settings.analytics_days` days.
pub fn compute(config: &Config, env: &Env, paths: &Paths, now: i64) -> Result<AnalyticsSummary> {
    compute_report(config, env, paths, now, &HashMap::new()).map(|r| r.summary)
}

/// Like [`compute`], with the full report. `plan_names` maps account id → plan name of its
/// latest reading (e.g. `"Max 20x"`), used to look up `planPricesEur` (see [`plan_price_eur`]);
/// pass an empty map when unknown.
pub fn compute_report(
    config: &Config,
    env: &Env,
    paths: &Paths,
    now: i64,
    plan_names: &HashMap<String, String>,
) -> Result<AnalyticsReport> {
    let _guard = RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let started = Instant::now();
    let days = report::period_days(&config.settings);
    // Files are kept for the period, the previous period and the 53-week heatmap
    // (`fact_days`), plus one day of margin for time zones; older files cannot hold such data.
    let horizon = now - (i64::from(codenotch_core::analytics::fact_days(days)) + 1) * 86_400;

    let cache_path = paths.analytics_cache();
    let mut cache = cache::Cache::load(&cache_path);
    let roots = index::roots(config, env);
    let stats = index::update(&mut cache, &roots, horizon);
    let prices = PriceTable::load(&pricing_path(paths));

    let cache_saved = match cache.save(&cache_path) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("analytics: cache not saved ({})", e.kind());
            false
        }
    };

    let mut report = report::build(&cache, &prices, config, plan_names, now, &chrono::Local);
    report.stats = ReportStats {
        files: stats.files,
        files_read: stats.files_read,
        files_restarted: stats.files_restarted,
        bytes_read: stats.bytes_read,
        malformed_lines: stats.malformed,
        read_errors: stats.errors,
        elapsed_ms: started.elapsed().as_millis() as u64,
        cache_saved,
    };
    log::info!(
        "analytics: {} files ({} read, {} KiB), {} models, {} ms",
        stats.files,
        stats.files_read,
        stats.bytes_read / 1024,
        report.per_model.len(),
        report.stats.elapsed_ms
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn unknown_model_has_no_cost() {
        assert_eq!(super::cost_usd("mystery", &Default::default()), None);
        assert!(super::price_for("claude-opus-5-5").is_some());
    }

    fn write(path: &Path, text: &str) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).expect("mkdir");
        }
        std::fs::write(path, text).expect("write");
    }

    fn append(path: &Path, text: &str) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open");
        f.write_all(text.as_bytes()).expect("append");
    }

    fn claude(id: &str, ts: &str, model: &str, out: u64) -> String {
        format!(
            "{{\"type\":\"assistant\",\"cwd\":\"C:\\\\work\\\\app\",\"timestamp\":\"{ts}\",\"message\":{{\"id\":\"{id}\",\"model\":\"{model}\",\"content\":[{{\"type\":\"text\",\"text\":\"private\"}}],\"usage\":{{\"input_tokens\":100,\"output_tokens\":{out},\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}}}}}}\n"
        )
    }

    #[test]
    fn end_to_end_incremental_with_temp_dirs() {
        let dir = tempfile::tempdir().expect("tmp");
        let env = Env::for_test(dir.path());
        let paths = Paths::at(dir.path().join("data"));
        // File mtimes are real, so timestamps are relative to the real clock.
        let now = codenotch_core::now_secs();
        let t = |hours_ago: i64| {
            chrono::DateTime::from_timestamp(now - hours_ago * 3600, 0)
                .map(|d| d.to_rfc3339())
                .unwrap_or_default()
        };

        let session = env
            .home_path(".claude")
            .join("projects")
            .join("C--work-app")
            .join("s1.jsonl");
        write(
            &session,
            &format!(
                "{}{}",
                claude("m1", &t(3), "claude-opus-5-5", 1000),
                claude("m1", &t(3), "claude-opus-5-5", 1000)
            ),
        );
        let sub = session
            .with_extension("")
            .join("subagents")
            .join("agent-1.jsonl");
        write(&sub, &claude("m2", &t(2), "claude-sonnet-5-5", 500));
        let rollout = env
            .home_path(".codex")
            .join("sessions")
            .join("2026")
            .join("11")
            .join("12")
            .join("rollout-x.jsonl");
        let tc = |ts: &str, input: u64, cached: u64, output: u64| {
            format!("{{\"timestamp\":\"{ts}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":{input},\"cached_input_tokens\":{cached},\"output_tokens\":{output}}}}}}}}}\n")
        };
        let head = "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"C:\\\\work\\\\cli\"}}\n{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-6-luna\"}}\n";
        write(&rollout, &format!("{head}{}", tc(&t(3), 300, 200, 30)));

        let r =
            compute_report(&Config::default(), &env, &paths, now, &HashMap::new()).expect("report");
        assert_eq!((r.stats.files, r.stats.files_read), (3, 3));
        assert!(r.stats.cache_saved);
        assert_eq!(r.total.tokens.output, 1530);
        assert_eq!(r.total.unpriced_tokens, 330);
        assert_eq!(r.unpriced_models, ["gpt-6-luna"]);
        let expected = (100.0 * 4.0 + 1000.0 * 20.0 + 100.0 * 2.0 + 500.0 * 10.0) / 1e6;
        assert!((r.total.cost_usd - expected).abs() < 1e-12);
        assert_eq!(r.summary.total_tokens, r.total.tokens);
        assert_eq!(
            r.per_cli
                .iter()
                .map(|c| (c.provider.id().to_string(), c.sessions))
                .collect::<Vec<_>>(),
            [("claude".to_string(), 2), ("codex".to_string(), 1)]
        );

        // Second run: nothing new is read.
        let r2 =
            compute_report(&Config::default(), &env, &paths, now, &HashMap::new()).expect("report");
        assert_eq!((r2.stats.files_read, r2.stats.bytes_read), (0, 0));
        assert_eq!(r2.total, r.total);

        // Appended data: only the new bytes are read.
        let extra = claude("m3", &t(1), "claude-opus-5-5", 10);
        append(&session, &extra);
        append(&rollout, &tc(&t(1), 400, 250, 40));
        let r3 =
            compute_report(&Config::default(), &env, &paths, now, &HashMap::new()).expect("report");
        assert_eq!(r3.stats.files_read, 2);
        assert!(r3.stats.bytes_read < 1000);
        assert_eq!(r3.total.tokens.output, 1550);
        assert_eq!(r3.per_cli[1].usage.tokens.input, 100 + 50);

        // The cache file is the documented format.
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(paths.analytics_cache()).expect("cache"))
                .expect("json");
        assert_eq!(v["version"], 1);
        assert_eq!(v["files"].as_object().map(|f| f.len()), Some(3));

        // pricing.json prices the Codex model.
        write(
            &pricing_path(&paths),
            r#"{"models":{"gpt-6-luna":{"input":1,"output":8,"cacheRead":0.1,"cacheWrite":1}}}"#,
        );
        let r4 = compute(&Config::default(), &env, &paths, now).expect("summary");
        assert!(r4.total_cost_usd > r3.total.cost_usd);
        assert_eq!(r4.days, 7);
    }

    #[test]
    fn corrupt_cache_is_rebuilt() {
        let dir = tempfile::tempdir().expect("tmp");
        let env = Env::for_test(dir.path());
        let paths = Paths::at(dir.path().join("data"));
        write(&paths.analytics_cache(), "{ not json");
        let now = parse::parse_timestamp("2026-11-12T12:00:00Z").expect("ts");
        let s = compute(&Config::default(), &env, &paths, now).expect("summary");
        assert_eq!(s.total_tokens.total(), 0);
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(paths.analytics_cache()).expect("cache"))
                .expect("json");
        assert_eq!(v["version"], 1);
    }
}
