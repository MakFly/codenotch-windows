//! # codenotch-providers-extra (owner: agent B)
//!
//! Cursor, GitHub Copilot, OpenCode (Go), Command Code, GLM (z.ai), Antigravity, Qoder.
//! Spec: data-spec.md §5.1–§5.7, §6, §7. Contract: `codenotch_core::provider` (env/http
//! injection, 0–100 used, `None` for unknown, no secrets).
//!
//! Headline window ids (core `ProviderKind::headline_candidates`): cursor `included`→`api`,
//! copilot `premium_interactions`→first, glm `session` (+ weekly `weekly`), opencode `rolling`
//! (+ `weekly`), commandcode `monthly` (+ `weekly`), qoder `quota`. Antigravity sets
//! `Reading::headline_window` / `weekly_window` itself with [`antigravity_headline`].
//!
//! Every provider borrows a sign-in another tool already holds and only ever **reads** it (SQLite
//! opened read-only, files never written, tokens never logged or put in a `Reading`). Child
//! processes (`gh`, `agy`, `node`, `powershell`, `netstat`) run hidden, bounded, in a kill-on-close
//! job, and not at all when `CODENOTCH_NO_SPAWN=1`.
//!
//! Status mapping (data-spec → `ReadingStatus`): needsAuth → `SignedOut` (note = what to do),
//! none → `Empty`, absent → `NotInstalled`, a failed read with an earlier reading → that reading
//! as `Stale` (original `fetched_at`, `error` set), else `Error`; HTTP 429 → `RateLimited{until}`
//! carrying the previous windows.

mod antigravity;
mod commandcode;
mod copilot;
mod cursor;
mod glm;
mod i18n;
mod opencode;
mod process;
mod qoder;
mod util;

use codenotch_core::config::{AntigravityLimit, AntigravityModel};
use codenotch_core::model::{LimitWindow, ProviderKind, Reading};
use codenotch_core::provider::{Env, FetchContext, LocalIdentity, Provider};
use codenotch_core::Account;
use std::time::Duration;

/// French strings of this crate's notes, errors and window labels (keys are the English texts).
/// Fixed messages stay English in a `Reading` and are translated at display time, so the app
/// must call `codenotch_core::i18n::register_fr(codenotch_providers_extra::FR)` once at
/// start-up. Messages with a variable part are already localised at fetch time
/// (`FetchContext::settings.lang()`).
pub use i18n::FR;
pub use qoder::QODER_SIDECAR_SOURCE;

/// Every provider of this crate polls at most every 5 minutes (data-spec §5).
const POLL: Duration = Duration::from_secs(300);

/// All providers of this crate, one per kind.
pub fn all() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(CursorProvider),
        Box::new(CopilotProvider),
        Box::new(OpenCodeProvider),
        Box::new(CommandCodeProvider),
        Box::new(GlmProvider),
        Box::new(AntigravityProvider),
        Box::new(QoderProvider),
    ]
}

fn one(found: bool, kind: ProviderKind) -> Vec<Account> {
    if found {
        let name = kind.display_name().to_string();
        vec![Account::new(kind, name)]
    } else {
        Vec::new()
    }
}

/// Cursor (data-spec §5.1): session token from Cursor's `state.vscdb`.
pub struct CursorProvider;

impl Provider for CursorProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Cursor
    }
    fn discover(&self, env: &Env) -> Vec<Account> {
        one(cursor::discover(env), self.kind())
    }
    fn local_identity(&self, account: &Account, env: &Env) -> LocalIdentity {
        cursor::local_identity(env, account)
    }
    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
        cursor::fetch(account, ctx)
    }
    fn min_interval(&self) -> Duration {
        POLL
    }
}

/// GitHub Copilot (data-spec §5.2): `gh` / Copilot token, premium requests.
pub struct CopilotProvider;

impl Provider for CopilotProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Copilot
    }
    fn discover(&self, env: &Env) -> Vec<Account> {
        one(copilot::discover(env), self.kind())
    }
    fn local_identity(&self, _account: &Account, env: &Env) -> LocalIdentity {
        copilot::local_identity(env)
    }
    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
        copilot::fetch(account, ctx)
    }
    fn min_interval(&self) -> Duration {
        POLL
    }
}

