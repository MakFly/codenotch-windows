//! Replacing the installed exes (pure file operations, unit tested on temp folders).
//!
//! Windows will not overwrite or delete a running `.exe`, but it will rename one. So, for
//! `codenotch.exe` then `codenotch-hook.exe`:
//! 1. the staged file is copied next to the target as `<name>.new` (same volume);
//! 2. the installed file is renamed `<name>.old` (retried briefly: a Claude Code hook may be
//!    starting `codenotch-hook.exe` at that very moment);
//! 3. `<name>.new` is renamed `<name>`.
//!
//! Any failure rolls every step back (`.old` renamed back). The `.old` files are deleted at the
//! next start ([`cleanup_install_dir`]), once the old process is gone.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The app.
pub const APP_EXE: &str = "codenotch.exe";
/// The Claude Code hook, next to the app.
pub const HOOK_EXE: &str = "codenotch-hook.exe";
/// What a package installs, in swap order. Only the app is required.
pub const PACKAGE_FILES: [&str; 2] = [APP_EXE, HOOK_EXE];

/// Retries of a rename that fails (locked file).
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    pub attempts: u32,
    pub pause: Duration,
}

impl Retry {
    pub const DEFAULT: Retry = Retry {
        attempts: 10,
        pause: Duration::from_millis(200),
    };
    pub const NONE: Retry = Retry {
        attempts: 1,
        pause: Duration::ZERO,
    };
}

fn with_suffix(dir: &Path, name: &str, suffix: &str) -> PathBuf {
    dir.join(format!("{name}{suffix}"))
}

