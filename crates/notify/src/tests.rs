//! Behaviour tests with an injected clock, FakeHttp, a recording toast sink and a fake sleeper.
//! No network, no real toast, no registry, no real secrets.

use super::*;
use codenotch_core::model::{ProviderKind, WindowKind};
use codenotch_core::{FakeHttp, HttpError, HttpResponse, MemorySecrets, SecretKey};
use std::sync::Mutex;
use std::time::Duration;

const NOW: i64 = 1_700_000_000;
const R1: i64 = NOW + 3 * 3600;
const TOKEN: &str = "123456789:AAHsecretTOKENsecretTOKEN_xyz";
const CHAT: &str = "987654321";
const HOOK: &str = "https://discord.com/api/webhooks/4242/hookSECRETvalue_abc";
const TG_URL: &str = "https://api.telegram.org/bot";

struct Rig {
    n: Notifier,
    http: Arc<FakeHttp>,
    toast: Arc<RecordingToast>,
    sleeps: Arc<Mutex<Vec<Duration>>>,
    secrets: Arc<MemorySecrets>,
    state: PersistedState,
    prev: Option<Reading>,
    settings: Settings,
    desktop: DesktopContext,
    history: Vec<HistoryPoint>,
}

fn rig_with(http: FakeHttp) -> Rig {
    let http = Arc::new(http);
    let secrets = Arc::new(MemorySecrets::new());
    let toast = Arc::new(RecordingToast::new());
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let env = Env::for_test(std::path::Path::new("Z:/notify-test"));
    let mut n = Notifier::new(http.clone(), env, secrets.clone()).with_toast(toast.clone());
    n.set_delivery(Delivery::Inline);
    let rec = sleeps.clone();
    n.set_sleeper(Arc::new(move |d| rec.lock().unwrap().push(d)));
    Rig {
        n,
        http,
        toast,
        sleeps,
        secrets,
        state: PersistedState::default(),
        prev: None,
        settings: Settings::default(),
        desktop: DesktopContext::default(),
        history: Vec::new(),
    }
}

fn rig() -> Rig {
    rig_with(FakeHttp::new())
}

fn window(used: Option<f64>, resets: Option<i64>) -> LimitWindow {
    let mut w = LimitWindow::new("session", "Current session", WindowKind::Session);
    w.used_percent = used;
    w.resets_at = resets;
    w
}

fn reading_with(at: i64, windows: Vec<LimitWindow>) -> Reading {
    Reading {
        account_id: "acc-1".into(),
        provider: ProviderKind::Claude,
        fetched_at: at,
        status: ReadingStatus::Fresh,
        windows,
        identity: None,
        plan: None,
        note: None,
        error: None,
        headline_window: None,
        weekly_window: None,
    }
}

fn reading(at: i64, used: Option<f64>, resets: Option<i64>) -> Reading {
    reading_with(at, vec![window(used, resets)])
}

impl Rig {
    fn feed(&mut self, r: Reading) -> Vec<Event> {
        let now = r.fetched_at.max(1);
        self.feed_at(r, now)
    }
    fn feed_at(&mut self, r: Reading, now: i64) -> Vec<Event> {
        let input = AlertInput {
            previous: self.prev.as_ref(),
            reading: &r,
            history: &self.history,
            settings: &self.settings,
            desktop: self.desktop,
            now,
        };
        let events = self.n.process(&input, &mut self.state);
        self.prev = Some(r);
        events
    }
    fn enable_telegram(&mut self) {
        self.settings.telegram_enabled = true;
        self.settings.telegram_chat_id = CHAT.into();
        self.secrets.set(&SecretKey::TelegramToken, TOKEN).unwrap();
    }
    fn enable_discord(&mut self) {
        self.settings.discord_enabled = true;
        self.secrets.set(&SecretKey::DiscordWebhook, HOOK).unwrap();
    }
}

fn alerts(events: &[Event]) -> Vec<&Alert> {
    events
        .iter()
        .filter_map(|e| {
            if let Event::Alert(a) = e {
                Some(a)
            } else {
                None
            }
        })
        .collect()
}

fn kinds(events: &[Event]) -> Vec<AlertKind> {
    alerts(events).into_iter().map(|a| a.kind.clone()).collect()
}

fn assert_secret_free(text: &str) {
    for s in [
        TOKEN,
        "AAHsecretTOKEN",
        CHAT,
        HOOK,
        "hookSECRETvalue",
        "webhooks/4242",
    ] {
        assert!(!text.contains(s), "secret leaked in {text:?}");
    }
}

// ------------------------------------------------------------------------------------------
// Thresholds
// ------------------------------------------------------------------------------------------

#[test]
fn warn_fires_once_on_upward_crossing() {
    let mut r = rig();
    assert!(r.feed(reading(NOW, Some(70.0), Some(R1))).is_empty());
    let ev = r.feed(reading(NOW + 60, Some(85.0), Some(R1)));
    assert_eq!(kinds(&ev), [AlertKind::Warn]);
    assert_eq!(alerts(&ev)[0].title, "Claude: Current session at 85%");
    assert_eq!(r.toast.shown().len(), 1);
    assert!(r
        .state
        .alerts_sent
        .contains_key(&format!("acc-1/session/warn/{R1}")));
    assert!(r.feed(reading(NOW + 120, Some(86.0), Some(R1))).is_empty());
    assert!(r.feed(reading(NOW + 180, Some(90.0), Some(R1))).is_empty());
    assert_eq!(r.toast.shown().len(), 1);
}

