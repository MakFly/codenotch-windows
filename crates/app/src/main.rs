//! `codenotch.exe` — wiring and CLI.
//!
//! ```text
//! codenotch                start the notch (GUI); a second launch opens the first one's settings
//! codenotch --settings     start and open the settings window
//! codenotch --silent       start at login (same as no argument)
//! codenotch --demo         GUI on fake figures for the providers detected here, no engine, no network
//! codenotch --demo --settings      the same, with the settings window open
//! codenotch --settings-page=<page> open the settings on accounts | appearance | notifications |
//!                                  analytics | general (implies --settings; works with --demo)
//! codenotch --demo --theme=<mode>  demo with the theme setting forced to light | dark | system
//! codenotch --demo --preview-reset  demo that shows the reset card at start (visual checks)
//! codenotch --demo --preview-card   demo with the notch open on the first account's hover card
//! codenotch --once         fetch every enabled account once and print one line each (read-only)
//! codenotch doctor         data folder, config, providers, hooks, autostart (no secrets)
//! codenotch --version      print the version
//! ```
//!
//! Release builds use the Windows subsystem (no console window when started from Explorer);
//! the CLI modes attach to the parent console to print.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod doctor;
mod gui;
mod i18n;
mod icon;
mod taskbar;
mod updates;

use codenotch_core::config::ThemeMode;
use codenotch_core::format::{age, percent, reset_in};
use codenotch_core::logging::redact;
use codenotch_core::{i18n::tr, Config, Env, Paths};
use codenotch_engine::EngineOptions;
use codenotch_ui_settings::Page;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Gui {
        settings: bool,
        page: Option<Page>,
    },
    Demo {
        settings: bool,
        page: Option<Page>,
        theme: Option<ThemeMode>,
        preview_reset: bool,
        preview_card: bool,
    },
    Once,
    Doctor,
    Version,
    Help,
}

/// `--settings-page=<name>`: the page to open the settings window on (unknown names ignored).
fn settings_page(args: &[String]) -> Option<Page> {
    let value = args
        .iter()
        .skip(1)
        .find_map(|a| a.strip_prefix("--settings-page="))?;
    match value.to_ascii_lowercase().as_str() {
        "accounts" => Some(Page::Accounts),
        "appearance" => Some(Page::Appearance),
        "notifications" => Some(Page::Notifications),
        "analytics" => Some(Page::Analytics),
        "general" => Some(Page::General),
        _ => None,
    }
}

/// `--theme=<mode>` (demo only): force the demo config's theme setting (unknown names ignored).
fn theme_flag(args: &[String]) -> Option<ThemeMode> {
    let value = args
        .iter()
        .skip(1)
        .find_map(|a| a.strip_prefix("--theme="))?;
    match value.to_ascii_lowercase().as_str() {
        "light" => Some(ThemeMode::Light),
        "dark" => Some(ThemeMode::Dark),
        "system" => Some(ThemeMode::System),
        _ => None,
    }
}

/// Value of a `--name=value` flag.
fn flag_value<'a>(args: &'a [String], prefix: &str) -> Option<&'a str> {
    args.iter()
        .skip(1)
        .find_map(|a| a.strip_prefix(prefix))
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// Self-update restart flags: `--updated-from=<version>` and `--wait-pid=<pid>` (the previous
/// process, waited for before taking the single-instance lock).
fn restart_flags(args: &[String]) -> (Option<String>, Option<u32>) {
    (
        flag_value(args, "--updated-from=").map(str::to_string),
        flag_value(args, "--wait-pid=").and_then(|p| p.parse().ok()),
    )
}

/// Arguments the new version is started with after an update: ours, minus the program name.
fn relaunch_args(args: &[String]) -> Vec<String> {
    args.iter().skip(1).cloned().collect()
}

