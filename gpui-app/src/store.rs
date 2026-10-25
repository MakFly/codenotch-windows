use crate::{alerts, login, model::*, providers};
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
}

impl Store {
    pub fn new(config: Config, storage_error: Option<String>) -> Self {
        Self {
            config,
            readings: HashMap::new(),
            refreshing: false,
            storage_error,
            tracker: alerts::Tracker::default(),
            selected: None,
            popover: None,
        }
    }

    pub fn french(&self) -> bool {
        self.config.settings.language == "fr"
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

    fn apply(&mut self, account: Account, result: Result<Reading, providers::Failure>) {
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
fn config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("dev.kev.smashtoken").join("config.json"))
}

pub fn load() -> (Config, Option<String>) {
    let Some(path) = config_path().filter(|p| p.exists()) else {
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
