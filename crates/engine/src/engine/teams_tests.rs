//! Teams wiring tests: a hand-driven engine (no engine thread: worker results are pumped by
//! the test) with a fake Teams server behind the shared HTTP client. No provider makes a
//! request here, so every request the fake sees comes from Teams.

use super::*;
use crate::engine::Engine;
use crate::{EngineOptions, Tuning, UpdateOptions};
use codenotch_core::model::{Event, EventSink, LimitWindow, WindowKind};
use codenotch_core::secrets::{MemorySecrets, SecretStore};
use codenotch_core::{
    Account, Config, FakeHttp, HttpError, HttpRequest, HttpResponse, NoopSystem, ProviderKind,
};
use std::collections::VecDeque;
use std::sync::atomic::AtomicI64;
use std::sync::Mutex;
use std::time::Instant;

const T0: i64 = 1_800_000_000;
const ORIGIN: &str = "http://127.0.0.1:47999";
const INVITE_A: &str = "http://127.0.0.1:47999/j/teama-code-0001";
const INVITE_B: &str = "http://127.0.0.1:47999/j/teamb-code-0002";
const WAIT: Duration = Duration::from_secs(10);
const TEAM_A: &str = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
const TEAM_B: &str = "1b2c3d4e-5f6a-4b7c-9d8e-0f1a2b3c4d5e";

// ---------------------------------------------------------------------------------------------
// Fake Teams server (sequenced: scripted statuses for uploads, an optional gate)
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Server {
    log: Mutex<Vec<HttpRequest>>,
    /// Statuses for the next `/v1/events` calls (then 200 accepting every event).
    uploads: Mutex<VecDeque<u16>>,
    /// Requests whose URL contains this path wait for `release` (after signalling `entered`),
    /// then get a 200 (an upload: every event accepted) whatever `uploads` holds.
    gate: Mutex<Option<&'static str>>,
    entered: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl Server {
    fn requests(&self) -> Vec<HttpRequest> {
        self.log.lock().unwrap().clone()
    }

    fn uploads(&self) -> Vec<HttpRequest> {
        self.requests()
            .into_iter()
            .filter(|r| r.url.ends_with("/v1/events"))
            .collect()
    }

    /// Hold requests to `path` until the returned sender is used (or dropped).
    fn hold(
        &self,
        path: &'static str,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *self.gate.lock().unwrap() = Some(path);
        *self.entered.lock().unwrap() = Some(entered_tx);
        *self.release.lock().unwrap() = Some(release_rx);
        (entered_rx, release_tx)
    }
}

fn body(req: &HttpRequest) -> serde_json::Value {
    serde_json::from_slice(req.body.as_deref().unwrap_or(b"{}")).unwrap_or_default()
}

fn bearer(req: &HttpRequest) -> Option<String> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v.trim_start_matches("Bearer ").to_string())
}

