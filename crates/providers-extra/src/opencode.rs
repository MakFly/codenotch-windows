//! OpenCode / OpenCode Go.
//!
//! The credential is borrowed from OpenCode's data folder (`$XDG_DATA_HOME\opencode`, then
//! `~\.local\share\opencode`), read only, and its kind decides the route (each refuses the other):
//! 1. `auth.json` → `opencode-go` (`{"type":"api","key"}` or a bare string): the Go API key →
//!    `GET https://opencode.ai/zen/go/v1/usage`;
//! 2. `opencode.db` table `credential`, active `opencode-go` row first, then `opencode`
//!    (`{"type":"oauth","access","expires"(ms, number or string),"metadata":{"server","orgID"}}`) →
//!    OAuth → `GET https://opencode.ai/inference/go/v1/usage` (+ `x-opencode-org-id`);
//! 3. `auth.json` → `opencode` OAuth entry (fallback when the database cannot be read).
//!
//! Reply `{"usage":{"rolling"|"weekly"|"monthly":{"percent"(used 0–100),"resetsAt"}}}`. A 401 is
//! ambiguous (bad token or no Go plan): for OAuth it is checked against `<console>/api/user`.

use crate::util;
use codenotch_core::model::{LimitWindow, Reading, ReadingStatus, WindowKind};
use codenotch_core::provider::{Env, FetchContext};
use codenotch_core::{Account, HttpRequest};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) const API_KEY_URL: &str = "https://opencode.ai/zen/go/v1/usage";
pub(crate) const OAUTH_URL: &str = "https://opencode.ai/inference/go/v1/usage";
const DEFAULT_CONSOLE: &str = "https://opencode.ai/console";
pub(crate) const NO_GO: &str = "No OpenCode Go subscription on this account";
pub(crate) const NOTE_SIGN_IN: &str = "Run opencode auth login to sign in to OpenCode.";
pub(crate) const NOTE_EXPIRED: &str = "OpenCode's sign-in has expired — open OpenCode to renew it";
pub(crate) const NOTE_OAUTH_REJECTED: &str =
    "OpenCode rejected the sign-in — run opencode auth login";
pub(crate) const NOTE_KEY_REJECTED: &str =
    "OpenCode rejected the opencode-go key, or it has no Go plan";
pub(crate) const NOTE_NO_WINDOWS: &str = "The Go plan reported no usage windows";
pub(crate) const LABEL_ROLLING: &str = "5-hour Limit";

#[derive(Debug, PartialEq)]
pub(crate) struct Credential {
    token: String,
    pub oauth: bool,
    pub console: Option<String>,
    pub org: Option<String>,
    /// Unix seconds.
    pub expires: Option<i64>,
}

/// OpenCode's data folders (the account's `profile_path` alone when set).
pub(crate) fn data_dirs(env: &Env, account: Option<&Account>) -> Vec<PathBuf> {
    if let Some(p) = account
        .map(|a| a.profile_path.trim())
        .filter(|p| !p.is_empty())
    {
        return vec![PathBuf::from(p)];
    }
    let mut out = Vec::new();
    if let Some(x) = env.var("XDG_DATA_HOME") {
        out.push(PathBuf::from(x).join("opencode"));
    }
    let default = env.home.join(".local").join("share").join("opencode");
    if !out.contains(&default) {
        out.push(default);
    }
    out
}

pub(crate) fn installed(env: &Env, account: Option<&Account>) -> bool {
    data_dirs(env, account)
        .iter()
        .any(|d| d.join("auth.json").is_file() || d.join("opencode.db").is_file())
}

/// One stored credential in either shape; a bare string is the key itself.
pub(crate) fn credential_from(entry: &Value) -> Option<Credential> {
    if let Some(token) = util::json_str(Some(entry)) {
        return Some(Credential {
            token,
            oauth: false,
            console: None,
            org: None,
            expires: None,
        });
    }
    let obj = entry.as_object()?;
    if obj.get("type").and_then(Value::as_str) == Some("oauth") {
        let token = util::json_str(obj.get("access"))?;
        let meta = obj.get("metadata");
        let expires = obj
            .get("expires")
            .and_then(|e| {
                e.as_f64()
                    .or_else(|| e.as_str().and_then(|s| s.trim().parse().ok()))
            })
            // Always milliseconds.
            .and_then(|ms| util::epoch_secs(ms / 1000.0, f64::MAX));
        return Some(Credential {
            token,
            oauth: true,
            console: util::json_str(meta.and_then(|m| m.get("server"))),
            org: util::json_str(meta.and_then(|m| m.get("orgID"))),
            expires,
        });
    }
    let token = ["key", "apiKey", "api_key", "token", "accessToken"]
        .iter()
        .find_map(|f| util::json_str(obj.get(*f)))?;
    Some(Credential {
        token,
        oauth: false,
        console: None,
        org: None,
        expires: None,
    })
}

