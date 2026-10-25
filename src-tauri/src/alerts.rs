use crate::model::{Account, Reading, Settings};
use std::collections::{HashMap, HashSet};

/// Readings older than this no longer describe the current pace.
const PACE_WINDOW: i64 = 3600;
/// A forecast needs this many readings, spread over at least `MIN_SPAN` seconds.
const MIN_SAMPLES: usize = 3;
const MIN_SPAN: i64 = 600;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Level {
    Recovered,
    Forecast,
    Warning,
    Critical,
    Reached,
}

#[derive(Clone, Debug)]
pub struct Alert {
    pub account: String,
    pub provider: String,
    pub label: String,
    pub level: Level,
    pub used: f64,
    pub threshold: u8,
    pub resets_at: Option<i64>,
    pub exhausts_at: Option<i64>,
}

impl Alert {
    /// Alerts important enough to interrupt a full-screen app when the user allows it.
    pub fn urgent(&self) -> bool {
        matches!(
            self.level,
            Level::Forecast | Level::Critical | Level::Reached
        )
    }
}

type Key = (String, String);

/// Pace history and the alerts already sent, kept in memory for the session.
#[derive(Default)]
pub struct Tracker {
    samples: HashMap<Key, (Option<i64>, Vec<(i64, f64)>)>,
    sent: HashSet<(String, String, Option<i64>, Level)>,
    above: HashSet<Key>,
}

/// Time at which consumption reaches 100 % if the recent pace continues (least squares).
pub fn forecast(samples: &[(i64, f64)], now: i64) -> Option<i64> {
    let recent: Vec<(i64, f64)> = samples
        .iter()
        .copied()
        .filter(|(t, _)| now - t <= PACE_WINDOW)
        .collect();
    let (first, last) = (*recent.first()?, *recent.last()?);
    if recent.len() < MIN_SAMPLES || last.0 - first.0 < MIN_SPAN || last.1 >= 100.0 {
        return None;
    }
    let n = recent.len() as f64;
    let mean_t = recent
        .iter()
        .map(|(t, _)| (t - first.0) as f64)
        .sum::<f64>()
        / n;
    let mean_u = recent.iter().map(|(_, u)| u).sum::<f64>() / n;
    let (mut cov, mut var) = (0.0, 0.0);
    for (t, u) in &recent {
        let dt = (t - first.0) as f64 - mean_t;
        cov += dt * (u - mean_u);
        var += dt * dt;
    }
    let slope = cov / var; // percentage points per second
    (var > 0.0 && slope > 0.0).then(|| last.0 + ((100.0 - last.1) / slope).ceil() as i64)
}

impl Tracker {
    /// Records a successful reading, stores its forecasts in it, and returns the alerts
    /// it triggers. Each level fires once per window and period; a higher level silences
    /// the lower ones it skipped.
    pub fn observe(
        &mut self,
        account: &Account,
        reading: &mut Reading,
        settings: &Settings,
        now: i64,
    ) -> Vec<Alert> {
        let mut alerts = vec![];
        self.sent
            .retain(|k| k.2.is_none_or(|reset| reset > now - 86_400));
        for w in &reading.windows {
            let key = (account.id.clone(), w.label.clone());
            let (period, samples) = self
                .samples
                .entry(key.clone())
                .or_insert_with(|| (w.resets_at, vec![]));
            if *period != w.resets_at {
                *period = w.resets_at;
                samples.clear();
            }
            samples.push((now, w.used_percent));
            samples.retain(|(t, _)| now - t <= PACE_WINDOW * 2);
            let exhausts =
                forecast(samples, now).filter(|at| w.resets_at.is_none_or(|reset| *at < reset));
            if let Some(at) = exhausts {
                reading.exhausts_at.insert(w.label.clone(), at);
            }
            let alert = |level, threshold| Alert {
                account: account.name.clone(),
                provider: account.provider.clone(),
                label: w.label.clone(),
                level,
                used: w.used_percent,
                threshold,
                resets_at: w.resets_at,
                exhausts_at: exhausts,
            };
            let level = if w.used_percent >= 100.0 {
                Some((Level::Reached, 100))
            } else if w.used_percent >= f64::from(settings.critical_percent) {
                Some((Level::Critical, settings.critical_percent))
            } else if w.used_percent >= f64::from(settings.alert_percent) {
                Some((Level::Warning, settings.alert_percent))
            } else {
                None
            };
            match level {
                Some((level, threshold)) => {
                    self.above.insert(key.clone());
                    let id = (key.0.clone(), key.1.clone(), w.resets_at, level);
                    if !self.sent.contains(&id) {
                        // The forecast stays useful after a warning, not once critical.
                        for lower in [
                            Level::Warning,
                            Level::Critical,
                            Level::Reached,
                            Level::Forecast,
                        ] {
                            let skipped = if lower == Level::Forecast {
                                level >= Level::Critical
                            } else {
                                lower <= level
                            };
                            if skipped {
                                self.sent.insert((
                                    key.0.clone(),
                                    key.1.clone(),
                                    w.resets_at,
                                    lower,
                                ));
                            }
                        }
                        alerts.push(alert(level, threshold));
                    }
                }
                None => {
                    if self.above.remove(&key) {
                        self.sent.retain(|k| (&k.0, &k.1) != (&key.0, &key.1));
                        if settings.reset_alerts {
                            alerts.push(alert(Level::Recovered, settings.alert_percent));
                        }
                    }
                }
            }
            let horizon = i64::from(settings.forecast_minutes) * 60;
            let forecast_id = (key.0.clone(), key.1, w.resets_at, Level::Forecast);
            if settings.forecast_alerts
                && exhausts.is_some_and(|at| at - now <= horizon)
                && !self.sent.contains(&forecast_id)
            {
                self.sent.insert(forecast_id);
                alerts.push(alert(Level::Forecast, settings.alert_percent));
            }
        }
        alerts
    }