fn event_ids(req: &HttpRequest) -> Vec<String> {
    body(req)["events"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e["eventId"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

impl HttpClient for Server {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.log.lock().unwrap().push(req.clone());
        let gated = self
            .gate
            .lock()
            .unwrap()
            .is_some_and(|p| req.url.contains(p));
        if gated {
            *self.gate.lock().unwrap() = None;
            if let Some(tx) = self.entered.lock().unwrap().take() {
                let _ = tx.send(());
            }
            let rx = self.release.lock().unwrap().take();
            if let Some(rx) = rx {
                let _ = rx.recv();
            }
            if req.url.ends_with("/v1/events") {
                let ack = serde_json::json!({ "accepted": event_ids(req), "duplicate": [], "rejected": [] });
                return Ok(HttpResponse::new(200, ack.to_string().into_bytes()));
            }
        }
        let Some(path) = req.url.strip_prefix(ORIGIN) else {
            return Err(HttpError::Connect("unknown host".into()));
        };
        let b = body(req);
        let team_b = b["inviteCode"]
            .as_str()
            .is_some_and(|c| c.starts_with("teamb"));
        let json = |status: u16, v: serde_json::Value| {
            Ok(HttpResponse::new(status, v.to_string().into_bytes()))
        };
        match path {
            "/v1/invites/preview" => json(
                200,
                serde_json::json!({
                    "teamName": if team_b { "Team B" } else { "Team A" },
                    "visibility": "named",
                    "consentVersion": 1
                }),
            ),
            "/v1/devices/pair" => json(
                200,
                serde_json::json!({
                    "deviceToken": if team_b { "tok-B" } else { "tok-A" },
                    "teamId": if team_b { TEAM_B } else { TEAM_A },
                    "teamName": if team_b { "Team B" } else { "Team A" },
                    "dataEpoch": 1,
                    "floorAt": T0 - 3600
                }),
            ),
            "/v1/events" => match self.uploads.lock().unwrap().pop_front() {
                Some(status) => json(status, serde_json::json!({})),
                None => json(
                    200,
                    serde_json::json!({ "accepted": event_ids(req), "duplicate": [], "rejected": [] }),
                ),
            },
            "/v1/devices/self/detach" | "/v1/members/self/erase" | "/v1/accounts/declare" => {
                json(200, serde_json::json!({}))
            }
            _ => json(404, serde_json::json!({})),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

struct Rig {
    e: Engine,
    rx: Receiver<Msg>,
    events: Receiver<Event>,
    clock: Arc<AtomicI64>,
    secrets: Arc<MemorySecrets>,
    _dir: tempfile::TempDir,
    data: std::path::PathBuf,
}

fn claude() -> Account {
    let mut a = Account::new(ProviderKind::Claude, "Claude");
    a.id = "c1".into();
    a
}

fn config(enabled: bool) -> Config {
    let mut c = Config {
        accounts: vec![claude()],
        ..Config::default()
    };
    c.settings.teams_sync_enabled = enabled;
    c.settings.teams_endpoint = ORIGIN.into();
    c
}

fn rig_with(config: Config, http: Arc<dyn HttpClient>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let clock = Arc::new(AtomicI64::new(T0));
    let c = clock.clone();
    let secrets = Arc::new(MemorySecrets::new());
    let opts = EngineOptions {
        paths: Paths::at(data.clone()),
        env: Env::for_test(dir.path()).with_var("CODENOTCH_NO_SPAWN", "1"),
        http,
        system: Arc::new(NoopSystem),
        secrets: secrets.clone(),
        toast: None,
        providers: Some(Arc::new(Vec::new())),
        clock: Some(Arc::new(move || c.load(Ordering::SeqCst))),
        hook_exe: None,
        storage_error: None,
        tuning: Tuning {
            activity: false,
            teams_wake: Duration::from_millis(5),
            ..Tuning::default()
        },
        update: UpdateOptions::default(),
    };
    let (sink, events) = EventSink::channel();
    let (tx, rx) = std::sync::mpsc::channel();
    let (commands, _commands_rx) = std::sync::mpsc::channel();
    let e = Engine::new(
        config,
        opts,
        sink,
        tx,
        commands,
        Arc::new(std::sync::Mutex::new(None)),
    );
    Rig {
        e,
        rx,
        events,
        clock,
        secrets,
        _dir: dir,
        data,
    }
}

fn rig(enabled: bool, server: &Arc<Server>) -> Rig {
    let mut r = rig_with(config(enabled), server.clone());
    r.e.teams_refresh();
    r
}

impl Rig {
    /// Handle worker messages until `done` holds.
    fn pump(&mut self, mut done: impl FnMut(&Engine) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(&self.e) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "condition not reached");
            if let Ok(msg) = self.rx.recv_timeout(left.min(Duration::from_millis(50))) {
                self.e.handle(msg);
            }
        }
    }

    /// Handle worker messages for `ms`.
    fn settle(&mut self, ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while let Ok(msg) = self
            .rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            self.e.handle(msg);
        }
    }

    fn notices(&self) -> Vec<String> {
        self.events
            .try_iter()
            .filter_map(|e| match e {
                Event::Notice(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    fn teams_files(&self) -> Vec<String> {
        std::fs::read_dir(&self.data)
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with("teams"))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn token(&self) -> Option<String> {
        self.secrets.get(&SecretKey::TeamsDeviceToken)
    }

    fn save(&mut self, f: impl FnOnce(&mut Config)) {
        let mut c = self.e.snap.config.clone();
        f(&mut c);
        self.e.command(Command::SaveConfig(Box::new(c)));
    }

    fn pair(&mut self, invite: &str, team: &str) {
        self.e
            .command(Command::TeamsPreviewInvite(SecretValue::new(invite)));
        self.pump(|e| e.snap.teams.preview.is_some());
        assert_eq!(self.e.snap.teams.preview.as_ref().unwrap().team_name, team);
        assert_eq!(self.e.snap.teams.preview.as_ref().unwrap().origin, ORIGIN);
        self.e.command(Command::TeamsPair(SecretValue::new(invite)));
        // The first heartbeat goes out at once; the next upload waits 60 s.
        self.pump(|e| {
            let t = &e.snap.teams;
            t.team_name.as_deref() == Some(team) && t.last_sent_at.is_some() && t.queued == 0
        });
        assert_eq!(self.e.snap.teams.state, TeamsState::Active);
        self.advance(61);
    }

    /// A fresh Claude reading through the engine's fetch path.
    fn reading(&mut self, used: f64) {
        let at = self.clock.load(Ordering::SeqCst);
        let mut r = Reading::fresh(
            &claude(),
            at,
            vec![
                LimitWindow::new("session", "Current session", WindowKind::Session)
                    .used(used)
                    .resets_at(at + 3 * 3600)
                    .window_seconds(18_000),
            ],
        );
        r.plan = Some("Max".into());
        self.e.next_ticket += 1;
        let ticket = self.e.next_ticket;
        self.e
            .in_flight
            .insert("c1".into(), (ticket, std::time::Instant::now()));
        self.e.on_fetched("c1", ticket, r);
    }

    fn advance(&self, secs: i64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }
}

fn snapshots_with_windows(req: &HttpRequest) -> usize {
    body(req)["events"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| {
                    e["kind"] == "quota.snapshot"
                        && e["payload"]["windows"]
                            .as_array()
                            .is_some_and(|w| !w.is_empty())
                })
                .count()
        })
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------------------------
// Gate
// ---------------------------------------------------------------------------------------------

#[test]
fn off_means_no_request_no_file_no_worker() {
    let server = Arc::new(Server::default());
    let mut r = rig(false, &server);
    // Even with a token stored from an earlier pairing.
    r.secrets
        .set(&SecretKey::TeamsDeviceToken, "tok-old")
        .unwrap();
    r.e.teams_refresh();
    r.reading(40.0);
    for cmd in [
        Command::TeamsPreviewInvite(SecretValue::new(INVITE_A)),
        Command::TeamsPair(SecretValue::new(INVITE_A)),
        Command::TeamsFlush,
        Command::TeamsAllowBackfill(true),
        Command::TeamsDeclareAccount {
            account_id: "c1".into(),
            plan: codenotch_core::PlanCode::Unknown,
            payer: TeamsPayer::Company,
            price_eur_month: None,
        },
        Command::TeamsEraseServerData,
    ] {
        r.e.command(cmd);
    }
    r.settle(100);
    assert!(server.requests().is_empty());
    assert!(r.teams_files().is_empty());
    assert!(!r.e.teams.worker_running());
    assert_eq!(r.e.snap.teams, TeamsStatus::default());
    assert!(r.notices().iter().any(|n| n == MSG_TURN_ON));
}

#[test]
fn on_but_not_paired_or_without_token_makes_no_request() {
    let server = Arc::new(Server::default());
    // On, no token, no teams.json.
    let mut r = rig(true, &server);
    r.reading(40.0);
    r.e.command(Command::TeamsFlush);
    r.e.command(Command::TeamsEraseServerData);
    r.e.command(Command::TeamsDetach);
    r.settle(50);
    assert_eq!(r.e.snap.teams.state, TeamsState::NotPaired);
    assert!(!r.e.teams.worker_running());
    // A token without teams.json: still not paired, no worker.
    r.secrets
        .set(&SecretKey::TeamsDeviceToken, "tok-x")
        .unwrap();
    r.e.teams_refresh();
    r.reading(41.0);
    r.settle(50);
    assert_eq!(r.e.snap.teams.state, TeamsState::NotPaired);
    assert!(!r.e.teams.worker_running());
    assert!(server.requests().is_empty());
    assert!(r.teams_files().is_empty());
}

#[test]
fn once_never_touches_teams() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path().join("data"));
    let http = Arc::new(FakeHttp::new());
    let secrets = Arc::new(MemorySecrets::new());
    secrets.set(&SecretKey::TeamsDeviceToken, "tok-A").unwrap();
    let opts = EngineOptions {
        paths: paths.clone(),
        env: Env::for_test(dir.path()).with_var("CODENOTCH_NO_SPAWN", "1"),
        http: http.clone(),
        system: Arc::new(NoopSystem),
        secrets,
        toast: None,
        providers: Some(Arc::new(Vec::new())),
        clock: None,
        hook_exe: None,
        storage_error: None,
        tuning: Tuning::default(),
        update: UpdateOptions::default(),
    };
    let out = crate::fetch_once(&config(true), &opts, T0);
    assert_eq!(out.len(), 1);
    assert!(http.requests().is_empty());
    assert!(!paths.teams_state().exists() && !paths.teams_outbox().exists());
}

// ---------------------------------------------------------------------------------------------
// Pairing and uploads
// ---------------------------------------------------------------------------------------------

#[test]
fn pairing_and_uploads_go_to_the_admitted_origin_only() {
    let server = Arc::new(Server::default());
    let mut r = rig(true, &server);
    // An invite for another server is refused before any request.
    r.e.command(Command::TeamsPreviewInvite(SecretValue::new(
        "https://elsewhere.example.test/j/teama-code-0001",
    )));
    r.e.command(Command::TeamsPair(SecretValue::new(INVITE_A)));
    assert!(server.requests().is_empty());
    let notices = r.notices();
    assert!(notices.iter().any(|n| n == MSG_OTHER_SERVER), "{notices:?}");
    assert!(
        notices.iter().any(|n| n == MSG_PREVIEW_FIRST),
        "{notices:?}"
    );

    r.pair(INVITE_A, "Team A");
    assert_eq!(r.token().as_deref(), Some("tok-A"));
    assert!(r.data.join("teams.json").is_file());
    let before = server.uploads().len();
    r.reading(40.0);
    r.pump(|_| server.uploads().len() > before);
    r.pump(|e| e.snap.teams.queued == 0);
    for req in server.requests() {
        assert!(req.url.starts_with(ORIGIN), "{}", req.url);
    }
    let upload = &server.uploads()[before];
    assert_eq!(bearer(upload).as_deref(), Some("tok-A"));
    assert_eq!(snapshots_with_windows(upload), 1);
    // The invite code went out only in the two invite calls, never with the token.
    assert!(server
        .requests()
        .iter()
        .filter(|q| bearer(q).is_some())
        .all(|q| !String::from_utf8_lossy(q.body.as_deref().unwrap_or(b"")).contains("teama")));
    // Already paired: a new invite is refused locally.
    r.e.command(Command::TeamsPreviewInvite(SecretValue::new(INVITE_B)));
    assert!(r.notices().iter().any(|n| n == MSG_DETACH_FIRST));
    r.e.teams_shutdown();
}

#[test]
fn uploads_are_spaced_and_respect_the_backoff() {
    let server = Arc::new(Server::default());
    let mut r = rig(true, &server);
    r.pair(INVITE_A, "Team A");
    server.uploads.lock().unwrap().extend([503]);
    let before = server.uploads().len();
    r.reading(40.0);
    r.pump(|e| e.snap.teams.state == TeamsState::Paused);
    let after_failure = server.uploads().len();
    assert_eq!(after_failure, before + 1);
    // New observations do not cause a request before the backoff deadline.
    r.reading(55.0);
    r.settle(150);
    assert_eq!(server.uploads().len(), after_failure);
    // Past the deadline (at most 30 s + 20 %), one upload.
    r.advance(61);
    r.pump(|e| e.snap.teams.state == TeamsState::Active && e.snap.teams.queued == 0);
    assert_eq!(server.uploads().len(), after_failure + 1);
    r.e.teams_shutdown();
}

// ---------------------------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------------------------

#[test]
fn turning_sync_off_during_an_upload_drops_its_result() {
    let server = Arc::new(Server::default());
    let mut r = rig(true, &server);
    r.pair(INVITE_A, "Team A");
    let (entered, release) = server.hold("/v1/events");
    let before = server.uploads().len();
    r.reading(40.0);
    entered.recv_timeout(WAIT).expect("upload started");
    r.save(|c| c.settings.teams_sync_enabled = false);
    assert_eq!(r.e.snap.teams, TeamsStatus::default());
    assert!(!r.e.teams.worker_running());
    drop(release);
    r.settle(200);
    assert_eq!(
        r.e.snap.teams,
        TeamsStatus::default(),
        "late result ignored"
    );
    assert_eq!(server.uploads().len(), before + 1);
    // Turning off is not detaching: the pairing and the token stay.
    assert_eq!(r.token().as_deref(), Some("tok-A"));
    assert!(r.data.join("teams.json").is_file());
    // Back on: the pairing is reopened.
    r.save(|c| c.settings.teams_sync_enabled = true);
    r.pump(|e| e.snap.teams.team_name.as_deref() == Some("Team A"));
    r.e.teams_shutdown();
}

#[test]
fn a_late_pairing_result_is_ignored() {
    let server = Arc::new(Server::default());
    let mut r = rig(true, &server);
    r.e.command(Command::TeamsPreviewInvite(SecretValue::new(INVITE_A)));
    r.pump(|e| e.snap.teams.preview.is_some());
    let (entered, release) = server.hold("/v1/devices/pair");
    r.e.command(Command::TeamsPair(SecretValue::new(INVITE_A)));
    entered.recv_timeout(WAIT).expect("pairing started");
    // Off and on again while the pairing request runs: a new generation.
    r.save(|c| c.settings.teams_sync_enabled = false);
    r.save(|c| c.settings.teams_sync_enabled = true);
    drop(release);
    r.settle(200);
    assert_eq!(r.token(), None, "token of a stale pairing never stored");
    assert!(r.teams_files().is_empty());
    assert_eq!(r.e.snap.teams.state, TeamsState::NotPaired);
    assert!(!r.e.snap.teams.busy);
    r.e.teams_shutdown();
}

#[test]
fn detach_then_pair_with_another_team_sends_nothing_queued_for_the_first() {
    let server = Arc::new(Server::default());
    let mut r = rig(true, &server);
    r.pair(INVITE_A, "Team A");
    server.uploads.lock().unwrap().extend([503, 503, 503]);
    r.reading(40.0);
    r.pump(|e| e.snap.teams.queued > 0 && e.snap.teams.state == TeamsState::Paused);
    let team_a: Vec<String> = server.uploads().iter().flat_map(event_ids).collect();
    assert!(!team_a.is_empty());

    r.e.command(Command::TeamsDetach);
    assert_eq!(r.token(), None);
    r.pump(|e| !e.snap.teams.busy);
    assert!(r.teams_files().is_empty(), "{:?}", r.teams_files());
    let detach = server
        .requests()
        .into_iter()
        .find(|q| q.url.ends_with("/v1/devices/self/detach"))
        .expect("detach call");
    assert_eq!(bearer(&detach).as_deref(), Some("tok-A"));
    assert_eq!(r.e.snap.teams.state, TeamsState::NotPaired);

    server.uploads.lock().unwrap().clear();
    r.pair(INVITE_B, "Team B");
    assert_eq!(r.token().as_deref(), Some("tok-B"));
    r.reading(60.0);
    r.pump(|_| {
        server
            .uploads()
            .iter()
            .any(|q| bearer(q).as_deref() == Some("tok-B"))
    });
    for q in server.uploads() {
        if bearer(&q).as_deref() == Some("tok-B") {
            assert!(event_ids(&q).iter().all(|id| !team_a.contains(id)));
        }
    }
    r.e.teams_shutdown();
}

#[test]
fn an_account_excluded_after_enqueue_is_purged_before_sending() {
    let server = Arc::new(Server::default());
    let mut r = rig(true, &server);
    r.pair(INVITE_A, "Team A");
    server.uploads.lock().unwrap().extend([503]);
    r.reading(40.0);
    r.pump(|e| e.snap.teams.state == TeamsState::Paused);
    assert_eq!(snapshots_with_windows(server.uploads().last().unwrap()), 1);
    r.save(|c| c.settings.teams_excluded_accounts = vec!["c1".into()]);
    // A reading of the excluded account after the change is not queued either.
    r.reading(70.0);
    r.advance(120);
    r.pump(|e| e.snap.teams.state == TeamsState::Active && e.snap.teams.queued == 0);
    let last = server.uploads().last().cloned().unwrap();
    assert_eq!(snapshots_with_windows(&last), 0, "{}", body(&last));
    r.e.teams_shutdown();
}

#[test]
fn an_exclusion_stops_uploads_before_queued_observations_are_handled() {
    let server = Arc::new(Server::default());
    let mut r = rig(true, &server);
    r.pair(INVITE_A, "Team A");
    // The worker is blocked in an upload of a reading still allowed.
    let (entered, release) = server.hold("/v1/events");
    r.reading(40.0);
    entered.recv_timeout(WAIT).expect("upload started");
    let held = server.uploads().len();
    // Observations queue up behind it, then the account is excluded.
    r.reading(45.0);
    r.reading(50.0);
    r.save(|c| c.settings.teams_excluded_accounts = vec!["c1".into()]);
    // Next upload due as soon as the worker is free again.
    r.advance(120);
    drop(release);
    r.pump(|e| e.snap.teams.state == TeamsState::Active && e.snap.teams.queued == 0);
    r.settle(100);
    assert!(server.uploads().len() > held, "the tombstone went out");
    for q in &server.uploads()[held..] {
        assert_eq!(snapshots_with_windows(q), 0, "{}", body(q));
    }
    r.e.teams_shutdown();
}

#[test]
fn off_and_on_during_an_upload_keeps_one_writer() {
    let server = Arc::new(Server::default());
    let mut r = rig(true, &server);
    r.pair(INVITE_A, "Team A");
    // The old worker is blocked in an upload (that will be accepted) …
    let (entered, release) = server.hold("/v1/events");
    r.reading(40.0);
    entered.recv_timeout(WAIT).expect("upload started");
    // … while sync goes off and on: a new worker, whose uploads fail.
    r.save(|c| c.settings.teams_sync_enabled = false);
    r.save(|c| c.settings.teams_sync_enabled = true);
    server.uploads.lock().unwrap().extend([503; 8]);
    r.advance(120);
    r.reading(60.0);
    r.settle(200);
    drop(release);
    r.pump(|e| e.snap.teams.state == TeamsState::Paused && e.snap.teams.queued > 0);
    // Let the old worker finish with its answer.
    r.settle(300);
    r.e.teams_shutdown();
    // The old worker's late answer changed no file: the second snapshot is still queued and
    // its sequence number is recorded, so it is neither lost nor reused.
    let paths = Paths::at(r.data.clone());
    let state = codenotch_teams::state::StateFile::load(&paths.teams_state())
        .unwrap()
        .expect("still paired");
    assert_eq!(state.snapshots.len(), 1);
    assert_eq!(state.snapshots[0].seq, 2);
    let sync = TeamsSync::open(&paths).unwrap().expect("still paired");
    assert!(sync.queued() >= 1);
}

#[test]
fn a_full_queue_drops_observations_without_blocking() {
    let server = Arc::new(Server::default());
    let mut r = rig(true, &server);
    r.pair(INVITE_A, "Team A");
    // Keep the worker busy in an upload so nothing is drained.
    let (entered, release) = server.hold("/v1/events");
    r.reading(10.0);
    entered.recv_timeout(WAIT).expect("upload started");
    for i in 0..(QUEUE_CAP + 10) {
        r.e.teams_observe_reading(&{
            let mut x = Reading::fresh(&claude(), T0, Vec::new());
            x.plan = Some(format!("{i}"));
            x
        });
    }
    assert_eq!(r.e.teams.dropped, 10);
    drop(release);
    r.e.teams_shutdown();
}

#[test]
fn declaration_sends_the_plan_code() {
    let server = Arc::new(Server::default());
    let mut r = rig(true, &server);
    r.pair(INVITE_A, "Team A");
    r.e.command(Command::TeamsDeclareAccount {
        account_id: "c1".into(),
        plan: codenotch_core::PlanCode::Max,
        payer: TeamsPayer::Member,
        price_eur_month: Some(90.0),
    });
    let declared = |s: &Server| {
        s.requests()
            .into_iter()
            .find(|q| q.url.ends_with("/v1/accounts/declare"))
    };
    r.pump(|_| declared(&server).is_some());
    let b = body(&declared(&server).unwrap());
    assert_eq!(b["plan"], "max");
    assert_eq!(b["payer"], "member");
    assert_eq!(b["priceEurMonth"], 90.0);
    r.e.teams_shutdown();
}

#[test]
fn every_message_has_french() {
    for key in MESSAGES {
        assert_ne!(
            crate::i18n::tr(codenotch_core::Lang::Fr, key),
            *key,
            "no French for {key}"
        );
    }
}
