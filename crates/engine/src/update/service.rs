//! The updater thread: periodic and on-demand checks, background download, install, relaunch.
//!
//! It owns its state and reports every change through a callback ([`Report`]); the engine
//! (or the demo command thread) copies it into the snapshot. It never blocks the caller:
//! [`Updater`]'s methods only post a message.
//!
//! Modes ([`UpdateMode`]):
//! * `notify` — check at start (after `first_check`) and every `every`; a newer version is
//!   reported as `Available`; [`Updater::install`] (the toast's button) downloads, installs and
//!   restarts.
//! * `auto` — same checks; a newer version is downloaded in the background and installed when
//!   the app is idle ([`Updater::idle`]), or at the next start.
//! * `off` — no automatic check; [`Updater::check`] still works.
//!
//! A version below the manifest's `min_version` is *forced*: downloaded and installed at once,
//! whatever the mode.

use super::manifest::{self, Offer, Outcome};
use super::stage::{self, Pending, Staged};
use super::swap::{self, Retry, APP_EXE};
use super::version::Version;
use super::*;
use codenotch_core::config::UpdateMode;
use codenotch_core::model::UpdateState;
use codenotch_core::HttpClient;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Starts the new exe: `(exe, args)`. The real one spawns a process; tests record the call.
pub type Relaunch = Arc<dyn Fn(&Path, &[String]) -> std::io::Result<()> + Send + Sync>;

/// What the updater tells its owner.
#[derive(Debug, Clone, PartialEq)]
pub enum Report {
    /// New state; `last_check` = time of the last completed check.
    State {
        state: UpdateState,
        last_check: Option<i64>,
    },
    /// An automatic check found a version not reported before (the owner may show a Windows
    /// notification). Not sent for forced updates (they install at once).
    NewVersion { version: String },
    /// The new version was started: quit this process now.
    Restart,
}

/// Everything the updater needs.
pub struct UpdaterSetup {
    pub http: Arc<dyn HttpClient>,
    /// Version this process runs (what the manifest is compared with).
    pub current: String,
    /// The running exe; its folder is the install folder. `None`: install refused.
    pub exe: Option<PathBuf>,
    /// `<data dir>\updates`.
    pub work_dir: PathBuf,
    pub mode: UpdateMode,
    /// Manifest URL from the settings (empty = default); see `url_override`.
    pub manifest_url: String,
    /// `CODENOTCH_UPDATE_URL`: wins over the setting.
    pub url_override: Option<String>,
    /// `file://` and local paths accepted (tests, debug builds, `CODENOTCH_UPDATE_URL`).
    pub allow_file: bool,
    /// Install even from a cargo `target\` folder (`CODENOTCH_UPDATE_ALLOW_DEV=1`).
    pub allow_dev: bool,
    /// First automatic check after start (`None` = never automatic).
    pub first_check: Option<Duration>,
    /// Period of the automatic checks.
    pub every: Duration,
    pub clock: crate::Clock,
    pub relaunch: Relaunch,
    /// Arguments for the relaunched app; `--updated-from=` / `--wait-pid=` are added.
    pub args: Vec<String>,
    /// Retries of the file swap.
    pub retry: Retry,
}

enum In {
    Check { manual: bool },
    Install,
    Settings { mode: UpdateMode, url: String },
    Idle(bool),
    Stop,
}

/// Handle on the updater thread. Dropping it stops the thread (an in-flight download finishes
/// first, its result ignored).
pub struct Updater {
    tx: Sender<In>,
}

impl Updater {
    pub fn start(setup: UpdaterSetup, report: Box<dyn Fn(Report) + Send>) -> Updater {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("codenotch-update".into())
            .spawn(move || Worker::new(setup, report).run(rx));
        if let Err(err) = spawned {
            log::error!("cannot start the updater: {err}");
        }
        Updater { tx }
    }

