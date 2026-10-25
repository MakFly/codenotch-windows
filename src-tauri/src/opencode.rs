use crate::{
    model::{number, timestamp, Account, Reading, Window},
    providers::{request, Failure},
};
use serde_json::Value;
use std::path::PathBuf;

/// Reads OpenCode Go quotas with the key `opencode auth login` saved, as T3 Code does.
/// Other OpenCode connections (Anthropic, OpenAI…) have no OpenCode quota to report.
pub async fn fetch(account: &Account) -> Result<Reading, Failure> {
    let profile = if account.profile_path.is_empty() {
        crate::login::default_profile("opencode")
    } else {
        PathBuf::from(&account.profile_path)
    };
    let auth: Value = tokio::fs::read(profile.join("auth.json"))
        .await
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let go = &auth["opencode-go"];
    // Stored credentials take precedence over the environment, as in OpenCode.
    let key = (go["type"] == "api")
        .then(|| go["key"].as_str().map(str::to_owned))
        .flatten()
        .or_else(|| std::env::var("OPENCODE_API_KEY").ok())
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
        .ok_or_else(|| Failure {
            message: "Aucun abonnement OpenCode Go connecté. Choisis « OpenCode Go » dans « Se connecter »."
                .into(),
            status: "needsAuth".into(),
            retry_seconds: 300,
        })?;
    let value = request(
        "https://opencode.ai/zen/go/v1/usage",
        Some(key),
        false,
        None,
    )
    .await?;
    parse(&value).map_err(Failure::from)
}

pub fn parse(value: &Value) -> Result<Reading, String> {
    let usage = &value["usage"];
    let mut reading = Reading {
        source: "OpenCode Go · endpoint d’usage".into(),
        plan: Some("Go".into()),
        ..Default::default()
    };
    for (key, label) in [
        ("rolling", "5 h"),
        ("weekly", "7 jours"),
        ("monthly", "Mensuel"),
    ] {
        if let Some(used) = number(&usage[key]["percent"]) {
            reading.windows.push(Window {
                label: label.into(),
                used_percent: used.min(100.0),
                resets_at: timestamp(&usage[key]["resetsAt"]),
            });
        }
    }
    if reading.windows.is_empty() {
        return Err(
            "Aucun quota OpenCode Go dans la réponse. Le connecteur peut nécessiter une mise à jour."
                .into(),
        );
    }
    Ok(reading)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn go_windows_keep_partial_readings() {
        let r = parse(&json!({"usage":{"rolling":{"percent":12,"resetsAt":"2026-10-25T18:00:00Z"},"weekly":{"percent":0,"resetsAt":"2026-10-30T00:00:00Z"},"monthly":null}})).unwrap();
        assert_eq!(r.windows.len(), 2);
        assert_eq!(r.windows[0].label, "5 h");
        assert_eq!(r.windows[1].used_percent, 0.0);
        assert!(parse(&json!({"usage":{}})).is_err());
    }
}
