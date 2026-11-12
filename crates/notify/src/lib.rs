//! # codenotch-notify
//!
//! Decides when the user should be told something and tells them.
//!
//! * **Threshold alerts** — a window's used percent crosses `settings.warn_percent()` (Warn) or
//!   `critical_percent()` (Critical) upward between two fresh readings of this run. Never from a
//!   first, stale, restored, signed-out or error reading; an unknown percent is never 0. One
//!   alert per window, kind and reset period, de-duplicated across restarts in
//!   `PersistedState::alerts_sent` (key `"<account>/<window>/<warn|critical|forecast>/<resets_at|none>"`,
//!   value = when sent; reset times within 10 min count as the same period). Re-armed when the
//!   window resets (new period) or drops [`REARM_MARGIN`] points below the threshold.
//! * **Forecast alerts** — [`forecast_minutes_left`] (Theil–Sen slope over the last hour of the
//!   window's history, after its last reset) ≤ `forecastMinutes` and before the window's own
//!   reset: `AlertKind::Forecast { minutes_left }`, once per window per period.
//! * **Reset detection** — [`ResetWatcher`]. Gives `Event::ResetDetected`
//!   (when `resetCards`) and an `AlertKind::Reset` alert (when `resetAlerts`).
//! * **Gating** — `notifications` is the master switch for every alert (reset cards are
//!   governed by `resetCards` alone). While a full-screen app or game is in front
//!   ([`DesktopContext`], or `settings.game_mode`), on-screen output (toast, `Event::Alert`,
//!   `Event::ResetDetected`) is held back, except Critical alerts when `criticalInFullscreen`.
//!   Telegram / Discord are not affected by full screen.
//! * **Channels** — Windows toast through a [`ToastSink`] ([`WinToast`] in the app; none by
//!   default), Telegram (`sendMessage`, token = `resolve_secret(SecretKey::TelegramToken)`, chat
//!   id = `settings.telegramChatId`) when `telegramEnabled`, Discord (webhook =
//!   `resolve_secret(SecretKey::DiscordWebhook)`) when `discordEnabled`. Secrets are resolved
//!   at send time and never logged or put in an error.
//!
//! ## Threading
//! [`Notifier`] lives on the **engine thread**. [`Notifier::process`] is cheap: it decides,
//! updates `state`, returns the events, and hands delivery (toast + HTTP, which may take up to
//! ~30 s in the worst case) to a short-lived `codenotch-notify` thread ([`Delivery::Background`],
//! the default). [`Notifier::deliver`] and [`Notifier::send_test`] are **synchronous and
//! blocking**: call them from a worker thread (e.g. a thread spawned by the engine for
//! `Command::TestTelegram`), never from the UI thread.

mod channels;
mod forecast;
pub mod messages;
mod reset;
pub mod toast;

use codenotch_core::history::HistoryPoint;
use codenotch_core::model::{
    Alert, AlertKind, Event, LimitWindow, Reading, ReadingStatus, STALE_AFTER_SECS,
};
use codenotch_core::secrets::SecretStore;
use codenotch_core::state::PersistedState;
use codenotch_core::{Account, Env, Error, HttpClient, Result, Settings};
use std::collections::HashMap;
use std::sync::Arc;

pub use channels::{MAX_RETRY_AFTER_SECS, REQUEST_TIMEOUT, TELEGRAM_API};
#[cfg(windows)]
pub use toast::WinToast;
pub use toast::{RecordingToast, ToastSink, APP_USER_MODEL_ID};

/// Env var holding the Discord webhook URL (re-exported from core).
pub use codenotch_core::secrets::DISCORD_WEBHOOK_ENV;
/// Env var holding the Telegram bot token (re-exported from core).
pub use codenotch_core::secrets::TELEGRAM_TOKEN_ENV;

/// Points below a threshold the usage must fall before that threshold can fire again in the
/// same reset period.
pub const REARM_MARGIN: f64 = 5.0;
/// Reset times closer than this are the same period (providers jitter by a few seconds).
pub const PERIOD_TOLERANCE_SECS: i64 = 10 * 60;
/// `alerts_sent` entries older than this are dropped.
pub const ALERTS_SENT_KEEP_SECS: i64 = 35 * 86_400;

/// What a test button targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestTarget {
    Telegram,
    Discord,
    Toast,
}

/// Desktop context for gating alerts. Fill it from the engine's latest
/// `Command::DesktopState { fullscreen, game }` (the same values copied into
/// `Snapshot::fullscreen` / `Snapshot::game_mode`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DesktopContext {
    /// A full-screen app is in front of the notch's monitor.
    pub fullscreen: bool,
    /// Game mode (config switch or a detected game).
    pub game_mode: bool,
}

