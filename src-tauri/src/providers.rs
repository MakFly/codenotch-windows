use crate::{codex, model::*};
use serde_json::Value;
use std::{path::PathBuf, time::Duration};

pub struct Failure {
    pub message: String,
    pub status: String,
    pub retry_seconds: u64,
}
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            status: "error".into(),
            retry_seconds: 60,
        }
    }
}
fn auth(message: &str) -> Failure {
    Failure {
        message: message.into(),
        status: "needsAuth".into(),
        retry_seconds: 120,
    }
}

pub fn credential(account: &Account) -> Option<String> {
    if account.has_secret {
        return keyring::Entry::new("SmashToken", &account.id)
            .ok()?
            .get_password()
            .ok()
            .filter(|s| !s.is_empty());
    }
    if !account.secret_env.is_empty() {
        return std::env::var(&account.secret_env)
            .ok()
            .filter(|s| !s.is_empty());
    }
    None
}

async fn request(url: &str, token: Option<String>, claude: bool) -> Result<Value, Failure> {
    validate_endpoint(url).map_err(Failure::from)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(18))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Failure::from("Client réseau indisponible.".to_owned()))?;
    let mut req = client
        .get(url)
        .header("Accept", "application/json")
        .header("User-Agent", "SmashToken/0.1");
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    if claude {
        req = req.header("anthropic-beta", "oauth-2025-04-20");
    }
    let response = req.send().await.map_err(|_| {
        Failure::from("Service injoignable. Vérifie le réseau et l’adresse configurée.".to_owned())
    })?;
    match response.status().as_u16() {
        401 | 403 => {
            return Err(auth(
                "Accès refusé. Reconnecte le compte ou vérifie le secret configuré.",
            ))
        }
        429 => {
            let retry = response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(120)
                .clamp(60, 86400);
            return Err(Failure {
                message: "Trop de requêtes. Les relevés reprendront après le délai du fournisseur."
                    .into(),
                status: "rateLimited".into(),
                retry_seconds: retry,
            });
        }
        200..=299 => {}
        status => {
            return Err(Failure::from(format!(
                "Le service a répondu HTTP {status}. Vérifie le connecteur."
            )))
        }
    }
    // Bound response size even for chunked custom endpoints.
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Failure::from("Réponse interrompue.".to_owned()))?
    {
        if body.len() + chunk.len() > 2_000_000 {
            return Err(Failure::from(
                "Réponse JSON trop volumineuse (maximum 2 Mo).".to_owned(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|_| Failure::from("Le service n’a pas renvoyé du JSON valide.".to_owned()))
}

pub async fn fetch(account: &Account) -> Result<Reading, Failure> {
    match account.provider.as_str() {
        "codex" => codex::fetch(account).await.map_err(Failure::from),
        "claude" => {
            let token = if let Some(t) = credential(account) {
                t
            } else {
                let base = if account.profile_path.is_empty() {
                    dirs::home_dir().unwrap_or_default().join(".claude")
                } else {
                    PathBuf::from(&account.profile_path)
                };
                crate::claude::token(&base).await?
            };
            let value = request(
                "https://api.anthropic.com/api/oauth/usage",
                Some(token),
                true,
            )
            .await?;
            parse_claude(&value).map_err(Failure::from)
        }
        "copilot" => {
            let token = credential(account)
                .or_else(|| std::env::var("GH_TOKEN").ok())
                .or_else(|| std::env::var("GITHUB_TOKEN").ok())
                .ok_or_else(|| {
                    auth("Ajoute un jeton GitHub autorisé pour Copilot, ou configure GH_TOKEN.")
                })?;
            let value = request(
                "https://api.github.com/copilot_internal/user",
                Some(token),
                false,
            )
            .await?;
            parse_copilot(&value).map_err(Failure::from)
        }
        "ollama" => {
            let value = request(&account.endpoint, credential(account), false).await?;
            let models = value["models"].as_array().ok_or_else(|| {
                Failure::from("Réponse Ollama inattendue. Utilise l’URL /api/ps.".to_owned())
            })?;
            Ok(Reading {
                source: "Ollama · modèles en mémoire".into(),
                models: models
                    .iter()
                    .filter_map(|m| m["name"].as_str().map(str::to_owned))
                    .collect(),
                message: "Le serveur local ne publie pas de quota ni de total de tokens.".into(),
                ..Default::default()
            })
        }
        "custom" => parse_custom(&request(&account.endpoint, credential(account), false).await?)
            .map_err(Failure::from),
        _ => Err(Failure::from("Connecteur inconnu.".to_owned())),
    }
}

pub fn parse_claude(value: &Value) -> Result<Reading, String> {
    let mut reading = Reading {
        source: "Claude · endpoint OAuth interne".into(),
        ..Default::default()
    };
    for (key, label) in [
        ("five_hour", "5 h"),
        ("seven_day", "7 jours"),
        ("seven_day_sonnet", "Sonnet · 7 jours"),
        ("seven_day_opus", "Opus · 7 jours"),
    ] {
        if let Some(used) = number(&value[key]["utilization"]) {
            reading.windows.push(Window {
                label: label.into(),
                used_percent: used,
                resets_at: timestamp(&value[key]["resets_at"]),
            });
        }
    }
    if reading.windows.is_empty() {
        return Err(
            "Aucun quota Claude dans la réponse. Le connecteur peut nécessiter une mise à jour."
                .into(),
        );
    }
    Ok(reading)
}

pub fn parse_copilot(value: &Value) -> Result<Reading, String> {
    let mut reading = Reading {
        source: "GitHub Copilot · endpoint interne".into(),
        plan: value["copilot_plan"].as_str().map(str::to_owned),
        ..Default::default()
    };
    let quotas = value["quota_snapshots"]
        .as_object()
        .ok_or("Format de quotas Copilot inconnu.")?;
    let mut unlimited = false;
    for (key, quota) in quotas {
        if quota["unlimited"] == true {
            unlimited = true;
            continue;
        }
        let entitlement = number(&quota["entitlement"]).filter(|n| *n > 0.0);
        let used = number(&quota["used"])
            .or_else(|| Some((entitlement? - number(&quota["remaining"])?).max(0.0)));
        if let (Some(total), Some(used)) = (entitlement, used) {
            reading.windows.push(Window {
                label: match key.as_str() {
                    "premium_interactions" => "Requêtes premium",
                    "chat" => "Chat",
                    "completions" => "Complétions",
                    _ => key,
                }
                .into(),
                used_percent: used / total * 100.0,
                resets_at: timestamp(&quota["reset_date"])
                    .or_else(|| timestamp(&value["quota_reset_date"])),
            });
        }
    }
    if reading.windows.is_empty() && !unlimited {
        return Err("GitHub n’a fourni aucun quota mesurable.".into());
    }
    if unlimited {
        reading.message = "Certaines ressources sont indiquées comme illimitées par GitHub.".into();
    }
    Ok(reading)
}

pub fn parse_custom(value: &Value) -> Result<Reading, String> {
    let mut reading = Reading {
        source: "Endpoint JSON configuré".into(),
        plan: value["plan"].as_str().map(|s| s.chars().take(80).collect()),
        tokens_today: value["tokensToday"].as_u64(),
        lifetime_tokens: value["lifetimeTokens"].as_u64(),
        ..Default::default()
    };
    if let Some(windows) = value["windows"].as_array() {
        for w in windows.iter().take(20) {
            reading.windows.push(Window {
                label: w["label"]
                    .as_str()
                    .ok_or("Chaque fenêtre doit avoir un label.")?
                    .chars()
                    .take(80)
                    .collect(),
                used_percent: number(&w["usedPercent"])
                    .ok_or("Chaque fenêtre doit avoir un usedPercent positif ou nul.")?,
                resets_at: timestamp(&w["resetsAt"]),
            });
        }
    }
    if let Some(daily) = value["daily"].as_array() {
        for d in daily.iter().take(366) {
            if let (Some(date), Some(tokens)) = (d["date"].as_str(), d["tokens"].as_u64()) {
                if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok() {
                    reading.daily.push(Daily {
                        date: date.into(),
                        tokens,
                    });
                }
            }
        }
        reading.daily.sort_by(|a, b| a.date.cmp(&b.date));
    }
    if reading.windows.is_empty()
        && reading.tokens_today.is_none()
        && reading.lifetime_tokens.is_none()
        && reading.daily.is_empty()
    {
        return Err("JSON sans mesure reconnue. Consulte le format dans l’aide.".into());
    }
    Ok(reading)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn claude_partial_and_zero() {
        let r = parse_claude(&json!({"five_hour":{"utilization":0},"seven_day":null})).unwrap();
        assert_eq!(r.windows.len(), 1);
        assert_eq!(r.windows[0].used_percent, 0.0);
    }
    #[test]
    fn copilot_missing_remaining_is_not_empty_usage() {
        assert!(parse_copilot(&json!({"quota_snapshots":{"chat":{"entitlement":100}}})).is_err());
    }
    #[test]
    fn copilot_consumption() {
        let r = parse_copilot(
            &json!({"quota_snapshots":{"premium_interactions":{"entitlement":300,"remaining":75}}}),
        )
        .unwrap();
        assert_eq!(r.windows[0].used_percent, 75.0);
    }
    #[test]
    fn custom_rejects_fake_success() {
        assert!(parse_custom(&json!({"hello":"world"})).is_err());
        assert!(parse_custom(&json!({"windows":[{"label":"test","usedPercent":-2}]})).is_err());
        assert_eq!(
            parse_custom(&json!({"tokensToday":0}))
                .unwrap()
                .tokens_today,
            Some(0)
        );
    }
}
