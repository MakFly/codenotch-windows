use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

/// One quota window reported by a provider. Percent is consumption, not remaining.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaWindow {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub windows: Vec<QuotaWindow>,
    /// Why the reading is missing or incomplete. Never a substitute for a number.
    pub note: Option<String>,
}

impl Reading {
    pub fn ready(windows: Vec<QuotaWindow>) -> Self {
        Self {
            windows,
            note: None,
        }
    }

    pub fn failed(note: impl Into<String>) -> Self {
        Self {
            windows: Vec::new(),
            note: Some(note.into()),
        }
    }

    pub fn headline(&self) -> Option<f64> {
        self.windows
            .iter()
            .map(|window| window.used_percent)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    }
}

/// Values at most 1 are fractions. Larger values are already percentages.
pub fn as_percent(value: f64) -> f64 {
    let percent = if (0.0..=1.0).contains(&value) {
        value * 100.0
    } else {
        value
    };
    percent.clamp(0.0, 100.0)
}

pub fn parse_claude_usage(body: &str, now: i64) -> Result<Vec<QuotaWindow>, String> {
    let value: Value = serde_json::from_str(body).map_err(|err| err.to_string())?;
    const KEYS: &[(&str, &str)] = &[
        ("five_hour", "5 h"),
        ("fiveHour", "5 h"),
        ("seven_day", "7 d"),
        ("sevenDay", "7 d"),
        ("seven_day_opus", "7 d Opus"),
        ("seven_day_sonnet", "7 d Sonnet"),
    ];
    let mut windows = Vec::new();
    for (key, label) in KEYS {
        if let Some(item) = value.get(key) {
            if let Some(window) = window_from(label, item, now) {
                windows.push(window);
            }
        }
    }
    if windows.is_empty() {
        Err("Claude did not report a quota window".into())
    } else {
        Ok(windows)
    }
}

pub fn parse_codex_usage(body: &str, now: i64) -> Result<Vec<QuotaWindow>, String> {
    let value: Value = serde_json::from_str(body).map_err(|err| err.to_string())?;
    let limit = value.get("rate_limit").unwrap_or(&value);
    let mut windows = Vec::new();
    if let Some(window) = window_from("5 h", limit.get("primary_window").unwrap_or(&Value::Null), now)
    {
        windows.push(window);
    }
    if let Some(window) = window_from(
        "7 d",
        limit.get("secondary_window").unwrap_or(&Value::Null),
        now,
    ) {
        windows.push(window);
    }
    if windows.is_empty() {
        Err("Codex did not report a quota window".into())
    } else {
        Ok(windows)
    }
}

pub fn parse_custom_usage(body: &str, now: i64) -> Result<Vec<QuotaWindow>, String> {
    let value: Value = serde_json::from_str(body).map_err(|err| err.to_string())?;
    let mut windows = Vec::new();
    if let Some(list) = value.get("windows").and_then(|item| item.as_array()) {
        for item in list {
            let label = item
                .get("label")
                .and_then(|label| label.as_str())
                .unwrap_or("quota");
            if let Some(window) = window_from(label, item, now) {
                windows.push(window);
            }
        }
    }
    if windows.is_empty() {
        Err("custom endpoint did not report a quota window".into())
    } else {
        Ok(windows)
    }
}

fn window_from(label: &str, item: &Value, now: i64) -> Option<QuotaWindow> {
    if item.is_null() {
        return None;
    }
    let used = item
        .get("utilization")
        .or_else(|| item.get("usedPercent"))
        .or_else(|| item.get("used_percent"))
        .and_then(|value| value.as_f64())?;
    Some(QuotaWindow {
        label: label.to_string(),
        used_percent: as_percent(used),
        resets_at: resets_at(item, now),
    })
}

fn resets_at(item: &Value, now: i64) -> Option<i64> {
    if let Some(after) = item.get("reset_after_seconds").and_then(|value| value.as_i64()) {
        return Some(now.saturating_add(after));
    }
    let raw = item
        .get("resetsAt")
        .or_else(|| item.get("resets_at"))
        .or_else(|| item.get("reset_at"))?;
    if let Some(secs) = raw.as_i64() {
        return Some(if secs > 10_000_000_000 { secs / 1000 } else { secs });
    }
    let text = raw.as_str()?;
    if let Ok(secs) = text.parse::<i64>() {
        return Some(if secs > 10_000_000_000 { secs / 1000 } else { secs });
    }
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.timestamp())
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub fn format_reset(resets_at: Option<i64>, now: i64, french: bool) -> String {
    let Some(resets_at) = resets_at else {
        return if french { "reset —".into() } else { "reset —".into() };
    };
    let Some(when) = chrono::DateTime::from_timestamp(resets_at, 0) else {
        return "reset —".into();
    };
    let local = when.with_timezone(&chrono::Local);
    let today = chrono::DateTime::from_timestamp(now, 0)
        .map(|time| time.with_timezone(&chrono::Local).date_naive());
    if today == Some(local.date_naive()) {
        format!("reset {}", local.format("%H:%M"))
    } else {
        format!("reset {}", local.format("%d/%m %H:%M"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_and_percent_both_parse() {
        assert_eq!(as_percent(0.42), 42.0);
        assert_eq!(as_percent(1.0), 100.0);
        assert_eq!(as_percent(0.01), 1.0);
        assert_eq!(as_percent(42.0), 42.0);
    }

    #[test]
    fn claude_windows_keep_labels_and_reset() {
        let body = r#"{
            "five_hour": {"utilization": 0.62, "resets_at": "2026-11-12T14:30:00Z"},
            "seven_day": {"utilization": 0.2, "resets_at": 1763000000}
        }"#;
        let windows = parse_claude_usage(body, 0).unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "5 h");
        assert!((windows[0].used_percent - 62.0).abs() < 0.01);
        let expected = chrono::DateTime::parse_from_rfc3339("2026-11-12T14:30:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(windows[0].resets_at, Some(expected));
        assert_eq!(windows[1].used_percent, 20.0);
    }

    #[test]
    fn claude_without_windows_is_an_error() {
        assert!(parse_claude_usage("{}", 0).is_err());
    }

    #[test]
    fn codex_primary_and_secondary() {
        let body = r#"{
            "rate_limit": {
                "primary_window": {"used_percent": 15, "reset_after_seconds": 600},
                "secondary_window": {"used_percent": 40, "reset_at": 1763000000}
            }
        }"#;
        let windows = parse_codex_usage(body, 1_000).unwrap();
        assert_eq!(windows[0].used_percent, 15.0);
        assert_eq!(windows[0].resets_at, Some(1_600));
        assert_eq!(windows[1].label, "7 d");
    }

    #[test]
    fn custom_endpoint_ignores_token_counts_without_a_window() {
        let body = r#"{"tokensToday": 10, "windows": []}"#;
        assert!(parse_custom_usage(body, 0).is_err());
    }

    #[test]
    fn custom_endpoint_reads_documented_windows() {
        let body = r#"{"windows":[{"label":"Monthly quota","usedPercent":42,"resetsAt":"2026-11-01T00:00:00Z"}]}"#;
        let windows = parse_custom_usage(body, 0).unwrap();
        assert_eq!(windows[0].label, "Monthly quota");
        assert_eq!(windows[0].used_percent, 42.0);
    }
}
