//! One live, read-only sanity pass over this machine:
//! `cargo run -p codenotch-providers-extra --example live_check`
//!
//! Runs `discover()` and one `fetch()` per discovered account with `CODENOTCH_NO_SPAWN=1` (no
//! child process is started). Prints only the provider, the status, window ids and percentages:
//! never notes, identities, tokens or raw responses.

use codenotch_core::provider::{Env, FetchContext};
use codenotch_core::{now_secs, UreqHttp};
use std::sync::Arc;

fn main() {
    let env = Env::system().with_var("CODENOTCH_NO_SPAWN", "1");
    let http = Arc::new(UreqHttp::new());
    for provider in codenotch_providers_extra::all() {
        let accounts = provider.discover(&env);
        if accounts.is_empty() {
            println!("{:<12} not installed here", provider.kind().id());
            continue;
        }
        for account in accounts {
            let ctx = FetchContext::new(now_secs(), http.clone(), env.clone());
            let r = provider.fetch(&account, &ctx);
            let status = match &r.status {
                codenotch_core::ReadingStatus::RateLimited { .. } => "RateLimited".to_string(),
                s => format!("{s:?}"),
            };
            let windows: Vec<String> = r
                .windows
                .iter()
                .map(|w| match (w.used_percent, w.count) {
                    (Some(p), _) => format!("{}={p:.1}%", w.id),
                    (None, Some(n)) => format!("{}=~{n}", w.id),
                    (None, None) => format!("{}=—", w.id),
                })
                .collect();
            println!(
                "{:<12} {:<12} headline={:<8} [{}]",
                provider.kind().id(),
                status,
                r.headline_percent()
                    .map_or("—".to_string(), |p| format!("{p:.1}%")),
                windows.join(", ")
            );
            // Not-fresh readings carry this crate's own fixed status text (no identity, no token).
            if !matches!(r.status, codenotch_core::ReadingStatus::Fresh) {
                println!(
                    "{:<12} why: {}",
                    "",
                    r.error.as_deref().or(r.note.as_deref()).unwrap_or("—")
                );
            }
        }
    }
}
