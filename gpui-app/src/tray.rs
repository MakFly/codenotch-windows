use std::cell::RefCell;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

thread_local! {
    static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
}

pub fn install() -> Result<(), String> {
    let menu = Menu::new();
    let quit = MenuItem::with_id("quit", "Quit CodeNotch", true, None);
    menu.append(&quit).map_err(|err| err.to_string())?;
    let icon = Icon::from_rgba(icon_pixels(), 32, 32).map_err(|err| err.to_string())?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("CodeNotch")
        .with_icon(icon)
        .build()
        .map_err(|err| err.to_string())?;
    TRAY.with(|slot| *slot.borrow_mut() = Some(tray));
    Ok(())
}

/// Call from the thread that created the tray (the UI thread).
pub fn set_tooltip(text: &str) {
    TRAY.with(|slot| {
        if let Some(tray) = slot.borrow().as_ref() {
            let _ = tray.set_tooltip(Some(text));
        }
    });
}

pub fn quit_requested() -> bool {
    let Ok(receiver) = MenuEvent::receiver().try_recv() else {
        return false;
    };
    receiver.id.0 == "quit"
}

fn icon_pixels() -> Vec<u8> {
    let mut pixels = vec![0u8; 32 * 32 * 4];
    for y in 0..32 {
        for x in 0..32 {
            let dx = x as i32 - 16;
            let dy = y as i32 - 16;
            if dx * dx + dy * dy <= 13 * 13 {
                let index = (y * 32 + x) * 4;
                pixels[index] = 20;
                pixels[index + 1] = 20;
                pixels[index + 2] = 20;
                pixels[index + 3] = 255;
            }
        }
    }
    pixels
}
