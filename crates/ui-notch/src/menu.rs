//! Notch right-click menu (visual-spec §10.2): Refresh now · Open <host> · — · ☑ Keep open ·
//! Settings… · — · Quit. Built as data here, shown natively by `win32::popup_menu`.

use crate::text::{host, t, tf};
use crate::win32::MenuItem;
use codenotch_core::config::{Config, ShowMode};
use codenotch_core::i18n::Lang;
use codenotch_core::model::Command;

pub const REFRESH: u32 = 1;
pub const OPEN: u32 = 2;
pub const KEEP_OPEN: u32 = 3;
pub const SETTINGS: u32 = 4;
pub const QUIT: u32 = 5;

/// Menu for the ring under the pointer (`usage_url` of its provider, if any).
pub fn items(lang: Lang, usage_url: Option<&str>, show_mode: ShowMode) -> Vec<MenuItem> {
    let item = |id, label: String, checked| MenuItem::Item {
        id,
        label,
        checked,
        enabled: true,
    };
    let mut v = vec![item(REFRESH, t(lang, "Refresh now").to_string(), false)];
    if let Some(url) = usage_url {
        v.push(item(
            OPEN,
            tf(lang, "Open {host}", &[("host", host(url))]),
            false,
        ));
    }
    v.push(MenuItem::Separator);
    v.push(item(
        KEEP_OPEN,
        t(lang, "Keep open").to_string(),
        show_mode == ShowMode::Always,
    ));
    v.push(item(SETTINGS, t(lang, "Settings…").to_string(), false));
    v.push(MenuItem::Separator);
    v.push(item(QUIT, t(lang, "Quit CodeNotch").to_string(), false));
    v
}

/// Command for a chosen entry. "Keep open" flips Always ↔ Show on hover through `SaveConfig`.
pub fn command(id: u32, usage_url: Option<&str>, config: &Config) -> Option<Command> {
    match id {
        REFRESH => Some(Command::Refresh(None)),
        OPEN => usage_url.map(|u| Command::OpenUrl(u.to_string())),
        KEEP_OPEN => {
            let mut c = config.clone();
            let next = if c.settings.show_mode() == ShowMode::Always {
                ShowMode::Hover
            } else {
                ShowMode::Always
            };
            c.settings.show_mode = Some(next);
            Some(Command::SaveConfig(Box::new(c)))
        }
        SETTINGS => Some(Command::OpenSettings),
        QUIT => Some(Command::Quit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(v: &[MenuItem]) -> Vec<String> {
        v.iter()
            .map(|i| match i {
                MenuItem::Item { label, checked, .. } => {
                    format!("{}{label}", if *checked { "[x] " } else { "" })
                }
                MenuItem::Separator => "-".into(),
            })
            .collect()
    }

    #[test]
    fn english_and_french_menus() {
        let en = items(
            Lang::En,
            Some("https://claude.ai/settings/usage"),
            ShowMode::Always,
        );
        assert_eq!(
            labels(&en),
            [
                "Refresh now",
                "Open claude.ai",
                "-",
                "[x] Keep open",
                "Settings…",
                "-",
                "Quit CodeNotch"
            ]
        );
        let fr = items(Lang::Fr, None, ShowMode::Hover);
        assert_eq!(
            labels(&fr),
            [
                "Actualiser maintenant",
                "-",
                "Garder ouverte",
                "Réglages…",
                "-",
                "Quitter CodeNotch"
            ]
        );
    }

    #[test]
    fn commands() {
        let mut cfg = Config::default();
        cfg.settings.show_mode = Some(ShowMode::Always);
        assert_eq!(command(REFRESH, None, &cfg), Some(Command::Refresh(None)));
        assert_eq!(
            command(OPEN, Some("https://x.y"), &cfg),
            Some(Command::OpenUrl("https://x.y".into()))
        );
        assert_eq!(command(OPEN, None, &cfg), None);
        match command(KEEP_OPEN, None, &cfg) {
            Some(Command::SaveConfig(c)) => assert_eq!(c.settings.show_mode, Some(ShowMode::Hover)),
            other => panic!("{other:?}"),
        }
        cfg.settings.show_mode = Some(ShowMode::Hover);
        match command(KEEP_OPEN, None, &cfg) {
            Some(Command::SaveConfig(c)) => {
                assert_eq!(c.settings.show_mode, Some(ShowMode::Always))
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(command(QUIT, None, &cfg), Some(Command::Quit));
        assert_eq!(command(99, None, &cfg), None);
    }
}
