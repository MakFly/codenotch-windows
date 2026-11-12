//! Claude (data-spec §2): Claude Code OAuth credentials, `GET /api/oauth/usage`, renewal by the
//! standalone CLI, interactive sign-in and extra profiles (`~\.claude-<n>`).
//!
//! Two read-only credential stores, the freshest non-expired token wins:
//! * the CLI's `<profile>\.credentials.json` (`claudeAiOauth`);
//! * the Claude desktop app's token cache (its Code tab runs Claude Code with the app's own
//!   sign-in, which it refreshes itself; the CLI file then stays blank or expired):
//!   `config.json` → `oauth:tokenCacheV2` (then the legacy `oauth:tokenCache`), encrypted with
//!   Electron safeStorage (see [`crate::safe_storage`]), in `%LOCALAPPDATA%\Claude-Data\<pkg>`,
//!   `%LOCALAPPDATA%\Packages\Claude_*\LocalCache\Roaming\Claude` or `%APPDATA%\Claude`.
//!   Only entries of the profile's own account (`.claude.json` → `oauthAccount.accountUuid`)
//!   holding the `user:profile` scope are used. Nothing is ever written back or refreshed.

use crate::i18n::tf;
use crate::safe_storage::SafeStorage;
use crate::util::{
    self, capitalise, carry_or, first_file, humanise, num, path_dirs, rfc3339_secs,
    scrub_claude_env, set_flags, short, text, RenewState, CREATE_NEW_CONSOLE, CREATE_NO_WINDOW,
};
use codenotch_core::http::{HttpError, HttpRequest, HttpResponse};
use codenotch_core::model::{LimitWindow, ProviderKind, Reading, ReadingStatus, WindowKind};
use codenotch_core::provider::{Env, FetchContext, LocalIdentity, Provider};
use codenotch_core::{Account, Error, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

pub const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const BETA_HEADER: &str = "oauth-2025-04-20";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CRED_NAMES: [&str; 2] = [".credentials.json", "credentials.json"];
/// Scope `GET /api/oauth/usage` needs.
const PROFILE_SCOPE: &str = "user:profile";
const CLAUDE_CODE_SCOPE: &str = "user:sessions:claude_code";
const FIVE_HOURS: i64 = 5 * 3600;
const SEVEN_DAYS: i64 = 7 * 86400;
/// Sign-in console lifetime.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Sign-in lifetime for a profile just created by [`add_claude_account`].
pub const ADD_ACCOUNT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub(crate) const NOTE_NO_CREDENTIAL: &str = "No Claude Code credential found";
pub(crate) const NOTE_EXPIRED: &str =
    "Credential expired — run claude once in a terminal to renew it";
pub(crate) const NOTE_REJECTED: &str = "Credential rejected (switched accounts?)";
pub(crate) const NOTE_403: &str =
    "Claude HTTP 403: access denied. Check network or account access; sign-in may still be valid.";
pub(crate) const NOTE_NO_WINDOWS: &str = "Claude reported no usage windows";
/// Template translated at fetch time.
pub(crate) const TPL_RATE_LIMITED: &str = "Rate limited, retrying in {wait}s";
pub(crate) const LABEL_SCOPED: &str = "Weekly limit (model-scoped)";
pub(crate) const LABEL_EXTRA_USAGE: &str = "Extra usage";
// Sign-in messages (English; the UI translates them with the registered table).
pub(crate) const MSG_SIGN_IN_PENDING: &str = "Complete sign-in in the browser or terminal window.";
pub(crate) const MSG_SIGN_IN_DONE: &str = "Sign-in complete. Refreshing usage...";
pub(crate) const MSG_SIGN_IN_FAILED: &str = "Sign-in cancelled, failed or timed out. Try again.";
pub(crate) const MSG_CLI_NOT_FOUND: &str =
    "Claude Code CLI not found. Install the standalone CLI first.";
pub(crate) const MSG_AUTH_BUSY: &str = "Claude sign-in or renewal is already running.";
pub(crate) const MSG_SIGN_IN_WINDOW: &str = "Unable to open Claude sign-in window.";
pub(crate) const MSG_NO_SPAWN: &str = "process launch is disabled (CODENOTCH_NO_SPAWN=1)";

// ---------------------------------------------------------------------------------------------
// Credential
// ---------------------------------------------------------------------------------------------

/// Claude Code OAuth credential. `Debug` never prints the token.
#[derive(Clone, PartialEq)]
pub(crate) struct Credential {
    token: String,
    /// Epoch ms; `None` = the file names no expiry (never treated as expired).
    pub expires_at_ms: Option<i64>,
    pub subscription: Option<String>,
    pub tier: Option<String>,
    /// Where it came from (for logs): [`Source::Cli`] or a desktop-app cache.
    pub source: Source,
}

/// Credential store, best first when expiries tie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Source {
    /// Desktop app, `oauth:tokenCacheV2`, an entry made for Claude Code sessions.
    DesktopCode,
    /// Desktop app, `oauth:tokenCacheV2`, another entry.
    Desktop,
    /// The CLI's `.credentials.json`.
    Cli,
    /// Desktop app, legacy `oauth:tokenCache` (used only when nothing else is valid).
    DesktopLegacy,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("token", &"<hidden>")
            .field("expires_at_ms", &self.expires_at_ms)
            .field("subscription", &self.subscription)
            .field("source", &self.source)
            .finish()
    }
}

impl Credential {
    pub fn expired(&self, now_secs: i64) -> bool {
        self.expires_at_ms
            .is_some_and(|e| e <= now_secs.saturating_mul(1000))
    }

    /// Plan label: `max` → "Max" (+ " 20x"/" 5x" from the rate-limit tier), `pro` → "Pro" …
    pub fn plan(&self) -> Option<String> {
        plan_label(self.subscription.as_deref()?, self.tier.as_deref())
    }
}