    /// True when a window is past the warning level or forecast to run out within the hour,
    /// so polling should use its fastest allowed pace.
    pub fn near_limit(reading: &Reading, settings: &Settings, now: i64) -> bool {
        reading
            .windows
            .iter()
            .any(|w| w.used_percent >= f64::from(settings.alert_percent))
            || reading.exhausts_at.values().any(|at| at - now <= 3600)
    }
}

/// "25 min", "1 h 40", "2 j" — compact and language-aware.
pub fn duration(seconds: i64, french: bool) -> String {
    let minutes = (seconds.max(60) + 59) / 60;
    if minutes < 60 {
        format!("{minutes} min")
    } else if minutes < 48 * 60 {
        match minutes % 60 {
            0 => format!("{} h", minutes / 60),
            m => format!("{} h {m:02}", minutes / 60),
        }
    } else {
        format!("{} {}", minutes / 1440, if french { "j" } else { "d" })
    }
}

/// Window labels come from the connectors in French.
pub fn label(label: &str, french: bool) -> String {
    if french {
        return label.to_owned();
    }
    label
        .replace("jours", "days")
        .replace("Mensuel", "Monthly")
        .replace("Abonnement", "Subscription")
        .replace("Requêtes premium", "Premium requests")
        .replace("Complétions", "Completions")
}