/// Inputs for one [`Notifier::process`] call.
#[derive(Debug, Clone, Copy)]
pub struct AlertInput<'a> {
    /// The reading this one replaces (None on the first reading).
    pub previous: Option<&'a Reading>,
    pub reading: &'a Reading,
    /// Recent samples of this account (for forecasts), oldest first. At least the last hour.
    pub history: &'a [HistoryPoint],
    pub settings: &'a Settings,
    pub desktop: DesktopContext,
    pub now: i64,
}

/// Where [`Notifier::process`] performs delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Delivery {
    /// On a short-lived helper thread (default; the engine never waits for HTTP).
    #[default]
    Background,
    /// On the calling thread (tests, CLI).
    Inline,
}

/// Alert engine + delivery. One per app, owned by the engine thread.
pub struct Notifier {
    remote: channels::Remote,
    resets: ResetWatcher,
    toast: Option<Arc<dyn ToastSink>>,
    names: HashMap<String, String>,
    /// Last fresh percent seen in this run, per (account, window).
    baselines: HashMap<(String, String), f64>,
    delivery: Delivery,
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notifier")
            .field("toast", &self.toast.is_some())
            .field("delivery", &self.delivery)
            .field("baselines", &self.baselines.len())
            .finish()
    }
}

impl Notifier {
    /// `secrets`: the app's store (Credential Manager in production, `MemorySecrets` in tests);
    /// values are looked up with `resolve_secret(key, &env, &*secrets)` at send time.
    ///
    /// No toast backend is installed by default: the app calls
    /// `with_toast(Arc::new(WinToast::new()))`. Without one, alerts still reach the UI as
    /// `Event::Alert` (and Telegram / Discord).
    pub fn new(http: Arc<dyn HttpClient>, env: Env, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            remote: channels::Remote {
                http,
                env,
                secrets,
                sleep: Arc::new(std::thread::sleep),
            },
            resets: ResetWatcher::new(),
            toast: None,
            names: HashMap::new(),
            baselines: HashMap::new(),
            delivery: Delivery::Background,
        }
    }

    /// Builder: install the on-screen notification backend.
    pub fn with_toast(mut self, sink: Arc<dyn ToastSink>) -> Self {
        self.toast = Some(sink);
        self
    }

    /// Replace (or remove) the on-screen notification backend.
    pub fn set_toast(&mut self, sink: Option<Arc<dyn ToastSink>>) {
        self.toast = sink;
    }

    /// True when a toast backend is installed.
    pub fn has_toasts(&self) -> bool {
        self.toast.is_some()
    }

    /// Choose where `process` delivers (default [`Delivery::Background`]).
    pub fn set_delivery(&mut self, delivery: Delivery) {
        self.delivery = delivery;
    }

    /// Account names used in alert texts (call at start and after every config change).
    /// Without it the provider's display name is used.
    pub fn set_accounts(&mut self, accounts: &[Account]) {
        self.names = accounts
            .iter()
            .map(|a| (a.id.clone(), account_label(a)))
            .collect();
    }

    #[cfg(test)]
    pub(crate) fn set_sleeper(&mut self, sleep: channels::Sleeper) {
        self.remote.sleep = sleep;
    }

    fn display_name(&self, reading: &Reading) -> String {
        self.names
            .get(&reading.account_id)
            .cloned()
            .unwrap_or_else(|| reading.provider.display_name().to_string())
    }

    /// Process one new reading (call for **every** reading of every account, including stale,
    /// error and signed-out ones: they reset the detectors). Decides alerts, records them in
    /// `state.alerts_sent` (the engine persists `state`), starts delivery, and returns the
    /// events for the UI: `Event::ResetDetected` (reset card) and `Event::Alert` (only for
    /// alerts allowed on screen right now).
    pub fn process(&mut self, input: &AlertInput<'_>, state: &mut PersistedState) -> Vec<Event> {
        let AlertInput {
            previous,
            reading,
            history,
            settings,
            desktop,
            now,
        } = *input;
        let lang = settings.lang();
        let name = self.display_name(reading);
        let mut events = Vec::new();
        let mut raised: Vec<Alert> = Vec::new();

        for ev in self.resets.observe(reading, now) {
            if settings.reset_cards && on_screen_allowed(false, desktop, settings) {
                events.push(Event::ResetDetected {
                    account_id: ev.account_id.clone(),
                    window_id: ev.window_id.clone(),
                    used_percent: ev.used_percent,
                    next_reset_at: ev.next_reset_at,
                });
            }
            if settings.notifications && settings.reset_alerts {
                raised.push(messages::reset_alert(
                    &ev.account_id,
                    &name,
                    &ev.window_id,
                    &ev.label_key,
                    ev.used_percent,
                    ev.next_reset_at,
                    lang,
                    now,
                ));
            }
        }

        if reset::is_fresh(reading, now) {
            prune_sent(state, now);
            for w in alert_windows(reading) {
                let Some(used) = w.used_percent.filter(|u| u.is_finite()) else {
                    continue; // unknown: keep the old baseline, never 0
                };
                let key = (reading.account_id.clone(), w.id.clone());
                let baseline = self
                    .baselines
                    .get(&key)
                    .copied()
                    .or_else(|| previous_baseline(previous, reading, w, now));
                self.baselines.insert(key, used);
                rearm(state, &reading.account_id, &w.id, used, settings);
                let Some(prev) = baseline else {
                    continue; // first observation in this run: never alert
                };
                if !settings.notifications {
                    continue;
                }
                if let Some(alert) =
                    self.threshold(state, reading, w, prev, used, &name, settings, now)
                {
                    raised.push(alert);
                } else if let Some(alert) =
                    self.forecast(state, reading, w, used, history, &name, settings, now)
                {
                    raised.push(alert);
                }
            }
        }

        let mut jobs = Vec::with_capacity(raised.len());
        for alert in raised {
            let screen = on_screen_allowed(alert.kind == AlertKind::Critical, desktop, settings);
            if screen {
                events.push(Event::Alert(alert.clone()));
            }
            jobs.push((alert, screen));
        }
        self.dispatch(jobs, settings);
        events
    }

    #[allow(clippy::too_many_arguments)]
    fn threshold(
        &self,
        state: &mut PersistedState,
        reading: &Reading,
        w: &LimitWindow,
        prev: f64,
        used: f64,
        name: &str,
        settings: &Settings,
        now: i64,
    ) -> Option<Alert> {
        let (warn, crit) = (settings.warn_percent(), settings.critical_percent());
        let kind = if prev < crit && used >= crit {
            AlertKind::Critical
        } else if prev < warn && used >= warn {
            AlertKind::Warn
        } else {
            return None;
        };
        let tag = if kind == AlertKind::Critical {
            "critical"
        } else {
            "warn"
        };
        if already_sent(state, &reading.account_id, &w.id, tag, w.resets_at) {
            return None;
        }
        record_sent(state, &reading.account_id, &w.id, tag, w.resets_at, now);
        if kind == AlertKind::Critical
            && !already_sent(state, &reading.account_id, &w.id, "warn", w.resets_at)
        {
            record_sent(state, &reading.account_id, &w.id, "warn", w.resets_at, now);
        }
        Some(messages::threshold_alert(
            kind,
            &reading.account_id,
            name,
            w,
            settings.lang(),
            now,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn forecast(
        &self,
        state: &mut PersistedState,
        reading: &Reading,
        w: &LimitWindow,
        used: f64,
        history: &[HistoryPoint],
        name: &str,
        settings: &Settings,
        now: i64,
    ) -> Option<Alert> {
        if !settings.forecast_alerts || settings.forecast_minutes == 0 || used >= 100.0 {
            return None;
        }
        let minutes = forecast_minutes_left(w, history, now)?;
        let limit = i64::try_from(settings.forecast_minutes).unwrap_or(i64::MAX);
        if minutes < 1 || minutes > limit {
            return None;
        }
        if w.resets_at.is_some_and(|r| now + minutes * 60 >= r) {
            return None; // the window renews before it runs out
        }
        if already_sent(state, &reading.account_id, &w.id, "forecast", w.resets_at) {
            return None;
        }
        record_sent(
            state,
            &reading.account_id,
            &w.id,
            "forecast",
            w.resets_at,
            now,
        );
        Some(messages::forecast_alert(
            &reading.account_id,
            name,
            w,
            minutes,
            settings.lang(),
            now,
        ))
    }

    fn dispatch(&self, jobs: Vec<(Alert, bool)>, settings: &Settings) {
        if jobs.is_empty() {
            return;
        }
        let remote = self.remote.clone();
        let toast = self.toast.clone();
        let settings = settings.clone();
        let work = move || {
            for (alert, screen) in jobs {
                for (target, result) in
                    deliver_all(&remote, toast.as_deref(), &alert, &settings, screen)
                {
                    match result {
                        Ok(()) => log::debug!("alert delivered via {target:?}"),
                        // Errors are secret-free by construction (see channels.rs).
                        Err(err) => log::warn!("alert delivery via {target:?} failed: {err}"),
                    }
                }
            }
        };
        match self.delivery {
            Delivery::Inline => work(),
            Delivery::Background => {
                if let Err(err) = std::thread::Builder::new()
                    .name("codenotch-notify".into())
                    .spawn(work)
                {
                    log::warn!("cannot start the alert delivery thread: {err}");
                }
            }
        }
    }

    /// Send a clearly labelled test message on one channel (synchronous, blocking: call from a
    /// worker thread). Works whether or not the channel is enabled, so the user can try it
    /// before switching it on. The error text is localised, short and secret-free: show it.
    pub fn send_test(&self, target: TestTarget, settings: &Settings) -> Result<()> {
        let lang = settings.lang();
        let channel = match target {
            TestTarget::Telegram => "Telegram",
            TestTarget::Discord => "Discord",
            TestTarget::Toast => "Windows",
        };
        let (title, body) = messages::test_message(lang, channel);
        match target {
            TestTarget::Telegram => self.remote.telegram(settings, &title, &body),
            TestTarget::Discord => self.remote.discord(settings, &title, &body),
            TestTarget::Toast => show_toast(self.toast.as_deref(), lang, &title, &body),
        }
    }

    /// Deliver an already built alert on every enabled channel (toast when `notifications` and
    /// a backend is installed, Telegram when `telegramEnabled`, Discord when `discordEnabled`).
    /// Synchronous and blocking: call from a worker thread. Ignores the desktop context.
    pub fn deliver(&self, alert: &Alert, settings: &Settings) -> Vec<(TestTarget, Result<()>)> {
        deliver_all(&self.remote, self.toast.as_deref(), alert, settings, true)
    }
}

fn show_toast(
    sink: Option<&dyn ToastSink>,
    lang: codenotch_core::Lang,
    title: &str,
    body: &str,
) -> Result<()> {
    match sink {
        Some(sink) => sink.show(title, body).map_err(|e| {
            Error::other(messages::tf(
                lang,
                "Windows notification failed: {reason}",
                &[("reason", &e.to_string())],
            ))
        }),
        None => Err(Error::other(messages::t(
            lang,
            "Windows notifications are not available.",
        ))),
    }
}

fn deliver_all(
    remote: &channels::Remote,
    toast: Option<&dyn ToastSink>,
    alert: &Alert,
    settings: &Settings,
    on_screen: bool,
) -> Vec<(TestTarget, Result<()>)> {
    let mut out = Vec::new();
    if on_screen && settings.notifications {
        out.push((
            TestTarget::Toast,
            show_toast(toast, settings.lang(), &alert.title, &alert.body),
        ));
    }
    if settings.telegram_enabled {
        out.push((
            TestTarget::Telegram,
            remote.telegram(settings, &alert.title, &alert.body),
        ));
    }
    if settings.discord_enabled {
        out.push((
            TestTarget::Discord,
            remote.discord(settings, &alert.title, &alert.body),
        ));
    }
    out
}

/// "Claude · perso" — the account name, prefixed with the provider when it does not say it.
fn account_label(a: &Account) -> String {
    let provider = a.provider.display_name();
    let name = a.name.trim();
    if name.is_empty() {
        provider.to_string()
    } else if name.to_lowercase().contains(&provider.to_lowercase()) {
        name.to_string()
    } else {
        format!("{provider} · {name}")
    }
}

/// On-screen output allowed now? Full screen / game hold everything but critical alerts (and
/// those only with `criticalInFullscreen`).
fn on_screen_allowed(critical: bool, desktop: DesktopContext, settings: &Settings) -> bool {
    let busy = desktop.fullscreen || desktop.game_mode || settings.game_mode;
    !busy || (critical && settings.critical_in_fullscreen)
}

/// Windows that can raise threshold / forecast alerts: known-percent windows without a count,
/// ungrouped ones plus the headline and weekly windows (even when grouped).
fn alert_windows(reading: &Reading) -> Vec<&LimitWindow> {
    if !reading.shows_figures() {
        return Vec::new();
    }
    let head = reading.headline().map(|w| w.id.clone());
    let weekly = reading.weekly().map(|w| w.id.clone());
    let mut seen = std::collections::HashSet::new();
    reading
        .windows
        .iter()
        .filter(|w| w.count.is_none())
        .filter(|w| {
            w.group.is_none() || Some(&w.id) == head.as_ref() || Some(&w.id) == weekly.as_ref()
        })
        .filter(|w| seen.insert(w.id.clone()))
        .collect()
}

/// The previous reading as a baseline: only when it is a fresh live reading of the same account,
/// older than this one and not stale.
fn previous_baseline(
    previous: Option<&Reading>,
    reading: &Reading,
    w: &LimitWindow,
    now: i64,
) -> Option<f64> {
    let p = previous?;
    let ok = p.account_id == reading.account_id
        && p.status == ReadingStatus::Fresh
        && p.fetched_at > 0
        && p.fetched_at < reading.fetched_at
        && now - p.fetched_at <= STALE_AFTER_SECS;
    if !ok {
        return None;
    }
    p.window(&w.id)?.used_percent.filter(|u| u.is_finite())
}

fn sent_prefix(account: &str, window: &str, tag: &str) -> String {
    format!("{account}/{window}/{tag}/")
}

fn same_period(stored: Option<i64>, current: Option<i64>) -> bool {
    match (stored, current) {
        (Some(a), Some(b)) => (a - b).abs() <= PERIOD_TOLERANCE_SECS,
        // One side unknown: assume the same period (never send twice by accident).
        _ => true,
    }
}

fn already_sent(
    state: &PersistedState,
    account: &str,
    window: &str,
    tag: &str,
    resets_at: Option<i64>,
) -> bool {
    let prefix = sent_prefix(account, window, tag);
    state
        .alerts_sent
        .range(prefix.clone()..)
        .take_while(|(k, _)| k.starts_with(&prefix))
        .any(|(k, _)| same_period(k[prefix.len()..].parse::<i64>().ok(), resets_at))
}

fn record_sent(
    state: &mut PersistedState,
    account: &str,
    window: &str,
    tag: &str,
    resets_at: Option<i64>,
    now: i64,
) {
    let epoch = resets_at.map_or_else(|| "none".to_string(), |r| r.to_string());
    state
        .alerts_sent
        .insert(format!("{}{epoch}", sent_prefix(account, window, tag)), now);
}

fn remove_sent(state: &mut PersistedState, account: &str, window: &str, tag: &str) {
    let prefix = sent_prefix(account, window, tag);
    state.alerts_sent.retain(|k, _| !k.starts_with(&prefix));
}

/// Usage fell clearly below a threshold: that threshold may fire again.
fn rearm(state: &mut PersistedState, account: &str, window: &str, used: f64, settings: &Settings) {
    if used < settings.warn_percent() - REARM_MARGIN {
        remove_sent(state, account, window, "warn");
    }
    if used < settings.critical_percent() - REARM_MARGIN {
        remove_sent(state, account, window, "critical");
    }
}

fn prune_sent(state: &mut PersistedState, now: i64) {
    state
        .alerts_sent
        .retain(|_, sent| now - *sent <= ALERTS_SENT_KEEP_SECS);
}

/// A detected reset.
#[derive(Debug, Clone, PartialEq)]
pub struct ResetEvent {
    pub account_id: String,
    pub window_id: String,
    pub label_key: String,
    pub used_percent: Option<f64>,
    pub next_reset_at: Option<i64>,
}

/// Reset detector for every account (fresh-only, ≥ 10 % peak, reset time rolled
/// ≥ 60 s or usage dropped ≥ 10 points, 15 min cool-down per window, grouped and count windows
/// skipped). In-memory only: after a restart the first fresh reading is a baseline.
#[derive(Debug, Default)]
pub struct ResetWatcher {
    accounts: HashMap<String, reset::AccountWatch>,
}

impl ResetWatcher {
    pub fn new() -> Self {
        Self::default()
    }
    /// Feed a reading; returns the resets it reveals. A non-fresh reading clears that
    /// account's state and returns nothing.
    pub fn observe(&mut self, reading: &Reading, now: i64) -> Vec<ResetEvent> {
        if !reset::is_fresh(reading, now) {
            self.accounts.remove(&reading.account_id);
            return Vec::new();
        }
        let watch = self.accounts.entry(reading.account_id.clone()).or_default();
        reset::observe_account(watch, reading)
            .into_iter()
            .map(|(window_id, label_key, used, next_reset_at)| ResetEvent {
                account_id: reading.account_id.clone(),
                window_id,
                label_key,
                used_percent: Some(used),
                next_reset_at,
            })
            .collect()
    }
}

/// Minutes until `window` reaches 100 % at the recent pace (from `history` of that window),
/// `None` when unknown or not rising. Pure. `history` may hold other windows: only samples
/// whose canonical label matches `window.label_key` are used. `Some(0)` when already at 100 %.
pub fn forecast_minutes_left(
    window: &LimitWindow,
    history: &[HistoryPoint],
    now: i64,
) -> Option<i64> {
    forecast::minutes_left(window, history, now)
}

#[cfg(test)]
mod tests;