/// `max` + `default_claude_max_20x` → "Max 20x", `pro` → "Pro", `team` → "Team" …
pub(crate) fn plan_label(subscription: &str, tier: Option<&str>) -> Option<String> {
    let sub = subscription.trim();
    if sub.is_empty() {
        return None;
    }
    let mut plan = capitalise(&sub.replace('_', " "));
    if sub.eq_ignore_ascii_case("max") {
        let tier = tier.unwrap_or("").to_ascii_lowercase();
        for mult in ["20x", "5x"] {
            if tier.contains(mult) {
                plan.push(' ');
                plan.push_str(mult);
                break;
            }
        }
    }
    Some(plan)
}

/// Parse one credential file's text. An empty token counts as signed out (`None`).
pub(crate) fn parse_credential(body: &str) -> Option<Credential> {
    let v: Value = serde_json::from_str(body).ok()?;
    let oauth = v
        .get("claudeAiOauth")
        .filter(|o| o.is_object())
        .unwrap_or(&v);
    let token = oauth.get("accessToken").and_then(Value::as_str)?.trim();
    if token.is_empty() {
        return None;
    }
    Some(Credential {
        token: token.to_string(),
        expires_at_ms: num(oauth.get("expiresAt")).map(|ms| ms as i64),
        subscription: text(oauth.get("subscriptionType")),
        tier: text(oauth.get("rateLimitTier")),
        source: Source::Cli,
    })
}

/// `<dir>\.credentials.json`, then `<dir>\credentials.json`. Read only, never written.
pub(crate) fn read_credential(dir: &Path) -> Option<Credential> {
    CRED_NAMES.iter().find_map(|name| {
        std::fs::read_to_string(dir.join(name))
            .ok()
            .and_then(|t| parse_credential(&t))
    })
}

fn has_credential_file(dir: &Path) -> bool {
    CRED_NAMES.iter().any(|n| dir.join(n).is_file())
}

// ---------------------------------------------------------------------------------------------
// Claude desktop app token cache (read-only)
// ---------------------------------------------------------------------------------------------

/// Folders that may hold the desktop app's `config.json` + `Local State`, newest layout first.
fn desktop_dirs(env: &Env) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let subdirs = |parent: &Path, prefix: &str| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(parent)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .starts_with(prefix)
            })
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        v.sort();
        v
    };
    // Current MSIX builds: %LOCALAPPDATA%\Claude-Data\claude_<publisher id>.
    out.extend(subdirs(&env.local_appdata.join("Claude-Data"), "claude"));
    // Older MSIX builds: the package's redirected %APPDATA%.
    for pkg in subdirs(&env.local_appdata.join("Packages"), "claude_") {
        out.push(pkg.join("LocalCache").join("Roaming").join("Claude"));
    }
    // Non-packaged installs.
    out.push(env.appdata.join("Claude"));
    out.retain(|d| d.join("config.json").is_file() && d.join("Local State").is_file());
    out
}

/// One usable entry of a desktop token cache.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DesktopEntry {
    /// Account UUID from the entry key (`acct:<account>|<org>:…`).
    pub account: String,
    pub cred: Credential,
}

