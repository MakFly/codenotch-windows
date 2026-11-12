//! `history.jsonl`: one usage sample per line, format frozen since 0.2:
//! `{"a":"<account id>","l":"<window label>","t":<unix secs>,"u":<percent used>,"r":<resets_at|null>}`
//!
//! New lines write the canonical English label (`LimitWindow::label_key`). Old files contain
//! localised labels ("5 h", "7 jours"); [`canonical_label`] maps them so series line up.
//! The reader is tolerant: blank or malformed lines are skipped.

use crate::model::Reading;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// One sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryPoint {
    #[serde(rename = "a")]
    pub account_id: String,
    #[serde(rename = "l")]
    pub label: String,
    #[serde(rename = "t")]
    pub at: i64,
    #[serde(rename = "u")]
    pub used_percent: f64,
    #[serde(rename = "r", default)]
    pub resets_at: Option<i64>,
}

impl HistoryPoint {
    /// Samples for every window of `reading` that has a known percent (count-only windows and
    /// unknown values are skipped: nothing is recorded as 0). Signed-out/empty readings give none.
    pub fn from_reading(reading: &Reading) -> Vec<HistoryPoint> {
        if !reading.shows_figures() {
            return Vec::new();
        }
        reading
            .windows
            .iter()
            .filter(|w| w.count.is_none())
            .filter_map(|w| {
                Some(HistoryPoint {
                    account_id: reading.account_id.clone(),
                    label: w.label_key.clone(),
                    at: reading.fetched_at,
                    used_percent: w.used_percent?,
                    resets_at: w.resets_at,
                })
            })
            .collect()
    }

    /// Label mapped to its canonical English key.
    pub fn canonical_label(&self) -> &str {
        canonical_label(&self.label)
    }
}

/// Map labels written by older versions to the canonical English keys.
pub fn canonical_label(label: &str) -> &str {
    match label.trim() {
        "5 h" | "5h" | "5\u{a0}h" | "5-hour" | "5-Hour Limit" | "Limite 5 h"
        | "Session en cours" => "Current session",
        "7 d" | "7d" | "7 jours" | "7 days" | "Limite hebdomadaire" | "Weekly" => "Weekly limit",
        "7 d Opus" | "7 jours Opus" => "Weekly limit (Opus)",
        "7 d Sonnet" | "7 jours Sonnet" => "Weekly limit (Sonnet)",
        "Limite mensuelle" => "Monthly limit",
        other => other,
    }
}

/// Samples of a window still inside its period (`resets_at` in the future) are kept this long
/// at most, past the normal retention, so weekly/monthly forecasts see their whole period.
pub const PERIOD_KEEP_SECS: i64 = crate::forecast::MAX_PERIOD_SECS;
/// Samples kept only for their period are thinned to one per this many seconds per series.
pub const PERIOD_THIN_SECS: i64 = 10 * 60;

/// Retention rule: inside the last `keep_secs`, or (up to [`PERIOD_KEEP_SECS`] old) for a
/// window whose period has not ended yet.
pub fn keep_point(p: &HistoryPoint, now: i64, keep_secs: i64) -> bool {
    p.at >= now - keep_secs
        || (p.resets_at.is_some_and(|r| r > now) && p.at >= now - PERIOD_KEEP_SECS)
}

/// Apply [`keep_point`], then thin the samples older than `keep_secs` to one per
/// [`PERIOD_THIN_SECS`] per (account, canonical label). Sorts by time.
pub fn retain_for_forecast(points: &mut Vec<HistoryPoint>, now: i64, keep_secs: i64) {
    points.retain(|p| keep_point(p, now, keep_secs));
    points.sort_by_key(|p| p.at);
    let dense = now - keep_secs;
    let mut last: std::collections::HashMap<(String, String), i64> = Default::default();
    points.retain(|p| {
        let key = (p.account_id.clone(), p.canonical_label().to_string());
        if p.at >= dense {
            return true;
        }
        match last.get(&key) {
            Some(prev) if p.at - prev < PERIOD_THIN_SECS => false,
            _ => {
                last.insert(key, p.at);
                true
            }
        }
    });
}

