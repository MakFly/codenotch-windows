//! Pure tray content: menu entries, tooltip and menu-id ↔ event mapping (data-spec §13,
//! visual-spec §10.1). No Win32 here; everything is unit-tested.
//!
//! The menu is fixed: Refresh all, Settings…, Game mode (check mark = `settings.game_mode`),
//! a separator, Quit CodeNotch. Usage figures live in the tooltip and the notch.

use super::TrayEvent;
use codenotch_core::format;
use codenotch_core::i18n::{tr_in, Lang};
use codenotch_core::model::{AccountView, ReadingStatus};
use codenotch_core::Snapshot;

/// Crate-local French for the tray (the core table has most of these too; this one wins).
pub static FR: &[(&str, &str)] = &[
    ("Refresh all", "Tout actualiser"),
    ("Settings…", "Réglages…"),
    ("Game mode", "Mode jeu"),
    ("Quit CodeNotch", "Quitter CodeNotch"),
];

pub const ID_REFRESH_ALL: &str = "refresh-all";
pub const ID_SETTINGS: &str = "settings";
pub const ID_GAME_MODE: &str = "game-mode";
pub const ID_QUIT: &str = "quit";

/// Longest tooltip Windows shows (`NOTIFYICONDATAW::szTip` is 128 UTF-16 units incl. NUL).
pub const TOOLTIP_MAX_UNITS: usize = 127;

/// One menu entry, in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Item {
        id: String,
        text: String,
    },
    /// Item with a check mark.
    Check {
        id: String,
        text: String,
        checked: bool,
    },
    Separator,
}

fn t(lang: Lang, key: &str) -> String {
    tr_in(lang, key, FR).to_string()
}

fn item(id: &str, text: String) -> MenuEntry {
    MenuEntry::Item {
        id: id.into(),
        text,
    }
}

/// Accounts listed in the tray: enabled, in provider order (including ones hidden from the
/// notch), minus those whose provider is not installed.
pub fn tray_accounts(snapshot: &Snapshot) -> Vec<&AccountView> {
    snapshot
        .config
        .ordered_accounts()
        .into_iter()
        .filter_map(|a| snapshot.account(&a.id))
        .filter(|v| {
            !matches!(
                v.reading.as_ref().map(|r| &r.status),
                Some(ReadingStatus::NotInstalled)
            )
        })
        .collect()
}

/// Display name of an account: its own name, else the provider's.
pub fn account_label(view: &AccountView) -> &str {
    let name = view.account.name.trim();
    if name.is_empty() {
        view.account.provider.display_name()
    } else {
        name
    }
}

/// The whole menu, top to bottom. The game-mode check mark follows the config switch only
/// (a game detected in front is not shown).
pub fn menu_model(snapshot: &Snapshot, lang: Lang) -> Vec<MenuEntry> {
    vec![
        item(ID_REFRESH_ALL, t(lang, "Refresh all")),
        item(ID_SETTINGS, t(lang, "Settings…")),
        MenuEntry::Check {
            id: ID_GAME_MODE.into(),
            text: t(lang, "Game mode"),
            checked: snapshot.settings().game_mode,
        },
        MenuEntry::Separator,
        item(ID_QUIT, t(lang, "Quit CodeNotch")),
    ]
}

/// Menu id → event. Unknown ids give `None`.
pub fn event_for_id(id: &str) -> Option<TrayEvent> {
    match id {
        ID_REFRESH_ALL => Some(TrayEvent::RefreshAll),
        ID_SETTINGS => Some(TrayEvent::OpenSettings),
        ID_GAME_MODE => Some(TrayEvent::ToggleGameMode),
        ID_QUIT => Some(TrayEvent::Quit),
        _ => None,
    }
}

/// Tooltip: "CodeNotch — Claude · perso 61% · Codex 23% · …", or "CodeNotch v<version>" when
/// no account is listed. Cut to [`TOOLTIP_MAX_UNITS`] at an account boundary.
pub fn tooltip_text(snapshot: &Snapshot) -> String {
    let accounts = tray_accounts(snapshot);
    if accounts.is_empty() {
        return format!("CodeNotch v{}", codenotch_core::VERSION);
    }
    let parts: Vec<String> = accounts
        .iter()
        .map(|v| format!("{} {}", account_label(v), format::cell_figure(v, false)))
        .collect();
    let units = |s: &str| s.encode_utf16().count();
    let mut text = String::from("CodeNotch —");
    for (i, part) in parts.iter().enumerate() {
        let sep = if i == 0 { " " } else { " · " };
        let candidate = format!("{text}{sep}{part}");
        let room_for_more = if i + 1 < parts.len() {
            units(" · …")
        } else {
            0
        };
        if units(&candidate) + room_for_more > TOOLTIP_MAX_UNITS {
            let ellipsis = format!("{text}{sep}…");
            if units(&ellipsis) <= TOOLTIP_MAX_UNITS {
                return ellipsis;
            }
            return truncate_units(&text, TOOLTIP_MAX_UNITS);
        }
        text = candidate;
    }
    text
}

fn truncate_units(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    for c in s.chars() {
        n += c.len_utf16();
        if n > max {
            break;
        }
        out.push(c);
    }
    out
}