/// Parse a decrypted token cache: `{"acct:<account>|<org>:<client>:<api url>:<scopes>":
/// {token, expiresAt, subscriptionType, rateLimitTier, …}}`. Entries without the
/// `user:profile` scope (the usage endpoint refuses them) or without a token are dropped.
pub(crate) fn parse_token_cache(json: &str, legacy: bool) -> Vec<DesktopEntry> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, entry) in &map {
        let Some(rest) = key.strip_prefix("acct:") else {
            continue;
        };
        let account: String = rest
            .chars()
            .take_while(|c| *c != '|' && *c != ':')
            .collect();
        // Scopes follow the API base URL (`https://host:` then space-separated scopes).
        let scopes = rest
            .find("://")
            .map(|i| &rest[i + 3..])
            .and_then(|after| after.split_once(':'))
            .map(|(_, scopes)| scopes)
            .unwrap_or("");
        let has = |s: &str| scopes.split_whitespace().any(|x| x == s);
        if account.is_empty() || !has(PROFILE_SCOPE) {
            continue;
        }
        let token = entry
            .get("token")
            .or_else(|| entry.get("accessToken"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if token.is_empty() {
            continue;
        }
        let source = match (legacy, has(CLAUDE_CODE_SCOPE)) {
            (true, _) => Source::DesktopLegacy,
            (false, true) => Source::DesktopCode,
            (false, false) => Source::Desktop,
        };
        out.push(DesktopEntry {
            account,
            cred: Credential {
                token: token.to_string(),
                expires_at_ms: num(entry.get("expiresAt")).map(|ms| ms as i64),
                subscription: text(entry.get("subscriptionType")),
                tier: text(entry.get("rateLimitTier")),
                source,
            },
        });
    }
    out
}

/// Every usable desktop-app credential of `account_uuid` (or, when the profile's account is
/// unknown and `fallback_last_known`, of the app's last signed-in account).
fn desktop_credentials(
    env: &Env,
    account_uuid: Option<&str>,
    fallback_last_known: bool,
) -> Vec<Credential> {
    let mut out = Vec::new();
    for dir in desktop_dirs(env) {
        let Some(config) = std::fs::read_to_string(dir.join("config.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        else {
            continue;
        };
        let wanted = match account_uuid {
            Some(a) => a.to_string(),
            None if fallback_last_known => match text(config.get("lastKnownAccountUuid")) {
                Some(a) => a,
                None => continue,
            },
            None => continue,
        };
        let Some(key) = SafeStorage::from_local_state(&dir.join("Local State")) else {
            log::debug!("claude: desktop app key not readable");
            continue;
        };
        for (field, legacy) in [("oauth:tokenCacheV2", false), ("oauth:tokenCache", true)] {
            let Some(plain) = config
                .get(field)
                .and_then(Value::as_str)
                .and_then(|b| key.decrypt_base64(b))
            else {
                continue;
            };
            out.extend(
                parse_token_cache(&plain, legacy)
                    .into_iter()
                    .filter(|e| e.account == wanted)
                    .map(|e| e.cred),
            );
        }
    }
    out
}

/// Non-expired credentials, best first: legacy desktop entries last, then the latest expiry,
/// then the store order of [`Source`]. Duplicate tokens are dropped.
pub(crate) fn rank_credentials(mut all: Vec<Credential>, now_secs: i64) -> Vec<Credential> {
    all.retain(|c| !c.expired(now_secs));
    all.sort_by(|a, b| {
        let legacy = |c: &Credential| c.source == Source::DesktopLegacy;
        legacy(a)
            .cmp(&legacy(b))
            .then(
                b.expires_at_ms
                    .unwrap_or(i64::MAX)
                    .cmp(&a.expires_at_ms.unwrap_or(i64::MAX)),
            )
            .then(a.source.cmp(&b.source))
    });
    let mut seen = HashSet::new();
    all.retain(|c| seen.insert(c.token.clone()));
    all
}

// ---------------------------------------------------------------------------------------------
// Identity (`.claude.json` → oauthAccount), cached by (mtime, len)
// ---------------------------------------------------------------------------------------------

/// What `.claude.json` says about the signed-in account (never a secret).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AccountMeta {
    pub email: Option<String>,
    /// From `organizationType` (`claude_max` → "Max") and `organizationRateLimitTier`.
    pub plan: Option<String>,
    /// `oauthAccount.accountUuid`: matches the desktop app's token cache entries.
    pub account_uuid: Option<String>,
}

type IdentityCache = HashMap<PathBuf, (Option<SystemTime>, u64, AccountMeta)>;

fn identity_cache() -> &'static Mutex<IdentityCache> {
    static CACHE: OnceLock<Mutex<IdentityCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `oauthAccount.emailAddress`, and the plan named by `oauthAccount.organizationType`
/// (`claude_max`, `claude_pro`, `claude_team`, `claude_enterprise` …) in the credential's form.
pub(crate) fn meta_from_claude_json(text: &str) -> AccountMeta {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return AccountMeta::default();
    };
    let org_type = text_at(&v, &["oauthAccount", "organizationType"]);
    let tier = text_at(&v, &["oauthAccount", "organizationRateLimitTier"])
        .or_else(|| text_at(&v, &["oauthAccount", "userRateLimitTier"]));
    let plan = org_type
        .as_deref()
        .and_then(|t| t.strip_prefix("claude_"))
        .and_then(|sub| plan_label(sub, tier.as_deref()));
    AccountMeta {
        email: text_at(&v, &["oauthAccount", "emailAddress"]),
        plan,
        account_uuid: text_at(&v, &["oauthAccount", "accountUuid"]),
    }
}

fn text_at(v: &Value, path: &[&str]) -> Option<String> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    text(Some(cur))
}

fn read_meta(file: &Path) -> Option<AccountMeta> {
    let meta = std::fs::metadata(file).ok()?;
    let key = (meta.modified().ok(), meta.len());
    if let Ok(cache) = identity_cache().lock() {
        if let Some((m, l, found)) = cache.get(file) {
            if (*m, *l) == key {
                return Some(found.clone());
            }
        }
    }
    let found = std::fs::read_to_string(file)
        .map(|t| meta_from_claude_json(&t))
        .unwrap_or_default();
    if let Ok(mut cache) = identity_cache().lock() {
        cache.insert(file.to_path_buf(), (key.0, key.1, found.clone()));
    }
    Some(found)
}

fn is_default_dir(env: &Env, dir: &Path) -> bool {
    util::same_path(dir, &env.home_path(".claude"))
}

/// Account metadata of a profile: the default profile's sits beside the folder
/// (`~\.claude.json`), other profiles keep it inside (`<dir>\.claude.json`). Each field comes
/// from the first file that has it.
pub(crate) fn profile_meta(env: &Env, dir: &Path) -> AccountMeta {
    let mut files = Vec::new();
    if is_default_dir(env, dir) {
        files.push(env.home_path(".claude.json"));
    }
    files.push(dir.join(".claude.json"));
    let metas: Vec<AccountMeta> = files.iter().filter_map(|f| read_meta(f)).collect();
    AccountMeta {
        email: metas.iter().find_map(|m| m.email.clone()),
        plan: metas.iter().find_map(|m| m.plan.clone()),
        account_uuid: metas.iter().find_map(|m| m.account_uuid.clone()),
    }
}

/// Signed-in e-mail of a profile (see [`profile_meta`]).
pub(crate) fn profile_email(env: &Env, dir: &Path) -> Option<String> {
    profile_meta(env, dir).email
}

/// Identity and plan of a profile from its files: the credential's subscription first, then
/// `.claude.json`.
pub(crate) fn local_identity(env: &Env, dir: &Path) -> LocalIdentity {
    let meta = profile_meta(env, dir);
    LocalIdentity {
        identity: meta.email,
        plan: read_credential(dir).and_then(|c| c.plan()).or(meta.plan),
    }
}

// ---------------------------------------------------------------------------------------------
// Discovery and naming
// ---------------------------------------------------------------------------------------------

/// A Claude profile folder found on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeProfile {
    /// `default` or the `<id>` of `~\.claude-<id>`.
    pub id: String,
    pub dir: PathBuf,
    pub email: Option<String>,
}

fn valid_profile_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && id != "default"
}

/// Profile id of a `CLAUDE_CONFIG_DIR` folder: its name without a leading `.` / `claude-`
/// (`D:\cfg\.claude-work` → `work`), characters outside `[A-Za-z0-9-]` dropped.
fn config_dir_id(dir: &Path) -> String {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = name.trim_start_matches('.');
    let name = name.strip_prefix("claude-").unwrap_or(name);
    let id: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(64)
        .collect();
    if valid_profile_id(&id) && id != "claude" {
        id
    } else {
        "config".into()
    }
}

