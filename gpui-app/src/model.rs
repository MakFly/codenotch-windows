use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub enabled: bool,
    pub profile_path: String,
    pub endpoint: String,
    pub secret_env: String,
    pub has_secret: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default = "default_language")]
    pub language: String,
    pub poll_seconds: u64,
    pub notifications: bool,
    pub alert_percent: u8,
    pub widget: bool,
    pub widget_position: String,
    #[serde(default = "default_auto_hide")]
    pub auto_hide: bool,
    #[serde(default = "default_auto_hide")]
    pub hide_in_fullscreen: bool,
    #[serde(default)]
    pub game_mode: bool,
    #[serde(default)]
    pub widget_placement: Option<WidgetPlacement>,
    /// Whether the welcome screen was completed. `None` in configurations saved before it
    /// existed; startup then decides from whether any account is configured.
    #[serde(default)]
    pub onboarding_done: Option<bool>,
    /// Second, louder level after `alert_percent`.
    #[serde(default = "default_critical")]
    pub critical_percent: u8,
    /// Warn when the recent pace reaches 100 % before the quota resets.
    #[serde(default = "default_true")]
    pub forecast_alerts: bool,
    /// How far ahead a forecast alert looks, in minutes.
    #[serde(default = "default_forecast_minutes")]
    pub forecast_minutes: u16,
    /// Announce when a window drops back under the warning level.
    #[serde(default = "default_true")]
    pub reset_alerts: bool,
    /// Let forecast, critical and limit alerts through in full screen and game mode.
    #[serde(default = "default_true")]
    pub critical_in_fullscreen: bool,
    #[serde(default)]
    pub telegram_enabled: bool,
    /// Telegram chat that receives alerts; the bot token stays in Windows Credential Manager.
    #[serde(default)]
    pub telegram_chat_id: String,
    /// Post alerts to a Discord channel; the webhook URL stays in Windows Credential Manager.
    #[serde(default)]
    pub discord_enabled: bool,
    /// Start with the Windows session (a value under HKCU\…\Run).
    #[serde(default)]
    pub launch_at_startup: bool,
    /// Draw the most used window's gauge on the tray icon.
    #[serde(default = "default_true")]
    pub tray_shows_usage: bool,
    /// System-wide shortcut that shows the notch, e.g. "Ctrl+Alt+N"; `None` when off.
    #[serde(default)]
    pub hotkey: Option<String>,
    /// How far back the Home page history goes, in hours (24, 168 or 720).
    #[serde(default = "default_history_hours")]
    pub history_hours: u16,
    /// Span of the Analytics page, in days (7 or 30).
    #[serde(default = "default_analytics_days")]
    pub analytics_days: u16,
    /// Converts API list prices, in dollars, to the euros the Analytics page shows.
    #[serde(default = "default_usd_to_eur")]
    pub usd_to_eur: f64,
    /// Monthly subscription price per account id, in euros, as the user set it.
    #[serde(default)]
    pub plan_prices_eur: std::collections::BTreeMap<String, f64>,
}
fn default_analytics_days() -> u16 {
    7
}
fn default_usd_to_eur() -> f64 {
    0.92
}
fn default_history_hours() -> u16 {
    168
}
fn default_critical() -> u8 {
    95
}
fn default_true() -> bool {
    true
}
fn default_forecast_minutes() -> u16 {
    30
}
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WidgetPlacement {
    pub monitor: Option<String>,
    pub x: f64,
    pub y: f64,
}
fn default_auto_hide() -> bool {
    true
}
fn default_language() -> String {
    "en".into()
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            language: default_language(),
            poll_seconds: 60,
            notifications: true,
            alert_percent: 80,
            widget: true,
            widget_position: "right".into(),
            auto_hide: true,
            hide_in_fullscreen: true,
            game_mode: false,
            widget_placement: None,
            onboarding_done: Some(false),
            critical_percent: default_critical(),
            forecast_alerts: true,
            forecast_minutes: default_forecast_minutes(),
            reset_alerts: true,
            critical_in_fullscreen: true,
            telegram_enabled: false,
            telegram_chat_id: String::new(),
            discord_enabled: false,
            launch_at_startup: false,
            tray_shows_usage: true,
            hotkey: None,
            history_hours: default_history_hours(),
            analytics_days: default_analytics_days(),
            usd_to_eur: default_usd_to_eur(),
            plan_prices_eur: Default::default(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub accounts: Vec<Account>,
    pub settings: Settings,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Daily {
    pub date: String,
    pub tokens: u64,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub account_id: String,
    pub status: String,
    pub message: String,
    pub source: String,
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    pub tokens_today: Option<u64>,
    pub lifetime_tokens: Option<u64>,
    pub daily: Vec<Daily>,
    pub models: Vec<String>,
    pub updated_at: Option<i64>,
    pub retry_at: Option<i64>,
    /// Estimated time each window reaches 100 % at the recent pace, when before its reset.
    #[serde(default)]
    pub exhausts_at: std::collections::BTreeMap<String, i64>,
    /// Where each window should stand at its reset, from the history; computed after each
    /// successful reading and never saved.
    #[serde(skip)]
    pub outlooks: std::collections::BTreeMap<String, crate::analytics::Outlook>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub config: Config,
    pub readings: Vec<Reading>,
    pub refreshing: bool,
    pub storage_error: Option<String>,
    /// A Telegram bot token is stored in Windows Credential Manager (never sent to the UI).
    pub telegram_token_saved: bool,
    pub discord_webhook_saved: bool,
}

pub fn validate_account(a: &Account) -> Result<(), String> {
    if a.name.trim().is_empty() || a.name.len() > 80 {
        return Err("Donne un nom de 1 à 80 caractères au compte.".into());
    }
    if ![
        "codex", "claude", "gemini", "grok", "opencode", "copilot", "ollama", "custom",
    ]
    .contains(&a.provider.as_str())
    {
        return Err("Connecteur inconnu.".into());
    }
    if a.profile_path.len() > 1024 || a.endpoint.len() > 2048 || a.secret_env.len() > 128 {
        return Err("Configuration trop longue.".into());
    }
    if !a.profile_path.is_empty() && !std::path::Path::new(&a.profile_path).is_absolute() {
        return Err("Le dossier du profil doit être un chemin absolu.".into());
    }
    if !a
        .secret_env
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("Nom de variable d’environnement invalide.".into());
    }
    if ["custom", "ollama"].contains(&a.provider.as_str()) {
        validate_endpoint(&a.endpoint)?;
    }
    Ok(())
}

pub fn validate_endpoint(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|_| "URL invalide.")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("Utilise HTTPS, ou HTTP sur localhost. Aucun identifiant dans l’URL.".into());
    }
    Ok(url)
}