#[test]
fn first_reading_never_fires_even_high() {
    let mut r = rig();
    assert!(r.feed(reading(NOW, Some(99.0), Some(R1))).is_empty());
    assert!(r.state.alerts_sent.is_empty());
    assert!(r.toast.shown().is_empty());
}

#[test]
fn stale_error_signed_out_and_old_readings_never_fire() {
    let mut r = rig();
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    let mut stale = reading(NOW + 60, Some(90.0), Some(R1));
    stale.status = ReadingStatus::Stale;
    assert!(r.feed(stale).is_empty());
    let mut err = reading(NOW + 120, Some(90.0), Some(R1));
    err.status = ReadingStatus::Error;
    assert!(r.feed(err).is_empty());
    let mut out = reading(NOW + 180, Some(90.0), Some(R1));
    out.status = ReadingStatus::SignedOut;
    assert!(r.feed(out).is_empty());
    // Fresh status but 10 min old at `now`: not live.
    assert!(r
        .feed_at(reading(NOW + 200, Some(90.0), Some(R1)), NOW + 800)
        .is_empty());
    assert!(r.state.alerts_sent.is_empty());
    // A later live reading still compares with the last live value (70): fires.
    assert_eq!(
        kinds(&r.feed(reading(NOW + 900, Some(90.0), Some(R1)))),
        [AlertKind::Warn]
    );
}

#[test]
fn unknown_percent_is_never_zero() {
    let mut r = rig();
    r.feed(reading(NOW, None, Some(R1)));
    assert!(
        r.feed(reading(NOW + 60, Some(85.0), Some(R1))).is_empty(),
        "unknown is not a 0 % baseline"
    );
    r.feed(reading(NOW + 120, None, Some(R1)));
    assert!(
        r.feed(reading(NOW + 180, Some(86.0), Some(R1))).is_empty(),
        "unknown does not erase the 85 % baseline"
    );
}

#[test]
fn restored_previous_reading_is_not_a_baseline() {
    let mut r = rig();
    let mut restored = reading(NOW - 3600, Some(10.0), Some(R1));
    restored.status = ReadingStatus::Stale;
    r.prev = Some(restored);
    assert!(r.feed(reading(NOW, Some(90.0), Some(R1))).is_empty());
    // A fresh but old (> 15 min) previous reading is not trusted either.
    let mut r = rig();
    r.prev = Some(reading(NOW - 3600, Some(10.0), Some(R1)));
    assert!(r.feed(reading(NOW, Some(90.0), Some(R1))).is_empty());
}

#[test]
fn jump_to_critical_fires_critical_only_and_warn_then_critical() {
    let mut r = rig();
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    assert_eq!(
        kinds(&r.feed(reading(NOW + 60, Some(97.0), Some(R1)))),
        [AlertKind::Critical]
    );
    assert!(
        r.state
            .alerts_sent
            .contains_key(&format!("acc-1/session/warn/{R1}")),
        "warn marked too"
    );

    let mut r = rig();
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    assert_eq!(
        kinds(&r.feed(reading(NOW + 60, Some(82.0), Some(R1)))),
        [AlertKind::Warn]
    );
    assert_eq!(
        kinds(&r.feed(reading(NOW + 120, Some(95.0), Some(R1)))),
        [AlertKind::Critical]
    );
    assert!(r.feed(reading(NOW + 180, Some(99.0), Some(R1))).is_empty());
}

#[test]
fn custom_thresholds_are_used() {
    let mut r = rig();
    r.settings.alert_percent = 50;
    r.settings.critical_percent = 70;
    r.feed(reading(NOW, Some(40.0), Some(R1)));
    assert_eq!(
        kinds(&r.feed(reading(NOW + 60, Some(55.0), Some(R1)))),
        [AlertKind::Warn]
    );
    assert_eq!(
        kinds(&r.feed(reading(NOW + 120, Some(71.0), Some(R1)))),
        [AlertKind::Critical]
    );
}

#[test]
fn rearms_after_the_window_resets() {
    let mut r = rig();
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    assert_eq!(
        kinds(&r.feed(reading(NOW + 60, Some(85.0), Some(R1)))).len(),
        1
    );
    // The window renews: next period ends 5 h later.
    let r2 = R1 + 5 * 3600;
    r.feed(reading(R1 + 30, Some(3.0), Some(r2)));
    r.feed(reading(R1 + 90, Some(60.0), Some(r2)));
    let ev = r.feed(reading(R1 + 150, Some(81.0), Some(r2)));
    assert_eq!(kinds(&ev), [AlertKind::Warn]);
    assert!(r
        .state
        .alerts_sent
        .contains_key(&format!("acc-1/session/warn/{r2}")));
}

#[test]
fn rearms_only_after_a_clear_drop() {
    let mut r = rig();
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    assert_eq!(
        kinds(&r.feed(reading(NOW + 60, Some(85.0), Some(R1)))).len(),
        1
    );
    r.feed(reading(NOW + 120, Some(78.0), Some(R1))); // 2 points below: not re-armed
    assert!(r.feed(reading(NOW + 180, Some(85.0), Some(R1))).is_empty());
    r.feed(reading(NOW + 240, Some(74.0), Some(R1))); // 6 points below: re-armed
    assert_eq!(
        kinds(&r.feed(reading(NOW + 300, Some(85.0), Some(R1)))),
        [AlertKind::Warn]
    );
}