/// `default` (`~\.claude`, when the folder exists) then every `~\.claude-<id>` folder holding a
/// credential, ids sorted alphabetically, then the `CLAUDE_CONFIG_DIR` folder when it is another
/// one holding a credential.
pub fn claude_profiles(env: &Env) -> Vec<ClaudeProfile> {
    let mut out = Vec::new();
    let default = env.home_path(".claude");
    if default.is_dir() {
        out.push(ClaudeProfile {
            id: "default".into(),
            email: profile_email(env, &default),
            dir: default,
        });
    }
    let mut extra = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&env.home) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_prefix(".claude-") else {
                continue;
            };
            let dir = entry.path();
            if valid_profile_id(id) && dir.is_dir() && has_credential_file(&dir) {
                extra.push(ClaudeProfile {
                    id: id.to_string(),
                    email: profile_email(env, &dir),
                    dir,
                });
            }
        }
    }
    extra.sort_by(|a, b| a.id.cmp(&b.id));
    out.extend(extra);
    // The folder `CLAUDE_CONFIG_DIR` points at, when it holds a credential and is not listed yet.
    if let Some(dir) = env.var("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
        let listed = out.iter().any(|p| util::same_path(&p.dir, &dir));
        if !listed && dir.is_dir() && has_credential_file(&dir) {
            out.push(ClaudeProfile {
                id: config_dir_id(&dir),
                email: profile_email(env, &dir),
                dir,
            });
        }
    }
    out
}

/// "Acme" from `me@acme.co.uk`; `None` when the first domain label has no letter.
fn domain_label(email: &str) -> Option<String> {
    let domain = email.rsplit_once('@')?.1;
    let first = domain.split('.').next()?.trim();
    first
        .chars()
        .any(char::is_alphabetic)
        .then(|| capitalise(&first.to_lowercase()))
}

/// Display names per data-spec §2.1: one profile → "Claude"; otherwise "Claude <Domain>",
/// "Claude (<id>)" without e-mail ("Claude" for default); colliding names → "Claude <e-mail>".
pub fn claude_names(profiles: &[ClaudeProfile]) -> Vec<String> {
    if profiles.len() == 1 {
        return vec!["Claude".into()];
    }
    let mut names: Vec<String> = profiles
        .iter()
        .map(|p| match p.email.as_deref().and_then(domain_label) {
            Some(label) => format!("Claude {label}"),
            None if p.id == "default" => "Claude".into(),
            None => format!("Claude ({})", p.id),
        })
        .collect();
    let snapshot = names.clone();
    for (i, name) in names.iter_mut().enumerate() {
        let collides = snapshot
            .iter()
            .enumerate()
            .any(|(j, n)| j != i && n == name);
        if collides {
            if let Some(email) = &profiles[i].email {
                *name = format!("Claude {email}");
            }
        }
    }
    names
}

// ---------------------------------------------------------------------------------------------
// Usage parsing
// ---------------------------------------------------------------------------------------------

/// (id, label, kind, window length) for a usage kind.
fn window_spec(kind: &str) -> (String, String, WindowKind, Option<i64>) {
    let (id, label, wk, secs): (&str, String, WindowKind, Option<i64>) = match kind {
        "session" | "five_hour" => (
            "session",
            "Current session".into(),
            WindowKind::Session,
            Some(FIVE_HOURS),
        ),
        "seven_day" | "weekly_all" | "weekly" => (
            "seven_day",
            "Weekly limit".into(),
            WindowKind::Weekly,
            Some(SEVEN_DAYS),
        ),
        "seven_day_opus" | "weekly_opus" => (
            "seven_day_opus",
            "Weekly limit (Opus)".into(),
            WindowKind::Weekly,
            Some(SEVEN_DAYS),
        ),
        "seven_day_sonnet" | "weekly_sonnet" => (
            "seven_day_sonnet",
            "Weekly limit (Sonnet)".into(),
            WindowKind::Weekly,
            Some(SEVEN_DAYS),
        ),
        "weekly_scoped" => (
            "weekly_scoped",
            LABEL_SCOPED.into(),
            WindowKind::Weekly,
            Some(SEVEN_DAYS),
        ),
        other if other.starts_with("weekly_") || other.starts_with("seven_day") => {
            (other, humanise(other), WindowKind::Weekly, Some(SEVEN_DAYS))
        }
        other => (other, humanise(other), WindowKind::Other, None),
    };
    (id.to_string(), label, wk, secs)
}

fn make_window(kind: &str, used_percent: f64, resets_at: Option<i64>) -> LimitWindow {
    let (id, label, wk, secs) = window_spec(kind);
    let mut w = LimitWindow::new(id, label, wk).used(used_percent);
    w.resets_at = resets_at;
    w.window_seconds = secs;
    w
}

/// Format a minor-unit amount: `297`, exponent 2, `USD` → `2.97 USD`.
fn money(minor: f64, exponent: i64, currency: &str) -> String {
    let exp = exponent.clamp(0, 6);
    let value = minor / 10f64.powi(exp as i32);
    format!("{:.*} {}", exp as usize, value, currency)
        .trim_end()
        .to_string()
}

