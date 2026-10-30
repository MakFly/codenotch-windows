//! System-wide shortcuts: one shows or folds the notch, the same keys with G toggle game
//! mode. Windows delivers them to a hidden window on the UI thread; the events arrive on a
//! channel drained like the tray menu's.
use crate::store::Store;
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui::{App, Entity, Global};
use std::time::Duration;

/// The shortcuts offered in the settings, for the notch; game mode uses G instead of N.
pub const PRESETS: [&str; 2] = ["Ctrl+Alt+N", "Ctrl+Shift+Alt+N"];

struct Hotkeys {
    manager: GlobalHotKeyManager,
    /// Registered (notch, game mode) shortcuts.
    active: Option<(HotKey, HotKey)>,
    /// The setting they were registered for.
    applied: Option<Option<String>>,
}
impl Global for Hotkeys {}

fn game_keys(notch: &str) -> String {
    format!("{}G", notch.trim_end_matches('N'))
}

/// Registers the shortcuts the settings ask for, once per change.
fn apply(store: &Entity<Store>, cx: &mut App) {
    let wanted = store.read(cx).config.settings.hotkey.clone();
    let hotkeys = cx.global_mut::<Hotkeys>();
    if hotkeys.applied.as_ref() == Some(&wanted) {
        return;
    }
    if let Some((notch, game)) = hotkeys.active.take() {
        let _ = hotkeys.manager.unregister_all(&[notch, game]);
    }
    hotkeys.applied = Some(wanted.clone());
    let error = wanted.and_then(|keys| {
        let parsed = keys.parse::<HotKey>().ok().zip(game_keys(&keys).parse::<HotKey>().ok());
        let Some((notch, game)) = parsed else {
            return Some("Raccourci invalide.".to_owned());
        };
        match hotkeys.manager.register_all(&[notch, game]) {
            Ok(()) => {
                hotkeys.active = Some((notch, game));
                None
            }
            Err(_) => Some("Ce raccourci est déjà utilisé par une autre application.".to_owned()),
        }
    });
    store.update(cx, |s, cx| {
        if s.hotkey_error != error {
            s.hotkey_error = error;
            cx.notify();
        }
    });
}

/// Shows the notch folded to its tab, or folds it; without auto-hide it is always shown,
/// so the shortcut opens the settings with every quota instead.
fn toggle_notch(store: &Entity<Store>, cx: &mut App) {
    let s = store.read(cx);
    if !s.config.settings.auto_hide {
        return crate::settings::open(store.clone(), cx);
    }
    if s.collapsed || s.folding {
        store.update(cx, |s, cx| {
            s.collapsed = false;
            s.folding = false;
            s.fold_generation += 1;
            cx.notify();
        });
    } else {
        crate::ui::fold(store, cx);
    }
}

fn toggle_game_mode(store: &Entity<Store>, cx: &mut App) {
    store.update(cx, |s, cx| {
        let mut settings = s.config.settings.clone();
        settings.game_mode = !settings.game_mode;
        let _ = s.save_settings(settings);
        cx.notify();
    });
}

pub fn install(store: Entity<Store>, cx: &mut App) {
    let Ok(manager) = GlobalHotKeyManager::new() else {
        return;
    };
    cx.set_global(Hotkeys { manager, active: None, applied: None });
    apply(&store, cx);
    cx.observe(&store, |store, cx| apply(&store, cx)).detach();
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(Duration::from_millis(100)).await;
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.state() != HotKeyState::Pressed {
                continue;
            }
            let _ = cx.update(|cx| {
                let Some((notch, game)) = cx.global::<Hotkeys>().active else {
                    return;
                };
                if event.id() == notch.id() {
                    toggle_notch(&store, cx);
                } else if event.id() == game.id() {
                    toggle_game_mode(&store, cx);
                }
            });
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_parse_with_their_game_mode_twin() {
        for preset in PRESETS {
            assert!(preset.parse::<HotKey>().is_ok(), "{preset}");
            assert!(game_keys(preset).parse::<HotKey>().is_ok(), "{preset}");
            assert_ne!(preset.parse::<HotKey>().unwrap(), game_keys(preset).parse::<HotKey>().unwrap());
        }
    }
}
