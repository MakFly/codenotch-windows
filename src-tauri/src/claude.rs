use crate::providers::Failure;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tokio::process::Command;

static RENEWED: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();

fn expires_soon(value: &Value, now_ms: u64) -> bool {
    value["claudeAiOauth"]["expiresAt"]
        .as_u64()
        .is_some_and(|at| at <= now_ms.saturating_add(60_000))
}

fn auth_error() -> Failure {
    Failure { message: "Session Claude expirée. Utilise « Se connecter » dans Comptes ou « claude auth login », puis actualise.".into(), status: "needsAuth".into(), retry_seconds: 120 }
}

async fn read(path: &Path) -> Result<Value, Failure> {
    let bytes = tokio::fs::read(path).await.map_err(|_| auth_error())?;
    serde_json::from_slice(&bytes).map_err(|_| auth_error())
}

pub async fn token(profile: &Path) -> Result<String, Failure> {
    let path = profile.join(".credentials.json");
    let mut value = read(&path).await?;
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    if expires_soon(&value, now) {
        let expiry = value["claudeAiOauth"]["expiresAt"].as_u64().unwrap_or(0);
        let should_renew = {
            let mut attempts = RENEWED
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .map_err(|_| auth_error())?;
            if attempts.get(&path) == Some(&expiry) {
                false
            } else {
                attempts.insert(path.clone(), expiry);
                true
            }
        };
        if should_renew {
            if let Some(cli) = crate::login::executable("claude") {
                // Claude Code owns token rotation. Empty stdin supplies no prompt:
                // it renews authentication and exits without running inference.
                let mut cmd = Command::new(cli);
                cmd.arg("-p")
                    .env("CLAUDE_CONFIG_DIR", profile)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                for (key, _) in std::env::vars_os() {
                    let name = key.to_string_lossy();
                    if name == "CLAUDECODE"
                        || name.starts_with("CLAUDE_CODE_")
                        || matches!(name.as_ref(), "ANTHROPIC_API_KEY" | "ANTHROPIC_AUTH_TOKEN")
                    {
                        cmd.env_remove(key);
                    }
                }
                #[cfg(windows)]
                cmd.creation_flags(0x08000000);
                if let Ok(mut child) = cmd.spawn() {
                    if tokio::time::timeout(Duration::from_secs(20), child.wait())
                        .await
                        .is_err()
                    {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                    }
                }
                value = read(&path).await?;
            }
        }
        if value["claudeAiOauth"]["expiresAt"]
            .as_u64()
            .is_some_and(|at| at <= chrono::Utc::now().timestamp_millis().max(0) as u64)
        {
            return Err(auth_error());
        }
    }
    value["claudeAiOauth"]["accessToken"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(auth_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_and_near_expiry_credentials_are_renewed() {
        assert!(expires_soon(
            &serde_json::json!({"claudeAiOauth":{"expiresAt":900}}),
            1000
        ));
        assert!(expires_soon(
            &serde_json::json!({"claudeAiOauth":{"expiresAt":61000}}),
            1000
        ));
        assert!(!expires_soon(
            &serde_json::json!({"claudeAiOauth":{"expiresAt":61001}}),
            1000
        ));
        assert!(!expires_soon(
            &serde_json::json!({"claudeAiOauth":{}}),
            1000
        ));
    }
}
