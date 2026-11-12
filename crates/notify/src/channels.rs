//! Remote delivery: Telegram Bot API and Discord webhooks.
//!
//! * Secrets are resolved at send time (`resolve_secret`: env var, then the store).
//! * 8 s timeout per request; one retry after 1 s on a transient failure (timeout, connection,
//!   HTTP 5xx); HTTP 429 waits `Retry-After` (header or JSON body) once when it is ≤ 10 s,
//!   otherwise gives up with "try again in N s". Worst case per channel ≈ 2 × 8 s + 10 s.
//! * Errors are localised, short and secret-free: they never contain the bot token, the chat id
//!   or the webhook URL (transport error texts are not passed through, response descriptions are
//!   scrubbed of every secret value and redacted).

use crate::messages::{t, tf};
use codenotch_core::http::{HttpClient, HttpError, HttpRequest, HttpResponse};
use codenotch_core::i18n::Lang;
use codenotch_core::secrets::{
    resolve_secret, SecretKey, SecretStore, DISCORD_WEBHOOK_ENV, TELEGRAM_TOKEN_ENV,
};
use codenotch_core::{Env, Error, Result, Settings};
use std::sync::Arc;
use std::time::Duration;

/// Telegram Bot API base.
pub const TELEGRAM_API: &str = "https://api.telegram.org";
/// Per-request timeout.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
/// Pause before the single retry after a transient failure.
pub const RETRY_PAUSE: Duration = Duration::from_secs(1);
/// Longest `Retry-After` honoured inline.
pub const MAX_RETRY_AFTER_SECS: i64 = 10;
/// Discord message limit is 2000 characters, Telegram 4096.
const DISCORD_MAX: usize = 1900;
const TELEGRAM_MAX: usize = 4000;

/// Injectable pause (tests record instead of sleeping).
pub type Sleeper = Arc<dyn Fn(Duration) + Send + Sync>;

/// Everything remote delivery needs; cheap to clone and `Send` (moved to a delivery thread).
#[derive(Clone)]
pub struct Remote {
    pub http: Arc<dyn HttpClient>,
    pub env: Env,
    pub secrets: Arc<dyn SecretStore>,
    pub sleep: Sleeper,
}

enum Failure {
    RateLimited(i64),
    Status(u16, HttpResponse),
    Transport(HttpError),
}

impl Remote {
    /// Send `title` + `body` to the configured Telegram chat.
    pub fn telegram(&self, settings: &Settings, title: &str, body: &str) -> Result<()> {
        let lang = settings.lang();
        let token = resolve_secret(&SecretKey::TelegramToken, &self.env, &*self.secrets)
            .ok_or_else(|| {
                Error::other(tf(
                    lang,
                    "Telegram bot token is missing. Add it in Settings or set {var}.",
                    &[("var", TELEGRAM_TOKEN_ENV)],
                ))
            })?;
        let token = token.trim().to_string();
        let chat = settings.telegram_chat_id.as_str().trim().to_string();
        let secrets = [token.as_str(), chat.as_str()];
        if chat.is_empty() {
            return Err(Error::other(t(lang, "Telegram chat id is missing.")));
        }
        if !valid_telegram_token(&token) {
            return Err(Error::other(t(lang, "Telegram bot token looks invalid.")));
        }
        if !valid_chat_id(&chat) {
            return Err(Error::other(t(lang, "Telegram chat id looks invalid.")));
        }
        let text = truncate(&format!("{title}\n{body}"), TELEGRAM_MAX);
        let payload = serde_json::json!({
            "chat_id": chat,
            "text": text,
            "disable_web_page_preview": true,
        });
        let request = HttpRequest::post(format!("{TELEGRAM_API}/bot{token}/sendMessage"))
            .json(&payload)
            .timeout(REQUEST_TIMEOUT);
        match self.send(&request) {
            Ok(()) => Ok(()),
            Err(Failure::Status(401 | 404, _)) => {
                Err(Error::other(t(lang, "Telegram rejected the bot token.")))
            }
            Err(Failure::Status(code @ (400 | 403), resp)) => {
                let reason = response_reason(&resp, "description", &secrets)
                    .unwrap_or_else(|| format!("HTTP {code}"));
                Err(Error::other(tf(
                    lang,
                    "Telegram refused the message: {reason}",
                    &[("reason", &reason)],
                )))
            }
            Err(other) => Err(failure_error(lang, "Telegram", other)),
        }
    }

