//! Quota readings kept across sessions, one JSON line per window reading in
//! `history.jsonl` next to the configuration. Only successful readings are written, so a
//! gap stays a gap; a line is added when a value moves, when its period changes, or every
//! five minutes otherwise, and lines older than 30 days are dropped at launch.
use crate::model::Reading;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Write, path::PathBuf};

const KEEP: i64 = 30 * 86_400;
const HEARTBEAT: i64 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    #[serde(rename = "t")]
    pub at: i64,
    #[serde(rename = "u")]
    pub used: f64,
    #[serde(rename = "r")]
    pub resets_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct Line {
    #[serde(rename = "a")]
    account: String,
    #[serde(rename = "l")]
    label: String,
    #[serde(flatten)]
    sample: Sample,
}

/// Samples per (account id, window label), oldest first.
#[derive(Default)]
pub struct History {
    path: Option<PathBuf>,
    series: HashMap<(String, String), Vec<Sample>>,
}

pub fn path() -> Option<PathBuf> {
    Some(crate::store::config_path()?.with_file_name("history.jsonl"))
}

impl History {
    /// Reads the saved history, rewriting the file when it holds expired or broken lines.
    pub fn load(path: Option<PathBuf>, now: i64) -> Self {
        let text = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let (mut history, dropped) = Self::parse(&text, now);
        history.path = path;
        if dropped {
            history.rewrite();
        }
        history
    }

    fn parse(text: &str, now: i64) -> (Self, bool) {
        let mut history = Self::default();
        let mut dropped = false;
        for raw in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Line>(raw) {
                Ok(line) if now - line.sample.at <= KEEP => history
                    .series
                    .entry((line.account, line.label))
                    .or_default()
                    .push(line.sample),
                _ => dropped = true,
            }
        }
        for samples in history.series.values_mut() {
            samples.sort_by_key(|s| s.at);
        }
        (history, dropped)
    }

    fn lines(&self) -> String {
        let mut out = String::new();
        for ((account, label), samples) in &self.series {
            for sample in samples {
                let line = Line { account: account.clone(), label: label.clone(), sample: *sample };
                if let Ok(json) = serde_json::to_string(&line) {
                    out.push_str(&json);
                    out.push('\n');
                }
            }
        }
        out
    }

    fn rewrite(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let temporary = path.with_extension("tmp");
        if std::fs::write(&temporary, self.lines()).is_ok() {
            let _ = std::fs::rename(temporary, path);
        }
    }

    /// Adds the windows of a successful reading and appends the new lines to the file.
    pub fn record(&mut self, reading: &Reading, now: i64) {
        let mut added = String::new();
        for w in &reading.windows {
            let samples = self
                .series
                .entry((reading.account_id.clone(), w.label.clone()))
                .or_default();
            let due = samples.last().is_none_or(|last| {
                last.used != w.used_percent || last.resets_at != w.resets_at || now - last.at >= HEARTBEAT
            });
            if !due {
                continue;
            }
            let sample = Sample { at: now, used: w.used_percent, resets_at: w.resets_at };
            samples.push(sample);
            let line = Line { account: reading.account_id.clone(), label: w.label.clone(), sample };
            if let Ok(json) = serde_json::to_string(&line) {
                added.push_str(&json);
                added.push('\n');
            }
        }
        let Some(path) = self.path.as_ref().filter(|_| !added.is_empty()) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(added.as_bytes());
        }
    }

    /// The samples of one window since `since`, oldest first.
    pub fn series(&self, account: &str, label: &str, since: i64) -> &[Sample] {
        let samples = self
            .series
            .get(&(account.to_owned(), label.to_owned()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let start = samples.partition_point(|s| s.at < since);
        &samples[start..]
    }

    /// Every window with samples: (account id, label, samples).
    pub fn windows(&self) -> impl Iterator<Item = (&str, &str, &[Sample])> {
        self.series.iter().map(|((a, l), s)| (a.as_str(), l.as_str(), s.as_slice()))
    }

    /// The whole history as CSV, oldest first; `name` gives each account id its label.
    pub fn csv(&self, name: impl Fn(&str) -> (String, String)) -> String {
        let iso = |t: i64| {
            chrono::DateTime::from_timestamp(t, 0)
                .map(|d| d.to_rfc3339())
                .unwrap_or_default()
        };
        let mut rows: Vec<(i64, String)> = vec![];
        for ((account, label), samples) in &self.series {
            let (account_name, provider) = name(account);
            for s in samples {
                let fields = [
                    iso(s.at),
                    account_name.clone(),
                    provider.clone(),
                    label.clone(),
                    format!("{:.1}", s.used),
                    s.resets_at.map(iso).unwrap_or_default(),
                ];
                rows.push((s.at, fields.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(",")));
            }
        }
        rows.sort_by_key(|(at, _)| *at);
        let mut out = String::from("timestamp,account,provider,window,used_percent,resets_at\r\n");
        for (_, row) in rows {
            out.push_str(&row);
            out.push_str("\r\n");
        }
        out
    }
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Window;

    fn reading(used: f64, resets_at: Option<i64>) -> Reading {
        Reading {
            account_id: "a".into(),
            windows: vec![Window { label: "5 h".into(), used_percent: used, resets_at }],
            ..Default::default()
        }
    }

    #[test]
    fn writes_on_change_or_every_five_minutes() {
        let mut h = History::default();
        h.record(&reading(10., Some(9_999)), 1_000);
        h.record(&reading(10., Some(9_999)), 1_060);
        h.record(&reading(12., Some(9_999)), 1_120);
        h.record(&reading(12., Some(9_999)), 1_120 + HEARTBEAT);
        h.record(&reading(12., Some(19_999)), 1_120 + HEARTBEAT + 10);
        let at: Vec<i64> = h.series("a", "5 h", 0).iter().map(|s| s.at).collect();
        assert_eq!(at, vec![1_000, 1_120, 1_420, 1_430]);
        assert_eq!(h.series("a", "5 h", 1_400).len(), 2);
    }

    #[test]
    fn drops_expired_and_broken_lines() {
        let now = 100 * 86_400;
        let old = format!(r#"{{"a":"a","l":"5 h","t":{},"u":1.0,"r":null}}"#, now - KEEP - 1);
        let fresh = format!(r#"{{"a":"a","l":"5 h","t":{},"u":2.0,"r":null}}"#, now - 60);
        let (h, dropped) = History::parse(&format!("{old}\nnot json\n{fresh}\n"), now);
        assert!(dropped);
        assert_eq!(h.series("a", "5 h", 0), &[Sample { at: now - 60, used: 2.0, resets_at: None }]);
        let (again, dropped) = History::parse(&h.lines(), now);
        assert!(!dropped);
        assert_eq!(again.series("a", "5 h", 0).len(), 1);
    }

    #[test]
    fn csv_quotes_names() {
        let mut h = History::default();
        h.record(&reading(42., None), 0);
        let csv = h.csv(|_| ("Claude, \"perso\"".into(), "claude".into()));
        assert!(csv.ends_with("1970-01-01T00:00:00+00:00,\"Claude, \"\"perso\"\"\",claude,5 h,42.0,\r\n"));
    }
}