/// Active `opencode-go` row first, else `opencode`; newest first; other vendors' rows ignored.
pub(crate) fn credential_from_db(conn: &rusqlite::Connection) -> Option<Credential> {
    let mut stmt = conn
        .prepare(
            "SELECT integration_id, value FROM credential \
             WHERE integration_id IN ('opencode-go', 'opencode') AND COALESCE(active, 1) != 0 \
             ORDER BY time_updated DESC",
        )
        .ok()?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .ok()?
        .flatten()
        .collect();
    ["opencode-go", "opencode"].iter().find_map(|id| {
        rows.iter().filter(|(i, _)| i == id).find_map(|(_, v)| {
            serde_json::from_str::<Value>(v)
                .ok()
                .and_then(|v| credential_from(&v))
        })
    })
}

fn load_credential(dirs: &[PathBuf]) -> Option<Credential> {
    let auth = |d: &Path| util::read_json(&d.join("auth.json"));
    for d in dirs {
        if let Some(c) = auth(d).and_then(|r| r.get("opencode-go").and_then(credential_from)) {
            return Some(c);
        }
    }
    for d in dirs {
        let db = util::open_sqlite_ro(&d.join("opencode.db"), "SELECT 1 FROM credential LIMIT 1");
        if let Some(c) = db.and_then(|conn| credential_from_db(&conn)) {
            return Some(c);
        }
    }
    for d in dirs {
        let entry = auth(d).and_then(|r| r.get("opencode").cloned());
        if let Some(c) = entry
            .filter(|e| e.get("type").and_then(Value::as_str) == Some("oauth"))
            .and_then(|e| credential_from(&e))
        {
            return Some(c);
        }
    }
    None
}

pub(crate) fn usage_url(c: &Credential) -> &'static str {
    if c.oauth {
        OAUTH_URL
    } else {
        API_KEY_URL
    }
}

fn request(url: &str, c: &Credential) -> HttpRequest {
    let mut req = HttpRequest::get(url)
        .bearer(&c.token)
        .header("Accept", "application/json")
        .header("User-Agent", util::USER_AGENT)
        .timeout(util::HTTP_TIMEOUT);
    if let Some(org) = &c.org {
        req = req.header("x-opencode-org-id", org.as_str());
    }
    req
}

/// The Go plan's windows, in headline order.
pub(crate) fn windows_from(v: &Value) -> Vec<LimitWindow> {
    let Some(usage) = v.get("usage") else {
        return Vec::new();
    };
    [
        ("rolling", LABEL_ROLLING, WindowKind::Session),
        ("weekly", "Weekly limit", WindowKind::Weekly),
        ("monthly", "Monthly limit", WindowKind::Monthly),
    ]
    .into_iter()
    .filter_map(|(id, label, kind)| {
        let entry = usage.get(id)?;
        let pct = util::json_f64(entry.get("percent"))?;
        let mut w = LimitWindow::new(id, label, kind).used(pct);
        w.resets_at = util::json_rfc3339(entry.get("resetsAt"));
        w.window_seconds = match id {
            "rolling" => Some(util::FIVE_HOURS),
            "weekly" => Some(util::SEVEN_DAYS),
            _ => w.resets_at.and_then(util::calendar_month_secs),
        };
        Some(w)
    })
    .collect()
}

