//! Manual sanity run against the real `~\.claude` / `~\.codex` transcripts:
//! `cargo test -p codenotch-analytics --test real_sanity -- --ignored --nocapture`
//!
//! Uses a TEMP data dir (the real data folder is only read for the account list) and prints
//! aggregate numbers only — no paths, no content, no project names.

use std::collections::HashMap;

use codenotch_core::{Config, Env, Paths};

#[test]
#[ignore = "reads the real transcripts of this machine"]
fn real_transcripts_aggregate_only() {
    let env = Env::system();
    let real = Paths::resolve();
    let (config, _) = Config::load_or_default(&real.config());
    let tmp = tempfile::tempdir().expect("tmp");
    let paths = Paths::at(tmp.path());
    let now = codenotch_core::now_secs();

    let first = codenotch_analytics::compute_report(&config, &env, &paths, now, &HashMap::new())
        .expect("first");
    let second = codenotch_analytics::compute_report(&config, &env, &paths, now, &HashMap::new())
        .expect("second");

    println!("days: {}", first.days);
    println!(
        "first run:  {} files, {} read, {:.1} MiB, {} malformed lines, {} read errors, {} ms",
        first.stats.files,
        first.stats.files_read,
        first.stats.bytes_read as f64 / 1_048_576.0,
        first.stats.malformed_lines,
        first.stats.read_errors,
        first.stats.elapsed_ms
    );
    println!(
        "second run: {} files, {} read, {} bytes, {} ms",
        second.stats.files,
        second.stats.files_read,
        second.stats.bytes_read,
        second.stats.elapsed_ms
    );
    for c in &first.per_cli {
        let t = c.usage.tokens;
        println!(
            "{:>7}: sessions {:>4}  input {:>12}  output {:>11}  cache_write {:>12} (1h {:>12})  cache_read {:>14}  priced ${:>9.2}  unpriced tokens {}",
            c.provider.id(),
            c.sessions,
            t.input,
            t.output,
            t.cache_write,
            c.usage.cache_write_1h,
            t.cache_read,
            c.usage.cost_usd,
            c.usage.unpriced_tokens
        );
    }
    let priced = first.per_model.iter().filter(|m| m.priced).count();
    println!(
        "models: {} priced, {} unpriced {:?}",
        priced,
        first.per_model.len() - priced,
        first.unpriced_models
    );
    println!(
        "total: ${:.2} / EUR {:.2} (priced part), unpriced tokens {}, projects {}",
        first.total.cost_usd,
        first.total_cost_eur(),
        first.total.unpriced_tokens,
        first.per_project.len()
    );
    assert_eq!(second.total, first.total);
    assert_eq!(second.stats.bytes_read, 0);
}