    /// Post `title` + `body` to the configured Discord webhook.
    pub fn discord(&self, settings: &Settings, title: &str, body: &str) -> Result<()> {
        let lang = settings.lang();
        let url = resolve_secret(&SecretKey::DiscordWebhook, &self.env, &*self.secrets)
            .ok_or_else(|| {
                Error::other(tf(
                    lang,
                    "Discord webhook URL is missing. Add it in Settings or set {var}.",
                    &[("var", DISCORD_WEBHOOK_ENV)],
                ))
            })?;
        let url = url.trim().to_string();
        if !valid_discord_webhook(&url) {
            return Err(Error::other(t(lang, "This is not a Discord webhook URL.")));
        }
        let secrets = [url.as_str(), webhook_token(&url)];
        let content = truncate(
            &format!("**{}**\n{}", discord_escape(title), body),
            DISCORD_MAX,
        );
        let payload = serde_json::json!({
            "content": content,
            "username": "CodeNotch",
            "allowed_mentions": { "parse": [] },
        });
        let request = HttpRequest::post(url.clone())
            .json(&payload)
            .timeout(REQUEST_TIMEOUT);
        match self.send(&request) {
            Ok(()) => Ok(()),
            Err(Failure::Status(401 | 403 | 404, _)) => Err(Error::other(t(
                lang,
                "Discord rejected the webhook (deleted or wrong URL).",
            ))),
            Err(Failure::Status(400, resp)) => {
                let reason = response_reason(&resp, "message", &secrets)
                    .unwrap_or_else(|| "HTTP 400".into());
                Err(Error::other(tf(
                    lang,
                    "Discord refused the message: {reason}",
                    &[("reason", &reason)],
                )))
            }
            Err(other) => Err(failure_error(lang, "Discord", other)),
        }
    }

    /// One request with at most one retry.
    fn send(&self, request: &HttpRequest) -> std::result::Result<(), Failure> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let last = attempt >= 2;
            match self.http.send(request) {
                Ok(resp) if resp.is_success() => return Ok(()),
                Ok(resp) if resp.status == 429 => {
                    let wait = retry_after(&resp);
                    match wait {
                        Some(secs) if !last && secs <= MAX_RETRY_AFTER_SECS => {
                            (self.sleep)(Duration::from_secs(secs.max(1) as u64));
                        }
                        _ => return Err(Failure::RateLimited(wait.unwrap_or(60).max(1))),
                    }
                }
                Ok(resp) if resp.status >= 500 && !last => (self.sleep)(RETRY_PAUSE),
                Ok(resp) => return Err(Failure::Status(resp.status, resp)),
                Err(HttpError::Timeout | HttpError::Connect(_)) if !last => {
                    (self.sleep)(RETRY_PAUSE)
                }
                Err(err) => return Err(Failure::Transport(err)),
            }
        }
    }
}

/// `Retry-After` header (seconds or HTTP date), else the JSON body (`retry_after` for Discord,
/// `parameters.retry_after` for Telegram). Rounded up.
fn retry_after(resp: &HttpResponse) -> Option<i64> {
    if let Some(secs) = resp.retry_after_secs(codenotch_core::now_secs()) {
        return Some(secs);
    }
    let v: serde_json::Value = resp.json().ok()?;
    let raw = v
        .get("retry_after")
        .or_else(|| v.get("parameters").and_then(|p| p.get("retry_after")))?;
    let secs = raw.as_f64()?;
    secs.is_finite().then(|| secs.max(0.0).ceil() as i64)
}

fn failure_error(lang: Lang, channel: &str, failure: Failure) -> Error {
    match failure {
        Failure::RateLimited(s) => Error::other(tf(
            lang,
            "{channel} is rate limiting: try again in {s} s.",
            &[("channel", channel), ("s", &s.to_string())],
        )),
        Failure::Status(code, _) => Error::other(tf(
            lang,
            "{channel} answered with HTTP {code}.",
            &[("channel", channel), ("code", &code.to_string())],
        )),
        Failure::Transport(err) => {
            // The transport text may contain the URL (and so the token): never pass it through.
            let reason = match err {
                HttpError::Timeout => t(lang, "timed out"),
                HttpError::Connect(_) => t(lang, "no connection"),
                HttpError::Tls(_) => t(lang, "TLS error"),
                _ => t(lang, "unexpected error"),
            };
            Error::other(tf(
                lang,
                "Could not reach {channel}: {reason}",
                &[("channel", channel), ("reason", reason)],
            ))
        }
    }
}

/// A short, secret-free reason taken from a JSON error body.
fn response_reason(resp: &HttpResponse, field: &str, secrets: &[&str]) -> Option<String> {
    let v: serde_json::Value = resp.json().ok()?;
    let text = v.get(field)?.as_str()?;
    let clean = scrub(text, secrets);
    (!clean.is_empty()).then_some(clean)
}

/// Remove every secret value (and anything that looks like one), keep it short.
pub(crate) fn scrub(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for s in secrets.iter().filter(|s| s.len() >= 4) {
        out = out.replace(*s, "***");
    }
    let out = codenotch_core::logging::redact(&out);
    let out: String = out.chars().filter(|c| !c.is_control()).collect();
    truncate(out.trim(), 120)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    s.push('…');
    s
}

