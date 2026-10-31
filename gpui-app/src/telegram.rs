use crate::alerts::{duration, label, Alert, Level};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

/// Windows Credential Manager entry holding the bot token.
const SECRET: &str = "telegram-bot";
/// Lets the Telegram channel be configured and alerts be sent.
pub const AVAILABLE: bool = true;
/// Longest `retry_after` worth waiting for once, rather than dropping the message.
const MAX_RETRY_WAIT: u64 = 30;

pub fn saved_token() -> Option<String> {
    keyring::Entry::new("SmashToken", SECRET)
        .ok()?
        .get_password()
        .ok()
        .filter(|t| !t.is_empty())
}

pub fn save_token(token: &str) -> Result<(), String> {
    let token = token.trim();
    // Bot tokens look like 123456789:AA… (BotFather format).
    let valid = token.split_once(':').is_some_and(|(id, key)| {
        !id.is_empty()
            && id.chars().all(|c| c.is_ascii_digit())
            && key.len() >= 30
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    });
    if !valid {
        return Err("Jeton de bot invalide. Copie le jeton complet donné par @BotFather.".into());
    }
    keyring::Entry::new("SmashToken", SECRET)
        .map_err(|_| "Coffre Windows indisponible.")?
        .set_password(token)
        .map_err(|_| "Impossible d’enregistrer le jeton dans le coffre Windows.".into())
}

pub fn clear_token() -> Result<(), String> {
    match keyring::Entry::new("SmashToken", SECRET).map(|e| e.delete_credential()) {
        Ok(Ok(())) | Ok(Err(keyring::Error::NoEntry)) => Ok(()),
        _ => Err("Impossible d’effacer le jeton du coffre Windows.".into()),
    }
}

pub fn valid_chat_id(chat: &str) -> bool {
    let chat = chat.trim();
    (chat.len() <= 64)
        && (chat
            .strip_prefix('-')
            .unwrap_or(chat)
            .chars()
            .all(|c| c.is_ascii_digit())
            && !chat.trim_start_matches('-').is_empty()
            || chat.strip_prefix('@').is_some_and(|name| {
                name.len() >= 4 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            }))
}

async fn call(token: &str, method: &str, body: Value) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Client réseau indisponible.")?;
    let url = format!("https://api.telegram.org/bot{token}/{method}");
    let post = || async {
        let response = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|_| "Telegram injoignable. Vérifie la connexion.")?;
        let status = response.status().as_u16();
        let value: Value = response.json().await.unwrap_or(Value::Null);
        Ok::<_, String>((status, value))
    };
    let (mut status, mut value) = post().await?;
    // Flood control: wait the delay Telegram asks for, once, when it is short.
    if let Some(wait) = retry_after(status, &value) {
        tokio::time::sleep(Duration::from_secs(wait)).await;
        (status, value) = post().await?;
    }
    if value["ok"] == true {
        return Ok(value["result"].clone());
    }
    let description = value["description"].as_str().unwrap_or("").to_lowercase();
    Err(match status {
        401 | 404 => "Jeton de bot refusé par Telegram. Vérifie-le auprès de @BotFather.",
        403 => "Le bot ne peut pas écrire dans ce chat : envoie-lui /start, ou ajoute-le au groupe.",
        409 => "Ce bot utilise un webhook : Telegram ne permet pas de détecter le chat. Saisis son identifiant.",
        400 if description.contains("chat not found") => {
            "Chat introuvable. Envoie /start au bot, puis détecte le chat à nouveau."
        }
        429 => "Telegram limite les envois. Réessaie dans un instant.",
        _ => "Telegram a refusé le message.",
    }
    .into())
}

fn retry_after(status: u16, value: &Value) -> Option<u64> {
    (status == 429)
        .then(|| value["parameters"]["retry_after"].as_u64())
        .flatten()
        .filter(|s| *s <= MAX_RETRY_WAIT)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub id: String,
    pub name: String,
}

