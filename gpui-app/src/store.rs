use crate::model::{self, Account, Config};
use crate::providers;
use crate::usage::Reading;
use gpui::{AsyncApp, Entity};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Slot {
    reading: Reading,
    fetched_at: Option<Instant>,
}

pub struct Store {
    pub config: Config,
    pub storage_error: Option<String>,
    pub fullscreen: bool,
    slots: HashMap<String, Slot>,
}

impl Store {
    pub fn new(config: Config, storage_error: Option<String>) -> Self {
        Self {
            config,
            storage_error,
            fullscreen: false,
            slots: HashMap::new(),
        }
    }

    pub fn french(&self) -> bool {
        self.config.settings.french()
    }

    pub fn hidden(&self) -> bool {
        if !self.config.settings.widget {
            return true;
        }
        if self.config.settings.game_mode {
            return true;
        }
        self.config.settings.hide_in_fullscreen && self.fullscreen
    }

    pub fn reading(&self, id: &str) -> Option<&Reading> {
        self.slots.get(id).map(|slot| &slot.reading)
    }

    pub fn worst_percent(&self) -> Option<f64> {
        self.config
            .accounts
            .iter()
            .filter(|account| account.enabled)
            .filter_map(|account| self.reading(&account.id)?.headline())
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    }

    pub fn tooltip(&self) -> String {
        let mut lines = Vec::new();
        for account in self.config.accounts.iter().filter(|account| account.enabled) {
            match self.reading(&account.id) {
                Some(reading) if !reading.windows.is_empty() => {
                    let parts: Vec<String> = reading
                        .windows
                        .iter()
                        .map(|window| format!("{} {:.0}%", window.label, window.used_percent))
                        .collect();
                    lines.push(format!("{}: {}", account.name, parts.join(" · ")));
                }
                Some(reading) => {
                    let note = reading.note.as_deref().unwrap_or("—");
                    lines.push(format!("{}: {note}", account.name));
                }
                None => lines.push(format!("{}: …", account.name)),
            }
        }
        if lines.is_empty() {
            "CodeNotch".into()
        } else {
            lines.join("\n")
        }
    }

    pub fn due_accounts(&self, force: bool) -> Vec<Account> {
        let interval = Duration::from_secs(self.config.settings.poll_interval());
        self.config
            .accounts
            .iter()
            .filter(|account| account.enabled)
            .filter(|account| {
                let min = if account.provider.eq_ignore_ascii_case("claude") && account.endpoint.is_empty()
                {
                    Duration::from_secs(120)
                } else {
                    interval
                };
                match self.slots.get(&account.id).and_then(|slot| slot.fetched_at) {
                    Some(at) if !force && at.elapsed() < min => false,
                    Some(at) if force && account.provider.eq_ignore_ascii_case("claude") && at.elapsed() < Duration::from_secs(120) => false,
                    _ => true,
                }
            })
            .cloned()
            .collect()
    }

    pub fn apply(&mut self, id: &str, reading: Reading) {
        self.slots.insert(
            id.to_string(),
            Slot {
                reading,
                fetched_at: Some(Instant::now()),
            },
        );
    }

    pub fn save(&mut self) -> Result<(), String> {
        self.config.settings.onboarding_done = Some(
            self.config
                .settings
                .onboarding_done
                .unwrap_or(!self.config.accounts.is_empty()),
        );
        match model::save_config(&self.config) {
            Ok(()) => {
                self.storage_error = None;
                Ok(())
            }
            Err(err) => {
                self.storage_error = Some(err.clone());
                Err(err)
            }
        }
    }
}

pub async fn poll(store: &Entity<Store>, force: bool, cx: &mut AsyncApp) {
    let due = store
        .update(cx, |store, _| store.due_accounts(force))
        .unwrap_or_default();
    for account in due {
        let id = account.id.clone();
        let reading = cx
            .background_executor()
            .spawn(async move { providers::fetch(&account) })
            .await;
        let _ = store.update(cx, |store, cx| {
            store.apply(&id, reading);
            if store.config.settings.tray_shows_usage {
                crate::tray::set_tooltip(&store.tooltip());
            }
            cx.notify();
        });
    }
}

pub fn usage_color(percent: Option<f64>, warn: u8, critical: u8) -> gpui::Hsla {
    let Some(percent) = percent else {
        return gpui::rgb(0x8a8f98).into();
    };
    if percent >= f64::from(critical) {
        gpui::rgb(0xe15a4a).into()
    } else if percent >= f64::from(warn) {
        gpui::rgb(0xe6b15a).into()
    } else {
        gpui::rgb(0x3dbe7a).into()
    }
}

pub fn open_settings_if_needed(config: &Config, args: &[String]) -> bool {
    config.needs_welcome() || args.iter().any(|arg| arg == "--settings")
}