/// Enterprise `spend`: used / limit (the rounded `percent` is ignored); skipped when disabled
/// or the limit is not positive.
fn spend_window(v: &Value) -> Option<LimitWindow> {
    let s = v.get("spend").filter(|s| s.is_object())?;
    if s.get("enabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let amount = |k: &str| num(s.get(k).and_then(|m| m.get("amount_minor")));
    let (used, limit) = (amount("used")?, amount("limit")?);
    if limit <= 0.0 || used < 0.0 {
        return None;
    }
    let fmt = |k: &str, value: f64| {
        let m = s.get(k);
        let exp = m
            .and_then(|m| m.get("exponent"))
            .and_then(Value::as_i64)
            .unwrap_or(2);
        let cur = m
            .and_then(|m| m.get("currency"))
            .and_then(Value::as_str)
            .unwrap_or("");
        money(value, exp, cur)
    };
    let detail = format!("{} / {}", fmt("used", used), fmt("limit", limit));
    Some(
        LimitWindow::new("spend", "Spend limit", WindowKind::Other)
            .used(used / limit * 100.0)
            .detail(detail),
    )
}

/// `extra_usage` (paid usage beyond the plan): shown only when enabled with a positive monthly
/// limit. Its own id, so it can never take the session's place as headline.
fn extra_usage_window(v: &Value) -> Option<LimitWindow> {
    let e = v.get("extra_usage").filter(|e| e.is_object())?;
    if e.get("is_enabled").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let limit = num(e.get("monthly_limit")).filter(|l| *l > 0.0)?;
    let used = num(e.get("used_credits")).filter(|u| *u >= 0.0)?;
    let exp = e.get("decimal_places").and_then(Value::as_i64).unwrap_or(2);
    let cur = e.get("currency").and_then(Value::as_str).unwrap_or("");
    let detail = format!("{} / {}", money(used, exp, cur), money(limit, exp, cur));
    Some(
        LimitWindow::new("extra_usage", LABEL_EXTRA_USAGE, WindowKind::Monthly)
            .used(used / limit * 100.0)
            .detail(detail),
    )
}

/// Parse the body of `GET /api/oauth/usage` (data-spec §2.3). Windows: `limits[]` first (an
/// entry without kind, percent or a parsable reset is dropped), then the named fields merged
/// in unless they duplicate an existing window, then `spend` / `extra_usage`; `session` first.
pub fn parse_claude_usage(body: &str, _now: i64) -> Result<Vec<LimitWindow>> {
    let v: Value = serde_json::from_str(body)?;
    if !v.is_object() {
        return Err(Error::other("Claude usage: unexpected response shape"));
    }
    let mut out: Vec<LimitWindow> = Vec::new();
    if let Some(limits) = v.get("limits").and_then(Value::as_array) {
        for l in limits {
            let Some(kind) = l
                .get("kind")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|k| !k.is_empty())
            else {
                continue;
            };
            let Some(pct) = num(l.get("percent")) else {
                continue;
            };
            let Some(resets) = l
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(rfc3339_secs)
            else {
                continue;
            };
            let mut w = make_window(kind, pct, Some(resets));
            if kind == "weekly_scoped" {
                if let Some(model) = l
                    .get("scope")
                    .and_then(|s| s.get("model"))
                    .and_then(|m| text(m.get("display_name")))
                {
                    w.label_key = format!("Weekly limit ({model})");
                }
            }
            if out.iter().any(|x| x.id == w.id) {
                continue;
            }
            out.push(w);
        }
    }
    // Named fields: a window that just rolled over leaves `limits` while its field remains.
    for field in [
        "five_hour",
        "seven_day",
        "seven_day_opus",
        "seven_day_sonnet",
    ] {
        let Some(obj) = v.get(field).filter(|o| o.is_object()) else {
            continue;
        };
        let Some(util) = num(obj.get("utilization")) else {
            continue;
        };
        let resets = obj
            .get("resets_at")
            .and_then(Value::as_str)
            .and_then(rfc3339_secs);
        let w = make_window(field, util, resets);
        let pct = w.used_percent.unwrap_or(f64::NAN);
        let model = match field {
            "seven_day_opus" => Some("opus"),
            "seven_day_sonnet" => Some("sonnet"),
            _ => None,
        };
        let duplicate = out.iter().any(|x| {
            x.id == w.id
                || x.label_key == w.label_key
                || match model {
                    // per-model windows: a scoped window naming the same model
                    Some(m) => x.id == "weekly_scoped" && x.label_key.to_lowercase().contains(m),
                    // session / weekly: same reset second and the same figure
                    None => {
                        resets.is_some()
                            && x.resets_at == resets
                            && x.used_percent.is_some_and(|u| (u - pct).abs() < 0.5)
                    }
                }
        });
        if !duplicate {
            out.push(w);
        }
    }
    if let Some(w) = spend_window(&v) {
        out.push(w);
    }
    if let Some(w) = extra_usage_window(&v) {
        out.push(w);
    }
    out.sort_by_key(|w| if w.id == "session" { 0 } else { 1 });
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Auth gate (sign-in and renewal never overlap)
// ---------------------------------------------------------------------------------------------

static AUTH_BUSY: AtomicBool = AtomicBool::new(false);

pub(crate) struct AuthGuard;

impl AuthGuard {
    pub(crate) fn try_acquire() -> Option<AuthGuard> {
        AUTH_BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| AuthGuard)
    }
}

impl Drop for AuthGuard {
    fn drop(&mut self) {
        AUTH_BUSY.store(false, Ordering::Release);
    }
}

/// State of the interactive Claude sign-in, for the UI.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaudeAuthState {
    /// A sign-in (or a renewal) holds the auth gate.
    pub busy: bool,
    /// "Complete sign-in in the browser or terminal window." / outcome message.
    pub message: Option<String>,
    /// Profile folder of the sign-in in progress or last finished.
    pub profile: Option<PathBuf>,
    /// When the sign-in console will be closed (unix secs).
    pub deadline: Option<i64>,
}

fn auth_state_cell() -> &'static Mutex<ClaudeAuthState> {
    static STATE: OnceLock<Mutex<ClaudeAuthState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ClaudeAuthState::default()))
}

/// Current sign-in state (busy flag reflects the shared gate).
pub fn claude_auth_state() -> ClaudeAuthState {
    let mut s = auth_state_cell()
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    s.busy = AUTH_BUSY.load(Ordering::Acquire);
    s
}