/// Windows menu text: `&` would underline the next letter, a tab starts accelerator text.
pub fn escape_menu_text(text: &str) -> String {
    text.replace('&', "&&").replace('\t', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::demo::ids;
    use codenotch_core::model::Reading;

    const NOW: i64 = 1_763_000_000;

    fn texts(model: &[MenuEntry]) -> Vec<String> {
        model
            .iter()
            .map(|e| match e {
                MenuEntry::Item { text, .. } => text.clone(),
                MenuEntry::Check { text, checked, .. } => {
                    format!("{}{text}", if *checked { "[x] " } else { "[ ] " })
                }
                MenuEntry::Separator => "---".into(),
            })
            .collect()
    }

    #[test]
    fn menu_is_the_fixed_action_list() {
        let snap = Snapshot::demo_at(NOW);
        assert_eq!(
            texts(&menu_model(&snap, Lang::En)),
            [
                "Refresh all",
                "Settings…",
                "[ ] Game mode",
                "---",
                "Quit CodeNotch"
            ]
        );
        // No account lines, whatever the snapshot holds.
        let empty = Snapshot::from_config(Default::default(), NOW);
        assert_eq!(menu_model(&empty, Lang::En), menu_model(&snap, Lang::En));
    }

    #[test]
    fn french_menu() {
        let snap = Snapshot::demo_at(NOW);
        assert_eq!(
            texts(&menu_model(&snap, Lang::Fr)),
            [
                "Tout actualiser",
                "Réglages…",
                "[ ] Mode jeu",
                "---",
                "Quitter CodeNotch"
            ]
        );
    }

    #[test]
    fn game_mode_check_follows_the_config_switch_only() {
        let mut snap = Snapshot::demo_at(NOW);
        snap.game_mode = true; // a detected game, not the switch
        let checked = |s: &Snapshot| {
            menu_model(s, Lang::En).iter().any(
                |e| matches!(e, MenuEntry::Check { id, checked: true, .. } if id == ID_GAME_MODE),
            )
        };
        assert!(!checked(&snap));
        snap.config.settings.game_mode = true;
        assert!(checked(&snap));
        // A changed switch changes the entries (the tray rebuilds its menu on that).
        let mut off = snap.clone();
        off.config.settings.game_mode = false;
        assert_ne!(menu_model(&snap, Lang::En), menu_model(&off, Lang::En));
    }

    #[test]
    fn every_entry_maps_to_an_event() {
        let snap = Snapshot::demo_at(NOW);
        for e in menu_model(&snap, Lang::En) {
            match e {
                MenuEntry::Item { id, .. } | MenuEntry::Check { id, .. } => {
                    assert!(event_for_id(&id).is_some(), "{id}")
                }
                MenuEntry::Separator => {}
            }
        }
        assert_eq!(event_for_id("refresh-all"), Some(TrayEvent::RefreshAll));
        assert_eq!(event_for_id("settings"), Some(TrayEvent::OpenSettings));
        assert_eq!(event_for_id("game-mode"), Some(TrayEvent::ToggleGameMode));
        assert_eq!(event_for_id("quit"), Some(TrayEvent::Quit));
        assert_eq!(event_for_id("refresh:acc-1"), None);
        assert_eq!(event_for_id("nonsense"), None);
    }

    #[test]
    fn empty_snapshot_tooltip_is_the_version() {
        let snap = Snapshot::from_config(Default::default(), NOW);
        assert_eq!(
            tooltip_text(&snap),
            format!("CodeNotch v{}", codenotch_core::VERSION)
        );
    }

    #[test]
    fn not_installed_and_disabled_accounts_are_left_out() {
        let mut snap = Snapshot::demo_at(NOW);
        let qoder = snap.config.account(ids::QODER).unwrap().clone();
        snap.account_mut(ids::QODER).unwrap().reading = Some(Reading::not_installed(&qoder, NOW));
        snap.config.account_mut(ids::OPENCODE).unwrap().enabled = false;
        let listed: Vec<&str> = tray_accounts(&snap)
            .iter()
            .map(|v| v.account.id.as_str())
            .collect();
        assert!(!listed.contains(&ids::QODER));
        assert!(!listed.contains(&ids::OPENCODE));
        assert!(listed.contains(&ids::CLAUDE));
    }

    #[test]
    fn tooltip_lists_accounts_and_fits() {
        let snap = Snapshot::demo_at(NOW);
        let tip = tooltip_text(&snap);
        assert!(
            tip.starts_with("CodeNotch — Claude · perso 61% · Claude · work 88%"),
            "{tip}"
        );
        assert!(tip.encode_utf16().count() <= TOOLTIP_MAX_UNITS, "{tip}");
        // Demo has many accounts: the tail is cut at a boundary with an ellipsis.
        assert!(tip.ends_with(" · …") || !tip.contains('…'), "{tip}");

        let mut small = Snapshot::demo_at(NOW);
        small
            .config
            .accounts
            .retain(|a| a.id == ids::CLAUDE || a.id == ids::CODEX);
        assert_eq!(
            tooltip_text(&small),
            "CodeNotch — Claude · perso 61% · Codex 23%"
        );
    }

    #[test]
    fn menu_text_escaping() {
        assert_eq!(escape_menu_text("R&D — 5%"), "R&&D — 5%");
        assert_eq!(escape_menu_text("a\tb"), "a b");
    }

    #[test]
    fn truncation_counts_utf16_units() {
        assert_eq!(truncate_units("ab😀c", 3), "ab");
        assert_eq!(truncate_units("ab😀c", 4), "ab😀");
    }
}