#[test]
fn dedup_survives_restart_and_tolerates_reset_jitter() {
    let mut a = rig();
    a.feed(reading(NOW, Some(70.0), Some(R1)));
    assert_eq!(
        kinds(&a.feed(reading(NOW + 60, Some(85.0), Some(R1)))).len(),
        1
    );
    let saved = a.state.clone();

    // New process: state restored, usage dipped then crosses again (same period, reset time
    // reported a few seconds later as Codex does).
    let mut b = rig();
    b.state = saved;
    b.prev = Some(reading(NOW + 600, Some(79.0), Some(R1 + 4)));
    assert!(b
        .feed(reading(NOW + 660, Some(81.0), Some(R1 + 7)))
        .is_empty());
    assert!(b.toast.shown().is_empty());

    // Same state but another period → allowed.
    let mut c = rig();
    c.state = a.state.clone();
    let r2 = R1 + 5 * 3600;
    c.prev = Some(reading(R1 + 600, Some(79.0), Some(r2)));
    assert_eq!(
        kinds(&c.feed(reading(R1 + 660, Some(81.0), Some(r2)))),
        [AlertKind::Warn]
    );
}

#[test]
fn old_dedup_entries_are_pruned() {
    let mut r = rig();
    r.state
        .alerts_sent
        .insert("acc-9/x/warn/1".into(), NOW - ALERTS_SENT_KEEP_SECS - 1);
    r.state
        .alerts_sent
        .insert("acc-9/x/warn/2".into(), NOW - 3600);
    r.feed(reading(NOW, Some(10.0), Some(R1)));
    assert!(!r.state.alerts_sent.contains_key("acc-9/x/warn/1"));
    assert!(r.state.alerts_sent.contains_key("acc-9/x/warn/2"));
}

#[test]
fn master_switch_off_sends_nothing() {
    let mut r = rig();
    r.settings.notifications = false;
    r.enable_telegram();
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    assert!(r.feed(reading(NOW + 60, Some(97.0), Some(R1))).is_empty());
    assert!(r.state.alerts_sent.is_empty());
    assert!(r.http.requests().is_empty());
    assert!(r.toast.shown().is_empty());
}

#[test]
fn grouped_and_count_windows_are_ignored_except_headline() {
    let mut r = rig();
    let mk = |at: i64, used: f64| {
        let mut grouped = LimitWindow::new("spark", "Current session", WindowKind::Session)
            .used(used)
            .resets_at(R1);
        grouped.group = Some("Spark".into());
        let mut count = LimitWindow::new("reqs", "Requests", WindowKind::Other).used(used);
        count.count = Some(3);
        let mut r = reading_with(at, vec![grouped, count]);
        r.provider = ProviderKind::Codex;
        r
    };
    r.feed(mk(NOW, 70.0));
    assert!(r.feed(mk(NOW + 60, 90.0)).is_empty());
    // An explicit grouped headline (Antigravity lane) is watched.
    let mk2 = |at: i64, used: f64| {
        let mut lane = LimitWindow::new("gemini_5h", "5-Hour Limit", WindowKind::Session)
            .used(used)
            .resets_at(R1);
        lane.group = Some("Gemini".into());
        let mut r = reading_with(at, vec![lane]);
        r.provider = ProviderKind::Antigravity;
        r.headline_window = Some("gemini_5h".into());
        r
    };
    let mut r = rig();
    r.feed(mk2(NOW, 70.0));
    assert_eq!(kinds(&r.feed(mk2(NOW + 60, 90.0))), [AlertKind::Warn]);
}

#[test]
fn account_names_are_used() {
    let mut r = rig();
    let mut acc = Account::new(ProviderKind::Claude, "perso");
    acc.id = "acc-1".into();
    r.n.set_accounts(&[acc]);
    r.settings.language = "fr".into();
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    let ev = r.feed(reading(NOW + 60, Some(85.0), Some(R1)));
    assert_eq!(
        alerts(&ev)[0].title,
        "Claude · perso : Session en cours à 85 %"
    );
    assert!(alerts(&ev)[0].body.starts_with(
        "Attention · 85 % utilisés · 15 % restants\nRéinitialisation dans 2 h 59 min"
    ));
}

// ------------------------------------------------------------------------------------------
// Full screen / game mode
// ------------------------------------------------------------------------------------------

