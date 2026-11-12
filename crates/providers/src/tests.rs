//! Provider tests: synthetic fixtures only (`tempfile`, `Env::for_test`, fake HTTP).

use super::*;
use crate::util::NO_SPAWN_ENV;
use codenotch_core::http::{HttpClient, HttpError, HttpRequest, HttpResponse};
use codenotch_core::model::{LimitWindow, Reading, ReadingStatus, WindowKind};
use codenotch_core::provider::FetchContext;
use codenotch_core::{Account, FakeHttp};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const NOW: i64 = 1_792_000_000; // 2026-10-15

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn ctx_with(env: Env, http: Arc<dyn HttpClient>, now: i64) -> FetchContext {
    FetchContext::new(now, http, env)
}

fn b64url(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

fn jwt(payload: &serde_json::Value) -> String {
    format!(
        "{}.{}.sig",
        b64url(br#"{"alg":"none"}"#),
        b64url(payload.to_string().as_bytes())
    )
}

type Responder = Box<dyn FnMut(&HttpRequest) -> Result<HttpResponse, HttpError> + Send>;

/// HTTP double driven by a closure (for multi-step scenarios).
struct FnHttp {
    f: Mutex<Responder>,
    log: Mutex<Vec<HttpRequest>>,
}

impl FnHttp {
    fn new(
        f: impl FnMut(&HttpRequest) -> Result<HttpResponse, HttpError> + Send + 'static,
    ) -> Self {
        Self {
            f: Mutex::new(Box::new(f)),
            log: Mutex::new(Vec::new()),
        }
    }
}

impl HttpClient for FnHttp {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.log.lock().unwrap().push(request.clone());
        (self.f.lock().unwrap())(request)
    }
}

fn ids(windows: &[LimitWindow]) -> Vec<&str> {
    windows.iter().map(|w| w.id.as_str()).collect()
}

fn assert_no_secret(r: &Reading, secret: &str) {
    let dump = format!("{r:?}");
    assert!(!dump.contains(secret), "secret leaked into reading");
}

#[test]
fn all_kinds_are_distinct() {
    let kinds: Vec<_> = all().iter().map(|p| p.kind()).collect();
    assert_eq!(
        kinds,
        [
            ProviderKind::Claude,
            ProviderKind::Codex,
            ProviderKind::Grok,
            ProviderKind::Custom
        ]
    );
    let mins: Vec<u64> = all().iter().map(|p| p.min_interval().as_secs()).collect();
    assert_eq!(mins, [300, 300, 300, 60]);
}

#[test]
fn default_profile_dirs() {
    let env = Env::for_test(Path::new("Z:/t"));
    assert_eq!(
        default_profile_dir(&env, &ProviderKind::Claude),
        Some(PathBuf::from("Z:/t/home/.claude"))
    );
    assert_eq!(default_profile_dir(&env, &ProviderKind::Cursor), None);
}

// =============================================================================================
// Claude
// =============================================================================================

mod claude_tests {
    use super::*;
    use crate::claude::{self, parse_credential};

    const SPEC_EXAMPLE: &str = r#"{"limits":[{"kind":"session","percent":42,"resets_at":"2026-11-12T18:00:00Z"},
           {"kind":"weekly_all","percent":17,"resets_at":"2026-11-17T09:00:00Z"}],
 "five_hour":{"utilization":42,"resets_at":"2026-11-12T18:00:00Z"},
 "seven_day":{"utilization":17,"resets_at":"2026-11-17T09:00:00Z"}}"#;

    #[test]
    fn spec_example_drops_duplicate_fallbacks() {
        let w = parse_claude_usage(SPEC_EXAMPLE, NOW).unwrap();
        assert_eq!(ids(&w), ["session", "seven_day"]);
        assert_eq!(w[0].label_key, "Current session");
        assert_eq!(w[0].used_percent, Some(42.0));
        assert_eq!(w[0].kind, WindowKind::Session);
        assert_eq!(w[0].window_seconds, Some(5 * 3600));
        assert_eq!(w[0].resets_at, util::rfc3339_secs("2026-11-12T18:00:00Z"));
        assert_eq!(w[1].label_key, "Weekly limit");
        assert_eq!(w[1].used_percent, Some(17.0));
        assert_eq!(w[1].window_seconds, Some(7 * 86400));
    }

    #[test]
    fn limits_entries_need_kind_percent_and_reset() {
        let body = r#"{"limits":[{"percent":5,"resets_at":"2026-11-12T18:00:00Z"},
            {"kind":"session","resets_at":"2026-11-12T18:00:00Z"},
            {"kind":"weekly_all","percent":3},
            {"kind":"weekly_all","percent":3,"resets_at":"garbage"},
            {"kind":"mystery_thing","percent":12.5,"resets_at":"2026-11-12T18:00:00Z"}]}"#;
        let w = parse_claude_usage(body, NOW).unwrap();
        assert_eq!(ids(&w), ["mystery_thing"]);
        assert_eq!(w[0].label_key, "Mystery thing");
        assert_eq!(w[0].window_seconds, None);
    }

    #[test]
    fn rolled_over_session_comes_back_from_named_field_and_first() {
        let body = r#"{"limits":[{"kind":"weekly_all","percent":17,"resets_at":"2026-11-17T09:00:00Z"}],
            "five_hour":{"utilization":3,"resets_at":null},
            "seven_day":{"utilization":17,"resets_at":"2026-11-17T09:00:00Z"}}"#;
        let w = parse_claude_usage(body, NOW).unwrap();
        assert_eq!(ids(&w), ["session", "seven_day"]);
        assert_eq!(w[0].used_percent, Some(3.0));
        assert_eq!(w[0].resets_at, None, "unknown reset stays unknown");
    }

    #[test]
    fn same_reset_and_figure_is_a_duplicate() {
        // a kind we do not alias, but the same window by reset + percent
        let body = r#"{"limits":[{"kind":"weekly_everything","percent":17,"resets_at":"2026-11-17T09:00:00Z"}],
            "seven_day":{"utilization":17.2,"resets_at":"2026-11-17T09:00:00Z"}}"#;
        assert_eq!(
            ids(&parse_claude_usage(body, NOW).unwrap()),
            ["weekly_everything"]
        );
        let body = r#"{"limits":[{"kind":"weekly_everything","percent":17,"resets_at":"2026-11-17T09:00:00Z"}],
            "seven_day":{"utilization":30,"resets_at":"2026-11-17T09:00:00Z"}}"#;
        assert_eq!(
            ids(&parse_claude_usage(body, NOW).unwrap()),
            ["weekly_everything", "seven_day"]
        );
    }

    #[test]
    fn per_model_windows() {
        let body = r#"{"limits":[
              {"kind":"session","percent":52,"resets_at":"2026-08-28T09:50:00.316290+00:00","scope":null},
              {"kind":"weekly_scoped","percent":61,"resets_at":"2026-09-02T17:00:00.316321+00:00",
               "scope":{"model":{"display_name":"Sonnet","id":"x"}}}],
            "seven_day_sonnet":{"utilization":61,"resets_at":"2026-09-02T17:00:00Z"},
            "seven_day_opus":{"utilization":9,"resets_at":"2026-09-02T17:00:00Z"}}"#;
        let w = parse_claude_usage(body, NOW).unwrap();
        assert_eq!(ids(&w), ["session", "weekly_scoped", "seven_day_opus"]);
        assert_eq!(w[1].label_key, "Weekly limit (Sonnet)");
        assert_eq!(w[2].label_key, "Weekly limit (Opus)");
        // malformed scope keeps the reading
        let body = r#"{"limits":[{"kind":"weekly_scoped","percent":61,"resets_at":"2026-09-02T17:00:00Z","scope":"x"}],
            "seven_day_sonnet":{"utilization":4,"resets_at":"2026-09-02T17:00:00Z"},
            "seven_day_opus":null}"#;
        let w = parse_claude_usage(body, NOW).unwrap();
        assert_eq!(ids(&w), ["weekly_scoped", "seven_day_sonnet"]);
        assert_eq!(w[0].label_key, "Weekly limit (model-scoped)");
    }

    #[test]
    fn enterprise_spend_window() {
        let body = r#"{"limits":[],"five_hour":null,"seven_day":null,
          "spend":{"enabled":true,"percent":1,
            "limit":{"amount_minor":20000,"currency":"USD","exponent":2},
            "used":{"amount_minor":297,"currency":"USD","exponent":2}}}"#;
        let w = parse_claude_usage(body, NOW).unwrap();
        assert_eq!(ids(&w), ["spend"]);
        assert!(
            (w[0].used_percent.unwrap() - 1.485).abs() < 1e-9,
            "used / limit, not the rounded percent"
        );
        assert_eq!(w[0].label_key, "Spend limit");
        assert_eq!(w[0].detail.as_deref(), Some("2.97 USD / 200.00 USD"));
        assert_eq!(w[0].resets_at, None);
        // headline falls back to spend when there is no session
        let r = Reading::fresh(&Account::new(ProviderKind::Claude, "c"), NOW, w);
        assert_eq!(r.headline().unwrap().id, "spend");

        for off in [
            r#"{"spend":{"enabled":false,"limit":{"amount_minor":100},"used":{"amount_minor":1}}}"#,
            r#"{"spend":{"enabled":true,"limit":{"amount_minor":0},"used":{"amount_minor":1}}}"#,
            r#"{"spend":{"enabled":true,"used":{"amount_minor":1}}}"#,
        ] {
            assert!(parse_claude_usage(off, NOW).unwrap().is_empty(), "{off}");
        }
        // exponent default 2, clamp 0..6
        let body =
            r#"{"spend":{"limit":{"amount_minor":1000,"exponent":9},"used":{"amount_minor":10}}}"#;
        assert_eq!(
            parse_claude_usage(body, NOW).unwrap()[0].detail.as_deref(),
            Some("0.10 / 0.001000")
        );
    }

    #[test]
    fn extra_usage_is_its_own_window_never_headline() {
        let body = r#"{"limits":[],"extra_usage":{"is_enabled":true,"currency":"USD","monthly_limit":5000,
            "used_credits":1250,"utilization":25.0,"decimal_places":2}}"#;
        let w = parse_claude_usage(body, NOW).unwrap();
        assert_eq!(ids(&w), ["extra_usage"]);
        assert_eq!(w[0].used_percent, Some(25.0));
        assert_eq!(w[0].detail.as_deref(), Some("12.50 USD / 50.00 USD"));
        let r = Reading::fresh(&Account::new(ProviderKind::Claude, "c"), NOW, w);
        assert!(r.headline().is_none());
        let off = r#"{"extra_usage":{"is_enabled":false,"monthly_limit":5000,"used_credits":1}}"#;
        assert!(parse_claude_usage(off, NOW).unwrap().is_empty());
    }

    #[test]
    fn bad_bodies_are_errors() {
        assert!(parse_claude_usage("not json", NOW).is_err());
        assert!(parse_claude_usage("[1,2]", NOW).is_err());
        assert!(parse_claude_usage("{}", NOW).unwrap().is_empty());
    }

    #[test]
    fn credential_parsing_and_expiry() {
        let c = parse_credential(
            r#"{"claudeAiOauth":{"accessToken":"tok-A","expiresAt":1792000000000,"subscriptionType":"max",
                "rateLimitTier":"default_claude_max_20x"}}"#,
        )
        .unwrap();
        assert_eq!(c.expires_at_ms, Some(1_792_000_000_000));
        assert_eq!(c.plan().as_deref(), Some("Max 20x"));
        assert!(c.expired(NOW), "expiresAt <= now is expired");
        assert!(!c.expired(NOW - 1));
        assert!(!format!("{c:?}").contains("tok-A"));
        // root object, no expiry → never expired
        let c = parse_credential(r#"{"accessToken":"t","subscriptionType":"pro"}"#).unwrap();
        assert_eq!(c.expires_at_ms, None);
        assert!(!c.expired(i64::MAX / 2000));
        assert_eq!(c.plan().as_deref(), Some("Pro"));
        // empty token = signed out
        assert!(parse_credential(r#"{"claudeAiOauth":{"accessToken":"  "}}"#).is_none());
        assert!(parse_credential("garbage").is_none());
    }

    #[test]
    fn credential_file_fallback_name() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":""}}"#,
        );
        write(
            &dir.path().join("credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"second"}}"#,
        );
        assert!(claude::read_credential(dir.path()).is_some());
    }

    #[test]
    fn backoff_rule() {
        assert_eq!(claude_backoff_secs(1, None), 60);
        assert_eq!(claude_backoff_secs(2, None), 120);
        assert_eq!(claude_backoff_secs(4, None), 480);
        assert_eq!(claude_backoff_secs(5, None), 900);
        assert_eq!(claude_backoff_secs(9, None), 900);
        assert_eq!(claude_backoff_secs(0, None), 60);
        assert_eq!(claude_backoff_secs(1, Some(30)), 60);
        assert_eq!(
            claude_backoff_secs(5, Some(3600)),
            3600,
            "Retry-After may pass the cap"
        );
    }

    // ---- discovery ------------------------------------------------------------------------

    fn cred_json(token: &str, expires_ms: Option<i64>) -> String {
        let exp = expires_ms
            .map(|e| format!(r#","expiresAt":{e}"#))
            .unwrap_or_default();
        format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}"{exp},"subscriptionType":"pro"}}}}"#)
    }

    #[test]
    fn discovers_profiles_and_names_them() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path());
        assert!(ClaudeProvider.discover(&env).is_empty());
        std::fs::create_dir_all(env.home_path(".claude")).unwrap();
        let one = ClaudeProvider.discover(&env);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].name, "Claude");
        assert_eq!(
            one[0].profile_path, "",
            "default profile uses the default folder"
        );

        write(
            &env.home_path(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"me@acme.co.uk"}}"#,
        );
        write(
            &env.home_path(".claude-work/.credentials.json"),
            &cred_json("w", None),
        );
        write(
            &env.home_path(".claude-work/.claude.json"),
            r#"{"oauthAccount":{"emailAddress":"x@bigcorp.com"}}"#,
        );
        write(
            &env.home_path(".claude-alt/credentials.json"),
            &cred_json("a", None),
        );
        std::fs::create_dir_all(env.home_path(".claude-nocred")).unwrap();
        write(
            &env.home_path(".claude-bad id/.credentials.json"),
            &cred_json("b", None),
        );
        write(
            &env.home_path(".claude-default/.credentials.json"),
            &cred_json("d", None),
        );

        let profiles = claude_profiles(&env);
        let pids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(pids, ["default", "alt", "work"]);
        let accounts = ClaudeProvider.discover(&env);
        let names: Vec<&str> = accounts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Claude Acme", "Claude (alt)", "Claude Bigcorp"]);
        assert!(accounts[2].profile_path.ends_with(".claude-work"));
        assert!(accounts.iter().all(|a| a.provider == ProviderKind::Claude));
    }

    #[test]
    fn naming_collisions_and_odd_domains() {
        let p = |id: &str, email: Option<&str>| ClaudeProfile {
            id: id.into(),
            dir: PathBuf::from(id),
            email: email.map(String::from),
        };
        let names = claude_names(&[
            p("default", Some("a@gmail.com")),
            p("x", Some("b@gmail.com")),
            p("y", Some("c@123.io")),
        ]);
        assert_eq!(
            names,
            ["Claude a@gmail.com", "Claude b@gmail.com", "Claude (y)"]
        );
        assert_eq!(
            claude_names(&[p("default", None), p("x", None)]),
            ["Claude", "Claude (x)"]
        );
    }

    #[test]
    fn claude_config_dir_is_a_profile_when_it_holds_a_credential() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = root.path().join("cfg").join(".claude-perso");
        let env = Env::for_test(root.path())
            .with_var("CLAUDE_CONFIG_DIR", &elsewhere.to_string_lossy());
        std::fs::create_dir_all(env.home_path(".claude")).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        assert_eq!(claude_profiles(&env).len(), 1, "no credential: not a profile");
        write(&elsewhere.join(".credentials.json"), &cred_json("p", None));
        let accounts = ClaudeProvider.discover(&env);
        assert_eq!(
            accounts.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Claude", "Claude (perso)"]
        );
        assert_eq!(accounts[1].profile_path, elsewhere.to_string_lossy());
        // Pointing at the default folder (or a listed one) adds nothing.
        let default = env.home_path(".claude").to_string_lossy().into_owned();
        let env = env.with_var("CLAUDE_CONFIG_DIR", &default);
        assert_eq!(claude_profiles(&env).len(), 1);
    }

    #[test]
    fn local_identity_from_credential_then_claude_json() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path());
        let a = account();
        assert_eq!(
            ClaudeProvider.local_identity(&a, &env),
            codenotch_core::LocalIdentity::default()
        );
        write(
            &env.home_path(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"me@acme.test","organizationType":"claude_max",
                "organizationRateLimitTier":"default_claude_max_5x","billingType":"stripe_subscription"}}"#,
        );
        std::fs::create_dir_all(env.home_path(".claude")).unwrap();
        let local = ClaudeProvider.local_identity(&a, &env);
        assert_eq!(local.identity.as_deref(), Some("me@acme.test"));
        assert_eq!(
            local.plan.as_deref(),
            Some("Max 5x"),
            "signed out: plan from .claude.json"
        );
        write(
            &env.home_path(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"t","subscriptionType":"max","rateLimitTier":"default_claude_max_20x"}}"#,
        );
        let local = ClaudeProvider.local_identity(&a, &env);
        assert_eq!(local.plan.as_deref(), Some("Max 20x"), "the credential wins");
        assert_eq!(
            claude::meta_from_claude_json(
                r#"{"oauthAccount":{"organizationType":"claude_enterprise"}}"#
            )
            .plan
            .as_deref(),
            Some("Enterprise")
        );
        assert_eq!(claude::meta_from_claude_json("not json"), Default::default());
    }

    // ---- fetch ----------------------------------------------------------------------------

    fn setup(token: &str, expires_ms: Option<i64>) -> (tempfile::TempDir, Env) {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path()).with_var(NO_SPAWN_ENV, "1");
        write(
            &env.home_path(".claude/.credentials.json"),
            &cred_json(token, expires_ms),
        );
        write(
            &env.home_path(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"me@acme.test"}}"#,
        );
        (root, env)
    }

    fn account() -> Account {
        Account::new(ProviderKind::Claude, "Claude")
    }

    #[test]
    fn not_installed_and_signed_out() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path());
        let http = Arc::new(FakeHttp::new());
        let r = ClaudeProvider.fetch(&account(), &ctx_with(env.clone(), http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::NotInstalled);
        std::fs::create_dir_all(env.home_path(".claude")).unwrap();
        let r = ClaudeProvider.fetch(&account(), &ctx_with(env, http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::SignedOut);
        assert_eq!(r.note.as_deref(), Some(claude::NOTE_NO_CREDENTIAL));
        assert!(http.requests().is_empty());
    }

    #[test]
    fn fresh_read_sends_headers_and_fills_identity() {
        let (_root, env) = setup("tok-live-123", Some((NOW + 3600) * 1000));
        let http = Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 200, SPEC_EXAMPLE));
        let r = ClaudeProvider.fetch(&account(), &ctx_with(env, http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(ids(&r.windows), ["session", "seven_day"]);
        assert_eq!(r.fetched_at, NOW);
        assert_eq!(r.identity.as_deref(), Some("me@acme.test"));
        assert_eq!(r.plan.as_deref(), Some("Pro"));
        assert_eq!(r.headline_percent(), Some(42.0));
        let req = &http.requests()[0];
        assert_eq!(
            req.header_value("authorization"),
            Some("Bearer tok-live-123")
        );
        assert_eq!(req.header_value("anthropic-beta"), Some("oauth-2025-04-20"));
        assert_eq!(req.timeout.as_secs(), 15);
        assert_no_secret(&r, "tok-live-123");
    }

    #[test]
    fn explicit_default_path_still_reads_identity_beside_the_folder() {
        let (_root, env) = setup("t", None);
        let mut a = account();
        a.profile_path = env
            .home_path(".claude")
            .to_string_lossy()
            .replace('\\', "/")
            + "/";
        let http = Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 200, SPEC_EXAMPLE));
        let r = ClaudeProvider.fetch(&a, &ctx_with(env, http, NOW));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.identity.as_deref(), Some("me@acme.test"));
    }

    #[test]
    fn custom_profile_path_is_used() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path()).with_var(NO_SPAWN_ENV, "1");
        let dir = env.home_path(".claude-work");
        write(
            &dir.join(".credentials.json"),
            &cred_json("work-token", None),
        );
        write(
            &dir.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"w@corp.test"}}"#,
        );
        let mut a = account();
        a.profile_path = dir.to_string_lossy().into_owned();
        let http = Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 200, SPEC_EXAMPLE));
        let r = ClaudeProvider.fetch(&a, &ctx_with(env, http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.identity.as_deref(), Some("w@corp.test"));
        assert_eq!(
            http.requests()[0].header_value("authorization"),
            Some("Bearer work-token")
        );
    }

    #[test]
    fn expired_token_is_never_sent() {
        let (_root, env) = setup("old-token", Some(NOW * 1000));
        let http = Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 200, SPEC_EXAMPLE));
        let a = account();
        let r = ClaudeProvider.fetch(&a, &ctx_with(env.clone(), http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::SignedOut);
        assert_eq!(r.note.as_deref(), Some(claude::NOTE_EXPIRED));
        // with earlier windows: stale, original time kept
        let mut ctx = ctx_with(env, http.clone(), NOW);
        let prev = Reading::fresh(
            &a,
            NOW - 600,
            parse_claude_usage(SPEC_EXAMPLE, NOW).unwrap(),
        );
        ctx.previous = Some(prev);
        let r = ClaudeProvider.fetch(&a, &ctx);
        assert_eq!(r.status, ReadingStatus::Stale);
        assert_eq!(r.fetched_at, NOW - 600);
        assert_eq!(r.windows.len(), 2);
        assert!(
            http.requests().is_empty(),
            "nothing sent with an expired token"
        );
    }

    #[test]
    fn expired_without_cli_does_not_spawn_and_records_attempt() {
        // spawning allowed, but the test env has no CLI anywhere (no PATH either)
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path());
        write(
            &env.home_path(".claude/.credentials.json"),
            &cred_json("t", Some((NOW + 60) * 1000)),
        );
        assert!(find_claude_cli(&env).is_none());
        let http = Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 200, SPEC_EXAMPLE));
        let r = ClaudeProvider.fetch(&account(), &ctx_with(env, http.clone(), NOW));
        // still valid (60 s left): it is read
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(http.requests().len(), 1);
    }

    #[test]
    fn rotated_token_after_401_is_retried_once() {
        let (_root, env) = setup("first", None);
        let cred_path = env.home_path(".claude/.credentials.json");
        let http = Arc::new(FnHttp::new(move |req| {
            if req.header_value("authorization") == Some("Bearer first") {
                std::fs::write(&cred_path, cred_json("second", None)).unwrap();
                Ok(HttpResponse::new(401, "{}"))
            } else {
                Ok(HttpResponse::new(200, SPEC_EXAMPLE))
            }
        }));
        let r = ClaudeProvider.fetch(&account(), &ctx_with(env, http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(http.log.lock().unwrap().len(), 2);
    }

    // ---- Claude desktop app token cache ----------------------------------------------------

    const API: &str = "https://api.anthropic.com";

    fn cache_key(account: &str, scopes: &str) -> String {
        format!("acct:{account}|org-1:client-1:{API}:{scopes}")
    }

    #[test]
    fn token_cache_keeps_profile_scoped_entries_of_each_account() {
        let json = serde_json::json!({
            cache_key("acc-1", "user:inference user:profile user:sessions:claude_code"):
                {"token": "code", "expiresAt": 5_000, "subscriptionType": "max",
                 "rateLimitTier": "default_claude_max_20x"},
            cache_key("acc-1", "user:inference"): {"token": "inference-only", "expiresAt": 9_000},
            cache_key("acc-2", "user:profile"): {"token": "other", "expiresAt": 9_000},
            cache_key("acc-1", "user:profile"): {"token": ""},
            "not-an-entry": {"token": "x"},
        })
        .to_string();
        let mut entries = claude::parse_token_cache(&json, false);
        entries.sort_by(|a, b| a.account.cmp(&b.account));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].account, "acc-1");
        assert_eq!(entries[0].cred.source, claude::Source::DesktopCode);
        assert_eq!(entries[0].cred.plan().as_deref(), Some("Max 20x"));
        assert_eq!(entries[1].account, "acc-2");
        assert_eq!(entries[1].cred.source, claude::Source::Desktop);
        let legacy = claude::parse_token_cache(&json, true);
        assert!(legacy
            .iter()
            .all(|e| e.cred.source == claude::Source::DesktopLegacy));
        assert!(claude::parse_token_cache("not json", false).is_empty());
    }

    /// A desktop app folder whose `oauth:tokenCacheV2` holds `entries`, encrypted like Electron
    /// safeStorage with a fresh DPAPI-wrapped key.
    fn write_desktop_cache(env: &Env, last_account: &str, entries: serde_json::Value) {
        use crate::safe_storage::{base64_encode, SafeStorage};
        let dir = env.local_appdata.join("Claude-Data").join("claude_test123");
        let key = SafeStorage::from_key([5u8; 32]);
        write(&dir.join("Local State"), &key.local_state_for_test());
        let blob = key.encrypt_for_test(entries.to_string().as_bytes(), [3u8; 12]);
        write(
            &dir.join("config.json"),
            &serde_json::json!({
                "lastKnownAccountUuid": last_account,
                "oauth:tokenCacheV2": base64_encode(&blob),
            })
            .to_string(),
        );
    }

    #[test]
    fn desktop_app_token_is_used_when_the_cli_file_is_expired() {
        let (_root, env) = setup("old-cli", Some((NOW - 3600) * 1000));
        write(
            &env.home_path(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"me@acme.test","accountUuid":"acc-1"}}"#,
        );
        write_desktop_cache(
            &env,
            "acc-1",
            serde_json::json!({
                cache_key("acc-1", "user:inference user:profile user:sessions:claude_code"):
                    {"token": "desktop-code", "expiresAt": (NOW + 86_400) * 1000,
                     "subscriptionType": "max", "rateLimitTier": "default_claude_max_5x"},
                cache_key("acc-1", "user:profile"):
                    {"token": "desktop-expired", "expiresAt": (NOW - 1) * 1000},
                cache_key("acc-9", "user:profile"):
                    {"token": "someone-else", "expiresAt": (NOW + 999_999) * 1000},
            }),
        );
        let http = Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 200, SPEC_EXAMPLE));
        let r = ClaudeProvider.fetch(&account(), &ctx_with(env, http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.plan.as_deref(), Some("Max 5x"));
        assert_eq!(r.identity.as_deref(), Some("me@acme.test"));
        let reqs = http.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(
            reqs[0].header_value("authorization"),
            Some("Bearer desktop-code")
        );
        assert!(!format!("{r:?}").contains("desktop-code"));
    }

    #[test]
    fn desktop_tokens_of_another_account_are_never_used() {
        let (_root, env) = setup("old-cli", Some((NOW - 3600) * 1000));
        write(
            &env.home_path(".claude.json"),
            r#"{"oauthAccount":{"accountUuid":"acc-1"}}"#,
        );
        write_desktop_cache(
            &env,
            "acc-2",
            serde_json::json!({
                cache_key("acc-2", "user:profile"):
                    {"token": "other", "expiresAt": (NOW + 86_400) * 1000},
            }),
        );
        let http = Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 200, SPEC_EXAMPLE));
        let r = ClaudeProvider.fetch(&account(), &ctx_with(env, http.clone(), NOW));
        assert_eq!(r.note.as_deref(), Some(claude::NOTE_EXPIRED));
        assert!(http.requests().is_empty());
    }

    #[test]
    fn rejected_desktop_token_falls_back_to_the_cli_file() {
        // CLI file blank (as the desktop app leaves it), default profile without accountUuid:
        // the app's last account is used; a 401 moves on to the next store.
        let (_root, env) = setup("cli-valid", Some((NOW + 600) * 1000));
        write_desktop_cache(
            &env,
            "acc-1",
            serde_json::json!({
                cache_key("acc-1", "user:profile"):
                    {"token": "desktop-revoked", "expiresAt": (NOW + 86_400) * 1000},
            }),
        );
        let http = Arc::new(FnHttp::new(|req| {
            if req.header_value("authorization") == Some("Bearer desktop-revoked") {
                Ok(HttpResponse::new(401, "{}"))
            } else {
                Ok(HttpResponse::new(200, SPEC_EXAMPLE))
            }
        }));
        let r = ClaudeProvider.fetch(&account(), &ctx_with(env, http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::Fresh);
        let log = http.log.lock().unwrap();
        assert_eq!(log.len(), 2, "freshest first, then the CLI file");
        assert_eq!(
            log[1].header_value("authorization"),
            Some("Bearer cli-valid")
        );
    }

    #[test]
    fn ranking_prefers_fresh_tokens_and_drops_expired_ones() {
        let entries = claude::parse_token_cache(
            &serde_json::json!({
                cache_key("a", "user:profile"): {"token": "late", "expiresAt": 9_000_000},
                cache_key("b", "user:profile"): {"token": "early", "expiresAt": 8_000_000},
                cache_key("c", "user:profile"): {"token": "gone", "expiresAt": 1_000},
            })
            .to_string(),
            false,
        );
        let mut legacy = claude::parse_token_cache(
            &serde_json::json!({
                cache_key("d", "user:profile"): {"token": "legacy", "expiresAt": 99_000_000},
            })
            .to_string(),
            true,
        );
        let mut all: Vec<_> = entries.into_iter().map(|e| e.cred).collect();
        all.push(legacy.remove(0).cred);
        let ranked = claude::rank_credentials(all, 5);
        let order: Vec<_> = ranked.iter().map(|c| c.expires_at_ms).collect();
        assert_eq!(
            order,
            [Some(9_000_000), Some(8_000_000), Some(99_000_000)],
            "legacy cache last, expired dropped"
        );
    }

    #[test]
    fn unchanged_token_401_is_signed_out_with_windows_kept() {
        let (_root, env) = setup("same", None);
        let http = Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 401, "{}"));
        let a = account();
        let mut ctx = ctx_with(env, http.clone(), NOW);
        ctx.previous = Some(Reading::fresh(
            &a,
            NOW - 300,
            parse_claude_usage(SPEC_EXAMPLE, NOW).unwrap(),
        ));
        let r = ClaudeProvider.fetch(&a, &ctx);
        assert_eq!(r.status, ReadingStatus::SignedOut);
        assert_eq!(r.note.as_deref(), Some(claude::NOTE_REJECTED));
        assert_eq!(r.windows.len(), 2);
        assert_eq!(r.headline_percent(), None, "signed out shows no figure");
        assert_eq!(
            http.requests().len(),
            1,
            "no retry when the token did not change"
        );
    }

    #[test]
    fn status_403_is_not_signed_out() {
        let (_root, env) = setup("t", None);
        let http = Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 403, "{}"));
        let r = ClaudeProvider.fetch(&account(), &ctx_with(env, http, NOW));
        assert_eq!(r.status, ReadingStatus::Error);
        assert_eq!(r.note.as_deref(), Some(claude::NOTE_403));
    }

    #[test]
    fn rate_limit_honours_retry_after() {
        let (_root, env) = setup("t", None);
        let a = account();
        let cases = [
            (
                HttpResponse::new(429, "").with_header("Retry-After", "120"),
                NOW + 120,
            ),
            (HttpResponse::new(429, ""), NOW + 60),
            (
                HttpResponse::new(429, "").with_header("Retry-After", "5"),
                NOW + 60,
            ),
            (
                HttpResponse::new(429, "").with_header(
                    "Retry-After",
                    &chrono::DateTime::from_timestamp(NOW + 1800, 0)
                        .unwrap()
                        .to_rfc2822(),
                ),
                NOW + 1800,
            ),
        ];
        for (resp, until) in cases {
            let http = Arc::new(FakeHttp::new().on_response(CLAUDE_USAGE_URL, resp));
            let mut ctx = ctx_with(env.clone(), http, NOW);
            ctx.previous = Some(Reading::fresh(
                &a,
                NOW - 300,
                parse_claude_usage(SPEC_EXAMPLE, NOW).unwrap(),
            ));
            let r = ClaudeProvider.fetch(&a, &ctx);
            assert_eq!(r.status, ReadingStatus::RateLimited { until: Some(until) });
            assert_eq!(r.fetched_at, NOW - 300, "429 keeps the old reading's time");
            assert_eq!(r.windows.len(), 2);
            assert!(r.shows_figures());
        }
    }

    #[test]
    fn other_failures_keep_windows_or_error() {
        let (_root, env) = setup("t", None);
        let a = account();
        let http = Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 503, "busy"));
        let r = ClaudeProvider.fetch(&a, &ctx_with(env.clone(), http, NOW));
        assert_eq!(r.status, ReadingStatus::Error);
        assert_eq!(r.error.as_deref(), Some("HTTP 503"));
        assert!(r.windows.is_empty());
        let http = Arc::new(FakeHttp::new().on_error(CLAUDE_USAGE_URL, HttpError::Timeout));
        let mut ctx = ctx_with(env.clone(), http, NOW);
        ctx.previous = Some(Reading::fresh(
            &a,
            NOW - 60,
            parse_claude_usage(SPEC_EXAMPLE, NOW).unwrap(),
        ));
        let r = ClaudeProvider.fetch(&a, &ctx);
        assert_eq!(r.status, ReadingStatus::Error);
        assert_eq!(r.windows.len(), 2);
        assert_eq!(r.fetched_at, NOW - 60);
        // previous of another account is never carried
        let mut ctx = ctx_with(
            env.clone(),
            Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 500, "")),
            NOW,
        );
        ctx.previous = Some(Reading::fresh(
            &account(),
            NOW - 60,
            parse_claude_usage(SPEC_EXAMPLE, NOW).unwrap(),
        ));
        assert!(ClaudeProvider.fetch(&a, &ctx).windows.is_empty());
        // unparsable 200 and empty 200
        let http = Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 200, "<html>"));
        let r = ClaudeProvider.fetch(&a, &ctx_with(env.clone(), http, NOW));
        assert_eq!(r.status, ReadingStatus::Error);
        assert!(r.error.unwrap().starts_with("parse:"));
        let http = Arc::new(FakeHttp::new().on(CLAUDE_USAGE_URL, 200, "{}"));
        let r = ClaudeProvider.fetch(&a, &ctx_with(env, http, NOW));
        assert_eq!(r.status, ReadingStatus::Empty);
    }

    #[test]
    fn cli_lookup_order_and_desktop_exclusion() {
        let root = tempfile::tempdir().unwrap();
        let desktop = root.path().join("AnthropicClaude").join("app");
        write(&desktop.join("claude.exe"), "");
        let path_dir = root.path().join("tools");
        write(&path_dir.join("claude.cmd"), "");
        let path = std::env::join_paths([&desktop, &path_dir]).unwrap();
        let env = Env::for_test(root.path()).with_var("PATH", path.to_str().unwrap());
        assert_eq!(find_claude_cli(&env), Some(path_dir.join("claude.cmd")));
        write(&env.appdata.join("npm").join("claude.cmd"), "");
        assert_eq!(
            find_claude_cli(&env),
            Some(env.appdata.join("npm").join("claude.cmd"))
        );
        write(&env.home.join(".local").join("bin").join("claude.exe"), "");
        assert_eq!(
            find_claude_cli(&env),
            Some(env.home.join(".local").join("bin").join("claude.exe"))
        );
        assert!(claude::is_desktop_owned(Path::new(
            "C:/Program Files/WindowsApps/x/claude.exe"
        )));
    }

    #[test]
    fn add_account_creates_next_folder_and_failed_sign_in_removes_it() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path());
        std::fs::create_dir_all(env.home_path(".claude-2")).unwrap();
        let a = add_claude_account(&env).unwrap();
        let dir = env.home_path(".claude-3");
        assert_eq!(PathBuf::from(&a.profile_path), dir);
        assert_eq!(a.provider, ProviderKind::Claude);
        assert!(dir.is_dir());
        assert!(claude::is_pending(&dir));
        // no CLI in the test env: sign-in fails and the empty folder goes away
        let err = ClaudeProvider.sign_in(&a, &env).unwrap_err();
        assert!(err.to_string().contains("CLI not found"));
        assert!(!dir.exists());
        assert!(!claude::is_pending(&dir));
        assert!(!claude_auth_state().busy);
        // spawning disabled
        let env = env.with_var(NO_SPAWN_ENV, "1");
        let a = add_claude_account(&env).unwrap();
        assert!(ClaudeProvider.sign_in(&a, &env).is_err());
        assert!(!PathBuf::from(&a.profile_path).exists());
    }
}

