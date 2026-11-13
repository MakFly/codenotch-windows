//! The engine thread: owns config, persisted state and the snapshot; schedules fetches on
//! worker threads; runs notify, activity, analytics, discovery and the update check.

use crate::i18n::tr;
use crate::schedule::{self, AccountSched, Decision, Policy, Restored};
use crate::{discovery, provider_for, update, Clock, EngineOptions, Registry, Tuning};
use codenotch_activity::{ActivityHandle, ActivityReport};
use codenotch_analytics::AnalyticsReport;
use codenotch_core::config::UpdateMode;
use codenotch_core::forecast::ActivityProfile;
use codenotch_core::history::{retain_for_forecast, History, HistoryPoint, PERIOD_KEEP_SECS};
use codenotch_core::model::ActivityState;
use codenotch_core::model::{
    AccountView, Command, Event, EventSink, Reading, ReadingStatus, Snapshot, UpdateState,
};
use codenotch_core::provider::FetchContext;
use codenotch_core::secrets::SecretsPresent;
use codenotch_core::state::PersistedState;
use codenotch_core::{Account, Config, ProviderKind};
use codenotch_notify::{AlertInput, DesktopContext, Notifier, TestTarget, ToastSink};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Reading error while an account's previous fetch thread is stuck (English key, see `FR`).
pub(crate) const MSG_FETCH_STUCK: &str = "The last refresh did not finish.";

/// Clears an account's "fetch thread alive" flag when the fetch thread ends (also on panic).
struct AliveGuard(Arc<AtomicBool>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// History kept densely in memory per account for the alert forecast (≥ the last hour).
/// Older samples of windows whose period is still open are kept too, thinned, for the
/// period forecast (see `history::retain_for_forecast`).
const RECENT_SECS: i64 = 2 * 3600;
/// How often every period forecast is recomputed without a new reading.
const FORECAST_EVERY: std::time::Duration = std::time::Duration::from_secs(300);
/// `PersistedState::extra` keys owned by the engine.
const LAST_ATTEMPT_KEY: &str = "lastAttemptAt";
const CLAUDE_429_KEY: &str = "claude429";

/// Messages into the engine thread.
pub(crate) enum Msg {
    Cmd(Command),
    CommandsClosed,
    Fetched {
        account_id: String,
        ticket: u64,
        reading: Box<Reading>,
    },
    Discovered(Vec<Account>),
    Activity(ActivityReport),
    Analytics(Result<Box<AnalyticsReport>, String>),
    Update(update::Report),
    Hooks {
        install: bool,
        result: Result<(), String>,
    },
    SignInFailed {
        account_id: String,
        remove: bool,
        message: String,
    },
    SetToast(Option<Arc<dyn ToastSink>>),
}

pub(crate) struct Engine {
    opts: EngineOptions,
    registry: Registry,
    clock: Clock,
    tuning: Tuning,
    sink: EventSink,
    tx: Sender<Msg>,
    commands: Sender<Command>,
    snap: Snapshot,
    last_published: Option<Snapshot>,
    last_publish_at: Instant,
    sched: HashMap<String, AccountSched>,
    in_flight: HashMap<String, (u64, Instant)>,
    /// Per account: true while its last fetch thread is still running (cleared by the thread
    /// itself when it really ends, even after `check_stalls` gave up on it).
    fetch_alive: HashMap<String, Arc<AtomicBool>>,
    next_ticket: u64,
    /// A hooks install/remove job is running.
    hooks_running: bool,
    state: PersistedState,
    state_dirty: bool,
    last_state_save: Instant,
    history: History,
    recent: HashMap<String, Vec<HistoryPoint>>,
    last_history_at: HashMap<String, i64>,
    forecast_next: Instant,
    notifier: Notifier,
    desktop: DesktopContext,
    activity: Option<ActivityHandle>,
    activity_report: ActivityReport,
    analytics_running: bool,
    /// A UI request arrived while a run was in flight: run once more when it ends (the
    /// request may follow a period change the in-flight run does not reflect).
    analytics_rerun: bool,
    analytics_next: Option<Instant>,
    analytics_shared: Arc<Mutex<Option<AnalyticsReport>>>,
    /// Self-updater thread (`None` when disabled).
    updater: Option<update::Updater>,
    /// Mode and manifest URL last given to the updater.
    updater_settings: (UpdateMode, String),
    /// Idle state last given to the updater.
    updater_idle: Option<bool>,
    /// The user is busy in a CodeNotch window (`Command::UiBusy`).
    ui_busy: bool,
    /// Windows notification backend, for "update available".
    toast: Option<Arc<dyn ToastSink>>,
    prune_next: Instant,
    discovering: bool,
    no_provider: HashSet<String>,
}

/// Period forecast of every window of `reading` (local time for the profile). Readings without
/// figures get none.
fn fill_forecasts(
    reading: &mut Reading,
    history: &[HistoryPoint],
    profile: Option<&ActivityProfile>,
    now: i64,
) {
    let shows = reading.shows_figures();
    let observed = reading.fetched_at;
    for w in &mut reading.windows {
        w.forecast = if shows {
            codenotch_core::forecast::forecast(w, observed, history, now, profile, &chrono::Local)
        } else {
            None
        };
    }
}

fn int_map(state: &PersistedState, key: &str) -> BTreeMap<String, i64> {
    state
        .extra
        .get(key)
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_i64()?)))
                .collect()
        })
        .unwrap_or_default()
}

fn set_int_map(state: &mut PersistedState, key: &str, map: &BTreeMap<String, i64>) {
    if map.is_empty() {
        state.extra.remove(key);
    } else {
        let obj: serde_json::Map<String, serde_json::Value> =
            map.iter().map(|(k, v)| (k.clone(), (*v).into())).collect();
        state
            .extra
            .insert(key.to_string(), serde_json::Value::Object(obj));
    }
}

