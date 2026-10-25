use crate::{alerts, discord, login, model::*, providers, telegram};
use gpui::{AsyncApp, Entity, WindowHandle};
use std::{collections::HashMap, path::PathBuf, sync::OnceLock};

pub static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();

pub struct Store {
    pub config: Config,
    pub readings: HashMap<String, Reading>,
    pub refreshing: bool,
    pub storage_error: Option<String>,
    pub tracker: alerts::Tracker,
    pub selected: Option<String>,
    pub popover: Option<WindowHandle<crate::ui::Popover>>,
    pub settings_window: Option<WindowHandle<crate::settings::SettingsView>>,
    /// Folded to the peek tab; only ever true with auto-hide.
    pub collapsed: bool,
    /// Whether the pointer is over the rail and over the popover.
    pub hover: [bool; 2],
    /// Bumped on every hover change so a pending fold can tell it is outdated.
    pub fold_generation: u64,
    pub fullscreen: bool,
    /// A bot token / webhook is stored in Windows Credential Manager.
    pub telegram_saved: bool,
    pub discord_saved: bool,
}

impl Store {
    pub fn new(config: Config, storage_error: Option<String>) -> Self {
        let collapsed = config.settings.auto_hide;
        Self {
            config,
            readings: HashMap::new(),
            refreshing: false,
            storage_error,
            tracker: alerts::Tracker::default(),
            selected: None,
            popover: None,
            settings_window: None,
            collapsed,
            hover: [false; 2],
            fold_generation: 0,
            fullscreen: false,
            telegram_saved: telegram::saved_token().is_some(),
            discord_saved: discord::saved_webhook().is_some(),
        }
    }

    pub fn french(&self) -> bool {
        self.config.settings.language == "fr"
    }

    /// Game mode, or full screen when the setting asks for it, hides the notch entirely.
    pub fn visible(&self) -> bool {
        let s = &self.config.settings;
        s.widget && !s.game_mode && !(s.hide_in_fullscreen && self.fullscreen)
    }

    pub fn enabled(&self) -> Vec<Account> {
        self.config
            .accounts
            .iter()
            .filter(|a| a.enabled)
            .cloned()
            .collect()
    }

    /// Same scheduling rules as the Tauri backend: provider backoff always wins,
    /// Claude is never read more than every two minutes.
    fn due(&self, force: bool, now: i64) -> Vec<Account> {
        let settings = &self.config.settings;
        self.enabled()
            .into_iter()
            .filter(|a| match self.readings.get(&a.id) {
                None => true,
                Some(r) => {
                    if r.retry_at.is_some_and(|at| at > now) {
                        return false;
                    }
                    let floor = if a.provider == "claude" { 120 } else { 10 };
                    let interval = if force {
                        floor
                    } else if alerts::Tracker::near_limit(r, settings, now) {
                        floor.max(30)
                    } else {
                        settings.poll_seconds.max(floor)
                    } as i64;
                    r.updated_at.is_none_or(|at| now - at >= interval)
                }
            })
            .collect()
    }

    /// Validates and saves new settings, as `save_settings` does in the Tauri backend.
    pub fn save_settings(&mut self, mut settings: Settings) -> Result<(), String> {
        if !["en", "fr"].contains(&settings.language.as_str())
            || !(50..=100).contains(&settings.alert_percent)
            || !(50..=100).contains(&settings.critical_percent)
            || ![15, 30, 60].contains(&settings.forecast_minutes)
        {
            return Err("Invalid settings.".into());
        }
        settings.telegram_chat_id = settings.telegram_chat_id.trim().to_owned();
        if !settings.telegram_chat_id.is_empty()
            && !telegram::valid_chat_id(&settings.telegram_chat_id)
        {
            return Err("Identifiant de chat Telegram invalide : un nombre (ex. 123456789 ou -100…) ou @nom_du_canal.".into());
        }
        let mut next = self.config.clone();
        next.settings = settings;
        self.commit(next)
    }