// =============================================================================================
// Codex
// =============================================================================================

mod codex_tests {
    use super::*;
    use crate::codex::{self, parse_app_server_limits, parse_rollout_line};
    use serde_json::json;

    const USAGE: &str = r#"{"plan_type":"plus",
 "rate_limit":{"primary_window":{"used_percent":37.0,"limit_window_seconds":18000,"reset_at":1792000000,"reset_after_seconds":5400},
               "secondary_window":{"used_percent":12.0,"limit_window_seconds":604800,"reset_at":1792400000}},
 "additional_rate_limits":[{"limit_name":"other","rate_limit":{"primary_window":{"used_percent":99}}},
    {"limit_name":"GPT-5-Codex-Spark","rate_limit":{"primary_window":{"used_percent":5,"limit_window_seconds":18000,"reset_after_seconds":60},"secondary_window":null}}],
 "code_review_rate_limit":{"primary_window":{"used_percent":1,"limit_window_seconds":604800,"reset_at":1792400000},"secondary_window":null}}"#;

    #[test]
    fn parses_all_window_families() {
        let w = parse_codex_usage(USAGE, NOW).unwrap();
        assert_eq!(ids(&w), ["primary", "secondary", "spark", "code-review"]);
        assert_eq!(w[0].label_key, "Current session");
        assert_eq!(w[0].kind, WindowKind::Session);
        assert_eq!(w[0].used_percent, Some(37.0));
        assert_eq!(
            w[0].resets_at,
            Some(1_792_000_000),
            "reset_at wins over reset_after_seconds"
        );
        assert_eq!(w[0].window_seconds, Some(18000));
        assert_eq!(w[1].label_key, "Weekly limit");
        assert_eq!(w[1].kind, WindowKind::Weekly);
        assert_eq!(w[2].group.as_deref(), Some("Spark"));
        assert_eq!(w[2].resets_at, Some(NOW + 60));
        assert_eq!(w[3].group.as_deref(), Some("Code review"));
        assert_eq!(w[3].label_key, "Weekly limit");
    }

    #[test]
    fn tolerant_window_parsing() {
        let body = r#"{"rate_limit":{"primary_window":{"limit_window_seconds":18000},
            "secondary_window":{"used_percent":3,"reset_at":-4}},"additional_rate_limits":{"x":1}}"#;
        let w = parse_codex_usage(body, NOW).unwrap();
        assert_eq!(
            ids(&w),
            ["secondary"],
            "a window without used_percent is skipped"
        );
        assert_eq!(w[0].resets_at, None, "negative reset is missing");
        assert_eq!(w[0].label_key, "Longer window");
        assert!(parse_codex_usage("{}", NOW).unwrap().is_empty());
        assert!(parse_codex_usage("nope", NOW).is_err());
        // metered_feature also names Spark
        let body = r#"{"additional_rate_limits":[{"metered_feature":"codex_spark","rate_limit":{"primary_window":{"used_percent":2}}}]}"#;
        assert_eq!(ids(&parse_codex_usage(body, NOW).unwrap()), ["spark"]);
    }

    #[test]
    fn auth_identity_reads_only_the_id_token_payload() {
        let id = jwt(&json!({"email":"dev@example.test",
            "https://api.openai.com/auth":{"chatgpt_plan_type":"pro"}}));
        let body = json!({"tokens":{"id_token":id,"access_token":"opaque-s3cret"}}).to_string();
        let local = codex::auth_identity(&body);
        assert_eq!(local.identity.as_deref(), Some("dev@example.test"));
        assert_eq!(local.plan.as_deref(), Some("Pro"));
        assert!(!format!("{local:?}").contains("s3cret"));
        // No account id or access token needed; the access token's claim is a fallback.
        let body = json!({"tokens":{"access_token":jwt(&json!({
            "https://api.openai.com/auth":{"chatgpt_plan_type":"team"}}))}})
        .to_string();
        assert_eq!(codex::auth_identity(&body).plan.as_deref(), Some("Team"));
        assert_eq!(codex::auth_identity("{}"), Default::default());
    }

    #[test]
    fn extra_profiles_with_an_auth_file_are_discovered() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("cx-home");
        let env = Env::for_test(root.path()).with_var("CODEX_HOME", &home.to_string_lossy());
        assert!(CodexProvider.discover(&env).is_empty());
        write(&env.home_path(".codex-work/auth.json"), "{}");
        std::fs::create_dir_all(env.home_path(".codex-empty")).unwrap();
        write(&env.home_path(".codex-bad id/auth.json"), "{}");
        let one = CodexProvider.discover(&env);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].name, "Codex", "a single profile keeps the plain name");
        assert!(one[0].profile_path.ends_with(".codex-work"));

        write(&env.home_path(".codex/auth.json"), "{}");
        write(&home.join("auth.json"), "{}");
        let all = CodexProvider.discover(&env);
        assert_eq!(
            all.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Codex", "Codex (work)", "Codex (cx-home)"]
        );
        assert_eq!(all[0].profile_path, "");
        assert_eq!(all[2].profile_path, home.to_string_lossy());
        // CODEX_HOME = the default folder adds nothing.
        let default = env.home_path(".codex").to_string_lossy().into_owned();
        let env = env.with_var("CODEX_HOME", &default);
        assert_eq!(CodexProvider.discover(&env).len(), 2);
    }

    #[test]
    fn labels_follow_window_length() {
        let lbl = |secs: i64| {
            let body = format!(
                r#"{{"rate_limit":{{"secondary_window":{{"used_percent":1,"limit_window_seconds":{secs}}}}}}}"#
            );
            parse_codex_usage(&body, NOW).unwrap()[0].label_key.clone()
        };
        assert_eq!(lbl(1800), "30m limit");
        assert_eq!(lbl(3600), "1h limit");
        assert_eq!(lbl(18000), "Current session");
        assert_eq!(lbl(30 * 86400), "Monthly limit");
        assert_eq!(lbl(14 * 86400), "14d limit");
        // a weekly-only primary (free plan) is labelled weekly
        let body =
            r#"{"rate_limit":{"primary_window":{"used_percent":1,"limit_window_seconds":604800}}}"#;
        let w = parse_codex_usage(body, NOW).unwrap();
        assert_eq!(
            (w[0].id.as_str(), w[0].label_key.as_str()),
            ("primary", "Weekly limit")
        );
    }

    #[test]
    fn auth_file_parsing() {
        let access = jwt(&json!({"exp": NOW - 1}));
        let id = jwt(
            &json!({"email":"dev@example.test","https://api.openai.com/auth":{"chatgpt_plan_type":"pro"}}),
        );
        let body = json!({"tokens":{"access_token":access,"account_id":" acc-1 ","id_token":id}})
            .to_string();
        let a = codex::parse_auth(&body, NOW).unwrap();
        assert!(a.expired);
        assert_eq!(a.plan.as_deref(), Some("pro"));
        assert_eq!(a.email.as_deref(), Some("dev@example.test"));
        assert!(!format!("{a:?}").contains("acc-1"));
        let flat = jwt(&json!({"https://api.openai.com/auth.chatgpt_plan_type":"team"}));
        let body = json!({"tokens":{"access_token":"opaque","account_id":"a","id_token":flat}})
            .to_string();
        let a = codex::parse_auth(&body, NOW).unwrap();
        assert!(!a.expired, "unknown exp is not expired");
        assert_eq!(a.plan.as_deref(), Some("team"));
        assert!(
            codex::parse_auth(r#"{"tokens":{"access_token":"x","account_id":""}}"#, NOW).is_none()
        );
        assert!(codex::parse_auth(r#"{"OPENAI_API_KEY":"k"}"#, NOW).is_none());
    }

    #[test]
    fn app_server_bucket_selection() {
        let res = json!({"rateLimitsByLimitId":{"other":{"limitId":"other","primary":{"usedPercent":90}},
            "codex":{"limitId":"codex","primary":{"usedPercent":37,"windowDurationMins":300,"resetsAt":1792000000},
            "secondary":{"usedPercent":12,"windowDurationMins":10080,"resetsAt":1792400000}}}});
        let w = parse_app_server_limits(&res).unwrap();
        assert_eq!(ids(&w), ["primary", "secondary"]);
        assert_eq!(w[0].used_percent, Some(37.0));
        assert_eq!(w[0].window_seconds, Some(18000));
        assert_eq!(w[1].label_key, "Weekly limit");
        let res = json!({"rateLimitsByLimitId":null,"rateLimits":{"primary":{"usedPercent":4}}});
        assert_eq!(ids(&parse_app_server_limits(&res).unwrap()), ["primary"]);
        assert!(parse_app_server_limits(
            &json!({"rateLimits":{"limitId":"x","primary":{"usedPercent":4}}})
        )
        .is_none());
        assert!(parse_app_server_limits(&json!({"rateLimitsByLimitId":{"x":{}}})).is_none());
        assert!(parse_app_server_limits(&json!({"rateLimits":{"primary":null}})).is_none());
    }

    fn rollout_line(ts: &str, used: f64, limit_id: &str) -> String {
        json!({"timestamp":ts,"type":"event_msg","payload":{"type":"token_count",
            "rate_limits":{"limit_id":limit_id,"primary":{"used_percent":used,"window_minutes":300,"resets_at":1792000000},
            "secondary":{"used_percent":11.0,"window_minutes":10080,"resets_in_seconds":3600},"plan_type":"plus"}}})
        .to_string()
    }

    #[test]
    fn rollout_line_parsing() {
        let s = parse_rollout_line(&rollout_line("2026-10-15T08:00:00Z", 37.0, "codex"), NOW, 1)
            .unwrap();
        assert_eq!(s.at, util::rfc3339_secs("2026-10-15T08:00:00Z").unwrap());
        assert_eq!(ids(&s.windows), ["primary", "secondary"]);
        assert_eq!(s.windows[1].resets_at, Some(NOW + 3600));
        assert_eq!(s.windows[1].window_seconds, Some(10080 * 60));
        assert_eq!(s.plan.as_deref(), Some("plus"));
        assert!(
            parse_rollout_line(&rollout_line("2026-10-15T08:00:00Z", 37.0, "other"), NOW, 1)
                .is_none()
        );
        // top-level rate_limits, no timestamp → fallback time
        let top = r#"{"rate_limits":{"primary":{"used_percent":5}}}"#;
        assert_eq!(parse_rollout_line(top, NOW, 77).unwrap().at, 77);
        assert!(parse_rollout_line(r#"{"payload":{"rate_limits":null}}"#, NOW, 1).is_none());
        assert!(parse_rollout_line("garbage rate_limits", NOW, 1).is_none());
    }

    #[test]
    fn newest_valid_line_in_the_tail_wins() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rollout-a.jsonl");
        let mut text = String::new();
        // > 256 KiB of filler first, so the head (with an old snapshot) is cut off
        text.push_str(&rollout_line("2026-10-01T00:00:00Z", 1.0, "codex"));
        text.push('\n');
        let filler = format!("{}\n", "x".repeat(1000));
        for _ in 0..300 {
            text.push_str(&filler);
        }
        text.push_str(&rollout_line("2026-10-15T07:00:00Z", 30.0, "codex"));
        text.push('\n');
        text.push_str(&rollout_line("2026-10-15T08:00:00Z", 40.0, "codex"));
        text.push('\n');
        text.push_str(&rollout_line("2026-10-15T09:00:00Z", 50.0, "spark"));
        text.push_str("\n{\"type\":\"other\"}\n");
        write(&file, &text);
        let s = codex::snapshot_from_file(&file, NOW).unwrap();
        assert_eq!(s.windows[0].used_percent, Some(40.0));
        // only an old snapshot beyond the tail: none
        let file2 = dir.path().join("rollout-b.jsonl");
        let mut t2 = rollout_line("2026-10-01T00:00:00Z", 1.0, "codex");
        t2.push('\n');
        for _ in 0..300 {
            t2.push_str(&filler);
        }
        write(&file2, &t2);
        assert!(codex::snapshot_from_file(&file2, NOW).is_none());
    }

    #[test]
    fn day_directory_scan() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path();
        let line = rollout_line("2026-10-15T08:00:00Z", 22.0, "codex");
        write(
            &profile.join("sessions/2026/10/15/rollout-new.jsonl"),
            &line,
        );
        write(
            &profile.join("sessions/2026/10/14/rollout-x.jsonl"),
            &rollout_line("2026-10-14T08:00:00Z", 9.0, "codex"),
        );
        write(&profile.join("sessions/2026/09/30/rollout-y.jsonl"), &line);
        write(&profile.join("sessions/2025/12/31/rollout-z.jsonl"), &line);
        write(
            &profile.join("sessions/2026/10/15/notes.txt"),
            "rate_limits",
        );
        let files = codex::scanned_rollouts(profile);
        assert_eq!(files.len(), 3, "the 3 newest day folders only");
        assert!(files.iter().all(|f| !f.to_string_lossy().contains("2025")));
        let (at, windows) = codex_rollout_windows(profile, NOW).unwrap();
        assert!(
            at == util::rfc3339_secs("2026-10-15T08:00:00Z").unwrap()
                || at == util::rfc3339_secs("2026-10-14T08:00:00Z").unwrap()
        );
        assert!(!windows.is_empty());
        assert!(codex_rollout_windows(&profile.join("nothing"), NOW).is_none());
    }

    #[test]
    fn sqlite_index_is_preferred() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path();
        let chosen = profile.join("elsewhere").join("rollout-db.jsonl");
        write(
            &chosen,
            &rollout_line("2026-10-15T08:30:00Z", 66.0, "codex"),
        );
        write(
            &profile.join("sessions/2026/10/15/rollout-scan.jsonl"),
            &rollout_line("2026-10-15T08:00:00Z", 1.0, "codex"),
        );
        {
            let conn = rusqlite::Connection::open(profile.join("state_5.sqlite")).unwrap();
            conn.execute_batch(
                "CREATE TABLE threads (rollout_path TEXT, archived INTEGER, updated_at INTEGER);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO threads VALUES (?1, 0, 5), ('C:/missing/rollout-gone.jsonl', 0, 9), (?2, 1, 99)",
                [chosen.to_string_lossy().to_string(), "C:/x/rollout-archived.jsonl".to_string()],
            )
            .unwrap();
        }
        assert_eq!(
            codex::sqlite_rollouts(profile),
            vec![chosen.clone()],
            "fallback query on updated_at"
        );
        let (_, w) = codex_rollout_windows(profile, NOW).unwrap();
        assert_eq!(w[0].used_percent, Some(66.0));
    }

    // ---- fetch ----------------------------------------------------------------------------

    fn setup(expired: bool) -> (tempfile::TempDir, Env) {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path()).with_var(NO_SPAWN_ENV, "1");
        let exp = if expired { NOW - 10 } else { NOW + 3600 };
        let body = json!({"tokens":{"access_token":jwt(&json!({"exp":exp})),"account_id":"acct-9",
            "id_token":jwt(&json!({"email":"dev@example.test"}))}});
        write(&env.home_path(".codex/auth.json"), &body.to_string());
        (root, env)
    }

    fn account() -> Account {
        Account::new(ProviderKind::Codex, "Codex")
    }

    #[test]
    fn not_installed_when_nothing_exists() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path());
        let r = CodexProvider.fetch(
            &account(),
            &ctx_with(env.clone(), Arc::new(FakeHttp::new()), NOW),
        );
        assert_eq!(r.status, ReadingStatus::NotInstalled);
        assert!(CodexProvider.discover(&env).is_empty());
        std::fs::create_dir_all(env.home_path(".codex/sessions")).unwrap();
        assert_eq!(CodexProvider.discover(&env).len(), 1);
    }

    #[test]
    fn live_read() {
        let (_root, env) = setup(false);
        let http = Arc::new(FakeHttp::new().on(CODEX_USAGE_URL, 200, USAGE));
        let r = CodexProvider.fetch(&account(), &ctx_with(env, http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.note.as_deref(), Some("Plus · via Codex"));
        assert_eq!(r.plan.as_deref(), Some("Plus"));
        assert_eq!(r.identity.as_deref(), Some("dev@example.test"));
        assert_eq!(r.headline().unwrap().id, "primary");
        assert_eq!(r.weekly().unwrap().id, "secondary");
        let req = &http.requests()[0];
        assert_eq!(req.header_value("chatgpt-account-id"), Some("acct-9"));
        assert_eq!(
            req.header_value("cache-control"),
            Some("no-cache, no-store")
        );
        assert!(req
            .header_value("user-agent")
            .unwrap()
            .starts_with("codenotch/"));
        assert!(req
            .header_value("authorization")
            .unwrap()
            .starts_with("Bearer "));
        assert_no_secret(&r, "acct-9");
    }

    #[test]
    fn rejected_live_read_falls_back_to_rollout() {
        let (_root, env) = setup(true);
        let fresh_ts = chrono::DateTime::from_timestamp(NOW - 120, 0)
            .unwrap()
            .to_rfc3339();
        write(
            &env.home_path(".codex/sessions/2026/10/15/rollout-1.jsonl"),
            &rollout_line(&fresh_ts, 37.0, "codex"),
        );
        let http = Arc::new(FakeHttp::new().on(CODEX_USAGE_URL, 401, "{}"));
        let r = CodexProvider.fetch(&account(), &ctx_with(env.clone(), http.clone(), NOW));
        assert_eq!(
            r.status,
            ReadingStatus::Fresh,
            "≤ 5 min old by its own timestamp"
        );
        assert_eq!(r.fetched_at, NOW - 120);
        assert_eq!(
            r.note.as_deref(),
            Some(format!("{} · Plus · from last Codex run", codex::NOTE_EXPIRED).as_str())
        );
        // same snapshot read later is stale
        let r = CodexProvider.fetch(&account(), &ctx_with(env, http, NOW + 600));
        assert_eq!(r.status, ReadingStatus::Stale);
        assert_eq!(r.fetched_at, NOW - 120);
    }

    #[test]
    fn rate_limit_skips_live_but_reads_rollout() {
        let (_root, env) = setup(false);
        let ts = chrono::DateTime::from_timestamp(NOW - 30, 0)
            .unwrap()
            .to_rfc3339();
        write(
            &env.home_path(".codex/sessions/2026/10/15/rollout-1.jsonl"),
            &rollout_line(&ts, 37.0, "codex"),
        );
        let http = Arc::new(FakeHttp::new().on_response(
            CODEX_USAGE_URL,
            HttpResponse::new(429, "").with_header("Retry-After", "30"),
        ));
        let a = account();
        let r = CodexProvider.fetch(&a, &ctx_with(env.clone(), http.clone(), NOW));
        assert_eq!(
            r.status,
            ReadingStatus::RateLimited {
                until: Some(NOW + 60)
            },
            "at least 60 s"
        );
        assert_eq!(r.windows.len(), 2);
        // during the back-off no request is made
        let mut ctx = ctx_with(env, http.clone(), NOW + 10);
        ctx.previous = Some(r);
        let r2 = CodexProvider.fetch(&a, &ctx);
        assert_eq!(
            r2.status,
            ReadingStatus::RateLimited {
                until: Some(NOW + 60)
            }
        );
        assert_eq!(http.requests().len(), 1);
    }

    #[test]
    fn no_snapshot_outcomes() {
        let a = account();
        // 429, nothing anywhere
        let (_root, env) = setup(false);
        let http = Arc::new(FakeHttp::new().on(CODEX_USAGE_URL, 429, ""));
        let r = CodexProvider.fetch(&a, &ctx_with(env.clone(), http, NOW));
        assert_eq!(
            r.status,
            ReadingStatus::RateLimited {
                until: Some(NOW + 60)
            }
        );
        // 403 → signed out
        let http = Arc::new(FakeHttp::new().on(CODEX_USAGE_URL, 403, ""));
        let r = CodexProvider.fetch(&a, &ctx_with(env.clone(), http, NOW));
        assert_eq!(r.status, ReadingStatus::SignedOut);
        assert_eq!(r.note.as_deref(), Some(codex::NOTE_REJECTED));
        // 200 without windows → empty
        let http = Arc::new(FakeHttp::new().on(CODEX_USAGE_URL, 200, r#"{"plan_type":"free"}"#));
        let r = CodexProvider.fetch(&a, &ctx_with(env.clone(), http, NOW));
        assert_eq!(r.status, ReadingStatus::Empty);
        assert_eq!(r.note.as_deref(), Some(codex::NOTE_NO_WINDOWS));
        // transport failure → error
        let http = Arc::new(FakeHttp::new());
        let r = CodexProvider.fetch(&a, &ctx_with(env, http, NOW));
        assert_eq!(r.status, ReadingStatus::Error);
        assert!(r.note.unwrap().starts_with("Live read failed"));
        // sessions folder but no auth.json → signed out, nothing sent
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path()).with_var(NO_SPAWN_ENV, "1");
        std::fs::create_dir_all(env.home_path(".codex/sessions")).unwrap();
        let http = Arc::new(FakeHttp::new());
        let r = CodexProvider.fetch(&a, &ctx_with(env, http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::SignedOut);
        assert!(http.requests().is_empty());
    }

    #[test]
    fn app_server_response_wait() {
        use std::time::{Duration, Instant};
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        for line in [
            "not json",
            r#"{"method":"note"}"#,
            r#"{"id":1,"result":{}}"#,
            r#"{"id":2,"result":{"ok":true}}"#,
        ] {
            tx.send(line.into()).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let v = codex::wait_response(&rx, 2, deadline).unwrap();
        assert_eq!(v["result"]["ok"], true);
        // nothing more arrives: gives up at the deadline
        let start = Instant::now();
        assert!(
            codex::wait_response(&rx, 3, Instant::now() + Duration::from_millis(100)).is_none()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        drop(tx);
        assert!(
            codex::wait_response(&rx, 3, Instant::now() + Duration::from_secs(5)).is_none(),
            "closed pipe"
        );
    }

    #[test]
    fn executable_lookup() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path());
        assert!(find_codex(&env, false).is_none());
        write(&env.appdata.join("npm").join("codex.cmd"), "");
        assert!(find_codex(&env, false).is_some());
        assert!(
            find_codex(&env, true).is_none(),
            "no .cmd for the app-server"
        );
        let native = env.local_appdata.join("OpenAI/Codex/bin/1.2.3/codex.exe");
        write(&native, "");
        assert_eq!(find_codex(&env, true), Some(native));
        let env2 = Env::for_test(root.path()).with_var(NO_SPAWN_ENV, "1");
        codex::reset_app_server_rest(&env2.home_path(".codex"));
    }
}

// =============================================================================================
// Grok
// =============================================================================================

mod grok_tests {
    use super::*;
    use crate::grok::{self, is_trusted, pick_creds};
    use serde_json::json;

    const BILLING: &str = r#"{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-11-08T08:00:00Z","end":"2026-11-15T08:00:00Z"},
           "creditUsagePercent":8.0,
           "productUsage":[{"product":"GrokBuild","usagePercent":8.0}],
           "billingPeriodStart":"2026-11-01T00:00:00Z","billingPeriodEnd":"2026-12-01T00:00:00Z"}}"#;

    #[test]
    fn credit_window() {
        let w = parse_grok_usage(BILLING, NOW).unwrap();
        assert_eq!(ids(&w), ["credits"]);
        assert_eq!(w[0].label_key, "Credits");
        assert_eq!(w[0].detail.as_deref(), Some("Grok Build"));
        assert_eq!(w[0].used_percent, Some(8.0));
        assert_eq!(w[0].kind, WindowKind::Weekly);
        assert_eq!(w[0].resets_at, util::rfc3339_secs("2026-11-15T08:00:00Z"));
        assert_eq!(w[0].window_seconds, Some(7 * 86400));
    }

    #[test]
    fn plan_from_the_cli_settings_cache() {
        let inner = json!({"settings":{"subscription_tier":null,
            "subscription_tier_display":"SuperGrok Heavy"}})
        .to_string();
        let body = json!({"payload": inner, "signature": "sig"}).to_string();
        assert_eq!(
            grok::plan_from_settings_cache(&body).as_deref(),
            Some("SuperGrok Heavy")
        );
        let body = r#"{"payload":{"settings":{"subscription_tier":"super_grok"}}}"#;
        assert_eq!(
            grok::plan_from_settings_cache(body).as_deref(),
            Some("Super grok")
        );
        assert_eq!(grok::plan_from_settings_cache(r#"{"payload":"x"}"#), None);

        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path());
        let dir = env.home_path(".grok");
        write(
            &dir.join("auth.json"),
            &json!({"https://auth.x.ai::c": {"key":"k-s3cret","email":"me@x.test"}}).to_string(),
        );
        write(
            &dir.join("settings_cache.json"),
            &json!({"payload": inner}).to_string(),
        );
        let local = GrokProvider.local_identity(&Account::new(ProviderKind::Grok, "Grok"), &env);
        assert_eq!(local.identity.as_deref(), Some("me@x.test"));
        assert_eq!(local.plan.as_deref(), Some("SuperGrok Heavy"));
        assert!(!format!("{local:?}").contains("s3cret"));
    }

    #[test]
    fn product_windows_and_fallbacks() {
        let body = r#"{"config":{"billingPeriodStart":"2026-11-01T00:00:00Z","billingPeriodEnd":"2026-12-01T00:00:00Z",
            "productUsage":[{"product":"GrokBuild","usagePercent":20},{"product":"NoFigure"},{"product":"GrokImagine","usagePercent":3}]}}"#;
        let w = parse_grok_usage(body, NOW).unwrap();
        assert_eq!(ids(&w), ["credits", "GrokImagine"]);
        assert_eq!(w[1].label_key, "Grok Imagine");
        assert_eq!(w[0].resets_at, util::rfc3339_secs("2026-12-01T00:00:00Z"));
        assert_eq!(w[0].window_seconds, Some(30 * 86400));
        // fresh weekly period with nothing yet reads 0 %, not unmetered
        let body = r#"{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-11-15T08:00:00Z"}}}"#;
        let w = parse_grok_usage(body, NOW).unwrap();
        assert_eq!(w[0].used_percent, Some(0.0));
        assert_eq!(w[0].window_seconds, None);
        // nothing metered vs no config
        assert!(parse_grok_usage(r#"{"config":{}}"#, NOW)
            .unwrap()
            .is_empty());
        assert!(parse_grok_usage(r#"{"other":1}"#, NOW).is_err());
        assert!(parse_grok_usage("x", NOW).is_err());
    }

    #[test]
    fn issuer_check() {
        let e = json!({"key":"k"});
        assert!(is_trusted("https://auth.x.ai::client", &e));
        assert!(!is_trusted("https://auth.x.ai.example.com::client", &e));
        assert!(!is_trusted("https://idp.corp.test::client", &e));
        assert!(is_trusted(
            "https://idp.corp.test::client",
            &json!({"oidc_issuer":"https://auth.x.ai"})
        ));
        assert!(!is_trusted(
            "x",
            &json!({"oidc_issuer":"https://auth.x.ai/"})
        ));
    }

    #[test]
    fn credential_selection() {
        let body = json!({
            "https://idp.corp.test::c": {"key":"corp-secret","expires_at":"2099-01-01T00:00:00Z"},
            "https://auth.x.ai::a": {"key":"old","expires_at":"2020-01-01T00:00:00Z","email":"a@x.test"},
            "https://auth.x.ai::b": {"key":"live","expires_at":"2099-01-01T00:00:00Z","email":"b@x.test"},
            "https://auth.x.ai::c": {"key":"","expires_at":"2099-01-01T00:00:00Z"}
        })
        .to_string();
        let c = pick_creds(&body, NOW).unwrap();
        assert_eq!(c.email.as_deref(), Some("b@x.test"));
        assert!(!format!("{c:?}").contains("live"));
        let only_old =
            json!({"https://auth.x.ai::a": {"key":"old","expires_at":"2020-01-01T00:00:00Z"}})
                .to_string();
        assert!(
            pick_creds(&only_old, NOW).is_some(),
            "an expired session is still sent"
        );
        let untrusted = json!({"https://idp.corp.test::c": {"key":"corp"}}).to_string();
        assert!(pick_creds(&untrusted, NOW).is_none());
    }

    fn setup(entries: serde_json::Value) -> (tempfile::TempDir, Env) {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path()).with_var(NO_SPAWN_ENV, "1");
        write(&env.home_path(".grok/auth.json"), &entries.to_string());
        (root, env)
    }

    fn account() -> Account {
        Account::new(ProviderKind::Grok, "Grok")
    }

    #[test]
    fn fetch_paths() {
        let root = tempfile::tempdir().unwrap();
        let r = GrokProvider.fetch(
            &account(),
            &ctx_with(Env::for_test(root.path()), Arc::new(FakeHttp::new()), NOW),
        );
        assert_eq!(r.status, ReadingStatus::NotInstalled);

        let (_r1, env) = setup(json!({"https://idp.corp.test::c": {"key":"corp-key"}}));
        let http = Arc::new(FakeHttp::new().on(GROK_BILLING_URL, 200, BILLING));
        let r = GrokProvider.fetch(&account(), &ctx_with(env, http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::SignedOut);
        assert!(http.requests().is_empty(), "foreign issuer never sent");

        let (_r2, env) =
            setup(json!({"https://auth.x.ai::a": {"key":"xai-key","email":"me@x.test"}}));
        let r = GrokProvider.fetch(&account(), &ctx_with(env.clone(), http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.headline_percent(), Some(8.0));
        assert_eq!(r.identity.as_deref(), Some("me@x.test"));
        assert_eq!(r.note.as_deref(), Some("me@x.test · via Grok CLI"));
        let req = &http.requests()[0];
        assert_eq!(req.header_value("x-xai-token-auth"), Some("xai-grok-cli"));
        assert_eq!(req.header_value("authorization"), Some("Bearer xai-key"));
        assert_no_secret(&r, "xai-key");

        let a = account();
        let prev = r.clone();
        for (status, expect) in [(401, ReadingStatus::SignedOut), (500, ReadingStatus::Error)] {
            let http = Arc::new(FakeHttp::new().on(GROK_BILLING_URL, status, ""));
            let mut ctx = ctx_with(env.clone(), http, NOW + 300);
            ctx.previous = Some(Reading {
                account_id: a.id.clone(),
                ..prev.clone()
            });
            let r = GrokProvider.fetch(&a, &ctx);
            assert_eq!(r.status, expect);
            assert_eq!(r.windows.len(), 1);
            assert_eq!(r.fetched_at, NOW);
        }
        let http = Arc::new(FakeHttp::new().on_response(
            GROK_BILLING_URL,
            HttpResponse::new(429, "").with_header("Retry-After", "90"),
        ));
        let r = GrokProvider.fetch(&a, &ctx_with(env.clone(), http, NOW));
        assert_eq!(
            r.status,
            ReadingStatus::RateLimited {
                until: Some(NOW + 90)
            }
        );
        assert_eq!(r.note.as_deref(), Some(grok::NOTE_RATE_LIMITED));
        let http = Arc::new(FakeHttp::new().on(GROK_BILLING_URL, 200, r#"{"config":{}}"#));
        assert_eq!(
            GrokProvider
                .fetch(&a, &ctx_with(env.clone(), http, NOW))
                .status,
            ReadingStatus::Empty
        );
        let http = Arc::new(FakeHttp::new().on(GROK_BILLING_URL, 200, r#"{}"#));
        let r = GrokProvider.fetch(&a, &ctx_with(env, http, NOW));
        assert_eq!(r.note.as_deref(), Some(grok::NOTE_NO_CONFIG));
    }

    #[test]
    fn cli_lookup() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path());
        let profile = env.home_path(".grok");
        assert!(find_grok_cli(&env, &profile).is_none());
        write(&profile.join("bin/grok.cmd"), "");
        assert_eq!(
            find_grok_cli(&env, &profile),
            Some(profile.join("bin/grok.cmd"))
        );
    }
}

// =============================================================================================
// Custom endpoint
// =============================================================================================

mod custom_tests {
    use super::*;

    #[test]
    fn parses_the_documented_body() {
        let body = r#"{"windows":[{"id":"quota","label":"Monthly quota","usedPercent":42,
            "resetsAt":"2026-11-01T00:00:00Z","windowSeconds":2592000}]}"#;
        let w = parse_custom_usage(body, NOW).unwrap();
        assert_eq!(ids(&w), ["quota"]);
        assert_eq!(w[0].label_key, "Monthly quota");
        assert_eq!(w[0].used_percent, Some(42.0));
        assert_eq!(w[0].kind, WindowKind::Monthly);
        assert_eq!(w[0].resets_at, util::rfc3339_secs("2026-11-01T00:00:00Z"));
    }

    #[test]
    fn tolerant_fields() {
        let body = r#"{"windows":[
            {"label":"Daily tokens","used_percent":0.42,"resets_at":1792000000},
            {"name":"Weekly cap","utilization":55,"reset_at":1792000000000},
            {"label":"Requests","used":30,"limit":120,"detail":"30 / 120","group":"API"},
            {"label":"Calls","count":17},
            {"label":"Nothing here"},
            "junk",
            {"label":"Daily tokens","usedPercent":1}]}"#;
        let w = parse_custom_usage(body, NOW).unwrap();
        assert_eq!(ids(&w), ["daily_tokens", "weekly_cap", "requests", "calls"]);
        assert_eq!(w[0].used_percent, Some(42.0), "0–1 fraction");
        assert_eq!(w[0].resets_at, Some(1_792_000_000));
        assert_eq!(w[0].kind, WindowKind::Session);
        assert_eq!(w[1].resets_at, Some(1_792_000_000), "epoch ms");
        assert_eq!(w[1].kind, WindowKind::Weekly);
        assert_eq!(w[2].used_percent, Some(25.0));
        assert_eq!(w[2].group.as_deref(), Some("API"));
        assert_eq!(w[3].count, Some(17));
        assert_eq!(w[3].used_percent, None, "a count is not a percentage");
        // top-level array
        assert_eq!(
            parse_custom_usage(r#"[{"label":"A","usedPercent":5}]"#, NOW)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn bodies_without_windows_are_errors() {
        assert!(parse_custom_usage(r#"{"windows":[]}"#, NOW).is_err());
        assert!(parse_custom_usage(r#"{"tokens":12345}"#, NOW).is_err());
        assert!(parse_custom_usage(r#"{"windows":[{"label":"x"}]}"#, NOW).is_err());
        assert!(parse_custom_usage("42", NOW).is_err());
        assert!(parse_custom_usage("nope", NOW).is_err());
    }

    fn account(endpoint: &str, secret_env: &str) -> Account {
        let mut a = Account::new(ProviderKind::Custom, "Mine");
        a.endpoint = endpoint.into();
        a.secret_env = secret_env.into();
        a
    }

    const BODY: &str = r#"{"windows":[{"label":"Quota","usedPercent":10}]}"#;

    #[test]
    fn endpoint_allow_list_is_enforced_before_sending() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path());
        let http = Arc::new(FakeHttp::new().on("http://", 200, BODY));
        for bad in [
            "",
            "http://example.com/usage",
            "file:///c:/x.json",
            "ftp://h/x",
        ] {
            let r =
                CustomProvider.fetch(&account(bad, ""), &ctx_with(env.clone(), http.clone(), NOW));
            assert_eq!(r.status, ReadingStatus::Error, "{bad}");
        }
        assert!(http.requests().is_empty());
        let r = CustomProvider.fetch(
            &account("http://127.0.0.1:9000/u", ""),
            &ctx_with(env, http.clone(), NOW),
        );
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert!(http.requests()[0].header_value("authorization").is_none());
    }

    #[test]
    fn bearer_comes_from_the_named_variable() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path()).with_var("MY_QUOTA_TOKEN", "s3cr3t");
        let http = Arc::new(FakeHttp::new().on("https://quota.example.test/", 200, BODY));
        let a = account("https://quota.example.test/usage", "MY_QUOTA_TOKEN");
        let r = CustomProvider.fetch(&a, &ctx_with(env.clone(), http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.headline_percent(), Some(10.0));
        assert_eq!(
            http.requests()[0].header_value("authorization"),
            Some("Bearer s3cr3t")
        );
        assert_no_secret(&r, "s3cr3t");
        let missing = account("https://quota.example.test/usage", "NOT_SET_VAR");
        let r = CustomProvider.fetch(&missing, &ctx_with(env.clone(), http.clone(), NOW));
        assert_eq!(r.status, ReadingStatus::Error);
        assert_eq!(http.requests().len(), 1);
        for (code, status) in [(401, ReadingStatus::SignedOut), (500, ReadingStatus::Error)] {
            let http = Arc::new(FakeHttp::new().on("https://", code, ""));
            assert_eq!(
                CustomProvider
                    .fetch(&a, &ctx_with(env.clone(), http, NOW))
                    .status,
                status
            );
        }
        let http = Arc::new(FakeHttp::new().on("https://", 429, ""));
        assert_eq!(
            CustomProvider.fetch(&a, &ctx_with(env, http, NOW)).status,
            ReadingStatus::RateLimited {
                until: Some(NOW + 60)
            }
        );
    }

    #[test]
    fn bearer_falls_back_to_the_secret_store() {
        use codenotch_core::secrets::{MemorySecrets, SecretKey, SecretStore};
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path());
        let http = Arc::new(FakeHttp::new().on("https://quota.example.test/", 200, BODY));
        let store = Arc::new(MemorySecrets::new());
        // Saved from the settings window, no `secretEnv`.
        let a = account("https://quota.example.test/usage", "");
        store
            .set(&SecretKey::Account(a.id.clone()), "st0red-tok")
            .unwrap();
        let mut ctx = ctx_with(env.clone(), http.clone(), NOW);
        ctx.secrets = Some(store.clone());
        let r = CustomProvider.fetch(&a, &ctx);
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(
            http.requests()[0].header_value("authorization"),
            Some("Bearer st0red-tok")
        );
        assert_no_secret(&r, "st0red-tok");
        // `secretEnv` names an unset variable: the stored token is used.
        let mut b = account("https://quota.example.test/usage", "NOT_SET_VAR");
        b.id = a.id.clone();
        assert_eq!(CustomProvider.fetch(&b, &ctx).status, ReadingStatus::Fresh);
        assert_eq!(
            http.requests()[1].header_value("authorization"),
            Some("Bearer st0red-tok")
        );
        // The variable wins over the store.
        let mut ctx = ctx_with(env.with_var("NOT_SET_VAR", "env-tok"), http.clone(), NOW);
        ctx.secrets = Some(store);
        assert_eq!(CustomProvider.fetch(&b, &ctx).status, ReadingStatus::Fresh);
        assert_eq!(
            http.requests()[2].header_value("authorization"),
            Some("Bearer env-tok")
        );
    }
}

// =============================================================================================
// French messages (crate::FR)
// =============================================================================================

mod i18n_tests {
    use super::*;
    use codenotch_core::i18n::{tr_in, Lang};

    fn french(mut ctx: FetchContext) -> FetchContext {
        ctx.settings.language = "fr".into();
        ctx
    }

    fn claude_env() -> (tempfile::TempDir, Env) {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path()).with_var(NO_SPAWN_ENV, "1");
        write(
            &env.home_path(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"tok"}}"#,
        );
        (root, env)
    }

    #[test]
    fn claude_rate_limit_note_is_built_in_the_user_language() {
        let (_root, env) = claude_env();
        let a = Account::new(ProviderKind::Claude, "Claude");
        let resp = || HttpResponse::new(429, "").with_header("Retry-After", "120");
        let http = Arc::new(FakeHttp::new().on_response(CLAUDE_USAGE_URL, resp()));
        let r = ClaudeProvider.fetch(&a, &ctx_with(env.clone(), http, NOW));
        assert_eq!(
            r.note.as_deref(),
            Some("Rate limited, retrying in 120s"),
            "English unchanged"
        );
        let http = Arc::new(FakeHttp::new().on_response(CLAUDE_USAGE_URL, resp()));
        let r = ClaudeProvider.fetch(&a, &french(ctx_with(env, http, NOW)));
        assert_eq!(r.note.as_deref(), Some("Limité, nouvel essai dans 120 s"));
        assert_eq!(
            r.error.as_deref(),
            Some("HTTP 429"),
            "technical error untouched"
        );
    }

    #[test]
    fn codex_http_failure_is_french() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path()).with_var(NO_SPAWN_ENV, "1");
        let body = serde_json::json!({"tokens":{"access_token":jwt(&serde_json::json!({"exp":NOW + 3600})),"account_id":"acct"}});
        write(&env.home_path(".codex/auth.json"), &body.to_string());
        let a = Account::new(ProviderKind::Codex, "Codex");
        let http = Arc::new(FakeHttp::new().on(CODEX_USAGE_URL, 500, ""));
        let r = CodexProvider.fetch(&a, &ctx_with(env.clone(), http, NOW));
        assert_eq!(r.note.as_deref(), Some("Live read failed (HTTP 500)"));
        let http = Arc::new(FakeHttp::new().on(CODEX_USAGE_URL, 500, ""));
        let r = CodexProvider.fetch(&a, &french(ctx_with(env, http, NOW)));
        assert_eq!(r.status, ReadingStatus::Error);
        assert_eq!(
            r.note.as_deref(),
            Some("Échec de la lecture en direct (HTTP 500)")
        );
    }

    #[test]
    fn custom_missing_variable_is_french() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path());
        let mut a = Account::new(ProviderKind::Custom, "Mine");
        a.endpoint = "https://quota.example.test/usage".into();
        a.secret_env = "NOT_SET_VAR".into();
        let http = Arc::new(FakeHttp::new());
        let r = CustomProvider.fetch(&a, &ctx_with(env.clone(), http.clone(), NOW));
        assert_eq!(
            r.error.as_deref(),
            Some("Environment variable NOT_SET_VAR is not set")
        );
        let r = CustomProvider.fetch(&a, &french(ctx_with(env, http, NOW)));
        assert_eq!(
            r.error.as_deref(),
            Some("La variable d'environnement NOT_SET_VAR n'est pas définie")
        );
    }

    #[test]
    fn fixed_notes_stay_english_and_translate_at_display() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::for_test(root.path()).with_var(NO_SPAWN_ENV, "1");
        write(
            &env.home_path(".grok/auth.json"),
            r#"{"https://auth.x.ai::u":{"key":"k"}}"#,
        );
        let http = Arc::new(FakeHttp::new().on(GROK_BILLING_URL, 401, ""));
        let r = GrokProvider.fetch(
            &Account::new(ProviderKind::Grok, "Grok"),
            &french(ctx_with(env, http, NOW)),
        );
        let note = r.note.as_deref().unwrap();
        assert_eq!(
            note,
            crate::grok::NOTE_REJECTED,
            "a fixed note stays English in the reading"
        );
        assert_eq!(
            tr_in(Lang::Fr, note, FR),
            "Session Grok refusée — relancez grok login"
        );
        assert_eq!(tr_in(Lang::En, note, FR), note);
    }
}