impl Engine {
    pub(crate) fn new(
        config: Config,
        opts: EngineOptions,
        sink: EventSink,
        tx: Sender<Msg>,
        commands: Sender<Command>,
        analytics_shared: Arc<Mutex<Option<AnalyticsReport>>>,
    ) -> Self {
        let clock = opts.clock();
        let registry = opts.registry();
        let tuning = opts.tuning.clone();
        let now = clock();
        let state = PersistedState::load(&opts.paths.state());
        let restored = state.restored_readings();
        let mut snap = Snapshot::from_config(config, now);
        for view in &mut snap.accounts {
            view.reading = restored.get(&view.account.id).cloned();
        }
        snap.secrets = SecretsPresent::detect(&opts.env, &*opts.secrets);
        snap.storage_error = opts.storage_error.clone();

        let history = History::new(opts.paths.history());
        let mut recent: HashMap<String, Vec<HistoryPoint>> = HashMap::new();
        for p in history.read_since(now - PERIOD_KEEP_SECS) {
            recent.entry(p.account_id.clone()).or_default().push(p);
        }
        let mut last_history_at = HashMap::new();
        for (id, pts) in &mut recent {
            retain_for_forecast(pts, now, RECENT_SECS);
            if let Some(last) = pts.last() {
                last_history_at.insert(id.clone(), last.at);
            }
        }

        let mut notifier = Notifier::new(opts.http.clone(), opts.env.clone(), opts.secrets.clone());
        notifier.set_toast(opts.toast.clone());
        notifier.set_accounts(&snap.config.accounts);

        let started = Instant::now();
        let mut engine = Engine {
            analytics_next: tuning.analytics_delay.map(|d| started + d),
            updater: None,
            updater_settings: (
                snap.config.settings.update_mode(),
                snap.config.settings.update_manifest_url.clone(),
            ),
            updater_idle: None,
            ui_busy: false,
            toast: opts.toast.clone(),
            prune_next: started + tuning.prune_delay,
            opts,
            registry,
            clock,
            tuning,
            sink,
            tx,
            commands,
            snap,
            last_published: None,
            last_publish_at: started,
            sched: HashMap::new(),
            in_flight: HashMap::new(),
            fetch_alive: HashMap::new(),
            next_ticket: 0,
            hooks_running: false,
            state,
            state_dirty: false,
            last_state_save: started,
            history,
            recent,
            last_history_at,
            forecast_next: started + FORECAST_EVERY,
            notifier,
            desktop: DesktopContext::default(),
            activity: None,
            activity_report: ActivityReport::default(),
            analytics_running: false,
            analytics_rerun: false,
            analytics_shared,
            discovering: false,
            no_provider: HashSet::new(),
        };
        engine.restore_schedules(now);
        engine.refresh_forecasts(now);
        if engine.tuning.activity {
            engine.start_activity();
        }
        if engine.opts.update.enabled {
            engine.start_updater();
        }
        engine
    }

    // -----------------------------------------------------------------------------------------
    // Main loop
    // -----------------------------------------------------------------------------------------

    pub(crate) fn run(mut self, rx: Receiver<Msg>) {
        self.publish(true);
        self.start_discovery();
        self.tick();
        loop {
            match rx.recv_timeout(self.tuning.tick) {
                Ok(msg) => {
                    if !self.handle(msg) {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.tick();
        }
    }

    fn tick(&mut self) {
        let now = (self.clock)();
        self.check_stalls();
        self.schedule(now);
        self.timers(now);
        self.maybe_save(false);
        self.publish(false);
    }

    /// Returns false when the engine must stop.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Cmd(Command::Quit) => {
                self.shutdown();
                self.sink.emit(Event::Quit);
                return false;
            }
            Msg::CommandsClosed => {
                self.shutdown();
                return false;
            }
            Msg::Cmd(command) => self.command(command),
            Msg::Fetched {
                account_id,
                ticket,
                reading,
            } => self.on_fetched(&account_id, ticket, *reading),
            Msg::Discovered(found) => {
                self.discovering = false;
                let added = discovery::merge(&mut self.snap.config, found, &self.opts.env);
                if !added.is_empty() {
                    log::info!("discovery: {} new account(s)", added.len());
                    self.save_config();
                    self.config_changed();
                }
            }
            Msg::Activity(report) => self.activity_report = report,
            Msg::Analytics(result) => {
                self.analytics_running = false;
                match result {
                    Ok(report) => {
                        self.snap.analytics = Some(crate::summary_from_report(&report));
                        if let Ok(mut slot) = self.analytics_shared.lock() {
                            *slot = Some(*report);
                        }
                        // New activity profile.
                        self.refresh_forecasts((self.clock)());
                    }
                    Err(err) => {
                        log::warn!("analytics failed: {err}");
                        let days = self.snap.config.settings.analytics_days.clamp(1, 366) as u32;
                        let now = (self.clock)();
                        self.snap.analytics = Some(crate::summary_after_error(
                            self.snap.analytics.as_ref(),
                            days,
                            now,
                            &err,
                        ));
                    }
                }
                if std::mem::take(&mut self.analytics_rerun) {
                    self.start_analytics(AnalyticsWhy::Queued);
                }
            }
            Msg::Update(report) => {
                if !self.on_update(report) {
                    return false;
                }
            }
            Msg::Hooks { install, result } => self.on_hooks(install, result),
            Msg::SignInFailed {
                account_id,
                remove,
                message,
            } => {
                if remove {
                    self.snap.config.accounts.retain(|a| a.id != account_id);
                    self.save_config();
                    self.config_changed();
                }
                self.notice(&message);
            }
            Msg::SetToast(sink) => {
                self.toast = sink.clone();
                self.notifier.set_toast(sink);
            }
        }
        true
    }

    fn shutdown(&mut self) {
        self.sync_all_schedules();
        self.maybe_save(true);
        if let Some(activity) = self.activity.take() {
            activity.stop();
        }
    }

    // -----------------------------------------------------------------------------------------
    // Scheduling
    // -----------------------------------------------------------------------------------------

    fn claude_active(&self) -> bool {
        self.snap
            .accounts
            .iter()
            .any(|v| v.account.provider == ProviderKind::Claude && !v.sessions.is_empty())
    }

    fn policy(&self, account: &Account, active: bool) -> Option<Policy> {
        let provider = provider_for(&self.registry, account)?;
        Some(Policy::for_account(
            account,
            provider.min_interval(),
            &self.snap.config.settings,
            active,
        ))
    }

    fn restore_schedules(&mut self, now: i64) {
        let last_attempts = int_map(&self.state, LAST_ATTEMPT_KEY);
        let counts = int_map(&self.state, CLAUDE_429_KEY);
        let accounts: Vec<Account> = self.snap.config.enabled_accounts().cloned().collect();
        for (i, account) in accounts.iter().enumerate() {
            let Some(policy) = self.policy(account, false) else {
                continue;
            };
            let reading = self
                .snap
                .account(&account.id)
                .and_then(|v| v.reading.as_ref());
            let saved = Restored {
                reading,
                backoff_until: self.state.backoff_until.get(&account.id).copied(),
                last_attempt: last_attempts.get(&account.id).copied(),
                consecutive_429: counts
                    .get(&account.id)
                    .and_then(|n| u32::try_from(*n).ok())
                    .unwrap_or(0),
            };
            let not_before = now + self.tuning.stagger_secs * i as i64;
            let s = schedule::restore(&policy, saved, now, not_before);
            self.sched.insert(account.id.clone(), s);
        }
    }

