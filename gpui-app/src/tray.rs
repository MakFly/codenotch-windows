use crate::store::Store;
use gpui::{App, Entity, Global};
use std::time::Duration;
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

/// Keeps the tray icon alive for the whole session; dropping it removes the icon.
struct Tray {
    icon: TrayIcon,
    settings: MenuItem,
    game: CheckMenuItem,
    quit: MenuItem,
    /// The gauge (percentage rounded to 5, color) and tooltip last shown, to skip redraws.
    shown: Option<(Option<(u8, [u8; 3])>, String)>,
}
impl Global for Tray {}

/// Windows cuts tray tooltips at 127 characters.
const TOOLTIP_MAX: usize = 127;

fn base() -> Option<image::RgbaImage> {
    Some(
        image::load_from_memory(include_bytes!("../assets/app/32x32.png"))
            .ok()?
            .into_rgba8(),
    )
}

fn icon() -> Option<Icon> {
    let image = base()?;
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).ok()
}

/// The app icon with a usage gauge along its bottom edge.
fn gauge_icon(percent: u8, color: [u8; 3]) -> Option<Icon> {
    let mut image = base()?;
    let (width, height) = image.dimensions();
    let thickness = (height / 5).max(3);
    let filled = (width * percent.min(100) as u32).div_ceil(100);
    for y in height - thickness..height {
        for x in 0..width {
            let [r, g, b] = if x < filled { color } else { [0x33, 0x33, 0x33] };
            image.put_pixel(x, y, image::Rgba([r, g, b, 0xff]));
        }
    }
    Icon::from_rgba(image.into_raw(), width, height).ok()
}

/// The most used window among the enabled accounts, and one tooltip line per account.
fn usage(s: &Store) -> (Option<f64>, String) {
    let french = s.french();
    let mut top: Option<f64> = None;
    let mut tooltip = String::from("CodeNotch");
    for account in s.enabled() {
        let Some(quota) = crate::ui::headline(s.readings.get(&account.id)) else {
            continue;
        };
        top = Some(top.map_or(quota.used_percent, |t| t.max(quota.used_percent)));
        let line = format!(
            "\n{} · {} : {}",
            account.name,
            crate::ui::t(french, &quota.label),
            crate::ui::percentage(quota.used_percent, french)
        );
        if tooltip.chars().count() + line.chars().count() > TOOLTIP_MAX {
            break;
        }
        tooltip.push_str(&line);
    }
    (top, tooltip)
}

/// Menu labels and the game mode check follow the saved settings; the icon shows the
/// most used quota when the setting asks for it.
fn sync(store: &Entity<Store>, cx: &mut App) {
    let s = store.read(cx);
    let french = s.french();
    let game_mode = s.config.settings.game_mode;
    let (top, tooltip) = usage(s);
    let gauge = top.filter(|_| s.config.settings.tray_shows_usage).map(|v| {
        let c = crate::ui::usage_color(Some(v));
        let channel = |f: f32| (f * 255.).round() as u8;
        ((v / 5.).round() as u8 * 5, [channel(c.r), channel(c.g), channel(c.b)])
    });
    let tray = cx.global_mut::<Tray>();
    if tray.shown.as_ref() != Some(&(gauge, tooltip.clone())) {
        let image = match gauge {
            Some((percent, color)) => gauge_icon(percent, color),
            None => icon(),
        };
        let _ = tray.icon.set_icon(image);
        let _ = tray.icon.set_tooltip(Some(&tooltip));
        tray.shown = Some((gauge, tooltip));
    }
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
        icon: tray,
        settings,
        game,
        quit,
        shown: None,
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