/// `<digits>:<[A-Za-z0-9_-]+>` — also guarantees the token cannot alter the URL.
pub(crate) fn valid_telegram_token(token: &str) -> bool {
    let Some((id, secret)) = token.split_once(':') else {
        return false;
    };
    !id.is_empty()
        && id.chars().all(|c| c.is_ascii_digit())
        && secret.len() >= 20
        && secret
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Numeric id (`-100…` for groups/channels) or `@channelusername`.
pub(crate) fn valid_chat_id(chat: &str) -> bool {
    if let Some(name) = chat.strip_prefix('@') {
        return name.len() >= 4 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    }
    let digits = chat.strip_prefix('-').unwrap_or(chat);
    !digits.is_empty() && digits.len() <= 20 && digits.chars().all(|c| c.is_ascii_digit())
}

/// `https://{discord.com|discordapp.com|ptb.discord.com|canary.discord.com}/api[/vN]/webhooks/<id>/<token>`.
pub(crate) fn valid_discord_webhook(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "discord.com"
            | "www.discord.com"
            | "discordapp.com"
            | "ptb.discord.com"
            | "canary.discord.com"
    ) {
        return false;
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    let mut parts = path.split('/').filter(|p| !p.is_empty());
    if parts.next() != Some("api") {
        return false;
    }
    let mut next = parts.next();
    if next.is_some_and(|p| {
        p.len() >= 2 && p.starts_with('v') && p[1..].chars().all(|c| c.is_ascii_digit())
    }) {
        next = parts.next();
    }
    if next != Some("webhooks") {
        return false;
    }
    let id = parts.next().unwrap_or("");
    let token = parts.next().unwrap_or("");
    !id.is_empty()
        && id.chars().all(|c| c.is_ascii_digit())
        && !token.is_empty()
        && parts.next().is_none()
}

/// The token segment of a webhook URL (for scrubbing).
fn webhook_token(url: &str) -> &str {
    url.split(['?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('/')
        .find(|p| !p.is_empty())
        .unwrap_or("")
}

/// Escape Discord markdown in a title.
fn discord_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '*' | '_' | '~' | '`' | '|' | '\\' | '>') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validators() {
        assert!(valid_telegram_token(
            "123456:ABCdefGHIjklMNOpqrSTUvwx_yz-12"
        ));
        assert!(!valid_telegram_token("123456:short"));
        assert!(!valid_telegram_token("abc:ABCdefGHIjklMNOpqrSTUvwx"));
        assert!(!valid_telegram_token("123456:ABCdefGHIjklMNOpqrSTU/../x"));
        assert!(valid_chat_id("987654321"));
        assert!(valid_chat_id("-1001234567890"));
        assert!(valid_chat_id("@my_channel"));
        assert!(!valid_chat_id("12 34"));
        assert!(!valid_chat_id(""));
        assert!(valid_discord_webhook(
            "https://discord.com/api/webhooks/123/abc-DEF_x"
        ));
        assert!(valid_discord_webhook(
            "https://discordapp.com/api/v10/webhooks/123/abc?thread_id=5"
        ));
        assert!(!valid_discord_webhook(
            "http://discord.com/api/webhooks/123/abc"
        ));
        assert!(!valid_discord_webhook(
            "https://discord.com.evil.io/api/webhooks/123/abc"
        ));
        assert!(!valid_discord_webhook(
            "https://discord.com/api/webhooks/123"
        ));
        assert!(!valid_discord_webhook(
            "https://discord.com/api/webhooks/123/abc/github"
        ));
        assert_eq!(
            webhook_token("https://discord.com/api/webhooks/123/tok?x=1"),
            "tok"
        );
    }

    #[test]
    fn scrub_removes_secrets_and_truncates() {
        let s = scrub(
            "bad chat 987654321 for bot123:SECRETSECRET\nx",
            &["987654321", "123:SECRETSECRET"],
        );
        assert!(!s.contains("987654321"));
        assert!(!s.contains("SECRETSECRET"));
        assert!(!s.contains('\n'));
        assert!(scrub(&"x".repeat(500), &[]).chars().count() <= 120);
        assert_eq!(discord_escape("a*b_c"), "a\\*b\\_c");
    }

    #[test]
    fn retry_after_sources() {
        let r = HttpResponse::new(429, "").with_header("Retry-After", "3");
        assert_eq!(retry_after(&r), Some(3));
        let r = HttpResponse::new(429, r#"{"ok":false,"parameters":{"retry_after":7}}"#);
        assert_eq!(retry_after(&r), Some(7));
        let r = HttpResponse::new(429, r#"{"message":"slow","retry_after":0.4}"#);
        assert_eq!(retry_after(&r), Some(1));
        assert_eq!(retry_after(&HttpResponse::new(429, "nope")), None);
    }
}