fn rename_retry(from: &Path, to: &Path, retry: Retry) -> std::io::Result<()> {
    let mut last = None;
    for i in 0..retry.attempts.max(1) {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if i + 1 < retry.attempts {
                    std::thread::sleep(retry.pause);
                }
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

/// Remove `path` if it exists; when it cannot be removed (a running old exe), move it aside
/// under a unique `.old` name so the slot is free.
fn clear_slot(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if std::fs::remove_file(path).is_ok() {
        return Ok(());
    }
    let aside = path.with_extension(format!("{}.old", crate::update::unique_suffix()));
    std::fs::rename(path, aside)
}

/// What [`swap_in`] changed, to undo it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swapped {
    pub dir: PathBuf,
    /// Names replaced, in order (each has a `<name>.old` unless it did not exist before).
    pub replaced: Vec<(String, bool)>,
}

/// Install the files of `staged` into `install_dir` (see the module docs). `staged` must hold
/// [`APP_EXE`]; [`HOOK_EXE`] is installed when present. On error everything is rolled back and
/// the message says what failed.
pub fn swap_in(install_dir: &Path, staged: &Path, retry: Retry) -> Result<Swapped, String> {
    let names: Vec<&str> = PACKAGE_FILES
        .iter()
        .copied()
        .filter(|n| staged.join(n).is_file())
        .collect();
    if !names.contains(&APP_EXE) {
        return Err(format!("{APP_EXE} is missing from the package"));
    }
    // 1. Copy everything next to the targets first (the slow, most likely to fail part).
    for name in &names {
        let new = with_suffix(install_dir, name, ".new");
        let copied = clear_slot(&new).and_then(|_| std::fs::copy(staged.join(name), &new));
        if let Err(e) = copied {
            remove_news(install_dir);
            return Err(format!("cannot copy {name}: {}", e.kind()));
        }
    }
    // 2–3. Swap each file.
    let mut swapped = Swapped {
        dir: install_dir.to_path_buf(),
        replaced: Vec::new(),
    };
    for name in &names {
        let target = install_dir.join(name);
        let old = with_suffix(install_dir, name, ".old");
        let new = with_suffix(install_dir, name, ".new");
        let had = target.exists();
        let step = (|| -> Result<(), String> {
            if had {
                clear_slot(&old).map_err(|e| format!("cannot free {name}.old: {}", e.kind()))?;
                rename_retry(&target, &old, retry)
                    .map_err(|e| format!("cannot move {name} aside: {}", e.kind()))?;
            }
            if let Err(e) = rename_retry(&new, &target, retry) {
                if had {
                    let _ = rename_retry(&old, &target, retry);
                }
                return Err(format!("cannot put the new {name} in place: {}", e.kind()));
            }
            Ok(())
        })();
        if let Err(msg) = step {
            rollback(&swapped, retry);
            remove_news(install_dir);
            return Err(msg);
        }
        swapped.replaced.push((name.to_string(), had));
    }
    Ok(swapped)
}

/// Undo [`swap_in`]: each replaced file goes back from `<name>.old` (newest first).
pub fn rollback(swapped: &Swapped, retry: Retry) -> Vec<String> {
    let mut errors = Vec::new();
    for (name, had) in swapped.replaced.iter().rev() {
        let target = swapped.dir.join(name);
        let old = with_suffix(&swapped.dir, name, ".old");
        if !had {
            if let Err(e) = std::fs::remove_file(&target) {
                errors.push(format!("{name}: {}", e.kind()));
            }
            continue;
        }
        if !old.exists() {
            errors.push(format!("{name}.old is missing"));
            continue;
        }
        // The new file may be running already (unlikely): move it aside rather than fail.
        let failed = with_suffix(&swapped.dir, name, ".failed");
        let _ = clear_slot(&failed);
        if target.exists() && rename_retry(&target, &failed, retry).is_err() {
            errors.push(format!("{name}: cannot move the new file aside"));
            continue;
        }
        if let Err(e) = rename_retry(&old, &target, retry) {
            errors.push(format!("{name}: {}", e.kind()));
        }
        let _ = std::fs::remove_file(&failed);
    }
    errors
}

fn remove_news(dir: &Path) {
    for name in PACKAGE_FILES {
        let _ = std::fs::remove_file(with_suffix(dir, name, ".new"));
    }
}

/// Delete the leftovers of a previous update in `dir` (`*.old`, `*.new`, `*.failed` of the
/// package files). Returns the names that could not be deleted yet (old process still running).
pub fn cleanup_install_dir(dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut left = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        let ours = PACKAGE_FILES.iter().any(|p| lower.starts_with(p));
        let leftover =
            lower.ends_with(".old") || lower.ends_with(".new") || lower.ends_with(".failed");
        if !(ours && leftover) || !entry.path().is_file() {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_err() {
            left.push(name);
        }
    }
    left
}

/// The running exe is a cargo build output (a `target` folder in its path): self-update would
/// replace a developer's build, so it is refused unless explicitly allowed.
pub fn is_dev_build(exe: &Path) -> bool {
    exe.parent().is_some_and(|dir| {
        dir.components().any(|c| {
            c.as_os_str()
                .to_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("target"))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn read(dir: &Path, name: &str) -> Option<String> {
        std::fs::read_to_string(dir.join(name)).ok()
    }

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("CodeNotch");
        let staged = tmp.path().join("staged");
        write(&install, APP_EXE, "old app");
        write(&install, HOOK_EXE, "old hook");
        write(&install, "keep.txt", "user file");
        write(&staged, APP_EXE, "new app");
        write(&staged, HOOK_EXE, "new hook");
        (tmp, install, staged)
    }

    #[test]
    fn swap_replaces_both_and_keeps_old_copies() {
        let (_tmp, install, staged) = setup();
        let s = swap_in(&install, &staged, Retry::NONE).unwrap();
        assert_eq!(read(&install, APP_EXE).as_deref(), Some("new app"));
        assert_eq!(read(&install, HOOK_EXE).as_deref(), Some("new hook"));
        assert_eq!(
            read(&install, "codenotch.exe.old").as_deref(),
            Some("old app")
        );
        assert_eq!(
            read(&install, "codenotch-hook.exe.old").as_deref(),
            Some("old hook")
        );
        assert!(read(&install, "codenotch.exe.new").is_none());
        assert_eq!(s.replaced.len(), 2);
        // Rollback restores the previous files.
        assert!(rollback(&s, Retry::NONE).is_empty());
        assert_eq!(read(&install, APP_EXE).as_deref(), Some("old app"));
        assert_eq!(read(&install, HOOK_EXE).as_deref(), Some("old hook"));
        assert!(read(&install, "codenotch.exe.old").is_none());
        assert_eq!(read(&install, "keep.txt").as_deref(), Some("user file"));
    }

    #[test]
    fn missing_hook_in_install_or_package() {
        let (_tmp, install, staged) = setup();
        std::fs::remove_file(install.join(HOOK_EXE)).unwrap();
        let s = swap_in(&install, &staged, Retry::NONE).unwrap();
        assert_eq!(s.replaced[1], (HOOK_EXE.to_string(), false));
        assert!(rollback(&s, Retry::NONE).is_empty());
        assert!(
            read(&install, HOOK_EXE).is_none(),
            "added hook removed again"
        );

        let (_tmp, install, staged) = setup();
        std::fs::remove_file(staged.join(HOOK_EXE)).unwrap();
        swap_in(&install, &staged, Retry::NONE).unwrap();
        assert_eq!(
            read(&install, HOOK_EXE).as_deref(),
            Some("old hook"),
            "kept"
        );

        let (_tmp, install, staged) = setup();
        std::fs::remove_file(staged.join(APP_EXE)).unwrap();
        assert!(swap_in(&install, &staged, Retry::NONE)
            .unwrap_err()
            .contains("missing"));
        assert_eq!(read(&install, APP_EXE).as_deref(), Some("old app"));
    }

    #[cfg(windows)]
    #[test]
    fn failure_on_the_second_file_rolls_the_first_back() {
        use std::os::windows::fs::OpenOptionsExt;
        let (_tmp, install, staged) = setup();
        // The hook held open without FILE_SHARE_DELETE (as a process reading it could): it
        // cannot be renamed, so the swap fails after the app was already swapped.
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ only
            .open(install.join(HOOK_EXE))
            .unwrap();
        let err = swap_in(&install, &staged, Retry::NONE).unwrap_err();
        drop(lock);
        assert!(err.contains(HOOK_EXE), "{err}");
        assert_eq!(read(&install, APP_EXE).as_deref(), Some("old app"));
        assert_eq!(read(&install, HOOK_EXE).as_deref(), Some("old hook"));
        assert!(read(&install, "codenotch.exe.new").is_none());
        assert!(read(&install, "codenotch-hook.exe.new").is_none());
    }

    #[test]
    fn stale_old_files_are_replaced_and_cleaned() {
        let (_tmp, install, staged) = setup();
        write(&install, "codenotch.exe.old", "very old");
        swap_in(&install, &staged, Retry::NONE).unwrap();
        assert_eq!(
            read(&install, "codenotch.exe.old").as_deref(),
            Some("old app")
        );
        write(&install, "codenotch.exe.new", "half copied");
        write(&install, "other.old", "not ours");
        assert!(cleanup_install_dir(&install).is_empty());
        assert!(read(&install, "codenotch.exe.old").is_none());
        assert!(read(&install, "codenotch-hook.exe.old").is_none());
        assert!(read(&install, "codenotch.exe.new").is_none());
        assert_eq!(read(&install, "other.old").as_deref(), Some("not ours"));
        assert_eq!(read(&install, APP_EXE).as_deref(), Some("new app"));
        assert!(cleanup_install_dir(&install.join("nope")).is_empty());
    }

    #[test]
    fn dev_builds_are_recognised() {
        assert!(is_dev_build(Path::new(
            r"C:\src\codenotch\target\debug\codenotch.exe"
        )));
        assert!(is_dev_build(Path::new(
            r"C:\src\cn\TARGET\x86_64-pc-windows-msvc\release\codenotch.exe"
        )));
        assert!(!is_dev_build(Path::new(
            r"C:\Users\u\AppData\Local\Programs\CodeNotch\codenotch.exe"
        )));
        assert!(!is_dev_build(Path::new(
            r"D:\tools\CodeNotch-1.0.0\codenotch.exe"
        )));
        assert!(!is_dev_build(Path::new(r"C:\targets\codenotch.exe")));
    }
}
