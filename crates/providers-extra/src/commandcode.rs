//! Command Code.
//!
//! Key: the account's `secretEnv` variable, `COMMAND_CODE_API_KEY`, else `apiKey` in
//! `~\.commandcode\auth.json` (the desktop app's sign-in, read only). Four GETs under
//! `https://api.commandcode.ai/alpha`: `/whoami` (→ `org.id`), `/billing/credits?orgId=`,
//! `/billing/subscriptions?orgId=` (→ `data.currentPeriodStart/End`), `/usage/summary?orgId=&since=`.
//! Windows: `monthly` = totalCost / (totalCost + credits.monthlyCredits), plus `fiveHour` and
//! `weekly` from `windowLimits.{fiveHour,weekly}.{used,cap,resetAt}` when `cap > 0`.

use crate::i18n::tf;
use crate::util;
use codenotch_core::model::{LimitWindow, Reading, ReadingStatus, WindowKind};
use codenotch_core::provider::{Env, FetchContext};
use codenotch_core::secrets::SecretStore;
use codenotch_core::{Account, HttpRequest};
use serde_json::Value;
use std::path::PathBuf;

pub(crate) const API: &str = "https://api.commandcode.ai/alpha";
pub(crate) const NOTE_SIGN_IN: &str = "Sign in with the Command Code app to read your usage";
pub(crate) const NOTE_BILLING: &str = "Command Code billing";
pub(crate) const NOTE_NOTHING_METERED: &str =
    "Command Code has nothing metered on this account yet";
pub(crate) const NOTE_REJECTED: &str =
    "Command Code rejected its sign-in; sign in again in the Command Code app";
pub(crate) const NOTE_RATE_LIMITED: &str = "Command Code is rate limited; retrying shortly";
pub(crate) const LABEL_FIVE_HOUR: &str = "5h limit";
/// Template translated at fetch time (`{e}` stays as is).
pub(crate) const TPL_FAILED: &str = "Command Code read failed ({e})";

pub(crate) fn auth_path(env: &Env, account: Option<&Account>) -> PathBuf {
    env.profile_dir_opt(account, ".commandcode")
        .join("auth.json")
}

/// `Env::profile_dir` for an optional account.
trait ProfileDirOpt {
    fn profile_dir_opt(&self, account: Option<&Account>, default: &str) -> PathBuf;
}

impl ProfileDirOpt for Env {
    fn profile_dir_opt(&self, account: Option<&Account>, default: &str) -> PathBuf {
        match account {
            Some(a) => self.profile_dir(a, default),
            None => self.home.join(default),
        }
    }
}

fn credential(
    env: &Env,
    account: Option<&Account>,
    store: Option<&dyn SecretStore>,
) -> Option<String> {
    if let Some(k) = account.and_then(|a| util::account_secret(a, env, store)) {
        return Some(k);
    }
    if let Some(k) = env
        .var("COMMAND_CODE_API_KEY")
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
    {
        return Some(k);
    }
    util::json_str(util::read_json(&auth_path(env, account))?.get("apiKey"))
}

pub(crate) fn discover(env: &Env) -> bool {
    auth_path(env, None).is_file() || env.var("COMMAND_CODE_API_KEY").is_some()
}

/// Reset in RFC 3339 or a number (> 1e12 = ms, else seconds); 0 = unknown.
fn reset_at(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    match v.as_str() {
        Some(s) => util::rfc3339_secs(s),
        None => util::epoch_secs(v.as_f64()?, 1e12),
    }
}

fn limit_window(
    id: &str,
    label: &str,
    kind: WindowKind,
    len: i64,
    v: &Value,
) -> Option<LimitWindow> {
    let cap = util::json_f64(v.get("cap")).filter(|c| *c > 0.0)?;
    let used = util::json_f64(v.get("used"))?;
    let mut w = LimitWindow::new(id, label, kind)
        .used(used / cap * 100.0)
        .window_seconds(len);
    w.resets_at = reset_at(v.get("resetAt"));
    Some(w)
}

