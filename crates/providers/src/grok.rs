//! Grok (data-spec §4): `~\.grok\auth.json` entries minted by `https://auth.x.ai` only, the
//! CLI billing endpoint, renewal by running `grok models` hidden.

use crate::util::{
    self, carry_or, first_file, num, path_dirs, rfc3339_secs, set_flags, short, split_camel, text,
    RenewState,
};
use codenotch_core::http::HttpRequest;
use codenotch_core::model::{LimitWindow, ProviderKind, Reading, ReadingStatus, WindowKind};
use codenotch_core::provider::{Env, FetchContext, LocalIdentity, Provider};
use codenotch_core::{Account, Error, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const GROK_BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
/// The only issuer whose sessions are ever sent.
pub const TRUSTED_ISSUER: &str = "https://auth.x.ai";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) const NOTE_SIGNED_OUT: &str =
    "Run grok login — it signs in and refreshes the token this reads.";
pub(crate) const NOTE_REJECTED: &str = "Grok session was rejected — run grok login again";
pub(crate) const NOTE_RATE_LIMITED: &str = "Grok is rate limiting; the last reading stands";
pub(crate) const NOTE_NOTHING_METERED: &str = "Grok has nothing metered on this account yet";
pub(crate) const NOTE_NO_CONFIG: &str = "Grok returned no billing config";

/// A trusted Grok session. `Debug` hides the key.
pub(crate) struct GrokCreds {
    key: String,
    /// Unix secs; `None` = the entry does not say.
    pub expires_at: Option<i64>,
    pub email: Option<String>,
}

impl std::fmt::Debug for GrokCreds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrokCreds")
            .field("expires_at", &self.expires_at)
            .field("email", &self.email)
            .finish()
    }
}

/// Issuer check: the part of the map key before `::` equals the trusted issuer exactly, or the
/// entry's `oidc_issuer` does.
pub(crate) fn is_trusted(key: &str, entry: &Value) -> bool {
    key.split("::").next() == Some(TRUSTED_ISSUER)
        || entry.get("oidc_issuer").and_then(Value::as_str) == Some(TRUSTED_ISSUER)
}

/// First trusted entry still valid (or with no expiry), else the first trusted entry.
pub(crate) fn pick_creds(body: &str, now: i64) -> Option<GrokCreds> {
    let root: Value = serde_json::from_str(body).ok()?;
    let mut trusted: Vec<GrokCreds> = root
        .as_object()?
        .iter()
        .filter(|(k, e)| e.is_object() && is_trusted(k, e))
        .filter_map(|(_, e)| {
            let key = text(e.get("key"))?;
            Some(GrokCreds {
                key,
                expires_at: e
                    .get("expires_at")
                    .and_then(Value::as_str)
                    .and_then(rfc3339_secs),
                email: text(e.get("email")),
            })
        })
        .collect();
    let live = trusted
        .iter()
        .position(|c| c.expires_at.is_none_or(|e| e > now));
    match live {
        Some(i) => Some(trusted.swap_remove(i)),
        None => trusted.into_iter().next(),
    }
}

fn read_creds(dir: &Path, now: i64) -> Option<GrokCreds> {
    std::fs::read_to_string(dir.join("auth.json"))
        .ok()
        .and_then(|t| pick_creds(&t, now))
}

/// Plan the Grok CLI cached in `settings_cache.json`: `payload` (a JSON object, or a string
/// holding one) → `settings.subscription_tier_display` ("SuperGrok Heavy"), else
/// `settings.subscription_tier` made readable.
pub(crate) fn plan_from_settings_cache(body: &str) -> Option<String> {
    let root: Value = serde_json::from_str(body).ok()?;
    let payload = root.get("payload")?;
    let payload = match payload.as_str() {
        Some(s) => serde_json::from_str::<Value>(s).ok()?,
        None => payload.clone(),
    };
    let settings = payload.get("settings")?;
    text(settings.get("subscription_tier_display"))
        .or_else(|| text(settings.get("subscription_tier")).map(|t| util::humanise(&t)))
}

/// E-mail of the trusted session and the cached plan (see [`plan_from_settings_cache`]).
pub(crate) fn local_identity(dir: &Path, now: i64) -> LocalIdentity {
    LocalIdentity {
        identity: read_creds(dir, now).and_then(|c| c.email),
        plan: std::fs::read_to_string(dir.join("settings_cache.json"))
            .ok()
            .and_then(|t| plan_from_settings_cache(&t)),
    }
}

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

