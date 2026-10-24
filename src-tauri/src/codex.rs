use crate::model::{number, Account, Daily, Reading, Window};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

fn executable() -> PathBuf {
    // Resolve the desktop installation before PATH; never invoke a shell or a user-supplied command.
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let path = PathBuf::from(local).join("Programs/OpenAI/Codex/bin/codex.exe");
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(if cfg!(windows) { "codex.exe" } else { "codex" })
}

pub async fn fetch(account: &Account) -> Result<Reading, String> {
    let mut cmd = Command::new(executable());
    cmd.arg("app-server")
        .arg("--listen")
        .arg("stdio://")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if !account.profile_path.is_empty() {
        cmd.env("CODEX_HOME", &account.profile_path);
    }
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let mut child = cmd.spawn().map_err(|_| {
        "Codex CLI introuvable. Installe Codex et connecte ton compte ChatGPT.".to_string()
    })?;
    let mut stdin = child.stdin.take().ok_or("Canal Codex indisponible.")?;
    let mut lines =
        BufReader::new(child.stdout.take().ok_or("Sortie Codex indisponible.")?).lines();
    let result = tokio::time::timeout(Duration::from_secs(25), async {
        stdin.write_all(format!("{}\n", json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"smashtoken","title":"SmashToken","version":"0.1.0"},"capabilities":{"experimentalApi":true}}})).as_bytes()).await.map_err(|_| "Connexion Codex interrompue.")?;
        loop {
            let line = lines.next_line().await.map_err(|_| "Réponse Codex illisible.")?.ok_or("Codex s’est arrêté.")?;
            let value: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            if value["id"] == 1 { if value.get("error").is_some() { return Err("Initialisation Codex refusée.".to_string()); } break; }
        }
        for message in [json!({"method":"initialized","params":{}}), json!({"id":2,"method":"account/read","params":{"refreshToken":false}}), json!({"id":3,"method":"account/rateLimits/read"}), json!({"id":4,"method":"account/usage/read"})] {
            stdin.write_all(format!("{message}\n").as_bytes()).await.map_err(|_| "Connexion Codex interrompue.")?;
        }
        let mut responses = std::collections::HashMap::new();
        while responses.len() < 3 {
            let line = lines.next_line().await.map_err(|_| "Réponse Codex illisible.")?.ok_or("Codex s’est arrêté.")?;
            let value: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            if let Some(id @ 2..=4) = value["id"].as_u64() { responses.insert(id, value); }
        }
        let account_response = &responses[&2];
        if account_response["result"]["account"].is_null() { return Err("Connecte ce profil avec « codex login ».".into()); }
        if !responses[&3]["error"].is_null() { return Err("Quotas Codex indisponibles. Vérifie la connexion ChatGPT de ce profil.".into()); }
        let mut reading = parse(&responses[&3]["result"], &responses[&4]["result"])?;
        reading.plan = account_response["result"]["account"]["planType"].as_str().map(str::to_owned);
        Ok(reading)
    }).await;
    let _ = child.kill().await;
    let _ = child.wait().await;
    result.map_err(|_| {
        "Codex n’a pas répondu dans les 25 secondes. Réessaie après avoir ouvert Codex.".to_string()
    })?
}

pub fn parse(limits: &Value, usage: &Value) -> Result<Reading, String> {
    let mut reading = Reading {
        source: "Codex App Server".into(),
        ..Default::default()
    };
    let buckets: Vec<(String, &Value)> = if let Some(map) = limits["rateLimitsByLimitId"]
        .as_object()
        .filter(|m| !m.is_empty())
    {
        map.iter()
            .map(|(k, v)| (v["limitName"].as_str().unwrap_or(k).to_owned(), v))
            .collect()
    } else {
        vec![("Codex".into(), &limits["rateLimits"])]
    };
    let multiple = buckets.len() > 1;
    for (name, bucket) in buckets {
        for key in ["primary", "secondary"] {
            let window = &bucket[key];
            if let Some(used) = number(&window["usedPercent"]) {
                let minutes = window["windowDurationMins"].as_u64();
                let duration = match minutes {
                    Some(10080) => "7 jours".into(),
                    Some(m) if m % 60 == 0 => format!("{} h", m / 60),
                    Some(m) => format!("{m} min"),
                    None => if key == "primary" {
                        "Session"
                    } else {
                        "Période longue"
                    }
                    .into(),
                };
                reading.windows.push(Window {
                    label: if multiple {
                        format!("{name} · {duration}")
                    } else {
                        duration
                    },
                    used_percent: used,
                    resets_at: window["resetsAt"].as_i64(),
                });
            }
        }
    }
    if reading.windows.is_empty() {
        return Err("Aucun quota communiqué par Codex pour ce compte.".into());
    }
    reading.lifetime_tokens = usage["summary"]["lifetimeTokens"].as_u64();
    if let Some(buckets) = usage["dailyUsageBuckets"].as_array() {
        reading.daily = buckets
            .iter()
            .filter_map(|b| {
                Some(Daily {
                    date: b["startDate"].as_str()?.into(),
                    tokens: b["tokens"].as_u64()?,
                })
            })
            .collect();
        reading.daily.sort_by(|a, b| a.date.cmp(&b.date));
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        reading.tokens_today = reading
            .daily
            .iter()
            .find(|b| b.date == today)
            .map(|b| b.tokens);
    }
    Ok(reading)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uses_all_buckets_and_preserves_unknown_tokens() {
        let r = parse(&json!({"rateLimits":{"primary":{"usedPercent":99}},"rateLimitsByLimitId":{"a":{"primary":{"usedPercent":0,"windowDurationMins":300}},"b":{"primary":{"usedPercent":45}}}}), &Value::Null).unwrap();
        assert_eq!(r.windows.len(), 2);
        assert_eq!(r.windows[0].used_percent, 0.0);
        assert_eq!(r.tokens_today, None);
    }
    #[test]
    fn null_limits_are_not_zero() {
        assert!(parse(&json!({"rateLimits":{"primary":null}}), &Value::Null).is_err());
    }
}