fn set_auth_state(f: impl FnOnce(&mut ClaudeAuthState)) {
    if let Ok(mut s) = auth_state_cell().lock() {
        f(&mut s);
    }
}

/// Clear the sign-in message once a profile reads successfully.
fn clear_auth_message_for(dir: &Path) {
    set_auth_state(|s| {
        if s.profile.as_deref() == Some(dir) && !AUTH_BUSY.load(Ordering::Acquire) {
            s.message = None;
        }
    });
}

// ---------------------------------------------------------------------------------------------
// CLI lookup and renewal
// ---------------------------------------------------------------------------------------------

/// Paths that belong to the Claude desktop app (its bundled CLI never writes the file we read).
pub(crate) fn is_desktop_owned(p: &Path) -> bool {
    let s = p.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
    s.contains("\\anthropicclaude\\")
        || s.contains("\\claude\\claude-code\\")
        || s.contains("\\windowsapps\\")
}

/// The standalone Claude Code CLI: `~\.local\bin`, `%APPDATA%\npm`, `%LOCALAPPDATA%\pnpm`,
/// `~\.volta\bin`, `~\.npm-global\bin`, `~\.bun\bin`, then PATH; `claude.exe` before `claude.cmd`.
pub fn find_claude_cli(env: &Env) -> Option<PathBuf> {
    let mut dirs = vec![
        env.home.join(".local").join("bin"),
        env.appdata.join("npm"),
        env.local_appdata.join("pnpm"),
        env.home.join(".volta").join("bin"),
        env.home.join(".npm-global").join("bin"),
        env.home.join(".bun").join("bin"),
    ];
    dirs.extend(path_dirs(env));
    first_file(&dirs, &["claude.exe", "claude.cmd"], is_desktop_owned)
}

fn renew_states() -> &'static Mutex<HashMap<PathBuf, RenewState>> {
    static STATES: OnceLock<Mutex<HashMap<PathBuf, RenewState>>> = OnceLock::new();
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Renew the token by running `<cli> -p` hidden when it is about to expire. Returns `true` when
/// the expiry in the file moved forward (the exit code is ignored).
fn maybe_renew(env: &Env, dir: &Path, cred: &Credential, now: i64) -> bool {
    if !util::spawn_allowed(env) {
        return false;
    }
    let Some(_gate) = AuthGuard::try_acquire() else {
        return false;
    };
    let due = renew_states()
        .lock()
        .map(|mut m| {
            m.entry(dir.to_path_buf())
                .or_default()
                .begin(cred.expires_at_ms, now.saturating_mul(1000))
        })
        .unwrap_or(false);
    if !due {
        return false;
    }
    let Some(cli) = find_claude_cli(env) else {
        log::warn!("claude: token about to expire and no standalone claude CLI found to renew it");
        return false;
    };
    let mut cmd = Command::new(&cli);
    cmd.arg("-p")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    scrub_claude_env(&mut cmd, &[]);
    cmd.env("CLAUDE_CONFIG_DIR", dir);
    set_flags(&mut cmd, CREATE_NO_WINDOW);
    match cmd.spawn() {
        Ok(mut child) => {
            let _ = util::wait_or_kill_tree(
                env,
                &mut child,
                util::RENEW_TIMEOUT,
                Duration::from_millis(100),
            );
        }
        Err(err) => {
            log::warn!("claude: token renewal could not start: {}", err.kind());
            return false;
        }
    }
    let after = read_credential(dir).and_then(|c| c.expires_at_ms);
    let renewed = matches!((after, cred.expires_at_ms), (Some(a), Some(b)) if a > b);
    if renewed {
        log::info!("claude: token renewed");
    } else {
        log::info!("claude: renewal ran but the token expiry did not move");
    }
    renewed
}

// ---------------------------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------------------------

fn request(cred: &Credential) -> HttpRequest {
    HttpRequest::get(CLAUDE_USAGE_URL)
        .bearer(&cred.token)
        .header("anthropic-beta", BETA_HEADER)
        .header("Accept", "application/json")
        .timeout(REQUEST_TIMEOUT)
}

/// `RateLimited.until` for a 429: `now + max(Retry-After, 60 s)`. The engine extends it with
/// its exponential back-off (see [`claude_backoff_secs`]).
fn rate_limit_until(resp: &HttpResponse, now: i64) -> (i64, i64) {
    let wait = resp.retry_after_secs(now).unwrap_or(0).max(60);
    (now + wait, wait)
}

/// Engine rule for Claude 429s: `60 s × 2^min(n−1, 4)` clamped to 60–900 s, then
/// `max(Retry-After)` (which may exceed the cap). `n` = consecutive 429s (≥ 1).
pub fn claude_backoff_secs(consecutive_429: u32, retry_after_secs: Option<i64>) -> i64 {
    let n = consecutive_429.max(1);
    let base = (60i64 << (n - 1).min(4)).clamp(60, 900);
    base.max(retry_after_secs.unwrap_or(0))
}

pub struct ClaudeProvider;

