#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use codenotch::model;
use codenotch::settings;
use codenotch::store::{self, Store};
use codenotch::ui;
use gpui::{App, AppContext, Application};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (config, storage_error) = model::load_config();
    if args.iter().any(|arg| arg == "--once") {
        let reading = poll_once(&config);
        for (name, line) in reading {
            println!("{name}: {line}");
        }
        return;
    }

    if let Err(err) = codenotch::tray::install() {
        eprintln!("tray: {err}");
    }

    let open_settings = store::open_settings_if_needed(&config, &args);
    Application::new().run(move |cx: &mut App| {
        let store = cx.new(|_| Store::new(config, storage_error));
        ui::Rail::open(store.clone(), cx);
        if open_settings {
            settings::open(store.clone(), cx);
        }

        let watched = store.clone();
        cx.spawn(async move |cx| loop {
            if let Some(fullscreen) = codenotch::desktop::fullscreen_active() {
                let _ = watched.update(cx, |store, cx| {
                    if store.fullscreen != fullscreen {
                        store.fullscreen = fullscreen;
                        cx.notify();
                    }
                });
            }
            if codenotch::tray::quit_requested() {
                let _ = cx.update(|app| app.quit());
                break;
            }
            cx.background_executor().timer(Duration::from_secs(1)).await;
        })
        .detach();

        let polled = store.clone();
        cx.spawn(async move |cx| loop {
            store::poll(&polled, false, cx).await;
            cx.background_executor().timer(Duration::from_secs(10)).await;
        })
        .detach();

        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
}

fn poll_once(config: &codenotch::model::Config) -> Vec<(String, String)> {
    config
        .accounts
        .iter()
        .filter(|account| account.enabled)
        .map(|account| {
            let reading = codenotch::providers::fetch(account);
            let line = if reading.windows.is_empty() {
                reading.note.unwrap_or_else(|| "unavailable".into())
            } else {
                reading
                    .windows
                    .iter()
                    .map(|window| format!("{} {:.0}%", window.label, window.used_percent))
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            (account.name.clone(), line)
        })
        .collect()
}
