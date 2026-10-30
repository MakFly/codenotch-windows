use crate::{
    model::{number, timestamp, Account, Reading, Window},
    providers::{request, Failure},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::Duration,
};

/// Expiry already handed to Grok CLI for renewal, per profile, so a failure is not retried in a loop.
static RENEWED: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();

/// Accounts Grok CLI stores for xAI's own sign-in; other entries belong to custom deployments.
const ACCOUNTS: [&str; 2] = [
    "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828",
    "https://accounts.x.ai/sign-in",
];

fn needs_auth(message: &str) -> Failure {
    Failure {
        message: message.into(),
        status: "needsAuth".into(),
        retry_seconds: 120,
    }
}

fn signed_out() -> Failure {
    needs_auth("Grok CLI n’est pas connecté. Utilise « Se connecter » dans Comptes.")
}

async fn read_credential(profile: &Path) -> Result<Value, Failure> {
    let bytes = tokio::fs::read(profile.join("auth.json"))
        .await
        .map_err(|_| signed_out())?;
    let accounts: Value = serde_json::from_slice(&bytes).map_err(|_| signed_out())?;
    ACCOUNTS
        .iter()
        .map(|k| accounts[*k].clone())
        .find(Value::is_object)
        .ok_or_else(signed_out)
}

fn expires_soon(credential: &Value, now: i64) -> bool {
    crate::model::timestamp(&credential["expires_at"]).is_some_and(|at| at <= now + 60)
}

/// Grok CLI owns token rotation; `grok models` renews the session without running a prompt.
async fn renew(profile: &Path) {
    let Some(cli) = crate::login::executable("grok") else {
        return;
    };
    let mut cmd = tokio::process::Command::new(cli);
    cmd.arg("models")
        .env("GROK_HOME", profile)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    if let Ok(mut child) = cmd.spawn() {
        if tokio::time::timeout(Duration::from_secs(30), child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
        }
    }
}

/// Reads the session saved by `grok login`, as T3 Code does, and its subscription credits.
pub async fn fetch(account: &Account) -> Result<Reading, Failure> {
    let profile = if account.profile_path.is_empty() {
        crate::login::default_profile("grok")
    } else {
        PathBuf::from(&account.profile_path)
    };
    let mut credential = read_credential(&profile).await?;
    if expires_soon(&credential, chrono::Utc::now().timestamp()) {
        let expiry = credential["expires_at"].as_str().unwrap_or("").to_owned();
        let first_attempt = RENEWED
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map(|mut seen| seen.insert(profile.clone(), expiry.clone()) != Some(expiry))
            .unwrap_or(false);
        if first_attempt {
            renew(&profile).await;
            credential = read_credential(&profile).await?;
        }
        if expires_soon(&credential, chrono::Utc::now().timestamp()) {
            return Err(needs_auth(
                "Session Grok expirée. Utilise « Se connecter » dans Comptes, puis actualise.",
            ));
        }
    }
    if credential["auth_mode"] == "api_key" {
        return Err(Failure::from(
            "Ce profil Grok utilise une clé API : xAI ne publie pas de quota d’abonnement pour elle."
                .to_owned(),
        ));
    }
    let token = credential["key"]
        .as_str()
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(signed_out)?;
    let value = request(
        "https://cli-chat-proxy.grok.com/v1/billing?format=credits",
        Some(token.trim().to_owned()),
        false,
        None,
    )
    .await?;
    parse(&value).map_err(Failure::from)
}

pub fn parse(value: &Value) -> Result<Reading, String> {
    let config = &value["config"];
    let period = &config["currentPeriod"];
    let kind = period["type"]
        .as_str()
        .unwrap_or("")
        .trim_start_matches("USAGE_PERIOD_TYPE_");
    let label = match kind {
        "WEEKLY" => "7 jours",
        "MONTHLY" => "Mensuel",
        _ => "Abonnement",
    };
    let resets_at = timestamp(&period["end"]);
    // Grok omits a zero percentage while still reporting a complete weekly or monthly period.
    let used = number(&config["creditUsagePercent"]).or_else(|| {
        let start = timestamp(&period["start"])?;
        (matches!(kind, "WEEKLY" | "MONTHLY") && resets_at? > start).then_some(0.0)
    });
    let used = used.ok_or(
        "Aucun quota Grok dans la réponse. Le connecteur peut nécessiter une mise à jour.",
    )?;
    Ok(Reading {
        source: "Grok CLI · endpoint de facturation interne".into(),
        windows: vec![Window {
            label: label.into(),
            used_percent: used.min(100.0),
            resets_at,
        }],
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn weekly_credits_and_omitted_zero() {
        let r = parse(&json!({"config":{"creditUsagePercent":42.5,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-11-01T00:00:00Z"}}})).unwrap();
        assert_eq!(r.windows[0].label, "7 jours");
        assert_eq!(r.windows[0].used_percent, 42.5);
        assert!(r.windows[0].resets_at.is_some());
        let zero = parse(&json!({"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_MONTHLY","start":"2026-10-01T00:00:00Z","end":"2026-11-01T00:00:00Z"}}})).unwrap();
        assert_eq!(zero.windows[0].used_percent, 0.0);
    }
    #[test]
    #[ignore = "reads this machine's Grok session and calls xAI"]
    fn print_local_reading() {
        let account = Account {
            id: "local".into(),
            name: "Grok".into(),
            provider: "grok".into(),
            enabled: true,
            profile_path: String::new(),
            endpoint: String::new(),
            secret_env: String::new(),
            has_secret: false,
        };
        match tokio::runtime::Runtime::new().unwrap().block_on(fetch(&account)) {
            Ok(r) => r.windows.iter().for_each(|w| {
                println!(
                    "grok window: {} {:.1}% resets={:?}",
                    w.label, w.used_percent, w.resets_at
                )
            }),
            Err(f) => println!("grok failure: [{}] {}", f.status, f.message),
        }
    }
    #[test]
    fn nanosecond_expiry_is_understood() {
        let credential = json!({"expires_at":"2026-10-25T02:24:25.014189100Z"});
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-25T02:24:25Z")
            .unwrap()
            .timestamp();
        assert!(expires_soon(&credential, at));
        assert!(!expires_soon(&credential, at - 3600));
        assert!(!expires_soon(&json!({}), at));
    }
    #[test]
    fn missing_credits_are_not_zero() {
        assert!(
            parse(&json!({"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY"}}}))
                .is_err()
        );
        assert!(parse(&json!({})).is_err());
    }
}
