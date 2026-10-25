use crate::alerts::{label, Alert, Level};
use crate::telegram::{advice, gauge, status, usage_page};
use serde_json::{json, Value};
use std::time::Duration;

/// Windows Credential Manager entry: anyone holding a webhook URL can post to the channel.
const SECRET: &str = "discord-webhook";

pub fn saved_webhook() -> Option<String> {
    keyring::Entry::new("SmashToken", SECRET)
        .ok()?
        .get_password()
        .ok()
        .filter(|t| !t.is_empty())
}

/// Only Discord's own webhook endpoint is accepted, so alerts cannot be sent elsewhere.
pub fn valid_webhook(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url.trim()) else {
        return false;
    };
    let host_ok = matches!(
        url.host_str(),
        Some("discord.com" | "discordapp.com" | "canary.discord.com" | "ptb.discord.com")
    );
    let parts: Vec<_> = url.path_segments().map(|s| s.collect()).unwrap_or_default();
    url.scheme() == "https"
        && host_ok
        && url.username().is_empty()
        && url.fragment().is_none()
        && parts.len() == 4
        && parts[0] == "api"
        && parts[1] == "webhooks"
        && parts[2].chars().all(|c| c.is_ascii_digit())
        && !parts[2].is_empty()
        && parts[3].len() >= 20
        && parts[3]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn save_webhook(url: &str) -> Result<(), String> {
    if !valid_webhook(url) {
        return Err("Webhook Discord invalide. Copie l’URL complète : Paramètres du salon → Intégrations → Webhooks.".into());
    }
    keyring::Entry::new("SmashToken", SECRET)
        .map_err(|_| "Coffre Windows indisponible.")?
        .set_password(url.trim())
        .map_err(|_| "Impossible d’enregistrer le webhook dans le coffre Windows.".into())
}

pub fn clear_webhook() -> Result<(), String> {
    match keyring::Entry::new("SmashToken", SECRET).map(|e| e.delete_credential()) {
        Ok(Ok(())) | Ok(Err(keyring::Error::NoEntry)) => Ok(()),
        _ => Err("Impossible d’effacer le webhook du coffre Windows.".into()),
    }
}

/// Embed colour per level, matching the notch's usage colours.
fn color(level: Level) -> u32 {
    match level {
        Level::Recovered => 0x4caf72,
        Level::Forecast => 0x5c9bf5,
        Level::Warning => 0xe0b84a,
        Level::Critical => 0xe8873a,
        Level::Reached => 0xd9534f,
    }
}

/// A Discord embed card. `<t:…:R>` timestamps render in each reader's language and timezone.
pub fn message(a: &Alert, french: bool) -> Value {
    let (icon, state) = status(a.level, french);
    let used = if french {
        format!("{:.0} %", a.used)
    } else {
        format!("{:.0}%", a.used)
    };
    let mut fields = vec![json!({
        "name": if french { "Fenêtre" } else { "Window" },
        "value": label(&a.label, french),
        "inline": true,
    })];
    if let Some(at) = a.exhausts_at.filter(|_| a.level != Level::Recovered) {
        fields.push(json!({
            "name": if french { "Plafond estimé" } else { "Expected limit" },
            "value": format!("<t:{at}:R>"),
            "inline": true,
        }));
    }
    if let Some(at) = a.resets_at {
        fields.push(json!({
            "name": if french { "Réinitialisation" } else { "Resets" },
            "value": format!("<t:{at}:R> · <t:{at}:t>"),
            "inline": true,
        }));
    }
    let mut embed = json!({
        "title": format!("{icon} {} · {state}", a.account),
        "description": format!("`{}`  **{used}**\n\n*{}*", gauge(a.used), advice(a.level, french)),
        "color": color(a.level),
        "fields": fields,
        "footer": {"text": "CodeNotch"},
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    if let Some((_, url)) = usage_page(&a.provider) {
        embed["url"] = json!(url);
    }
    json!({
        "username": "CodeNotch",
        "embeds": [embed],
        // Account names come from the user; never let them ping anyone.
        "allowed_mentions": {"parse": []},
    })
}

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
    );
    body["content"] = json!(if french {
        "✅ **CodeNotch est connecté à ce salon.** Voici à quoi ressemblera une alerte :"
    } else {
        "✅ **CodeNotch is connected to this channel.** This is what an alert will look like:"
    });
    body
}

