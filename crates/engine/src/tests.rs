//! Engine integration tests: the real engine thread with fake providers, a fake clock,
//! `FakeHttp`, `MemorySecrets` and a temp data folder. No network, no real profile folders.

use super::*;
use codenotch_core::model::{
    AlertKind, Event, LimitWindow, ReadingStatus, UpdateState, WindowKind,
};
use codenotch_core::{FakeHttp, MemorySecrets, NoopSystem, Snapshot};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Instant;

const T0: i64 = 1_800_000_000;
const WAIT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Outcome {
    Percent(f64),
    RateLimited(Option<i64>),
    Error,
}

#[derive(Debug, Clone)]
struct Call {
    at: i64,
    manual: bool,
    previous: Option<Reading>,
}

struct FakeProvider {
    kind: ProviderKind,
    min: Duration,
    found: Vec<Account>,
    script: Mutex<VecDeque<Outcome>>,
    calls: Mutex<Vec<Call>>,
    delay: Duration,
}

impl FakeProvider {
    fn new(kind: ProviderKind, min_secs: u64) -> Arc<Self> {
        Arc::new(Self {
            kind,
            min: Duration::from_secs(min_secs),
            found: Vec::new(),
            script: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
            delay: Duration::ZERO,
        })
    }
    fn with(
        kind: ProviderKind,
        min_secs: u64,
        found: Vec<Account>,
        script: &[Outcome],
        delay_ms: u64,
    ) -> Arc<Self> {
        Arc::new(Self {
            kind,
            min: Duration::from_secs(min_secs),
            found,
            script: Mutex::new(script.iter().cloned().collect()),
            calls: Mutex::new(Vec::new()),
            delay: Duration::from_millis(delay_ms),
        })
    }
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

/// Delegates to a shared fake so the test keeps a handle on it.
struct Shared(Arc<FakeProvider>);

impl Provider for Shared {
    fn kind(&self) -> ProviderKind {
        self.0.kind.clone()
    }
    fn discover(&self, _env: &Env) -> Vec<Account> {
        self.0
            .found
            .iter()
            .map(|a| Account {
                id: Account::new(a.provider.clone(), "x").id,
                ..a.clone()
            })
            .collect()
    }
    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
        let f = &self.0;
        f.calls.lock().unwrap().push(Call {
            at: ctx.now,
            manual: ctx.manual,
            previous: ctx.previous.clone(),
        });
        if !f.delay.is_zero() {
            std::thread::sleep(f.delay);
        }
        let outcome = f
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Outcome::Percent(10.0));
        match outcome {
            Outcome::Percent(p) => {
                let id = account
                    .provider
                    .headline_candidates()
                    .first()
                    .copied()
                    .unwrap_or("w");
                let mut r = Reading::fresh(
                    account,
                    ctx.now,
                    vec![LimitWindow::new(id, "Current session", WindowKind::Session)
                        .used(p)
                        .resets_at(ctx.now + 3 * 3600)
                        .window_seconds(5 * 3600)],
                );
                r.plan = Some("Max".into());
                r
            }
            Outcome::RateLimited(until) => {
                let mut r = ctx
                    .previous
                    .clone()
                    .unwrap_or_else(|| Reading::new(account, ctx.now));
                r.status = ReadingStatus::RateLimited { until };
                r
            }
            Outcome::Error => Reading::error(account, ctx.now, "boom"),
        }
    }
    fn min_interval(&self) -> Duration {
        self.0.min
    }
}

#[derive(Default)]
struct RecordingSystem {
    urls: Mutex<Vec<String>>,
    launch: AtomicBool,
}

impl SystemActions for RecordingSystem {
    fn open_url(&self, url: &str) -> codenotch_core::Result<()> {
        self.urls.lock().unwrap().push(url.to_string());
        Ok(())
    }
    fn open_path(&self, _path: &std::path::Path) -> codenotch_core::Result<()> {
        Ok(())
    }
    fn set_launch_at_startup(&self, enabled: bool) -> codenotch_core::Result<()> {
        self.launch.store(enabled, Ordering::SeqCst);
        Ok(())
    }
    fn launch_at_startup(&self) -> bool {
        self.launch.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

fn fast_tuning() -> Tuning {
    Tuning {
        tick: Duration::from_millis(15),
        max_workers: 4,
        stagger_secs: 0,
        save_debounce: Duration::ZERO,
        analytics_delay: None,
        analytics_every: Duration::from_secs(3600),
        update_delay: None,
        update_every: Duration::from_secs(3600),
        prune_delay: Duration::from_secs(3600),
        prune_every: Duration::from_secs(3600),
        publish_every: Duration::from_secs(3600),
        fetch_stall: Duration::from_secs(60),
        activity: false,
    }
}

fn test_env(dir: &std::path::Path) -> Env {
    Env::for_test(dir).with_var("CODENOTCH_NO_SPAWN", "1")
}

fn options(
    dir: &std::path::Path,
    providers: Vec<Arc<FakeProvider>>,
    clock: &Arc<AtomicI64>,
) -> EngineOptions {
    let c = clock.clone();
    EngineOptions {
        paths: Paths::at(dir.join("data")),
        env: test_env(dir),
        http: Arc::new(FakeHttp::new()),
        system: Arc::new(NoopSystem),
        secrets: Arc::new(MemorySecrets::new()),
        toast: None,
        providers: Some(Arc::new(
            providers
                .into_iter()
                .map(|p| Box::new(Shared(p)) as Box<dyn Provider>)
                .collect(),
        )),
        clock: Some(Arc::new(move || c.load(Ordering::SeqCst))),
        hook_exe: None,
        storage_error: None,
        tuning: fast_tuning(),
        update: UpdateOptions::default(),
    }
}

fn clock() -> Arc<AtomicI64> {
    Arc::new(AtomicI64::new(T0))
}

struct Running {
    handle: Option<EngineHandle>,
    tx: Sender<Command>,
    events: Receiver<Event>,
}

impl Running {
    fn start(config: Config, opts: EngineOptions) -> Self {
        let (sink, events) = EventSink::channel();
        let handle = start(config, opts, std::sync::mpsc::channel(), sink);
        let tx = handle.commands();
        Self {
            handle: Some(handle),
            tx,
            events,
        }
    }
    fn send(&self, c: Command) {
        self.tx.send(c).unwrap();
    }
    fn handle(&self) -> &EngineHandle {
        self.handle.as_ref().unwrap()
    }
    /// Next event matching `pred` (others are skipped).
    fn event<T>(&self, mut pred: impl FnMut(&Event) -> Option<T>) -> T {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(e) => {
                    if let Some(t) = pred(&e) {
                        return t;
                    }
                }
                Err(_) => panic!("timed out waiting for an event"),
            }
        }
    }
    fn snapshot(&self, mut pred: impl FnMut(&Snapshot) -> bool) -> Snapshot {
        self.event(|e| match e {
            Event::SnapshotChanged(s) if pred(s) => Some((**s).clone()),
            _ => None,
        })
    }
    fn notice(&self) -> String {
        self.event(|e| match e {
            Event::Notice(n) => Some(n.clone()),
            _ => None,
        })
    }
    /// No event matching `pred` within `ms`.
    fn quiet(&self, ms: u64, mut pred: impl FnMut(&Event) -> bool) {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while let Ok(e) = self
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            assert!(!pred(&e), "unexpected event {e:?}");
        }
    }
    fn stop(mut self) {
        if let Some(h) = self.handle.take() {
            h.shutdown();
        }
    }
}

