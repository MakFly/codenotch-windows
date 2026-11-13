//! Shared test helpers: a sequenced HTTP double and a paired fixture.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Mutex;

use codenotch_core::analytics::{HourlyUsage, RootOwners};
use codenotch_core::{
    Account, Config, HttpClient, HttpError, HttpRequest, HttpResponse, Paths, ProviderKind,
    TokenCounts,
};
use codenotch_teams::{Origin, PairingInfo, TeamsSync};

pub const DEVICE: &str = "6f1c2a9e-0b7d-4a51-9c7e-2f0d7b1a4e11";
pub const TEAM_ID: &str = "3b2f0c4e-5d6a-4b7c-8d9e-0f1a2b3c4d5e";
pub const ORIGIN: &str = "https://teams.example.test";
/// Aligned hour: the pairing floor.
pub const H0: i64 = 1_794_553_200;
pub const NOW: i64 = H0 + 100 * 3600;

/// Answers requests in order from a script (`core::FakeHttp` answers per URL prefix and
/// cannot say "500, then 200"). Records every request. Once the script is empty every
/// request fails to connect.
pub enum Reply {
    Status(u16, String),
    Response(HttpResponse),
    /// Build the reply from the request (to acknowledge its event ids).
    With(Box<dyn Fn(&HttpRequest) -> HttpResponse + Send + Sync>),
    Fail,
}

#[derive(Default)]
pub struct SeqHttp {
    script: Mutex<VecDeque<Reply>>,
    log: Mutex<Vec<HttpRequest>>,
}

impl SeqHttp {
    pub fn new(script: Vec<Reply>) -> Self {
        Self {
            script: Mutex::new(script.into()),
            log: Mutex::new(Vec::new()),
        }
    }
    pub fn push(&self, reply: Reply) {
        self.script.lock().unwrap().push_back(reply);
    }
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.log.lock().unwrap().clone()
    }
}

impl HttpClient for SeqHttp {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.log.lock().unwrap().push(request.clone());
        match self.script.lock().unwrap().pop_front() {
            Some(Reply::Status(code, body)) => Ok(HttpResponse::new(code, body.into_bytes())),
            Some(Reply::Response(r)) => Ok(r),
            Some(Reply::With(f)) => Ok(f(request)),
            Some(Reply::Fail) | None => Err(HttpError::Connect("scripted".into())),
        }
    }
}

/// Event ids of the batch in `request`.
pub fn batch_ids(request: &HttpRequest) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["eventId"].as_str().unwrap().to_string())
        .collect()
}

/// A 200 reply accepting every event of the request.
pub fn accept_all() -> Reply {
    Reply::With(Box::new(|req| {
        let ids = batch_ids(req);
        HttpResponse::new(
            200,
            serde_json::json!({ "accepted": ids, "duplicate": [], "rejected": [] }).to_string(),
        )
    }))
}

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub paths: Paths,
    pub config: Config,
    pub owners: Vec<RootOwners>,
}

pub fn account(kind: ProviderKind, id: &str) -> Account {
    let mut a = Account::new(kind, "name");
    a.id = id.into();
    a
}

pub fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path().join("data"));
    let config = Config {
        accounts: vec![
            account(ProviderKind::Claude, "c1"),
            account(ProviderKind::Codex, "x1"),
        ],
        ..Default::default()
    };
    let owners = vec![
        RootOwners {
            source: "R-claude".into(),
            provider: ProviderKind::Claude,
            accounts: vec!["c1".into()],
        },
        RootOwners {
            source: "R-codex".into(),
            provider: ProviderKind::Codex,
            accounts: vec!["x1".into()],
        },
    ];
    Fixture {
        dir,
        paths,
        config,
        owners,
    }
}

pub fn info(team: &str) -> PairingInfo {
    PairingInfo {
        origin: Origin::parse(ORIGIN).unwrap(),
        team_id: team.into(),
        team_name: format!("Team {team}"),
        device_id: DEVICE.into(),
        generation: 1,
        consent_version: 1,
        data_epoch: 1,
        floor_at: H0,
    }
}

pub fn start(f: &Fixture) -> TeamsSync {
    let mut s = TeamsSync::create(&f.paths, info("a")).unwrap();
    s.set_policy(&f.config, &f.owners, NOW).unwrap();
    s
}

pub fn claude(hour: i64, input: u64) -> HourlyUsage {
    HourlyUsage {
        hour,
        source: "R-claude".into(),
        provider: ProviderKind::Claude,
        model: "claude-opus-5-5".into(),
        tokens: TokenCounts {
            input,
            ..Default::default()
        },
        cache_write_1h: 0,
    }
}

/// Queue `n` usage events (distinct hours).
pub fn queue(s: &mut TeamsSync, n: usize) {
    let rows: Vec<HourlyUsage> = (0..n)
        .map(|i| claude(NOW - (i as i64 + 1) * 3600, 10 + i as u64))
        .collect();
    assert_eq!(s.observe_usage(&rows, NOW).unwrap(), n);
}