/// Chats that recently wrote to the bot, newest first, so the user does not have to
/// look up an identifier: they send /start, then pick their chat.
pub async fn detect_chats(token: &str) -> Result<Vec<Chat>, String> {
    let updates = call(token, "getUpdates", json!({"limit": 50, "timeout": 0})).await?;
    let mut chats: Vec<Chat> = vec![];
    for update in updates.as_array().into_iter().flatten().rev() {
        let message = [
            "message",
            "channel_post",
            "my_chat_member",
            "edited_message",
        ]
        .iter()
        .map(|k| &update[*k])
        .find(|m| m.is_object());
        let Some(chat) = message.map(|m| &m["chat"]) else {
            continue;
        };
        let Some(id) = chat["id"].as_i64().map(|i| i.to_string()) else {
            continue;
        };
        if chats.iter().any(|c| c.id == id) {
            continue;
        }
        let name = chat["title"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                let first = chat["first_name"].as_str().unwrap_or("");
                let last = chat["last_name"].as_str().unwrap_or("");
                Some(format!("{first} {last}").trim().to_owned()).filter(|n| !n.is_empty())
            })
            .or_else(|| chat["username"].as_str().map(|u| format!("@{u}")))
            .unwrap_or_else(|| id.clone());
        chats.push(Chat { id, name });
    }
    if chats.is_empty() {
        return Err(
            "Aucun message reçu par le bot. Envoie-lui /start dans Telegram, puis réessaie.".into(),
        );
    }
    Ok(chats)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Ten-segment gauge: ▰▰▰▰▰▰▰▱▱▱
pub fn gauge(used: f64) -> String {
    let filled = ((used.clamp(0.0, 100.0) / 10.0).round()) as usize;
    format!("{}{}", "▰".repeat(filled), "▱".repeat(10 - filled))
}

fn clock(at: i64, now: i64) -> String {
    use chrono::TimeZone;
    let Some(local) = chrono::Local.timestamp_opt(at, 0).single() else {
        return String::new();
    };
    if at - now < 20 * 3600 {
        local.format("%H:%M").to_string()
    } else {
        local.format("%d/%m %H:%M").to_string()
    }
}

/// Provider page where the user can check or manage the quota.
pub fn usage_page(provider: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        "claude" => Some(("Claude", "https://claude.ai/settings/usage")),
        "codex" => Some(("ChatGPT", "https://chatgpt.com/codex/settings/usage")),
        "opencode" => Some(("OpenCode", "https://opencode.ai/zen")),
        _ => None,
    }
}

/// Icon and wording shared by the Telegram and Discord cards.
pub fn status(level: Level, french: bool) -> (&'static str, &'static str) {
    match (level, french) {
        (Level::Forecast, true) => ("⏳", "plafond en approche"),
        (Level::Forecast, false) => ("⏳", "limit approaching"),
        (Level::Warning, true) => ("🟡", "seuil d’alerte"),
        (Level::Warning, false) => ("🟡", "warning threshold"),
        (Level::Critical, true) => ("🟠", "seuil critique"),
        (Level::Critical, false) => ("🟠", "critical threshold"),
        (Level::Reached, true) => ("🔴", "plafond atteint"),
        (Level::Reached, false) => ("🔴", "limit reached"),
        (Level::Recovered, true) => ("🟢", "quota rétabli"),
        (Level::Recovered, false) => ("🟢", "quota available again"),
    }
}

/// Closing advice under each card.
pub fn advice(level: Level, french: bool) -> &'static str {
    match (level, french) {
        (Level::Recovered, true) => "Les agents en pause peuvent reprendre.",
        (Level::Recovered, false) => "Paused agents can resume.",
        (Level::Reached, true) => {
            "Les agents de ce compte sont bloqués jusqu’à la réinitialisation."
        }
        (Level::Reached, false) => "Agents on this account are blocked until the reset.",
        (_, true) => "Pense à ralentir ou à mettre en pause les agents en fond.",
        (_, false) => "Consider slowing down or pausing background agents.",
    }
}