    /// Check now (`manual`: every outcome is reported to the user).
    pub fn check(&self, manual: bool) {
        let _ = self.tx.send(In::Check { manual });
    }
    /// The user asked to install the update on offer.
    pub fn install(&self) {
        let _ = self.tx.send(In::Install);
    }
    pub fn settings(&self, mode: UpdateMode, url: &str) {
        let _ = self.tx.send(In::Settings {
            mode,
            url: url.to_string(),
        });
    }
    /// The app is idle (no agent working, no settings window in use).
    pub fn idle(&self, idle: bool) {
        let _ = self.tx.send(In::Idle(idle));
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        let _ = self.tx.send(In::Stop);
    }
}

/// Relaunch arguments: `args` without our own flags, then `--updated-from=` and `--wait-pid=`.
pub fn relaunch_args(args: &[String], from: &str, pid: u32) -> Vec<String> {
    let mut out: Vec<String> = args
        .iter()
        .filter(|a| !a.starts_with("--updated-from=") && !a.starts_with("--wait-pid="))
        .cloned()
        .collect();
    out.push(format!("--updated-from={from}"));
    out.push(format!("--wait-pid={pid}"));
    out
}

/// The real relaunch: start `exe` detached from this process (its own working folder; the
/// test-only `CODENOTCH_UPDATE_PRETEND_VERSION` is not passed on).
pub fn spawn_relaunch() -> Relaunch {
    Arc::new(|exe: &Path, args: &[String]| {
        let mut cmd = std::process::Command::new(exe);
        cmd.args(args)
            .env_remove(PRETEND_VERSION_ENV)
            .stdin(std::process::Stdio::null());
        if let Some(dir) = exe.parent() {
            cmd.current_dir(dir);
        }
        cmd.spawn().map(|_| ())
    })
}

struct Worker {
    s: UpdaterSetup,
    report: Box<dyn Fn(Report) + Send>,
    state: UpdateState,
    last_check: Option<i64>,
    offer: Option<Offer>,
    staged: Option<Staged>,
    idle: bool,
    /// The user asked to install (cleared on failure).
    install_wanted: bool,
    /// Version whose install did not take effect: never installed automatically again.
    bad_version: Option<String>,
    /// Versions already reported by `NewVersion`.
    announced: Vec<String>,
    next_check: Option<Instant>,
    /// Leftover `.old` files to delete again later.
    cleanup_retry: Option<(Instant, u32)>,
    /// Restart requested: ignore everything else.
    done: bool,
}

impl Worker {
    fn new(s: UpdaterSetup, report: Box<dyn Fn(Report) + Send>) -> Self {
        Worker {
            s,
            report,
            state: UpdateState::Idle,
            last_check: None,
            offer: None,
            staged: None,
            idle: false,
            install_wanted: false,
            bad_version: None,
            announced: Vec::new(),
            next_check: None,
            cleanup_retry: None,
            done: false,
        }
    }

    fn url(&self) -> String {
        match self.s.url_override.as_deref().map(str::trim) {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => match self.s.manifest_url.trim() {
                "" => codenotch_core::DEFAULT_UPDATE_MANIFEST_URL.to_string(),
                u => u.to_string(),
            },
        }
    }

    fn install_dir(&self) -> Option<PathBuf> {
        self.s.exe.as_ref()?.parent().map(Path::to_path_buf)
    }

    fn set(&mut self, state: UpdateState) {
        if state != self.state {
            self.state = state.clone();
            (self.report)(Report::State {
                state,
                last_check: self.last_check,
            });
        }
    }