/// (summary, credits, subscription) → windows. Nothing is invented when a figure is missing.
pub(crate) fn windows_from(
    summary: &Value,
    credits: &Value,
    subscription: &Value,
) -> Vec<LimitWindow> {
    let sub = subscription.get("data").unwrap_or(subscription);
    let mut out = Vec::new();
    let spent = util::json_f64(summary.get("totalCost"));
    let left = util::json_f64(credits.pointer("/credits/monthlyCredits"));
    if let (Some(spent), Some(left)) = (spent, left) {
        let cap = spent + left.max(0.0);
        if cap > 0.0 {
            let mut w = LimitWindow::new("monthly", "Monthly limit", WindowKind::Monthly)
                .used(spent / cap * 100.0);
            w.resets_at = reset_at(sub.get("currentPeriodEnd"));
            w.window_seconds = util::span(reset_at(sub.get("currentPeriodStart")), w.resets_at);
            out.push(w);
        }
    }
    if let Some(w) = credits.pointer("/windowLimits/fiveHour").and_then(|v| {
        limit_window(
            "fiveHour",
            LABEL_FIVE_HOUR,
            WindowKind::Session,
            util::FIVE_HOURS,
            v,
        )
    }) {
        out.push(w);
    }
    if let Some(w) = credits.pointer("/windowLimits/weekly").and_then(|v| {
        limit_window(
            "weekly",
            "Weekly limit",
            WindowKind::Weekly,
            util::SEVEN_DAYS,
            v,
        )
    }) {
        out.push(w);
    }
    out
}

enum Fail {
    Auth,
    Rate(Option<i64>),
    Other(String),
}

fn get(ctx: &FetchContext, token: &str, path: &str, query: &[(&str, &str)]) -> Result<Value, Fail> {
    let req = HttpRequest::get(util::with_query(&format!("{API}{path}"), query))
        .bearer(token)
        .header("User-Agent", "command-code-desktop")
        .header("x-command-code-version", "desktop")
        .header("Accept", "application/json")
        .timeout(util::HTTP_TIMEOUT);
    let resp = ctx
        .http
        .send(&req)
        .map_err(|e| Fail::Other(e.to_string()))?;
    match resp.status {
        401 | 403 => Err(Fail::Auth),
        429 => Err(Fail::Rate(util::retry_after(&resp, ctx.now))),
        s if !resp.is_success() => Err(Fail::Other(format!("HTTP {s}"))),
        _ => resp
            .json::<Value>()
            .map_err(|_| Fail::Other("invalid JSON".into())),
    }
}

