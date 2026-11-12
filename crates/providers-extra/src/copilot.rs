//! GitHub Copilot.
//!
//! Token, first found: the account's `secretEnv` variable, `GH_TOKEN`, `GITHUB_TOKEN`,
//! `oauth_token` in the `github.com:` block of GitHub CLI's `hosts.yml` (`GH_CONFIG_DIR`,
//! `XDG_CONFIG_HOME\gh`, `%APPDATA%\GitHub CLI`, `~\.config\gh`), then `gh auth token --hostname
//! github.com` run hidden (10 s) — the CLI keeps the token in Credential Manager when it can, and
//! only the CLI itself hands it over. `GET https://api.github.com/copilot_internal/user`:
//! `{copilot_plan, quota_reset_date, quota_snapshots:{premium_interactions, chat, completions, …:
//!   {entitlement, remaining, used, unlimited, reset_date|reset_at|resets_at}}}`.
//! An unlimited quota or one without entitlement is not a limit: it draws nothing.

use crate::util;
use codenotch_core::model::{LimitWindow, Reading, ReadingStatus, WindowKind};
use codenotch_core::provider::{Env, FetchContext, LocalIdentity};
use codenotch_core::secrets::SecretStore;
use codenotch_core::{Account, HttpRequest};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

pub(crate) const ENDPOINT: &str = "https://api.github.com/copilot_internal/user";
const GH_TIMEOUT: Duration = Duration::from_secs(10);
const ORDER: [&str; 3] = ["premium_interactions", "chat", "completions"];
pub(crate) const NOTE_SIGN_IN: &str =
    "Run gh auth login — CodeNotch borrows the GitHub CLI's session.";
pub(crate) const NOTE_NO_SPAWN: &str = "GitHub CLI token read skipped (CODENOTCH_NO_SPAWN=1)";
pub(crate) const NOTE_REJECTED: &str = "GitHub session was rejected — run gh auth login again";
pub(crate) const NOTE_RATE_LIMITED: &str = "GitHub is rate limiting; the last reading stands";
pub(crate) const NOTE_BAD_JSON: &str = "GitHub answered with unreadable JSON";
pub(crate) const NOTE_NO_SNAPSHOTS: &str = "GitHub Copilot returned no quota snapshots";
pub(crate) const NOTE_NOTHING_METERED: &str = "GitHub Copilot reported no metered quotas";

/// Where `gh` keeps `hosts.yml`, in `gh`'s own order.
fn config_dirs(env: &Env) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = env.var("GH_CONFIG_DIR") {
        v.push(PathBuf::from(d));
    }
    if let Some(d) = env.var("XDG_CONFIG_HOME") {
        v.push(PathBuf::from(d).join("gh"));
    }
    v.push(env.appdata.join("GitHub CLI"));
    v.push(env.home.join(".config").join("gh"));
    v
}

fn hosts_file(env: &Env) -> Option<PathBuf> {
    config_dirs(env)
        .into_iter()
        .map(|d| d.join("hosts.yml"))
        .find(|p| p.is_file())
}

/// `gh.exe` (never a `.cmd` wrapper): Program Files, per-user install, winget links, scoop, PATH.
pub(crate) fn find_gh(env: &Env) -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(d) = env.var(var) {
            c.push(PathBuf::from(d).join("GitHub CLI").join("gh.exe"));
        }
    }
    c.push(
        env.local_appdata
            .join("Programs")
            .join("GitHub CLI")
            .join("gh.exe"),
    );
    c.push(
        env.local_appdata
            .join("Microsoft")
            .join("WinGet")
            .join("Links")
            .join("gh.exe"),
    );
    c.push(env.home.join("scoop").join("shims").join("gh.exe"));
    c.extend(util::path_dirs(env).into_iter().map(|d| d.join("gh.exe")));
    c.into_iter().find(|p| p.is_file())
}

fn env_token(
    account: Option<&Account>,
    env: &Env,
    store: Option<&dyn SecretStore>,
) -> Option<String> {
    account
        .and_then(|a| util::account_secret(a, env, store))
        .or_else(|| env.var("GH_TOKEN"))
        .or_else(|| env.var("GITHUB_TOKEN"))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

pub(crate) fn discover(env: &Env) -> bool {
    env_token(None, env, None).is_some() || hosts_file(env).is_some() || find_gh(env).is_some()
}

/// `key: value` on one YAML line, quotes stripped.
fn yaml_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let v = line
        .strip_prefix(key)?
        .strip_prefix(':')?
        .trim()
        .trim_matches(|c| c == '"' || c == '\'');
    (!v.is_empty()).then_some(v)
}

