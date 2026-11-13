//! Sentinel tests: private values placed everywhere a provider, a transcript, a hook or the
//! user could put them never appear in the bytes sent, in the files written, or in `Debug`.

mod common;

use codenotch_core::analytics::{HourlyUsage, RootOwners};
use codenotch_core::model::{ActivityState, ReadingStatus, SessionInfo, WindowKind};
use codenotch_core::{FakeHttp, LimitWindow, ProviderKind, Reading, TeamsPayer, TokenCounts};
use codenotch_teams::{pairing, InviteCode, Origin, PairError, Policy, TeamsSync};
use common::*;

const PROMPT: &str = "PROMPT-SENTINEL please refactor the billing module";
const PATH: &str = r"C:\Users\alice\secret-repo";
const EMAIL: &str = "alice@corp.invalid";
const TOKEN_SENTINEL: &str = "sk-secret123";
const SLUG: &str = "C_Users_Kev_client";
const HOOK: &str = "HOOK-SENTINEL";
const DEVICE_TOKEN: &str = "cnt_DEVICE_TOKEN_SENTINEL";

/// Everything that must never leave or be stored.
fn sentinels() -> Vec<&'static str> {
    vec![
        "PROMPT-SENTINEL",
        "billing module",
        "alice",
        "secret-repo",
        "corp.invalid",
        TOKEN_SENTINEL,
        SLUG,
        HOOK,
    ]
}

fn assert_clean(what: &str, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    for s in sentinels() {
        assert!(!text.contains(s), "{s:?} found in {what}");
    }
    assert!(!text.contains(DEVICE_TOKEN), "device token found in {what}");
}

/// A 256 KiB hook body full of sentinels, as the hook server receives it.
fn hook_body() -> String {
    let unit = format!(
        r#"{{"hook_event_name":"UserPromptSubmit","prompt":"{PROMPT}","cwd":"{}","session_id":"{HOOK}","transcript_path":"{}"}}"#,
        PATH.replace('\\', "\\\\"),
        PATH.replace('\\', "\\\\")
    );
    unit.repeat(256 * 1024 / unit.len() + 1)
}