fn read(ctx: &FetchContext, token: &str) -> Result<Vec<LimitWindow>, Fail> {
    let whoami = get(ctx, token, "/whoami", &[])?;
    let org = whoami
        .pointer("/org/id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut q: Vec<(&str, &str)> = Vec::new();
    if let Some(org) = org.as_deref() {
        q.push(("orgId", org));
    }
    let credits = get(ctx, token, "/billing/credits", &q)?;
    let subscription = get(ctx, token, "/billing/subscriptions", &q)?;
    let since = subscription
        .get("data")
        .unwrap_or(&subscription)
        .get("currentPeriodStart")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut sq = q.clone();
    if let Some(since) = since.as_deref() {
        sq.push(("since", since));
    }
    let summary = get(ctx, token, "/usage/summary", &sq)?;
    if !summary.is_object() || !credits.is_object() {
        return Err(Fail::Other("invalid billing response".into()));
    }
    Ok(windows_from(&summary, &credits, &subscription))
}

pub(crate) fn fetch(account: &Account, ctx: &FetchContext) -> Reading {
    let env = &ctx.env;
    let Some(token) = credential(env, Some(account), ctx.secrets.as_deref()) else {
        if auth_path(env, Some(account)).is_file() {
            return Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, NOTE_SIGN_IN);
        }
        return Reading::not_installed(account, ctx.now);
    };
    match read(ctx, &token) {
        Ok(windows) if !windows.is_empty() => {
            let mut r = Reading::fresh(account, ctx.now, windows);
            r.note = Some(NOTE_BILLING.into());
            r
        }
        Ok(_) => Reading::with_status(account, ctx.now, ReadingStatus::Empty, NOTE_NOTHING_METERED),
        Err(Fail::Auth) => {
            Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, NOTE_REJECTED)
        }
        Err(Fail::Rate(ra)) => {
            let wait = ra.unwrap_or(0).max(60);
            util::rate_limited(account, ctx, wait, 1, NOTE_RATE_LIMITED)
        }
        Err(Fail::Other(e)) => util::failed(
            account,
            ctx,
            &tf(ctx.settings.lang(), TPL_FAILED, &[("e", &e)]),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::ProviderKind;
    use codenotch_core::FakeHttp;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn monthly_and_window_limits() {
        let ws = windows_from(
            &json!({"totalCost":50.5}),
            &json!({"credits":{"monthlyCredits":19.5},"windowLimits":{
                "fiveHour":{"used":0,"cap":14,"resetAt":0},
                "weekly":{"used":22.45,"cap":35,"resetAt":1787857355088u64}}}),
            &json!({"data":{"currentPeriodStart":"2026-08-13T15:21:27.000Z","currentPeriodEnd":"2026-09-13T15:21:27.000Z"}}),
        );
        assert_eq!(
            ws.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
            ["monthly", "fiveHour", "weekly"]
        );
        assert!((ws[0].used_percent.unwrap() - 50.5 / 70.0 * 100.0).abs() < 1e-6);
        assert_eq!(ws[0].window_seconds, Some(31 * 86_400));
        assert_eq!(ws[1].used_percent, Some(0.0));
        assert_eq!(ws[1].resets_at, None);
        assert_eq!(ws[2].resets_at, Some(1_787_857_355));
    }

    #[test]
    fn nothing_is_invented() {
        assert!(windows_from(
            &json!({"totalCost":0}),
            &json!({"credits":{"monthlyCredits":0}}),
            &json!({})
        )
        .is_empty());
        assert!(windows_from(
            &json!({}),
            &json!({"credits":{"monthlyCredits":5}}),
            &json!({})
        )
        .is_empty());
        assert!(windows_from(
            &json!({"totalCost":1}),
            &json!({"windowLimits":{"weekly":{"cap":0,"used":1}}}),
            &json!({})
        )
        .is_empty());
        assert!(windows_from(
            &json!({"totalCost":1}),
            &json!({"windowLimits":{"weekly":{"cap":10}}}),
            &json!({})
        )
        .is_empty());
    }

    #[test]
    fn fetch_sequence_with_org_and_since() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let account = Account::new(ProviderKind::CommandCode, "Command Code");
        assert!(!discover(&env));
        assert_eq!(
            fetch(
                &account,
                &FetchContext::new(0, Arc::new(FakeHttp::new()), env.clone())
            )
            .status,
            ReadingStatus::NotInstalled
        );
        let cc = env.home.join(".commandcode");
        std::fs::create_dir_all(&cc).unwrap();
        std::fs::write(cc.join("auth.json"), r#"{"apiKey":"  "}"#).unwrap();
        assert!(discover(&env));
        assert_eq!(
            fetch(
                &account,
                &FetchContext::new(0, Arc::new(FakeHttp::new()), env.clone())
            )
            .status,
            ReadingStatus::SignedOut
        );
        std::fs::write(cc.join("auth.json"), r#"{"apiKey":"cc-key"}"#).unwrap();
        let http = Arc::new(
            FakeHttp::new()
                .on(&format!("{API}/whoami"), 200, r#"{"org":{"id":"org 1"}}"#)
                .on(&format!("{API}/billing/credits"), 200, r#"{"credits":{"monthlyCredits":30},"windowLimits":{"weekly":{"used":7,"cap":35}}}"#)
                .on(&format!("{API}/billing/subscriptions"), 200, r#"{"data":{"currentPeriodStart":"2026-09-01T00:00:00Z","currentPeriodEnd":"2026-10-01T00:00:00Z"}}"#)
                .on(&format!("{API}/usage/summary"), 200, r#"{"totalCost":10}"#),
        );
        let r = fetch(&account, &FetchContext::new(0, http.clone(), env.clone()));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.headline_percent(), Some(25.0));
        assert_eq!(r.weekly().unwrap().used_percent, Some(20.0));
        let urls: Vec<String> = http.requests().iter().map(|q| q.url.clone()).collect();
        assert_eq!(urls[1], format!("{API}/billing/credits?orgId=org%201"));
        assert_eq!(
            urls[3],
            format!("{API}/usage/summary?orgId=org%201&since=2026-09-01T00%3A00%3A00Z")
        );
        assert_eq!(
            http.requests()[0].header_value("user-agent"),
            Some("command-code-desktop")
        );

        let http = Arc::new(FakeHttp::new().on(&format!("{API}/whoami"), 401, ""));
        assert_eq!(
            fetch(&account, &FetchContext::new(0, http, env.clone())).status,
            ReadingStatus::SignedOut
        );
        let http = Arc::new(FakeHttp::new().on(&format!("{API}/whoami"), 429, ""));
        assert_eq!(
            fetch(&account, &FetchContext::new(10, http, env.clone())).status,
            ReadingStatus::RateLimited { until: Some(70) }
        );
        let env = env.with_var("COMMAND_CODE_API_KEY", "env-key");
        let http = Arc::new(FakeHttp::new().on(&format!("{API}/whoami"), 500, ""));
        let r = fetch(&account, &FetchContext::new(0, http.clone(), env));
        assert_eq!(r.status, ReadingStatus::Error);
        assert_eq!(
            http.requests()[0].header_value("authorization"),
            Some("Bearer env-key")
        );
    }
}
