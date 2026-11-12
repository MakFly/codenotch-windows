//! The running application: engine + notch + settings + tray + hotkey + desktop polling.
//!
//! Threads (contract §7): everything here runs on the gpui main thread, except the engine (its
//! own threads), the hotkey and single-instance helper threads (they only send a `Command`), and
//! the demo command drain. Timers: one 200 ms UI task polls the tray menu; every second tick
//! (400 ms) it samples the foreground window for full-screen / game state; every 300th tick
//! (60 s) it refreshes the tray's countdowns.

use crate::i18n::tf;
use codenotch_core::config::ThemeMode;
use codenotch_core::model::{Command, Event, EventSink, ProviderKind, Snapshot};
use codenotch_core::{Config, Paths};
use codenotch_engine::{EngineHandle, EngineOptions};
use codenotch_platform::hotkey::{self, HotkeyHandle};
use codenotch_platform::single_instance::{self, Instance, InstanceGuard};
use codenotch_platform::tray::{Tray, TrayEvent};
use codenotch_ui_kit::UiModel;
use gpui::{App, Application, Entity};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

/// UI timer period (tray polling).
const TICK: Duration = Duration::from_millis(200);
/// Desktop (full-screen / game) sampling: every 2 ticks = 400 ms.
const DESKTOP_EVERY: u64 = 2;
/// Tray countdown refresh: every 300 ticks = 60 s.
const TRAY_EVERY: u64 = 300;
/// Longest wait for the engine to flush and stop when quitting.
const ENGINE_STOP_TIMEOUT: Duration = Duration::from_secs(3);
/// Quit anyway this long after the tray's Quit if the engine never answered.
const QUIT_FALLBACK: Duration = Duration::from_secs(3);
/// When quitting for an update, the "Restarting…" toast stays this long first.
const RESTART_TOAST: Duration = Duration::from_millis(700);
/// First automatic update check in `--demo` (only with `CODENOTCH_UPDATE_URL`).
const DEMO_UPDATE_DELAY: Duration = Duration::from_secs(3);

/// Start-up details from the command line.
#[derive(Debug, Clone, Default)]
pub struct Launch {
    /// `--updated-from=<version>`: this start follows a self-update.
    pub updated_from: Option<String>,
    /// Arguments to start a new version with after an update.
    pub args: Vec<String>,
}

/// UI-thread resources released, in order, when the app quits.
#[derive(Default)]
struct Shell {
    tray: Option<Tray>,
    hotkey: Option<HotkeyHandle>,
    /// The `hotkey` setting last applied (`None` = not applied yet).
    hotkey_setting: Option<Option<String>>,
    engine: Option<EngineHandle>,
    instance: Option<InstanceGuard>,
    /// `Command::UiBusy` last sent.
    ui_busy: Option<bool>,
}

impl Shell {
    /// (Re-)register the global hotkey when the setting changed. Returns a notice to show when
    /// it cannot be registered.
    fn apply_hotkey(&mut self, snapshot: &Snapshot, tx: &Sender<Command>) -> Option<String> {
        let wanted = snapshot
            .settings()
            .hotkey
            .clone()
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty());
        if self.hotkey_setting.as_ref() == Some(&wanted) {
            return None;
        }
        self.hotkey_setting = Some(wanted.clone());
        self.hotkey = None; // unregisters the previous combination first
        let text = wanted?;
        let lang = snapshot.lang();
        let Some(spec) = hotkey::parse(&text) else {
            log::warn!("hotkey setting not understood");
            return Some(tf(
                lang,
                "The global shortcut {key} is not valid.",
                &[("key", &text)],
            ));
        };
        let tx = tx.clone();
        match hotkey::register(
            spec,
            Box::new(move || {
                let _ = tx.send(Command::ToggleNotch);
            }),
        ) {
            Ok(handle) => {
                self.hotkey = Some(handle);
                None
            }
            Err(err) => {
                log::warn!("hotkey {spec} not registered: {err}");
                Some(tf(
                    lang,
                    "The global shortcut {key} could not be registered: {error}",
                    &[("key", &spec.to_string()), ("error", &err.to_string())],
                ))
            }
        }
    }

    /// Release everything: tray icon removed, hotkey unregistered, engine flushed and stopped
    /// (bounded wait), single-instance lock released.
    fn shutdown(&mut self) {
        self.tray = None;
        self.hotkey = None;
        if let Some(engine) = self.engine.take() {
            let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
            let spawned = std::thread::Builder::new()
                .name("codenotch-shutdown".into())
                .spawn(move || {
                    engine.shutdown();
                    let _ = done_tx.send(());
                });
            match spawned {
                Ok(_) => {
                    if done_rx.recv_timeout(ENGINE_STOP_TIMEOUT).is_err() {
                        log::warn!(
                            "engine did not stop within {} s",
                            ENGINE_STOP_TIMEOUT.as_secs()
                        );
                    }
                }
                Err(err) => log::warn!("cannot stop the engine cleanly: {err}"),
            }
        }
        self.instance = None;
        log::info!("codenotch stopped");
    }
}

