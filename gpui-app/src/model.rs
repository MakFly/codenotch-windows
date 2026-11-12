use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Account row stored in `config.gpui.json`. Field names match the 0.2 file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub enabled: bool,
    #[serde(default)]
    pub profile_path: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub secret_env: String,
    #[serde(default)]
    pub has_secret: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WidgetPlacement {
    #[serde(default)]
    pub monitor: String,
    #[serde(default = "default_x")]
    pub x: f64,
    #[serde(default = "default_y")]
    pub y: f64,
}

impl Default for WidgetPlacement {
    fn default() -> Self {
        Self {
            monitor: String::new(),
            x: default_x(),
            y: default_y(),
        }
    }
}

fn default_x() -> f64 {
    1.0
}
fn default_y() -> f64 {
    0.5
}
fn default_lang() -> String {
    "en".into()
}
fn default_theme() -> String {
    "dark".into()
}
fn default_poll() -> u64 {
    60
}
fn default_true() -> bool {
    true
}
fn default_alert() -> u8 {
    80
}
fn default_edge() -> String {
    "right".into()
}
fn default_critical() -> u8 {
    95
}
fn default_forecast_min() -> u64 {
    30
}
fn default_history() -> u64 {
    24
}
fn default_analytics() -> u64 {
    7
}
fn default_fx() -> f64 {
    0.92
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default = "default_lang")]
    pub language: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_poll")]
    pub poll_seconds: u64,
    #[serde(default = "default_true")]
    pub notifications: bool,
    #[serde(default = "default_alert")]
    pub alert_percent: u8,
    #[serde(default = "default_true")]
    pub widget: bool,
    #[serde(default = "default_edge")]
    pub widget_position: String,
    #[serde(default = "default_true")]
    pub auto_hide: bool,
    #[serde(default = "default_true")]
    pub hide_in_fullscreen: bool,
    #[serde(default)]
    pub game_mode: bool,
    #[serde(default)]
    pub widget_placement: WidgetPlacement,
    /// `None` means the key was absent. Existing accounts then count as already onboarded.
    #[serde(default)]
    pub onboarding_done: Option<bool>,
    #[serde(default = "default_critical")]
    pub critical_percent: u8,
    #[serde(default = "default_true")]
    pub forecast_alerts: bool,
    #[serde(default = "default_forecast_min")]
    pub forecast_minutes: u64,
    #[serde(default = "default_true")]
    pub reset_alerts: bool,
    #[serde(default = "default_true")]
    pub critical_in_fullscreen: bool,
    #[serde(default)]
    pub telegram_enabled: bool,
    #[serde(default)]
    pub telegram_chat_id: String,
    #[serde(default)]
    pub discord_enabled: bool,
    #[serde(default)]
    pub launch_at_startup: bool,
    #[serde(default = "default_true")]
    pub tray_shows_usage: bool,
    #[serde(default)]
    pub hotkey: Option<String>,
    #[serde(default = "default_history")]
    pub history_hours: u64,
    #[serde(default = "default_analytics")]
    pub analytics_days: u64,
    #[serde(default = "default_fx")]
    pub usd_to_eur: f64,
    #[serde(default)]
    pub plan_prices_eur: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: default_lang(),
            theme: default_theme(),
            poll_seconds: default_poll(),
            notifications: true,
            alert_percent: default_alert(),
            widget: true,
            widget_position: default_edge(),
            auto_hide: true,
            hide_in_fullscreen: true,
            game_mode: false,
            widget_placement: WidgetPlacement::default(),
            onboarding_done: None,
            critical_percent: default_critical(),
            forecast_alerts: true,
            forecast_minutes: default_forecast_min(),
            reset_alerts: true,
            critical_in_fullscreen: true,
            telegram_enabled: false,
            telegram_chat_id: String::new(),
            discord_enabled: false,
            launch_at_startup: false,
            tray_shows_usage: true,
            hotkey: None,
            history_hours: default_history(),
            analytics_days: default_analytics(),
            usd_to_eur: default_fx(),
            plan_prices_eur: Map::new(),
        }
    }
}

