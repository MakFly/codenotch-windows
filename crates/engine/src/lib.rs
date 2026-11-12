//! # codenotch-engine
//!
//! The non-UI heart: owns config + state, schedules fetches with back-off, runs activity,
//! notify and analytics, persists `state.json` / `history.jsonl`, and publishes [`Snapshot`]s.
//!
//! ## Threading
//! * [`start`] spawns **one engine thread** that owns all mutable state (config, persisted
//!   state, snapshot, notifier). It wakes on every message and at least every
//!   [`Tuning::tick`] (the scheduler tick); [`EventSink::emit`] is non-blocking.
//! * A small forwarder thread moves [`Command`]s from the UI's channel into the engine's own
//!   channel, so worker results and commands are handled in arrival order.
//! * Fetches run on short-lived worker threads (at most [`Tuning::max_workers`] at once, one
//!   per account) and report back through the engine channel; providers never touch engine
//!   state. Discovery, analytics, the update check, hook installation, sign-in and test
//!   messages also run on worker threads.
//! * Activity runs its own threads; its `on_change` callback just posts to the engine.
//!
//! ## Scheduling
//! See [`schedule`]: pure functions, one [`schedule::AccountSched`] per enabled account.
//!
//! ## Persistence
//! `state.json`: last reading, back-off deadline, alert bookkeeping, and (in `extra`)
//! `lastAttemptAt` / `claude429` per account. Saved atomically, debounced
//! ([`Tuning::save_debounce`]) and on shutdown. `history.jsonl`: one line per window of every
//! new fresh reading; pruned to `historyHours` every [`Tuning::prune_every`].

mod discovery;
mod engine;
mod i18n;
pub mod schedule;
pub mod update;

#[cfg(test)]
mod tests;

pub use discovery::{discover_all, is_configured, profile_key};
pub use i18n::FR;
pub use update::{Version, UPDATE_REPO};

use codenotch_analytics::AnalyticsReport;
use codenotch_core::model::{Command, EventSink, Reading, ReadingStatus, Snapshot};
use codenotch_core::provider::{Env, FetchContext, Provider};
use codenotch_core::secrets::SecretStore;
use codenotch_core::state::PersistedState;
use codenotch_core::{Account, Config, HttpClient, Paths, ProviderKind, SystemActions};
use codenotch_notify::ToastSink;
use engine::{Engine, Msg};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Engine clock (unix seconds). Injectable for tests.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;
/// A shared provider list.
pub type Registry = Arc<Vec<Box<dyn Provider>>>;

/// Global timeout of [`fetch_once`].
pub const ONCE_TIMEOUT: Duration = Duration::from_secs(130);

/// Timings of the engine thread (production defaults; tests shorten them).
#[derive(Debug, Clone)]
pub struct Tuning {
    /// Longest sleep between two scheduling passes.
    pub tick: Duration,
    /// Fetches running at the same time.
    pub max_workers: usize,
    /// First automatic fetches are spread by this many seconds per account.
    pub stagger_secs: i64,
    /// Minimum time between two `state.json` writes (shutdown always writes).
    pub save_debounce: Duration,
    /// First analytics run after start (`None` = only on `RefreshAnalytics`).
    pub analytics_delay: Option<Duration>,
    /// Analytics period after the first run.
    pub analytics_every: Duration,
    /// First automatic update check after start, unless `updateMode` is off (`None` = never
    /// automatic).
    pub update_delay: Option<Duration>,
    /// Period of the automatic update checks.
    pub update_every: Duration,
    /// First history prune after start.
    pub prune_delay: Duration,
    /// History prune period.
    pub prune_every: Duration,
    /// Snapshot republished at least this often (keeps `now` and ages moving).
    pub publish_every: Duration,
    /// A fetch running longer than this frees its worker slot (its result is then ignored).
    pub fetch_stall: Duration,
    /// Start the activity services (hook server, transcript watcher, pollers).
    pub activity: bool,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            tick: Duration::from_secs(1),
            max_workers: 4,
            stagger_secs: 2,
            save_debounce: Duration::from_secs(2),
            analytics_delay: Some(Duration::from_secs(10)),
            analytics_every: Duration::from_secs(15 * 60),
            update_delay: Some(Duration::from_secs(20)),
            update_every: Duration::from_secs(6 * 3600),
            prune_delay: Duration::from_secs(60),
            prune_every: Duration::from_secs(3600),
            publish_every: Duration::from_secs(30),
            fetch_stall: Duration::from_secs(10 * 60),
            activity: true,
        }
    }
}