/// OpenCode / OpenCode Go (data-spec §5.3).
pub struct OpenCodeProvider;

impl Provider for OpenCodeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenCode
    }
    fn discover(&self, env: &Env) -> Vec<Account> {
        one(opencode::installed(env, None), self.kind())
    }
    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
        opencode::fetch(account, ctx)
    }
    fn min_interval(&self) -> Duration {
        POLL
    }
}

/// Command Code (data-spec §5.4).
pub struct CommandCodeProvider;

impl Provider for CommandCodeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::CommandCode
    }
    fn discover(&self, env: &Env) -> Vec<Account> {
        one(commandcode::discover(env), self.kind())
    }
    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
        commandcode::fetch(account, ctx)
    }
    fn min_interval(&self) -> Duration {
        POLL
    }
}

/// GLM / z.ai coding plan (data-spec §5.5).
pub struct GlmProvider;

impl Provider for GlmProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Glm
    }
    fn discover(&self, env: &Env) -> Vec<Account> {
        one(glm::present(env), self.kind())
    }
    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
        glm::fetch(account, ctx)
    }
    fn min_interval(&self) -> Duration {
        POLL
    }
}

/// Antigravity (data-spec §5.6); provider id `gemini` is accepted as an alias.
pub struct AntigravityProvider;

impl Provider for AntigravityProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Antigravity
    }
    fn discover(&self, env: &Env) -> Vec<Account> {
        one(antigravity::installed(env), self.kind())
    }
    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
        antigravity::fetch(account, ctx)
    }
    fn min_interval(&self) -> Duration {
        POLL
    }
}

/// Qoder (data-spec §5.7).
pub struct QoderProvider;

impl Provider for QoderProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Qoder
    }
    fn discover(&self, env: &Env) -> Vec<Account> {
        one(qoder::installed(env, None), self.kind())
    }
    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
        qoder::fetch(account, ctx)
    }
    fn min_interval(&self) -> Duration {
        POLL
    }
}

/// Antigravity lane picker (data-spec §7): returns (headline id, weekly id) for the chosen
/// model family and limit setting. Pure function; the provider stores the result in
/// `Reading::headline_window` / `weekly_window`.
///
/// Lanes = the family's windows (`gemini…` → Gemini, `3p…`/`claude…` → third-party; all windows
/// when the family has none). A 5h/weekly setting takes the tightest matching lane; otherwise (or
/// when nothing matches) the tightest lane under 100 %, else the tightest, else the first.
/// Weekly = the tightest family lane whose id/label says "weekly", never the headline.
/// "Tightest" = highest used percent, ties to the lower id, count-only windows excluded.
pub fn antigravity_headline(
    windows: &[LimitWindow],
    limit: AntigravityLimit,
    model: AntigravityModel,
) -> (Option<String>, Option<String>) {
    antigravity::headline(windows, limit, model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::ReadingStatus;
    use codenotch_core::FakeHttp;
    use std::sync::Arc;

    #[test]
    fn seven_providers() {
        let all = super::all();
        assert_eq!(all.len(), 7);
        let kinds: Vec<ProviderKind> = all.iter().map(|p| p.kind()).collect();
        for k in [
            ProviderKind::Cursor,
            ProviderKind::Copilot,
            ProviderKind::OpenCode,
            ProviderKind::CommandCode,
            ProviderKind::Glm,
            ProviderKind::Antigravity,
            ProviderKind::Qoder,
        ] {
            assert!(kinds.contains(&k));
        }
        assert!(all
            .iter()
            .all(|p| p.min_interval() == Duration::from_secs(300)));
    }

    /// An empty machine: nothing discovered, every fetch says "not installed", no request sent.
    #[test]
    fn empty_machine_discovers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path()).with_var("CODENOTCH_NO_SPAWN", "1");
        let http = Arc::new(FakeHttp::new());
        for p in super::all() {
            assert!(p.discover(&env).is_empty(), "{}", p.kind());
            let account = Account::new(p.kind(), "x");
            let r = p.fetch(&account, &FetchContext::new(0, http.clone(), env.clone()));
            assert_eq!(r.status, ReadingStatus::NotInstalled, "{}", p.kind());
        }
        assert!(http.requests().is_empty());
    }
}