fn wait_until(mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !f() {
        assert!(Instant::now() < deadline, "condition not reached");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn reading_of<'a>(s: &'a Snapshot, id: &str) -> Option<&'a Reading> {
    s.account(id).and_then(|v| v.reading.as_ref())
}

fn config_with(accounts: Vec<Account>) -> Config {
    Config {
        accounts,
        ..Config::default()
    }
}

fn fresh_reading(account: &Account, at: i64, used: f64) -> Reading {
    let id = account
        .provider
        .headline_candidates()
        .first()
        .copied()
        .unwrap_or("w");
    Reading::fresh(
        account,
        at,
        vec![LimitWindow::new(id, "Current session", WindowKind::Session)
            .used(used)
            .resets_at(at + 3 * 3600)],
    )
}

// ---------------------------------------------------------------------------------------------
// Static helpers
// ---------------------------------------------------------------------------------------------

#[test]
fn every_kind_but_other_has_a_provider() {
    let reg = registry();
    for kind in ProviderKind::CANONICAL_ORDER {
        assert!(
            reg.iter().any(|p| &p.kind() == kind),
            "missing provider {kind}"
        );
    }
}

#[test]
fn validate_rejects_duplicate_or_empty_ids() {
    let a = Account::new(ProviderKind::Claude, "a");
    assert!(engine::validate(&config_with(vec![a.clone()])).is_ok());
    assert!(engine::validate(&config_with(vec![a.clone(), a.clone()])).is_err());
    let mut empty = a;
    empty.id = " ".into();
    assert!(engine::validate(&config_with(vec![empty])).is_err());
}

// ---------------------------------------------------------------------------------------------
// fetch_once
// ---------------------------------------------------------------------------------------------

#[test]
fn fetch_once_returns_one_reading_per_enabled_account() {
    let dir = tempfile::tempdir().unwrap();
    let mut opts = options(dir.path(), Vec::new(), &clock());
    opts.providers = None; // the real providers, on an empty temp machine
    let mut config = Config::default();
    config
        .accounts
        .push(Account::new(ProviderKind::Claude, "a"));
    let mut off = Account::new(ProviderKind::Codex, "b");
    off.enabled = false;
    config.accounts.push(off);
    config
        .accounts
        .push(Account::new(ProviderKind::Other("zzz".into()), "c"));
    let out = fetch_once(&config, &opts, 100);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].1.status, ReadingStatus::NotInstalled);
    assert_eq!(out[1].1.status, ReadingStatus::Error);
    assert!(!dir.path().join("data").exists(), "--once writes nothing");
}

#[test]
fn fetch_once_runs_in_parallel_with_a_global_timeout_and_includes_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let slow = FakeProvider::with(ProviderKind::Cursor, 300, Vec::new(), &[], 3_000);
    let quick = FakeProvider::with(
        ProviderKind::Grok,
        300,
        vec![Account::new(ProviderKind::Grok, "Grok")],
        &[Outcome::Percent(5.0)],
        50,
    );
    let quick2 = FakeProvider::with(
        ProviderKind::Codex,
        300,
        Vec::new(),
        &[Outcome::Percent(7.0)],
        50,
    );
    let opts = options(dir.path(), vec![slow, quick.clone(), quick2], &clock());
    let config = config_with(vec![
        Account::new(ProviderKind::Cursor, "Cursor"),
        Account::new(ProviderKind::Codex, "Codex"),
    ]);
    let t = Instant::now();
    let out = fetch_once_within(&config, &opts, T0, Duration::from_millis(600));
    assert!(t.elapsed() < Duration::from_millis(2_500));
    let names: Vec<_> = out.iter().map(|(a, _)| a.name.as_str()).collect();
    assert_eq!(
        names,
        ["Cursor", "Codex", "Grok"],
        "discovered account appended"
    );
    assert_eq!(out[0].1.error.as_deref(), Some("timed out"));
    assert_eq!(out[1].1.headline_percent(), Some(7.0));
    assert_eq!(out[2].1.headline_percent(), Some(5.0));
    assert!(quick.calls()[0].manual);
    assert!(!dir.path().join("data").exists(), "--once writes nothing");
}

#[test]
fn fetch_once_respects_a_persisted_backoff_and_passes_previous() {
    let dir = tempfile::tempdir().unwrap();
    let claude = FakeProvider::new(ProviderKind::Claude, 300);
    let codex = FakeProvider::new(ProviderKind::Codex, 300);
    let opts = options(dir.path(), vec![claude.clone(), codex.clone()], &clock());
    let a = Account::new(ProviderKind::Claude, "c");
    let b = Account::new(ProviderKind::Codex, "x");
    let mut state = PersistedState::default();
    state
        .readings
        .insert(a.id.clone(), fresh_reading(&a, T0 - 100, 40.0));
    state
        .readings
        .insert(b.id.clone(), fresh_reading(&b, T0 - 100, 20.0));
    state.backoff_until.insert(a.id.clone(), T0 + 500);
    state.backoff_until.insert(b.id.clone(), T0 + 500);
    state.save(&opts.paths.state()).unwrap();
    let before = std::fs::read(opts.paths.state()).unwrap();
    let out = fetch_once(&config_with(vec![a, b]), &opts, T0);
    assert!(
        claude.calls().is_empty(),
        "no request inside a Claude back-off"
    );
    assert_eq!(
        out[0].1.status,
        ReadingStatus::RateLimited {
            until: Some(T0 + 500)
        }
    );
    assert_eq!(out[0].1.headline_percent(), Some(40.0));
    let calls = codex.calls();
    assert_eq!(
        calls.len(),
        1,
        "Codex still reads (locally) during a back-off"
    );
    assert_eq!(
        calls[0].previous.as_ref().map(|r| r.status.clone()),
        Some(ReadingStatus::Stale)
    );
    assert_eq!(
        std::fs::read(opts.paths.state()).unwrap(),
        before,
        "state untouched"
    );
}

// ---------------------------------------------------------------------------------------------
// Engine thread
// ---------------------------------------------------------------------------------------------

#[test]
fn engine_publishes_saves_and_quits() {
    let dir = tempfile::tempdir().unwrap();
    let mut o = options(dir.path(), Vec::new(), &clock());
    o.tuning.activity = true; // real activity services on the temp machine
    let cfg_path = o.paths.config();
    let r = Running::start(Config::default(), o);
    r.snapshot(|_| true);
    let mut cfg = Config::default();
    cfg.settings.language = "fr".into();
    r.send(Command::SaveConfig(Box::new(cfg)));
    r.snapshot(|s| s.config.settings.language == "fr");
    assert!(cfg_path.exists());
    let key = codenotch_core::SecretKey::TelegramToken;
    r.send(Command::SetSecret {
        key: key.clone(),
        value: "t".into(),
    });
    r.snapshot(|s| s.secrets.telegram_token && !s.secrets.discord_webhook);
    r.send(Command::ClearSecret(key));
    r.snapshot(|s| !s.secrets.telegram_token);
    r.send(Command::ToggleNotch);
    r.snapshot(|s| s.notch_hidden);
    r.send(Command::DesktopState {
        fullscreen: true,
        game: false,
    });
    r.snapshot(|s| s.fullscreen && !s.game_mode);
    r.send(Command::OpenSettings);
    r.event(|e| matches!(e, Event::OpenSettings).then_some(()));
    let t = Instant::now();
    r.send(Command::Quit);
    r.event(|e| matches!(e, Event::Quit).then_some(()));
    r.stop();
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "quit is prompt (activity stopped)"
    );
}