/// `codenotch-hook.exe` next to the running exe.
fn hook_exe() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|e| e.with_file_name("codenotch-hook.exe"))
}

/// HWNDs of our own windows (notch, cards, settings), for the full-screen probe.
fn own_hwnds(cx: &mut App) -> Vec<isize> {
    cx.windows()
        .into_iter()
        .filter_map(|w| {
            w.update(cx, |_, window, _| codenotch_ui_kit::window::hwnd(window))
                .ok()
                .flatten()
        })
        .collect()
}

fn tray_command(event: TrayEvent, snapshot: &Snapshot) -> Command {
    match event {
        TrayEvent::RefreshAll => Command::Refresh(None),
        TrayEvent::OpenSettings => Command::OpenSettings,
        // Same path as the Settings switch: the whole config, game mode flipped.
        TrayEvent::ToggleGameMode => {
            let mut config = snapshot.config.clone();
            config.settings.game_mode = !config.settings.game_mode;
            Command::SaveConfig(Box::new(config))
        }
        TrayEvent::Quit => Command::Quit,
    }
}

/// `--demo` snapshot: fake figures for the providers discovered on this PC (file checks only,
/// read-only; worker thread, as in normal mode).
fn detected_demo(now: i64) -> Snapshot {
    let env = codenotch_core::Env::system();
    let found = codenotch_engine::discover_all(&codenotch_engine::registry(), &env);
    let kinds: Vec<ProviderKind> = found.into_iter().map(|a| a.provider).collect();
    log::info!(
        "demo: detected {}",
        if kinds.is_empty() {
            "nothing".to_string()
        } else {
            kinds.iter().map(|k| k.id()).collect::<Vec<_>>().join(", ")
        }
    );
    Snapshot::demo_for(&kinds, now)
}

/// How [`run`] starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run {
    /// Engine, real config.
    Normal,
    /// `--demo`: demo snapshot, no engine; `theme` forces the theme setting (`--theme=`);
    /// `preview_reset` shows the reset card once at start (`--preview-reset`, visual checks);
    /// `preview_card` keeps the notch open on the first account's hover card (`--preview-card`).
    Demo {
        theme: Option<ThemeMode>,
        preview_reset: bool,
        preview_card: bool,
    },
}

/// The demo snapshot with the `--theme` override applied to its config.
fn demo_snapshot(mut snapshot: Snapshot, theme: Option<ThemeMode>) -> Snapshot {
    if let Some(mode) = theme {
        snapshot.config.settings.theme = mode;
    }
    // Dev-only (visual checks): force the demo UI language, e.g. CODENOTCH_DEMO_LANG=fr.
    if let Ok(lang) = std::env::var("CODENOTCH_DEMO_LANG") {
        snapshot.config.settings.language = lang;
    }
    snapshot
}

/// `--demo`: no engine. Discovers the installed providers, publishes their demo snapshot, then
/// answers the few commands that make sense without an engine. Saved settings are applied to
/// the in-memory snapshot (nothing is written), so a change made in the settings window — the
/// theme, for one — reaches the notch live, as it does through the engine.
/// What the demo thread waits on: UI commands, and the updater's reports when one runs.
enum DemoMsg {
    Cmd(Command),
    Update(codenotch_engine::update::Report),
}

/// `--demo` with `CODENOTCH_UPDATE_URL` set: a real updater (end-to-end dry runs of the
/// self-update on a copy of the app). Never started otherwise in demo mode.
fn demo_updater(
    paths: &Paths,
    settings: &codenotch_core::Settings,
    args: Vec<String>,
    tx: std::sync::mpsc::Sender<DemoMsg>,
) -> Option<codenotch_engine::update::Updater> {
    let env = codenotch_core::Env::system();
    env.var(codenotch_engine::update::UPDATE_URL_ENV)?;
    log::info!(
        "demo: updater on ({} is set)",
        codenotch_engine::update::UPDATE_URL_ENV
    );
    let tuning = codenotch_engine::Tuning {
        update_delay: Some(DEMO_UPDATE_DELAY),
        ..codenotch_engine::Tuning::default()
    };
    let setup = codenotch_engine::updater_setup(
        paths,
        &env,
        Arc::new(codenotch_core::UreqHttp::new()),
        Arc::new(codenotch_core::now_secs),
        settings,
        &codenotch_engine::UpdateOptions {
            enabled: true,
            args,
            ..Default::default()
        },
        &tuning,
    );
    let tx = std::sync::Mutex::new(tx);
    Some(codenotch_engine::update::Updater::start(
        setup,
        Box::new(move |r| {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(DemoMsg::Update(r));
            }
        }),
    ))
}

