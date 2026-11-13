//! Secrets (Telegram bot token, Discord webhook URL, custom-endpoint tokens, Teams device
//! token).
//!
//! Never stored in `config.gpui.json`, `state.json`, history, snapshots or logs. The real store
//! is the Windows Credential Manager (`codenotch_platform::secrets::CredentialStore`, generic
//! credentials named by [`SecretKey::target_name`]). Values only ever travel inside
//! [`SecretValue`], whose `Debug` prints `***`.
//!
//! Lookup order ([`resolve_secret`]): environment variable first (fixed name for Telegram /
//! Discord, the variable NAMED by `Account::secret_env` for an account), then the store. The
//! Teams device token has no environment variable: it lives in the store only.

use crate::config::Config;
use crate::provider::Env;

/// Env var holding the Telegram bot token.
pub const TELEGRAM_TOKEN_ENV: &str = "CODENOTCH_TELEGRAM_TOKEN";
/// Env var holding the Discord webhook URL.
pub const DISCORD_WEBHOOK_ENV: &str = "CODENOTCH_DISCORD_WEBHOOK";
/// Prefix of every Credential Manager target name.
pub const TARGET_PREFIX: &str = "CodeNotch/";

/// Which secret. The key itself is not secret (safe to log).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SecretKey {
    TelegramToken,
    DiscordWebhook,
    /// Bearer token of one account (custom endpoint, API-key providers), by account id.
    Account(String),
    /// Device token issued by a CodeNotch Teams server at pairing. Credential Manager only.
    TeamsDeviceToken,
}

impl SecretKey {
    /// Credential Manager target: `CodeNotch/telegram-token`, `CodeNotch/discord-webhook`,
    /// `CodeNotch/account/<id>`, `CodeNotch/teams-device-token`.
    pub fn target_name(&self) -> String {
        match self {
            SecretKey::TelegramToken => format!("{TARGET_PREFIX}telegram-token"),
            SecretKey::DiscordWebhook => format!("{TARGET_PREFIX}discord-webhook"),
            SecretKey::Account(id) => format!("{TARGET_PREFIX}account/{id}"),
            SecretKey::TeamsDeviceToken => format!("{TARGET_PREFIX}teams-device-token"),
        }
    }

    /// Fixed environment variable for the Telegram and Discord secrets; `None` for accounts
    /// (their variable name comes from `Account::secret_env`, see [`resolve_secret`]) and for
    /// the Teams device token (store only: an environment variable could leak it to child
    /// processes and is not tied to the pairing).
    pub fn env_var(&self) -> Option<&'static str> {
        match self {
            SecretKey::TelegramToken => Some(TELEGRAM_TOKEN_ENV),
            SecretKey::DiscordWebhook => Some(DISCORD_WEBHOOK_ENV),
            SecretKey::Account(_) | SecretKey::TeamsDeviceToken => None,
        }
    }
}

/// A secret value in transit (e.g. `Command::SetSecret`). `Debug` never shows the content.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    /// The secret itself. Do not log or display it.
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() {
            "SecretValue(empty)"
        } else {
            "SecretValue(***)"
        })
    }
}

impl From<&str> for SecretValue {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Persistent secret storage. Implementations must be thread-safe, must not log values, and
/// must treat "not found" as `None` (not an error).
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &SecretKey) -> Option<String>;
    fn set(&self, key: &SecretKey, value: &str) -> crate::Result<()>;
    /// Deleting a missing secret is `Ok`.
    fn delete(&self, key: &SecretKey) -> crate::Result<()>;
}

/// In-memory store (tests, `--demo`).
#[derive(Default)]
pub struct MemorySecrets {
    map: std::sync::Mutex<std::collections::BTreeMap<SecretKey, String>>,
}

impl MemorySecrets {
    pub fn new() -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for MemorySecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let keys: Vec<SecretKey> = self
            .map
            .lock()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        f.debug_struct("MemorySecrets")
            .field("keys", &keys)
            .finish()
    }
}

impl SecretStore for MemorySecrets {
    fn get(&self, key: &SecretKey) -> Option<String> {
        self.map.lock().ok()?.get(key).cloned()
    }
    fn set(&self, key: &SecretKey, value: &str) -> crate::Result<()> {
        self.map
            .lock()
            .map_err(|_| crate::Error::other("secret store poisoned"))?
            .insert(key.clone(), value.to_string());
        Ok(())
    }
    fn delete(&self, key: &SecretKey) -> crate::Result<()> {
        self.map
            .lock()
            .map_err(|_| crate::Error::other("secret store poisoned"))?
            .remove(key);
        Ok(())
    }
}

/// Store that holds nothing and refuses writes (headless modes without a real store).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoSecrets;

impl SecretStore for NoSecrets {
    fn get(&self, _key: &SecretKey) -> Option<String> {
        None
    }
    fn set(&self, _key: &SecretKey, _value: &str) -> crate::Result<()> {
        Err(crate::Error::unsupported("no secret store"))
    }
    fn delete(&self, _key: &SecretKey) -> crate::Result<()> {
        Ok(())
    }
}

/// Resolve a secret: env var first (fixed name, or for `Account(id)` the variable named by that
/// account's `secret_env` in `config`, when given), then `store`. Empty values count as missing.
pub fn resolve_secret(key: &SecretKey, env: &Env, store: &dyn SecretStore) -> Option<String> {
    resolve_secret_with(key, None, env, store)
}