    fn schedule(&mut self, now: i64) {
        let accounts: Vec<Account> = self.snap.config.enabled_accounts().cloned().collect();
        let uses_claude = accounts
            .iter()
            .any(|a| a.provider == ProviderKind::Claude && !a.uses_custom_endpoint());
        let busy = uses_claude && codenotch_providers::claude_auth_state().busy;
        let active = self.claude_active();
        let mut due: Vec<(Account, bool)> = Vec::new();
        self.fetch_alive
            .retain(|_, alive| alive.load(Ordering::SeqCst));
        for account in accounts {
            let Some(policy) = self.policy(&account, active) else {
                self.report_no_provider(&account, now);
                continue;
            };
            // Never a second thread for an account whose previous fetch is still running
            // (a provider that hangs every time would otherwise pile up threads).
            if self.fetch_alive.contains_key(&account.id) {
                continue;
            }
            let s = self
                .sched
                .entry(account.id.clone())
                .or_insert_with(|| AccountSched {
                    not_before: now,
                    ..AccountSched::default()
                });
            if let Decision::Fetch { manual } = schedule::next_action(s, &policy, now, busy) {
                due.push((account, manual));
            }
        }
        due.sort_by_key(|(_, manual)| !*manual);
        for (account, manual) in due {
            if self.in_flight.len() >= self.tuning.max_workers.max(1) {
                break;
            }
            self.start_fetch(account, manual, now);
        }
    }

    fn report_no_provider(&mut self, account: &Account, now: i64) {
        if !self.no_provider.insert(account.id.clone()) {
            return;
        }
        if let Some(view) = self.snap.account_mut(&account.id) {
            view.reading = Some(Reading::error(
                account,
                now,
                format!("no provider for '{}'", account.provider),
            ));
        }
    }

    fn start_fetch(&mut self, account: Account, manual: bool, now: i64) {
        let Some(s) = self.sched.get_mut(&account.id) else {
            return;
        };
        schedule::on_start(s, now);
        self.next_ticket += 1;
        let ticket = self.next_ticket;
        self.in_flight
            .insert(account.id.clone(), (ticket, Instant::now()));

        let mut ctx = FetchContext::new(now, self.opts.http.clone(), self.opts.env.clone());
        ctx.previous = self
            .snap
            .account(&account.id)
            .and_then(|v| v.reading.clone());
        ctx.settings = self.snap.config.settings.clone();
        ctx.manual = manual;
        ctx.secrets = Some(self.opts.secrets.clone());
        let registry = self.registry.clone();
        let tx = self.tx.clone();
        let id = account.id.clone();
        let alive = Arc::new(AtomicBool::new(true));
        self.fetch_alive.insert(id.clone(), alive.clone());
        let spawned = std::thread::Builder::new()
            .name("codenotch-fetch".into())
            .spawn(move || {
                let _alive = AliveGuard(alive);
                let reading = match provider_for(&registry, &account) {
                    Some(p) => crate::fetch_account(p, &account, &ctx),
                    None => Reading::error(
                        &account,
                        ctx.now,
                        format!("no provider for '{}'", account.provider),
                    ),
                };
                let _ = tx.send(Msg::Fetched {
                    account_id: account.id.clone(),
                    ticket,
                    reading: Box::new(reading),
                });
            });
        if let Err(err) = spawned {
            log::error!("cannot start a fetch thread: {err}");
            self.in_flight.remove(&id);
            self.fetch_alive.remove(&id);
            if let Some(s) = self.sched.get_mut(&id) {
                s.in_flight = false;
            }
        }
    }

    /// A fetch that never returns frees its slot after `fetch_stall` (its late result is ignored).
    /// The account shows an error reading and is not fetched again until that thread ends.
    fn check_stalls(&mut self) {
        let limit = self.tuning.fetch_stall;
        let stalled: Vec<String> = self
            .in_flight
            .iter()
            .filter(|(_, (_, at))| at.elapsed() >= limit)
            .map(|(id, _)| id.clone())
            .collect();
        if stalled.is_empty() {
            return;
        }
        let now = (self.clock)();
        for id in stalled {
            log::warn!("fetch of account {id} did not return; giving up on it");
            self.in_flight.remove(&id);
            if let Some(s) = self.sched.get_mut(&id) {
                s.in_flight = false;
            }
            if let Some(account) = self.snap.config.account(&id).cloned() {
                let reading = Reading::error(&account, now, MSG_FETCH_STUCK);
                if let Some(view) = self.snap.account_mut(&id) {
                    view.reading = Some(reading.clone());
                }
                self.state.readings.insert(id, reading);
                self.state_dirty = true;
            }
        }
    }

    fn on_fetched(&mut self, id: &str, ticket: u64, mut reading: Reading) {
        match self.in_flight.get(id) {
            Some((t, _)) if *t == ticket => {
                self.in_flight.remove(id);
            }
            _ => {
                log::debug!("late fetch result ignored");
                return;
            }
        }
        let now = (self.clock)();
        let Some(account) = self.snap.config.account(id).cloned() else {
            self.sched.remove(id);
            return;
        };
        reading.account_id = account.id.clone();
        let active = self.claude_active();
        if let (Some(policy), Some(s)) = (self.policy(&account, active), self.sched.get_mut(id)) {
            schedule::on_result(s, &policy, &reading, now);
        } else if let Some(s) = self.sched.get_mut(id) {
            s.in_flight = false;
        }

        // History: every new fresh reading.
        if reading.status == ReadingStatus::Fresh
            && reading.fetched_at > self.last_history_at.get(id).copied().unwrap_or(0)
        {
            let points = HistoryPoint::from_reading(&reading);
            if !points.is_empty() {
                if let Err(err) = self.history.append(&points) {
                    log::warn!("history append failed: {err}");
                }
                let list = self.recent.entry(id.to_string()).or_default();
                list.extend(points);
                retain_for_forecast(list, now, RECENT_SECS);
            }
            self.last_history_at
                .insert(id.to_string(), reading.fetched_at);
        }
        let profile = self.profile_for(&reading.provider);
        fill_forecasts(
            &mut reading,
            self.recent.get(id).map(Vec::as_slice).unwrap_or(&[]),
            profile.as_ref(),
            now,
        );

        let previous = self
            .snap
            .account_mut(id)
            .and_then(|v| v.reading.replace(reading.clone()));

        // Alerts (every reading, including stale / error / signed out).
        let history = self.recent.get(id).map(Vec::as_slice).unwrap_or(&[]);
        let input = AlertInput {
            previous: previous.as_ref(),
            reading: &reading,
            history,
            settings: &self.snap.config.settings,
            desktop: self.desktop,
            now,
        };
        let events = self.notifier.process(&input, &mut self.state);

        self.state.readings.insert(id.to_string(), reading);
        self.sync_schedule(id);
        self.state_dirty = true;
        self.publish(false);
        for event in events {
            self.sink.emit(event);
        }
    }

