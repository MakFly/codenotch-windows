use crate::model::{number, Account, Daily, Reading, Window};
use serde_json::{json, Value};
use std::{future::Future, path::PathBuf, pin::Pin, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};

/// A `codex app-server` process speaking JSON-RPC over stdio; killed when dropped.
struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}

impl Session {
    fn spawn(profile_path: &str) -> Result<Self, String> {
        // Resolve the desktop installation before PATH; never invoke a shell or a user-supplied command.
        let mut cmd =
            Command::new(crate::login::executable("codex").unwrap_or_else(|| {
                PathBuf::from(if cfg!(windows) { "codex.exe" } else { "codex" })
            }));
        cmd.arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if !profile_path.is_empty() {
            cmd.env("CODEX_HOME", profile_path);
        }
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        let mut child = cmd.spawn().map_err(|_| {
            "Codex CLI introuvable. Installe Codex et connecte ton compte ChatGPT.".to_string()
        })?;
        let stdin = child.stdin.take().ok_or("Canal Codex indisponible.")?;
        let lines =
            BufReader::new(child.stdout.take().ok_or("Sortie Codex indisponible.")?).lines();
        Ok(Self {
            child,
            stdin,
            lines,
        })
    }
    async fn send(&mut self, message: Value) -> Result<(), String> {
        self.stdin
            .write_all(format!("{message}\n").as_bytes())
            .await
            .map_err(|_| "Connexion Codex interrompue.".to_string())
    }
    async fn next(&mut self) -> Result<Value, String> {
        let line = self
            .lines
            .next_line()
            .await
            .map_err(|_| "Réponse Codex illisible.")?
            .ok_or("Codex s’est arrêté.")?;
        Ok(serde_json::from_str(&line).unwrap_or(Value::Null))
    }
    async fn initialize(&mut self) -> Result<(), String> {
        self.send(json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"smashtoken","title":"SmashToken","version":"0.1.3"},"capabilities":{"experimentalApi":true}}})).await?;
        loop {
            let value = self.next().await?;
            if value["id"] == 1 {
                if value.get("error").is_some() {
                    return Err("Initialisation Codex refusée.".to_string());
                }
                return self.send(json!({"method":"initialized","params":{}})).await;
            }
        }
    }
}

pub struct LoginStart {
    /// Page to open: the OAuth authorize URL, or the device verification page.
    pub url: String,
    /// One-time code to type on the verification page (device code sign-in only).
    pub user_code: Option<String>,
}
pub type LoginOutcome = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Starts ChatGPT sign-in through the App Server, as the Codex apps do. Codex hosts the
/// OAuth callback and writes the profile's credentials; CodeNotch only opens the URL.
/// Falls back to a device code when the browser flow cannot start (callback port in use).
/// Returns `Ok(None)` when this Codex version has no App Server login.
pub async fn login(profile_path: &str) -> Result<Option<(LoginStart, LoginOutcome)>, String> {
    let mut session = Session::spawn(profile_path)?;
    let started = tokio::time::timeout(Duration::from_secs(25), async {
        session.initialize().await?;
        for (id, params) in [
            (2, json!({"type":"chatgpt"})),
            (3, json!({"type":"chatgptDeviceCode"})),
        ] {
            session
                .send(json!({"id":id,"method":"account/login/start","params":params}))
                .await?;
            let response = loop {
                let value = session.next().await?;
                if value["id"] == id {
                    break value;
                }
            };
            let result = &response["result"];
            let url = result["authUrl"]
                .as_str()
                .or_else(|| result["verificationUrl"].as_str());
            if let Some(url) = url.filter(|u| u.starts_with("https://auth.openai.com/")) {
                return Ok(Some(LoginStart {
                    url: url.to_owned(),
                    user_code: result["userCode"].as_str().map(str::to_owned),
                }));
            }
            // -32601: method not found, i.e. a Codex release without App Server login.
            if response["error"]["code"] == -32601 {
                return Ok(None);
            }
        }
        Err("Codex n’a pas pu démarrer la connexion ChatGPT. Réessaie dans un instant.".to_string())
    })
    .await
    .map_err(|_| "Codex n’a pas répondu dans les 25 secondes.".to_string())??;
    let Some(start) = started else {
        return Ok(None);
    };
    let outcome: LoginOutcome = Box::pin(async move {
        loop {
            let value = session.next().await?;
            if value["method"] == "account/login/completed" {
                let params = &value["params"];
                return if params["success"] == true {
                    Ok(())
                } else {
                    Err(params["error"]
                        .as_str()
                        .map(|e| format!("Connexion ChatGPT refusée : {e}"))
                        .unwrap_or_else(|| "Connexion ChatGPT annulée ou refusée.".into()))
                };
            }
        }
    });
    Ok(Some((start, outcome)))
}

pub async fn fetch(account: &Account) -> Result<Reading, String> {
    let mut session = Session::spawn(&account.profile_path)?;
    let (stdin, lines) = (&mut session.stdin, &mut session.lines);
    let result = tokio::time::timeout(Duration::from_secs(25), async {
        stdin.write_all(format!("{}\n", json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"smashtoken","title":"SmashToken","version":"0.1.3"},"capabilities":{"experimentalApi":true}}})).as_bytes()).await.map_err(|_| "Connexion Codex interrompue.")?;
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
        if account_response["result"]["account"].is_null() { return Err("Profil Codex non connecté. Utilise « Se connecter » dans Comptes ou « codex login ».".into()); }
        if !responses[&3]["error"].is_null() { return Err("Quotas Codex indisponibles. Vérifie la connexion ChatGPT de ce profil.".into()); }
        let mut reading = parse(&responses[&3]["result"], &responses[&4]["result"])?;
        reading.plan = account_response["result"]["account"]["planType"].as_str().map(str::to_owned);
        Ok(reading)
    }).await;
    let _ = session.child.kill().await;
    let _ = session.child.wait().await;
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
    #[ignore = "starts a real Codex login, then abandons it without opening a browser"]
    fn app_server_login_returns_an_openai_url() {
        let started = tauri::async_runtime::block_on(login("")).unwrap();
        let (start, outcome) = started.expect("this Codex has App Server login");
        assert!(start.url.starts_with("https://auth.openai.com/"));
        drop(outcome); // Kills app-server and its callback listener.
    }
    #[test]
    fn null_limits_are_not_zero() {
        assert!(parse(&json!({"rateLimits":{"primary":null}}), &Value::Null).is_err());
    }
}