/// `None` = no `config` object at all.
pub(crate) fn grok_windows(body: &str) -> Result<Option<Vec<LimitWindow>>> {
    let v: Value = serde_json::from_str(body)?;
    let Some(cfg) = v.get("config").filter(|c| c.is_object()) else {
        return Ok(None);
    };
    let period = cfg.get("currentPeriod").filter(|p| p.is_object());
    let period_type = period
        .and_then(|p| p.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_uppercase();
    let (end, start) = match period
        .and_then(|p| p.get("end"))
        .and_then(Value::as_str)
        .and_then(rfc3339_secs)
    {
        Some(end) => (
            Some(end),
            period
                .and_then(|p| p.get("start"))
                .and_then(Value::as_str)
                .and_then(rfc3339_secs),
        ),
        None => (
            cfg.get("billingPeriodEnd")
                .and_then(Value::as_str)
                .and_then(rfc3339_secs),
            cfg.get("billingPeriodStart")
                .and_then(Value::as_str)
                .and_then(rfc3339_secs),
        ),
    };
    let kind = if period_type.contains("WEEKLY") {
        WindowKind::Weekly
    } else if period_type.contains("MONTHLY") {
        WindowKind::Monthly
    } else {
        WindowKind::Other
    };
    let shape = |mut w: LimitWindow| {
        w.kind = kind;
        w.resets_at = end;
        w.window_seconds = match (start, end) {
            (Some(s), Some(e)) if e > s => Some(e - s),
            _ => None,
        };
        w
    };
    let products: Vec<(String, Option<f64>)> = cfg
        .get("productUsage")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| Some((text(p.get("product"))?, num(p.get("usagePercent")))))
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    if let Some(pct) = num(cfg.get("creditUsagePercent")) {
        let product = products
            .first()
            .map(|(p, _)| split_camel(p))
            .unwrap_or_else(|| "Grok Build".into());
        out.push(shape(
            LimitWindow::new("credits", "Credits", kind)
                .used(pct)
                .detail(product),
        ));
    } else {
        for (product, pct) in &products {
            let Some(pct) = pct else { continue };
            let w = if out.is_empty() {
                LimitWindow::new("credits", "Credits", kind).detail(split_camel(product))
            } else {
                LimitWindow::new(product.clone(), split_camel(product), kind)
            };
            if !out.iter().any(|x: &LimitWindow| x.id == w.id) {
                out.push(shape(w.used(*pct)));
            }
        }
    }
    if out.is_empty() && period_type.contains("WEEKLY") {
        // A fresh weekly period reads 0 %, not "unmetered".
        out.push(shape(
            LimitWindow::new("credits", "Credits", kind).used(0.0),
        ));
    }
    Ok(Some(out))
}

/// Parse the body of `GET /v1/billing?format=credits` (data-spec §4.3). An empty list means
/// nothing is metered; a body without `config` is an error.
pub fn parse_grok_usage(body: &str, _now: i64) -> Result<Vec<LimitWindow>> {
    grok_windows(body)?.ok_or_else(|| Error::other(NOTE_NO_CONFIG))
}

// ---------------------------------------------------------------------------------------------
// Renewal
// ---------------------------------------------------------------------------------------------

/// `<profile>\bin\grok.exe|.cmd`, then PATH.
pub fn find_grok_cli(env: &Env, profile: &Path) -> Option<PathBuf> {
    let mut dirs = vec![profile.join("bin")];
    dirs.extend(path_dirs(env));
    first_file(&dirs, &["grok.exe", "grok.cmd"], |_| false)
}

