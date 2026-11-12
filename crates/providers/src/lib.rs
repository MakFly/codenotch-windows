//! # codenotch-providers
//!
//! Claude, Codex, Grok and the custom JSON endpoint, implementing the
//! `codenotch_core::provider` trait.
//!
//! Rules:
//! * Files only via `ctx.env` / `env` paths; HTTP only via `ctx.http` (tests use `FakeHttp`).
//! * Window ids and English `label_key`s:
//!   - Claude: `session` "Current session", `seven_day` "Weekly limit", `seven_day_opus`
//!     "Weekly limit (Opus)", `seven_day_sonnet` "Weekly limit (Sonnet)", `weekly_scoped`
//!     "Weekly limit (<model>)", `spend` "Spend limit", `extra_usage` "Extra usage".
//!   - Codex: `primary` "Current session", `secondary` "Weekly limit" (labels follow the window
//!     length), `spark`/`spark-secondary` (group "Spark"), `code-review`/`code-review-secondary`
//!     (group "Code review").
//!   - Grok: `credits` "Credits" (product name in `detail`; other products by wire name).
//!   - Custom: as declared by the endpoint (see [`parse_custom_usage`]).
//! * Percentages 0–100 used; unknown = `None`; never put a token in any `Reading` field or log.
//! * 429 → `ReadingStatus::RateLimited { until }` (Retry-After via `HttpResponse::retry_after_secs`).
//! * Not signed in → `SignedOut`; folder missing → `NotInstalled`.
//! * A failed read keeps the previous reading's windows (with their original `fetched_at`) and
//!   reports what happened in `status` / `note` / `error`.
//! * Child processes (renewal, app-server, sign-in) are never started when
//!   `CODENOTCH_NO_SPAWN=1` ([`NO_SPAWN_ENV`]).

mod claude;
mod codex;
mod custom;
mod grok;
mod i18n;
pub mod safe_storage;
mod util;

#[cfg(test)]
mod tests;

use codenotch_core::model::ProviderKind;
use codenotch_core::provider::{Env, Provider};
use std::path::PathBuf;

pub use claude::{
    add_claude_account, claude_auth_state, claude_backoff_secs, claude_names, claude_profiles,
    claude_sign_in, find_claude_cli, parse_claude_usage, ClaudeAuthState, ClaudeProfile,
    ClaudeProvider, ADD_ACCOUNT_TIMEOUT, CLAUDE_USAGE_URL, SIGN_IN_TIMEOUT,
};
pub use codex::{
    codex_rollout_windows, find_codex, parse_codex_usage, CodexProvider, APP_SERVER_REST_SECS,
    APP_SERVER_TIMEOUT, CODEX_USAGE_URL, ROLLOUT_FRESH_SECS,
};
pub use custom::{parse_custom_usage, CustomProvider};
pub use grok::{find_grok_cli, parse_grok_usage, GrokProvider, GROK_BILLING_URL, TRUSTED_ISSUER};
/// French strings of this crate's notes, errors, sign-in messages and window labels (keys are
/// the English texts). Fixed messages stay English in a `Reading` and are translated at display
/// time, so the app must call `codenotch_core::i18n::register_fr(codenotch_providers::FR)` once
/// at start-up. Messages with a variable part are already localised at fetch time
/// (`FetchContext::settings.lang()`).
pub use i18n::FR;
pub use util::{
    should_renew, NO_SPAWN_ENV, RENEW_COOLDOWN_MS, RENEW_MARGIN_MS, RENEW_RETRY_CAP_MS,
    RENEW_TIMEOUT,
};

/// All providers of this crate, one per kind.
pub fn all() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(ClaudeProvider),
        Box::new(CodexProvider),
        Box::new(GrokProvider),
        Box::new(CustomProvider),
    ]
}

/// Default profile folder for a kind (`~\.claude`, `~\.codex`, `~\.grok`).
pub fn default_profile_dir(env: &Env, kind: &ProviderKind) -> Option<PathBuf> {
    match kind {
        ProviderKind::Claude => Some(env.home_path(".claude")),
        ProviderKind::Codex => Some(env.home_path(".codex")),
        ProviderKind::Grok => Some(env.home_path(".grok")),
        _ => None,
    }
}
