#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// The provider modules are shared with the Tauri app; not every helper is used here yet.
#![allow(dead_code)]

#[path = "../../src-tauri/src/alerts.rs"]
mod alerts;
#[path = "../../src-tauri/src/claude.rs"]
mod claude;
#[path = "../../src-tauri/src/codex.rs"]
mod codex;
#[path = "../../src-tauri/src/discord.rs"]
mod discord;
#[path = "../../src-tauri/src/gemini.rs"]
mod gemini;
#[path = "../../src-tauri/src/grok.rs"]
mod grok;
#[path = "../../src-tauri/src/login.rs"]
mod login;
#[path = "../../src-tauri/src/model.rs"]
mod model;
#[path = "../../src-tauri/src/opencode.rs"]
mod opencode;
#[path = "../../src-tauri/src/providers.rs"]
mod providers;
#[path = "../../src-tauri/src/telegram.rs"]
mod telegram;

mod account_form;
mod agents;
mod assets;
mod autostart;
mod channels;
mod desktop;
mod history;
mod home;
mod hotkey;
mod input;
mod settings;
mod store;
mod tray;
mod updater;
mod usage;
mod welcome;
mod ui;

use gpui::{App, AppContext, Application};
use std::time::Duration;

fn main() {
    // Provider requests use reqwest and tokio processes, so they run on a tokio runtime
    // while GPUI owns the main thread.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    store::RUNTIME.set(runtime.handle().clone()).ok();

    Application::new().with_assets(assets::Assets).run(|cx: &mut App| {
        let (mut config, storage_error) = store::load();
        // Configurations from before the welcome screen: offer it only to people who have
        // not connected anything yet.
        let first_run = !*config
            .settings
            .onboarding_done
            .get_or_insert(!config.accounts.is_empty());
        autostart::reconcile(config.settings.launch_at_startup);
        let store = cx.new(|_| store::Store::new(config, storage_error));
        ui::open_rail(store.clone(), cx);
        input::bind_keys(cx);
        tray::install(store.clone(), cx);
        hotkey::install(store.clone(), cx);
        if first_run || std::env::args().any(|a| a == "--settings") {
            settings::open(store.clone(), cx);
        }
        let watched = store.clone();
        cx.spawn(async move |cx| loop {
            if let Some(fullscreen) = desktop::fullscreen_active() {
                let _ = watched.update(cx, |s, cx| {
                    if s.fullscreen != fullscreen {
                        s.fullscreen = fullscreen;
                        cx.notify();
                    }
                });
            }
            cx.background_executor().timer(Duration::from_secs(1)).await;
        })
        .detach();
        // Like the Tauri app: shortly after launch, then twice a day. Errors stay silent
        // since the settings offer a manual check.
        let updates = store.clone();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs(20)).await;
            loop {
                let _ = store::check_update(&updates, true, cx).await;
                cx.background_executor().timer(Duration::from_secs(12 * 3600)).await;
            }
        })
        .detach();
        cx.spawn(async move |cx| loop {
            store::poll(&store, false, cx).await;
            cx.background_executor().timer(Duration::from_secs(10)).await;
        })
        .detach();
    });
    drop(runtime);
}