    /// Copy one account's back-off / last attempt / 429 count into the persisted state.
    fn sync_schedule(&mut self, id: &str) {
        let Some(s) = self.sched.get(id).cloned() else {
            return;
        };
        match s.backoff_until {
            Some(u) => self.state.backoff_until.insert(id.to_string(), u),
            None => self.state.backoff_until.remove(id),
        };
        let mut attempts = int_map(&self.state, LAST_ATTEMPT_KEY);
        let mut counts = int_map(&self.state, CLAUDE_429_KEY);
        match s.last_attempt {
            Some(t) => attempts.insert(id.to_string(), t),
            None => attempts.remove(id),
        };
        if s.consecutive_429 > 0 {
            counts.insert(id.to_string(), i64::from(s.consecutive_429));
        } else {
            counts.remove(id);
        }
        set_int_map(&mut self.state, LAST_ATTEMPT_KEY, &attempts);
        set_int_map(&mut self.state, CLAUDE_429_KEY, &counts);
    }

    fn sync_all_schedules(&mut self) {
        let ids: Vec<String> = self.sched.keys().cloned().collect();
        let before = self.state.clone();
        for id in ids {
            self.sync_schedule(&id);
        }
        if self.state != before {
            self.state_dirty = true;
        }
    }

    // -----------------------------------------------------------------------------------------
    // Timers, persistence, publishing
    // -----------------------------------------------------------------------------------------

    fn timers(&mut self, now: i64) {
        let instant = Instant::now();
        if self.analytics_next.is_some_and(|at| instant >= at) {
            self.analytics_next = Some(instant + self.tuning.analytics_every);
            self.start_analytics(AnalyticsWhy::Scheduled);
        }
        if instant >= self.forecast_next {
            self.forecast_next = instant + FORECAST_EVERY;
            self.refresh_forecasts(now);
        }
        if instant >= self.prune_next {
            self.prune_next = instant + self.tuning.prune_every;
            let hours = self.snap.config.settings.history_hours;
            match self.history.prune(now, hours) {
                Ok(0) => {}
                Ok(n) => log::info!("history: pruned {n} sample(s)"),
                Err(err) => log::warn!("history prune failed: {err}"),
            }
        }
    }

    /// The provider's usual hour-of-week activity from the analytics fact table (Claude and
    /// Codex only; `None` without enough activity).
    fn profile_for(&self, provider: &ProviderKind) -> Option<ActivityProfile> {
        let detail = &self.snap.analytics.as_ref()?.detail;
        let grid =
            detail.week_hour_tokens(&codenotch_core::UsageFilter::provider(provider.clone()));
        ActivityProfile::from_week_hour(&grid)
    }