#[test]
fn discovery_adds_new_accounts_once_and_they_get_fetched() {
    let dir = tempfile::tempdir().unwrap();
    let mine = Account::new(ProviderKind::Claude, "My Claude");
    let claude = FakeProvider::with(
        ProviderKind::Claude,
        300,
        vec![Account::new(ProviderKind::Claude, "Claude")],
        &[],
        0,
    );
    let codex = FakeProvider::with(
        ProviderKind::Codex,
        300,
        vec![Account::new(ProviderKind::Codex, "Codex")],
        &[Outcome::Percent(12.0)],
        0,
    );
    let o = options(dir.path(), vec![claude.clone(), codex.clone()], &clock());
    let cfg_path = o.paths.config();
    let r = Running::start(config_with(vec![mine.clone()]), o);
    let s =
        r.snapshot(|s| s.accounts.len() == 2 && reading_of(s, &s.accounts[1].account.id).is_some());
    assert_eq!(s.accounts[0].account, mine, "existing account untouched");
    assert_eq!(s.accounts[1].account.name, "Codex");
    assert_eq!(
        reading_of(&s, &s.accounts[1].account.id)
            .unwrap()
            .headline_percent(),
        Some(12.0)
    );
    let saved = Config::load(&cfg_path).unwrap();
    assert_eq!(saved.accounts.len(), 2);
    assert_eq!(saved.accounts[0], mine);
    // Refresh(None) re-runs discovery: nothing new.
    r.send(Command::Refresh(None));
    wait_until(|| codex.calls().len() >= 2);
    r.quiet(
        200,
        |e| matches!(e, Event::SnapshotChanged(s) if s.accounts.len() != 2),
    );
    assert_eq!(Config::load(&cfg_path).unwrap().accounts.len(), 2);
    assert_eq!(
        claude.calls().len(),
        1,
        "Claude manual refresh stays behind the 300 s floor"
    );
    r.stop();
}

#[test]
fn fresh_reading_is_persisted_to_state_and_history() {
    let dir = tempfile::tempdir().unwrap();
    let codex = FakeProvider::with(
        ProviderKind::Codex,
        300,
        Vec::new(),
        &[Outcome::Percent(33.0)],
        0,
    );
    let o = options(dir.path(), vec![codex], &clock());
    let paths = o.paths.clone();
    let a = Account::new(ProviderKind::Codex, "Codex");
    let r = Running::start(config_with(vec![a.clone()]), o);
    r.snapshot(|s| reading_of(s, &a.id).is_some_and(|x| x.status == ReadingStatus::Fresh));
    wait_until(|| {
        PersistedState::load(&paths.state())
            .readings
            .contains_key(&a.id)
    });
    let history = History::new(paths.history()).read_all();
    assert_eq!(history.len(), 1);
    assert_eq!(
        (
            history[0].account_id.as_str(),
            history[0].used_percent,
            history[0].at
        ),
        (a.id.as_str(), 33.0, T0)
    );
    let state = PersistedState::load(&paths.state());
    assert_eq!(state.extra["lastAttemptAt"][&a.id], T0);
    r.stop();
}

use codenotch_core::History;

#[test]
fn fresh_reading_carries_a_period_forecast_from_history() {
    let dir = tempfile::tempdir().unwrap();
    let codex = FakeProvider::with(
        ProviderKind::Codex,
        300,
        Vec::new(),
        &[Outcome::Percent(33.0)],
        0,
    );
    let o = options(dir.path(), vec![codex], &clock());
    let paths = o.paths.clone();
    let a = Account::new(ProviderKind::Codex, "Codex");
    // The fake reading's session started 2 h before T0 and resets 3 h after it: 16.5 %/h.
    std::fs::create_dir_all(paths.root()).unwrap();
    let reset = T0 + 3 * 3600;
    let points: Vec<_> = (0..12)
        .map(|k| codenotch_core::HistoryPoint {
            account_id: a.id.clone(),
            label: "Current session".into(),
            at: T0 - 2 * 3600 + k * 600,
            used_percent: (k as f64 * 600.0 * 16.5 / 3600.0).floor(),
            resets_at: Some(reset),
        })
        .collect();
    History::new(paths.history()).append(&points).unwrap();
    let r = Running::start(config_with(vec![a.clone()]), o);
    let s = r.snapshot(|s| reading_of(s, &a.id).is_some_and(|x| x.status == ReadingStatus::Fresh));
    let f = reading_of(&s, &a.id).unwrap().windows[0]
        .forecast
        .expect("forecast");
    assert_eq!(f.basis, codenotch_core::forecast::Basis::Trajectory);
    assert_eq!(f.exhaust_at, None);
    assert!((75.0..90.0).contains(&f.at_reset_percent), "{f:?}");
    r.stop();
}

#[test]
fn claude_manual_refresh_waits_for_the_floor_and_gets_previous() {
    let dir = tempfile::tempdir().unwrap();
    let clock = clock();
    let claude = FakeProvider::with(
        ProviderKind::Claude,
        300,
        Vec::new(),
        &[Outcome::Percent(20.0), Outcome::Percent(25.0)],
        0,
    );
    let a = Account::new(ProviderKind::Claude, "c");
    let r = Running::start(
        config_with(vec![a.clone()]),
        options(dir.path(), vec![claude.clone()], &clock),
    );
    r.snapshot(|s| {
        reading_of(s, &a.id).is_some()
            && s.accounts[0].next_attempt_at == Some(T0 + 300)
            && !s.accounts[0].refreshing
    });
    r.send(Command::Refresh(Some(a.id.clone())));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        claude.calls().len(),
        1,
        "manual refresh inside the floor waits"
    );
    clock.store(T0 + 300, Ordering::SeqCst);
    wait_until(|| claude.calls().len() == 2);
    let calls = claude.calls();
    assert!(!calls[0].manual && calls[1].manual);
    assert_eq!(calls[1].at, T0 + 300);
    assert_eq!(
        calls[1]
            .previous
            .as_ref()
            .and_then(|p| p.headline_percent()),
        Some(20.0)
    );
    r.snapshot(|s| reading_of(s, &a.id).and_then(|x| x.headline_percent()) == Some(25.0));
    r.stop();
}

#[test]
fn claude_429_backoff_is_persisted_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let clock = clock();
    let claude = FakeProvider::with(
        ProviderKind::Claude,
        300,
        Vec::new(),
        &[Outcome::RateLimited(Some(T0 + 60))],
        0,
    );
    let a = Account::new(ProviderKind::Claude, "c");
    let o = options(dir.path(), vec![claude.clone()], &clock);
    let paths = o.paths.clone();
    let r = Running::start(config_with(vec![a.clone()]), o);
    r.snapshot(|s| s.accounts[0].next_attempt_at == Some(T0 + 60));
    wait_until(|| {
        PersistedState::load(&paths.state())
            .backoff_until
            .get(&a.id)
            == Some(&(T0 + 60))
    });
    r.stop();
    assert_eq!(
        PersistedState::load(&paths.state()).extra["claude429"][&a.id],
        1
    );

    // Restart 30 s later: still inside the deadline, nothing is fetched.
    clock.store(T0 + 30, Ordering::SeqCst);
    let claude2 = FakeProvider::with(
        ProviderKind::Claude,
        300,
        Vec::new(),
        &[Outcome::RateLimited(Some(T0 + 400))],
        0,
    );
    let r = Running::start(
        config_with(vec![a.clone()]),
        options(dir.path(), vec![claude2.clone()], &clock),
    );
    r.snapshot(|s| s.accounts[0].next_attempt_at == Some(T0 + 60));
    std::thread::sleep(Duration::from_millis(150));
    assert!(claude2.calls().is_empty());
    // At the deadline the floor (300 s after the last attempt) still applies.
    clock.store(T0 + 60, Ordering::SeqCst);
    r.snapshot(|s| s.accounts[0].next_attempt_at == Some(T0 + 300));
    assert!(claude2.calls().is_empty());
    clock.store(T0 + 300, Ordering::SeqCst);
    wait_until(|| claude2.calls().len() == 1);
    // Second consecutive 429 (count restored): 120 s, or the provider's later deadline.
    r.snapshot(|s| s.accounts[0].next_attempt_at == Some(T0 + 420));
    r.stop();
}