pub(crate) fn fetch(account: &Account, ctx: &FetchContext) -> Reading {
    let dirs = data_dirs(&ctx.env, Some(account));
    let key_from_env =
        util::account_secret(account, &ctx.env, ctx.secrets.as_deref()).map(|token| Credential {
            token,
            oauth: false,
            console: None,
            org: None,
            expires: None,
        });
    if key_from_env.is_none() && !installed(&ctx.env, Some(account)) {
        return Reading::not_installed(account, ctx.now);
    }
    let Some(cred) = key_from_env.or_else(|| load_credential(&dirs)) else {
        return Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, NOTE_SIGN_IN);
    };
    // An expired OAuth token is still sent: OpenCode refreshes it whenever it runs, and only the
    // endpoint can tell whether this copy is still good.
    let resp = match ctx.http.send(&request(usage_url(&cred), &cred)) {
        Ok(r) => r,
        Err(e) => return util::failed(account, ctx, &util::live_failed(ctx, &e.to_string())),
    };
    match resp.status {
        403 => return Reading::with_status(account, ctx.now, ReadingStatus::Empty, NO_GO),
        401 => {
            if cred.oauth && signed_in(ctx, &cred) {
                return Reading::with_status(account, ctx.now, ReadingStatus::Empty, NO_GO);
            }
            let expired = cred.expires.is_some_and(|e| e <= ctx.now);
            let note = if expired {
                NOTE_EXPIRED
            } else if cred.oauth {
                NOTE_OAUTH_REJECTED
            } else {
                NOTE_KEY_REJECTED
            };
            return Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, note);
        }
        429 => {
            let (wait, n) = util::exponential_backoff(ctx, util::retry_after(&resp, ctx.now), true);
            return util::rate_limited(account, ctx, wait, n, &util::retrying_note(ctx, wait));
        }
        s if !resp.is_success() => return util::failed(account, ctx, &util::live_http(ctx, s)),
        _ => {}
    }
    let windows = resp
        .json::<Value>()
        .map(|v| windows_from(&v))
        .unwrap_or_default();
    if windows.is_empty() {
        return util::failed(account, ctx, NOTE_NO_WINDOWS);
    }
    let mut r = Reading::fresh(account, ctx.now, windows);
    r.plan = Some("Go".into());
    r.note = Some("Go · via OpenCode".into());
    r
}