/// Everything the engine needs from outside.
#[derive(Clone)]
pub struct EngineOptions {
    pub paths: Paths,
    pub env: Env,
    pub http: Arc<dyn HttpClient>,
    pub system: Arc<dyn SystemActions>,
    /// Secret store (Credential Manager in production). Used for `SetSecret`/`ClearSecret`,
    /// snapshot presence flags, and handed to notify.
    pub secrets: Arc<dyn SecretStore>,
    /// Windows notification backend for alerts (`codenotch_notify::WinToast` in the app).
    /// `None` (default): alerts reach the UI as `Event::Alert` only, never a real toast.
    pub toast: Option<Arc<dyn ToastSink>>,
    /// Provider list. `None` (default) = [`registry()`]. Tests inject fakes here.
    pub providers: Option<Registry>,
    /// Clock. `None` (default) = the system clock.
    pub clock: Option<Clock>,
    /// `codenotch-hook.exe` for `InstallHooks`. `None` (default) = next to the running exe.
    pub hook_exe: Option<PathBuf>,
    /// Config load error to show in the first snapshot (`Config::load_or_default`).
    pub storage_error: Option<String>,
    pub tuning: Tuning,
    /// Self-update wiring.
    pub update: UpdateOptions,
}

/// How the engine runs the self-updater ([`update::Updater`]).
#[derive(Clone, Default)]
pub struct UpdateOptions {
    /// Start the updater thread (`EngineOptions::system`: on; tests: off unless they test it).
    pub enabled: bool,
    /// The running exe, whose folder is replaced. `None` = `std::env::current_exe()`.
    pub exe: Option<PathBuf>,
    /// Arguments to start the new version with (the app's own, minus update flags).
    pub args: Vec<String>,
    /// Starts the new version. `None` = [`update::spawn_relaunch`].
    pub relaunch: Option<update::Relaunch>,
}

impl EngineOptions {
    /// Real machine: `Paths::resolve()`, `Env::system()`, `UreqHttp`, the given system actions,
    /// and `NoSecrets` (the app sets `secrets` to the platform `CredentialStore`).
    pub fn system(system: Arc<dyn SystemActions>) -> Self {
        Self {
            paths: Paths::resolve(),
            env: Env::system(),
            http: Arc::new(codenotch_core::UreqHttp::new()),
            system,
            secrets: Arc::new(codenotch_core::NoSecrets),
            toast: None,
            providers: None,
            clock: None,
            hook_exe: None,
            storage_error: None,
            tuning: Tuning::default(),
            update: UpdateOptions {
                enabled: true,
                ..UpdateOptions::default()
            },
        }
    }

    fn clock(&self) -> Clock {
        self.clock
            .clone()
            .unwrap_or_else(|| Arc::new(codenotch_core::now_secs))
    }

    fn registry(&self) -> Registry {
        self.providers
            .clone()
            .unwrap_or_else(|| Arc::new(registry()))
    }
}

/// The updater's setup from the engine's inputs: work folder `<data dir>\updates`, manifest URL
/// from the settings unless `CODENOTCH_UPDATE_URL` is set (which also allows local files, as
/// debug builds do), `CODENOTCH_UPDATE_ALLOW_DEV=1` to update a cargo build.
pub fn updater_setup(
    paths: &Paths,
    env: &Env,
    http: Arc<dyn HttpClient>,
    clock: Clock,
    settings: &codenotch_core::Settings,
    opts: &UpdateOptions,
    tuning: &Tuning,
) -> update::UpdaterSetup {
    let url_override = env
        .var(update::UPDATE_URL_ENV)
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty());
    update::UpdaterSetup {
        http,
        current: update::current_version(env),
        exe: opts.exe.clone().or_else(|| std::env::current_exe().ok()),
        work_dir: paths.root().join("updates"),
        mode: settings.update_mode(),
        manifest_url: settings.update_manifest_url.clone(),
        allow_file: cfg!(debug_assertions) || url_override.is_some(),
        url_override,
        allow_dev: env.var(update::ALLOW_DEV_ENV).as_deref() == Some("1"),
        first_check: tuning.update_delay,
        every: tuning.update_every,
        clock,
        relaunch: opts.relaunch.clone().unwrap_or_else(update::spawn_relaunch),
        args: opts.args.clone(),
        retry: update::Retry::DEFAULT,
    }
}

/// Running engine.
pub struct EngineHandle {
    commands: Sender<Command>,
    internal: Sender<Msg>,
    analytics: Arc<Mutex<Option<AnalyticsReport>>>,
    thread: Option<JoinHandle<()>>,
}

impl EngineHandle {
    /// Sender for UI/tray/hotkey commands.
    pub fn commands(&self) -> Sender<Command> {
        self.commands.clone()
    }

    /// Install (or remove) the Windows notification backend while running. Any thread.
    pub fn set_toast(&self, sink: Option<Arc<dyn ToastSink>>) {
        let _ = self.internal.send(Msg::SetToast(sink));
    }