    /// Recompute the forecasts of every reading shown (time passed, or a new profile).
    fn refresh_forecasts(&mut self, now: i64) {
        let mut views = std::mem::take(&mut self.snap.accounts);
        for view in &mut views {
            let Some(reading) = view.reading.as_mut() else {
                continue;
            };
            let profile = self.profile_for(&reading.provider);
            let history = self
                .recent
                .get(&view.account.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            fill_forecasts(reading, history, profile.as_ref(), now);
        }
        self.snap.accounts = views;
    }

    fn maybe_save(&mut self, force: bool) {
        if !self.state_dirty
            || (!force && self.last_state_save.elapsed() < self.tuning.save_debounce)
        {
            return;
        }
        match self.state.save(&self.opts.paths.state()) {
            Ok(()) => {
                self.state_dirty = false;
                self.last_state_save = Instant::now();
            }
            Err(err) => {
                log::warn!("state not saved: {err}");
                self.last_state_save = Instant::now();
            }
        }
    }

    fn publish(&mut self, force: bool) {
        let now = (self.clock)();
        let busy = codenotch_providers::claude_auth_state().busy;
        let active = self.claude_active();
        let mut views = std::mem::take(&mut self.snap.accounts);
        for view in &mut views {
            let (activity, sessions) = self
                .activity_report
                .for_account(&view.account, &self.opts.env);
            view.activity = activity;
            view.sessions = sessions;
            let sched = self.sched.get(&view.account.id);
            view.refreshing = sched.is_some_and(|s| s.in_flight);
            view.next_attempt_at = match (
                view.account.enabled,
                sched,
                self.policy(&view.account, active),
            ) {
                (true, Some(s), Some(p)) => {
                    schedule::next_action(s, &p, now, busy).next_attempt_at(now)
                }
                _ => None,
            };
        }
        self.snap.accounts = views;
        self.snap.now = now;
        self.sync_updater_idle();
        let changed = match &self.last_published {
            Some(last) => {
                let mut l = last.clone();
                l.now = now;
                l != self.snap
            }
            None => true,
        };
        if force || changed || self.last_publish_at.elapsed() >= self.tuning.publish_every {
            self.last_published = Some(self.snap.clone());
            self.last_publish_at = Instant::now();
            self.sink
                .emit(Event::SnapshotChanged(Box::new(self.snap.clone())));
        }
    }

    fn notice(&self, english: &str) {
        let lang = self.snap.config.settings.lang();
        self.sink.emit(Event::Notice(tr(lang, english).to_string()));
    }

    fn save_config(&mut self) {
        match self.snap.config.save(&self.opts.paths.config()) {
            Ok(()) => self.snap.storage_error = None,
            Err(err) => {
                log::warn!("config not saved: {err}");
                self.snap.storage_error = Some(err.to_string());
            }
        }
    }

    /// Apply a changed config: views, schedules, persisted state, notify names, activity.
    fn config_changed(&mut self) {
        let now = (self.clock)();
        let mut old: HashMap<String, AccountView> = std::mem::take(&mut self.snap.accounts)
            .into_iter()
            .map(|v| (v.account.id.clone(), v))
            .collect();
        self.snap.accounts = self
            .snap
            .config
            .accounts
            .iter()
            .map(|a| match old.remove(&a.id) {
                Some(mut v) => {
                    v.account = a.clone();
                    v
                }
                None => AccountView::new(a.clone()),
            })
            .collect();
        let ids: HashSet<String> = self
            .snap
            .config
            .accounts
            .iter()
            .map(|a| a.id.clone())
            .collect();
        self.sched.retain(|id, _| ids.contains(id));
        self.no_provider.retain(|id| ids.contains(id));
        self.recent.retain(|id, _| ids.contains(id));
        let before = self.state.clone();
        self.state.readings.retain(|id, _| ids.contains(id));
        self.state.backoff_until.retain(|id, _| ids.contains(id));
        for key in [LAST_ATTEMPT_KEY, CLAUDE_429_KEY] {
            let mut map = int_map(&self.state, key);
            map.retain(|id, _| ids.contains(id));
            set_int_map(&mut self.state, key, &map);
        }
        if self.state != before {
            self.state_dirty = true;
        }
        // New enabled accounts: staggered first fetch.
        let new: Vec<String> = self
            .snap
            .config
            .enabled_accounts()
            .filter(|a| !self.sched.contains_key(&a.id))
            .map(|a| a.id.clone())
            .collect();
        for (i, id) in new.into_iter().enumerate() {
            let not_before = now + self.tuning.stagger_secs * i as i64;
            self.sched.insert(
                id,
                AccountSched {
                    not_before,
                    ..AccountSched::default()
                },
            );
        }
        self.notifier.set_accounts(&self.snap.config.accounts);
        if let Some(activity) = &self.activity {
            activity.update_settings(&self.snap.config.settings, &self.snap.config.accounts);
        }
        let wanted = (
            self.snap.config.settings.update_mode(),
            self.snap.config.settings.update_manifest_url.clone(),
        );
        if wanted != self.updater_settings {
            if let Some(u) = &self.updater {
                u.settings(wanted.0, &wanted.1);
            }
            self.updater_settings = wanted;
        }
    }

    // -----------------------------------------------------------------------------------------
    // Commands
    // -----------------------------------------------------------------------------------------

    fn command(&mut self, command: Command) {
        match command {
            Command::Refresh(Some(id)) => {
                if self.snap.config.account(&id).is_some_and(|a| a.enabled) {
                    let now = (self.clock)();
                    self.sched
                        .entry(id)
                        .or_insert_with(|| AccountSched {
                            not_before: now,
                            ..AccountSched::default()
                        })
                        .manual_pending = true;
                }
            }
            Command::Refresh(None) => {
                let now = (self.clock)();
                let ids: Vec<String> = self
                    .snap
                    .config
                    .enabled_accounts()
                    .map(|a| a.id.clone())
                    .collect();
                for id in ids {
                    self.sched
                        .entry(id)
                        .or_insert_with(|| AccountSched {
                            not_before: now,
                            ..AccountSched::default()
                        })
                        .manual_pending = true;
                }
                self.start_discovery();
            }
            Command::SaveConfig(config) => self.save_config_command(*config),
            Command::OpenSettings => self.sink.emit(Event::OpenSettings),
            Command::OpenUrl(url) => {
                let lower = url.trim().to_ascii_lowercase();
                if !(lower.starts_with("https://") || lower.starts_with("http://")) {
                    self.notice("Only web links can be opened.");
                } else {
                    let url = url.trim().to_string();
                    self.spawn_open(move |system| system.open_url(&url));
                }
            }
            Command::OpenDataFolder => {
                let root = self.opts.paths.root().to_path_buf();
                self.spawn_open(move |system| system.open_path(&root));
            }
            Command::SignIn(id) => match self.snap.config.account(&id).cloned() {
                Some(account) => self.spawn_sign_in(account, false),
                None => self.notice("Unknown account."),
            },
            Command::AddClaudeAccount => self.add_claude_account(),
            Command::InstallHooks => self.spawn_hooks(true),
            Command::RemoveHooks => self.spawn_hooks(false),
            Command::TestTelegram => self.spawn_test(TestTarget::Telegram),
            Command::TestDiscord => self.spawn_test(TestTarget::Discord),
            Command::PreviewResetCard => self.preview_reset_card(),
            Command::CheckUpdate => match &self.updater {
                Some(u) => u.check(true),
                None => {
                    self.snap.update = UpdateState::Failed {
                        message: crate::update::MSG_NOT_STARTED.into(),
                        manual: true,
                    }
                }
            },
            Command::InstallUpdate => {
                if let Some(u) = &self.updater {
                    u.install();
                }
            }
            Command::UiBusy(busy) => {
                self.ui_busy = busy;
                self.sync_updater_idle();
            }
            Command::Recentre => {
                let placement = &mut self.snap.config.settings.widget_placement;
                placement.monitor.clear();
                placement.y = 0.5;
                self.save_config();
            }
            Command::RefreshAnalytics => self.start_analytics(AnalyticsWhy::Requested),
            Command::DesktopState { fullscreen, game } => {
                self.snap.fullscreen = fullscreen;
                self.snap.game_mode = game;
                self.desktop = DesktopContext {
                    fullscreen,
                    game_mode: game,
                };
            }
            Command::SetSecret { key, value } => {
                if let Err(err) = self.opts.secrets.set(&key, value.expose()) {
                    self.sink.emit(Event::Notice(err.to_string()));
                }
                self.snap.secrets = SecretsPresent::detect(&self.opts.env, &*self.opts.secrets);
            }
            Command::ClearSecret(key) => {
                if let Err(err) = self.opts.secrets.delete(&key) {
                    self.sink.emit(Event::Notice(err.to_string()));
                }
                self.snap.secrets = SecretsPresent::detect(&self.opts.env, &*self.opts.secrets);
            }
            Command::ToggleNotch => self.snap.notch_hidden = !self.snap.notch_hidden,
            // Teams sync is not wired into the engine yet: these commands do nothing (no
            // request, no file) and the status stays `Off`.
            Command::TeamsPreviewInvite(_)
            | Command::TeamsPair(_)
            | Command::TeamsDetach
            | Command::TeamsFlush
            | Command::TeamsAllowBackfill(_)
            | Command::TeamsDeclareAccount { .. }
            | Command::TeamsEraseServerData => {
                log::debug!("teams: command ignored (sync not available in this build)");
            }
            Command::Quit => {}
        }
    }

    fn save_config_command(&mut self, mut config: Config) {
        if let Err(message) = validate(&config) {
            self.notice(message);
            return;
        }
        // The UI sends whole configs built from its last snapshot, which may predate an
        // engine-side change (discovery, sign-in, hooks result): keep those.
        keep_engine_owned(&self.snap.config, &mut config, &self.opts.env);
        let launch = config.settings.launch_at_startup;
        self.snap.config = config;
        self.save_config();
        if launch != self.opts.system.launch_at_startup() {
            if let Err(err) = self.opts.system.set_launch_at_startup(launch) {
                self.sink.emit(Event::Notice(err.to_string()));
            }
        }
        self.config_changed();
    }

    fn preview_reset_card(&mut self) {
        let event = self.snap.notch_accounts().first().map(|view| {
            let w = view.headline();
            Event::ResetDetected {
                account_id: view.account.id.clone(),
                window_id: w.map_or_else(|| "session".to_string(), |w| w.id.clone()),
                used_percent: w.and_then(|w| w.used_percent),
                next_reset_at: w.and_then(|w| w.resets_at),
            }
        });
        match event {
            Some(e) => self.sink.emit(e),
            None => self.notice("No account to preview."),
        }
    }

    fn add_claude_account(&mut self) {
        let account = match codenotch_providers::add_claude_account(&self.opts.env) {
            Ok(a) => a,
            Err(err) => {
                self.sink.emit(Event::Notice(err.to_string()));
                return;
            }
        };
        let existing = self
            .snap
            .config
            .accounts
            .iter()
            .find(|a| {
                a.provider == ProviderKind::Claude
                    && !a.uses_custom_endpoint()
                    && discovery::profile_key(a, &self.opts.env)
                        == discovery::profile_key(&account, &self.opts.env)
            })
            .cloned();
        let (target, added) = match existing {
            Some(a) => (a, false),
            None => {
                // Added on purpose: a "removed" marker for this profile no longer applies.
                let env = &self.opts.env;
                self.snap
                    .config
                    .settings
                    .removed_accounts
                    .retain(|m| !discovery::marker_matches(m, &account, env));
                self.snap.config.accounts.push(account.clone());
                self.save_config();
                self.config_changed();
                (account, true)
            }
        };
        self.spawn_sign_in(target, added);
    }

    fn spawn_sign_in(&self, account: Account, remove_on_failure: bool) {
        let env = self.opts.env.clone();
        let registry = self.registry.clone();
        let tx = self.tx.clone();
        let commands = self.commands.clone();
        let spawned = std::thread::Builder::new()
            .name("codenotch-sign-in".into())
            .spawn(move || {
                let id = account.id.clone();
                let result = if account.provider == ProviderKind::Claude
                    && !account.uses_custom_endpoint()
                {
                    let refresh_id = id.clone();
                    codenotch_providers::claude_sign_in(
                        &account,
                        &env,
                        Box::new(move |_ok| {
                            let _ = commands.send(Command::Refresh(Some(refresh_id)));
                        }),
                    )
                } else {
                    match provider_for(&registry, &account) {
                        Some(p) => p.sign_in(&account, &env),
                        None => Err(codenotch_core::Error::unsupported(format!(
                            "sign-in for {}",
                            account.provider
                        ))),
                    }
                };
                if let Err(err) = result {
                    let _ = tx.send(Msg::SignInFailed {
                        account_id: id,
                        remove: remove_on_failure,
                        message: err.to_string(),
                    });
                }
            });
        if let Err(err) = spawned {
            log::error!("cannot start the sign-in thread: {err}");
        }
    }

    /// Open a link or folder on a short-lived thread (the shell can take seconds); a failure
    /// comes back as a notice.
    fn spawn_open(
        &self,
        open: impl FnOnce(&dyn codenotch_core::SystemActions) -> codenotch_core::Result<()>
            + Send
            + 'static,
    ) {
        let system = self.opts.system.clone();
        let sink = self.sink.clone();
        let spawned = std::thread::Builder::new()
            .name("codenotch-open".into())
            .spawn(move || {
                if let Err(err) = open(&*system) {
                    sink.emit(Event::Notice(err.to_string()));
                }
            });
        if let Err(err) = spawned {
            log::error!("cannot start the open thread: {err}");
        }
    }

    fn spawn_test(&self, target: TestTarget) {
        let notifier = Notifier::new(
            self.opts.http.clone(),
            self.opts.env.clone(),
            self.opts.secrets.clone(),
        );
        let settings = self.snap.config.settings.clone();
        let sink = self.sink.clone();
        let spawned = std::thread::Builder::new()
            .name("codenotch-notify-test".into())
            .spawn(move || {
                let lang = settings.lang();
                let message = match notifier.send_test(target, &settings) {
                    Ok(()) => tr(lang, "Test message sent.").to_string(),
                    Err(err) => err.to_string(),
                };
                sink.emit(Event::Notice(message));
            });
        if let Err(err) = spawned {
            log::error!("cannot start the test thread: {err}");
        }
    }

    /// Claude profile folders the hooks go into: every folder the installer finds plus the
    /// existing custom profile folders of configured Claude accounts.
    fn hook_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = codenotch_activity::claude_profile_dirs(&self.opts.env);
        let norm = |p: &std::path::Path| {
            p.to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_lowercase()
        };
        for a in &self.snap.config.accounts {
            if a.provider != ProviderKind::Claude
                || a.uses_custom_endpoint()
                || a.profile_path.trim().is_empty()
            {
                continue;
            }
            let dir = PathBuf::from(a.profile_path.trim());
            if dir.is_dir() && !dirs.iter().any(|d| norm(d) == norm(&dir)) {
                dirs.push(dir);
            }
        }
        dirs
    }

