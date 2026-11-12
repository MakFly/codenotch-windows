//! System tray icon + menu (tray-icon crate). Create and use on the UI thread only (the gpui
//! main thread, which pumps the Win32 messages the tray window needs).
//!
//! * Menu: Refresh all, Settings…, Game mode (checked when the config switch is on; a click
//!   toggles it), then Quit CodeNotch. Opens on left and right click. Rebuilt only when an
//!   entry changed (language, game-mode switch).
//! * Tooltip: "CodeNotch — Claude 61% · Codex 23%" (see [`model::tooltip_text`]).
//! * Icon: drawn in code ([`icon`]); with `trayShowsUsage` it shows the worst headline
//!   percentage coloured by grade.
//!
//! Call [`Tray::update`] on every snapshot (and periodically: the icon follows the figures).

pub mod icon;
pub mod model;

use codenotch_core::{Error, Result, Snapshot};
use model::MenuEntry;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub use model::tooltip_text;

/// A click in the tray menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEvent {
    RefreshAll,
    OpenSettings,
    /// Flip `settings.game_mode` (the caller saves the config).
    ToggleGameMode,
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    menu_entries: Vec<MenuEntry>,
    /// The menu currently attached and the one before it: kept alive so a rebuild that
    /// happens while the popup is open never destroys the HMENU being tracked.
    menus: [Option<Menu>; 2],
    tooltip: String,
    icon_key: (icon::IconSpec, u32),
}

impl Tray {
    /// Create the tray icon with the plain mark and a "waiting" menu.
    pub fn new() -> Result<Tray> {
        let size = icon_size();
        let image = make_icon(&icon::IconSpec::MARK, size)?;
        let tooltip = format!("CodeNotch v{}", codenotch_core::VERSION);
        let entries = model::menu_model(
            &Snapshot::from_config(Default::default(), 0),
            crate::system_lang(),
        );
        let menu = build_menu(&entries)?;
        let tray = TrayIconBuilder::new()
            .with_id("codenotch")
            .with_icon(image)
            .with_tooltip(&tooltip)
            .with_menu(Box::new(menu.clone()))
            .with_menu_on_left_click(true)
            .with_menu_on_right_click(true)
            .build()
            .map_err(|e| Error::other(format!("cannot create the tray icon: {e}")))?;
        Ok(Tray {
            icon: tray,
            menu_entries: entries,
            menus: [Some(menu), None],
            tooltip,
            icon_key: (icon::IconSpec::MARK, size),
        })
    }

    /// Rebuild menu + tooltip + icon from the snapshot (each only when it changed).
    pub fn update(&mut self, snapshot: &Snapshot) {
        let lang = crate::resolve_lang(&snapshot.settings().language);
        let entries = model::menu_model(snapshot, lang);
        if entries != self.menu_entries {
            match build_menu(&entries) {
                Ok(menu) => {
                    self.icon.set_menu(Some(Box::new(menu.clone())));
                    self.menus.swap(0, 1);
                    self.menus[0] = Some(menu);
                    self.menu_entries = entries;
                }
                Err(e) => log::warn!("tray menu: {e}"),
            }
        }

        let tooltip = model::tooltip_text(snapshot);
        if tooltip != self.tooltip {
            match self.icon.set_tooltip(Some(&tooltip)) {
                Ok(()) => self.tooltip = tooltip,
                Err(e) => log::warn!("tray tooltip: {e}"),
            }
        }

        let key = (icon::icon_spec(snapshot), icon_size());
        if key != self.icon_key {
            match make_icon(&key.0, key.1).and_then(|img| {
                self.icon
                    .set_icon(Some(img))
                    .map_err(|e| Error::other(format!("tray icon: {e}")))
            }) {
                Ok(()) => self.icon_key = key,
                Err(e) => log::warn!("{e}"),
            }
        }
    }

    /// Next pending menu event, if any (poll from a UI timer, e.g. every 100–250 ms).
    pub fn poll_event(&mut self) -> Option<TrayEvent> {
        // Icon clicks/moves are not used (the menu opens by itself); drain them so the global
        // unbounded channel does not grow.
        while TrayIconEvent::receiver().try_recv().is_ok() {}
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if let Some(event) = model::event_for_id(&ev.id.0) {
                return Some(event);
            }
        }
        None
    }
}

fn build_menu(entries: &[MenuEntry]) -> Result<Menu> {
    let menu = Menu::new();
    for entry in entries {
        let appended = match entry {
            MenuEntry::Item { id, text } => {
                let item =
                    MenuItem::with_id(MenuId::new(id), model::escape_menu_text(text), true, None);
                menu.append(&item)
            }
            MenuEntry::Check { id, text, checked } => {
                let item = CheckMenuItem::with_id(
                    MenuId::new(id),
                    model::escape_menu_text(text),
                    true,
                    *checked,
                    None,
                );
                menu.append(&item)
            }
            MenuEntry::Separator => menu.append(&PredefinedMenuItem::separator()),
        };
        appended.map_err(|e| Error::other(format!("cannot build the tray menu: {e}")))?;
    }
    Ok(menu)
}

fn make_icon(spec: &icon::IconSpec, size: u32) -> Result<Icon> {
    Icon::from_rgba(icon::rasterize(spec, size), size, size)
        .map_err(|e| Error::other(format!("bad tray icon: {e}")))
}

/// Small-icon size for the current DPI, snapped to 16/20/24/32.
fn icon_size() -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
    // SAFETY: no preconditions.
    let px = unsafe { GetSystemMetrics(SM_CXSMICON) };
    icon::snap_size(if px > 0 { px } else { 16 })
}

/// Tooltip text (data-spec §13): "CodeNotch — Claude 42% · Codex 10%".
pub fn tooltip(snapshot: &Snapshot) -> String {
    model::tooltip_text(snapshot)
}