#[test]
fn sentinels_never_reach_the_wire_the_disk_or_debug() {
    let mut f = fixture();
    // Account fields full of private values.
    for a in &mut f.config.accounts {
        a.name = EMAIL.into();
        a.profile_path = PATH.into();
        a.endpoint = format!("https://{TOKEN_SENTINEL}.example/{SLUG}");
        a.secret_env = TOKEN_SENTINEL.into();
    }
    let source = format!(r"{PATH}\.claude\projects");
    f.owners = vec![RootOwners {
        source: source.clone(),
        provider: ProviderKind::Claude,
        accounts: vec!["c1".into()],
    }];

    // The session the activity view shows (its detail is the last prompt) and a hook body:
    // the Teams API takes neither, so the only way in would be through a reading field.
    let session = SessionInfo {
        id: HOOK.into(),
        title: SLUG.into(),
        cwd: Some(PATH.into()),
        model: Some(TOKEN_SENTINEL.into()),
        state: ActivityState::Working,
        since: NOW,
        cost: None,
        detail: Some(PROMPT.into()),
    };
    let hook = hook_body();
    assert!(hook.len() >= 256 * 1024);

    let account = &f.config.accounts[0];
    let mut reading = Reading::fresh(
        account,
        NOW - 5,
        vec![
            {
                let mut w =
                    LimitWindow::new("session", hook.clone(), WindowKind::Session).used(50.0);
                w.detail = session.detail.clone();
                w
            },
            {
                let mut w = LimitWindow::new("seven_day", PROMPT, WindowKind::Weekly).used(20.0);
                w.detail = Some(hook.clone());
                w
            },
            LimitWindow::new(TOKEN_SENTINEL, "custom", WindowKind::Other).used(1.0),
            LimitWindow::new(SLUG, PROMPT, WindowKind::Other).used(1.0),
            {
                let mut w = LimitWindow::new("seven_day_opus", "x", WindowKind::Weekly).used(5.0);
                w.group = Some(PATH.into());
                w
            },
        ],
    );
    reading.identity = Some(EMAIL.into());
    reading.plan = Some(TOKEN_SENTINEL.into());
    reading.note = session.detail.clone();
    reading.error = Some(format!("Bearer {TOKEN_SENTINEL}"));
    reading.headline_window = Some("session".into());
    reading.weekly_window = Some(SLUG.into());
    assert_eq!(reading.status, ReadingStatus::Fresh);

    let rows = vec![
        HourlyUsage {
            hour: NOW - 3600,
            source: source.clone(),
            provider: ProviderKind::Claude,
            model: TOKEN_SENTINEL.into(),
            tokens: TokenCounts {
                input: 5,
                ..Default::default()
            },
            cache_write_1h: 0,
        },
        HourlyUsage {
            hour: NOW - 3600,
            source: source.clone(),
            provider: ProviderKind::Claude,
            model: format!("{SLUG}-{PROMPT}"),
            tokens: TokenCounts {
                output: 7,
                ..Default::default()
            },
            cache_write_1h: 0,
        },
    ];

    let mut s = TeamsSync::create(&f.paths, info("a")).unwrap();
    s.set_policy(&f.config, &f.owners, NOW).unwrap();
    assert!(s.observe_reading(&reading, NOW).unwrap());
    assert_eq!(
        s.observe_usage(&rows, NOW).unwrap(),
        1,
        "unknown models add up"
    );
    assert!(s.heartbeat(NOW).unwrap());

    // Files on disk.
    assert_clean("teams.json", &std::fs::read(f.paths.teams_state()).unwrap());
    assert_clean("outbox", &std::fs::read(f.paths.teams_outbox()).unwrap());
    // Debug output.
    assert_clean("Debug", format!("{s:?} {:?}", s.status()).as_bytes());
    let policy = Policy::new(&f.config, &f.owners, DEVICE, H0);
    assert_clean("Policy Debug", format!("{policy:?}").as_bytes());

    // Bytes on the wire: the batch, then every pairing call.
    let http = SeqHttp::new(vec![accept_all()]);
    s.send(&http, DEVICE_TOKEN, NOW);
    let origin = Origin::parse(ORIGIN).unwrap();
    let code = InviteCode::parse("https://teams.example.test/j/invite-code-123", &origin).unwrap();
    let fake = FakeHttp::new()
        .on(
            &format!("{ORIGIN}/v1/invites/preview"),
            200,
            r#"{"teamName":"Acme","visibility":"named","consentVersion":1}"#,
        )
        .on(
            &format!("{ORIGIN}/v1/devices/pair"),
            200,
            &format!(
                r#"{{"deviceToken":"{DEVICE_TOKEN}","teamId":"t1","teamName":"Acme","dataEpoch":1,"floorAt":{H0}}}"#
            ),
        )
        .on(&format!("{ORIGIN}/v1/"), 200, "{}");
    pairing::preview(&fake, &origin, &code).unwrap();
    let paired = pairing::pair(&fake, &origin, &code, DEVICE, 1).unwrap();
    assert!(!format!("{paired:?}").contains(DEVICE_TOKEN));
    pairing::declare_account(
        &fake,
        &origin,
        DEVICE_TOKEN,
        &policy,
        "c1",
        TeamsPayer::Member,
        Some(21.6),
    )
    .unwrap();
    pairing::detach(&fake, &origin, DEVICE_TOKEN).unwrap();
    pairing::erase(&fake, &origin, DEVICE_TOKEN).unwrap();

    let mut requests = http.requests();
    requests.extend(fake.requests());
    assert_eq!(requests.len(), 6);
    for r in &requests {
        assert!(r.url.starts_with(ORIGIN), "only the admitted origin");
        let mut seen = r.url.clone().into_bytes();
        seen.extend(r.body.clone().unwrap_or_default());
        for (name, value) in &r.headers {
            seen.extend(name.as_bytes());
            if !name.eq_ignore_ascii_case("authorization") {
                seen.extend(value.as_bytes());
            }
        }
        assert_clean(&r.url, &seen);
        assert_clean("request Debug", format!("{r:?}").as_bytes());
        // No forbidden key anywhere in a body.
        if let Some(body) = &r.body {
            let v: serde_json::Value = serde_json::from_slice(body).unwrap();
            assert!(
                !codenotch_teams_schema::validate::contains_forbidden_key(&v),
                "{}",
                r.url
            );
        }
    }

    // What did go out: the closed values, and only them.
    let batch = codenotch_teams_schema::Batch::parse(requests[0].body.as_deref().unwrap()).unwrap();
    let text = serde_json::to_string(&batch).unwrap();
    assert!(text.contains("\"plan\":\"unknown\""));
    assert!(text.contains("\"model\":\"unknown\""));
    assert!(text.contains("\"id\":\"session\"") && text.contains("\"id\":\"seven_day\""));
    assert!(!text.contains("seven_day_opus"), "grouped window left out");
}