    /// One hooks job at a time: a request while one runs is refused with a notice (two jobs
    /// would read and rewrite the same `settings.json` files concurrently).
    fn spawn_hooks(&mut self, install: bool) {
        if self.hooks_running {
            self.notice("Hooks are already being updated; try again in a moment.");
            return;
        }
        self.hooks_running = true;
        let dirs = self.hook_dirs();
        let exe = self
            .opts
            .hook_exe
            .clone()
            .or_else(codenotch_activity::default_hook_exe);
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("codenotch-hooks".into())
            .spawn(move || {
                let job =
                    || {
                        if dirs.is_empty() {
                            Err("No Claude Code profile folder found.".to_string())
                        } else if install {
                            match exe {
                                Some(exe) if exe.is_file() => {
                                    codenotch_activity::install_hooks_in(&dirs, &exe)
                                        .map_err(|e| e.to_string())
                                }
                                _ => Err("codenotch-hook.exe was not found next to CodeNotch."
                                    .to_string()),
                            }
                        } else {
                            codenotch_activity::remove_hooks_in(&dirs).map_err(|e| e.to_string())
                        }
                    };
                // Always report back, so the one-job-at-a-time gate reopens.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                    .unwrap_or_else(|_| Err("hooks update failed".to_string()));
                let _ = tx.send(Msg::Hooks { install, result });
            });
        if let Err(err) = spawned {
            log::error!("cannot start the hooks thread: {err}");
            self.hooks_running = false;
        }
    }

