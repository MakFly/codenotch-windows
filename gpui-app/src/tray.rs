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
    game: CheckMenuItem,
}
impl Global for Tray {}

fn icon() -> Option<Icon> {
    let image = image::load_from_memory(include_bytes!("../../src-tauri/icons/32x32.png"))
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).ok()
}

pub fn install(store: Entity<Store>, cx: &mut App) {
    let s = store.read(cx);
    let french = s.french();
    let pick = |fr: &str, en: &str| if french { fr } else { en }.to_owned();
    // The settings window is not ported yet.
    let settings = MenuItem::new(
        pick("Réglages CodeNotch (bientôt)", "CodeNotch settings (soon)"),
        false,
        None,
    );
    let game = CheckMenuItem::new(
        pick("Mode jeu — masquer l’encoche", "Game mode — hide notch"),
        true,
        s.config.settings.game_mode,
        None,
    );
    let quit = MenuItem::new(pick("Quitter", "Quit"), true, None);
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
    let (game_id, quit_id) = (game.id().clone(), quit.id().clone());
    cx.set_global(Tray { _icon: tray, game });

    // Menu events arrive on a channel; drain it from the UI thread.
    cx.spawn(async move |cx| loop {
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == quit_id {
                let _ = cx.update(|cx| cx.quit());
            } else if event.id == game_id {
                let _ = cx.update(|cx| {
                    let on = store.update(cx, |s, cx| {
                        // In memory only: the prototype never writes the Tauri configuration.
                        s.config.settings.game_mode = !s.config.settings.game_mode;
                        cx.notify();
                        s.config.settings.game_mode
                    });
                    cx.global::<Tray>().game.set_checked(on);
                });
            }
        }
    })
    .detach();
}