/// A compact card: status line, window, gauge, forecast and reset, plus a link button.
pub fn message(a: &Alert, french: bool, now: i64) -> Value {
    let (icon, status) = status(a.level, french);
    let mut lines = vec![
        format!("{icon} <b>{} · {status}</b>", escape(&a.account)),
        String::new(),
        format!("<b>{}</b>", escape(&label(&a.label, french))),
        format!(
            "<code>{}</code>  <b>{}</b>",
            gauge(a.used),
            if french {
                format!("{:.0} %", a.used)
            } else {
                format!("{:.0}%", a.used)
            }
        ),
    ];
    if let Some(at) = a.exhausts_at.filter(|_| a.level != Level::Recovered) {
        lines.push(if french {
            format!(
                "⏱ Plafond estimé dans <b>{}</b> ({})",
                duration(at - now, true),
                clock(at, now)
            )
        } else {
            format!(
                "⏱ Limit expected in <b>{}</b> ({})",
                duration(at - now, false),
                clock(at, now)
            )
        });
    }
    if let Some(at) = a.resets_at.filter(|at| *at > now) {
        lines.push(if french {
            format!(
                "🔄 Réinitialisation dans <b>{}</b> ({})",
                duration(at - now, true),
                clock(at, now)
            )
        } else {
            format!(
                "🔄 Resets in <b>{}</b> ({})",
                duration(at - now, false),
                clock(at, now)
            )
        });
    }
    lines.push(String::new());
    lines.push(format!("<i>{}</i>", advice(a.level, french)));
    let mut body = json!({
        "text": lines.join("\n"),
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
    });
    if let Some((name, url)) = usage_page(&a.provider) {
        let text = if french {
            format!("Voir l’usage {name}")
        } else {
            format!("Open {name} usage")
        };
        body["reply_markup"] = json!({"inline_keyboard": [[{"text": text, "url": url}]]});
    }
    body
}

pub async fn send(token: &str, chat: &str, mut body: Value) -> Result<(), String> {
    body["chat_id"] = json!(chat.trim());
    call(token, "sendMessage", body).await.map(|_| ())
}

/// Sample card so the user sees exactly what alerts will look like.
pub fn test_message(french: bool, now: i64) -> Value {
    let mut body = message(
        &Alert {
            account: "CodeNotch".into(),
            provider: String::new(),
            label: "5 h".into(),
            level: Level::Forecast,
            used: 72.0,
            threshold: 80,
            resets_at: Some(now + 100 * 60),
            exhausts_at: Some(now + 25 * 60),
        },
        french,
        now,
    );
    let intro = if french {
        "✅ <b>CodeNotch est connecté à ce chat.</b>\nVoici à quoi ressemblera une alerte :\n\n"
    } else {
        "✅ <b>CodeNotch is connected to this chat.</b>\nThis is what an alert will look like:\n\n"
    };
    body["text"] = json!(format!("{intro}{}", body["text"].as_str().unwrap_or("")));
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flood_control_waits_only_for_short_delays() {
        let asked = |s| json!({"ok": false, "parameters": {"retry_after": s}});
        assert_eq!(retry_after(429, &asked(5)), Some(5));
        assert_eq!(retry_after(429, &asked(120)), None);
        assert_eq!(retry_after(400, &asked(5)), None);
        assert_eq!(retry_after(429, &json!({"ok": false})), None);
    }
    #[test]
    fn chat_ids_and_gauge() {
        assert!(valid_chat_id("123456"));
        assert!(valid_chat_id("-1001234567890"));
        assert!(valid_chat_id("@mon_canal"));
        assert!(!valid_chat_id("-"));
        assert!(!valid_chat_id("12a"));
        assert!(!valid_chat_id("@ab"));
        assert_eq!(gauge(72.0), "▰▰▰▰▰▰▰▱▱▱");
        assert_eq!(gauge(140.0), "▰▰▰▰▰▰▰▰▰▰");
    }
    #[test]
    fn card_escapes_names_and_links_the_usage_page() {
        let alert = Alert {
            account: "Perso <work>".into(),
            provider: "claude".into(),
            label: "7 jours".into(),
            level: Level::Reached,
            used: 100.0,
            threshold: 100,
            resets_at: Some(10_000),
            exhausts_at: None,
        };
        let body = message(&alert, false, 0);
        let text = body["text"].as_str().unwrap();
        assert!(text.contains("Perso &lt;work&gt; · limit reached"));
        assert!(text.contains("<b>7 days</b>"));
        assert!(text.contains("▰▰▰▰▰▰▰▰▰▰"));
        assert_eq!(
            body["reply_markup"]["inline_keyboard"][0][0]["url"],
            "https://claude.ai/settings/usage"
        );
    }
}