    /// Adds or updates an account, as `save_account` does in the Tauri backend: the secret
    /// goes to Windows Credential Manager, never to the configuration file.
    pub fn save_account(
        &mut self,
        mut account: Account,
        secret: Option<String>,
        clear_secret: bool,
    ) -> Result<(), String> {
        validate_account(&account)?;
        if account.id.is_empty() {
            account.id = uuid::Uuid::new_v4().to_string();
        } else if !self.config.accounts.iter().any(|a| a.id == account.id) {
            return Err("Ce compte n’existe plus.".into());
        }
        if self.config.accounts.len() >= 20
            && !self.config.accounts.iter().any(|a| a.id == account.id)
        {
            return Err("Maximum 20 comptes.".into());
        }
        account.has_secret = self
            .config
            .accounts
            .iter()
            .find(|a| a.id == account.id)
            .is_some_and(|a| a.has_secret);
        if clear_secret {
            if account.has_secret {
                keyring::Entry::new("SmashToken", &account.id)
                    .and_then(|e| e.delete_credential())
                    .map_err(|_| "Impossible d’effacer le secret du coffre Windows.")?;
            }
            account.has_secret = false;
        }
        if let Some(secret) = secret.filter(|s| !s.is_empty()) {
            if secret.len() > 8192 {
                return Err("Secret trop long.".into());
            }
            keyring::Entry::new("SmashToken", &account.id)
                .and_then(|e| e.set_password(&secret))
                .map_err(|_| "Impossible d’enregistrer le secret dans le coffre Windows.")?;
            account.has_secret = true;
        }
        let mut next = self.config.clone();
        match next.accounts.iter_mut().find(|a| a.id == account.id) {
            Some(a) => *a = account.clone(),
            None => next.accounts.push(account.clone()),
        }
        self.commit(next)?;
        self.readings.remove(&account.id);
        Ok(())
    }

    /// After a CLI sign-in: track the new profile and drop the failed readings and their
    /// backoff so the next poll reads the provider again.
    pub fn finish_login(&mut self, provider: &str) -> Result<(), String> {
        self.add_discovered()?;
        let ids: Vec<_> = self
            .config
            .accounts
            .iter()
            .filter(|a| a.provider == provider)
            .map(|a| a.id.clone())
            .collect();
        for id in ids {
            self.readings.remove(&id);
        }
        Ok(())
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        let mut next = self.config.clone();
        let account = next
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or("Account not found.")?;
        account.enabled = enabled;
        self.commit(next)
    }

    pub fn remove_account(&mut self, id: &str) -> Result<(), String> {
        let account = self
            .config
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found.")?;
        if account.has_secret {
            keyring::Entry::new("SmashToken", id)
                .and_then(|e| e.delete_credential())
                .map_err(|_| "Cannot erase the saved secret. Try again.")?;
        }
        let mut next = self.config.clone();
        next.accounts.retain(|a| a.id != id);
        self.commit(next)?;
        self.readings.remove(id);
        Ok(())
    }

    /// Adds the local CLI profiles that are not tracked yet; returns how many.
    pub fn add_discovered(&mut self) -> Result<usize, String> {
        let mut next = self.config.clone();
        let before = next.accounts.len();
        for account in discover() {
            if next.accounts.len() < 20
                && !next.accounts.iter().any(|a| {
                    a.provider == account.provider && profile_key(a) == profile_key(&account)
                })
            {
                next.accounts.push(account);
            }
        }
        let added = next.accounts.len() - before;
        if added > 0 {
            self.commit(next)?;
        }
        Ok(added)
    }

    fn commit(&mut self, next: Config) -> Result<(), String> {
        if let Some(error) = &self.storage_error {
            return Err(error.clone());
        }
        save(&next)?;
        self.config = next;
        Ok(())
    }

    fn apply(&mut self, account: Account, result: Result<Reading, providers::Failure>) {
        // Removed while its reading was in flight.
        if !self.config.accounts.iter().any(|a| a.id == account.id) {
            return;
        }
        let at = chrono::Utc::now().timestamp();
        let previous = self.readings.get(&account.id).cloned();
        let reading = match result {
            Ok(mut r) => {
                r.account_id = account.id.clone();
                r.status = "ok".into();
                r.updated_at = Some(at);
                // Alerts are not delivered yet; observing still fills the forecasts.
                let _ = self
                    .tracker
                    .observe(&account, &mut r, &self.config.settings, at);
                r
            }
            Err(f) => {
                let mut r = previous.unwrap_or_default();
                r.account_id = account.id.clone();
                r.status = if r.updated_at.is_some() && f.status == "error" {
                    "stale".into()
                } else {
                    f.status
                };
                r.message = f.message;
                r.retry_at = Some(at + f.retry_seconds as i64);
                r
            }
        };
        self.readings.insert(account.id, reading);
    }
}

