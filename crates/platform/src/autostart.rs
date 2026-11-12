//! "Open at login" via `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value [`RUN_VALUE`]
//! = `"<exe>" --silent` (REG_SZ). Per-user, no elevation.
//!
//! Task Manager's "Startup apps" switch writes a separate flag under
//! `…\Explorer\StartupApproved\Run` (first byte odd = disabled). [`is_enabled`] honours it, and
//! [`set_enabled`]`(true, …)` clears our own flag so turning the option on in the app really
//! turns it on. Only the value named [`RUN_VALUE`] is ever touched.

use crate::reg::Key;
use codenotch_core::Result;
use std::path::Path;

pub const RUN_VALUE: &str = "CodeNotch";

/// The real per-user Run key.
pub const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// Where Explorer / Task Manager remember a user's "disabled" choice.
pub const APPROVED_KEY: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";

/// Registry locations (injectable so tests use a throwaway key).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Location<'a> {
    pub run_key: &'a str,
    pub approved_key: &'a str,
    pub value: &'a str,
}

const REAL: Location<'static> = Location {
    run_key: RUN_KEY,
    approved_key: APPROVED_KEY,
    value: RUN_VALUE,
};

/// Registered to start at login (and not disabled in Task Manager).
pub fn is_enabled() -> bool {
    is_enabled_at(REAL)
}

/// `exe` = absolute path to `codenotch.exe`; the command line gets `--silent`.
pub fn set_enabled(enabled: bool, exe: &Path) -> Result<()> {
    set_enabled_at(REAL, enabled, exe)
}

/// The command line written to the Run value.
pub fn command_line(exe: &Path) -> String {
    format!("\"{}\" --silent", exe.display())
}

/// Task Manager flag: first byte even (2, 6) = enabled, odd (3, 7) = disabled by the user.
pub(crate) fn approved_says_disabled(flag: &[u8]) -> bool {
    flag.first().is_some_and(|b| b & 1 == 1)
}

pub(crate) fn is_enabled_at(loc: Location<'_>) -> bool {
    let registered = Key::open_read(loc.run_key)
        .and_then(|k| k.get_string(loc.value))
        .is_some_and(|cmd| !cmd.trim().is_empty());
    if !registered {
        return false;
    }
    let disabled = Key::open_read(loc.approved_key)
        .and_then(|k| k.get_binary(loc.value))
        .is_some_and(|flag| approved_says_disabled(&flag));
    !disabled
}

pub(crate) fn set_enabled_at(loc: Location<'_>, enabled: bool, exe: &Path) -> Result<()> {
    if enabled {
        if !exe.is_absolute() {
            return Err(codenotch_core::Error::other(
                "autostart needs an absolute exe path",
            ));
        }
        Key::create(loc.run_key)?.set_string(loc.value, &command_line(exe))?;
        // A stale "disabled in Task Manager" flag would silently keep it off.
        if let Some(approved) = Key::open_read(loc.approved_key) {
            if approved.get_binary(loc.value).is_some() {
                drop(approved);
                Key::create(loc.approved_key)?.delete_value(loc.value)?;
            }
        }
        log::info!("autostart enabled");
    } else {
        if Key::open_read(loc.run_key).is_some_and(|k| k.get_string(loc.value).is_some()) {
            Key::create(loc.run_key)?.delete_value(loc.value)?;
        }
        log::info!("autostart disabled");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reg;
    use crate::win::test_util::random_token;

    #[test]
    fn command_line_quotes_the_exe() {
        assert_eq!(
            command_line(Path::new("C:\\Program Files\\CodeNotch\\codenotch.exe")),
            "\"C:\\Program Files\\CodeNotch\\codenotch.exe\" --silent"
        );
    }

    #[test]
    fn approved_flag_parity() {
        assert!(!approved_says_disabled(&[2, 0, 0]));
        assert!(approved_says_disabled(&[3, 0, 0]));
        assert!(!approved_says_disabled(&[6]));
        assert!(approved_says_disabled(&[7]));
        assert!(!approved_says_disabled(&[]));
    }

    /// Uses only `HKCU\Software\CodeNotchTest\<random>`; never the real Run key.
    #[test]
    fn enable_disable_in_a_throwaway_key() {
        let root = format!("Software\\CodeNotchTest\\{}", random_token());
        let run = format!("{root}\\Run");
        let approved = format!("{root}\\Approved");
        let loc = Location {
            run_key: &run,
            approved_key: &approved,
            value: "CodeNotchTestValue",
        };
        let exe = Path::new("C:\\Apps\\codenotch.exe");

        let result = std::panic::catch_unwind(|| {
            assert!(!is_enabled_at(loc));
            set_enabled_at(loc, true, exe).unwrap();
            assert!(is_enabled_at(loc));
            assert_eq!(
                Key::open_read(&run)
                    .unwrap()
                    .get_string("CodeNotchTestValue")
                    .as_deref(),
                Some("\"C:\\Apps\\codenotch.exe\" --silent")
            );

            // Disabled from Task Manager → reported off; enabling again clears the flag.
            Key::create(&approved)
                .unwrap()
                .set_binary("CodeNotchTestValue", &[3, 0, 0, 0])
                .unwrap();
            assert!(!is_enabled_at(loc));
            set_enabled_at(loc, true, exe).unwrap();
            assert!(is_enabled_at(loc));
            assert!(Key::open_read(&approved)
                .unwrap()
                .get_binary("CodeNotchTestValue")
                .is_none());

            set_enabled_at(loc, false, exe).unwrap();
            assert!(!is_enabled_at(loc));
            // Disabling twice is fine.
            set_enabled_at(loc, false, exe).unwrap();
            assert!(set_enabled_at(loc, true, Path::new("relative.exe")).is_err());
        });
        reg::delete_tree(&root).unwrap();
        assert!(Key::open_read(&root).is_none());
        if let Err(p) = result {
            std::panic::resume_unwind(p);
        }
    }
}