fn renew_states() -> &'static Mutex<HashMap<PathBuf, RenewState>> {
    static STATES: OnceLock<Mutex<HashMap<PathBuf, RenewState>>> = OnceLock::new();
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Run `grok models` hidden (GROK_HOME = profile) when the session is about to expire. `true`
/// when the file's `expires_at` moved forward.
fn maybe_renew(env: &Env, dir: &Path, creds: &GrokCreds, now: i64) -> bool {
    if !util::spawn_allowed(env) {
        return false;
    }
    let exp_ms = creds.expires_at.map(|e| e.saturating_mul(1000));
    let due = renew_states()
        .lock()
        .map(|mut m| {
            m.entry(dir.to_path_buf())
                .or_default()
                .begin(exp_ms, now.saturating_mul(1000))
        })
        .unwrap_or(false);
    if !due {
        return false;
    }
    let Some(cli) = find_grok_cli(env, dir) else {
        log::warn!("grok: session about to expire and no grok CLI found to renew it");
        return false;
    };
    let mut cmd = Command::new(cli);
    cmd.arg("models")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("GROK_HOME", dir);
    set_flags(&mut cmd, util::CREATE_NO_WINDOW);
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
            log::warn!("grok: renewal could not start: {}", err.kind());
            return false;
        }
    }
    let after = read_creds(dir, now).and_then(|c| c.expires_at);
    let renewed = matches!((after, creds.expires_at), (Some(a), Some(b)) if a > b);
    log::info!("grok: renewal ran (renewed: {renewed})");
    renewed
}

// ---------------------------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------------------------

pub struct GrokProvider;

impl GrokProvider {
    fn fetch_inner(&self, account: &Account, ctx: &FetchContext) -> Reading {
        let now = ctx.now;
        let dir = ctx.env.profile_dir(account, ".grok");
        if !dir.join("auth.json").is_file() {
            return Reading::not_installed(account, now);
        }
        let Some(mut creds) = read_creds(&dir, now) else {
            return Reading::with_status(account, now, ReadingStatus::SignedOut, NOTE_SIGNED_OUT);
        };
        if maybe_renew(&ctx.env, &dir, &creds, now) {
            if let Some(c) = read_creds(&dir, now) {
                creds = c;
            }
        }
        // An expired session is still sent: only the endpoint can decide.
        let req = HttpRequest::get(GROK_BILLING_URL)
            .bearer(&creds.key)
            .header("X-XAI-Token-Auth", "xai-grok-cli")
            .header("Accept", "application/json")
            .timeout(REQUEST_TIMEOUT);
        let mut r = match ctx.http.send(&req) {
            Err(err) => {
                let msg = short(&err.to_string());
                carry_or(
                    account,
                    ctx,
                    ReadingStatus::Error,
                    Some(msg.clone()),
                    Some(msg),
                )
            }
            Ok(resp) => match resp.status {
                200..=299 => match grok_windows(&resp.text()) {
                    Ok(Some(w)) if w.is_empty() => Reading::with_status(
                        account,
                        now,
                        ReadingStatus::Empty,
                        NOTE_NOTHING_METERED,
                    ),
                    Ok(Some(w)) => {
                        let mut r = Reading::fresh(account, now, w);
                        r.note = Some(match &creds.email {
                            Some(e) => format!("{e} · via Grok CLI"),
                            None => "via Grok CLI".into(),
                        });
                        r
                    }
                    Ok(None) => {
                        Reading::with_status(account, now, ReadingStatus::Empty, NOTE_NO_CONFIG)
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
                },
                401 | 403 => carry_or(
                    account,
                    ctx,
                    ReadingStatus::SignedOut,
                    Some(NOTE_REJECTED.into()),
                    None,
                ),
                429 => {
                    let until = resp.retry_after_secs(now).map(|s| now + s);
                    carry_or(
                        account,
                        ctx,
                        ReadingStatus::RateLimited { until },
                        Some(NOTE_RATE_LIMITED.into()),
                        Some("HTTP 429".into()),
                    )
                }
                code => {
                    let msg = format!("HTTP {code}");
                    carry_or(
                        account,
                        ctx,
                        ReadingStatus::Error,
                        Some(msg.clone()),
                        Some(msg),
                    )
                }
            },
        };
        r.identity = creds.email.clone();
        r
    }
}

impl Provider for GrokProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Grok
    }

    /// One account when `~\.grok\auth.json` exists.
    fn discover(&self, env: &Env) -> Vec<Account> {
        if env.home_path(".grok").join("auth.json").is_file() {
            vec![Account::new(ProviderKind::Grok, "Grok")]
        } else {
            Vec::new()
        }
    }

    fn local_identity(&self, account: &Account, env: &Env) -> LocalIdentity {
        local_identity(
            &env.profile_dir(account, ".grok"),
            codenotch_core::now_secs(),
        )
    }

    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
        self.fetch_inner(account, ctx)
    }

    /// Loop period (data-spec §4.4); no attempt floor on manual refresh.
    fn min_interval(&self) -> Duration {
        Duration::from_secs(300)
    }
}