/// [`resolve_secret`] with the config, so an account's `secretEnv` variable is consulted.
pub fn resolve_secret_with(
    key: &SecretKey,
    config: Option<&Config>,
    env: &Env,
    store: &dyn SecretStore,
) -> Option<String> {
    let var_name: Option<String> = match key {
        SecretKey::Account(id) => config
            .and_then(|c| c.account(id))
            .map(|a| a.secret_env.clone())
            .filter(|n| !n.is_empty()),
        other => other.env_var().map(str::to_string),
    };
    if let Some(v) = var_name.and_then(|n| env.var(&n)) {
        return Some(v);
    }
    store.get(key).filter(|v| !v.is_empty())
}

/// Which app-wide secrets are available (presence only, never values). Part of `Snapshot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SecretsPresent {
    pub telegram_token: bool,
    pub discord_webhook: bool,
}

impl SecretsPresent {
    pub fn detect(env: &Env, store: &dyn SecretStore) -> Self {
        Self {
            telegram_token: resolve_secret(&SecretKey::TelegramToken, env, store).is_some(),
            discord_webhook: resolve_secret(&SecretKey::DiscordWebhook, env, store).is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Account;
    use crate::model::ProviderKind;
    use std::path::Path;

    #[test]
    fn target_names_and_env_vars() {
        assert_eq!(
            SecretKey::TelegramToken.target_name(),
            "CodeNotch/telegram-token"
        );
        assert_eq!(
            SecretKey::DiscordWebhook.target_name(),
            "CodeNotch/discord-webhook"
        );
        assert_eq!(
            SecretKey::Account("a1".into()).target_name(),
            "CodeNotch/account/a1"
        );
        assert_eq!(
            SecretKey::TelegramToken.env_var(),
            Some("CODENOTCH_TELEGRAM_TOKEN")
        );
        assert_eq!(SecretKey::Account("a".into()).env_var(), None);
        assert_eq!(
            SecretKey::TeamsDeviceToken.target_name(),
            "CodeNotch/teams-device-token"
        );
        assert_eq!(SecretKey::TeamsDeviceToken.env_var(), None);
    }

    #[test]
    fn teams_device_token_comes_from_the_store_only() {
        let store = MemorySecrets::new();
        let env = Env::for_test(Path::new("."))
            .with_var("CODENOTCH_TEAMS_DEVICE_TOKEN", "from-env")
            .with_var("CODENOTCH_TEAMS_TOKEN", "from-env");
        let key = SecretKey::TeamsDeviceToken;
        assert_eq!(resolve_secret(&key, &env, &store), None);
        store.set(&key, "cnt_s3cr3t").unwrap();
        assert_eq!(
            resolve_secret(&key, &env, &store).as_deref(),
            Some("cnt_s3cr3t")
        );
        assert!(!format!("{store:?}").contains("s3cr3t"));
        let cmd = crate::Command::TeamsPair(SecretValue::new("https://x/invite/s3cr3t"));
        assert!(!format!("{cmd:?}").contains("s3cr3t"));
    }

    #[test]
    fn env_wins_over_store_then_store() {
        let store = MemorySecrets::new();
        store.set(&SecretKey::TelegramToken, "from-store").unwrap();
        let env = Env::for_test(Path::new("."));
        assert_eq!(
            resolve_secret(&SecretKey::TelegramToken, &env, &store).as_deref(),
            Some("from-store")
        );
        let env = env.with_var(TELEGRAM_TOKEN_ENV, "from-env");
        assert_eq!(
            resolve_secret(&SecretKey::TelegramToken, &env, &store).as_deref(),
            Some("from-env")
        );
        store.delete(&SecretKey::TelegramToken).unwrap();
        store.delete(&SecretKey::TelegramToken).unwrap();
        assert_eq!(
            resolve_secret(&SecretKey::DiscordWebhook, &env, &store),
            None
        );
        assert!(SecretsPresent::detect(&env, &store).telegram_token);
        assert!(!SecretsPresent::detect(&env, &store).discord_webhook);
    }

    #[test]
    fn account_uses_its_secret_env_then_store() {
        let mut config = Config::default();
        let mut a = Account::new(ProviderKind::Custom, "c");
        a.secret_env = "MY_PROXY_TOKEN".into();
        config.accounts.push(a.clone());
        let key = SecretKey::Account(a.id.clone());
        let store = MemorySecrets::new();
        store.set(&key, "stored").unwrap();
        let env = Env::for_test(Path::new("."));
        assert_eq!(
            resolve_secret_with(&key, Some(&config), &env, &store).as_deref(),
            Some("stored")
        );
        let env = env.with_var("MY_PROXY_TOKEN", "envtok");
        assert_eq!(
            resolve_secret_with(&key, Some(&config), &env, &store).as_deref(),
            Some("envtok")
        );
    }

    #[test]
    fn values_never_in_debug() {
        let v = SecretValue::new("s3cr3t-value");
        assert!(!format!("{v:?}").contains("s3cr3t"));
        let store = MemorySecrets::new();
        store
            .set(&SecretKey::DiscordWebhook, "https://discord/hook/s3cr3t")
            .unwrap();
        assert!(!format!("{store:?}").contains("s3cr3t"));
        let cmd = crate::Command::SetSecret {
            key: SecretKey::TelegramToken,
            value: v,
        };
        assert!(!format!("{cmd:?}").contains("s3cr3t"));
        assert!(NoSecrets.set(&SecretKey::TelegramToken, "x").is_err());
    }
}