#[test]
fn fullscreen_holds_on_screen_but_not_telegram() {
    let mut r = rig_with(FakeHttp::new().on(TG_URL, 200, r#"{"ok":true}"#));
    r.enable_telegram();
    r.desktop.fullscreen = true;
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    let ev = r.feed(reading(NOW + 60, Some(85.0), Some(R1)));
    assert!(ev.is_empty(), "no Event::Alert while full screen");
    assert!(r.toast.shown().is_empty());
    assert_eq!(r.http.requests().len(), 1, "Telegram still sent");
    assert!(r
        .state
        .alerts_sent
        .contains_key(&format!("acc-1/session/warn/{R1}")));
    // Critical passes with criticalInFullscreen (default true).
    let ev = r.feed(reading(NOW + 120, Some(96.0), Some(R1)));
    assert_eq!(kinds(&ev), [AlertKind::Critical]);
    assert_eq!(r.toast.shown().len(), 1);
}

#[test]
fn critical_held_when_critical_in_fullscreen_is_off_and_in_game_mode() {
    let mut r = rig();
    r.settings.critical_in_fullscreen = false;
    r.desktop.game_mode = true;
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    assert!(r.feed(reading(NOW + 60, Some(96.0), Some(R1))).is_empty());
    assert!(r.toast.shown().is_empty());

    let mut r = rig();
    r.settings.game_mode = true; // config switch alone
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    assert!(r.feed(reading(NOW + 60, Some(85.0), Some(R1))).is_empty());
    assert_eq!(
        kinds(&r.feed(reading(NOW + 120, Some(96.0), Some(R1)))),
        [AlertKind::Critical]
    );
}

#[test]
fn toast_failure_still_returns_the_alert_event() {
    let mut r = rig();
    let failing = Arc::new(RecordingToast::failing(
        "Windows notifications are turned off",
    ));
    r.n.set_toast(Some(failing.clone()));
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    assert_eq!(
        kinds(&r.feed(reading(NOW + 60, Some(85.0), Some(R1)))).len(),
        1
    );
    assert_eq!(failing.shown().len(), 1);
    // No backend at all: still an event for the UI.
    let mut r = rig();
    r.n.set_toast(None);
    assert!(!r.n.has_toasts());
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    assert_eq!(
        kinds(&r.feed(reading(NOW + 60, Some(85.0), Some(R1)))).len(),
        1
    );
}

// ------------------------------------------------------------------------------------------
// Forecast
// ------------------------------------------------------------------------------------------

fn rising_history(
    from_min_ago: i64,
    used_now: f64,
    per_min: f64,
    reset: Option<i64>,
) -> Vec<HistoryPoint> {
    (1..=from_min_ago)
        .rev()
        .map(|m| HistoryPoint {
            account_id: "acc-1".into(),
            label: "Current session".into(),
            at: NOW - m * 60,
            used_percent: used_now - per_min * m as f64,
            resets_at: reset,
        })
        .collect()
}

#[test]
fn forecast_fires_within_lead_time_and_before_reset_once() {
    let mut r = rig();
    r.history = rising_history(20, 60.0, 2.0, Some(R1)); // 2 %/min → 40 % left in 20 min
    r.prev = Some(reading(NOW - 60, Some(58.0), Some(R1)));
    let ev = r.feed(reading(NOW, Some(60.0), Some(R1)));
    assert_eq!(kinds(&ev), [AlertKind::Forecast { minutes_left: 20 }]);
    assert_eq!(
        alerts(&ev)[0].title,
        "Claude: Current session will run out in about 20 min"
    );
    assert!(r
        .state
        .alerts_sent
        .contains_key(&format!("acc-1/session/forecast/{R1}")));
    // Once per period.
    assert!(r
        .feed_at(reading(NOW + 30, Some(61.0), Some(R1)), NOW + 30)
        .is_empty());
}

#[test]
fn forecast_respects_lead_time_reset_switch_and_baseline() {
    // 0.5 %/min, 40 % left → 80 min > 30.
    let mut r = rig();
    r.history = rising_history(20, 60.0, 0.5, Some(R1));
    r.prev = Some(reading(NOW - 60, Some(59.5), Some(R1)));
    assert!(r.feed(reading(NOW, Some(60.0), Some(R1))).is_empty());
    // Runs out in 20 min but the window resets in 10 min: nothing.
    let mut r = rig();
    let soon = NOW + 600;
    r.history = rising_history(20, 60.0, 2.0, Some(soon));
    r.prev = Some(reading(NOW - 60, Some(58.0), Some(soon)));
    assert!(r.feed(reading(NOW, Some(60.0), Some(soon))).is_empty());
    // Switch off.
    let mut r = rig();
    r.settings.forecast_alerts = false;
    r.history = rising_history(20, 60.0, 2.0, Some(R1));
    r.prev = Some(reading(NOW - 60, Some(58.0), Some(R1)));
    assert!(r.feed(reading(NOW, Some(60.0), Some(R1))).is_empty());
    // First reading of the run: no forecast either.
    let mut r = rig();
    r.history = rising_history(20, 60.0, 2.0, Some(R1));
    assert!(r.feed(reading(NOW, Some(60.0), Some(R1))).is_empty());
}

#[test]
fn threshold_wins_over_forecast_in_the_same_reading() {
    let mut r = rig();
    r.history = rising_history(20, 82.0, 0.5, Some(R1)); // 18 % left at 0.5/min = 36 min
    r.settings.forecast_minutes = 60;
    r.prev = Some(reading(NOW - 60, Some(79.0), Some(R1)));
    assert_eq!(
        kinds(&r.feed(reading(NOW, Some(82.0), Some(R1)))),
        [AlertKind::Warn]
    );
    // Next reading: forecast may now fire.
    let ev = r.feed_at(reading(NOW + 30, Some(82.2), Some(R1)), NOW + 30);
    assert!(matches!(kinds(&ev)[..], [AlertKind::Forecast { .. }]));
}

#[test]
fn forecast_public_fn_flat_negative_and_reset_spanning() {
    let w = window(Some(60.0), Some(R1));
    assert_eq!(
        forecast_minutes_left(&w, &rising_history(20, 60.0, 2.0, Some(R1)), NOW),
        Some(20)
    );
    assert_eq!(
        forecast_minutes_left(&w, &rising_history(20, 60.0, 0.0, Some(R1)), NOW),
        None
    );
    assert_eq!(
        forecast_minutes_left(&w, &rising_history(20, 60.0, -0.2, Some(R1)), NOW),
        None
    );
    // Old period at 98 % then reset: only the new 2 %/min part counts.
    let mut h = rising_history(40, 98.0, 0.1, Some(NOW - 21 * 60));
    h.retain(|p| p.at < NOW - 20 * 60);
    h.extend(rising_history(19, 60.0, 2.0, Some(R1)));
    assert_eq!(forecast_minutes_left(&w, &h, NOW), Some(20));
}

// ------------------------------------------------------------------------------------------
// Reset detection
// ------------------------------------------------------------------------------------------

#[test]
fn reset_watcher_date_roll() {
    let mut w = ResetWatcher::new();
    let old = NOW + 120;
    let new = NOW + 5 * 3600;
    assert!(
        w.observe(&reading(NOW, Some(40.0), Some(old)), NOW)
            .is_empty(),
        "first sighting is a baseline"
    );
    let ev = w.observe(&reading(NOW + 300, Some(1.0), Some(new)), NOW + 300);
    assert_eq!(
        ev,
        vec![ResetEvent {
            account_id: "acc-1".into(),
            window_id: "session".into(),
            label_key: "Current session".into(),
            used_percent: Some(1.0),
            next_reset_at: Some(new),
        }]
    );
}

#[test]
fn reset_watcher_date_roll_needs_60s_and_passed_reset() {
    let old = NOW + 120;
    // Old reset passed but the new one is only 30 s later: no roll; usage unchanged: no drop.
    let mut w = ResetWatcher::new();
    w.observe(&reading(NOW, Some(40.0), Some(old)), NOW);
    assert!(w
        .observe(&reading(NOW + 130, Some(40.0), Some(old + 30)), NOW + 130)
        .is_empty());
    // Reset time moved but the old one has not passed yet: no roll.
    let mut w = ResetWatcher::new();
    w.observe(&reading(NOW, Some(40.0), Some(R1)), NOW);
    assert!(w
        .observe(&reading(NOW + 60, Some(40.0), Some(R1 + 3600)), NOW + 60)
        .is_empty());
}

#[test]
fn reset_watcher_drop() {
    // Unknown reset time: a 10-point drop is a reset.
    let mut w = ResetWatcher::new();
    w.observe(&reading(NOW, Some(50.0), None), NOW);
    let ev = w.observe(&reading(NOW + 60, Some(39.0), None), NOW + 60);
    assert_eq!(ev.len(), 1);
    // Known reset not passed: a drop alone is not a reset.
    let mut w = ResetWatcher::new();
    w.observe(&reading(NOW, Some(50.0), Some(R1)), NOW);
    assert!(w
        .observe(&reading(NOW + 60, Some(20.0), Some(R1)), NOW + 60)
        .is_empty());
    // 9-point drop: not enough.
    let mut w = ResetWatcher::new();
    w.observe(&reading(NOW, Some(50.0), None), NOW);
    assert!(w
        .observe(&reading(NOW + 60, Some(41.0), None), NOW + 60)
        .is_empty());
}

#[test]
fn reset_watcher_needs_a_10_percent_peak() {
    let mut w = ResetWatcher::new();
    let old = NOW + 120;
    w.observe(&reading(NOW, Some(8.0), Some(old)), NOW);
    assert!(w
        .observe(
            &reading(NOW + 300, Some(0.0), Some(NOW + 5 * 3600)),
            NOW + 300
        )
        .is_empty());
    // The peak counts, not the last value.
    let mut w = ResetWatcher::new();
    w.observe(&reading(NOW, Some(12.0), Some(old)), NOW);
    w.observe(&reading(NOW + 60, Some(9.0), Some(old)), NOW + 60);
    assert_eq!(
        w.observe(
            &reading(NOW + 300, Some(0.0), Some(NOW + 5 * 3600)),
            NOW + 300
        )
        .len(),
        1
    );
}

#[test]
fn reset_watcher_cooldown() {
    let mut w = ResetWatcher::new();
    w.observe(&reading(NOW, Some(50.0), None), NOW);
    assert_eq!(
        w.observe(&reading(NOW + 60, Some(30.0), None), NOW + 60)
            .len(),
        1
    );
    w.observe(&reading(NOW + 120, Some(45.0), None), NOW + 120);
    assert!(
        w.observe(&reading(NOW + 180, Some(20.0), None), NOW + 180)
            .is_empty(),
        "within 15 min"
    );
    w.observe(&reading(NOW + 1000, Some(45.0), None), NOW + 1000);
    assert_eq!(
        w.observe(&reading(NOW + 1100, Some(20.0), None), NOW + 1100)
            .len(),
        1,
        "after 15 min"
    );
}

#[test]
fn reset_watcher_ignores_stale_saved_and_repeated_readings() {
    let old = NOW + 120;
    let new = NOW + 5 * 3600;
    // A stale reading in between resets the watcher: the next fresh one is a baseline.
    let mut w = ResetWatcher::new();
    w.observe(&reading(NOW, Some(40.0), Some(old)), NOW);
    let mut stale = reading(NOW + 200, Some(1.0), Some(new));
    stale.status = ReadingStatus::Stale;
    assert!(w.observe(&stale, NOW + 200).is_empty());
    assert!(w
        .observe(&reading(NOW + 300, Some(1.0), Some(new)), NOW + 300)
        .is_empty());
    // Too old by fetched_at (saved reading with Fresh status), or from the future.
    let mut w = ResetWatcher::new();
    w.observe(&reading(NOW, Some(40.0), Some(old)), NOW);
    assert!(w
        .observe(&reading(NOW + 10, Some(1.0), Some(new)), NOW + 400)
        .is_empty());
    assert!(
        w.observe(&reading(NOW + 500, Some(1.0), Some(new)), NOW + 400)
            .is_empty(),
        "future reading"
    );
    // Same fetched_at again: ignored.
    let mut w = ResetWatcher::new();
    w.observe(&reading(NOW, Some(40.0), Some(old)), NOW);
    assert!(w
        .observe(&reading(NOW, Some(1.0), Some(new)), NOW + 300)
        .is_empty());
}

#[test]
fn reset_watcher_skips_grouped_count_and_forgets_absent_ids() {
    let old = NOW + 120;
    let new = NOW + 5 * 3600;
    let grouped = |used: f64, reset: i64| {
        let mut w = window(Some(used), Some(reset));
        w.group = Some("Spark".into());
        w
    };
    let mut w = ResetWatcher::new();
    w.observe(&reading_with(NOW, vec![grouped(40.0, old)]), NOW);
    assert!(w
        .observe(&reading_with(NOW + 300, vec![grouped(1.0, new)]), NOW + 300)
        .is_empty());
    // Absent in one reading → forgotten → baseline again.
    let mut w = ResetWatcher::new();
    w.observe(&reading(NOW, Some(40.0), Some(old)), NOW);
    w.observe(&reading_with(NOW + 60, vec![]), NOW + 60);
    assert!(w
        .observe(&reading(NOW + 300, Some(1.0), Some(new)), NOW + 300)
        .is_empty());
}

#[test]
fn process_turns_resets_into_card_and_alert() {
    let mut r = rig();
    let old = NOW + 120;
    let new = NOW + 5 * 3600;
    r.feed(reading(NOW, Some(40.0), Some(old)));
    let ev = r.feed(reading(NOW + 300, Some(1.0), Some(new)));
    assert!(ev.iter().any(|e| matches!(e, Event::ResetDetected { window_id, next_reset_at: Some(n), .. } if window_id == "session" && *n == new)));
    assert_eq!(kinds(&ev), [AlertKind::Reset]);
    assert_eq!(alerts(&ev)[0].title, "Claude: Current session renewed");
    assert_eq!(
        alerts(&ev)[0].body,
        "Quota available · 99%\nNext reset in 4 h 55 min"
    );

    // Cards off, alerts off.
    let mut r = rig();
    r.settings.reset_cards = false;
    r.settings.reset_alerts = false;
    r.feed(reading(NOW, Some(40.0), Some(old)));
    assert!(r.feed(reading(NOW + 300, Some(1.0), Some(new))).is_empty());
    assert!(r.toast.shown().is_empty());

    // Full screen: card and toast held.
    let mut r = rig();
    r.desktop.fullscreen = true;
    r.feed(reading(NOW, Some(40.0), Some(old)));
    assert!(r.feed(reading(NOW + 300, Some(1.0), Some(new))).is_empty());
}

// ------------------------------------------------------------------------------------------
// Delivery: Telegram / Discord / toast
// ------------------------------------------------------------------------------------------

fn sample_alert() -> Alert {
    messages::threshold_alert(
        AlertKind::Warn,
        "acc-1",
        "Claude",
        &window(Some(85.0), Some(R1)),
        codenotch_core::Lang::En,
        NOW,
    )
}

#[test]
fn telegram_request_shape() {
    let mut r = rig_with(FakeHttp::new().on(TG_URL, 200, r#"{"ok":true}"#));
    r.enable_telegram();
    let results = r.n.deliver(&sample_alert(), &r.settings);
    assert!(results.iter().all(|(_, res)| res.is_ok()));
    let reqs = r.http.requests();
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    assert_eq!(req.method, codenotch_core::Method::Post);
    assert_eq!(
        req.url,
        format!("https://api.telegram.org/bot{TOKEN}/sendMessage")
    );
    assert_eq!(req.timeout, REQUEST_TIMEOUT);
    assert_eq!(req.header_value("content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
    assert_eq!(body["chat_id"], CHAT);
    assert_eq!(
        body["text"],
        "Claude: Current session at 85%\nWarning · 85% used · 15% left\nResets in 3 h"
    );
    assert_eq!(body["disable_web_page_preview"], true);
    assert_eq!(r.toast.shown().len(), 1, "deliver also shows the toast");
    // NB: core's `HttpRequest` Debug prints the URL path, which holds the bot token: this crate
    // never formats requests (reported to integration as a core change request).
}

#[test]
fn discord_request_shape() {
    let mut r = rig_with(FakeHttp::new().on("https://discord.com/api/webhooks/", 204, ""));
    r.enable_discord();
    let results = r.n.deliver(&sample_alert(), &r.settings);
    assert_eq!(
        results
            .iter()
            .filter(|(t, _)| *t == TestTarget::Discord)
            .count(),
        1
    );
    assert!(results.iter().all(|(_, res)| res.is_ok()));
    let req = &r.http.requests()[0];
    assert_eq!(req.url, HOOK);
    let body: serde_json::Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
    assert_eq!(
        body["content"],
        "**Claude: Current session at 85%**\nWarning · 85% used · 15% left\nResets in 3 h"
    );
    assert_eq!(body["username"], "CodeNotch");
    assert_eq!(body["allowed_mentions"]["parse"], serde_json::json!([]));
}

#[test]
fn disabled_channels_are_not_used() {
    let r = rig_with(FakeHttp::new().on(TG_URL, 200, "{}"));
    r.secrets.set(&SecretKey::TelegramToken, TOKEN).unwrap();
    let results = r.n.deliver(&sample_alert(), &r.settings);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, TestTarget::Toast);
    assert!(r.http.requests().is_empty());
}

#[test]
fn telegram_errors_are_secret_free() {
    // Unauthorized
    let mut r =
        rig_with(FakeHttp::new().on(TG_URL, 401, r#"{"ok":false,"description":"Unauthorized"}"#));
    r.enable_telegram();
    let err =
        r.n.send_test(TestTarget::Telegram, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(err, "Telegram rejected the bot token.");
    // Bad chat: the description echoes the chat id and token → scrubbed.
    let desc = format!(
        r#"{{"ok":false,"description":"Bad Request: chat {CHAT} not found for bot{TOKEN}"}}"#
    );
    let mut r = rig_with(FakeHttp::new().on(TG_URL, 400, &desc));
    r.enable_telegram();
    let err =
        r.n.send_test(TestTarget::Telegram, &r.settings)
            .unwrap_err()
            .to_string();
    assert!(
        err.starts_with("Telegram refused the message: Bad Request: chat"),
        "{err}"
    );
    assert_secret_free(&err);
    // Transport error whose text contains the full URL.
    let mut r = rig_with(FakeHttp::new().on_error(
        TG_URL,
        HttpError::Connect(format!("{TG_URL}{TOKEN}/sendMessage: dns")),
    ));
    r.enable_telegram();
    let err =
        r.n.send_test(TestTarget::Telegram, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(err, "Could not reach Telegram: no connection");
    assert_eq!(
        r.http.requests().len(),
        2,
        "one retry on a transient failure"
    );
    assert_eq!(*r.sleeps.lock().unwrap(), vec![Duration::from_secs(1)]);
    // French.
    r.settings.language = "fr".into();
    let err =
        r.n.send_test(TestTarget::Telegram, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(err, "Impossible de joindre Telegram : pas de connexion");
}

#[test]
fn telegram_missing_or_invalid_config() {
    let mut r = rig();
    let err =
        r.n.send_test(TestTarget::Telegram, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(
        err,
        "Telegram bot token is missing. Add it in Settings or set CODENOTCH_TELEGRAM_TOKEN."
    );
    r.secrets.set(&SecretKey::TelegramToken, TOKEN).unwrap();
    let err =
        r.n.send_test(TestTarget::Telegram, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(err, "Telegram chat id is missing.");
    r.settings.telegram_chat_id = "not a chat".into();
    let err =
        r.n.send_test(TestTarget::Telegram, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(err, "Telegram chat id looks invalid.");
    r.settings.telegram_chat_id = CHAT.into();
    r.secrets
        .set(&SecretKey::TelegramToken, "123:abc/../../x")
        .unwrap();
    let err =
        r.n.send_test(TestTarget::Telegram, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(err, "Telegram bot token looks invalid.");
    assert!(r.http.requests().is_empty());
}

#[test]
fn secrets_are_resolved_at_send_time_env_first() {
    let r = rig_with(FakeHttp::new().on(TG_URL, 200, "{}"));
    let env_token = "555:ENVtokenENVtokenENVtoken_1";
    let env = Env::for_test(std::path::Path::new("Z:/notify-test"))
        .with_var(TELEGRAM_TOKEN_ENV, env_token);
    let mut n = Notifier::new(r.http.clone(), env, r.secrets.clone());
    n.set_delivery(Delivery::Inline);
    r.secrets.set(&SecretKey::TelegramToken, TOKEN).unwrap();
    let settings = Settings {
        telegram_chat_id: CHAT.into(),
        ..Settings::default()
    };
    n.send_test(TestTarget::Telegram, &settings).unwrap();
    assert!(r.http.requests()[0].url.contains(env_token));
    // Store changes are seen without a new Notifier.
    let m = Notifier::new(
        r.http.clone(),
        Env::for_test(std::path::Path::new("Z:/x")),
        r.secrets.clone(),
    );
    m.send_test(TestTarget::Telegram, &settings).unwrap();
    r.secrets
        .set(&SecretKey::TelegramToken, "777:NEWtokenNEWtokenNEWtoken_2")
        .unwrap();
    m.send_test(TestTarget::Telegram, &settings).unwrap();
    let urls: Vec<String> = r.http.requests().iter().map(|q| q.url.clone()).collect();
    assert!(urls[1].contains(TOKEN));
    assert!(urls[2].contains("777:NEWtoken"));
}

#[test]
fn rate_limit_is_honoured_once_then_reported() {
    // Short Retry-After: waited once, then retried (fake answers 429 again → error).
    let resp = HttpResponse::new(429, r#"{"ok":false,"parameters":{"retry_after":3}}"#);
    let mut r = rig_with(FakeHttp::new().on_response(TG_URL, resp));
    r.enable_telegram();
    let err =
        r.n.send_test(TestTarget::Telegram, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(*r.sleeps.lock().unwrap(), vec![Duration::from_secs(3)]);
    assert_eq!(r.http.requests().len(), 2);
    assert_eq!(err, "Telegram is rate limiting: try again in 3 s.");
    // Long Retry-After: not waited at all.
    let resp = HttpResponse::new(429, "").with_header("Retry-After", "120");
    let mut r = rig_with(FakeHttp::new().on_response("https://discord.com/", resp));
    r.enable_discord();
    let err =
        r.n.send_test(TestTarget::Discord, &r.settings)
            .unwrap_err()
            .to_string();
    assert!(r.sleeps.lock().unwrap().is_empty());
    assert_eq!(r.http.requests().len(), 1);
    assert_eq!(err, "Discord is rate limiting: try again in 120 s.");
}

#[test]
fn server_errors_retry_once_client_errors_do_not() {
    let mut r = rig_with(FakeHttp::new().on("https://discord.com/", 502, "bad gateway"));
    r.enable_discord();
    let err =
        r.n.send_test(TestTarget::Discord, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(r.http.requests().len(), 2);
    assert_eq!(err, "Discord answered with HTTP 502.");
    let mut r = rig_with(FakeHttp::new().on(
        "https://discord.com/",
        404,
        r#"{"message":"Unknown Webhook"}"#,
    ));
    r.enable_discord();
    let err =
        r.n.send_test(TestTarget::Discord, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(r.http.requests().len(), 1);
    assert_eq!(err, "Discord rejected the webhook (deleted or wrong URL).");
    assert_secret_free(&err);
}

#[test]
fn discord_errors_are_secret_free() {
    let body = format!(r#"{{"message":"Invalid payload for {HOOK}"}}"#);
    let mut r = rig_with(FakeHttp::new().on("https://discord.com/", 400, &body));
    r.enable_discord();
    let err =
        r.n.send_test(TestTarget::Discord, &r.settings)
            .unwrap_err()
            .to_string();
    assert!(
        err.starts_with("Discord refused the message: Invalid payload"),
        "{err}"
    );
    assert_secret_free(&err);
    let mut r = rig_with(FakeHttp::new().on_error("https://discord.com/", HttpError::Timeout));
    r.enable_discord();
    let err =
        r.n.send_test(TestTarget::Discord, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(err, "Could not reach Discord: timed out");
    // Missing / not a webhook.
    let r = rig();
    let err =
        r.n.send_test(TestTarget::Discord, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(
        err,
        "Discord webhook URL is missing. Add it in Settings or set CODENOTCH_DISCORD_WEBHOOK."
    );
    r.secrets
        .set(
            &SecretKey::DiscordWebhook,
            "https://evil.example/api/webhooks/1/tok",
        )
        .unwrap();
    let err =
        r.n.send_test(TestTarget::Discord, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(err, "This is not a Discord webhook URL.");
    assert!(!err.contains("evil.example"));
    assert!(r.http.requests().is_empty());
}

#[test]
fn send_test_messages_and_toast() {
    let mut r = rig_with(
        FakeHttp::new()
            .on(TG_URL, 200, "{}")
            .on("https://discord.com/", 204, ""),
    );
    r.enable_telegram();
    r.enable_discord();
    r.n.send_test(TestTarget::Telegram, &r.settings).unwrap();
    r.n.send_test(TestTarget::Discord, &r.settings).unwrap();
    r.n.send_test(TestTarget::Toast, &r.settings).unwrap();
    let reqs = r.http.requests();
    let tg: serde_json::Value = serde_json::from_slice(reqs[0].body.as_ref().unwrap()).unwrap();
    assert_eq!(
        tg["text"],
        "Test message from CodeNotch\nIf you can read this, Telegram alerts work."
    );
    let dc: serde_json::Value = serde_json::from_slice(reqs[1].body.as_ref().unwrap()).unwrap();
    assert_eq!(
        dc["content"],
        "**Test message from CodeNotch**\nIf you can read this, Discord alerts work."
    );
    assert_eq!(
        r.toast.shown(),
        vec![(
            "Test message from CodeNotch".to_string(),
            "If you can read this, Windows alerts work.".to_string()
        )]
    );
    // No backend → clear error.
    r.n.set_toast(None);
    let err =
        r.n.send_test(TestTarget::Toast, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(err, "Windows notifications are not available.");
    // Failing backend → wrapped error.
    r.n.set_toast(Some(Arc::new(RecordingToast::failing("turned off"))));
    r.settings.language = "fr".into();
    let err =
        r.n.send_test(TestTarget::Toast, &r.settings)
            .unwrap_err()
            .to_string();
    assert_eq!(err, "La notification Windows a échoué : turned off");
}

#[test]
fn background_delivery_does_not_block_and_debug_hides_secrets() {
    let mut r = rig_with(FakeHttp::new().on(TG_URL, 200, "{}"));
    r.enable_telegram();
    r.n.set_delivery(Delivery::Background);
    r.feed(reading(NOW, Some(70.0), Some(R1)));
    let ev = r.feed(reading(NOW + 60, Some(85.0), Some(R1)));
    assert_eq!(kinds(&ev).len(), 1);
    // Wait (bounded) for the helper thread.
    for _ in 0..200 {
        if !r.http.requests().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(r.http.requests().len(), 1);
    assert_secret_free(&format!("{:?} {:?}", r.n, r.settings));
}

#[test]
fn notifier_new_has_no_toast_backend() {
    let n = Notifier::new(
        Arc::new(FakeHttp::new()),
        Env::for_test(std::path::Path::new(".")),
        Arc::new(MemorySecrets::new()),
    );
    assert!(!n.has_toasts());
    assert!(n
        .send_test(TestTarget::Toast, &Settings::default())
        .is_err());
}