    fn on_hooks(&mut self, install: bool, result: Result<(), String>) {
        self.hooks_running = false;
        match (&result, install) {
            (Ok(()), true) => self.notice("Hooks installed."),
            (Ok(()), false) => self.notice("Hooks removed."),
            (Err(err), true) => {
                let lang = self.snap.config.settings.lang();
                self.sink.emit(Event::Notice(format!(
                    "{} {}",
                    tr(lang, "Hooks not installed:"),
                    tr(lang, err)
                )));
            }
            (Err(err), false) => {
                let lang = self.snap.config.settings.lang();
                self.sink.emit(Event::Notice(format!(
                    "{} {}",
                    tr(lang, "Hooks not fully removed:"),
                    tr(lang, err)
                )));
            }
        }
        // Removing always turns the switch off; installing only when it worked.
        let wanted = install && result.is_ok();
        if self.snap.config.settings.hooks_enabled != wanted && (result.is_ok() || !install) {
            self.snap.config.settings.hooks_enabled = wanted;
            self.save_config();
            self.config_changed();
        }
    }

    // -----------------------------------------------------------------------------------------
    // Background jobs
    // -----------------------------------------------------------------------------------------

    fn start_discovery(&mut self) {
        if self.discovering {
            return;
        }
        self.discovering = true;
        let registry = self.registry.clone();
        let env = self.opts.env.clone();
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("codenotch-discover".into())
            .spawn(move || {
                let found = discovery::discover_all(&registry, &env);
                let _ = tx.send(Msg::Discovered(found));
            });
        if let Err(err) = spawned {
            log::error!("cannot start discovery: {err}");
            self.discovering = false;
        }
    }

    /// Start an analytics run. Runs are only started by the schedule (`analytics_delay`, then
    /// every `analytics_every`) and by `Command::RefreshAnalytics` (Settings > Statistics:
    /// Refresh button, period change); a config save never starts one by itself.
    fn start_analytics(&mut self, why: AnalyticsWhy) {
        if self.analytics_running {
            if why == AnalyticsWhy::Requested && !self.analytics_rerun {
                self.analytics_rerun = true;
                log::debug!("analytics: requested by UI while running, queued once");
            } else {
                log::debug!(
                    "analytics: {} run skipped, one is already running",
                    why.label()
                );
            }
            return;
        }
        log::debug!("analytics: starting ({})", why.label());
        self.analytics_running = true;
        let config = self.snap.config.clone();
        let env = self.opts.env.clone();
        let paths = self.opts.paths.clone();
        let now = (self.clock)();
        let plan_names: HashMap<String, String> = self
            .snap
            .accounts
            .iter()
            .filter_map(|v| Some((v.account.id.clone(), v.reading.as_ref()?.plan.clone()?)))
            .collect();
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("codenotch-analytics".into())
            .spawn(move || {
                let result =
                    codenotch_analytics::compute_report(&config, &env, &paths, now, &plan_names)
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                let _ = tx.send(Msg::Analytics(result));
            });
        if let Err(err) = spawned {
            log::error!("cannot start analytics: {err}");
            self.analytics_running = false;
        }
    }

    fn start_updater(&mut self) {
        let setup = crate::updater_setup(
            &self.opts.paths,
            &self.opts.env,
            self.opts.http.clone(),
            self.clock.clone(),
            &self.snap.config.settings,
            &self.opts.update,
            &self.tuning,
        );
        let tx = self.tx.clone();
        self.updater = Some(update::Updater::start(
            setup,
            Box::new(move |report| {
                let _ = tx.send(Msg::Update(report));
            }),
        ));
    }

    /// Idle for an automatic install: no agent working or waiting, the user not busy in a
    /// CodeNotch window. Sent to the updater when it changes.
    fn sync_updater_idle(&mut self) {
        let Some(updater) = &self.updater else {
            return;
        };
        let busy_agent = matches!(
            self.snap.activity(),
            ActivityState::Working | ActivityState::Waiting
        );
        let idle = !busy_agent && !self.ui_busy;
        if self.updater_idle != Some(idle) {
            self.updater_idle = Some(idle);
            updater.idle(idle);
        }
    }

    /// A report from the updater. False = the new version runs: stop the engine and quit.
    fn on_update(&mut self, report: update::Report) -> bool {
        match report {
            update::Report::State { state, last_check } => {
                self.snap.update = state;
                self.snap.update_checked_at = last_check;
            }
            update::Report::NewVersion { version } => self.notify_new_version(&version),
            update::Report::Restart => {
                log::info!("update: quitting for the new version");
                self.publish(true);
                self.shutdown();
                self.sink.emit(Event::Quit);
                return false;
            }
        }
        true
    }

    /// Windows notification for a newly found version (notify mode, notifications on, not in
    /// game mode or behind a full-screen app).
    fn notify_new_version(&self, version: &str) {
        let s = &self.snap.config.settings;
        if s.update_mode() != UpdateMode::Notify
            || !s.notifications
            || s.game_mode
            || self.desktop.game_mode
            || self.desktop.fullscreen
        {
            return;
        }
        let Some(toast) = self.toast.clone() else {
            return;
        };
        let lang = s.lang();
        let title = codenotch_core::i18n::fill(
            tr(lang, "CodeNotch {version} is available"),
            &[("version", version)],
        );
        let body = tr(lang, "Open CodeNotch to install it.").to_string();
        let spawned = std::thread::Builder::new()
            .name("codenotch-update-toast".into())
            .spawn(move || {
                if let Err(err) = toast.show(&title, &body) {
                    log::debug!("update notification not shown: {err}");
                }
            });
        if let Err(err) = spawned {
            log::warn!("cannot start the notification thread: {err}");
        }
    }

    fn start_activity(&mut self) {
        let tx = self.tx.clone();
        let on_change: Box<dyn Fn(ActivityReport) + Send + Sync> = Box::new(move |report| {
            let _ = tx.send(Msg::Activity(report));
        });
        match codenotch_activity::start(
            &self.opts.paths,
            &self.opts.env,
            &self.snap.config.settings,
            &self.snap.config.accounts,
            on_change,
        ) {
            Ok(handle) => {
                self.activity_report = handle.report();
                self.activity = Some(handle);
            }
            Err(err) => log::warn!("activity not started: {err}"),
        }
    }
}