#[test]
fn restored_claude_reading_is_stale_and_keeps_the_floor() {
    let dir = tempfile::tempdir().unwrap();
    let clock = clock();
    let claude = FakeProvider::new(ProviderKind::Claude, 300);
    let a = Account::new(ProviderKind::Claude, "c");
    let o = options(dir.path(), vec![claude.clone()], &clock);
    let mut state = PersistedState::default();
    state
        .readings
        .insert(a.id.clone(), fresh_reading(&a, T0 - 100, 55.0));
    state.save(&o.paths.state()).unwrap();
    let r = Running::start(config_with(vec![a.clone()]), o);
    let s = r.snapshot(|s| s.accounts[0].next_attempt_at.is_some());
    assert_eq!(s.accounts[0].next_attempt_at, Some(T0 + 200));
    assert_eq!(reading_of(&s, &a.id).unwrap().status, ReadingStatus::Stale);
    std::thread::sleep(Duration::from_millis(150));
    assert!(claude.calls().is_empty());
    r.stop();
}

#[test]
fn alerts_need_two_fresh_readings_and_reach_the_toast() {
    let dir = tempfile::tempdir().unwrap();
    let clock = clock();
    let codex = FakeProvider::with(
        ProviderKind::Codex,
        300,
        Vec::new(),
        &[
            Outcome::Percent(85.0),
            Outcome::Percent(96.0),
            Outcome::Error,
        ],
        0,
    );
    let a = Account::new(ProviderKind::Codex, "Codex");
    let o = options(dir.path(), vec![codex.clone()], &clock);
    // A restored 70 % reading is never a baseline.
    let mut state = PersistedState::default();
    state
        .readings
        .insert(a.id.clone(), fresh_reading(&a, T0 - 60, 70.0));
    state.save(&o.paths.state()).unwrap();
    let paths = o.paths.clone();
    let r = Running::start(config_with(vec![a.clone()]), o);
    let toast = Arc::new(codenotch_notify::RecordingToast::new());
    r.handle().set_toast(Some(toast.clone()));
    r.snapshot(|s| {
        reading_of(s, &a.id).and_then(|x| x.headline_percent()) == Some(85.0) && x_fresh(s, &a.id)
    });
    r.quiet(100, |e| matches!(e, Event::Alert(_)));
    clock.store(T0 + 30, Ordering::SeqCst);
    r.send(Command::Refresh(Some(a.id.clone())));
    let alert = r.event(|e| match e {
        Event::Alert(al) => Some(al.clone()),
        _ => None,
    });
    assert_eq!(alert.kind, AlertKind::Critical);
    assert_eq!(alert.account_id, a.id);
    wait_until(|| !toast.shown().is_empty());
    wait_until(|| {
        PersistedState::load(&paths.state())
            .alerts_sent
            .keys()
            .any(|k| k.contains("/critical/"))
    });
    // An error reading is processed too (no alert).
    clock.store(T0 + 60, Ordering::SeqCst);
    r.send(Command::Refresh(Some(a.id.clone())));
    r.snapshot(|s| reading_of(s, &a.id).is_some_and(|x| x.status == ReadingStatus::Error));
    r.quiet(100, |e| matches!(e, Event::Alert(_)));
    r.stop();
}

fn x_fresh(s: &Snapshot, id: &str) -> bool {
    reading_of(s, id).is_some_and(|x| x.status == ReadingStatus::Fresh)
}

#[test]
fn in_flight_fetches_are_deduplicated_and_shown_as_refreshing() {
    let dir = tempfile::tempdir().unwrap();
    let codex = FakeProvider::with(ProviderKind::Codex, 300, Vec::new(), &[], 400);
    let a = Account::new(ProviderKind::Codex, "Codex");
    let r = Running::start(
        config_with(vec![a.clone()]),
        options(dir.path(), vec![codex.clone()], &clock()),
    );
    r.snapshot(|s| s.accounts[0].refreshing);
    for _ in 0..5 {
        r.send(Command::Refresh(Some(a.id.clone())));
    }
    r.snapshot(|s| !s.accounts[0].refreshing && reading_of(s, &a.id).is_some());
    wait_until(|| codex.calls().len() == 2);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        codex.calls().len(),
        2,
        "one queued manual refresh, not five"
    );
    r.stop();
}

#[test]
fn unknown_providers_get_an_error_reading_and_disabled_accounts_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let codex = FakeProvider::new(ProviderKind::Codex, 300);
    let other = Account::new(ProviderKind::Other("deepseek".into()), "d");
    let mut off = Account::new(ProviderKind::Codex, "off");
    off.enabled = false;
    let r = Running::start(
        config_with(vec![other.clone(), off.clone()]),
        options(dir.path(), vec![codex.clone()], &clock()),
    );
    let s = r.snapshot(|s| reading_of(s, &other.id).is_some());
    assert_eq!(
        reading_of(&s, &other.id).unwrap().status,
        ReadingStatus::Error
    );
    assert_eq!(s.account(&off.id).unwrap().next_attempt_at, None);
    std::thread::sleep(Duration::from_millis(100));
    assert!(codex.calls().is_empty());
    r.stop();
}

#[test]
fn save_config_validates_applies_and_drops_removed_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let custom = FakeProvider::new(ProviderKind::Custom, 300);
    let grok = FakeProvider::new(ProviderKind::Grok, 300);
    let mut o = options(dir.path(), vec![custom.clone(), grok.clone()], &clock());
    let system = Arc::new(RecordingSystem::default());
    o.system = system.clone();
    let paths = o.paths.clone();
    // A custom endpoint: the only kind of account a SaveConfig may remove.
    let mut a = Account::new(ProviderKind::Custom, "Mine");
    a.endpoint = "https://usage.example.test/u".into();
    let r = Running::start(config_with(vec![a.clone()]), o);
    r.snapshot(|s| reading_of(s, &a.id).is_some());
    wait_until(|| {
        PersistedState::load(&paths.state())
            .readings
            .contains_key(&a.id)
    });

    // Invalid: duplicate ids.
    r.send(Command::SaveConfig(Box::new(config_with(vec![
        a.clone(),
        a.clone(),
    ]))));
    assert!(r.notice().contains("same id"));
    assert!(!paths.config().exists());

    // Valid: account replaced, launch at startup applied.
    let g = Account::new(ProviderKind::Grok, "Grok");
    let mut cfg = config_with(vec![g.clone()]);
    cfg.settings.launch_at_startup = true;
    r.send(Command::SaveConfig(Box::new(cfg)));
    let s = r.snapshot(|s| {
        s.accounts.len() == 1 && s.accounts[0].account.id == g.id && reading_of(s, &g.id).is_some()
    });
    assert!(s.account(&a.id).is_none());
    assert!(system.launch_at_startup());
    assert_eq!(
        Config::load(&paths.config()).unwrap().accounts,
        vec![g.clone()]
    );
    wait_until(|| {
        let st = PersistedState::load(&paths.state());
        !st.readings.contains_key(&a.id) && st.readings.contains_key(&g.id)
    });
    assert_eq!(grok.calls().len(), 1);
    r.stop();
}