pub fn number(value: &serde_json::Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite() && *n >= 0.0)
}
pub fn timestamp(v: &serde_json::Value) -> Option<i64> {
    v.as_i64().or_else(|| {
        v.as_str().and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|d| d.timestamp())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_settings_enable_auto_hide_without_losing_values() {
        let s: Settings = serde_json::from_value(serde_json::json!({"pollSeconds":120,"notifications":false,"alertPercent":90,"widget":true,"widgetPosition":"left"})).unwrap();
        assert!(s.auto_hide);
        assert_eq!(s.language, "en");
        assert!(s.hide_in_fullscreen);
        assert!(!s.game_mode);
        assert!(s.widget_placement.is_none());
        assert_eq!(s.onboarding_done, None);
        assert_eq!(Settings::default().onboarding_done, Some(false));
        assert_eq!(s.widget_position, "left");
        assert_eq!(s.poll_seconds, 120);
        assert!(!s.launch_at_startup);
        assert!(s.tray_shows_usage);
        assert_eq!(s.hotkey, None);
        assert_eq!(s.history_hours, 168);
        assert_eq!(s.analytics_days, 7);
        assert_eq!(s.usd_to_eur, 0.92);
        assert!(s.plan_prices_eur.is_empty());
    }
    #[test]
    fn endpoints_do_not_leak_credentials() {
        for bad in [
            "http://example.org",
            "https://token@example.org",
            "file:///tmp/a",
            "https://x.com/#secret",
        ] {
            assert!(validate_endpoint(bad).is_err());
        }
        for good in ["https://example.org/usage", "http://127.0.0.1:11434/api/ps"] {
            assert!(validate_endpoint(good).is_ok());
        }
    }
    #[test]
    fn language_choice_survives_saving_with_widget_placement() {
        let settings = Settings {
            language: "fr".into(),
            widget_placement: Some(WidgetPlacement {
                monitor: Some("screen".into()),
                x: 1.0,
                y: 0.45,
            }),
            ..Default::default()
        };
        let restored: Settings =
            serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(restored.language, "fr");
        assert_eq!(restored.widget_placement, settings.widget_placement);
    }
    #[test]
    fn missing_is_not_zero() {
        assert_eq!(number(&serde_json::Value::Null), None);
        assert_eq!(number(&serde_json::json!(0)), Some(0.0));
    }
}