/// (`user`, `oauth_token`) of the `github.com:` block only (an Enterprise host's token is never
/// sent to api.github.com). Every indented line of the block is read, nested `users:` included.
pub(crate) fn parse_hosts(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if !lines.any(|l| l.trim_end() == "github.com:" || l.trim() == "\"github.com\":") {
        return (None, None);
    }
    let (mut user, mut token) = (None, None);
    for line in lines {
        if !line.starts_with(' ') && !line.starts_with('\t') {
            if line.trim().is_empty() {
                continue;
            }
            break;
        }
        let t = line.trim();
        if let Some(v) = yaml_value(t, "user") {
            user.get_or_insert_with(|| v.to_string());
        }
        if let Some(v) = yaml_value(t, "oauth_token") {
            token.get_or_insert_with(|| v.to_string());
        }
    }
    (user, token)
}

pub(crate) struct Creds {
    token: String,
    pub username: Option<String>,
    pub source: &'static str,
}

/// Token order with injectable inputs: env, then the file, then the CLI (called only when needed).
pub(crate) fn pick(
    env_tok: Option<String>,
    hosts: Option<&str>,
    cli: impl FnOnce() -> Option<String>,
) -> Option<Creds> {
    let (username, file_token) = hosts.map(parse_hosts).unwrap_or((None, None));
    if let Some(token) = env_tok {
        return Some(Creds {
            token,
            username,
            source: "GitHub",
        });
    }
    if let Some(token) = file_token {
        return Some(Creds {
            token,
            username,
            source: "GitHub CLI",
        });
    }
    let token = cli()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())?;
    Some(Creds {
        token,
        username,
        source: "GitHub CLI",
    })
}