#[test]
fn save_config_from_a_stale_copy_keeps_engine_owned_state() {
    let dir = tempfile::tempdir().unwrap();
    let codex = FakeProvider::new(ProviderKind::Codex, 300);
    let grok = FakeProvider::new(ProviderKind::Grok, 300);
    let custom = FakeProvider::new(ProviderKind::Custom, 300);
    let o = options(dir.path(), vec![codex, grok, custom], &clock());
    let paths = o.paths.clone();
    // Engine side: Codex (e.g. found by discovery after the UI's snapshot), a custom endpoint,
    // Grok, hooks on.
    let c = Account::new(ProviderKind::Codex, "Codex");
    let mut x = Account::new(ProviderKind::Custom, "Mine");
    x.endpoint = "https://usage.example.test/u".into();
    let g = Account::new(ProviderKind::Grok, "Grok");
    let mut cfg = config_with(vec![c.clone(), x.clone(), g.clone()]);
    cfg.settings.hooks_enabled = true;
    let r = Running::start(cfg.clone(), o);
    r.snapshot(|s| s.accounts.len() == 3);

    // (a) hooksEnabled from the UI is ignored, (b) Codex missing from the UI copy is kept,
    // (c) unknown keys and ordinary settings are taken from the UI copy.
    let mut stale = config_with(vec![g.clone()]);
    stale.settings.hooks_enabled = false;
    stale.settings.poll_seconds = 777;
    stale
        .extra
        .insert("futureKey".into(), serde_json::json!({"keep": true}));
    stale
        .settings
        .extra
        .insert("futureSetting".into(), serde_json::json!(7));
    r.send(Command::SaveConfig(Box::new(stale)));
    let s = r.snapshot(|s| s.config.settings.poll_seconds == 777);
    assert!(
        s.config.settings.hooks_enabled,
        "hooksEnabled is engine-owned"
    );
    let ids: Vec<&str> = s.config.accounts.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![c.id.as_str(), g.id.as_str()],
        "Codex kept at its place, custom endpoint removed"
    );
    let saved = Config::load(&paths.config()).unwrap();
    assert!(saved.settings.hooks_enabled);
    assert_eq!(saved.accounts.len(), 2);
    assert_eq!(
        saved.extra.get("futureKey"),
        Some(&serde_json::json!({"keep": true}))
    );
    assert_eq!(
        saved.settings.extra.get("futureSetting"),
        Some(&serde_json::json!(7))
    );

    // An edit of a kept account (turned off) still applies.
    let mut edited = s.config.clone();
    edited.accounts[0].enabled = false;
    r.send(Command::SaveConfig(Box::new(edited)));
    let s = r.snapshot(|s| s.config.account(&c.id).is_some_and(|a| !a.enabled));
    assert_eq!(s.config.accounts.len(), 2);
    r.stop();
}

#[test]
fn keep_engine_owned_rules() {
    let dir = tempfile::tempdir().unwrap();
    let env = codenotch_core::Env::for_test(dir.path());
    let c = Account::new(ProviderKind::Codex, "Codex");
    let cl = Account::new(ProviderKind::Claude, "Claude");
    let mut x = Account::new(ProviderKind::Custom, "Mine");
    x.endpoint = "https://usage.example.test/u".into();
    let mut overridden = Account::new(ProviderKind::Cursor, "Cursor via endpoint");
    overridden.endpoint = "https://gw.example.test/u".into();
    let mut current = config_with(vec![cl.clone(), x.clone(), c.clone(), overridden.clone()]);
    current.settings.hooks_enabled = false;
    let mut incoming = config_with(vec![]);
    incoming.settings.hooks_enabled = true;
    crate::engine::keep_engine_owned(&current, &mut incoming, &env);
    assert!(!incoming.settings.hooks_enabled);
    let ids: Vec<&str> = incoming.accounts.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![cl.id.as_str(), c.id.as_str()],
        "custom-endpoint accounts are deletable, others kept"
    );
    // Present accounts keep the incoming version.
    let mut incoming = config_with(vec![Account {
        name: "Renamed".into(),
        ..c.clone()
    }]);
    crate::engine::keep_engine_owned(&current, &mut incoming, &env);
    assert_eq!(incoming.account(&c.id).unwrap().name, "Renamed");
    assert_eq!(incoming.accounts.len(), 2);
    // A discovered account removed in the UI (marker sent along) stays removed.
    let mut incoming = config_with(vec![c.clone()]);
    incoming
        .settings
        .removed_accounts
        .push(codenotch_core::config::RemovedAccount::of(&cl).unwrap());
    crate::engine::keep_engine_owned(&current, &mut incoming, &env);
    let ids: Vec<&str> = incoming.accounts.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, vec![c.id.as_str()]);
}

#[test]
fn a_hung_fetch_thread_is_not_doubled_and_shows_an_error() {
    let dir = tempfile::tempdir().unwrap();
    // Every fetch blocks for 1.5 s; the engine gives up after 150 ms.
    let codex = FakeProvider::with(ProviderKind::Codex, 300, Vec::new(), &[], 1500);
    let a = Account::new(ProviderKind::Codex, "Codex");
    let mut o = options(dir.path(), vec![codex.clone()], &clock());
    o.tuning.fetch_stall = Duration::from_millis(150);
    let r = Running::start(config_with(vec![a.clone()]), o);
    wait_until(|| codex.calls().len() == 1);
    let s = r.snapshot(|s| reading_of(s, &a.id).is_some_and(|x| x.status == ReadingStatus::Error));
    assert_eq!(
        reading_of(&s, &a.id).unwrap().error.as_deref(),
        Some(crate::engine::MSG_FETCH_STUCK)
    );
    // Manual refreshes while the stuck thread lives: no second thread.
    for _ in 0..3 {
        r.send(Command::Refresh(Some(a.id.clone())));
    }
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(
        codex.calls().len(),
        1,
        "no new fetch thread while the previous one is alive"
    );
    // Once it really ends, the queued manual refresh runs.
    wait_until(|| codex.calls().len() == 2);
    r.stop();
}

/// `open_url` that takes a while, then fails.
#[derive(Default)]
struct SlowSystem;

impl SystemActions for SlowSystem {
    fn open_url(&self, _url: &str) -> codenotch_core::Result<()> {
        std::thread::sleep(Duration::from_millis(1500));
        Err(codenotch_core::Error::other(
            "the system could not open it (code 2)",
        ))
    }
    fn open_path(&self, _path: &std::path::Path) -> codenotch_core::Result<()> {
        std::thread::sleep(Duration::from_millis(1500));
        Err(codenotch_core::Error::other("cannot open folder"))
    }
    fn set_launch_at_startup(&self, _enabled: bool) -> codenotch_core::Result<()> {
        Ok(())
    }
    fn launch_at_startup(&self) -> bool {
        false
    }
}

#[test]
fn opening_links_and_folders_never_blocks_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let mut o = options(dir.path(), Vec::new(), &clock());
    o.system = Arc::new(SlowSystem);
    let r = Running::start(Config::default(), o);
    r.snapshot(|_| true);
    let sent = Instant::now();
    r.send(Command::OpenUrl("https://claude.ai/settings/usage".into()));
    r.send(Command::OpenDataFolder);
    r.send(Command::ToggleNotch);
    r.snapshot(|s| s.notch_hidden);
    assert!(
        sent.elapsed() < Duration::from_millis(1000),
        "engine answered while the shell was busy"
    );
    let mut got = vec![r.notice(), r.notice()];
    got.sort();
    assert_eq!(
        got,
        vec![
            "cannot open folder".to_string(),
            "the system could not open it (code 2)".to_string()
        ]
    );
    r.stop();
}

