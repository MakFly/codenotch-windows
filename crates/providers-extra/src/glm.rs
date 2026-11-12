//! GLM / z.ai coding plan (data-spec §5.5).
//!
//! Key sources, first found (all read only):
//! 0. the account's `secretEnv` variable, 1. `GLM_API_KEY` (both: the z.ai console),
//! 2. `glm.json` `{api_key|apiKey, base_url|baseURL}` in our data folder (or the 0.x
//!    `%APPDATA%\codenotch\` one),
//! 3. Claude Code `~\.claude\settings.json` `env.ANTHROPIC_AUTH_TOKEN` — only when
//!    `env.ANTHROPIC_BASE_URL` points at a z.ai / bigmodel.cn host,
//! 4. ZCode `~\.zcode\v2\config.json`: an enabled `*coding-plan*` provider's `options.apiKey`,
//! 5. ZCode `~\.zcode\v2\credentials.json`: **plaintext values only**. Values sealed as
//!    `enc:v1:` are skipped, never decrypted (another product's credential store),
//! 6. OpenCode `auth.json` (`~\.local\share\opencode`, `%APPDATA%\opencode`).
//!
//! `GET {console}/api/monitor/usage/quota/limit` with the **raw** key as `Authorization` (no
//! Bearer). Errors ride inside HTTP 200 envelopes `{code, success, data:{level, limits:[…]}}`.
//! Windows are identified by length (`unit` 3 = hours, 6 = weeks; `TIME_LIMIT` = monthly MCP).

use crate::i18n::tf;
use crate::util;
use codenotch_core::model::{LimitWindow, Reading, ReadingStatus, WindowKind};
use codenotch_core::provider::{Env, FetchContext};
use codenotch_core::secrets::SecretStore;
use codenotch_core::{Account, HttpRequest};
use serde_json::Value;
use std::path::PathBuf;

const ZAI: &str = "https://api.z.ai";
const BIGMODEL: &str = "https://open.bigmodel.cn";
pub(crate) const NOTE_SEALED: &str =
    "ZCode keeps its z.ai key encrypted, which CodeNotch does not read: set GLM_API_KEY or add glm.json";
pub(crate) const NOTE_NO_KEY: &str =
    "No usable Z.ai key found (ZCode, OpenCode or a glm.json key file)";
pub(crate) const NOTE_REJECTED: &str = "Z.ai rejected the key — renew it in the tool that holds it";
pub(crate) const NOTE_BAD_JSON: &str = "Z.ai answered with unreadable JSON";
pub(crate) const NOTE_NO_WINDOWS: &str = "The plan reported no usage windows";
pub(crate) const LABEL_WEEKLY: &str = "Weekly";
pub(crate) const LABEL_MCP: &str = "MCP (1 month)";
pub(crate) const LABEL_USAGE: &str = "Usage";
/// Template translated at fetch time.
pub(crate) const TPL_REFUSED: &str = "Z.ai monitor refused the request ({code})";

#[derive(Debug)]
pub(crate) struct Credential {
    token: String,
    pub base: &'static str,
    pub source: &'static str,
}

fn is_zai_host(host: &str) -> bool {
    host == "api.z.ai"
        || host.ends_with(".z.ai")
        || host == "z.ai"
        || host == "open.bigmodel.cn"
        || host.ends_with(".bigmodel.cn")
}

fn console_for_host(host: &str) -> &'static str {
    if host.ends_with("bigmodel.cn") {
        BIGMODEL
    } else {
        ZAI
    }
}

