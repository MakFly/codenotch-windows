use crate::model::Account;
use crate::usage::{self, Reading};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

const MAX_BODY: usize = 2 * 1024 * 1024;

pub fn fetch(account: &Account) -> Reading {
    if !account.endpoint.is_empty() {
        return fetch_custom(account);
    }
    match account.provider.to_ascii_lowercase().as_str() {
        "claude" => fetch_claude(account),
        "codex" => fetch_codex(account),
        "grok" => fetch_grok(account),
        other => Reading::failed(format!("{other}: no reader in this rebuild")),
    }
}

fn fetch_claude(account: &Account) -> Reading {
    let profile = profile_path(account, ".claude");
    let creds = profile.join(".credentials.json");
    let token = match json_at(&creds, &["claudeAiOauth", "accessToken"]) {
        Some(token) => token,
        None => {
            return Reading::failed("Claude session not found. Run claude auth login.");
        }
    };
    let client = http_client();
    let response = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("anthropic-version", "2023-06-01")
        .header("user-agent", "claude-code")
        .send();
    finish(response, |body| usage::parse_claude_usage(body, usage::now_secs()))
}

fn fetch_codex(account: &Account) -> Reading {
    let profile = profile_path(account, ".codex");
    let auth_path = profile.join("auth.json");
    let Some(auth) = read_json(&auth_path) else {
        return Reading::failed("Codex session not found.");
    };
    let token = auth
        .pointer("/tokens/access_token")
        .and_then(|value| value.as_str());
    let account_id = auth
        .pointer("/tokens/account_id")
        .and_then(|value| value.as_str());
    let (Some(token), Some(account_id)) = (token, account_id) else {
        return Reading::failed("Codex auth.json has no ChatGPT session.");
    };
    let client = http_client();
    let response = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(token)
        .header("ChatGPT-Account-Id", account_id)
        .header("user-agent", "codex-cli")
        .send();
    finish(response, |body| usage::parse_codex_usage(body, usage::now_secs()))
}

fn fetch_grok(account: &Account) -> Reading {
    let profile = profile_path(account, ".grok");
    if !profile.join("auth.json").is_file() {
        return Reading::failed("Grok session not found. Run grok login.");
    }
    // The old app called an internal billing endpoint this rebuild does not have.
    // A missing number stays a dash. It is not reported as zero.
    Reading::failed("Grok session found. Quota reading is not reconnected in this rebuild.")
}

fn fetch_custom(account: &Account) -> Reading {
    if !allowed_endpoint(&account.endpoint) {
        return Reading::failed("custom endpoint must be https, or http on localhost");
    }
    let client = http_client();
    let mut request = client.get(&account.endpoint);
    if !account.secret_env.is_empty() {
        match std::env::var(&account.secret_env) {
            Ok(token) if !token.is_empty() => {
                request = request.bearer_auth(token);
            }
            _ => {
                return Reading::failed(format!(
                    "environment variable {} is empty",
                    account.secret_env
                ));
            }
        }
    }
    finish(request.send(), |body| {
        usage::parse_custom_usage(body, usage::now_secs())
    })
}

fn finish(
    response: Result<reqwest::blocking::Response, reqwest::Error>,
    parse: impl FnOnce(&str) -> Result<Vec<usage::QuotaWindow>, String>,
) -> Reading {
    let response = match response {
        Ok(response) => response,
        Err(err) => return Reading::failed(short_error(&err.to_string())),
    };
    let status = response.status();
    if status.as_u16() == 429 {
        return Reading::failed("rate limited (429)");
    }
    let bytes = match response.bytes() {
        Ok(bytes) => bytes,
        Err(err) => return Reading::failed(short_error(&err.to_string())),
    };
    if bytes.len() > MAX_BODY {
        return Reading::failed("response is larger than 2 MB");
    }
    let body = String::from_utf8_lossy(&bytes);
    if !status.is_success() {
        return Reading::failed(format!("HTTP {}", status.as_u16()));
    }
    match parse(&body) {
        Ok(windows) => Reading::ready(windows),
        Err(err) => Reading::failed(err),
    }
}

pub fn allowed_endpoint(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    match parsed.scheme() {
        "https" => true,
        "http" => matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1")),
        _ => false,
    }
}

fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

fn profile_path(account: &Account, default_dir: &str) -> std::path::PathBuf {
    if !account.profile_path.is_empty() {
        return std::path::PathBuf::from(&account.profile_path);
    }
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(default_dir)
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn json_at(path: &Path, pointer: &[&str]) -> Option<String> {
    let mut value = read_json(path)?;
    for key in pointer {
        value = value.get(*key)?.clone();
    }
    value.as_str().map(|text| text.to_string())
}

fn short_error(text: &str) -> String {
    let mut text = text.trim().to_string();
    if text.len() > 180 {
        text.truncate(180);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_scheme_rules() {
        assert!(allowed_endpoint("https://example.com/usage"));
        assert!(allowed_endpoint("http://127.0.0.1:9/usage"));
        assert!(!allowed_endpoint("http://example.com/usage"));
        assert!(!allowed_endpoint("file:///tmp/usage.json"));
    }
}
