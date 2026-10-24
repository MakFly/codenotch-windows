mod claude;
mod codex;
mod dashboard;
mod model;
mod providers;
mod widget;

use model::*;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Mutex;

struct Data {
    config: Config,
    readings: HashMap<String, Reading>,
    revision: u64,
    storage_error: Option<String>,
}
struct Backend {
    data: Mutex<Data>,
    polling: Mutex<()>,
    path: PathBuf,
    dragging: AtomicBool,
    fullscreen: AtomicBool,
    widget_mode: AtomicU8,
}
type Shared = Arc<Backend>;
struct GameMenu(tauri::menu::CheckMenuItem<tauri::Wry>);
struct MenuLabels {
    show: tauri::menu::MenuItem<tauri::Wry>,
    quit: tauri::menu::MenuItem<tauri::Wry>,
}
fn sync_language(app: &tauri::AppHandle, language: &str) {
    let french = language == "fr";
    if let Some(menu) = app.try_state::<MenuLabels>() {
        let _ = menu.show.set_text(if french {
            "Réglages CodeNotch"
        } else {
            "CodeNotch settings"
        });
        let _ = menu.quit.set_text(if french { "Quitter" } else { "Quit" });
    }
    if let Some(menu) = app.try_state::<GameMenu>() {
        let _ = menu.0.set_text(if french {
            "Mode jeu — masquer l’encoche"
        } else {
            "Game mode — hide notch"
        });
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title(if french {
            "CodeNotch — Réglages"
        } else {
            "CodeNotch — Settings"
        });
    }
}

fn suppress_widget(app: &tauri::AppHandle, settings: &Settings) -> bool {
    let state = app.try_state::<Shared>();
    let fullscreen = widget::fullscreen_active().unwrap_or_else(|| {
        state
            .as_ref()
            .is_some_and(|s| s.fullscreen.load(Ordering::Relaxed))
    });
    if let Some(state) = state {
        state.fullscreen.store(fullscreen, Ordering::Relaxed);
    }
    settings.game_mode || (settings.hide_in_fullscreen && fullscreen)
}

fn reconcile_visibility(app: &tauri::AppHandle, settings: &Settings) {
    if let Some(window) = app.get_webview_window("widget") {
        let show = settings.widget && !suppress_widget(app, settings);
        if window.is_visible().ok() != Some(show) {
            if show {
                let _ = window.show();
            } else {
                let _ = window.hide();
            }
        }
    }
}

fn ensure_writable(data: &Data) -> Result<(), String> {
    if data.storage_error.is_some() {
        Err("Corrige le fichier de configuration signalé avant d’enregistrer.".into())
    } else {
        Ok(())
    }
}

fn snapshot(data: &Data, refreshing: bool) -> Snapshot {
    Snapshot {
        config: data.config.clone(),
        readings: data.readings.values().cloned().collect(),
        refreshing,
        storage_error: data.storage_error.clone(),
    }
}

fn save(path: &PathBuf, config: &Config) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(config).map_err(|_| "Configuration invalide.")?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)
        .map_err(|_| "Impossible d’écrire la configuration locale.")?;
    std::fs::rename(temporary, path)
        .map_err(|_| "Impossible de remplacer la configuration locale.".to_owned())
}

fn discover() -> Vec<Account> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let mut profiles = vec![];
    if let Ok(entries) = std::fs::read_dir(&home) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let provider = if name == ".codex" || name.starts_with(".codex-") {
                "codex"
            } else if name == ".claude" || name.starts_with(".claude-") {
                "claude"
            } else {
                continue;
            };
            if !entry.path().is_dir() {
                continue;
            }
            let auth = if provider == "codex" {
                "auth.json"
            } else {
                ".credentials.json"
            };
            if !entry.path().join(auth).is_file() {
                continue;
            }
            let suffix = name
                .trim_start_matches(&format!(".{provider}"))
                .trim_start_matches('-');
            let title = if provider == "codex" {
                "Codex"
            } else {
                "Claude"
            };
            profiles.push(Account {
                id: uuid::Uuid::new_v4().to_string(),
                name: if suffix.is_empty() {
                    format!("{title} · personnel")
                } else {
                    format!("{title} · {suffix}")
                },
                provider: provider.into(),
                enabled: true,
                profile_path: entry.path().to_string_lossy().to_string(),
                endpoint: String::new(),
                secret_env: String::new(),
                has_secret: false,
            });
        }
    }
    profiles.sort_by(|a, b| a.profile_path.cmp(&b.profile_path));
    profiles
}