#[test]
fn recentre_and_open_url() {
    let dir = tempfile::tempdir().unwrap();
    let mut o = options(dir.path(), Vec::new(), &clock());
    let system = Arc::new(RecordingSystem::default());
    o.system = system.clone();
    let paths = o.paths.clone();
    let mut cfg = Config::default();
    cfg.settings.widget_placement.monitor = r"\\.\DISPLAY9".into();
    cfg.settings.widget_placement.y = 0.9;
    let r = Running::start(cfg, o);
    r.send(Command::Recentre);
    let s = r.snapshot(|s| s.config.settings.widget_placement.y == 0.5);
    assert!(s.config.settings.widget_placement.monitor.is_empty());
    let saved = Config::load(&paths.config()).unwrap();
    assert_eq!(
        (
            saved.settings.widget_placement.monitor.as_str(),
            saved.settings.widget_placement.y
        ),
        ("", 0.5)
    );
    r.send(Command::OpenUrl(
        "file:///C:/Windows/system32/calc.exe".into(),
    ));
    assert!(r.notice().contains("web links"));
    r.send(Command::OpenUrl("https://claude.ai/settings/usage".into()));
    wait_until(|| system.urls.lock().unwrap().len() == 1);
    assert_eq!(
        system.urls.lock().unwrap()[0],
        "https://claude.ai/settings/usage"
    );
    r.stop();
}

#[test]
fn preview_reset_card_uses_the_first_notch_account() {
    let dir = tempfile::tempdir().unwrap();
    let r0 = Running::start(Config::default(), options(dir.path(), Vec::new(), &clock()));
    r0.send(Command::PreviewResetCard);
    assert!(r0.notice().contains("No account"));
    r0.stop();

    let codex = FakeProvider::with(
        ProviderKind::Codex,
        300,
        Vec::new(),
        &[Outcome::Percent(42.0)],
        0,
    );
    let a = Account::new(ProviderKind::Codex, "Codex");
    let r = Running::start(
        config_with(vec![a.clone()]),
        options(dir.path(), vec![codex], &clock()),
    );
    r.snapshot(|s| reading_of(s, &a.id).is_some());
    r.send(Command::PreviewResetCard);
    let (acc, win, used) = r.event(|e| match e {
        Event::ResetDetected {
            account_id,
            window_id,
            used_percent,
            ..
        } => Some((account_id.clone(), window_id.clone(), *used_percent)),
        _ => None,
    });
    assert_eq!(
        (acc.as_str(), win.as_str(), used),
        (a.id.as_str(), "primary", Some(42.0))
    );
    r.stop();
}

#[test]
fn test_messages_report_the_outcome_without_network() {
    let dir = tempfile::tempdir().unwrap();
    let o = options(dir.path(), Vec::new(), &clock());
    let r = Running::start(Config::default(), o);
    r.send(Command::TestTelegram);
    let n = r.notice();
    assert!(!n.is_empty());
    r.send(Command::TestDiscord);
    let n2 = r.notice();
    assert!(!n2.is_empty());
    r.stop();
}

fn update_manifest(version: &str, zip: &[u8]) -> String {
    format!(
        r#"{{"version":"{version}","notes":"New things.","notes_url":"https://github.com/MakFly/codenotch-windows/releases/tag/v{version}",
            "windows_x64":{{"url":"https://example.com/CodeNotch-{version}.zip","sha256":"{}","size":{}}}}}"#,
        crate::update::stage::sha256_hex(zip),
        zip.len()
    )
}

fn update_options(dir: &std::path::Path, http: FakeHttp) -> EngineOptions {
    let mut o = options(dir, Vec::new(), &clock());
    o.http = Arc::new(http);
    o.update.enabled = true;
    o
}

#[test]
fn update_check_states() {
    let dir = tempfile::tempdir().unwrap();
    let url = codenotch_core::DEFAULT_UPDATE_MANIFEST_URL;
    let o = update_options(
        dir.path(),
        FakeHttp::new().on(url, 200, &update_manifest("99.0.0", b"zip")),
    );
    let r = Running::start(Config::default(), o);
    r.send(Command::CheckUpdate);
    let s = r.snapshot(|s| matches!(s.update, UpdateState::Available { .. }));
    assert!(matches!(
        &s.update,
        UpdateState::Available { version, manual: true, forced: false, notes_url: Some(_), .. }
            if version == "99.0.0"
    ));
    assert_eq!(s.update_checked_at, Some(T0));
    r.stop();

    // Automatic check (notify mode by default), nothing published yet.
    let mut o = update_options(dir.path(), FakeHttp::new().on(url, 404, "{}"));
    o.tuning.update_delay = Some(Duration::from_millis(10));
    let r = Running::start(Config::default(), o);
    r.snapshot(|s| {
        matches!(&s.update, UpdateState::Failed { message, manual: false }
            if message.contains("No update information"))
    });
    r.stop();

    // A ≤ 1.0 config with "updateChecks": false is "off": no automatic check, the button works.
    let http = Arc::new(FakeHttp::new().on(
        url,
        200,
        &format!(r#"{{"version":"{}"}}"#, codenotch_core::VERSION),
    ));
    let mut o = update_options(dir.path(), FakeHttp::new());
    o.http = http.clone();
    o.tuning.update_delay = Some(Duration::from_millis(10));
    let cfg = Config::parse(r#"{"settings":{"updateChecks":false}}"#).unwrap();
    let r = Running::start(cfg, o);
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        http.requests().is_empty(),
        "no automatic check when updates are off"
    );
    r.send(Command::CheckUpdate);
    r.snapshot(|s| {
        matches!(s.update, UpdateState::UpToDate { checked_at, manual: true } if checked_at == T0)
    });
    r.stop();
}

#[test]
fn update_manifest_url_setting_and_windows_notification() {
    let dir = tempfile::tempdir().unwrap();
    let url = "https://updates.example.com/codenotch.json";
    let mut o = update_options(
        dir.path(),
        FakeHttp::new().on(url, 200, &update_manifest("99.0.0", b"zip")),
    );
    o.tuning.update_delay = Some(Duration::from_millis(10));
    let toast = Arc::new(codenotch_notify::RecordingToast::new());
    o.toast = Some(toast.clone());
    let mut cfg = Config::default();
    cfg.settings.update_manifest_url = url.into();
    let r = Running::start(cfg, o);
    r.snapshot(|s| matches!(&s.update, UpdateState::Available { manual: false, .. }));
    wait_until(|| !toast.shown().is_empty());
    let shown = toast.shown();
    assert_eq!(shown.len(), 1);
    assert!(shown[0].0.contains("99.0.0"), "{shown:?}");
    // A manual check of the same version shows no second notification.
    r.send(Command::CheckUpdate);
    r.snapshot(|s| matches!(&s.update, UpdateState::Available { manual: true, .. }));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(toast.shown().len(), 1);
    r.stop();

    // Notifications off: none.
    let mut o = update_options(
        dir.path(),
        FakeHttp::new().on(url, 200, &update_manifest("99.0.0", b"zip")),
    );
    o.tuning.update_delay = Some(Duration::from_millis(10));
    let toast = Arc::new(codenotch_notify::RecordingToast::new());
    o.toast = Some(toast.clone());
    let mut cfg = Config::default();
    cfg.settings.update_manifest_url = url.into();
    cfg.settings.notifications = false;
    let r = Running::start(cfg, o);
    r.snapshot(|s| matches!(&s.update, UpdateState::Available { .. }));
    std::thread::sleep(Duration::from_millis(100));
    assert!(toast.shown().is_empty());
    r.stop();
}

#[test]
fn install_update_swaps_relaunches_and_quits() {
    let dir = tempfile::tempdir().unwrap();
    let install = dir.path().join("Programs").join("CodeNotch");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("codenotch.exe"), b"MZ old app").unwrap();
    let zip = crate::update::stage::tests_package();
    let http = FakeHttp::new()
        .on(
            codenotch_core::DEFAULT_UPDATE_MANIFEST_URL,
            200,
            &update_manifest("99.0.0", &zip),
        )
        .on_response(
            "https://example.com/CodeNotch-99.0.0.zip",
            codenotch_core::HttpResponse::new(200, zip),
        );
    let mut o = update_options(dir.path(), http);
    let launched: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let rec = launched.clone();
    o.update.exe = Some(install.join("codenotch.exe"));
    o.update.args = vec!["--silent".into()];
    o.update.relaunch = Some(Arc::new(
        move |_: &std::path::Path, args: &[String]| -> std::io::Result<()> {
            rec.lock().unwrap().push(args.to_vec());
            Ok(())
        },
    ));
    let r = Running::start(Config::default(), o);
    r.send(Command::CheckUpdate);
    r.snapshot(|s| matches!(s.update, UpdateState::Available { .. }));
    r.send(Command::InstallUpdate);
    r.event(|e| matches!(e, Event::Quit).then_some(()));
    assert_eq!(
        std::fs::read(install.join("codenotch.exe")).unwrap(),
        b"MZ new app"
    );
    assert_eq!(
        std::fs::read(install.join("codenotch-hook.exe")).unwrap(),
        b"MZ new hook"
    );
    let launched = launched.lock().unwrap().clone();
    assert_eq!(launched.len(), 1);
    assert_eq!(launched[0][0], "--silent");
    assert_eq!(
        launched[0][1],
        format!("--updated-from={}", codenotch_core::VERSION)
    );
    r.stop();
}