    /// The last full analytics report (the snapshot only carries its summary). Cheap clone;
    /// any thread. `None` until the first run finished.
    pub fn analytics_report(&self) -> Option<AnalyticsReport> {
        self.analytics.lock().ok().and_then(|r| r.clone())
    }

    /// A cloneable getter for [`EngineHandle::analytics_report`] (for UI code that does not
    /// own the handle).
    pub fn analytics_source(&self) -> Arc<Mutex<Option<AnalyticsReport>>> {
        self.analytics.clone()
    }

    /// Ask the engine to stop (state is flushed, activity stopped) and wait for it.
    pub fn shutdown(mut self) {
        let _ = self.internal.send(Msg::Cmd(Command::Quit));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The snapshot's analytics summary, with the detail the report has and the plain summary
/// lacks: unpriced tokens overall, per provider, per day and per model (so the UI shows costs
/// that leave tokens out as lower bounds), and `error: None`.
pub fn summary_from_report(report: &AnalyticsReport) -> codenotch_core::AnalyticsSummary {
    let mut s = report.summary.clone();
    s.unpriced_tokens = report.total.unpriced_tokens;
    s.error = None;
    for p in &mut s.per_provider {
        p.unpriced_tokens = report
            .per_cli
            .iter()
            .find(|c| c.provider == p.provider)
            .map_or(0, |c| c.usage.unpriced_tokens);
    }
    for d in &mut s.per_day {
        d.unpriced_tokens = report
            .per_day
            .iter()
            .find(|r| r.date == d.date)
            .and_then(|r| r.per_cli.iter().find(|c| c.provider == d.provider))
            .map_or(0, |c| c.usage.unpriced_tokens);
    }
    for m in &mut s.per_model {
        m.unpriced_tokens = report
            .per_model
            .iter()
            .find(|r| r.model == m.model && r.provider == m.provider)
            .map_or(0, |r| r.usage.unpriced_tokens);
    }
    s
}

/// The summary after a failed analytics run: the previous figures (if any) with `error` set and
/// `generated_at = now` (so a pending "Refresh" ends), else an empty summary carrying the error.
pub fn summary_after_error(
    previous: Option<&codenotch_core::AnalyticsSummary>,
    days: u32,
    now: i64,
    error: &str,
) -> codenotch_core::AnalyticsSummary {
    let mut s = previous
        .cloned()
        .unwrap_or_else(|| codenotch_core::AnalyticsSummary {
            days,
            ..Default::default()
        });
    s.generated_at = now;
    s.error = Some(codenotch_core::logging::redact(error));
    s
}

/// Every provider implementation (both provider crates).
pub fn registry() -> Vec<Box<dyn Provider>> {
    let mut all = codenotch_providers::all();
    all.extend(codenotch_providers_extra::all());
    all
}

/// Provider for an account (custom endpoint wins, as in 0.3).
pub fn provider_for<'a>(
    registry: &'a [Box<dyn Provider>],
    account: &Account,
) -> Option<&'a dyn Provider> {
    let kind = if account.uses_custom_endpoint() {
        ProviderKind::Custom
    } else {
        account.provider.clone()
    };
    registry
        .iter()
        .find(|p| p.kind() == kind)
        .map(|p| p.as_ref())
}

/// `provider.fetch`, then the identity / plan the fetch left out filled from the provider's
/// local files ([`Provider::local_identity`]), so they show even when the read failed. A panic
/// becomes an error reading.
pub fn fetch_account(provider: &dyn Provider, account: &Account, ctx: &FetchContext) -> Reading {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut reading = provider.fetch(account, ctx);
        if reading.status != ReadingStatus::NotInstalled {
            provider
                .local_identity(account, &ctx.env)
                .fill(&mut reading);
        }
        reading
    }));
    caught.unwrap_or_else(|_| {
        log::error!("{} fetch panicked", account.provider);
        Reading::error(account, ctx.now, "provider failed")
    })
}

/// First snapshot: config + readings restored from `state.json` (shown as stale).
pub fn initial_snapshot(config: Config, paths: &Paths, now: i64) -> Snapshot {
    let restored = PersistedState::load(&paths.state()).restored_readings();
    let mut snap = Snapshot::from_config(config, now);
    for view in &mut snap.accounts {
        view.reading = restored.get(&view.account.id).cloned();
    }
    snap
}

/// `--once`: discover (not saved), then fetch every enabled account once, in parallel, within
/// [`ONCE_TIMEOUT`]. Read-only: writes nothing to the data folder. Child processes follow
/// `CODENOTCH_NO_SPAWN` (the providers read it from `opts.env`).
pub fn fetch_once(config: &Config, opts: &EngineOptions, now: i64) -> Vec<(Account, Reading)> {
    fetch_once_within(config, opts, now, ONCE_TIMEOUT)
}