fn parse_args(args: &[String]) -> Mode {
    let has = |flag: &str| args.iter().skip(1).any(|a| a == flag);
    let page = settings_page(args);
    let settings = has("--settings") || args.iter().any(|a| a.starts_with("--settings-page="));
    if has("--version") || has("-V") {
        Mode::Version
    } else if has("--help") || has("-h") {
        Mode::Help
    } else if has("doctor") || has("--doctor") {
        Mode::Doctor
    } else if has("--once") {
        Mode::Once
    } else if has("--demo") {
        Mode::Demo {
            settings,
            page,
            theme: theme_flag(args),
            preview_reset: has("--preview-reset"),
            preview_card: has("--preview-card"),
        }
    } else {
        Mode::Gui { settings, page }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = parse_args(&args);
    // Main thread, before any other thread starts. Only the CLI modes attach to the parent
    // console: a GUI attached to a terminal would be killed when that terminal closes.
    if matches!(mode, Mode::Version | Mode::Help | Mode::Once | Mode::Doctor) {
        codenotch_platform::console::attach_parent_console();
    }
    codenotch_platform::apply_system_proxy_env();
    i18n::init();

    let (updated_from, wait_pid) = restart_flags(&args);
    if let (Some(pid), Mode::Gui { .. } | Mode::Demo { .. }) = (wait_pid, mode) {
        // Started by the previous version right after it replaced itself: let it exit (and
        // release the single-instance lock) first.
        if !codenotch_platform::process::wait_for_exit(pid, std::time::Duration::from_secs(30)) {
            eprintln!("the previous CodeNotch (pid {pid}) is still running");
        }
    }
    let launch = gui::Launch {
        updated_from,
        args: relaunch_args(&args),
    };

    let paths = Paths::resolve();
    match mode {
        Mode::Version => println!("codenotch {}", codenotch_core::VERSION),
        Mode::Help => println!(
            "codenotch {}\n\n\
             Usage: codenotch [--demo [--theme=<mode>]] [--settings] [--settings-page=<page>]\n\
             \x20      codenotch --once | doctor | --version\n\n\
             <page>: accounts | appearance | notifications | analytics | general\n\
             <mode>: light | dark | system (demo only)",
            codenotch_core::VERSION
        ),
        Mode::Doctor => doctor_cli(&paths),
        Mode::Once => once(&paths),
        Mode::Demo {
            settings,
            page,
            theme,
            preview_reset,
            preview_card,
        } => gui::run(
            paths,
            gui::Run::Demo {
                theme,
                preview_reset,
                preview_card,
            },
            settings,
            page,
            launch,
        ),
        Mode::Gui { settings, page } => gui::run(paths, gui::Run::Normal, settings, page, launch),
    }
}

// ---------------------------------------------------------------------------------------------
// CLI modes
// ---------------------------------------------------------------------------------------------

fn doctor_cli(paths: &Paths) {
    let env = Env::system();
    let store = codenotch_platform::secrets::CredentialStore::new();
    let overridden =
        std::env::var_os(codenotch_core::paths::DATA_DIR_ENV).is_some_and(|v| !v.is_empty());
    let text = doctor::report(&doctor::Probe {
        paths,
        env: &env,
        secrets: &store,
        exe: std::env::current_exe().ok(),
        autostart: Some(codenotch_platform::autostart::is_enabled()),
        data_dir_overridden: overridden,
        now: codenotch_core::now_secs(),
    });
    print!("{text}");
}

/// `--once`: read-only (nothing is written to the data folder; no log file).
fn once(paths: &Paths) {
    let (config, error) = Config::load_or_default(&paths.config());
    if let Some(err) = error {
        eprintln!("config: {}", redact(&err));
    }
    let lang = config.settings.lang();
    let opts = EngineOptions {
        paths: paths.clone(),
        secrets: Arc::new(codenotch_platform::secrets::CredentialStore::new()),
        ..EngineOptions::system(Arc::new(codenotch_core::NoopSystem))
    };
    let now = codenotch_core::now_secs();
    let results = codenotch_engine::fetch_once(&config, &opts, now);
    if results.is_empty() {
        println!("no enabled account");
    }
    for (account, reading) in results {
        let line = if reading.windows.is_empty() || !reading.shows_figures() {
            reading
                .note
                .clone()
                .or(reading.error.clone())
                .map(|n| tr(lang, &n).to_string())
                .unwrap_or_else(|| tr(lang, "Unavailable").to_string())
        } else {
            reading
                .windows
                .iter()
                .map(|w| {
                    let mut s = format!("{} {}", tr(lang, &w.label_key), percent(w.used_percent));
                    if let Some(r) = reset_in(lang, w.resets_at, now) {
                        s.push_str(&format!(" ({r})"));
                    }
                    s
                })
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let stale = if reading.is_stale(now) {
            format!(" [{}]", age(lang, reading.fetched_at, now))
        } else {
            String::new()
        };
        println!(
            "{} ({}): {}{}",
            account.name,
            account.provider.display_name(),
            redact(&line),
            stale
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("codenotch")
            .chain(list.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn cli_modes() {
        let gui = |settings, page| Mode::Gui { settings, page };
        let demo = |settings, page| Mode::Demo {
            settings,
            page,
            theme: None,
            preview_reset: false,
            preview_card: false,
        };
        assert_eq!(parse_args(&args(&[])), gui(false, None));
        assert_eq!(parse_args(&args(&["--silent"])), gui(false, None));
        assert_eq!(parse_args(&args(&["--settings"])), gui(true, None));
        assert_eq!(
            parse_args(&args(&["--settings-page=analytics"])),
            gui(true, Some(Page::Analytics))
        );
        assert_eq!(parse_args(&args(&["--demo"])), demo(false, None));
        assert_eq!(
            parse_args(&args(&["--demo", "--settings"])),
            demo(true, None)
        );
        assert_eq!(
            parse_args(&args(&["--settings", "--demo"])),
            demo(true, None)
        );
        assert_eq!(
            parse_args(&args(&["--demo", "--settings", "--settings-page=Accounts"])),
            demo(true, Some(Page::Accounts))
        );
        for (name, page) in [
            ("appearance", Page::Appearance),
            ("notifications", Page::Notifications),
            ("analytics", Page::Analytics),
            ("general", Page::General),
        ] {
            let flag = format!("--settings-page={name}");
            assert_eq!(
                parse_args(&args(&["--demo", &flag])),
                demo(true, Some(page))
            );
        }
        // An unknown page still opens the settings, on the default page.
        assert_eq!(
            parse_args(&args(&["--demo", "--settings-page=nope"])),
            demo(true, None)
        );
        assert_eq!(
            parse_args(&args(&["--demo", "--settings", "--theme=dark"])),
            Mode::Demo {
                settings: true,
                page: None,
                theme: Some(ThemeMode::Dark),
                preview_reset: false,
                preview_card: false,
            }
        );
        assert_eq!(
            parse_args(&args(&["--demo", "--theme=Light", "--preview-reset"])),
            Mode::Demo {
                settings: false,
                page: None,
                theme: Some(ThemeMode::Light),
                preview_reset: true,
                preview_card: false,
            }
        );
        assert_eq!(
            parse_args(&args(&["--demo", "--preview-card"])),
            Mode::Demo {
                settings: false,
                page: None,
                theme: None,
                preview_reset: false,
                preview_card: true,
            }
        );
        assert_eq!(
            parse_args(&args(&["--demo", "--theme=blue"])),
            demo(false, None)
        );
        // Not a demo: the flag is ignored (the real config decides).
        assert_eq!(parse_args(&args(&["--theme=dark"])), gui(false, None));
        assert_eq!(parse_args(&args(&["--once"])), Mode::Once);
        assert_eq!(parse_args(&args(&["doctor"])), Mode::Doctor);
        assert_eq!(parse_args(&args(&["--version"])), Mode::Version);
        assert_eq!(parse_args(&args(&["-h"])), Mode::Help);
        // Restart flags do not change the mode.
        assert_eq!(
            parse_args(&args(&[
                "--silent",
                "--updated-from=1.0.0",
                "--wait-pid=12"
            ])),
            gui(false, None)
        );
    }

    #[test]
    fn restart_flags_and_relaunch_args() {
        let a = args(&["--demo", "--updated-from= 1.0.0 ", "--wait-pid=1234"]);
        assert_eq!(restart_flags(&a), (Some("1.0.0".into()), Some(1234)));
        assert_eq!(restart_flags(&args(&["--wait-pid=x"])), (None, None));
        assert_eq!(restart_flags(&args(&["--updated-from="])), (None, None));
        assert_eq!(
            relaunch_args(&args(&["--silent"])),
            vec!["--silent".to_string()]
        );
    }

    /// Everything the settings page accepts as a global shortcut, the platform registers —
    /// and with the same canonical spelling.
    #[test]
    fn hotkeys_from_the_settings_validator_parse_in_platform() {
        use codenotch_platform::hotkey::parse;
        use codenotch_ui_settings::normalize_hotkey;
        let mut keys: Vec<String> = ('A'..='Z').chain('0'..='9').map(String::from).collect();
        keys.extend((1..=24).map(|n| format!("F{n}")));
        keys.extend(
            [
                "Space", "Insert", "Delete", "Home", "End", "PageUp", "PageDown", "Up", "Down",
                "Left", "Right",
            ]
            .map(String::from),
        );
        // Every modifier set the validator can accept (Shift needs a companion).
        let mods = [
            "Ctrl",
            "Alt",
            "Win",
            "Ctrl+Alt",
            "Ctrl+Shift",
            "Alt+Shift",
            "Win+Shift",
            "Ctrl+Win",
            "Alt+Win",
            "Ctrl+Alt+Shift",
            "Ctrl+Alt+Win",
            "Ctrl+Alt+Shift+Win",
            "Shift+Win",
        ];
        let mut checked = 0;
        for m in mods {
            for k in &keys {
                for typed in [
                    format!("{m}+{k}"),
                    format!("{m}+{k}").to_lowercase().replace('+', " + "),
                ] {
                    let Ok(Some(normal)) = normalize_hotkey(&typed) else {
                        panic!("validator refused {typed:?}");
                    };
                    let spec =
                        parse(&normal).unwrap_or_else(|| panic!("platform refused {normal:?}"));
                    assert_eq!(spec.to_string(), normal, "same canonical text");
                    checked += 1;
                }
            }
        }
        assert!(checked > 1500);
        // And what the validator refuses is not something the settings could have stored.
        for bad in ["Shift+N", "N", "Ctrl+F25", "Ctrl+Alt"] {
            assert!(normalize_hotkey(bad).is_err(), "{bad}");
            assert!(parse(bad).is_none(), "{bad}");
        }
    }
}