#[test]
fn hooks_install_and_remove_in_temp_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let mut o = options(dir.path(), Vec::new(), &clock());
    let paths = o.paths.clone();
    // No profile, no helper.
    let r = Running::start(Config::default(), o.clone());
    r.send(Command::InstallHooks);
    assert!(r.notice().contains("No Claude Code profile"));
    r.stop();

    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let custom = dir.path().join("elsewhere").join("claude-work");
    std::fs::create_dir_all(&custom).unwrap();
    let mut work = Account::new(ProviderKind::Claude, "work");
    work.profile_path = custom.to_string_lossy().into_owned();
    let mut cfg = config_with(vec![work]);
    cfg.settings.hook_port = 0; // ephemeral port: never the real app's 48666

    o.hook_exe = Some(dir.path().join("bin").join("codenotch-hook.exe"));
    let r = Running::start(cfg.clone(), o.clone());
    r.send(Command::InstallHooks);
    assert!(
        r.notice().contains("not found"),
        "missing helper is refused"
    );
    r.stop();

    let exe = dir.path().join("bin").join("codenotch-hook.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"").unwrap();
    o.tuning.activity = true;
    let r = Running::start(cfg, o);
    r.send(Command::InstallHooks);
    assert_eq!(r.notice(), "Hooks installed.");
    r.snapshot(|s| s.config.settings.hooks_enabled);
    assert!(
        codenotch_activity::hooks_installed(&test_env(dir.path())),
        "doctor's probe sees them"
    );
    assert!(
        Config::load(&paths.config())
            .unwrap()
            .settings
            .hooks_enabled
    );
    for d in [home.join(".claude"), custom.clone()] {
        let text = std::fs::read_to_string(d.join("settings.json")).unwrap();
        assert!(text.contains("codenotch-hook"), "{}", d.display());
    }
    r.send(Command::RemoveHooks);
    assert_eq!(r.notice(), "Hooks removed.");
    r.snapshot(|s| !s.config.settings.hooks_enabled);
    assert!(
        !Config::load(&paths.config())
            .unwrap()
            .settings
            .hooks_enabled
    );
    assert!(!codenotch_activity::hooks_installed(&test_env(dir.path())));
    for d in [home.join(".claude"), custom] {
        let text = std::fs::read_to_string(d.join("settings.json")).unwrap();
        assert!(!text.contains("codenotch-hook"));
    }
    r.stop();
}

#[test]
fn sign_in_errors_become_notices_and_a_failed_add_is_rolled_back() {
    let dir = tempfile::tempdir().unwrap();
    let cursor = FakeProvider::new(ProviderKind::Cursor, 300);
    let o = options(dir.path(), vec![cursor], &clock());
    let paths = o.paths.clone();
    let c = Account::new(ProviderKind::Cursor, "Cursor");
    let r = Running::start(config_with(vec![c.clone()]), o);
    r.send(Command::SignIn(c.id.clone()));
    assert!(r.notice().to_lowercase().contains("sign-in"));
    r.send(Command::SignIn("nope".into()));
    assert!(r.notice().contains("Unknown account"));

    // CODENOTCH_NO_SPAWN=1: the new profile cannot sign in → account and folder removed again.
    r.send(Command::AddClaudeAccount);
    assert!(r.notice().contains("CODENOTCH_NO_SPAWN"));
    let s = r.snapshot(|s| s.accounts.len() == 1);
    assert_eq!(s.accounts[0].account.id, c.id);
    assert_eq!(Config::load(&paths.config()).unwrap().accounts.len(), 1);
    assert!(!dir.path().join("home").join(".claude-2").exists());
    r.stop();
}

#[test]
fn analytics_on_demand_fills_snapshot_and_accessor() {
    let dir = tempfile::tempdir().unwrap();
    let o = options(dir.path(), Vec::new(), &clock());
    let r = Running::start(Config::default(), o);
    assert!(r.handle().analytics_report().is_none());
    r.send(Command::RefreshAnalytics);
    let s = r.snapshot(|s| s.analytics.is_some());
    assert_eq!(s.analytics.as_ref().unwrap().days, 7);
    wait_until(|| r.handle().analytics_report().is_some());
    assert_eq!(
        r.handle().analytics_report().unwrap().summary,
        s.analytics.unwrap()
    );
    assert!(r.handle().analytics_source().lock().unwrap().is_some());
    r.stop();
}

#[test]
fn state_is_flushed_on_shutdown_when_debounced() {
    let dir = tempfile::tempdir().unwrap();
    let codex = FakeProvider::new(ProviderKind::Codex, 300);
    let mut o = options(dir.path(), vec![codex.clone()], &clock());
    o.tuning.save_debounce = Duration::from_secs(3600);
    let paths = o.paths.clone();
    let a = Account::new(ProviderKind::Codex, "Codex");
    let r = Running::start(config_with(vec![a.clone()]), o);
    r.snapshot(|s| reading_of(s, &a.id).is_some());
    assert!(!paths.state().exists(), "debounced");
    r.stop();
    assert!(PersistedState::load(&paths.state())
        .readings
        .contains_key(&a.id));
}

#[test]
fn history_is_pruned_to_history_hours() {
    let dir = tempfile::tempdir().unwrap();
    let mut o = options(dir.path(), Vec::new(), &clock());
    o.tuning.prune_delay = Duration::ZERO;
    let paths = o.paths.clone();
    std::fs::create_dir_all(paths.root()).unwrap();
    let h = History::new(paths.history());
    let point = |at| codenotch_core::HistoryPoint {
        account_id: "a".into(),
        label: "Current session".into(),
        at,
        used_percent: 1.0,
        resets_at: None,
    };
    h.append(&[point(T0 - 30 * 3600), point(T0 - 3600)])
        .unwrap();
    let r = Running::start(Config::default(), o);
    wait_until(|| History::new(paths.history()).read_all().len() == 1);
    r.stop();
}