/// Title and body for the Windows notification.
pub fn desktop_text(a: &Alert, french: bool, now: i64) -> (String, String) {
    let window = label(&a.label, french);
    let used = a.used.round();
    let eta = a.exhausts_at.map(|at| duration(at - now, french));
    let reset = a.resets_at.map(|at| duration(at - now, french));
    match (a.level, french) {
        (Level::Forecast, true) => (
            format!("{} · plafond dans ~{}", a.account, eta.unwrap_or_default()),
            format!(
                "{window} : {used} % utilisés au rythme actuel{}.",
                reset
                    .map(|r| format!(", réinitialisation dans {r}"))
                    .unwrap_or_default()
            ),
        ),
        (Level::Forecast, false) => (
            format!("{} · limit in ~{}", a.account, eta.unwrap_or_default()),
            format!(
                "{window}: {used}% used at the current pace{}.",
                reset
                    .map(|r| format!(", resets in {r}"))
                    .unwrap_or_default()
            ),
        ),
        (Level::Reached, true) => (
            format!("{} · plafond atteint", a.account),
            format!(
                "{window} : 100 % utilisés.{}",
                reset
                    .map(|r| format!(" Réinitialisation dans {r}."))
                    .unwrap_or_default()
            ),
        ),
        (Level::Reached, false) => (
            format!("{} · limit reached", a.account),
            format!(
                "{window}: 100% used.{}",
                reset
                    .map(|r| format!(" Resets in {r}."))
                    .unwrap_or_default()
            ),
        ),
        (Level::Recovered, true) => (
            format!("{} · quota rétabli", a.account),
            format!("{window} : {used} % utilisés. Les agents peuvent reprendre."),
        ),
        (Level::Recovered, false) => (
            format!("{} · quota available again", a.account),
            format!("{window}: {used}% used. Agents can resume."),
        ),
        (_, true) => (
            format!("{} · quota à {used} %", a.account),
            format!(
                "{window} : seuil de {} % atteint.{}",
                a.threshold,
                reset
                    .map(|r| format!(" Réinitialisation dans {r}."))
                    .unwrap_or_default()
            ),
        ),
        (_, false) => (
            format!("{} · quota at {used}%", a.account),
            format!(
                "{window}: {}% threshold reached.{}",
                a.threshold,
                reset
                    .map(|r| format!(" Resets in {r}."))
                    .unwrap_or_default()
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Window;

    fn account() -> Account {
        Account {
            id: "a".into(),
            name: "Claude".into(),
            provider: "claude".into(),
            enabled: true,
            profile_path: String::new(),
            endpoint: String::new(),
            secret_env: String::new(),
            has_secret: false,
        }
    }
    fn reading(used: f64, resets_at: Option<i64>) -> Reading {
        Reading {
            windows: vec![Window {
                label: "5 h".into(),
                used_percent: used,
                resets_at,
            }],
            ..Default::default()
        }
    }
    fn levels(alerts: &[Alert]) -> Vec<Level> {
        alerts.iter().map(|a| a.level).collect()
    }

    #[test]
    fn steady_pace_forecasts_the_limit() {
        // 1 point per minute from 70 %: 100 % is 20 minutes after the last reading at 80 %.
        let samples: Vec<_> = (0..=10).map(|m| (m * 60, 70.0 + m as f64)).collect();
        assert_eq!(forecast(&samples, 600), Some(600 + 20 * 60));
    }

    #[test]
    fn too_little_or_flat_history_has_no_forecast() {
        assert_eq!(forecast(&[(0, 10.0), (700, 20.0)], 700), None);
        assert_eq!(forecast(&[(0, 10.0), (100, 11.0), (200, 12.0)], 200), None);
        assert_eq!(forecast(&[(0, 50.0), (400, 50.0), (800, 50.0)], 800), None);
        assert_eq!(forecast(&[(0, 60.0), (400, 50.0), (800, 40.0)], 800), None);
    }

    #[test]
    fn levels_fire_once_and_skip_lower_ones() {
        let (mut t, s) = (Tracker::default(), Settings::default());
        let reset = Some(100_000);
        assert!(t
            .observe(&account(), &mut reading(50.0, reset), &s, 0)
            .is_empty());
        let a = t.observe(&account(), &mut reading(85.0, reset), &s, 60);
        assert_eq!(levels(&a), [Level::Warning]);
        assert!(t
            .observe(&account(), &mut reading(86.0, reset), &s, 120)
            .is_empty());
        // Straight to the limit: one alert, not critical plus limit.
        let a = t.observe(&account(), &mut reading(100.0, reset), &s, 180);
        assert_eq!(levels(&a), [Level::Reached]);
        assert!(t
            .observe(&account(), &mut reading(100.0, reset), &s, 240)
            .is_empty());
    }

    #[test]
    fn startup_above_threshold_alerts_once() {
        let (mut t, s) = (Tracker::default(), Settings::default());
        let a = t.observe(&account(), &mut reading(97.0, None), &s, 0);
        assert_eq!(levels(&a), [Level::Critical]);
    }

    #[test]
    fn new_period_recovers_and_rearms() {
        let (mut t, s) = (Tracker::default(), Settings::default());
        t.observe(&account(), &mut reading(100.0, Some(1_000)), &s, 0);
        let a = t.observe(&account(), &mut reading(3.0, Some(20_000)), &s, 1_100);
        assert_eq!(levels(&a), [Level::Recovered]);
        let a = t.observe(&account(), &mut reading(82.0, Some(20_000)), &s, 1_200);
        assert_eq!(levels(&a), [Level::Warning]);
    }

    #[test]
    fn forecast_alert_respects_horizon_and_reset() {
        let (mut t, s) = (Tracker::default(), Settings::default());
        let far_reset = Some(100_000);
        let mut alerts = vec![];
        for m in 0..=10 {
            let mut r = reading(40.0 + 4.0 * m as f64, far_reset);
            alerts.extend(t.observe(&account(), &mut r, &s, m * 60));
            if m == 10 {
                // 80 % at 4 points/min: the limit is 5 minutes away.
                assert!(r.exhausts_at.contains_key("5 h"));
            }
        }
        assert!(levels(&alerts).contains(&Level::Forecast));

        // Same pace, but the quota resets before it runs out: no forecast.
        let mut t = Tracker::default();
        let mut alerts = vec![];
        for m in 0..=10 {
            let mut r = reading(10.0 + m as f64, Some(700));
            alerts.extend(t.observe(&account(), &mut r, &s, m * 60));
            assert!(r.exhausts_at.is_empty());
        }
        assert!(!levels(&alerts).contains(&Level::Forecast));
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(25 * 60, true), "25 min");
        assert_eq!(duration(100 * 60, true), "1 h 40");
        assert_eq!(duration(3 * 86_400, false), "3 d");
    }
}