/// Host of a URL (or a bare host), without port.
pub(crate) fn host_of(url: &str) -> Option<String> {
    let s = url.trim();
    let rest = s.split_once("://").map(|(_, r)| r).unwrap_or(s);
    let host = rest
        .split(['/', '?', '#'])
        .next()?
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

fn json_files(env: &Env) -> Vec<PathBuf> {
    vec![
        util::data_dir(env).join("glm.json"),
        env.appdata.join("codenotch").join("glm.json"),
    ]
}

fn zcode_dir(env: &Env) -> PathBuf {
    env.home.join(".zcode").join("v2")
}

fn opencode_auth_files(env: &Env) -> Vec<PathBuf> {
    vec![
        env.home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("auth.json"),
        env.appdata.join("opencode").join("auth.json"),
    ]
}

fn manual_key(env: &Env) -> Option<Credential> {
    json_files(env).iter().find_map(|p| {
        let root = util::read_json(p)?;
        let token = util::json_str(root.get("api_key").or_else(|| root.get("apiKey")))?;
        let base = util::json_str(root.get("base_url").or_else(|| root.get("baseURL")))
            .and_then(|u| host_of(&u))
            .filter(|h| is_zai_host(h))
            .map_or(ZAI, |h| console_for_host(&h));
        Some(Credential {
            token,
            base,
            source: "glm.json",
        })
    })
}

/// Claude Code pointed at z.ai: the base URL is what makes the token a GLM key.
pub(crate) fn claude_code_key(env: &Env) -> Option<Credential> {
    let root = util::read_json(&env.home.join(".claude").join("settings.json"))?;
    let e = root.get("env")?;
    let token = util::json_str(e.get("ANTHROPIC_AUTH_TOKEN"))
        .or_else(|| util::json_str(e.get("ANTHROPIC_API_KEY")))?;
    let host = util::json_str(e.get("ANTHROPIC_BASE_URL")).and_then(|u| host_of(&u))?;
    is_zai_host(&host).then(|| Credential {
        token,
        base: console_for_host(&host),
        source: "Claude Code",
    })
}

pub(crate) fn zcode_plan_key(env: &Env) -> Option<Credential> {
    let root = util::read_json(&zcode_dir(env).join("config.json"))?;
    let providers = root.get("provider")?.as_object()?;
    let mut ids: Vec<&String> = providers
        .keys()
        .filter(|k| k.contains("coding-plan"))
        .collect();
    ids.sort();
    ids.into_iter().find_map(|id| {
        let p = providers[id].as_object()?;
        if p.get("enabled").and_then(Value::as_bool) == Some(false) {
            return None;
        }
        let opts = p.get("options")?;
        let token = util::json_str(opts.get("apiKey")).filter(|t| !t.starts_with("enc:"))?;
        let base = util::json_str(opts.get("baseURL"))
            .and_then(|u| host_of(&u))
            .filter(|h| is_zai_host(h))
            .map_or(ZAI, |h| console_for_host(&h));
        Some(Credential {
            token,
            base,
            source: "ZCode",
        })
    })
}

fn is_account_api_key(key: &str) -> bool {
    key.starts_with("account-provider:")
        && key.ends_with(":api-key")
        && key.len() > "account-provider:".len() + ":api-key".len()
}

/// ZCode credentials file: plaintext values only (sealed `enc:` values are not decrypted).
/// Returns (credential, whether a sealed value was seen).
pub(crate) fn zcode_credentials(env: &Env) -> (Option<Credential>, bool) {
    let Some(root) = util::read_json(&zcode_dir(env).join("credentials.json")) else {
        return (None, false);
    };
    let Some(obj) = root.as_object() else {
        return (None, false);
    };
    // Candidates in preference order: per-account plan keys (sorted), then the sign-in tokens.
    let mut ids: Vec<&String> = obj.keys().filter(|k| is_account_api_key(k)).collect();
    ids.sort();
    let mut candidates: Vec<(&str, &'static str)> = ids
        .into_iter()
        .map(|id| {
            (
                id.as_str(),
                if id.to_ascii_lowercase().contains("bigmodel") {
                    BIGMODEL
                } else {
                    ZAI
                },
            )
        })
        .collect();
    candidates.push(("oauth:zai:access_token", ZAI));
    candidates.push(("oauth:bigmodel:access_token", BIGMODEL));
    let mut sealed = false;
    for (key, base) in candidates {
        let Some(raw) = util::json_str(obj.get(key)) else {
            continue;
        };
        if raw.starts_with("enc:") {
            sealed = true;
            continue;
        }
        return (
            Some(Credential {
                token: raw,
                base,
                source: "ZCode",
            }),
            sealed,
        );
    }
    (None, sealed)
}

pub(crate) fn opencode_key(env: &Env) -> Option<Credential> {
    const IDS: [&str; 7] = [
        "zai-coding-plan",
        "zai",
        "z-ai",
        "z.ai",
        "glm",
        "zhipu",
        "zhipuai",
    ];
    for p in opencode_auth_files(env) {
        let Some(root) = util::read_json(&p) else {
            continue;
        };
        for id in IDS {
            let Some(entry) = root.get(id) else { continue };
            let token = util::json_str(Some(entry)).or_else(|| {
                [
                    "apiKey",
                    "api_key",
                    "token",
                    "key",
                    "accessToken",
                    "auth_token",
                ]
                .iter()
                .find_map(|f| util::json_str(entry.get(*f)))
            });
            if let Some(token) = token {
                let base = if id.starts_with("zhipu") {
                    BIGMODEL
                } else {
                    ZAI
                };
                return Some(Credential {
                    token,
                    base,
                    source: "OpenCode",
                });
            }
        }
    }
    None
}

/// (credential, a ZCode sealed value was skipped)
fn load_credential(
    env: &Env,
    account: Option<&Account>,
    store: Option<&dyn SecretStore>,
) -> (Option<Credential>, bool) {
    if let Some(token) = account.and_then(|a| util::account_secret(a, env, store)) {
        return (
            Some(Credential {
                token,
                base: ZAI,
                source: "secretEnv",
            }),
            false,
        );
    }
    if let Some(token) = env
        .var("GLM_API_KEY")
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    {
        return (
            Some(Credential {
                token,
                base: ZAI,
                source: "GLM_API_KEY",
            }),
            false,
        );
    }
    if let Some(c) = manual_key(env)
        .or_else(|| claude_code_key(env))
        .or_else(|| zcode_plan_key(env))
    {
        return (Some(c), false);
    }
    let (z, sealed) = zcode_credentials(env);
    if let Some(c) = z {
        return (Some(c), sealed);
    }
    (opencode_key(env), sealed)
}

/// Any key source on this machine (file checks only).
pub(crate) fn present(env: &Env) -> bool {
    env.var("GLM_API_KEY").is_some()
        || json_files(env).iter().any(|p| p.is_file())
        || claude_code_key(env).is_some()
        || zcode_dir(env).join("config.json").is_file()
        || zcode_dir(env).join("credentials.json").is_file()
        || opencode_key(env).is_some()
}

fn window_id(l: &Value) -> String {
    let ty = l.get("type").and_then(Value::as_str).unwrap_or("");
    if ty == "TIME_LIMIT" {
        return "mcp".into();
    }
    match (
        l.get("unit").and_then(Value::as_i64),
        l.get("number").and_then(Value::as_i64),
    ) {
        (Some(3), Some(5)) => "session".into(),
        (Some(6), Some(1)) => "weekly".into(),
        (Some(u), Some(n)) => format!("window-{u}x{n}"),
        _ if !ty.is_empty() => ty.to_ascii_lowercase(),
        _ => "unknown".into(),
    }
}

fn label_and_kind(id: &str, unit: Option<i64>, number: Option<i64>) -> (String, WindowKind) {
    match id {
        "session" => ("Current session".into(), WindowKind::Session),
        "weekly" => (LABEL_WEEKLY.into(), WindowKind::Weekly),
        "mcp" => (LABEL_MCP.into(), WindowKind::Monthly),
        // `label_key` is the canonical English label (history series key): never localised here.
        _ => match (unit, number) {
            (Some(3), Some(n)) => (format!("Usage ({n} h)"), WindowKind::Other),
            (Some(6), Some(n)) => (format!("Usage ({n} wk)"), WindowKind::Other),
            _ => (LABEL_USAGE.into(), WindowKind::Other),
        },
    }
}

fn length(unit: Option<i64>, number: Option<i64>) -> Option<i64> {
    let per = match unit? {
        3 => 3600,
        6 => util::SEVEN_DAYS,
        _ => return None,
    };
    per.checked_mul(number?).filter(|s| *s > 0)
}

fn rank(id: &str) -> u8 {
    match id {
        "session" => 0,
        "weekly" => 1,
        "mcp" => 2,
        _ => 3,
    }
}

/// `data.limits` → windows; a row without a percentage is dropped, one without a reset kept.
pub(crate) fn windows_from(v: &Value) -> Vec<LimitWindow> {
    let Some(limits) = v.pointer("/data/limits").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<LimitWindow> = limits
        .iter()
        .filter_map(|l| {
            let pct = util::json_f64(l.get("percentage"))?;
            let id = window_id(l);
            let (unit, number) = (
                l.get("unit").and_then(Value::as_i64),
                l.get("number").and_then(Value::as_i64),
            );
            let (label, kind) = label_and_kind(&id, unit, number);
            let mut w = LimitWindow::new(id, label, kind).used(pct);
            w.resets_at =
                util::json_f64(l.get("nextResetTime")).and_then(|ms| util::epoch_secs(ms, 1e11));
            w.window_seconds = length(unit, number);
            Some(w)
        })
        .collect();
    out.sort_by(|a, b| rank(&a.id).cmp(&rank(&b.id)).then_with(|| a.id.cmp(&b.id)));
    out
}

pub(crate) fn fetch(account: &Account, ctx: &FetchContext) -> Reading {
    let env = &ctx.env;
    let (cred, sealed) = load_credential(env, Some(account), ctx.secrets.as_deref());
    let Some(cred) = cred else {
        if !present(env) {
            return Reading::not_installed(account, ctx.now);
        }
        let note = if sealed { NOTE_SEALED } else { NOTE_NO_KEY };
        return Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, note);
    };
    let req = HttpRequest::get(format!("{}/api/monitor/usage/quota/limit", cred.base))
        .header("Authorization", cred.token.as_str())
        .header("Accept", "application/json")
        .header("User-Agent", util::USER_AGENT)
        .timeout(util::HTTP_TIMEOUT);
    let resp = match ctx.http.send(&req) {
        Ok(r) => r,
        Err(e) => return util::failed(account, ctx, &util::live_failed(ctx, &e.to_string())),
    };
    let rejected =
        || Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, NOTE_REJECTED);
    match resp.status {
        401 | 403 => return rejected(),
        429 => {
            let (wait, n) = util::exponential_backoff(ctx, util::retry_after(&resp, ctx.now), true);
            return util::rate_limited(account, ctx, wait, n, &util::retrying_note(ctx, wait));
        }
        s if !resp.is_success() => return util::failed(account, ctx, &util::live_http(ctx, s)),
        _ => {}
    }
    let Ok(v) = resp.json::<Value>() else {
        return util::failed(account, ctx, NOTE_BAD_JSON);
    };
    // The envelope first: errors arrive inside HTTP 200. Only the code is kept, never the prose.
    let code = v.get("code").and_then(Value::as_i64).unwrap_or(0);
    let success = v.get("success").and_then(Value::as_bool).unwrap_or(false);
    if code == 401 || code == 403 {
        return rejected();
    }
    if !success && code != 200 {
        return util::failed(
            account,
            ctx,
            &tf(
                ctx.settings.lang(),
                TPL_REFUSED,
                &[("code", &code.to_string())],
            ),
        );
    }
    let windows = windows_from(&v);
    if windows.is_empty() {
        return util::failed(account, ctx, NOTE_NO_WINDOWS);
    }
    let level = v
        .pointer("/data/level")
        .and_then(Value::as_str)
        .map(util::capitalize)
        .filter(|l| !l.is_empty());
    let mut r = Reading::fresh(account, ctx.now, windows);
    r.note = Some(match &level {
        Some(l) => format!("{l} · via {}", cred.source),
        None => format!("via {}", cred.source),
    });
    r.plan = level;
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::ProviderKind;
    use codenotch_core::FakeHttp;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn hosts_and_consoles() {
        assert_eq!(
            host_of("https://api.z.ai/api/anthropic").as_deref(),
            Some("api.z.ai")
        );
        assert_eq!(host_of("api.z.ai:10443").as_deref(), Some("api.z.ai"));
        assert_eq!(
            host_of("https://open.bigmodel.cn/api").as_deref(),
            Some("open.bigmodel.cn")
        );
        assert!(is_zai_host("api.z.ai"));
        assert!(!is_zai_host("api.anthropic.com"));
        assert!(!is_zai_host("evilz.ai.example.com"));
        assert_eq!(console_for_host("open.bigmodel.cn"), BIGMODEL);
    }

    #[test]
    fn limits_by_length_not_type() {
        let ws = windows_from(&json!({"data":{"level":"pro","limits":[
            {"type":"TIME_LIMIT","unit":5,"number":1,"percentage":4},
            {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":20,"nextResetTime":1790000000000u64},
            {"type":"CREDIT_LIMIT","unit":3,"number":5,"percentage":61.5,"nextResetTime":1789000000000u64},
            {"type":"TOKENS_LIMIT","unit":3,"number":2,"percentage":1},
            {"type":"TOKENS_LIMIT","unit":3,"number":1}
        ]}}));
        assert_eq!(
            ws.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
            ["session", "weekly", "mcp", "window-3x2"]
        );
        assert_eq!(ws[0].used_percent, Some(61.5));
        assert_eq!(ws[0].resets_at, Some(1_789_000_000));
        assert_eq!(ws[0].window_seconds, Some(util::FIVE_HOURS));
        assert_eq!(ws[1].window_seconds, Some(util::SEVEN_DAYS));
        assert_eq!(ws[2].window_seconds, None);
        assert_eq!(ws[2].resets_at, None, "a row without reset is kept");
        assert_eq!(ws[3].label_key, "Usage (2 h)");
        assert_eq!(length(Some(6), Some(-1)), None);
        assert_eq!(length(Some(6), Some(i64::MAX)), None);
    }

    #[test]
    fn key_sources_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        assert!(!present(&env));
        // Claude Code aimed at Anthropic is not a GLM key.
        let claude = env.home.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(claude.join("settings.json"), r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"sk-ant","ANTHROPIC_BASE_URL":"https://api.anthropic.com"}}"#).unwrap();
        assert!(claude_code_key(&env).is_none());
        assert!(!present(&env));
        std::fs::write(claude.join("settings.json"), r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"zk","ANTHROPIC_BASE_URL":"https://open.bigmodel.cn/api/anthropic"}}"#).unwrap();
        let c = claude_code_key(&env).unwrap();
        assert_eq!((c.base, c.source), (BIGMODEL, "Claude Code"));
        // ZCode: sealed values are skipped, plaintext ones used.
        let z = zcode_dir(&env);
        std::fs::create_dir_all(&z).unwrap();
        std::fs::write(
            z.join("credentials.json"),
            r#"{"oauth:zai:access_token":"enc:v1:AAAA.BBBB.CCCC"}"#,
        )
        .unwrap();
        let (c, sealed) = zcode_credentials(&env);
        assert!(c.is_none() && sealed);
        std::fs::write(
            z.join("credentials.json"),
            r#"{"account-provider:coding-plan:bigmodel:account:u1:api-key":"plain-key"}"#,
        )
        .unwrap();
        let (c, sealed) = zcode_credentials(&env);
        let c = c.unwrap();
        assert_eq!(
            (c.token.as_str(), c.base, sealed),
            ("plain-key", BIGMODEL, false)
        );
        std::fs::write(z.join("config.json"), r#"{"provider":{"builtin:zai-coding-plan":{"enabled":true,"options":{"apiKey":"cfg-key","baseURL":"https://api.z.ai/api"}}}}"#).unwrap();
        assert_eq!(zcode_plan_key(&env).unwrap().token, "cfg-key");
        // OpenCode auth.json
        let oc = env.home.join(".local").join("share").join("opencode");
        std::fs::create_dir_all(&oc).unwrap();
        std::fs::write(
            oc.join("auth.json"),
            r#"{"zhipuai":{"type":"api","key":"oc-key"}}"#,
        )
        .unwrap();
        assert_eq!(opencode_key(&env).unwrap().base, BIGMODEL);
        // Order: env first, then glm.json, then Claude Code.
        let (c, _) = load_credential(&env, None, None);
        assert_eq!(c.unwrap().source, "Claude Code");
        let d = util::data_dir(&env);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("glm.json"),
            r#"{"apiKey":"file-key","baseURL":"https://open.bigmodel.cn"}"#,
        )
        .unwrap();
        let (c, _) = load_credential(&env, None, None);
        let c = c.unwrap();
        assert_eq!((c.token.as_str(), c.base), ("file-key", BIGMODEL));
        let env = env.with_var("GLM_API_KEY", "env-key");
        assert_eq!(
            load_credential(&env, None, None).0.unwrap().token,
            "env-key"
        );
        // A key saved in the settings window (secret store) wins over everything else.
        use codenotch_core::secrets::{MemorySecrets, SecretKey, SecretStore as _};
        let account = Account::new(ProviderKind::Glm, "z.ai");
        let store = MemorySecrets::new();
        store
            .set(&SecretKey::Account(account.id.clone()), "stored-key")
            .unwrap();
        let (c, _) = load_credential(&env, Some(&account), Some(&store));
        assert_eq!(c.unwrap().token, "stored-key");
    }

    #[test]
    fn fetch_sends_the_stored_key() {
        use codenotch_core::secrets::{MemorySecrets, SecretKey, SecretStore as _};
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let account = Account::new(ProviderKind::Glm, "z.ai");
        let store = Arc::new(MemorySecrets::new());
        store
            .set(&SecretKey::Account(account.id.clone()), "stored-key")
            .unwrap();
        let body = r#"{"code":200,"success":true,"data":{"level":"lite","limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":30}]}}"#;
        let http = Arc::new(FakeHttp::new().on(URL, 200, body));
        let mut ctx = FetchContext::new(0, http.clone(), env);
        ctx.secrets = Some(store);
        let r = fetch(&account, &ctx);
        assert_eq!(r.status, ReadingStatus::Fresh);
        let auth = http.requests()[0]
            .header_value("authorization")
            .map(str::to_string)
            .unwrap_or_default();
        assert!(auth.contains("stored-key"));
    }

    fn with_key() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path()).with_var("GLM_API_KEY", "raw-key");
        (dir, env)
    }

    const URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";

    #[test]
    fn fetch_envelope_rules() {
        let (_d, env) = with_key();
        let account = Account::new(ProviderKind::Glm, "z.ai");
        let body = r#"{"code":200,"success":true,"data":{"level":"lite","limits":[
            {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":30},{"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":12}]}}"#;
        let http = Arc::new(FakeHttp::new().on(URL, 200, body));
        let r = fetch(&account, &FetchContext::new(0, http.clone(), env.clone()));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.headline_percent(), Some(30.0));
        assert_eq!(r.weekly().unwrap().id, "weekly");
        assert_eq!(r.note.as_deref(), Some("Lite · via GLM_API_KEY"));
        assert_eq!(
            http.requests()[0].header_value("authorization"),
            Some("raw-key"),
            "no Bearer prefix"
        );

        let http = Arc::new(FakeHttp::new().on(
            URL,
            200,
            r#"{"code":401,"success":false,"msg":"token expired for x@y"}"#,
        ));
        assert_eq!(
            fetch(&account, &FetchContext::new(0, http, env.clone())).status,
            ReadingStatus::SignedOut
        );
        let http = Arc::new(FakeHttp::new().on(
            URL,
            200,
            r#"{"code":1001,"success":false,"msg":"secret detail"}"#,
        ));
        let r = fetch(&account, &FetchContext::new(0, http, env.clone()));
        assert_eq!(r.status, ReadingStatus::Error);
        assert_eq!(
            r.error.as_deref(),
            Some("Z.ai monitor refused the request (1001)")
        );
        let http = Arc::new(FakeHttp::new().on(URL, 429, ""));
        assert_eq!(
            fetch(&account, &FetchContext::new(0, http, env.clone())).status,
            ReadingStatus::RateLimited { until: Some(60) }
        );
        let http = Arc::new(FakeHttp::new().on(
            URL,
            200,
            r#"{"code":200,"success":true,"data":{"limits":[]}}"#,
        ));
        assert_eq!(
            fetch(&account, &FetchContext::new(0, http, env)).status,
            ReadingStatus::Error
        );
    }

    #[test]
    fn french_envelope_error_and_rate_limit() {
        let (_d, env) = with_key();
        let account = Account::new(ProviderKind::Glm, "z.ai");
        let fr = |http: FakeHttp| {
            let mut ctx = FetchContext::new(0, Arc::new(http), env.clone());
            ctx.settings.language = "fr".into();
            fetch(&account, &ctx)
        };
        let r = fr(FakeHttp::new().on(
            URL,
            200,
            r#"{"code":1001,"success":false,"msg":"secret detail"}"#,
        ));
        assert_eq!(
            r.error.as_deref(),
            Some("Le moniteur Z.ai a refusé la requête (1001)")
        );
        let r = fr(FakeHttp::new().on(URL, 429, ""));
        assert_eq!(r.note.as_deref(), Some("Limité — nouvel essai dans 60 s"));
        let r = fr(FakeHttp::new().on(URL, 401, ""));
        assert_eq!(
            r.note.as_deref(),
            Some(NOTE_REJECTED),
            "fixed notes stay English"
        );
    }

    #[test]
    fn sealed_only_explains_itself() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let account = Account::new(ProviderKind::Glm, "z.ai");
        assert_eq!(
            fetch(
                &account,
                &FetchContext::new(0, Arc::new(FakeHttp::new()), env.clone())
            )
            .status,
            ReadingStatus::NotInstalled
        );
        let z = zcode_dir(&env);
        std::fs::create_dir_all(&z).unwrap();
        std::fs::write(
            z.join("credentials.json"),
            r#"{"oauth:zai:access_token":"enc:v1:a.b.c"}"#,
        )
        .unwrap();
        let r = fetch(
            &account,
            &FetchContext::new(0, Arc::new(FakeHttp::new()), env),
        );
        assert_eq!(r.status, ReadingStatus::SignedOut);
        assert!(r.note.unwrap().contains("encrypted"));
    }
}