    fn run(mut self, rx: Receiver<In>) {
        self.start();
        loop {
            let now = Instant::now();
            let mut wait = Duration::from_secs(3600);
            if let Some(at) = self.next_check {
                wait = wait.min(at.saturating_duration_since(now));
            }
            if let Some((at, _)) = self.cleanup_retry {
                wait = wait.min(at.saturating_duration_since(now));
            }
            match rx.recv_timeout(wait) {
                Ok(In::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(_) if self.done => {}
                Ok(In::Check { manual }) => self.check(manual),
                Ok(In::Install) => {
                    self.install_wanted = true;
                    self.proceed();
                }
                Ok(In::Settings { mode, url }) => self.settings(mode, url),
                Ok(In::Idle(idle)) => {
                    let became = idle && !self.idle;
                    self.idle = idle;
                    if became {
                        self.proceed();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if self.done {
                continue;
            }
            let now = Instant::now();
            if self.next_check.is_some_and(|at| now >= at) {
                self.next_check = Some(now + self.s.every);
                self.check(false);
            }
            if self.cleanup_retry.is_some_and(|(at, _)| now >= at) {
                self.retry_cleanup();
            }
        }
    }

    /// Start-up: outcome of the last install, leftovers, a package staged earlier.
    fn start(&mut self) {
        let work = self.s.work_dir.clone();
        if let Some(Pending { from, to }) = stage::take_pending(&work) {
            let took = Version::parse(&self.s.current) == Version::parse(&to);
            if took {
                log::info!("update: now running {to} (updated from {from})");
            } else {
                log::warn!(
                    "update: the install of {to} did not take effect (running {})",
                    self.s.current
                );
                self.bad_version = Some(to);
                // Reported like a manual check: the user should hear about it.
                self.set(UpdateState::Failed {
                    message: MSG_NOT_TAKEN.into(),
                    manual: true,
                });
            }
        }
        if let Some(dir) = self.install_dir() {
            if !swap::cleanup_install_dir(&dir).is_empty() {
                // The previous process may still be exiting.
                self.cleanup_retry = Some((Instant::now() + Duration::from_secs(5), 12));
            }
        }
        let staged = stage::find_staged(&work, &self.s.current)
            .filter(|s| self.bad_version.as_deref() != Some(s.version.as_str()));
        stage::cleanup(&work, staged.as_ref().map(|s| s.version.as_str()));
        if let Some(staged) = staged {
            log::info!("update: {} was downloaded earlier", staged.version);
            self.set(UpdateState::Ready {
                version: staged.version.clone(),
                forced: false,
            });
            self.staged = Some(staged);
        }
        if self.s.mode != UpdateMode::Off {
            self.next_check = self.s.first_check.map(|d| Instant::now() + d);
        }
    }

    fn retry_cleanup(&mut self) {
        let Some((_, left)) = self.cleanup_retry.take() else {
            return;
        };
        let Some(dir) = self.install_dir() else {
            return;
        };
        let remaining = swap::cleanup_install_dir(&dir);
        if !remaining.is_empty() && left > 1 {
            self.cleanup_retry = Some((Instant::now() + Duration::from_secs(5), left - 1));
        } else if !remaining.is_empty() {
            log::warn!("update: could not delete {}", remaining.join(", "));
        }
    }

    fn settings(&mut self, mode: UpdateMode, url: String) {
        let was = self.s.mode;
        self.s.mode = mode;
        self.s.manifest_url = url;
        if mode == UpdateMode::Off {
            self.next_check = None;
        } else if was == UpdateMode::Off || self.next_check.is_none() {
            let delay = self.s.first_check.unwrap_or(self.s.every);
            self.next_check = Some(Instant::now() + delay);
        }
        if mode == UpdateMode::Auto && was != UpdateMode::Auto {
            self.proceed();
        }
    }

    fn check(&mut self, manual: bool) {
        if matches!(
            self.state,
            UpdateState::Downloading { .. } | UpdateState::Installing { .. }
        ) {
            return;
        }
        self.set(UpdateState::Checking { manual });
        let url = self.url();
        let result = fetch::get_manifest(&*self.s.http, &url, self.s.allow_file)
            .and_then(|bytes| manifest::parse(&bytes))
            .and_then(|m| manifest::evaluate(&m, &self.s.current));
        let now = (self.s.clock)();
        self.last_check = Some(now);
        match result {
            Ok(Outcome::UpToDate) => {
                self.offer = None;
                self.set(UpdateState::UpToDate {
                    checked_at: now,
                    manual,
                });
            }
            Ok(Outcome::Available(offer)) => {
                log::info!(
                    "update: {} is available{}",
                    offer.version,
                    if offer.forced { " (required)" } else { "" }
                );
                if !manual && !offer.forced && !self.announced.contains(&offer.version) {
                    self.announced.push(offer.version.clone());
                    (self.report)(Report::NewVersion {
                        version: offer.version.clone(),
                    });
                }
                // A package already downloaded for it: say so instead of "available".
                let state = if self
                    .staged
                    .as_ref()
                    .is_some_and(|s| s.version == offer.version)
                    && !manual
                {
                    UpdateState::Ready {
                        version: offer.version.clone(),
                        forced: offer.forced,
                    }
                } else {
                    UpdateState::Available {
                        version: offer.version.clone(),
                        notes: offer.notes.clone(),
                        notes_url: offer.notes_url.clone(),
                        forced: offer.forced,
                        manual,
                    }
                };
                self.offer = Some(offer);
                self.set(state);
                self.proceed();
            }
            Err(message) => {
                log::warn!("update check failed: {message}");
                self.set(UpdateState::Failed { message, manual });
            }
        }
    }

    fn fail(&mut self, message: &str, manual: bool) {
        log::warn!("update: {message}");
        self.install_wanted = false;
        self.set(UpdateState::Failed {
            message: message.to_string(),
            manual,
        });
    }

    /// Refuse to replace a cargo build output (unless allowed). True = refused.
    fn refuse_dev(&mut self) -> bool {
        let dev = self.s.exe.as_deref().is_some_and(swap::is_dev_build);
        if dev && !self.s.allow_dev {
            log::warn!(
                "update: running from a cargo target folder; self-update disabled \
                 (set {ALLOW_DEV_ENV}=1 to allow it)"
            );
            let manual = self.install_wanted;
            self.fail(MSG_DEV_BUILD, manual);
            return true;
        }
        false
    }

    /// Download and/or install when the mode, the user or a forced update asks for it.
    fn proceed(&mut self) {
        if self.done {
            return;
        }
        if let Some(offer) = self.offer.clone() {
            let auto = self.s.mode == UpdateMode::Auto
                && self.bad_version.as_deref() != Some(offer.version.as_str());
            let wanted = offer.forced || self.install_wanted || auto;
            let have = self
                .staged
                .as_ref()
                .is_some_and(|s| s.version == offer.version);
            if wanted && !have {
                if self.refuse_dev() {
                    return;
                }
                self.download(&offer);
                if !self
                    .staged
                    .as_ref()
                    .is_some_and(|s| s.version == offer.version)
                {
                    return;
                }
            }
        }
        let Some(staged) = self.staged.clone() else {
            return;
        };
        let forced = self
            .offer
            .as_ref()
            .is_some_and(|o| o.forced && o.version == staged.version);
        let auto = self.s.mode == UpdateMode::Auto
            && self.idle
            && self.bad_version.as_deref() != Some(staged.version.as_str());
        if forced || self.install_wanted || auto {
            if self.refuse_dev() {
                return;
            }
            self.install(&staged, forced);
        } else {
            self.set(UpdateState::Ready {
                version: staged.version.clone(),
                forced,
            });
        }
    }

    fn download(&mut self, offer: &Offer) {
        let version = offer.version.clone();
        let forced = offer.forced;
        self.set(UpdateState::Downloading {
            version: version.clone(),
            progress: 0,
            forced,
        });
        let http = self.s.http.clone();
        let work = self.s.work_dir.clone();
        let allow_file = self.s.allow_file;
        let size = offer.asset.size.max(1);
        let mut last_pct = 0u8;
        let mut updates: Vec<u8> = Vec::new();
        let result = {
            let report = &self.report;
            let last_check = self.last_check;
            let mut progress = |done: u64, _total: Option<u64>| {
                let pct = ((done.min(size) * 100) / size) as u8;
                if pct != last_pct {
                    last_pct = pct;
                    updates.push(pct);
                    report(Report::State {
                        state: UpdateState::Downloading {
                            version: version.clone(),
                            progress: pct,
                            forced,
                        },
                        last_check,
                    });
                }
            };
            stage::stage_offer(&*http, offer, &work, allow_file, &mut progress)
        };
        if let Some(pct) = updates.last() {
            self.state = UpdateState::Downloading {
                version: version.clone(),
                progress: *pct,
                forced,
            };
        }
        match result {
            Ok(staged) => {
                log::info!("update: {} downloaded and verified", staged.version);
                stage::cleanup(&work, Some(&staged.version));
                self.staged = Some(staged);
            }
            Err(message) => {
                let manual = self.install_wanted;
                self.fail(&message, manual);
            }
        }
    }

    fn install(&mut self, staged: &Staged, forced: bool) {
        let Some(dir) = self.install_dir() else {
            self.fail(MSG_NO_INSTALL_DIR, true);
            return;
        };
        let _ = forced;
        self.set(UpdateState::Installing {
            version: staged.version.clone(),
        });
        let pending = Pending {
            from: self.s.current.clone(),
            to: staged.version.clone(),
        };
        if let Err(err) = stage::write_pending(&self.s.work_dir, &pending) {
            log::warn!("update: cannot write the pending marker: {}", err.kind());
        }
        let swapped = match swap::swap_in(&dir, &staged.files, self.s.retry) {
            Ok(s) => s,
            Err(msg) => {
                log::warn!("update: install failed: {msg}");
                stage::clear_pending(&self.s.work_dir);
                self.fail(MSG_INSTALL_FAILED, true);
                return;
            }
        };
        let exe = dir.join(APP_EXE);
        let args = relaunch_args(&self.s.args, &self.s.current, std::process::id());
        match (self.s.relaunch)(&exe, &args) {
            Ok(()) => {
                log::info!(
                    "update: {} installed in {}, restarting",
                    staged.version,
                    dir.display()
                );
                self.done = true;
                self.staged = None;
                (self.report)(Report::Restart);
            }
            Err(err) => {
                log::warn!("update: cannot start the new version: {}", err.kind());
                let errors = swap::rollback(&swapped, self.s.retry);
                if !errors.is_empty() {
                    log::error!("update: rollback incomplete: {}", errors.join("; "));
                }
                stage::clear_pending(&self.s.work_dir);
                self.bad_version = Some(staged.version.clone());
                self.fail(MSG_RESTART_FAILED, true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::{FakeHttp, HttpResponse};
    use std::sync::mpsc;
    use std::sync::Mutex;

    const MANIFEST: &str = "https://example.com/update.json";
    const ZIP: &str = "https://example.com/CodeNotch-1.1.0.zip";

    /// Recorded relaunches: `(exe, args)`.
    type Launched = Arc<Mutex<Vec<(PathBuf, Vec<String>)>>>;

    struct Rig {
        _tmp: tempfile::TempDir,
        install: PathBuf,
        work: PathBuf,
        launched: Launched,
        rx: mpsc::Receiver<Report>,
        updater: Updater,
    }

    fn manifest(min: &str) -> String {
        let zip = stage::tests_package();
        format!(
            r#"{{"version":"1.1.0","min_version":"{min}","notes":"n","notes_url":"https://github.com/r/v1.1.0",
                "windows_x64":{{"url":"{ZIP}","sha256":"{}","size":{}}}}}"#,
            stage::sha256_hex(&zip),
            zip.len()
        )
    }

    fn dirs(exe_dir: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join(exe_dir);
        let work = tmp.path().join("data").join("updates");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join(APP_EXE), b"MZ old app").unwrap();
        std::fs::write(install.join(swap::HOOK_EXE), b"MZ old hook").unwrap();
        (tmp, install, work)
    }

    fn rig(mode: UpdateMode, body: &str, relaunch_ok: bool, exe_dir: &str) -> Rig {
        let (tmp, install, work) = dirs(exe_dir);
        let http = FakeHttp::new()
            .on(MANIFEST, 200, body)
            .on_response(ZIP, HttpResponse::new(200, stage::tests_package()));
        let launched = Arc::new(Mutex::new(Vec::new()));
        let rec = launched.clone();
        let relaunch: Relaunch = Arc::new(move |exe: &Path, args: &[String]| {
            rec.lock().unwrap().push((exe.to_path_buf(), args.to_vec()));
            if relaunch_ok {
                Ok(())
            } else {
                Err(std::io::Error::other("cannot start"))
            }
        });
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let updater = Updater::start(
            UpdaterSetup {
                http: Arc::new(http),
                current: "1.0.0".into(),
                exe: Some(install.join(APP_EXE)),
                work_dir: work.clone(),
                mode,
                manifest_url: MANIFEST.into(),
                url_override: None,
                allow_file: false,
                allow_dev: false,
                first_check: None,
                every: Duration::from_secs(3600),
                clock: Arc::new(|| 1_800_000_000),
                relaunch,
                args: vec!["--silent".into(), "--wait-pid=1".into()],
                retry: Retry::NONE,
            },
            Box::new(move |r| {
                let _ = tx.lock().unwrap().send(r);
            }),
        );
        Rig {
            _tmp: tmp,
            install,
            work,
            launched,
            rx,
            updater,
        }
    }

    /// Reports until one matches (10 s at most).
    fn until(rx: &mpsc::Receiver<Report>, pred: impl Fn(&Report) -> bool) -> Vec<Report> {
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let r = rx
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no matching report in {seen:#?}"));
            let hit = pred(&r);
            seen.push(r);
            if hit {
                return seen;
            }
        }
    }

    fn state(r: &Report) -> Option<&UpdateState> {
        match r {
            Report::State { state, .. } => Some(state),
            _ => None,
        }
    }

    #[test]
    fn notify_mode_offers_then_installs_on_request() {
        let r = rig(UpdateMode::Notify, &manifest("1.0.0"), true, "CodeNotch");
        r.updater.check(true);
        let seen = until(&r.rx, |x| {
            matches!(state(x), Some(UpdateState::Available { .. }))
        });
        assert!(matches!(
            state(&seen[0]),
            Some(UpdateState::Checking { manual: true })
        ));
        assert!(matches!(
            state(seen.last().unwrap()),
            Some(UpdateState::Available { version, forced: false, manual: true, .. }) if version == "1.1.0"
        ));
        // Idle alone does nothing in notify mode.
        r.updater.idle(true);
        std::thread::sleep(Duration::from_millis(200));
        assert!(r.launched.lock().unwrap().is_empty());
        assert_eq!(
            std::fs::read(r.install.join(APP_EXE)).unwrap(),
            b"MZ old app"
        );

        r.updater.install();
        let seen = until(&r.rx, |x| *x == Report::Restart);
        assert!(seen.iter().any(|x| matches!(
            state(x),
            Some(UpdateState::Downloading { progress: 100, .. })
        )));
        assert!(seen
            .iter()
            .any(|x| matches!(state(x), Some(UpdateState::Installing { .. }))));
        assert_eq!(
            std::fs::read(r.install.join(APP_EXE)).unwrap(),
            b"MZ new app"
        );
        assert_eq!(
            std::fs::read(r.install.join(swap::HOOK_EXE)).unwrap(),
            b"MZ new hook"
        );
        assert!(r.install.join("codenotch.exe.old").is_file());
        let launched = r.launched.lock().unwrap().clone();
        assert_eq!(launched.len(), 1);
        assert_eq!(launched[0].0, r.install.join(APP_EXE));
        let args = &launched[0].1;
        assert_eq!(args[0], "--silent");
        assert_eq!(args[1], "--updated-from=1.0.0");
        assert!(args[2].starts_with("--wait-pid=") && args[2] != "--wait-pid=1");
        assert_eq!(args.len(), 3);
        assert_eq!(
            stage::take_pending(&r.work),
            Some(Pending {
                from: "1.0.0".into(),
                to: "1.1.0".into()
            })
        );
    }

    #[test]
    fn forced_update_installs_without_asking() {
        let r = rig(UpdateMode::Off, &manifest("1.0.1"), true, "CodeNotch");
        r.updater.check(false);
        let seen = until(&r.rx, |x| *x == Report::Restart);
        assert!(seen
            .iter()
            .any(|x| matches!(state(x), Some(UpdateState::Available { forced: true, .. }))));
        assert!(
            !seen.iter().any(|x| matches!(x, Report::NewVersion { .. })),
            "no notification for a forced update"
        );
        assert_eq!(
            std::fs::read(r.install.join(APP_EXE)).unwrap(),
            b"MZ new app"
        );
    }

    #[test]
    fn auto_mode_downloads_then_waits_for_idle() {
        let r = rig(UpdateMode::Auto, &manifest("1.0.0"), true, "CodeNotch");
        r.updater.check(false);
        let seen = until(&r.rx, |x| {
            matches!(state(x), Some(UpdateState::Ready { .. }))
        });
        assert!(seen
            .iter()
            .any(|x| matches!(x, Report::NewVersion { version } if version == "1.1.0")));
        assert!(r.launched.lock().unwrap().is_empty(), "not idle yet");
        r.updater.idle(true);
        until(&r.rx, |x| *x == Report::Restart);
        assert_eq!(
            std::fs::read(r.install.join(APP_EXE)).unwrap(),
            b"MZ new app"
        );
    }

    #[test]
    fn failed_relaunch_rolls_back() {
        let r = rig(UpdateMode::Notify, &manifest("1.0.1"), false, "CodeNotch");
        r.updater.check(false);
        let seen = until(&r.rx, |x| {
            matches!(state(x), Some(UpdateState::Failed { .. }))
        });
        assert!(matches!(
            state(seen.last().unwrap()),
            Some(UpdateState::Failed { message, .. }) if message == MSG_RESTART_FAILED
        ));
        assert_eq!(
            std::fs::read(r.install.join(APP_EXE)).unwrap(),
            b"MZ old app"
        );
        assert_eq!(
            std::fs::read(r.install.join(swap::HOOK_EXE)).unwrap(),
            b"MZ old hook"
        );
        assert!(!r.install.join("codenotch.exe.old").exists());
        assert_eq!(stage::take_pending(&r.work), None);
    }

    #[test]
    fn running_from_target_is_refused() {
        let r = rig(
            UpdateMode::Notify,
            &manifest("1.0.0"),
            true,
            r"repo\target\debug",
        );
        r.updater.check(true);
        until(&r.rx, |x| {
            matches!(state(x), Some(UpdateState::Available { .. }))
        });
        r.updater.install();
        let seen = until(&r.rx, |x| {
            matches!(state(x), Some(UpdateState::Failed { .. }))
        });
        assert!(matches!(
            state(seen.last().unwrap()),
            Some(UpdateState::Failed { message, manual: true }) if message == MSG_DEV_BUILD
        ));
        assert!(r.launched.lock().unwrap().is_empty());
        assert_eq!(
            std::fs::read(r.install.join(APP_EXE)).unwrap(),
            b"MZ old app"
        );
        assert!(
            !stage::version_dir(&r.work, "1.1.0").exists(),
            "nothing downloaded"
        );
    }

    #[test]
    fn up_to_date_and_failures_are_reported() {
        let up = r#"{"version":"1.0.0"}"#;
        let r = rig(UpdateMode::Notify, up, true, "CodeNotch");
        r.updater.check(true);
        let seen = until(&r.rx, |x| {
            matches!(state(x), Some(UpdateState::UpToDate { .. }))
        });
        assert!(matches!(
            seen.last().unwrap(),
            Report::State {
                state: UpdateState::UpToDate {
                    checked_at: 1_800_000_000,
                    manual: true
                },
                last_check: Some(1_800_000_000)
            }
        ));
        let r = rig(UpdateMode::Notify, "garbage", true, "CodeNotch");
        r.updater.check(false);
        let seen = until(&r.rx, |x| {
            matches!(state(x), Some(UpdateState::Failed { .. }))
        });
        assert!(matches!(
            state(seen.last().unwrap()),
            Some(UpdateState::Failed { message, manual: false }) if message == MSG_UNREADABLE
        ));
    }

    #[test]
    fn leftovers_are_cleaned_and_a_staged_package_survives_restarts() {
        let (_tmp, install, work) = dirs("CodeNotch");
        // Leftovers of an earlier update, a stale download and a complete package.
        std::fs::write(install.join("codenotch.exe.old"), b"old").unwrap();
        std::fs::create_dir_all(work.join("0.5.0")).unwrap();
        let zip = stage::tests_package();
        let offer = Offer {
            version: "1.1.0".into(),
            notes: String::new(),
            notes_url: None,
            forced: false,
            asset: manifest::Asset {
                url: ZIP.into(),
                sha256: stage::sha256_hex(&zip),
                size: zip.len() as u64,
            },
        };
        let http = FakeHttp::new().on_response(ZIP, HttpResponse::new(200, zip));
        stage::stage_offer(&http, &offer, &work, false, &mut |_, _| {}).unwrap();
        // A restart in auto mode: Ready at once, installed when idle.
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let launched: Launched = Arc::new(Mutex::new(Vec::new()));
        let rec = launched.clone();
        let updater = Updater::start(
            UpdaterSetup {
                http: Arc::new(FakeHttp::new()),
                current: "1.0.0".into(),
                exe: Some(install.join(APP_EXE)),
                work_dir: work.clone(),
                mode: UpdateMode::Auto,
                manifest_url: MANIFEST.into(),
                url_override: None,
                allow_file: false,
                allow_dev: false,
                first_check: None,
                every: Duration::from_secs(3600),
                clock: Arc::new(|| 0),
                relaunch: Arc::new(move |e: &Path, a: &[String]| -> std::io::Result<()> {
                    rec.lock().unwrap().push((e.to_path_buf(), a.to_vec()));
                    Ok(())
                }),
                args: Vec::new(),
                retry: Retry::NONE,
            },
            Box::new(move |x| {
                let _ = tx.lock().unwrap().send(x);
            }),
        );
        until(
            &rx,
            |x| matches!(state(x), Some(UpdateState::Ready { version, .. }) if version == "1.1.0"),
        );
        assert!(!install.join("codenotch.exe.old").exists(), ".old deleted");
        assert!(!work.join("0.5.0").exists(), "stale folder deleted");
        updater.idle(true);
        until(&rx, |x| *x == Report::Restart);
        assert_eq!(std::fs::read(install.join(APP_EXE)).unwrap(), b"MZ new app");
        assert_eq!(launched.lock().unwrap().len(), 1);
    }

    #[test]
    fn an_install_that_did_not_take_is_reported_and_not_retried_automatically() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("updates");
        stage::write_pending(
            &work,
            &Pending {
                from: "1.0.0".into(),
                to: "1.1.0".into(),
            },
        )
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let _updater = Updater::start(
            UpdaterSetup {
                http: Arc::new(FakeHttp::new().on(MANIFEST, 200, &manifest("1.0.0"))),
                current: "1.0.0".into(),
                exe: Some(tmp.path().join("app").join(APP_EXE)),
                work_dir: work.clone(),
                mode: UpdateMode::Auto,
                manifest_url: MANIFEST.into(),
                url_override: None,
                allow_file: false,
                allow_dev: false,
                first_check: Some(Duration::from_millis(10)),
                every: Duration::from_secs(3600),
                clock: Arc::new(|| 0),
                relaunch: Arc::new(|_: &Path, _: &[String]| -> std::io::Result<()> {
                    panic!("must not relaunch")
                }),
                args: Vec::new(),
                retry: Retry::NONE,
            },
            Box::new(move |x| {
                let _ = tx.lock().unwrap().send(x);
            }),
        );
        let seen = until(&rx, |x| {
            matches!(state(x), Some(UpdateState::Failed { .. }))
        });
        assert!(matches!(
            state(seen.last().unwrap()),
            Some(UpdateState::Failed { message, .. }) if message == MSG_NOT_TAKEN
        ));
        // The automatic check still offers it, but auto mode does not download it again.
        until(&rx, |x| {
            matches!(state(x), Some(UpdateState::Available { .. }))
        });
        std::thread::sleep(Duration::from_millis(200));
        assert!(!stage::version_dir(&work, "1.1.0").exists());
    }

    #[test]
    fn relaunch_args_replace_our_flags() {
        let args: Vec<String> = [
            "--demo",
            "--updated-from=0.1.0",
            "--wait-pid=3",
            "--settings",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            relaunch_args(&args, "1.0.0", 42),
            [
                "--demo",
                "--settings",
                "--updated-from=1.0.0",
                "--wait-pid=42"
            ]
        );
    }
}