#[test]
fn stagger_spreads_first_fetches() {
    let dir = tempfile::tempdir().unwrap();
    let clock = clock();
    let cursor = FakeProvider::new(ProviderKind::Cursor, 300);
    let mut o = options(dir.path(), vec![cursor.clone()], &clock);
    o.tuning.stagger_secs = 5;
    let accounts: Vec<Account> = (0..3)
        .map(|i| Account::new(ProviderKind::Cursor, format!("c{i}")))
        .collect();
    let r = Running::start(config_with(accounts), o);
    wait_until(|| cursor.calls().len() == 1);
    let s = r.snapshot(|s| s.accounts[1].next_attempt_at == Some(T0 + 5));
    assert_eq!(s.accounts[2].next_attempt_at, Some(T0 + 10));
    clock.store(T0 + 10, Ordering::SeqCst);
    wait_until(|| cursor.calls().len() == 3);
    r.stop();
}

#[test]
fn analytics_detail_reaches_the_snapshot_summary() {
    use codenotch_analytics::{AnalyticsReport, CliUsage, DayReport, ModelReport, Usage};
    use codenotch_core::model::{DailyUsage, ModelUsage, ProviderUsage, TokenCounts};
    let tokens = |n: u64| TokenCounts {
        input: n,
        ..Default::default()
    };
    let usage = |n: u64, cost: f64, unpriced: u64| Usage {
        tokens: tokens(n),
        cost_usd: cost,
        unpriced_tokens: unpriced,
        ..Default::default()
    };
    let mut report = AnalyticsReport {
        total: usage(300, 1.5, 100),
        per_cli: vec![CliUsage {
            provider: ProviderKind::Claude,
            usage: usage(300, 1.5, 100),
            sessions: 2,
        }],
        per_model: vec![
            ModelReport {
                model: "known".into(),
                provider: ProviderKind::Claude,
                priced: true,
                usage: usage(200, 1.5, 0),
            },
            ModelReport {
                model: "mystery".into(),
                provider: ProviderKind::Claude,
                priced: false,
                usage: usage(100, 0.0, 100),
            },
        ],
        per_day: vec![DayReport {
            date: "2026-11-12".into(),
            usage: usage(300, 1.5, 100),
            per_cli: vec![CliUsage {
                provider: ProviderKind::Claude,
                usage: usage(300, 1.5, 100),
                sessions: 2,
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    report.summary = codenotch_core::AnalyticsSummary {
        days: 7,
        generated_at: T0,
        total_tokens: tokens(300),
        total_cost_usd: 1.5,
        per_provider: vec![ProviderUsage {
            provider: ProviderKind::Claude,
            tokens: tokens(300),
            cost_usd: 1.5,
            sessions: 2,
            ..Default::default()
        }],
        per_day: vec![DailyUsage {
            date: "2026-11-12".into(),
            provider: ProviderKind::Claude,
            tokens: tokens(300),
            cost_usd: 1.5,
            ..Default::default()
        }],
        per_model: vec![
            ModelUsage {
                model: "known".into(),
                provider: ProviderKind::Claude,
                tokens: tokens(200),
                cost_usd: 1.5,
                ..Default::default()
            },
            ModelUsage {
                model: "mystery".into(),
                provider: ProviderKind::Claude,
                tokens: tokens(100),
                cost_usd: 0.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let s = summary_from_report(&report);
    assert_eq!(s.unpriced_tokens, 100);
    assert!(!s.fully_priced());
    assert_eq!(s.per_provider[0].unpriced_tokens, 100);
    assert_eq!(s.per_day[0].unpriced_tokens, 100);
    assert_eq!(
        (
            s.per_model[0].unpriced_tokens,
            s.per_model[1].unpriced_tokens
        ),
        (0, 100)
    );
    assert_eq!(s.error, None);

    // A failed run keeps the figures, carries the error and moves `generated_at`.
    let failed = summary_after_error(Some(&s), 7, T0 + 60, "cache unreadable");
    assert_eq!((failed.total_cost_usd, failed.generated_at), (1.5, T0 + 60));
    assert_eq!(failed.error.as_deref(), Some("cache unreadable"));
    let first = summary_after_error(None, 30, T0, "boom");
    assert_eq!(
        (
            first.days,
            first.total_tokens.total(),
            first.error.as_deref()
        ),
        (30, 0, Some("boom"))
    );
    // Older snapshots without the new keys still parse.
    let old: codenotch_core::AnalyticsSummary = serde_json::from_str(r#"{"days":7,"generatedAt":1,"totalTokens":{"input":1,"output":0,"cacheRead":0,"cacheWrite":0},"totalCostUsd":0.1,"perProvider":[],"perDay":[],"perModel":[]}"#).unwrap();
    assert_eq!((old.unpriced_tokens, old.error), (0, None));
    assert!(old.detail.facts.is_empty());

    // The statistics detail travels with the summary, shared (not copied).
    let mut report = report;
    report.summary.detail = codenotch_core::SharedDetail::new(codenotch_core::AnalyticsDetail {
        today: "2026-11-12".into(),
        facts: vec![codenotch_core::UsageFact {
            date: "2026-11-12".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    let s = summary_from_report(&report);
    assert!(std::sync::Arc::ptr_eq(
        &s.detail.0,
        &report.summary.detail.0
    ));
    assert_eq!(s.detail.facts.len(), 1);
    // A failed run keeps the previous detail.
    let failed = summary_after_error(Some(&s), 7, T0 + 60, "x");
    assert_eq!(failed.detail, s.detail);
}

/// The local identity / plan fill what a failed fetch left out, never what it found, and a
/// panicking provider becomes an error reading.
#[test]
fn fetch_account_fills_identity_and_plan_from_local_files() {
    struct Local(ReadingStatus, Option<&'static str>);
    impl Provider for Local {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Cursor
        }
        fn discover(&self, _env: &Env) -> Vec<Account> {
            Vec::new()
        }
        fn local_identity(&self, _a: &Account, _env: &Env) -> codenotch_core::LocalIdentity {
            codenotch_core::LocalIdentity {
                identity: Some("me@example.test".into()),
                plan: Some("Ultra".into()),
            }
        }
        fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
            if self.0 == ReadingStatus::Error {
                panic!("boom");
            }
            let mut r = Reading::with_status(account, ctx.now, self.0.clone(), "n");
            r.plan = self.1.map(str::to_string);
            r
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let ctx = FetchContext::new(T0, Arc::new(FakeHttp::new()), Env::for_test(dir.path()));
    let a = Account::new(ProviderKind::Cursor, "Cursor");
    let r = fetch_account(
        &Local(ReadingStatus::RateLimited { until: None }, None),
        &a,
        &ctx,
    );
    assert_eq!(
        (r.identity.as_deref(), r.plan.as_deref()),
        (Some("me@example.test"), Some("Ultra"))
    );
    let r = fetch_account(&Local(ReadingStatus::Fresh, Some("Pro")), &a, &ctx);
    assert_eq!(r.plan.as_deref(), Some("Pro"), "the fetched plan wins");
    let r = fetch_account(&Local(ReadingStatus::NotInstalled, None), &a, &ctx);
    assert_eq!((r.identity, r.plan), (None, None));
    let r = fetch_account(&Local(ReadingStatus::Error, None), &a, &ctx);
    assert_eq!(r.status, ReadingStatus::Error);
}