impl ClaudeProvider {
    fn fetch_inner(&self, account: &Account, ctx: &FetchContext) -> Reading {
        let now = ctx.now;
        let dir = ctx.env.profile_dir(account, ".claude");
        if !dir.is_dir() {
            return Reading::not_installed(account, now);
        }
        let meta = profile_meta(&ctx.env, &dir);
        let identity = meta.email.clone();
        // The desktop app's sign-in for this account (the default profile falls back to the
        // app's last account when `.claude.json` names none).
        let desktop = desktop_credentials(
            &ctx.env,
            meta.account_uuid.as_deref(),
            is_default_dir(&ctx.env, &dir),
        );
        let mut file_cred = read_credential(&dir);
        if !desktop.iter().any(|c| !c.expired(now)) {
            if let Some(c) = &file_cred {
                if maybe_renew(&ctx.env, &dir, c, now) {
                    if let Some(c) = read_credential(&dir) {
                        file_cred = Some(c);
                    }
                }
            }
        }
        let decorate = |mut r: Reading, cred: &Credential| {
            r.identity = identity.clone();
            r.plan = cred.plan();
            r
        };
        let candidates: Vec<Credential> = file_cred.iter().cloned().chain(desktop).collect();
        let mut queue: VecDeque<Credential> = rank_credentials(candidates, now).into();
        let Some(mut cred) = queue.pop_front() else {
            let Some(expired) = file_cred else {
                let mut r = Reading::signed_out(account, now);
                r.note = Some(NOTE_NO_CREDENTIAL.into());
                r.identity = identity;
                return r;
            };
            // Never send an expired token.
            let r = util::carry_over(
                account,
                ctx,
                ReadingStatus::Stale,
                Some(NOTE_EXPIRED.into()),
                None,
            )
            .unwrap_or_else(|| {
                Reading::with_status(account, now, ReadingStatus::SignedOut, NOTE_EXPIRED)
            });
            return decorate(r, &expired);
        };
        let mut tried: HashSet<String> = HashSet::from([cred.token.clone()]);
        let mut reread = false;
        loop {
            let resp = match ctx.http.send(&request(&cred)) {
                Ok(resp) => resp,
                Err(err) => {
                    let msg = transport_message(&err);
                    return decorate(
                        carry_or(
                            account,
                            ctx,
                            ReadingStatus::Error,
                            Some(msg.clone()),
                            Some(msg),
                        ),
                        &cred,
                    );
                }
            };
            match resp.status {
                200..=299 => {
                    let r = match parse_claude_usage(&resp.text(), now) {
                        Ok(windows) if windows.is_empty() => Reading::with_status(
                            account,
                            now,
                            ReadingStatus::Empty,
                            NOTE_NO_WINDOWS,
                        ),
                        Ok(windows) => {
                            clear_auth_message_for(&dir);
                            Reading::fresh(account, now, windows)
                        }
                        Err(err) => {
                            let msg = short(&format!("parse: {err}"));
                            carry_or(
                                account,
                                ctx,
                                ReadingStatus::Error,
                                Some(msg.clone()),
                                Some(msg),
                            )
                        }
                    };
                    return decorate(r, &cred);
                }
                401 => {
                    // Next store's token, then (once) the CLI file again: it may have rotated.
                    let next = queue.pop_front().or_else(|| {
                        if reread {
                            return None;
                        }
                        reread = true;
                        read_credential(&dir)
                            .filter(|c| !c.expired(now) && !tried.contains(&c.token))
                    });
                    if let Some(next) = next {
                        log::info!(
                            "claude: {:?} credential rejected, trying the next one",
                            cred.source
                        );
                        tried.insert(next.token.clone());
                        cred = next;
                        continue;
                    }
                    let r = carry_or(
                        account,
                        ctx,
                        ReadingStatus::SignedOut,
                        Some(NOTE_REJECTED.into()),
                        None,
                    );
                    return decorate(r, &cred);
                }
                403 => {
                    let r = carry_or(
                        account,
                        ctx,
                        ReadingStatus::Error,
                        Some(NOTE_403.into()),
                        Some("HTTP 403".into()),
                    );
                    return decorate(r, &cred);
                }
                429 => {
                    let (until, wait) = rate_limit_until(&resp, now);
                    let note = tf(
                        ctx.settings.lang(),
                        TPL_RATE_LIMITED,
                        &[("wait", &wait.to_string())],
                    );
                    let r = carry_or(
                        account,
                        ctx,
                        ReadingStatus::RateLimited { until: Some(until) },
                        Some(note),
                        Some("HTTP 429".into()),
                    );
                    return decorate(r, &cred);
                }
                code => {
                    let msg = format!("HTTP {code}");
                    return decorate(
                        carry_or(
                            account,
                            ctx,
                            ReadingStatus::Error,
                            Some(msg.clone()),
                            Some(msg),
                        ),
                        &cred,
                    );
                }
            }
        }
    }
}

pub(crate) fn transport_message(err: &HttpError) -> String {
    short(&err.to_string())
}

impl Provider for ClaudeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    /// Default `~\.claude` profile (when the folder exists) plus every `~\.claude-<id>` profile
    /// holding a credential (and the `CLAUDE_CONFIG_DIR` folder), named per data-spec §2.1.
    fn discover(&self, env: &Env) -> Vec<Account> {
        let profiles = claude_profiles(env);
        let names = claude_names(&profiles);
        profiles
            .iter()
            .zip(names)
            .map(|(p, name)| {
                let mut a = Account::new(ProviderKind::Claude, name);
                if p.id != "default" {
                    a.profile_path = p.dir.to_string_lossy().into_owned();
                }
                a
            })
            .collect()
    }

    fn local_identity(&self, account: &Account, env: &Env) -> LocalIdentity {
        local_identity(env, &env.profile_dir(account, ".claude"))
    }

    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
        self.fetch_inner(account, ctx)
    }

    /// Per-account attempt floor (data-spec §2.4 `MIN_FETCH_SECS`).
    fn min_interval(&self) -> Duration {
        Duration::from_secs(300)
    }

    /// Opens `claude auth login --claudeai` in a new visible console with `CLAUDE_CONFIG_DIR`
    /// set to the account's profile folder (data-spec §2.6). Returns once the console is open.
    fn sign_in(&self, account: &Account, env: &Env) -> Result<()> {
        claude_sign_in(account, env, Box::new(|_| {}))
    }
}

// ---------------------------------------------------------------------------------------------
// Sign-in and extra profiles
// ---------------------------------------------------------------------------------------------

fn pending_profiles() -> &'static Mutex<HashSet<PathBuf>> {
    static PENDING: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashSet::new()))
}