fn gh_token(env: &Env) -> Option<String> {
    let gh = find_gh(env)?;
    let out = crate::process::run_hidden(
        env,
        &gh,
        &["auth", "token", "--hostname", "github.com"],
        None,
        GH_TIMEOUT,
        16 * 1024,
    )
    .map_err(|e| log::debug!("copilot: gh auth token failed: {e}"))
    .ok()?;
    if !out.success {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // One token on one line; anything else is not something to send.
    (!text.is_empty() && !text.contains(char::is_whitespace)).then_some(text)
}

/// Seconds or milliseconds (> 1e10 = ms), RFC 3339, or a bare `YYYY-MM-DD` (UTC midnight).
/// 0 is "not stated".
pub(crate) fn date_secs(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    if let Some(n) = v.as_f64() {
        return util::epoch_secs(n, 1e10);
    }
    let s = v.as_str()?.trim();
    if let Some(t) = util::rfc3339_secs(s) {
        return Some(t);
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc().timestamp())
}

/// Copilot's own names; anything new is the wire id made readable.
pub(crate) fn label(id: &str) -> String {
    match id {
        "premium_interactions" => "Premium requests".into(),
        "chat" => "Chat requests".into(),
        "completions" => "Completions".into(),
        _ => id
            .split('_')
            .filter(|w| !w.is_empty())
            .map(util::capitalize)
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// A calendar month, but only for a reset at exactly 00:00:00 UTC on the 1st.
fn monthly_len(reset: Option<i64>) -> Option<i64> {
    use chrono::{Datelike, Timelike};
    let r = reset?;
    let d = chrono::DateTime::<chrono::Utc>::from_timestamp(r, 0)?;
    (d.day() == 1 && d.hour() == 0 && d.minute() == 0 && d.second() == 0)
        .then(|| util::calendar_month_secs(r))
        .flatten()
}

fn window(id: &str, quota: &Value, root: &Value) -> Option<LimitWindow> {
    if quota.get("unlimited").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let entitlement = util::json_f64(quota.get("entitlement")).filter(|e| *e > 0.0)?;
    let used = match util::json_f64(quota.get("used")) {
        Some(u) => u,
        None => entitlement - util::json_f64(quota.get("remaining"))?,
    }
    .max(0.0);
    let resets_at = ["reset_date", "reset_at", "resets_at"]
        .iter()
        .find_map(|k| date_secs(quota.get(*k)))
        .or_else(|| date_secs(root.get("quota_reset_date")));
    let mut w = LimitWindow::new(id, label(id), WindowKind::Monthly)
        .used(used / entitlement * 100.0)
        .detail(format!("{} / {}", used.round(), entitlement.round()));
    w.resets_at = resets_at;
    w.window_seconds = monthly_len(resets_at);
    Some(w)
}

/// Reply → windows (premium, chat, completions, then the rest alphabetically), or why none.
pub(crate) fn parse(v: &Value) -> Result<Vec<LimitWindow>, String> {
    let Some(quotas) = v.get("quota_snapshots").and_then(Value::as_object) else {
        return Err(NOTE_NO_SNAPSHOTS.into());
    };
    let mut keys: Vec<&str> = ORDER
        .iter()
        .copied()
        .filter(|k| quotas.contains_key(*k))
        .collect();
    let mut rest: Vec<&str> = quotas
        .keys()
        .map(String::as_str)
        .filter(|k| !ORDER.contains(k))
        .collect();
    rest.sort_unstable();
    keys.extend(rest);
    let out: Vec<LimitWindow> = keys
        .iter()
        .filter_map(|k| window(k, &quotas[*k], v))
        .collect();
    if out.is_empty() {
        return Err(NOTE_NOTHING_METERED.into());
    }
    Ok(out)
}

/// `copilot_plan` made readable. GitHub still calls Copilot Pro `individual` on this endpoint;
/// other values (`free`, `business`, `enterprise` …) are shown as sent.
pub(crate) fn plan(v: &Value) -> Option<String> {
    util::json_str(v.get("copilot_plan").or_else(|| v.get("plan"))).map(|p| {
        if p.eq_ignore_ascii_case("individual") {
            "Pro".to_string()
        } else {
            util::capitalize(&p.replace('_', " "))
        }
    })
}

/// The GitHub login of `gh`'s `github.com` block (no token is read into the result). The plan
/// is only known from the usage reply.
pub(crate) fn local_identity(env: &Env) -> LocalIdentity {
    LocalIdentity {
        identity: hosts_file(env)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| parse_hosts(&t).0),
        plan: None,
    }
}

pub(crate) fn fetch(account: &Account, ctx: &FetchContext) -> Reading {
    let env = &ctx.env;
    let hosts = hosts_file(env).and_then(|p| std::fs::read_to_string(p).ok());
    let env_tok = env_token(Some(account), env, ctx.secrets.as_deref());
    if env_tok.is_none() && hosts.is_none() && find_gh(env).is_none() {
        return Reading::not_installed(account, ctx.now);
    }
    let mut cli_skipped = false;
    let creds = pick(env_tok, hosts.as_deref(), || {
        cli_skipped = util::spawn_disabled(env) && find_gh(env).is_some();
        gh_token(env)
    });
    let Some(creds) = creds else {
        if cli_skipped {
            return util::failed(account, ctx, NOTE_NO_SPAWN);
        }
        return Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, NOTE_SIGN_IN);
    };
    let req = HttpRequest::get(ENDPOINT)
        .bearer(&creds.token)
        .header("Accept", "application/json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "Codenotch")
        .timeout(util::HTTP_TIMEOUT);
    let resp = match ctx.http.send(&req) {
        Ok(r) => r,
        Err(e) => return util::failed(account, ctx, &e.to_string()),
    };
    match resp.status {
        401 | 403 => {
            return Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, NOTE_REJECTED)
        }
        429 => {
            let wait = util::retry_after(&resp, ctx.now).unwrap_or(300).max(60);
            return util::rate_limited(account, ctx, wait, 1, NOTE_RATE_LIMITED);
        }
        s if !resp.is_success() => return util::failed(account, ctx, &format!("HTTP {s}")),
        _ => {}
    }
    let Ok(v) = resp.json::<Value>() else {
        return util::failed(account, ctx, NOTE_BAD_JSON);
    };
    let plan = plan(&v);
    let mut reading = match parse(&v) {
        Ok(windows) => {
            let mut r = Reading::fresh(account, ctx.now, windows);
            let mut parts: Vec<String> = Vec::new();
            parts.extend(creds.username.clone());
            parts.extend(plan.clone());
            parts.push(format!("via {}", creds.source));
            r.note = Some(parts.join(" · "));
            r
        }
        Err(note) => Reading::with_status(account, ctx.now, ReadingStatus::Empty, note),
    };
    reading.identity = creds.username;
    reading.plan = plan;
    reading
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::ProviderKind;
    use codenotch_core::FakeHttp;
    use serde_json::json;
    use std::sync::Arc;

    const REPLY: &str = r#"{"copilot_plan":"individual","quota_reset_date":"2026-10-01T00:00:00Z",
        "quota_snapshots":{
          "chat":{"entitlement":50,"remaining":48,"used":2,"unlimited":false},
          "completions":{"entitlement":2000,"remaining":1990,"used":10,"unlimited":false},
          "premium_interactions":{"entitlement":300,"remaining":294,"used":6,"unlimited":false}}}"#;

    #[test]
    fn premium_first_with_monthly_reset() {
        let v: Value = serde_json::from_str(REPLY).unwrap();
        let w = parse(&v).unwrap();
        assert_eq!(
            w.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
            ["premium_interactions", "chat", "completions"]
        );
        assert_eq!(w[0].label_key, "Premium requests");
        assert!((w[0].used_percent.unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(w[0].resets_at, Some(1_790_812_800));
        assert!(w.iter().all(|w| w.window_seconds == Some(30 * 86_400)));
        assert_eq!(w[0].detail.as_deref(), Some("6 / 300"));
        assert_eq!(plan(&v).as_deref(), Some("Pro"));
        assert_eq!(
            plan(&json!({"copilot_plan":"business"})).as_deref(),
            Some("Business")
        );
    }

    #[test]
    fn used_from_remaining_and_overspend_clamped() {
        let w = parse(&json!({"quota_snapshots":{"premium_interactions":{"entitlement":300,"remaining":150,"reset_date":1790812800}}}))
            .unwrap();
        assert_eq!(w[0].used_percent, Some(50.0));
        assert_eq!(w[0].resets_at, Some(1_790_812_800));
        let w = parse(&json!({"copilot_plan":"business","quota_reset_date":"2026-10-01","quota_snapshots":{
            "chat":{"entitlement":0,"remaining":0,"unlimited":true,"quota_reset_at":0},
            "premium_interactions":{"entitlement":20000,"remaining":-4321,"unlimited":false,"quota_reset_at":0}}}))
        .unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].used_percent, Some(100.0));
        assert_eq!(w[0].resets_at, Some(1_790_812_800));
        // Neither used nor remaining: no figure is invented.
        assert!(parse(&json!({"quota_snapshots":{"chat":{"entitlement":50}}})).is_err());
    }

    #[test]
    fn unlimited_and_zero_entitlement_are_empty() {
        let err = parse(&json!({"quota_snapshots":{
            "chat":{"entitlement":0,"used":0,"unlimited":false},
            "completions":{"entitlement":0,"used":0,"unlimited":true}}}))
        .unwrap_err();
        assert!(err.contains("no metered quotas"));
        assert!(parse(&json!({})).is_err());
    }

    #[test]
    fn dates_in_every_shape() {
        assert_eq!(date_secs(Some(&json!(0))), None);
        assert_eq!(
            date_secs(Some(&json!(1_790_812_800_000u64))),
            Some(1_790_812_800)
        );
        assert_eq!(date_secs(Some(&json!("2026-10-01"))), Some(1_790_812_800));
        assert_eq!(date_secs(Some(&json!("garbage"))), None);
        assert_eq!(monthly_len(date_secs(Some(&json!("2026-10-02")))), None);
        assert_eq!(
            monthly_len(date_secs(Some(&json!("2026-10-01T01:00:00+01:00")))),
            Some(30 * 86_400)
        );
        assert_eq!(
            monthly_len(date_secs(Some(&json!("2024-03-01")))),
            Some(29 * 86_400)
        );
    }

    #[test]
    fn new_quota_ids_are_humanised_and_sorted_last() {
        let w = parse(
            &json!({"quota_snapshots":{"agent_mode_requests":{"entitlement":10,"used":1},
            "premium_interactions":{"entitlement":300,"used":6}}}),
        )
        .unwrap();
        assert_eq!(w[1].id, "agent_mode_requests");
        assert_eq!(w[1].label_key, "Agent Mode Requests");
    }

    const HOSTS: &str =
        "github.com:\n    user: octocat\n    oauth_token: cli-token\n    git_protocol: https\n";

    #[test]
    fn token_order() {
        let c = pick(Some("env-token".into()), Some(HOSTS), || {
            panic!("gh must not run")
        })
        .unwrap();
        assert_eq!(
            (c.token.as_str(), c.username.as_deref(), c.source),
            ("env-token", Some("octocat"), "GitHub")
        );
        let c = pick(None, Some(HOSTS), || panic!("gh must not run")).unwrap();
        assert_eq!((c.token.as_str(), c.source), ("cli-token", "GitHub CLI"));
        let vault = "github.com:\n    users:\n        octocat:\n    git_protocol: https\n    user: octocat\n";
        let c = pick(None, Some(vault), || Some("from-gh\n".into())).unwrap();
        assert_eq!(c.token, "from-gh");
        assert!(pick(None, Some(vault), || None).is_none());
        assert!(pick(None, None, || None).is_none());
    }

    #[test]
    fn only_the_github_com_block() {
        let hosts = "ghe.example.com:\n    user: alice\n    oauth_token: enterprise\ngithub.com:\n    user: bob\n    oauth_token: public\n";
        assert_eq!(
            parse_hosts(hosts),
            (Some("bob".into()), Some("public".into()))
        );
        assert_eq!(
            parse_hosts("ghe.example.com:\n    oauth_token: enterprise\n"),
            (None, None)
        );
        assert_eq!(
            parse_hosts("github.com:\n  user: \"octocat\"\n  oauth_token: 'tok'\n"),
            (Some("octocat".into()), Some("tok".into()))
        );
    }

    #[test]
    fn fetch_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let account = Account::new(ProviderKind::Copilot, "Copilot");
        let ctx = FetchContext::new(5, Arc::new(FakeHttp::new()), env.clone());
        assert!(!discover(&env));
        assert_eq!(fetch(&account, &ctx).status, ReadingStatus::NotInstalled);

        let gh = env.appdata.join("GitHub CLI");
        std::fs::create_dir_all(&gh).unwrap();
        std::fs::write(gh.join("hosts.yml"), "github.com:\n    user: octocat\n").unwrap();
        assert!(discover(&env));
        let r = fetch(&account, &ctx);
        assert_eq!(
            r.status,
            ReadingStatus::SignedOut,
            "no token and no gh: signed out"
        );

        std::fs::write(gh.join("hosts.yml"), HOSTS).unwrap();
        let http = Arc::new(FakeHttp::new().on(ENDPOINT, 200, REPLY));
        let r = fetch(&account, &FetchContext::new(5, http.clone(), env.clone()));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.headline().unwrap().id, "premium_interactions");
        assert_eq!(r.note.as_deref(), Some("octocat · Pro · via GitHub CLI"));
        assert_eq!(
            local_identity(&env),
            LocalIdentity {
                identity: Some("octocat".into()),
                plan: None
            }
        );
        assert_eq!(
            http.requests()[0].header_value("authorization"),
            Some("Bearer cli-token")
        );
        assert_eq!(
            http.requests()[0].header_value("x-github-api-version"),
            Some("2022-11-28")
        );

        let http = Arc::new(FakeHttp::new().on(
            ENDPOINT,
            200,
            r#"{"quota_snapshots":{"chat":{"unlimited":true}}}"#,
        ));
        let r = fetch(&account, &FetchContext::new(5, http, env.clone()));
        assert_eq!(r.status, ReadingStatus::Empty);
        assert_eq!(r.headline_percent(), None, "unlimited is never 0 %");

        let http = Arc::new(FakeHttp::new().on(ENDPOINT, 403, ""));
        assert_eq!(
            fetch(&account, &FetchContext::new(5, http, env.clone())).status,
            ReadingStatus::SignedOut
        );
        let http = Arc::new(FakeHttp::new().on(ENDPOINT, 429, ""));
        assert!(matches!(
            fetch(&account, &FetchContext::new(5, http, env)).status,
            ReadingStatus::RateLimited { until: Some(305) }
        ));
    }
}