/// After an OAuth 401: does the console still accept the token? (200 = signed in, no Go plan.)
fn signed_in(ctx: &FetchContext, cred: &Credential) -> bool {
    let console = cred
        .console
        .as_deref()
        .unwrap_or(DEFAULT_CONSOLE)
        .trim_end_matches('/');
    if !console.starts_with("https://") {
        return false;
    }
    matches!(ctx.http.send(&request(&format!("{console}/api/user"), cred)), Ok(r) if r.status == 200)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::ProviderKind;
    use codenotch_core::FakeHttp;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn go_windows_in_headline_order() {
        let ws = windows_from(&json!({"usage":{
            "monthly":{"percent":40,"resetsAt":"2026-10-03T13:09:45.611Z"},
            "rolling":{"percent":12.5,"resetsAt":"2026-09-06T12:31:06.611Z"},
            "weekly":{"percent":0,"resetsAt":"2026-09-07T00:00:00Z"}}}));
        assert_eq!(
            ws.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
            ["rolling", "weekly", "monthly"]
        );
        assert_eq!(ws[0].used_percent, Some(12.5));
        assert_eq!(ws[1].used_percent, Some(0.0), "0 % is a reading");
        assert_eq!(ws[0].resets_at, Some(1_788_697_866));
        assert_eq!(
            ws.iter().map(|w| w.window_seconds).collect::<Vec<_>>(),
            [Some(18_000), Some(604_800), Some(30 * 86_400)]
        );
        let ws = windows_from(&json!({"usage":{"monthly":{"percent":1,"resetsAt":null}}}));
        assert_eq!(ws[0].window_seconds, None);
        assert!(windows_from(&json!({"type":"error"})).is_empty());
        assert!(
            windows_from(&json!({"usage":{"rolling":{"resetsAt":"2026-09-06T12:31:06Z"}}}))
                .is_empty()
        );
    }

    #[test]
    fn credential_shapes_and_routes() {
        let c = credential_from(
            &json!({"type":"oauth","access":"st_abc","expires":1792854570201u64,
            "metadata":{"server":"https://opencode.ai/console","orgID":"org_1"}}),
        )
        .unwrap();
        assert!(c.oauth);
        assert_eq!(c.org.as_deref(), Some("org_1"));
        assert_eq!(c.expires, Some(1_792_854_570));
        assert_eq!(usage_url(&c), OAUTH_URL);
        assert_eq!(
            credential_from(&json!({"type":"oauth","access":"x","expires":"1792877970039"}))
                .unwrap()
                .expires,
            Some(1_792_877_970)
        );
        let k = credential_from(&json!({"type":"api","key":"sk-go"})).unwrap();
        assert!(!k.oauth);
        assert_eq!(usage_url(&k), API_KEY_URL);
        assert_eq!(credential_from(&json!("sk-bare")).unwrap().token, "sk-bare");
        assert!(credential_from(&json!({"type":"oauth","access":""})).is_none());
    }

    fn db_with(path: &Path, rows: &[(&str, &str, Option<i64>, i64)]) {
        let c = rusqlite::Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE credential (id text PRIMARY KEY, integration_id text, label text NOT NULL,
             value text NOT NULL, connector_id text, method_id text, active integer,
             time_created integer NOT NULL, time_updated integer NOT NULL)",
        )
        .unwrap();
        for (i, (integration, value, active, updated)) in rows.iter().enumerate() {
            c.execute(
                "INSERT INTO credential VALUES (?1, ?2, 'x', ?3, NULL, NULL, ?4, 0, ?5)",
                rusqlite::params![format!("c{i}"), integration, value, active, updated],
            )
            .unwrap();
        }
    }

    #[test]
    fn db_rows_are_picked_carefully() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.db");
        db_with(
            &p,
            &[
                ("openrouter", r#"{"type":"api","key":"sk-or"}"#, None, 5),
                (
                    "opencode",
                    r#"{"type":"oauth","access":"st_old"}"#,
                    Some(0),
                    9,
                ),
                (
                    "opencode",
                    r#"{"type":"oauth","access":"st_live","metadata":{"orgID":"org_1"}}"#,
                    Some(1),
                    3,
                ),
            ],
        );
        let conn = util::open_sqlite_ro(&p, "SELECT 1 FROM credential LIMIT 1").unwrap();
        let c = credential_from_db(&conn).unwrap();
        assert_eq!(c.token, "st_live");
        assert_eq!(c.org.as_deref(), Some("org_1"));
        let p2 = dir.path().join("b.db");
        db_with(
            &p2,
            &[
                ("opencode", r#"{"type":"oauth","access":"st"}"#, Some(1), 9),
                ("opencode-go", r#"{"type":"api","key":"sk-go"}"#, None, 1),
            ],
        );
        let conn = util::open_sqlite_ro(&p2, "SELECT 1 FROM credential LIMIT 1").unwrap();
        assert_eq!(credential_from_db(&conn).unwrap().token, "sk-go");
        let p3 = dir.path().join("c.db");
        db_with(
            &p3,
            &[("openrouter", r#"{"type":"api","key":"sk-or"}"#, None, 5)],
        );
        let conn = util::open_sqlite_ro(&p3, "SELECT 1 FROM credential LIMIT 1").unwrap();
        assert_eq!(credential_from_db(&conn), None);
    }

    fn setup() -> (tempfile::TempDir, Env, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let data = env.home.join(".local").join("share").join("opencode");
        std::fs::create_dir_all(&data).unwrap();
        (dir, env, data)
    }

    #[test]
    fn fetch_routes_and_statuses() {
        let (_d, env, data) = setup();
        let account = Account::new(ProviderKind::OpenCode, "OpenCode");
        assert_eq!(
            fetch(
                &account,
                &FetchContext::new(0, Arc::new(FakeHttp::new()), env.clone())
            )
            .status,
            ReadingStatus::NotInstalled
        );

        std::fs::write(
            data.join("auth.json"),
            r#"{"anthropic":{"type":"api","key":"x"}}"#,
        )
        .unwrap();
        assert!(installed(&env, None));
        assert_eq!(
            fetch(
                &account,
                &FetchContext::new(0, Arc::new(FakeHttp::new()), env.clone())
            )
            .status,
            ReadingStatus::SignedOut
        );

        std::fs::write(
            data.join("auth.json"),
            r#"{"opencode-go":{"type":"api","key":"sk-go"}}"#,
        )
        .unwrap();
        let body = r#"{"usage":{"rolling":{"percent":33,"resetsAt":"2026-09-06T12:31:06Z"},"weekly":{"percent":10}}}"#;
        let http = Arc::new(FakeHttp::new().on(API_KEY_URL, 200, body));
        let r = fetch(&account, &FetchContext::new(0, http.clone(), env.clone()));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.headline_percent(), Some(33.0));
        assert_eq!(r.weekly().unwrap().id, "weekly");
        assert_eq!(
            http.requests()[0].header_value("authorization"),
            Some("Bearer sk-go")
        );

        let http = Arc::new(FakeHttp::new().on(API_KEY_URL, 403, ""));
        assert_eq!(
            fetch(&account, &FetchContext::new(0, http, env.clone())).status,
            ReadingStatus::Empty
        );
        let http = Arc::new(FakeHttp::new().on(API_KEY_URL, 401, ""));
        let r = fetch(&account, &FetchContext::new(0, http, env.clone()));
        assert_eq!(r.status, ReadingStatus::SignedOut);
        assert!(r.note.unwrap().contains("opencode-go key"));
        let http = Arc::new(FakeHttp::new().on(API_KEY_URL, 200, r#"{"usage":{}}"#));
        assert_eq!(
            fetch(&account, &FetchContext::new(0, http, env.clone())).status,
            ReadingStatus::Error
        );
        let http = Arc::new(FakeHttp::new().on(API_KEY_URL, 429, ""));
        assert_eq!(
            fetch(&account, &FetchContext::new(100, http, env)).status,
            ReadingStatus::RateLimited { until: Some(160) }
        );
    }

    #[test]
    fn french_rate_limit_note_keeps_the_429_marker() {
        let (_d, env, data) = setup();
        let account = Account::new(ProviderKind::OpenCode, "OpenCode");
        std::fs::write(
            data.join("auth.json"),
            r#"{"opencode-go":{"type":"api","key":"sk-go"}}"#,
        )
        .unwrap();
        let http = Arc::new(FakeHttp::new().on(API_KEY_URL, 429, ""));
        let r = fetch(&account, &FetchContext::new(100, http, env.clone()));
        assert_eq!(
            r.note.as_deref(),
            Some("Rate limited — retrying in 60s"),
            "English unchanged"
        );
        let http = Arc::new(FakeHttp::new().on(API_KEY_URL, 429, ""));
        let mut ctx = FetchContext::new(100, http, env.clone());
        ctx.settings.language = "fr".into();
        let first = fetch(&account, &ctx);
        assert_eq!(
            first.note.as_deref(),
            Some("Limité — nouvel essai dans 60 s")
        );
        assert_eq!(
            first.error.as_deref(),
            Some("rate limited (429 x1)"),
            "marker never translated"
        );
        // The marker is read back: the second 429 in a row waits twice as long.
        let http = Arc::new(FakeHttp::new().on(API_KEY_URL, 429, ""));
        let mut ctx = FetchContext::new(200, http, env.clone());
        ctx.settings.language = "fr".into();
        ctx.previous = Some(first);
        let second = fetch(&account, &ctx);
        assert_eq!(
            second.note.as_deref(),
            Some("Limité — nouvel essai dans 120 s")
        );
        assert_eq!(second.error.as_deref(), Some("rate limited (429 x2)"));
        let http = Arc::new(FakeHttp::new().on(API_KEY_URL, 500, ""));
        let mut ctx = FetchContext::new(100, http, env);
        ctx.settings.language = "fr".into();
        assert_eq!(
            fetch(&account, &ctx).error.as_deref(),
            Some("Échec de la lecture en direct (HTTP 500)")
        );
    }

    #[test]
    fn oauth_401_is_checked_against_the_console() {
        let (_d, env, data) = setup();
        let account = Account::new(ProviderKind::OpenCode, "OpenCode");
        std::fs::write(
            data.join("auth.json"),
            r#"{"opencode":{"type":"oauth","access":"st_x","expires":4000,"metadata":{"server":"https://console.example/","orgID":"org_9"}}}"#,
        )
        .unwrap();
        let http = Arc::new(FakeHttp::new().on(OAUTH_URL, 401, "").on(
            "https://console.example/api/user",
            200,
            "{}",
        ));
        let r = fetch(&account, &FetchContext::new(5, http.clone(), env.clone()));
        assert_eq!(r.status, ReadingStatus::Empty);
        assert_eq!(
            http.requests()[0].header_value("x-opencode-org-id"),
            Some("org_9")
        );
        let http = Arc::new(FakeHttp::new().on(OAUTH_URL, 401, "").on(
            "https://console.example/api/user",
            401,
            "",
        ));
        let r = fetch(&account, &FetchContext::new(5, http, env));
        assert_eq!(r.status, ReadingStatus::SignedOut);
        assert!(r.note.unwrap().contains("expired"));
    }
}