#[test]
fn invite_for_another_host_makes_no_request() {
    let http = SeqHttp::new(vec![]);
    let mut settings = codenotch_core::Settings::default();
    // No endpoint and no official origin: pairing is impossible.
    assert!(Origin::admitted(&settings).is_err());
    settings.teams_endpoint = ORIGIN.into();
    let origin = Origin::admitted(&settings).unwrap();
    for link in [
        "https://evil.example.test/j/invite-code-123",
        "http://teams.example.test/j/invite-code-123",
        "https://user@teams.example.test/j/invite-code-123",
        "https://teams.example.test/j/invite-code-123?next=x",
        "https://teams.example.test/j/invite-code-123#x",
    ] {
        assert!(InviteCode::parse(link, &origin).is_err(), "{link}");
    }
    assert!(http.requests().is_empty());
}

#[test]
fn pairing_errors_and_redirects() {
    let origin = Origin::parse(ORIGIN).unwrap();
    let code = InviteCode::parse("invite-code-123", &origin).unwrap();
    let http = SeqHttp::new(vec![
        Reply::Status(404, String::new()),
        Reply::Response(
            codenotch_core::HttpResponse::new(302, Vec::new())
                .with_header("Location", "https://evil.example.test/"),
        ),
        Reply::Status(200, "<html>not json</html>".into()),
        Reply::Status(426, String::new()),
        Reply::Fail,
    ]);
    assert_eq!(
        pairing::preview(&http, &origin, &code),
        Err(PairError::InvalidInvite)
    );
    assert_eq!(
        pairing::preview(&http, &origin, &code),
        Err(PairError::NotATeamsServer)
    );
    assert_eq!(
        pairing::pair(&http, &origin, &code, DEVICE, 1).unwrap_err(),
        PairError::NotATeamsServer
    );
    assert_eq!(
        pairing::pair(&http, &origin, &code, DEVICE, 1).unwrap_err(),
        PairError::UpdateRequired
    );
    assert_eq!(
        pairing::detach(&http, &origin, "t"),
        Err(PairError::Network)
    );
    assert_eq!(
        http.requests().len(),
        5,
        "one request per call, no redirect followed"
    );
    let http = SeqHttp::new(vec![Reply::Status(401, String::new())]);
    assert_eq!(pairing::erase(&http, &origin, "t"), Err(PairError::Revoked));
}

#[test]
fn declaration_is_checked_locally() {
    let mut f = fixture();
    let origin = Origin::parse(ORIGIN).unwrap();
    let http = SeqHttp::new(vec![accept_all()]);
    let policy = Policy::new(&f.config, &f.owners, DEVICE, H0);
    for price in [Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
        assert_eq!(
            pairing::declare_account(
                &http,
                &origin,
                "t",
                &policy,
                "c1",
                TeamsPayer::Member,
                price
            ),
            Err(PairError::Refused)
        );
    }
    f.config.settings.teams_excluded_accounts = vec!["c1".into()];
    let policy = Policy::new(&f.config, &f.owners, DEVICE, H0);
    assert_eq!(
        pairing::declare_account(
            &http,
            &origin,
            "t",
            &policy,
            "c1",
            TeamsPayer::Company,
            None
        ),
        Err(PairError::Refused)
    );
    assert!(http.requests().is_empty());

    let http = SeqHttp::new(vec![Reply::Status(200, "{}".into())]);
    let policy = Policy::new(&fixture().config, &f.owners, DEVICE, H0);
    pairing::declare_account(
        &http,
        &origin,
        "t",
        &policy,
        "x1",
        TeamsPayer::Company,
        None,
    )
    .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(http.requests()[0].body.as_deref().unwrap()).unwrap();
    let mut keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["accountRef", "payer", "priceEurMonth"]);
    assert_eq!(
        body["accountRef"],
        codenotch_teams::account_ref(DEVICE, "x1")
    );
    assert_eq!(body["payer"], "company");
    assert!(body["priceEurMonth"].is_null());
}

#[test]
fn not_paired_means_no_file_and_no_request() {
    let f = fixture();
    assert!(TeamsSync::open(&f.paths).unwrap().is_none());
    assert!(!f.paths.root().exists());
}