/// Handle on one `history.jsonl` file.
#[derive(Debug, Clone)]
pub struct History {
    path: PathBuf,
}

impl History {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append samples (creates the file and folder).
    pub fn append(&self, points: &[HistoryPoint]) -> crate::Result<()> {
        if points.is_empty() {
            return Ok(());
        }
        let ctx = || self.path.display().to_string();
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::Error::io(ctx(), e))?;
        }
        let mut text = String::new();
        for p in points {
            text.push_str(&serde_json::to_string(p)?);
            text.push('\n');
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| crate::Error::io(ctx(), e))?;
        file.write_all(text.as_bytes())
            .map_err(|e| crate::Error::io(ctx(), e))
    }

    /// Every readable sample, file order. Missing file → empty.
    pub fn read_all(&self) -> Vec<HistoryPoint> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        parse_lines(&text)
    }

    /// Samples with `at >= since`.
    pub fn read_since(&self, since: i64) -> Vec<HistoryPoint> {
        self.read_all()
            .into_iter()
            .filter(|p| p.at >= since)
            .collect()
    }

    /// Samples of one account + canonical label, oldest first, `at >= since`.
    pub fn series(&self, account_id: &str, label_key: &str, since: i64) -> Vec<HistoryPoint> {
        let mut v: Vec<_> = self
            .read_since(since)
            .into_iter()
            .filter(|p| p.account_id == account_id && p.canonical_label() == label_key)
            .collect();
        v.sort_by_key(|p| p.at);
        v
    }

    /// Drop samples older than `keep_hours`, except those of a window whose period has not
    /// ended (`resets_at` in the future, up to [`PERIOD_KEEP_SECS`] old, thinned to one per
    /// [`PERIOD_THIN_SECS`]) so weekly/monthly forecasts keep their period. Rewrites
    /// atomically (time order; malformed lines are dropped too). Returns how many samples were
    /// removed.
    pub fn prune(&self, now: i64, keep_hours: u64) -> crate::Result<usize> {
        if !self.path.exists() {
            return Ok(0);
        }
        let all = self.read_all();
        let mut keep = all.clone();
        retain_for_forecast(&mut keep, now, (keep_hours.max(1) as i64) * 3600);
        let raw_lines = std::fs::read_to_string(&self.path)
            .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(all.len());
        let removed = raw_lines.saturating_sub(keep.len());
        if removed == 0 {
            return Ok(0);
        }
        let mut text = String::new();
        for p in &keep {
            text.push_str(&serde_json::to_string(p)?);
            text.push('\n');
        }
        crate::paths::write_atomic(&self.path, text.as_bytes())
            .map_err(|e| crate::Error::io(self.path.display().to_string(), e))?;
        Ok(removed)
    }
}