/// [`fetch_once`] with an explicit global timeout. An account still running at the deadline
/// gets an error reading ("timed out"). An account inside a persisted rate-limit deadline is
/// not fetched (its saved reading is returned) — except Codex, which then only reads locally.
pub fn fetch_once_within(
    config: &Config,
    opts: &EngineOptions,
    now: i64,
    timeout: Duration,
) -> Vec<(Account, Reading)> {
    let registry = opts.registry();
    let mut config = config.clone();
    discovery::merge(
        &mut config,
        discovery::discover_all(&registry, &opts.env),
        &opts.env,
    );
    let state = PersistedState::load(&opts.paths.state());
    let restored = state.restored_readings();
    let accounts: Vec<Account> = config.enabled_accounts().cloned().collect();
    let mut results: Vec<Option<Reading>> = vec![None; accounts.len()];
    let (tx, rx) = std::sync::mpsc::channel::<(usize, Reading)>();
    let mut pending = 0usize;
    for (i, account) in accounts.iter().enumerate() {
        if provider_for(&registry, account).is_none() {
            results[i] = Some(Reading::error(
                account,
                now,
                format!("no provider for '{}'", account.provider),
            ));
            continue;
        }
        let backoff = state
            .backoff_until
            .get(&account.id)
            .copied()
            .filter(|u| *u > now);
        let codex = account.provider == ProviderKind::Codex && !account.uses_custom_endpoint();
        if let (Some(until), false) = (backoff, codex) {
            let mut r = restored
                .get(&account.id)
                .cloned()
                .unwrap_or_else(|| Reading::new(account, now));
            r.status = ReadingStatus::RateLimited { until: Some(until) };
            let wait = (until - now).to_string();
            r.note = Some(codenotch_core::i18n::fill(
                i18n::tr(config.settings.lang(), "Rate limited, retrying in {n}s"),
                &[("n", &wait)],
            ));
            results[i] = Some(r);
            continue;
        }
        let mut ctx = FetchContext::new(now, opts.http.clone(), opts.env.clone());
        ctx.previous = restored.get(&account.id).cloned();
        ctx.settings = config.settings.clone();
        ctx.manual = true;
        ctx.secrets = Some(opts.secrets.clone());
        let registry = registry.clone();
        let account = account.clone();
        let tx = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("codenotch-once".into())
            .spawn(move || {
                let reading = match provider_for(&registry, &account) {
                    Some(p) => fetch_account(p, &account, &ctx),
                    None => Reading::error(&account, ctx.now, "no provider"),
                };
                let _ = tx.send((i, reading));
            });
        match spawned {
            Ok(_) => pending += 1,
            Err(_) => results[i] = Some(Reading::error(&accounts[i], now, "cannot start a thread")),
        }
    }
    drop(tx);
    let deadline = std::time::Instant::now() + timeout;
    while pending > 0 {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok((i, reading)) => {
                results[i] = Some(reading);
                pending -= 1;
            }
            Err(_) => break,
        }
    }
    accounts
        .into_iter()
        .zip(results)
        .map(|(account, r)| {
            let reading = r.unwrap_or_else(|| Reading::error(&account, now, "timed out"));
            (account, reading)
        })
        .collect()
}

/// Start the engine thread. `commands` is the channel whose sender the UI holds.
pub fn start(
    config: Config,
    opts: EngineOptions,
    commands: (Sender<Command>, Receiver<Command>),
    sink: EventSink,
) -> EngineHandle {
    let (tx, rx) = commands;
    let (internal_tx, internal_rx) = std::sync::mpsc::channel::<Msg>();
    let analytics = Arc::new(Mutex::new(None));

    let forward = internal_tx.clone();
    let forwarder = std::thread::Builder::new()
        .name("codenotch-engine-commands".into())
        .spawn(move || {
            for command in rx {
                if forward.send(Msg::Cmd(command)).is_err() {
                    return;
                }
            }
            let _ = forward.send(Msg::CommandsClosed);
        });
    if let Err(err) = forwarder {
        log::error!("cannot start the command forwarder: {err}");
    }

    let engine_tx = internal_tx.clone();
    let commands_tx = tx.clone();
    let shared = analytics.clone();
    let thread = std::thread::Builder::new()
        .name("codenotch-engine".into())
        .spawn(move || {
            Engine::new(config, opts, sink, engine_tx, commands_tx, shared).run(internal_rx)
        });
    let thread = match thread {
        Ok(t) => Some(t),
        Err(err) => {
            log::error!("cannot start the engine thread: {err}");
            None
        }
    };
    EngineHandle {
        commands: tx,
        internal: internal_tx,
        analytics,
        thread,
    }
}
