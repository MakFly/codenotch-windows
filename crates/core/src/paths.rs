//! Where CodeNotch keeps its files.
//!
//! Data folder = `$CODENOTCH_DATA_DIR` when set and non-empty, else
//! `%APPDATA%\dev.kev.smashtoken` (the folder the 0.2/0.3 apps already use).
//! Tests must always use [`Paths::at`] with a temp dir.

use std::path::{Path, PathBuf};

/// Folder name under `%APPDATA%` (kept from earlier versions).
pub const APP_DIR_NAME: &str = "dev.kev.smashtoken";
/// Environment variable that overrides the data folder.
pub const DATA_DIR_ENV: &str = "CODENOTCH_DATA_DIR";

/// All file locations, rooted at one data folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Resolve from the environment: `CODENOTCH_DATA_DIR`, else `%APPDATA%\dev.kev.smashtoken`.
    pub fn resolve() -> Self {
        let root = match std::env::var_os(DATA_DIR_ENV) {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(APP_DIR_NAME),
        };
        Self { root }
    }

    /// Use an explicit folder (tests, `--data-dir`).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The data folder itself.
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// `config.gpui.json`
    pub fn config(&self) -> PathBuf {
        self.root.join("config.gpui.json")
    }
    /// `history.jsonl`
    pub fn history(&self) -> PathBuf {
        self.root.join("history.jsonl")
    }
    /// `state.json` (engine-owned persisted readings, see [`crate::state`]).
    pub fn state(&self) -> PathBuf {
        self.root.join("state.json")
    }
    /// `analytics-cache.json` (owned by `codenotch-analytics`).
    pub fn analytics_cache(&self) -> PathBuf {
        self.root.join("analytics-cache.json")
    }
    /// `auth_token`: the hook server token (owned by `codenotch-activity`).
    pub fn hook_token(&self) -> PathBuf {
        self.root.join("auth_token")
    }
    /// `teams-outbox.jsonl`: Teams events waiting to be sent (owned by `codenotch-teams`;
    /// exists only while the device is paired and Teams sync is on).
    pub fn teams_outbox(&self) -> PathBuf {
        self.root.join("teams-outbox.jsonl")
    }
    /// `teams.json`: Teams pairing state and cursors, no secret (owned by `codenotch-teams`;
    /// exists only once the device is paired).
    pub fn teams_state(&self) -> PathBuf {
        self.root.join("teams.json")
    }
    /// `logs\`
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    /// `glyphs\`: user glyph overrides `<provider id>.svg|png`.
    pub fn glyphs_dir(&self) -> PathBuf {
        self.root.join("glyphs")
    }
    /// Create the data folder and the logs folder if missing.
    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.logs_dir())
    }
}

impl Default for Paths {
    fn default() -> Self {
        Self::resolve()
    }
}

/// Write `bytes` to `path` atomically: write a uniquely named temp file
/// (`<path>.<pid>-<n>.tmp`) in the same folder, flush, rename over. The previous file is
/// untouched if anything fails before the rename, and the temp file is removed on failure.
/// Concurrent writers never share a temp file (the last rename wins).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_checked(path, bytes, || Ok(()))
}

/// [`write_atomic`] with a last check: `before_rename` runs after the temp file is complete and
/// just before the rename; an `Err` from it aborts (temp file removed, `path` untouched) and is
/// returned. Used to refuse overwriting a file that changed since it was read.
pub fn write_atomic_checked(
    path: &Path,
    bytes: &[u8],
    before_rename: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let (tmp, mut file) = create_temp_beside(path)?;
    let written = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        before_rename()?;
        // std::fs::rename replaces an existing destination on Windows (MOVEFILE_REPLACE_EXISTING).
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Temp file name for `path`: `<path>.<pid>-<n>.tmp` (`n` from a process-wide counter).
fn temp_name(path: &Path, n: u64) -> PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}-{n}.tmp", std::process::id()));
    PathBuf::from(tmp)
}

/// Create (exclusively) a fresh temp file beside `path`.
fn create_temp_beside(path: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut last_err = None;
    for _ in 0..16 {
        let tmp = temp_name(path, COUNTER.fetch_add(1, Ordering::Relaxed));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => return Ok((tmp, file)),
            // Left over by a crashed run with a recycled pid: try the next number.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last_err = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::other("cannot create a temporary file")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_builds_every_path_under_root() {
        let p = Paths::at("X:/data");
        assert_eq!(p.config(), PathBuf::from("X:/data/config.gpui.json"));
        assert_eq!(p.history(), PathBuf::from("X:/data/history.jsonl"));
        assert_eq!(p.logs_dir(), PathBuf::from("X:/data/logs"));
        assert!(p.state().ends_with("state.json"));
        assert_eq!(
            p.teams_outbox(),
            PathBuf::from("X:/data/teams-outbox.jsonl")
        );
        assert_eq!(p.teams_state(), PathBuf::from("X:/data/teams.json"));
    }

    #[test]
    fn env_override_wins() {
        // Only test in this crate that touches the process environment.
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var(DATA_DIR_ENV, dir.path());
        let p = Paths::resolve();
        std::env::remove_var(DATA_DIR_ENV);
        assert_eq!(p.root(), dir.path());
        assert!(Paths::resolve().root().ends_with(APP_DIR_NAME));
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("a.json");
        write_atomic(&file, b"one").unwrap();
        write_atomic(&file, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two");
        assert!(!dir.path().join("sub").join("a.json.tmp").exists());
        assert_eq!(
            std::fs::read_dir(dir.path().join("sub")).unwrap().count(),
            1,
            "no temp file left"
        );
    }

    #[test]
    fn atomic_write_temp_names_are_unique_and_concurrent_writes_do_not_clash() {
        let p = Path::new("C:/x/settings.json");
        let (a, b) = (temp_name(p, 1), temp_name(p, 2));
        assert_ne!(a, b);
        assert_eq!(a.parent(), p.parent());
        assert!(
            a.to_string_lossy().ends_with(".tmp") && a.to_string_lossy().contains("settings.json.")
        );

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let file = file.clone();
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        // Renames may briefly fail on Windows while another rename holds the
                        // target; what matters is that no write corrupts another's temp file.
                        let _ = write_atomic(&file, format!("writer {i}").repeat(500).as_bytes());
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let text = std::fs::read_to_string(&file).unwrap();
        let first = &text[..8];
        assert_eq!(text, first.repeat(500), "content is one writer's, whole");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no temp file left"
        );
    }

    #[test]
    fn atomic_write_check_failure_aborts_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.json");
        write_atomic(&file, b"old").unwrap();
        let err = write_atomic_checked(&file, b"new", || Err(std::io::Error::other("changed")))
            .unwrap_err();
        assert_eq!(err.to_string(), "changed");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "temp file removed"
        );
        // Failure before the temp file can be written (directory in the way): nothing left.
        let blocked = dir.path().join("dir");
        std::fs::create_dir(&blocked).unwrap();
        assert!(write_atomic(&blocked, b"x").is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