pub async fn send(webhook: &str, body: Value) -> Result<(), String> {
    if !valid_webhook(webhook) {
        return Err("Webhook Discord invalide.".into());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Client réseau indisponible.")?;
    let response = client
        .post(webhook.trim())
        .json(&body)
        .send()
        .await
        .map_err(|_| "Discord injoignable. Vérifie la connexion.")?;
    match response.status().as_u16() {
        200..=299 => Ok(()),
        401 | 403 | 404 => Err(
            "Webhook refusé par Discord : il a peut-être été supprimé. Crée-en un nouveau.".into(),
        ),
        429 => Err("Discord limite les envois. Réessaie dans un instant.".into()),
        _ => Err("Discord a refusé le message.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_discord_webhooks_are_accepted() {
        let token = "abcdefghijklmnopqrstuvwxyz_0123456789-ABC";
        assert!(valid_webhook(&format!(
            "https://discord.com/api/webhooks/123456/{token}"
        )));
        assert!(valid_webhook(&format!(
            "https://ptb.discord.com/api/webhooks/123456/{token}"
        )));
        assert!(!valid_webhook(&format!(
            "http://discord.com/api/webhooks/123456/{token}"
        )));
        assert!(!valid_webhook(&format!(
            "https://discord.com.evil.io/api/webhooks/123456/{token}"
        )));
        assert!(!valid_webhook(&format!(
            "https://discord.com/api/webhooks/abc/{token}"
        )));
        assert!(!valid_webhook(
            "https://discord.com/api/webhooks/123456/short"
        ));
        assert!(!valid_webhook(&format!(
            "https://user@discord.com/api/webhooks/123456/{token}"
        )));
    }
    #[test]
    #[ignore = "posts to the webhook in CODENOTCH_DISCORD_WEBHOOK"]
    fn send_real_test_card() {
        let webhook =
            std::env::var("CODENOTCH_DISCORD_WEBHOOK").expect("set CODENOTCH_DISCORD_WEBHOOK");
        let now = chrono::Utc::now().timestamp();
        tauri::async_runtime::block_on(async {
            send(&webhook, test_message(true, now)).await.unwrap();
            // One real alert card as well, so the layout of each kind can be checked.
            let alert = Alert {
                account: "Claude · personnel".into(),
                provider: "claude".into(),
                label: "5 h".into(),
                level: Level::Critical,
                used: 96.0,
                threshold: 95,
                resets_at: Some(now + 95 * 60),
                exhausts_at: Some(now + 12 * 60),
            };
            send(&webhook, message(&alert, true)).await.unwrap();
        });
        println!("discord: 2 messages sent");
    }
    #[test]
    fn embed_carries_level_colour_fields_and_no_mentions() {
        let alert = Alert {
            account: "@everyone".into(),
            provider: "codex".into(),
            label: "7 jours".into(),
            level: Level::Critical,
            used: 96.0,
            threshold: 95,
            resets_at: Some(2_000_000_000),
            exhausts_at: Some(1_999_000_000),
        };
        let body = message(&alert, false);
        let embed = &body["embeds"][0];
        assert_eq!(embed["color"], 0xe8873a);
        assert_eq!(embed["url"], "https://chatgpt.com/codex/settings/usage");
        assert_eq!(embed["fields"][0]["value"], "7 days");
        assert_eq!(embed["fields"][1]["value"], "<t:1999000000:R>");
        assert_eq!(body["allowed_mentions"]["parse"], json!([]));
    }
}