/// Why an analytics run starts (for the debug log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnalyticsWhy {
    Scheduled,
    Requested,
    Queued,
}

impl AnalyticsWhy {
    fn label(self) -> &'static str {
        match self {
            AnalyticsWhy::Scheduled => "scheduled",
            AnalyticsWhy::Requested => "requested by UI",
            AnalyticsWhy::Queued => "UI request queued during the previous run",
        }
    }
}

/// Invariants applied to a config coming from the UI (`SaveConfig`) against the engine's
/// current one:
/// * `hooksEnabled` is engine-owned (only `InstallHooks`/`RemoveHooks` change it);
/// * an account the engine has that is missing from `incoming` counts as deleted when it is a
///   custom-endpoint account or `incoming` carries a `removedAccounts` marker for it (the UI
///   removed it); any other is kept, at its former position as far as possible (it was added by
///   discovery or sign-in after the UI's snapshot);
/// * everything else — including unknown keys — comes from `incoming`.
pub(crate) fn keep_engine_owned(
    current: &Config,
    incoming: &mut Config,
    env: &codenotch_core::Env,
) {
    incoming.settings.hooks_enabled = current.settings.hooks_enabled;
    for (index, account) in current.accounts.iter().enumerate() {
        if account.uses_custom_endpoint()
            || incoming.accounts.iter().any(|a| a.id == account.id)
            || discovery::is_removed(incoming, account, env)
        {
            continue;
        }
        let at = index.min(incoming.accounts.len());
        incoming.accounts.insert(at, account.clone());
    }
}

/// Reject configs the engine cannot work with (the UI keeps its edit).
pub(crate) fn validate(config: &Config) -> Result<(), &'static str> {
    let mut seen = HashSet::new();
    for a in &config.accounts {
        if a.id.trim().is_empty() {
            return Err("Settings not saved: an account has no id.");
        }
        if !seen.insert(a.id.as_str()) {
            return Err("Settings not saved: two accounts share the same id.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use crate::Tuning;
    use codenotch_core::{Env, FakeHttp, MemorySecrets, NoopSystem, Paths};

    /// An engine driven by hand (no engine thread): commands are handled synchronously, so
    /// "a second request while the first job runs" is deterministic.
    fn engine(dir: &std::path::Path) -> (Engine, Receiver<Msg>, Receiver<Event>) {
        let opts = EngineOptions {
            paths: Paths::at(dir.join("data")),
            env: Env::for_test(dir).with_var("CODENOTCH_NO_SPAWN", "1"),
            http: Arc::new(FakeHttp::new()),
            system: Arc::new(NoopSystem),
            secrets: Arc::new(MemorySecrets::new()),
            toast: None,
            providers: Some(Arc::new(Vec::new())),
            clock: Some(Arc::new(|| 1_800_000_000)),
            hook_exe: Some(dir.join("bin").join("codenotch-hook.exe")),
            storage_error: None,
            tuning: Tuning {
                activity: false,
                ..Tuning::default()
            },
            update: crate::UpdateOptions::default(),
        };
        let (sink, events) = EventSink::channel();
        let (tx, rx) = std::sync::mpsc::channel();
        let (commands, _commands_rx) = std::sync::mpsc::channel();
        let e = Engine::new(
            Config::default(),
            opts,
            sink,
            tx,
            commands,
            Arc::new(Mutex::new(None)),
        );
        (e, rx, events)
    }

    fn notices(events: &Receiver<Event>) -> Vec<String> {
        events
            .try_iter()
            .filter_map(|e| match e {
                Event::Notice(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn one_hooks_job_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let (mut e, rx, events) = engine(dir.path());
        e.command(Command::InstallHooks);
        e.command(Command::RemoveHooks);
        e.command(Command::InstallHooks);
        let busy = notices(&events);
        assert_eq!(busy.len(), 2, "{busy:?}");
        assert!(
            busy.iter().all(|n| n.contains("already being updated")),
            "{busy:?}"
        );
        // The first job reports back (no profile folder here) and the gate reopens.
        let msg = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(matches!(msg, Msg::Hooks { install: true, .. }));
        assert!(e.handle(msg));
        assert!(notices(&events)
            .iter()
            .any(|n| n.contains("No Claude Code profile")));
        e.command(Command::RemoveHooks);
        assert!(
            notices(&events).is_empty(),
            "accepted once the first job is done"
        );
        let msg = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(matches!(msg, Msg::Hooks { install: false, .. }));
    }

    fn next_analytics(rx: &Receiver<Msg>) -> Msg {
        loop {
            let msg = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            if matches!(msg, Msg::Analytics(_)) {
                return msg;
            }
        }
    }

    #[test]
    fn ui_requests_during_a_run_coalesce_into_one_rerun() {
        let dir = tempfile::tempdir().unwrap();
        let (mut e, rx, _events) = engine(dir.path());
        e.command(Command::RefreshAnalytics);
        assert!(e.analytics_running && !e.analytics_rerun);
        // Two more clicks while the first run is in flight: one rerun, not two.
        e.command(Command::RefreshAnalytics);
        e.command(Command::RefreshAnalytics);
        assert!(e.analytics_rerun);
        e.handle(next_analytics(&rx));
        assert!(e.analytics_running, "the queued rerun started");
        assert!(!e.analytics_rerun);
        e.handle(next_analytics(&rx));
        assert!(!e.analytics_running, "nothing more queued");
        assert!(rx.try_iter().all(|m| !matches!(m, Msg::Analytics(_))));
    }

    #[test]
    fn scheduled_tick_during_a_run_is_dropped_and_config_saves_do_not_run_analytics() {
        let dir = tempfile::tempdir().unwrap();
        let (mut e, rx, _events) = engine(dir.path());
        let mut config = e.snap.config.clone();
        config.settings.analytics_days = 30;
        e.command(Command::SaveConfig(Box::new(config)));
        assert!(
            !e.analytics_running,
            "a config save alone never starts a run"
        );
        e.command(Command::RefreshAnalytics);
        e.start_analytics(AnalyticsWhy::Scheduled);
        assert!(!e.analytics_rerun, "a scheduled tick is not queued");
        e.handle(next_analytics(&rx));
        assert!(!e.analytics_running);
        assert_eq!(e.snap.analytics.as_ref().unwrap().days, 30);
    }
}
