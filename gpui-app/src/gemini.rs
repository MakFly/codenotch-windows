use crate::{
    model::{number, timestamp, Account, Reading, Window},
    providers::{request, Failure},
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const API: &str = "https://cloudcode-pa.googleapis.com/v1internal";

fn needs_auth(message: &str) -> Failure {
    Failure {
        message: message.into(),
        status: "needsAuth".into(),
        retry_seconds: 120,
    }
}

/// Usable access token, or `None` when missing or expiring within a minute.
fn valid(token: Option<&str>, expiry: Option<i64>, now: i64) -> Option<String> {
    token
        .filter(|t| !t.is_empty() && expiry.is_some_and(|at| at > now + 60_000))
        .map(str::to_owned)
}

/// Reads the access token Gemini CLI saved. CodeNotch never refreshes it with Gemini CLI's
/// OAuth client: Google has suspended accounts whose CLI credentials were used by other apps.
/// Gemini CLI renews the session itself the next time it runs.
async fn token(profile: &Path) -> Result<String, Failure> {
    let signed_out =
        || needs_auth("Gemini CLI n’est pas connecté. Utilise « Se connecter » dans Comptes.");
    let bytes = tokio::fs::read(profile.join("oauth_creds.json"))
        .await
        .map_err(|_| signed_out())?;
    let creds: Value = serde_json::from_slice(&bytes).map_err(|_| signed_out())?;
    valid(
        creds["access_token"].as_str(),
        creds["expiry_date"].as_i64(),
        chrono::Utc::now().timestamp_millis(),
    )
    .ok_or_else(|| {
        needs_auth("Session Gemini expirée. Relance Gemini CLI pour la renouveler, puis actualise.")
    })
}

pub async fn fetch(account: &Account) -> Result<Reading, Failure> {
    let profile = if account.profile_path.is_empty() {
        crate::login::default_profile("gemini")
    } else {
        PathBuf::from(&account.profile_path)
    };
    let token = token(&profile).await?;
    let setup = request(
        &format!("{API}:loadCodeAssist"),
        Some(token.clone()),
        false,
        Some(&json!({"metadata":{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}})),
    )
    .await?;
    let project = setup["cloudaicompanionProject"]
        .as_str()
        .or_else(|| setup["cloudaicompanionProject"]["id"].as_str())
        .map(str::to_owned)
        .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT").ok())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Failure::from(
                "Projet Gemini introuvable. Lance Gemini CLI une fois pour terminer sa configuration."
                    .to_owned(),
            )
        })?;
    let quota = request(
        &format!("{API}:retrieveUserQuota"),
        Some(token),
        false,
        Some(&json!({ "project": project })),
    )
    .await?;
    let mut reading = parse(&quota).map_err(Failure::from)?;
    reading.plan = setup["currentTier"]["name"].as_str().map(str::to_owned);
    Ok(reading)
}

pub fn parse(value: &Value) -> Result<Reading, String> {
    let mut reading = Reading {
        source: "Gemini CLI · endpoint Code Assist interne".into(),
        ..Default::default()
    };
    for bucket in value["buckets"].as_array().into_iter().flatten().take(20) {
        let Some(remaining) = number(&bucket["remainingFraction"]) else {
            continue;
        };
        let label: String = bucket["modelId"]
            .as_str()
            .unwrap_or("Gemini")
            .chars()
            .take(80)
            .collect();
        if reading.windows.iter().any(|w| w.label == label) {
            continue;
        }
        reading.windows.push(Window {
            label,
            used_percent: ((1.0 - remaining) * 100.0).clamp(0.0, 100.0),
            resets_at: timestamp(&bucket["resetTime"]),
        });
    }
    if reading.windows.is_empty() {
        return Err(
            "Aucun quota Gemini dans la réponse. Le connecteur peut nécessiter une mise à jour."
                .into(),
        );
    }
    Ok(reading)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remaining_fraction_becomes_consumption() {
        let r = parse(&json!({"buckets":[
            {"modelId":"gemini-2.5-pro","remainingFraction":0.25,"resetTime":"2026-10-26T00:00:00Z"},
            {"modelId":"gemini-2.5-pro","remainingFraction":0.9},
            {"modelId":"gemini-2.5-flash","remainingFraction":1}
        ]}))
        .unwrap();
        assert_eq!(r.windows.len(), 2);
        assert_eq!(r.windows[0].used_percent, 75.0);
        assert!(r.windows[0].resets_at.is_some());
        assert_eq!(r.windows[1].used_percent, 0.0);
    }
    #[test]
    fn missing_quota_is_not_zero() {
        assert!(parse(&json!({"buckets":[{"modelId":"x"}]})).is_err());
        assert!(valid(Some("t"), Some(1_000), 1_000).is_none());
        assert_eq!(valid(Some("t"), Some(70_000), 1_000).as_deref(), Some("t"));
    }
}