fn run_demo_commands(
    rx: std::sync::mpsc::Receiver<Command>,
    sink: EventSink,
    theme: Option<ThemeMode>,
    paths: Paths,
    args: Vec<String>,
) {
    let (tx, msgs) = std::sync::mpsc::channel::<DemoMsg>();
    {
        let tx = tx.clone();
        let forwarded = std::thread::Builder::new()
            .name("codenotch-demo-commands".into())
            .spawn(move || {
                for command in rx {
                    if tx.send(DemoMsg::Cmd(command)).is_err() {
                        return;
                    }
                }
            });
        if let Err(err) = forwarded {
            log::error!("demo command forwarder not started: {err}");
        }
    }
    let spawned = std::thread::Builder::new()
        .name("codenotch-demo".into())
        .spawn(move || {
            let mut demo = demo_snapshot(detected_demo(codenotch_core::now_secs()), theme);
            let updater = demo_updater(&paths, &demo.config.settings, args, tx);
            if let Some(u) = &updater {
                demo.update = codenotch_core::UpdateState::Idle;
                demo.update_checked_at = None;
                u.idle(true);
            }
            let preview = demo.notch_accounts().first().map(|v| {
                let w = v.headline();
                Event::ResetDetected {
                    account_id: v.account.id.clone(),
                    window_id: w.map_or_else(|| "session".to_string(), |w| w.id.clone()),
                    used_percent: w.and_then(|w| w.used_percent),
                    next_reset_at: w.and_then(|w| w.resets_at),
                }
            });
            sink.emit(Event::SnapshotChanged(Box::new(demo.clone())));
            for msg in msgs {
                let command = match msg {
                    DemoMsg::Cmd(c) => c,
                    DemoMsg::Update(codenotch_engine::update::Report::State {
                        state,
                        last_check,
                    }) => {
                        demo.update = state;
                        demo.update_checked_at = last_check;
                        demo.now = codenotch_core::now_secs();
                        sink.emit(Event::SnapshotChanged(Box::new(demo.clone())));
                        continue;
                    }
                    DemoMsg::Update(codenotch_engine::update::Report::Restart) => {
                        sink.emit(Event::Quit);
                        continue;
                    }
                    DemoMsg::Update(codenotch_engine::update::Report::NewVersion { .. }) => {
                        continue;
                    }
                };
                match command {
                    Command::CheckUpdate if updater.is_some() => {
                        if let Some(u) = &updater {
                            u.check(true);
                        }
                    }
                    Command::InstallUpdate if updater.is_some() => {
                        if let Some(u) = &updater {
                            u.install();
                        }
                    }
                    Command::UiBusy(busy) if updater.is_some() => {
                        if let Some(u) = &updater {
                            u.idle(!busy);
                        }
                    }
                    Command::SaveConfig(config) => {
                        demo.config = *config;
                        sink.emit(Event::SnapshotChanged(Box::new(demo.clone())));
                    }
                    Command::Quit => sink.emit(Event::Quit),
                    Command::OpenSettings => sink.emit(Event::OpenSettings),
                    // A new summary stamp, so the settings window's refresh completes.
                    Command::RefreshAnalytics => {
                        if let Some(a) = demo.analytics.as_mut() {
                            a.generated_at = codenotch_core::now_secs().max(a.generated_at + 1);
                        }
                        sink.emit(Event::SnapshotChanged(Box::new(demo.clone())));
                    }
                    Command::PreviewResetCard => {
                        if let Some(e) = preview.clone() {
                            sink.emit(e);
                        }
                    }
                    other => log::info!("demo: ignoring {other:?}"),
                }
            }
        });
    if let Err(err) = spawned {
        log::error!("demo command thread not started: {err}");
    }
}

/// Taskbar mode: install the probe (platform measures the taskbar, ui-notch draws the strip).
fn wire_taskbar() {
    crate::taskbar::wire();
}

