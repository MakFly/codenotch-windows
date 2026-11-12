//! # codenotch-platform (owner: agent F)
//!
//! Win32 services for the app shell. No gpui. Everything here is called by `codenotch` (app):
//!
//! | item | thread | notes |
//! |---|---|---|
//! | [`console::attach_parent_console`], [`apply_system_proxy_env`] | main, first thing | before other threads |
//! | [`single_instance::acquire`] | main, before the UI | keep the guard alive; `on_activate` runs on a private thread |
//! | [`tray::Tray`] | **UI thread only** | `update` on each snapshot + every ~60 s; `poll_event` every 100–250 ms |
//! | [`desktop::fullscreen_monitor`], [`desktop::game_in_front`] | UI timer, 400 ms | cheap, read-only |
//! | [`backdrop::luminance`] | UI timer, 500 ms while folded | small GDI capture |
//! | [`taskbar::probe`] | UI timer, 250 ms in taskbar mode | cache read; measuring runs on its own thread |
//! | [`hotkey::register`] | any (UI) | runs its own message-loop thread; callback on that thread |
//! | [`WinSystem`], [`secrets::CredentialStore`] | engine thread / any | thread-safe, stateless |
//!
//! Note: monitor enumeration and window placement helpers live in `codenotch-ui-kit::window`
//! (UI crates need them and may not depend on this crate).

use codenotch_core::{Lang, Result, Snapshot, SystemActions};
use std::path::Path;

mod reg;
mod system;
mod win;

pub mod autostart;
pub mod backdrop;
pub mod desktop;
pub mod hotkey;
pub mod process;
pub mod secrets;
pub mod single_instance;
pub mod taskbar;
pub mod tray;

pub use system::{checked_url, proxy_from_settings};

/// Attach to the parent console so CLI modes print in a release (`windows_subsystem`) build.
pub mod console {
    /// Returns true when a console is available afterwards.
    pub fn attach_parent_console() -> bool {
        #[cfg(windows)]
        // SAFETY: console queries/attach have no preconditions; failure is reported by the result.
        unsafe {
            use windows_sys::Win32::System::Console::{
                AttachConsole, GetConsoleWindow, ATTACH_PARENT_PROCESS,
            };
            if !GetConsoleWindow().is_null() {
                return true;
            }
            AttachConsole(ATTACH_PARENT_PROCESS) != 0
        }
        #[cfg(not(windows))]
        true
    }

    /// Turn on ANSI escape processing for the console behind stderr. Returns true when escape
    /// codes will render: VT processing is (now) on, or stderr is not a console at all (an
    /// MSYS/mintty pty is a pipe that understands them itself).
    pub fn enable_vt_stderr() -> bool {
        #[cfg(windows)]
        // SAFETY: handle query and console mode calls on our own stderr handle; failures are
        // reported by the return values.
        unsafe {
            use windows_sys::Win32::System::Console::{
                GetConsoleMode, GetStdHandle, SetConsoleMode, CONSOLE_MODE,
                ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_ERROR_HANDLE,
            };
            let handle = GetStdHandle(STD_ERROR_HANDLE);
            let mut mode: CONSOLE_MODE = 0;
            if GetConsoleMode(handle, &mut mode) == 0 {
                return true; // not a console (pipe/pty)
            }
            mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0
                || SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
        }
        #[cfg(not(windows))]
        true
    }
}

/// If no proxy env var is set, copy the WinINet proxy (HKCU Internet Settings) into
/// `HTTPS_PROXY`/`HTTP_PROXY` before the first request (data-spec §0). Call early in `main`,
/// before other threads start. Also sets `NO_PROXY` (loopback + `ProxyOverride`) when unset.
pub fn apply_system_proxy_env() {
    system::apply_system_proxy_env()
}

/// Resolve `language: "auto"` from the OS UI language (French UI → Fr, else En).
pub fn system_lang() -> Lang {
    system::system_lang()
}

/// The UI language for a `language` setting: "auto"/"system"/"" → [`system_lang`], else the code.
pub fn resolve_lang(language: &str) -> Lang {
    match language.trim().to_ascii_lowercase().as_str() {
        "" | "auto" | "system" => system_lang(),
        code => Lang::from_code(code),
    }
}

/// [`SystemActions`] for the engine: ShellExecute for URLs/folders, autostart registry.
/// `open_url` refuses anything but http/https; `open_path` shows a folder (for a file, its
/// parent folder — files are never executed).
#[derive(Debug, Default, Clone, Copy)]
pub struct WinSystem;

impl SystemActions for WinSystem {
    fn open_url(&self, url: &str) -> Result<()> {
        system::open_url(url)
    }
    fn open_path(&self, path: &Path) -> Result<()> {
        system::open_path(path)
    }
    fn set_launch_at_startup(&self, enabled: bool) -> Result<()> {
        let exe =
            std::env::current_exe().map_err(|e| codenotch_core::Error::io("current exe", e))?;
        autostart::set_enabled(enabled, &exe)
    }
    fn launch_at_startup(&self) -> bool {
        autostart::is_enabled()
    }
}

/// Snapshot-derived tray tooltip, re-exported for convenience.
pub fn tray_tooltip(snapshot: &Snapshot) -> String {
    tray::tooltip(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_lang_codes() {
        assert_eq!(resolve_lang("fr"), Lang::Fr);
        assert_eq!(resolve_lang("FR-ca"), Lang::Fr);
        assert_eq!(resolve_lang("en"), Lang::En);
        assert_eq!(resolve_lang("de"), Lang::En);
        assert_eq!(resolve_lang("auto"), system_lang());
        assert_eq!(resolve_lang(""), system_lang());
    }

    #[test]
    fn open_url_refuses_other_schemes_without_launching() {
        // Rejected before any shell call.
        assert!(WinSystem
            .open_url("file:///C:/Windows/notepad.exe")
            .is_err());
        assert!(WinSystem.open_url("calc.exe").is_err());
    }
}