pub async fn poll(store: &Entity<Store>, force: bool, cx: &mut AsyncApp) {
    let now = chrono::Utc::now().timestamp();
    let Ok(accounts) = store.update(cx, |s, cx| {
        if s.refreshing {
            return vec![];
        }
        let due = s.due(force, now);
        if !due.is_empty() {
            s.refreshing = true;
            cx.notify();
        }
        due
    }) else {
        return;
    };
    if accounts.is_empty() {
        return;
    }
    let runtime = RUNTIME.get().expect("tokio runtime");
    let jobs: Vec<_> = accounts
        .into_iter()
        .map(|account| {
            runtime.spawn(async move {
                let result = providers::fetch(&account).await;
                (account, result)
            })
        })
        .collect();
    for job in jobs {
        if let Ok((account, result)) = job.await {
            let _ = store.update(cx, |s, cx| {
                s.apply(account, result);
                cx.notify();
            });
        }
    }
    let _ = store.update(cx, |s, cx| {
        s.refreshing = false;
        cx.notify();
    });
}

/// The Tauri app's configuration file, read but never written so both builds can coexist.
fn tauri_config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("dev.kev.smashtoken").join("config.json"))
}

/// The prototype saves next to the Tauri configuration, in its own file, so the two
/// builds never overwrite each other. It starts as a copy of the Tauri one.
pub fn config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("dev.kev.smashtoken").join("config.gpui.json"))
}

pub fn save(config: &Config) -> Result<(), String> {
    let path = config_path().ok_or("No configuration folder on this PC.")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|_| "Cannot create the configuration folder.")?;
    }
    let bytes = serde_json::to_vec_pretty(config).map_err(|_| "Invalid configuration.")?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes).map_err(|_| "Cannot write the local configuration.")?;
    std::fs::rename(temporary, path).map_err(|_| "Cannot replace the local configuration.".to_owned())
}

pub fn load() -> (Config, Option<String>) {
    let Some(path) = config_path()
        .filter(|p| p.exists())
        .or_else(|| tauri_config_path().filter(|p| p.exists()))
    else {
        return (
            Config {
                accounts: discover(),
                ..Default::default()
            },
            None,
        );
    };
    match std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Config>(&b).ok())
    {
        Some(c) => (c, None),
        None => (
            Config::default(),
            Some(format!("Unreadable configuration: {}", path.display())),
        ),
    }
}

/// Identifies a profile folder, treating an empty path as the CLI's default one.
fn profile_key(account: &Account) -> Option<String> {
    login::PROVIDERS
        .contains(&account.provider.as_str())
        .then(|| {
            let path = if account.profile_path.is_empty() {
                login::default_profile(&account.provider)
            } else {
                PathBuf::from(&account.profile_path)
            };
            let key = path.to_string_lossy().replace('/', "\\");
            key.trim_end_matches('\\').to_lowercase()
        })
}

/// Local CLI profiles, as the Tauri app finds them on first launch.
fn discover() -> Vec<Account> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let mut candidates: Vec<(&str, PathBuf)> = std::fs::read_dir(&home)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let provider = if name == ".codex" || name.starts_with(".codex-") {
                "codex"
            } else if name == ".claude" || name.starts_with(".claude-") {
                "claude"
            } else if name == ".gemini" {
                "gemini"
            } else if name == ".grok" {
                "grok"
            } else {
                return None;
            };
            Some((provider, entry.path()))
        })
        .collect();
    for provider in login::PROVIDERS {
        let path = login::default_profile(provider);
        if !candidates.iter().any(|(_, p)| *p == path) {
            candidates.push((provider, path));
        }
    }
    let mut profiles = vec![];
    for (provider, path) in candidates {
        if !path.is_dir() || !path.join(login::credential_file(provider)).is_file() {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let suffix = name
            .strip_prefix(&format!(".{provider}"))
            .unwrap_or("")
            .trim_start_matches('-');
        let title = crate::ui::provider_title(provider);
        profiles.push(Account {
            id: uuid::Uuid::new_v4().to_string(),
            name: if suffix.is_empty() {
                format!("{title} · personnel")
            } else {
                format!("{title} · {suffix}")
            },
            provider: provider.into(),
            enabled: true,
            profile_path: path.to_string_lossy().to_string(),
            endpoint: String::new(),
            secret_env: String::new(),
            has_secret: false,
        });
    }
    profiles.sort_by(|a, b| a.profile_path.cmp(&b.profile_path));
    profiles
}