impl Settings {
    pub fn french(&self) -> bool {
        self.language.eq_ignore_ascii_case("fr")
    }

    pub fn poll_interval(&self) -> u64 {
        match self.poll_seconds {
            30 | 60 | 120 | 300 => self.poll_seconds,
            other if other < 30 => 30,
            other if other > 300 => 300,
            other => other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub settings: Settings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            settings: Settings::default(),
        }
    }
}

impl Config {
    /// Welcome screen only when nothing is connected and onboarding was never finished.
    pub fn needs_welcome(&self) -> bool {
        let done = self
            .settings
            .onboarding_done
            .unwrap_or(!self.accounts.is_empty());
        !done
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dev.kev.smashtoken")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.gpui.json")
}

pub fn parse_config(text: &str) -> Result<Config, String> {
    serde_json::from_str(text).map_err(|err| err.to_string())
}

pub fn load_config() -> (Config, Option<String>) {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(text) => match parse_config(&text) {
            Ok(config) => (config, None),
            Err(err) => (
                Config::default(),
                Some(format!("{}: {err}", path.display())),
            ),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Config::default(), None),
        Err(err) => (
            Config::default(),
            Some(format!("{}: {err}", path.display())),
        ),
    }
}

/// Write the config without touching a previous file when serialization fails.
pub fn save_config(config: &Config) -> Result<(), String> {
    let path = config_path();
    save_config_to(config, &path)
}

pub fn save_config_to(config: &Config, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let text = serde_json::to_string_pretty(config).map_err(|err| err.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|err| err.to_string())?;
    fs::rename(&tmp, path).map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_the_0_2_keys() {
        let raw = r#"{
          "accounts": [{
            "id": "a",
            "name": "Claude · personnel",
            "provider": "claude",
            "enabled": true,
            "profilePath": "C:\\Users\\user\\.claude",
            "endpoint": "",
            "secretEnv": "",
            "hasSecret": false
          }],
          "settings": {
            "language": "fr",
            "theme": "dark",
            "pollSeconds": 60,
            "notifications": true,
            "alertPercent": 80,
            "widget": true,
            "widgetPosition": "right",
            "autoHide": true,
            "hideInFullscreen": true,
            "gameMode": false,
            "widgetPlacement": { "monitor": "\\\\.\\DISPLAY1", "x": 1.0, "y": 0.44 },
            "onboardingDone": true,
            "criticalPercent": 95,
            "forecastAlerts": true,
            "forecastMinutes": 30,
            "resetAlerts": true,
            "criticalInFullscreen": true,
            "telegramEnabled": false,
            "telegramChatId": "",
            "discordEnabled": false,
            "launchAtStartup": false,
            "trayShowsUsage": true,
            "hotkey": null,
            "historyHours": 24,
            "analyticsDays": 7,
            "usdToEur": 0.92,
            "planPricesEur": {}
          }
        }"#;
        let config = parse_config(raw).unwrap();
        assert!(config.settings.french());
        assert!(!config.needs_welcome());
        assert_eq!(config.accounts[0].profile_path, r"C:\Users\user\.claude");
        let again = serde_json::to_value(&config).unwrap();
        let original = serde_json::from_str::<Value>(raw).unwrap();
        assert_eq!(
            again["settings"]["widgetPlacement"]["y"],
            original["settings"]["widgetPlacement"]["y"]
        );
        assert_eq!(again["settings"]["language"], "fr");
        assert!(again["settings"]["hotkey"].is_null());
    }

    #[test]
    fn missing_onboarding_with_accounts_skips_welcome() {
        let config = parse_config(r#"{"accounts":[{"id":"a","name":"C","provider":"claude","enabled":true}]}"#).unwrap();
        assert!(!config.needs_welcome());
    }

    #[test]
    fn empty_config_needs_welcome() {
        assert!(Config::default().needs_welcome());
    }
}
