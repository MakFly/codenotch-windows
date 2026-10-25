use crate::store::Store;
use gpui::{App, Entity, Global};
use std::time::Duration;
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

/// Keeps the tray icon alive for the whole session; dropping it removes the icon.
struct Tray {
    _icon: TrayIcon,
    settings: MenuItem,
    game: CheckMenuItem,
    quit: MenuItem,
}
impl Global for Tray {}

fn icon() -> Option<Icon> {
    let image = image::load_from_memory(include_bytes!("../../src-tauri/icons/32x32.png"))
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).ok()
}

/// Menu labels and the game mode check follow the saved settings.
fn sync(store: &Entity<Store>, cx: &mut App) {
    let s = store.read(cx);
    let french = s.french();
    let game_mode = s.config.settings.game_mode;
    let tray = cx.global::<Tray>();
    tray.settings
        .set_text(if french { "Réglages CodeNotch" } else { "CodeNotch settings" });
    tray.game.set_text(if french {
        "Mode jeu — masquer l’encoche"
    } else {
        "Game mode — hide notch"
    });
    tray.game.set_checked(game_mode);
    tray.quit.set_text(if french { "Quitter" } else { "Quit" });
}

pub fn install(store: Entity<Store>, cx: &mut App) {
    let settings = MenuItem::new("", true, None);
    let game = CheckMenuItem::new("", true, false, None);
    let quit = MenuItem::new("", true, None);
    let Ok(menu) = Menu::with_items(&[&settings, &game, &quit]) else {
        return;
    };
    let mut builder = TrayIconBuilder::new()
        .with_tooltip("CodeNotch Windows")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true);
    if let Some(icon) = icon() {
        builder = builder.with_icon(icon);
    }
    let Ok(tray) = builder.build() else {
        return;
    };
    let (settings_id, game_id, quit_id) = (
        settings.id().clone(),
        game.id().clone(),
        quit.id().clone(),
    );
    cx.set_global(Tray {
        _icon: tray,
        settings,
        game,
        quit,
    });
    sync(&store, cx);
    cx.observe(&store, |store, cx| sync(&store, cx)).detach();

    // Menu events arrive on a channel; drain it from the UI thread.
    cx.spawn(async move |cx| loop {
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let _ = cx.update(|cx| {
                if event.id == quit_id {
                    cx.quit();
                } else if event.id == settings_id {
                    crate::settings::open(store.clone(), cx);
                } else if event.id == game_id {
                    let result = store.update(cx, |s, cx| {
                        let mut settings = s.config.settings.clone();
                        settings.game_mode = !settings.game_mode;
                        let result = s.save_settings(settings);
                        cx.notify();
                        result
                    });
                    // Windows already flipped the check; put it back on a failed save.
                    if result.is_err() {
                        sync(&store, cx);
                    }
                }
            });
        }
    })
    .detach();
}