async fn poll(app: &tauri::AppHandle, state: &Shared, force: bool) {
    let Ok(_guard) = state.polling.try_lock() else {
        return;
    };
    let now = chrono::Utc::now().timestamp();
    let (accounts, revision, settings) = {
        let data = state.data.lock().await;
        let accounts: Vec<_> = data
            .config
            .accounts
            .iter()
            .filter(|a| {
                if !a.enabled {
                    return false;
                }
                match data.readings.get(&a.id) {
                    None => true,
                    Some(r) => {
                        // Manual refresh never bypasses provider backoff or Claude's minimum interval.
                        if r.retry_at.is_some_and(|at| at > now) {
                            return false;
                        }
                        let floor = if a.provider == "claude" { 120 } else { 10 };
                        let interval = if force {
                            floor
                        } else {
                            data.config.settings.poll_seconds.max(floor)
                        } as i64;
                        r.updated_at.is_none_or(|at| now - at >= interval)
                    }
                }
            })
            .cloned()
            .collect();
        if accounts.is_empty() {
            return;
        }
        let _ = app.emit("snapshot", snapshot(&data, true));
        (accounts, data.revision, data.config.settings.clone())
    };
    let mut jobs = tokio::task::JoinSet::new();
    for account in accounts {
        jobs.spawn(async move {
            let result = providers::fetch(&account).await;
            (account, result)
        });
    }
    while let Some(Ok((account, result))) = jobs.join_next().await {
        let at = chrono::Utc::now().timestamp();
        let mut data = state.data.lock().await;
        if data.revision != revision {
            continue;
        }
        let previous = data.readings.get(&account.id).cloned();
        let reading = match result {
            Ok(mut r) => {
                r.account_id = account.id.clone();
                r.status = "ok".into();
                r.updated_at = Some(at);
                if settings.notifications && !suppress_widget(app, &data.config.settings) {
                    if let Some(old) = previous.as_ref().filter(|old| old.updated_at.is_some()) {
                        for w in &r.windows {
                            if let Some(before) = old
                                .windows
                                .iter()
                                .find(|old| old.label == w.label && old.resets_at == w.resets_at)
                            {
                                let threshold = f64::from(settings.alert_percent);
                                if before.used_percent < threshold && w.used_percent >= threshold {
                                    let _ = app
                                        .notification()
                                        .builder()
                                        .title(if data.config.settings.language == "fr" {
                                            format!(
                                                "{} · quota à {:.0} %",
                                                account.name, w.used_percent
                                            )
                                        } else {
                                            format!(
                                                "{} · quota at {:.0}%",
                                                account.name, w.used_percent
                                            )
                                        })
                                        .body(if data.config.settings.language == "fr" {
                                            format!(
                                                "{} : seuil de {} % atteint.",
                                                w.label, settings.alert_percent
                                            )
                                        } else {
                                            format!(
                                                "{}: {}% threshold reached.",
                                                w.label.replace("jours", "days").replace(
                                                    "Requêtes premium",
                                                    "Premium requests"
                                                ),
                                                settings.alert_percent
                                            )
                                        })
                                        .show();
                                }
                            }
                        }
                    }
                }
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
        data.readings.insert(account.id, reading);
        let _ = app.emit("snapshot", snapshot(&data, true));
    }
    let data = state.data.lock().await;
    let _ = app.emit("snapshot", snapshot(&data, false));
}

#[tauri::command]
async fn get_snapshot(state: tauri::State<'_, Shared>) -> Result<Snapshot, String> {
    let data = state.data.lock().await;
    Ok(snapshot(&data, state.polling.try_lock().is_err()))
}

#[tauri::command]
async fn refresh(app: tauri::AppHandle, state: tauri::State<'_, Shared>) -> Result<(), String> {
    poll(&app, &state, true).await;
    Ok(())
}

#[tauri::command]
async fn save_account(
    app: tauri::AppHandle,
    state: tauri::State<'_, Shared>,
    mut account: Account,
    secret: Option<String>,
    clear_secret: bool,
) -> Result<Snapshot, String> {
    validate_account(&account)?;
    let mut data = state.data.lock().await;
    ensure_writable(&data)?;
    if account.id.is_empty() {
        account.id = uuid::Uuid::new_v4().to_string();
    } else if !data.config.accounts.iter().any(|a| a.id == account.id) {
        return Err("Ce compte n’existe plus.".into());
    }
    if data.config.accounts.len() >= 20 && !data.config.accounts.iter().any(|a| a.id == account.id)
    {
        return Err("Maximum 20 comptes.".into());
    }
    account.has_secret = data
        .config
        .accounts
        .iter()
        .find(|a| a.id == account.id)
        .is_some_and(|a| a.has_secret);
    if clear_secret {
        if account.has_secret {
            keyring::Entry::new("SmashToken", &account.id)
                .map_err(|_| "Coffre Windows indisponible.")?
                .delete_credential()
                .map_err(|_| "Impossible d’effacer le secret du coffre Windows.")?;
        }
        account.has_secret = false;
    }
    if let Some(secret) = secret.filter(|s| !s.is_empty()) {
        if secret.len() > 8192 {
            return Err("Secret trop long.".into());
        }
        keyring::Entry::new("SmashToken", &account.id)
            .map_err(|_| "Coffre Windows indisponible.")?
            .set_password(&secret)
            .map_err(|_| "Impossible d’enregistrer le secret dans le coffre Windows.")?;
        account.has_secret = true;
    }
    let mut next = data.config.clone();
    if let Some(a) = next.accounts.iter_mut().find(|a| a.id == account.id) {
        *a = account.clone();
    } else {
        next.accounts.push(account.clone());
    }
    save(&state.path, &next)?;
    data.config = next;
    data.readings.remove(&account.id);
    data.revision += 1;
    let result = snapshot(&data, false);
    let _ = app.emit("snapshot", &result);
    Ok(result)
}

#[tauri::command]
async fn remove_account(
    app: tauri::AppHandle,
    state: tauri::State<'_, Shared>,
    id: String,
) -> Result<Snapshot, String> {
    let mut data = state.data.lock().await;
    ensure_writable(&data)?;
    let account = data
        .config
        .accounts
        .iter()
        .find(|a| a.id == id)
        .ok_or("Compte introuvable.")?;
    if account.has_secret {
        keyring::Entry::new("SmashToken", &id)
            .map_err(|_| "Coffre Windows indisponible.")?
            .delete_credential()
            .map_err(|_| "Impossible d’effacer le secret. Réessaie.")?;
    }
    let mut next = data.config.clone();
    next.accounts.retain(|a| a.id != id);
    save(&state.path, &next)?;
    data.config = next;
    data.readings.remove(&id);
    data.revision += 1;
    let result = snapshot(&data, false);
    let _ = app.emit("snapshot", &result);
    Ok(result)
}

#[tauri::command]
async fn discover_accounts(
    app: tauri::AppHandle,
    state: tauri::State<'_, Shared>,
) -> Result<Snapshot, String> {
    let found = discover();
    let mut data = state.data.lock().await;
    ensure_writable(&data)?;
    let mut next = data.config.clone();
    for account in found {
        if next.accounts.len() < 20
            && !next
                .accounts
                .iter()
                .any(|a| a.provider == account.provider && a.profile_path == account.profile_path)
        {
            next.accounts.push(account);
        }
    }
    save(&state.path, &next)?;
    data.config = next;
    data.revision += 1;
    let result = snapshot(&data, false);
    let _ = app.emit("snapshot", &result);
    Ok(result)
}

fn apply_widget(
    app: &tauri::AppHandle,
    settings: &Settings,
    count: usize,
    expanded: bool,
    collapsed: bool,
) -> Result<(), String> {
    let window = app
        .get_webview_window("widget")
        .ok_or("Widget indisponible.")?;
    if let Some(state) = app.try_state::<Shared>() {
        state.widget_mode.store(
            if collapsed {
                0
            } else if expanded {
                2
            } else {
                1
            },
            Ordering::Relaxed,
        );
        if state.dragging.load(Ordering::Relaxed) {
            return Ok(());
        }
    }
    if !settings.widget {
        return window
            .hide()
            .map_err(|_| "Impossible de masquer le widget.".into());
    }
    let saved_monitor = settings
        .widget_placement
        .as_ref()
        .and_then(|p| p.monitor.as_ref())
        .and_then(|name| {
            window
                .available_monitors()
                .ok()?
                .into_iter()
                .find(|m| m.name() == Some(name))
        });
    let monitor = saved_monitor
        .or_else(|| {
            if settings.widget_placement.is_some() {
                window.primary_monitor().ok().flatten()
            } else {
                window.current_monitor().ok().flatten()
            }
        })
        .or_else(|| window.primary_monitor().ok().flatten());
    if let Some(monitor) = monitor {
        let size = monitor.size();
        let origin = monitor.position();
        let scale = monitor.scale_factor();
        let vertical = matches!(settings.widget_position.as_str(), "left" | "right");
        let cells = count.clamp(1, 6) as f64;
        let (width, height) = if collapsed && settings.auto_hide {
            if vertical {
                (24.0, 96.0)
            } else {
                (96.0, 24.0)
            }
        } else if vertical {
            (
                if expanded { 420.0 } else { 74.0 },
                (cells * 94.0 + 136.0)
                    .max(if expanded { 440.0 } else { 0.0 })
                    .min(size.height as f64 / scale - 60.0),
            )
        } else {
            (
                // Include the full tool strip, notably the drag handle.
                (cells * 73.0 + 122.0)
                    .max(if expanded { 360.0 } else { 0.0 })
                    .min(size.width as f64 / scale),
                if expanded { 530.0 } else { 100.0 },
            )
        };
        let visible = ((width * scale) as i32, (height * scale) as i32);
        let (width, height) = if cfg!(windows) {
            widget::surface_size(
                &settings.widget_position,
                count,
                (size.width as f64 / scale, size.height as f64 / scale),
            )
        } else {
            (width, height)
        };
        let physical_width = (width * scale) as i32;
        let physical_height = (height * scale) as i32;
        let (x, y) = if let Some(saved) = &settings.widget_placement {
            widget::restore(
                &settings.widget_position,
                saved,
                (physical_width as u32, physical_height as u32),
                (origin.x, origin.y),
                (size.width, size.height),
            )
        } else {
            match settings.widget_position.as_str() {
                "left" => (
                    origin.x,
                    origin.y + (size.height as i32 - physical_height) / 2,
                ),
                "right" => (
                    origin.x + size.width as i32 - physical_width,
                    origin.y + (size.height as i32 - physical_height) / 2,
                ),
                "bottom" => (
                    origin.x + (size.width as i32 - physical_width) / 2,
                    origin.y + size.height as i32 - physical_height - (48.0 * scale) as i32,
                ),
                _ => (
                    origin.x + (size.width as i32 - physical_width) / 2,
                    origin.y,
                ),
            }
        };
        widget::set_bounds(&window, x, y, physical_width, physical_height)?;
        widget::set_input_region(
            &window,
            widget::visible_region(
                &settings.widget_position,
                (physical_width, physical_height),
                visible,
            ),
        )?;
    }
    reconcile_visibility(app, settings);
    Ok(())
}

#[tauri::command]
async fn drag_notch(app: tauri::AppHandle, state: tauri::State<'_, Shared>) -> Result<(), String> {
    if state.dragging.swap(true, Ordering::Relaxed) {
        return Ok(());
    }
    let result = async {
        let window = app
            .get_webview_window("widget")
            .ok_or("Widget indisponible.")?;
        widget::drag(&window).await?;
        let position = window
            .outer_position()
            .map_err(|_| "Position indisponible.")?;
        let size = window
            .outer_size()
            .map_err(|_| "Dimensions indisponibles.")?;
        let monitor = window
            .current_monitor()
            .map_err(|_| "Écran indisponible.")?
            .ok_or("Écran indisponible.")?;
        let mut data = state.data.lock().await;
        ensure_writable(&data)?;
        let mut next = data.config.clone();
        next.settings.widget_placement = Some(widget::capture(
            &next.settings.widget_position,
            (position.x, position.y),
            (size.width, size.height),
            (monitor.position().x, monitor.position().y),
            (monitor.size().width, monitor.size().height),
            monitor.name().cloned(),
        ));
        save(&state.path, &next)?;
        data.config = next;
        let _ = app.emit("snapshot", snapshot(&data, false));
        Ok(())
    }
    .await;
    state.dragging.store(false, Ordering::Relaxed);
    let data = state.data.lock().await;
    let mode = state.widget_mode.load(Ordering::Relaxed);
    apply_widget(
        &app,
        &data.config.settings,
        data.config.accounts.iter().filter(|a| a.enabled).count(),
        mode == 2,
        mode == 0,
    )?;
    result
}

async fn toggle_game_mode(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<Shared>();
    let mut data = state.data.lock().await;
    ensure_writable(&data)?;
    let mut next = data.config.clone();
    next.settings.game_mode = !next.settings.game_mode;
    save(&state.path, &next)?;
    data.config = next;
    if let Some(menu) = app.try_state::<GameMenu>() {
        let _ = menu.0.set_checked(data.config.settings.game_mode);
    }
    reconcile_visibility(&app, &data.config.settings);
    let _ = app.emit("snapshot", snapshot(&data, false));
    Ok(())
}

#[tauri::command]
async fn set_notch_mode(
    app: tauri::AppHandle,
    state: tauri::State<'_, Shared>,
    mode: String,
) -> Result<(), String> {
    if !["collapsed", "rail", "detail"].contains(&mode.as_str()) {
        return Err("Mode d’encoche invalide.".into());
    }
    let data = state.data.lock().await;
    apply_widget(
        &app,
        &data.config.settings,
        data.config.accounts.iter().filter(|a| a.enabled).count(),
        mode == "detail",
        mode == "collapsed",
    )
}

#[tauri::command]
async fn save_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, Shared>,
    mut settings: Settings,
) -> Result<Snapshot, String> {
    if ![30, 60, 120, 300].contains(&settings.poll_seconds)
        || !["en", "fr"].contains(&settings.language.as_str())
        || !(50..=100).contains(&settings.alert_percent)
        || !["top", "bottom", "left", "right"].contains(&settings.widget_position.as_str())
    {
        return Err("Paramètres invalides.".into());
    }
    let mut data = state.data.lock().await;
    ensure_writable(&data)?;
    // A stale settings form must not overwrite the placement saved by a drag.
    settings.widget_placement = if settings.widget_position == data.config.settings.widget_position
    {
        data.config.settings.widget_placement.clone()
    } else {
        None
    };
    let mut next = data.config.clone();
    next.settings = settings.clone();
    save(&state.path, &next)?;
    data.config = next;
    data.revision += 1;
    sync_language(&app, &settings.language);
    if let Some(menu) = app.try_state::<GameMenu>() {
        let _ = menu.0.set_checked(settings.game_mode);
    }
    apply_widget(
        &app,
        &settings,
        data.config.accounts.iter().filter(|a| a.enabled).count(),
        false,
        settings.auto_hide,
    )?;
    let result = snapshot(&data, false);
    let _ = app.emit("snapshot", &result);
    Ok(result)
}

#[tauri::command]
fn show_dashboard(app: tauri::AppHandle) {
    let _ = dashboard::apply(&app, dashboard::Action::Show);
}

#[tauri::command]
fn hide_dashboard(app: tauri::AppHandle) -> Result<(), String> {
    dashboard::apply(&app, dashboard::Action::Hide)
}

#[tauri::command]
fn minimize_dashboard(app: tauri::AppHandle) -> Result<(), String> {
    dashboard::apply(&app, dashboard::Action::Minimize)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_dashboard(app.clone())
        }))
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            refresh,
            save_account,
            remove_account,
            discover_accounts,
            save_settings,
            show_dashboard,
            hide_dashboard,
            minimize_dashboard,
            set_notch_mode,
            drag_notch
        ])
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&dir)?;
            let path = dir.join("config.json");
            let (config, storage_error) = if path.exists() {
                match std::fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Config>(&b).ok())
                {
                    Some(c) => (c, None),
                    None => (
                        Config::default(),
                        Some(format!(
                            "Configuration illisible : {}. Le fichier a été conservé.",
                            path.display()
                        )),
                    ),
                }
            } else {
                let c = Config {
                    accounts: discover(),
                    ..Default::default()
                };
                save(&path, &c).map_err(std::io::Error::other)?;
                (c, None)
            };
            apply_widget(
                app.handle(),
                &config.settings,
                config.accounts.iter().filter(|a| a.enabled).count(),
                false,
                config.settings.auto_hide,
            )
            .map_err(std::io::Error::other)?;
            let state = Arc::new(Backend {
                data: Mutex::new(Data {
                    config,
                    readings: HashMap::new(),
                    revision: 0,
                    storage_error,
                }),
                polling: Mutex::new(()),
                path,
                dragging: AtomicBool::new(false),
                fullscreen: AtomicBool::new(false),
                widget_mode: AtomicU8::new(0),
            });
            app.manage(state.clone());
            let show = tauri::menu::MenuItem::with_id(
                app,
                "show",
                "Réglages CodeNotch",
                true,
                None::<&str>,
            )?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "Quitter", true, None::<&str>)?;
            let game = tauri::menu::CheckMenuItem::with_id(
                app,
                "game",
                "Mode jeu — masquer l’encoche",
                true,
                state.data.blocking_lock().config.settings.game_mode,
                None::<&str>,
            )?;
            let menu = tauri::menu::Menu::with_items(app, &[&show, &game, &quit])?;
            app.manage(MenuLabels { show, quit });
            app.manage(GameMenu(game));
            sync_language(
                app.handle(),
                &state.data.blocking_lock().config.settings.language,
            );
            let mut tray = tauri::tray::TrayIconBuilder::new()
                .tooltip("CodeNotch Windows")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_dashboard(app.clone()),
                    "game" => {
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(error) = toggle_game_mode(handle.clone()).await {
                                let state = handle.state::<Shared>();
                                let data = state.data.lock().await;
                                if let Some(menu) = handle.try_state::<GameMenu>() {
                                    let _ = menu.0.set_checked(data.config.settings.game_mode);
                                }
                                let _ = handle.emit("widget-error", error);
                                show_dashboard(handle.clone());
                            }
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            let visibility_handle = app.handle().clone();
            let visibility_state = state.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let settings = visibility_state.data.lock().await.config.settings.clone();
                    reconcile_visibility(&visibility_handle, &settings);
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    poll(&handle, &state, false).await;
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if window.label() == "main" {
                    let _ = dashboard::apply(window.app_handle(), dashboard::Action::Hide);
                } else {
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("Impossible de démarrer CodeNotch");
}
