//! Custom JSON endpoint: `GET account.endpoint`, optional bearer from the environment variable
//! named by `account.secret_env`, else the token saved for the account in the secret store
//! (`FetchContext::account_secret`).

use crate::i18n::tf;
use crate::util::{carry_or, flexible_time, num, short, text};
use codenotch_core::http::{allowed_endpoint, HttpRequest};
use codenotch_core::model::{
    percent_from_fraction_or_percent, LimitWindow, ProviderKind, Reading, ReadingStatus, WindowKind,
};
use codenotch_core::provider::{Env, FetchContext, Provider};
use codenotch_core::{Account, Error, Result};
use serde_json::Value;
use std::time::Duration;

pub(crate) const NOTE_NO_ENDPOINT: &str = "No endpoint configured";
pub(crate) const NOTE_BAD_ENDPOINT: &str = "Endpoint must use HTTPS (plain HTTP only to localhost)";
pub(crate) const ERR_NO_LIST: &str = "endpoint returned no \"windows\" list";
pub(crate) const ERR_SHAPE: &str = "endpoint returned an unexpected JSON shape";
pub(crate) const ERR_NO_WINDOW: &str = "endpoint returned no usable window";
/// Template translated at fetch time.
pub(crate) const TPL_ENV_UNSET: &str = "Environment variable {var} is not set";

fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        "window".into()
    } else {
        out
    }
}

fn first<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| v.get(*k).filter(|x| !x.is_null()))
}

/// Kind from the window length, else from words in the label.
fn kind_for(seconds: Option<i64>, label: &str) -> WindowKind {
    if let Some(s) = seconds {
        let days = s as f64 / 86400.0;
        return if s <= 86400 {
            WindowKind::Session
        } else if (6.5..=7.5).contains(&days) {
            WindowKind::Weekly
        } else if (27.5..=31.5).contains(&days) {
            WindowKind::Monthly
        } else {
            WindowKind::Other
        };
    }
    let l = label.to_ascii_lowercase();
    if l.contains("week") {
        WindowKind::Weekly
    } else if l.contains("month") {
        WindowKind::Monthly
    } else if l.contains("session") || l.contains("hour") || l.contains("daily") {
        WindowKind::Session
    } else {
        WindowKind::Other
    }
}

fn custom_window(v: &Value) -> Option<LimitWindow> {
    if !v.is_object() {
        return None;
    }
    let label = text(first(v, &["label", "name"]));
    let id = text(v.get("id"));
    let label = label.or_else(|| id.clone())?;
    let id = id.unwrap_or_else(|| slug(&label));
    let seconds = num(first(v, &["windowSeconds", "window_seconds"]))
        .filter(|s| *s > 0.0)
        .map(|s| s as i64);
    let mut w = LimitWindow::new(id, label.clone(), kind_for(seconds, &label));
    w.window_seconds = seconds;
    w.used_percent = num(first(v, &["usedPercent", "used_percent", "utilization"]))
        .and_then(percent_from_fraction_or_percent);
    if w.used_percent.is_none() {
        let used = num(v.get("used"));
        let limit = num(v.get("limit")).filter(|l| *l > 0.0);
        if let (Some(u), Some(l)) = (used, limit) {
            w.used_percent = codenotch_core::model::clamp_percent(u / l * 100.0);
        }
    }
    w.resets_at = flexible_time(first(v, &["resetsAt", "resets_at", "reset_at", "resetAt"]));
    w.count = num(v.get("count")).map(|c| c.round() as i64);
    w.detail = text(v.get("detail"));
    w.group = text(v.get("group"));
    if w.used_percent.is_none() && w.count.is_none() && w.resets_at.is_none() {
        return None;
    }
    Some(w)
}

/// Endpoint body:
/// `{"windows":[{"id":"quota","label":"Monthly quota","usedPercent":42,
///   "resetsAt":"2026-11-01T00:00:00Z","windowSeconds":2592000}]}`
/// `usedPercent` | `used_percent` | `utilization` (≤ 1 = fraction) or `used` + `limit`,
/// `resetsAt` | `resets_at` | `reset_at` (RFC 3339, unix secs or ms), optional `id` (default:
/// slug of label), `count`, `detail`, `group`. A top-level array of windows is accepted too.
/// A body without any window is an error (token counts alone are not a quota).
pub fn parse_custom_usage(body: &str, _now: i64) -> Result<Vec<LimitWindow>> {
    let v: Value = serde_json::from_str(body)?;
    let list = match &v {
        Value::Array(a) => a,
        Value::Object(_) => v
            .get("windows")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::other(ERR_NO_LIST))?,
        _ => return Err(Error::other(ERR_SHAPE)),
    };
    let mut out: Vec<LimitWindow> = Vec::new();
    for w in list.iter().filter_map(custom_window) {
        if !out.iter().any(|x| x.id == w.id) {
            out.push(w);
        }
    }
    if out.is_empty() {
        return Err(Error::other(ERR_NO_WINDOW));
    }
    Ok(out)
}

pub struct CustomProvider;

impl CustomProvider {
    fn fetch_inner(&self, account: &Account, ctx: &FetchContext) -> Reading {
        let now = ctx.now;
        let url = account.endpoint.trim();
        if url.is_empty() {
            return Reading::error(account, now, NOTE_NO_ENDPOINT);
        }
        if !allowed_endpoint(url) {
            return Reading::error(account, now, NOTE_BAD_ENDPOINT);
        }
        let mut req = HttpRequest::get(url)
            .header("Accept", "application/json")
            .timeout(Duration::from_secs(15));
        // Token: the variable named by `secretEnv`, then the one saved in the settings window.
        let var = account.secret_env.trim();
        match ctx.account_secret(account) {
            Some(token) => req = req.bearer(&token),
            None if !var.is_empty() => {
                return Reading::error(
                    account,
                    now,
                    tf(ctx.settings.lang(), TPL_ENV_UNSET, &[("var", var)]),
                );
            }
            None => {}
        }
        match ctx.http.send(&req) {
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
                200..=299 => match parse_custom_usage(&resp.text(), now) {
                    Ok(w) => Reading::fresh(account, now, w),
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
                },
                401 | 403 => carry_or(
                    account,
                    ctx,
                    ReadingStatus::SignedOut,
                    Some(ProviderKind::Custom.sign_in_hint().into()),
                    Some(format!("HTTP {}", resp.status)),
                ),
                429 => {
                    let until = Some(now + resp.retry_after_secs(now).unwrap_or(0).max(60));
                    carry_or(
                        account,
                        ctx,
                        ReadingStatus::RateLimited { until },
                        Some("Rate limited, trying again soon".into()),
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
        }
    }
}

impl Provider for CustomProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Custom
    }
    /// Custom endpoints are configured by hand: nothing to discover.
    fn discover(&self, _env: &Env) -> Vec<Account> {
        Vec::new()
    }
    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
        self.fetch_inner(account, ctx)
    }
}