/// Start the GUI. [`Run::Demo`] = `Snapshot::demo_for` the providers detected on this PC,
/// without engine, network or single-instance lock. `open_settings` opens the settings window at start,
/// on `settings_page` when given.
pub fn run(
    paths: Paths,
    how: Run,
    open_settings: bool,
    settings_page: Option<codenotch_ui_settings::Page>,
    launch: Launch,
) {
    let (demo, demo_theme, preview_reset, preview_card) = match how {
        Run::Normal => (false, None, false, false),
        Run::Demo {
            theme,
            preview_reset,
            preview_card,
        } => (true, theme, preview_reset, preview_card),
    };
    if let Err(err) = paths.ensure() {
        eprintln!("cannot create the data folder: {err}");
    }
    let level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    let to_stderr = cfg!(debug_assertions);
    let color = to_stderr
        && codenotch_core::logging::stderr_color(codenotch_platform::console::enable_vt_stderr);
    let _ = codenotch_core::logging::init(&paths.logs_dir(), level, to_stderr, color);
    log::info!(
        "codenotch {} starting (demo: {demo})",
        codenotch_core::VERSION
    );

    // Single instance before any window: a second launch asks the first to open its settings.
    let instance = if demo {
        None
    } else {
        match single_instance::acquire() {
            Instance::Primary(guard) => Some(guard),
            Instance::AlreadyRunning => {
                log::info!("another instance is running; asked it to open settings");
                return;
            }
        }
    };

    let (config, storage_error) = if demo {
        (Config::default(), None)
    } else {
        Config::load_or_default(&paths.config())
    };
    if let Some(err) = &storage_error {
        log::warn!("config: {}", codenotch_core::logging::redact(err));
    }
    let welcome = !demo && config.needs_welcome();

    Application::new()
        .with_assets(codenotch_ui_notch::Assets::new())
        .run(move |cx: &mut App| {
            codenotch_ui_kit::init(cx);
            codenotch_ui_notch::set_backdrop_sampler(codenotch_platform::backdrop::luminance);
            wire_taskbar();

            let (tx, rx) = std::sync::mpsc::channel::<Command>();
            let snapshot = if demo {
                // Empty until the demo thread has run discovery (`run_demo_commands`).
                demo_snapshot(
                    Snapshot::demo_for(&[], codenotch_core::now_secs()),
                    demo_theme,
                )
            } else {
                let mut s = codenotch_engine::initial_snapshot(
                    config.clone(),
                    &paths,
                    codenotch_core::now_secs(),
                );
                s.storage_error = storage_error.clone();
                s
            };
            let model = UiModel::create(snapshot.clone(), tx.clone(), cx);
            let sink = UiModel::spawn_pump(&model, cx);
            let shell = Rc::new(RefCell::new(Shell::default()));

            if demo {
                run_demo_commands(rx, sink, demo_theme, paths.clone(), launch.args.clone());
                if preview_reset {
                    // Answered after the demo thread has published its snapshot.
                    let _ = tx.send(Command::PreviewResetCard);
                }
            } else {
                let opts = EngineOptions {
                    paths: paths.clone(),
                    secrets: Arc::new(codenotch_platform::secrets::CredentialStore::new()),
                    toast: Some(Arc::new(codenotch_notify::WinToast::new())),
                    hook_exe: hook_exe(),
                    storage_error: storage_error.clone(),
                    update: codenotch_engine::UpdateOptions {
                        enabled: true,
                        args: launch.args.clone(),
                        ..Default::default()
                    },
                    ..EngineOptions::system(Arc::new(codenotch_platform::WinSystem))
                };
                let engine = codenotch_engine::start(config.clone(), opts, (tx.clone(), rx), sink);
                let mut s = shell.borrow_mut();
                s.engine = Some(engine);
                if let Some(guard) = instance {
                    let tx = tx.clone();
                    guard.on_activate(Box::new(move || {
                        let _ = tx.send(Command::OpenSettings);
                    }));
                    s.instance = Some(guard);
                }
            }

            // One-shot events for the app shell (the notch and the settings window subscribe to
            // `Notice` / `ResetDetected` themselves).
            cx.subscribe(&model, |model, event: &Event, cx| match event {
                Event::OpenSettings => codenotch_ui_settings::open(model, cx),
                Event::Quit => {
                    let restarting = matches!(
                        model.read(cx).snapshot.update,
                        codenotch_core::UpdateState::Installing { .. }
                    );
                    if restarting {
                        // Let "Restarting…" be seen; the new version waits for this process.
                        cx.spawn(async move |cx| {
                            cx.background_executor().timer(RESTART_TOAST).await;
                            let _ = cx.update(|cx| cx.quit());
                        })
                        .detach();
                    } else {
                        cx.quit();
                    }
                }
                _ => {}
            })
            .detach();
            crate::updates::install(model.clone(), launch.updated_from.clone(), cx);

            if preview_card {
                codenotch_ui_notch::set_preview_card(true);
            }
            codenotch_ui_notch::open(model.clone(), cx);
            if welcome {
                codenotch_ui_settings::open_onboarding(model.clone(), cx);
            } else if open_settings {
                match settings_page {
                    Some(page) => codenotch_ui_settings::open_at(model.clone(), page, cx),
                    None => codenotch_ui_settings::open(model.clone(), cx),
                }
            }

            // Tray (UI thread only) and hotkey.
            {
                let mut s = shell.borrow_mut();
                s.tray = match Tray::new() {
                    Ok(mut t) => {
                        t.update(&snapshot);
                        Some(t)
                    }
                    Err(err) => {
                        log::warn!("tray: {err}");
                        None
                    }
                };
            }
            let hotkey_notice = if demo {
                None
            } else {
                shell.borrow_mut().apply_hotkey(&snapshot, &tx)
            };
            if let Some(text) = hotkey_notice {
                model.update(cx, |_, cx| cx.emit(Event::Notice(text)));
            }

            // Every snapshot: tray content, hotkey setting.
            {
                let shell = shell.clone();
                let tx = tx.clone();
                cx.observe(&model, move |model: Entity<UiModel>, cx| {
                    let notice = {
                        let snap = &model.read(cx).snapshot;
                        let mut s = shell.borrow_mut();
                        if let Some(t) = s.tray.as_mut() {
                            t.update(snap);
                        }
                        if demo {
                            None
                        } else {
                            s.apply_hotkey(snap, &tx)
                        }
                    };
                    if let Some(text) = notice {
                        model.update(cx, |_, cx| cx.emit(Event::Notice(text)));
                    }
                })
                .detach();
            }

            // UI timer: tray events (200 ms), desktop state (400 ms), tray countdowns (60 s).
            {
                let shell = shell.clone();
                let model = model.clone();
                cx.spawn(async move |cx| {
                    let mut tick: u64 = 0;
                    loop {
                        cx.background_executor().timer(TICK).await;
                        tick = tick.wrapping_add(1);
                        let alive = cx.update(|cx| {
                            let mut events = Vec::new();
                            {
                                let mut s = shell.borrow_mut();
                                if let Some(t) = s.tray.as_mut() {
                                    while let Some(ev) = t.poll_event() {
                                        events.push(ev);
                                    }
                                    if tick.is_multiple_of(TRAY_EVERY) {
                                        t.update(&model.read(cx).snapshot);
                                    }
                                }
                            }
                            for ev in events {
                                let quit = ev == TrayEvent::Quit;
                                let m = model.read(cx);
                                m.send(tray_command(ev, &m.snapshot));
                                if quit {
                                    // The engine answers Quit with Event::Quit; never hang if it is gone.
                                    cx.spawn(async move |cx| {
                                        cx.background_executor().timer(QUIT_FALLBACK).await;
                                        let _ = cx.update(|cx| cx.quit());
                                    })
                                    .detach();
                                }
                            }
                            if tick.is_multiple_of(DESKTOP_EVERY) {
                                // An automatic update waits while the settings window is in use.
                                let busy = codenotch_ui_settings::is_in_use(cx);
                                let changed = {
                                    let mut s = shell.borrow_mut();
                                    let changed = s.ui_busy != Some(busy);
                                    s.ui_busy = Some(busy);
                                    changed
                                };
                                if changed {
                                    model.read(cx).send(Command::UiBusy(busy));
                                }
                            }
                            if !demo && tick.is_multiple_of(DESKTOP_EVERY) {
                                let own = own_hwnds(cx);
                                let fullscreen =
                                    codenotch_platform::desktop::fullscreen_monitor(&own).is_some();
                                let game = codenotch_platform::desktop::game_in_front();
                                let m = model.read(cx);
                                if fullscreen != m.snapshot.fullscreen
                                    || game != m.snapshot.game_mode
                                {
                                    m.send(Command::DesktopState { fullscreen, game });
                                }
                            }
                        });
                        if alive.is_err() {
                            break;
                        }
                    }
                })
                .detach();
            }

            // Clean shutdown, whichever way the app quits.
            {
                let shell = shell.clone();
                cx.on_app_quit(move |_cx| {
                    shell.borrow_mut().shutdown();
                    async {}
                })
                .detach();
            }
        });
}