const SIGN_IN_SCRIPT: &str = r#"$Host.UI.RawUI.WindowTitle = 'CodeNotch - Claude sign-in'
Write-Host 'CodeNotch: signing in to Claude Code for this profile.'
Write-Host 'Finish the sign-in in your browser; this window closes by itself.'
Write-Host ''
& $env:CODENOTCH_CLAUDE_CLI auth login --claudeai
$code = $LASTEXITCODE
if ($null -eq $code) { $code = 1 }
Write-Host ''
if ($code -eq 0) { Write-Host 'Sign-in complete.'; Start-Sleep -Seconds 2 }
else { Write-Host 'Sign-in did not complete. You can try again from CodeNotch.'; Start-Sleep -Seconds 8 }
exit $code
"#;

/// Remove a profile folder created by [`add_claude_account`] when it still has no credential.
fn discard_pending(dir: &Path) {
    if !has_credential_file(dir) {
        if let Err(err) = std::fs::remove_dir_all(dir) {
            log::warn!(
                "claude: could not remove unused profile folder: {}",
                err.kind()
            );
        }
    }
}

/// Start the interactive sign-in for `account` and return once the console is open. A
/// background thread holds the auth gate, closes the console after [`SIGN_IN_TIMEOUT`]
/// ([`ADD_ACCOUNT_TIMEOUT`] for a profile made by [`add_claude_account`], whose folder is
/// removed again when no credential appears) and finally calls `on_done(success)` — the
/// engine should then refresh that account.
pub fn claude_sign_in(
    account: &Account,
    env: &Env,
    on_done: Box<dyn FnOnce(bool) + Send>,
) -> Result<()> {
    let dir = env.profile_dir(account, ".claude");
    let pending = pending_profiles()
        .lock()
        .map(|mut p| p.remove(&dir))
        .unwrap_or(false);
    let fail = |msg: &str| {
        if pending {
            discard_pending(&dir);
        }
        Err(Error::other(msg))
    };
    if !util::spawn_allowed(env) {
        if pending {
            discard_pending(&dir);
        }
        return Err(Error::unsupported(MSG_NO_SPAWN));
    }
    let Some(cli) = find_claude_cli(env) else {
        return fail(MSG_CLI_NOT_FOUND);
    };
    let Some(gate) = AuthGuard::try_acquire() else {
        return fail(MSG_AUTH_BUSY);
    };
    if let Err(err) = std::fs::create_dir_all(&dir) {
        return Err(Error::io("Claude profile folder", err));
    }
    let root = env
        .var("SystemRoot")
        .unwrap_or_else(|| "C:\\Windows".into());
    let powershell = PathBuf::from(root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let mut cmd = Command::new(powershell);
    cmd.args(["-NoLogo", "-NoProfile", "-Command", SIGN_IN_SCRIPT])
        .current_dir(&env.home);
    scrub_claude_env(
        &mut cmd,
        &[
            "CLAUDE_CONFIG_DIR",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
        ],
    );
    cmd.env("CLAUDE_CONFIG_DIR", &dir)
        .env("CODENOTCH_CLAUDE_CLI", &cli);
    set_flags(&mut cmd, CREATE_NEW_CONSOLE);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => {
            log::warn!("claude: sign-in console failed to start: {}", err.kind());
            drop(gate);
            return fail(MSG_SIGN_IN_WINDOW);
        }
    };
    let timeout = if pending {
        ADD_ACCOUNT_TIMEOUT
    } else {
        SIGN_IN_TIMEOUT
    };
    let now = codenotch_core::now_secs();
    set_auth_state(|s| {
        s.message = Some(MSG_SIGN_IN_PENDING.into());
        s.profile = Some(dir.clone());
        s.deadline = Some(now + timeout.as_secs() as i64);
    });
    let env = env.clone();
    let spawned = std::thread::Builder::new()
        .name("claude-sign-in".into())
        .spawn(move || {
            let _gate = gate;
            // The whole tree (powershell → claude.cmd → node) goes on timeout: killing the direct
            // child first, then `taskkill /T` by its pid, found no tree any more.
            let exited_ok =
                util::wait_or_kill_tree(&env, &mut child, timeout, Duration::from_millis(250))
                    .unwrap_or(false);
            let success = exited_ok && read_credential(&dir).is_some();
            if pending && !success {
                discard_pending(&dir);
            }
            set_auth_state(|s| {
                s.deadline = None;
                s.message = Some(if success {
                    MSG_SIGN_IN_DONE.into()
                } else {
                    MSG_SIGN_IN_FAILED.into()
                });
            });
            log::info!("claude: sign-in finished (success: {success})");
            drop(_gate);
            on_done(success);
        });
    if spawned.is_err() {
        log::error!("claude: could not start the sign-in watcher thread");
    }
    Ok(())
}

/// Create `~\.claude-<n>` (smallest n ≥ 2 whose folder does not exist) and its account. The
/// caller adds it to the config (skip it if its folder is already configured) and calls
/// `sign_in`; the folder is removed again if that sign-in does not leave a credential.
pub fn add_claude_account(env: &Env) -> Result<Account> {
    for n in 2..10_000u32 {
        let dir = env.home_path(&format!(".claude-{n}"));
        if dir.exists() {
            continue;
        }
        std::fs::create_dir_all(&dir).map_err(|e| Error::io("create Claude profile folder", e))?;
        if let Ok(mut p) = pending_profiles().lock() {
            p.insert(dir.clone());
        }
        let mut account = Account::new(ProviderKind::Claude, format!("Claude ({n})"));
        account.profile_path = dir.to_string_lossy().into_owned();
        return Ok(account);
    }
    Err(Error::other("no free Claude profile folder name"))
}

#[cfg(test)]
pub(crate) fn is_pending(dir: &Path) -> bool {
    pending_profiles()
        .lock()
        .map(|p| p.contains(dir))
        .unwrap_or(false)
}