/// Parse JSONL text, skipping bad lines.
pub fn parse_lines(text: &str) -> Vec<HistoryPoint> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str::<HistoryPoint>(l).ok())
        .filter(|p| p.used_percent.is_finite())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Account;
    use crate::model::{LimitWindow, ProviderKind, WindowKind};

    #[test]
    fn reads_the_existing_format() {
        let text = "{\"a\":\"acc-1\",\"l\":\"5 h\",\"t\":1762450000,\"u\":12.5,\"r\":1762460000}\n\
                    garbage line\n\
                    \n\
                    {\"a\":\"acc-1\",\"l\":\"7 jours\",\"t\":1762450001,\"u\":40.0,\"r\":null}\n\
                    {\"a\":\"acc-2\",\"l\":\"x\",\"t\":1,\"u\":3}\n";
        let pts = parse_lines(text);
        assert_eq!(pts.len(), 3);
        assert_eq!(pts[0].canonical_label(), "Current session");
        assert_eq!(pts[1].canonical_label(), "Weekly limit");
        assert_eq!(pts[1].resets_at, None);
        assert_eq!(pts[2].resets_at, None);
    }

    #[test]
    fn writes_the_existing_format() {
        let p = HistoryPoint {
            account_id: "a".into(),
            label: "Weekly limit".into(),
            at: 5,
            used_percent: 1.5,
            resets_at: Some(9),
        };
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"a":"a","l":"Weekly limit","t":5,"u":1.5,"r":9}"#
        );
    }

    #[test]
    fn append_read_series_prune() {
        let dir = tempfile::tempdir().unwrap();
        let h = History::new(dir.path().join("history.jsonl"));
        assert!(h.read_all().is_empty());
        let account = Account::new(ProviderKind::Claude, "c");
        let mut reading = Reading::fresh(
            &account,
            10_000,
            vec![
                LimitWindow::new("session", "Current session", WindowKind::Session).used(20.0),
                LimitWindow::new("seven_day", "Weekly limit", WindowKind::Weekly), // unknown: skipped
            ],
        );
        h.append(&HistoryPoint::from_reading(&reading)).unwrap();
        reading.fetched_at = 20_000;
        reading.windows[0].used_percent = Some(30.0);
        h.append(&HistoryPoint::from_reading(&reading)).unwrap();
        // legacy line for the same series
        std::fs::OpenOptions::new()
            .append(true)
            .open(h.path())
            .unwrap()
            .write_all(
                format!(
                    "{{\"a\":\"{}\",\"l\":\"5 h\",\"t\":15000,\"u\":25}}\nbroken\n",
                    account.id
                )
                .as_bytes(),
            )
            .unwrap();
        let s = h.series(&account.id, "Current session", 0);
        assert_eq!(
            s.iter().map(|p| p.used_percent).collect::<Vec<_>>(),
            [20.0, 25.0, 30.0]
        );
        // keep 1 h at now=20_000 → cutoff 16_400 → keeps only the 20_000 sample; broken line dropped
        let removed = h.prune(20_000, 1).unwrap();
        assert_eq!(removed, 3);
        assert_eq!(h.read_all().len(), 1);
        assert_eq!(h.prune(20_000, 1).unwrap(), 0);
    }

    #[test]
    fn prune_keeps_open_periods_thinned() {
        let dir = tempfile::tempdir().unwrap();
        let h = History::new(dir.path().join("history.jsonl"));
        let now = 100 * 86_400;
        let pt = |label: &str, at: i64, reset: Option<i64>| HistoryPoint {
            account_id: "a".into(),
            label: label.into(),
            at,
            used_percent: 1.0,
            resets_at: reset,
        };
        let mut pts = Vec::new();
        // Weekly window resetting in 2 days: one sample a minute over the last 5 days.
        for m in 0..(5 * 1440) {
            pts.push(pt(
                "Weekly limit",
                now - 5 * 86_400 + m * 60,
                Some(now + 2 * 86_400),
            ));
        }
        // Session samples of ended periods 3 days ago, and a weekly period that ended.
        for m in 0..60 {
            pts.push(pt(
                "Current session",
                now - 3 * 86_400 + m * 60,
                Some(now - 3 * 86_400 + 5 * 3600),
            ));
            pts.push(pt(
                "Weekly limit",
                now - 9 * 86_400 + m * 60,
                Some(now - 2 * 86_400),
            ));
        }
        // A monthly period older than the cap.
        pts.push(pt("Monthly limit", now - 40 * 86_400, Some(now + 86_400)));
        h.append(&pts).unwrap();
        h.prune(now, 24).unwrap();
        let left = h.read_all();
        assert!(left.iter().all(|p| p.canonical_label() == "Weekly limit"));
        assert!(left.iter().all(|p| p.resets_at == Some(now + 2 * 86_400)));
        // Last 24 h dense (1440), the 4 days before thinned to one per 10 min (576).
        let dense = left.iter().filter(|p| p.at >= now - 86_400).count();
        assert_eq!(dense, 1440);
        assert_eq!(left.len() - dense, 4 * 144);
        assert!(left.windows(2).all(|w| w[0].at <= w[1].at));
        // Idempotent.
        assert_eq!(h.prune(now, 24).unwrap(), 0);
    }

    #[test]
    fn signed_out_reading_records_nothing() {
        let account = Account::new(ProviderKind::Claude, "c");
        let mut r = Reading::signed_out(&account, 1);
        r.windows
            .push(LimitWindow::new("session", "Current session", WindowKind::Session).used(5.0));
        assert!(HistoryPoint::from_reading(&r).is_empty());
    }
}
