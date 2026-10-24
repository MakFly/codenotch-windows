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
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub config: Config,
    pub readings: Vec<Reading>,
    pub refreshing: bool,
    pub storage_error: Option<String>,
}

pub fn validate_account(a: &Account) -> Result<(), String> {
    if a.name.trim().is_empty() || a.name.len() > 80 {
        return Err("Donne un nom de 1 à 80 caractères au compte.".into());
    }
    if !["codex", "claude", "copilot", "ollama", "custom"].contains(&a.provider.as_str()) {
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
        assert_eq!(s.widget_position, "left");
        assert_eq!(s.poll_seconds, 120);
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
