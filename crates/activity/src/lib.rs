//! # codenotch-activity (owner: agent E)
//!
//! "Working" / "waiting on you" detection (data-spec §9):
//! * Hook server on `127.0.0.1:settings.hookPort` (protocol: `codenotch_core::hooks`), token in
//!   `Paths::hook_token()` (32 random bytes, 64 hex chars, created on first start). Runs while
//!   `settings.hooksEnabled` is on; a taken port is reported by [`ActivityHandle::server_status`].
//! * Session state machine (§9.2), transcript watcher fallback (§9.3), other providers' busy
//!   state (§9.4: Codex/Cursor/OpenCode SQLite via `rusqlite`, Antigravity transcript mtime).
//! * Hooks installer for every Claude profile's `settings.json` (§9.1): merge-only, idempotent,
//!   atomic write, one-time `.codenotch.bak` backup, refuses invalid JSON.
//!
//! Threading: [`start`] spawns its own threads (hook server + one per connection, Claude
//! watcher tick every 400 ms, other-provider poller every 2 s) and calls `on_change` from them
//! whenever the report changes (cheap callback; the engine just forwards it to its own thread).

mod installer;
mod json;
mod others;
pub mod server;
pub mod sessions;
mod token;
mod transcript;
mod win;

pub use server::ServerStatus;

use codenotch_core::model::{ActivityState, ProviderKind, SessionInfo};
use codenotch_core::{Account, Env, Error, Paths, Result, Settings};
use sessions::{SessionEvent, SessionStore};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

/// Activity of one provider profile.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProviderActivity {
    pub provider: ProviderKind,
    /// Profile folder the sessions belong to (Claude: the config dir the transcript lives in).
    /// `None` = not attributable (applies to every account of that provider).
    pub profile_dir: Option<PathBuf>,
    pub state: ActivityState,
    pub sessions: Vec<SessionInfo>,
}

/// Everything activity knows right now.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ActivityReport {
    pub at: i64,
    pub items: Vec<ProviderActivity>,
}

/// Default profile folder name under `home` for providers whose sessions are attributed.
fn default_profile_dir(kind: &ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::Claude => Some(".claude"),
        ProviderKind::Codex => Some(".codex"),
        _ => None,
    }
}

/// Case-insensitive, separator-insensitive path equality (Windows semantics).
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

fn sort_sessions(sessions: &mut [SessionInfo]) {
    sessions.sort_by(|a, b| {
        b.state
            .rank()
            .cmp(&a.state.rank())
            .then(b.since.cmp(&a.since))
            .then(a.id.cmp(&b.id))
    });
}

impl ActivityReport {
    /// Aggregated state and sessions for one configured account (match by provider, then by
    /// profile folder; `Env::profile_dir` gives the account's folder). Items without a profile
    /// folder apply to every account of their provider.
    pub fn for_account(&self, account: &Account, env: &Env) -> (ActivityState, Vec<SessionInfo>) {
        let own = default_profile_dir(&account.provider).map(|d| env.profile_dir(account, d));
        let mut sessions: Vec<SessionInfo> = Vec::new();
        for item in self.items.iter().filter(|i| i.provider == account.provider) {
            let matches = match (&item.profile_dir, &own) {
                (None, _) | (_, None) => true,
                (Some(p), Some(o)) => same_path(p, o),
            };
            if matches {
                for s in &item.sessions {
                    if !sessions.iter().any(|x| x.id == s.id) {
                        sessions.push(s.clone());
                    }
                }
            }
        }
        sort_sessions(&mut sessions);
        (
            ActivityState::aggregate(sessions.iter().map(|s| s.state)),
            sessions,
        )
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Claude items from the session store, grouped by profile folder.
fn claude_items(store: &SessionStore) -> Vec<ProviderActivity> {
    let mut groups: Vec<ProviderActivity> = Vec::new();
    for s in store.iter() {
        let idx = match groups
            .iter()
            .position(|g| match (&g.profile_dir, &s.profile_dir) {
                (None, None) => true,
                (Some(a), Some(b)) => same_path(a, b),
                _ => false,
            }) {
            Some(i) => i,
            None => {
                groups.push(ProviderActivity {
                    provider: ProviderKind::Claude,
                    profile_dir: s.profile_dir.clone(),
                    ..ProviderActivity::default()
                });
                groups.len() - 1
            }
        };
        groups[idx].sessions.push(s.info());
    }
    for g in &mut groups {
        sort_sessions(&mut g.sessions);
        g.state = ActivityState::aggregate(g.sessions.iter().map(|s| s.state));
    }
    groups.sort_by_key(|g| {
        g.profile_dir
            .as_ref()
            .map(|p| p.to_string_lossy().to_lowercase())
    });
    groups
}

/// Stop flag that wakes sleepers immediately.
#[derive(Default)]
struct Stopper {
    stopped: Mutex<bool>,
    cv: Condvar,
}

impl Stopper {
    fn stop(&self) {
        *lock(&self.stopped) = true;
        self.cv.notify_all();
    }
    /// Sleep up to `d`; true when stopped.
    fn wait(&self, d: Duration) -> bool {
        let guard = lock(&self.stopped);
        if *guard {
            return true;
        }
        match self.cv.wait_timeout(guard, d) {
            Ok((g, _)) => *g,
            Err(e) => *e.into_inner().0,
        }
    }
}

struct Shared {
    env: Env,
    store: Mutex<SessionStore>,
    others: Mutex<Vec<ProviderActivity>>,
    report: Mutex<ActivityReport>,
    publish_lock: Mutex<()>,
    on_change: Box<dyn Fn(ActivityReport) + Send + Sync>,
    accounts: Mutex<Vec<Account>>,
    generation: AtomicU64,
    server_status: Mutex<ServerStatus>,
    stopper: Stopper,
}

impl Shared {
    /// Rebuild the report; call `on_change` when its items changed.
    fn publish(&self) {
        let _guard = lock(&self.publish_lock);
        let mut items = claude_items(&lock(&self.store));
        items.extend(lock(&self.others).iter().cloned());
        let report = {
            let mut current = lock(&self.report);
            if current.items == items {
                return;
            }
            current.items = items;
            current.at = now_ms().div_euclid(1000);
            current.clone()
        };
        (self.on_change)(report);
    }

    fn apply(&self, ev: &SessionEvent) {
        let changed = lock(&self.store).apply(ev, now_ms());
        if changed {
            self.publish();
        }
    }
}

/// Running activity services. Dropping it stops them.
pub struct ActivityHandle {
    shared: Arc<Shared>,
    paths: Paths,
    threads: Vec<JoinHandle<()>>,
    server: Mutex<Option<server::ServerHandle>>,
    server_cfg: Mutex<Option<(bool, u16)>>,
}

impl ActivityHandle {
    /// Latest report.
    pub fn report(&self) -> ActivityReport {
        lock(&self.shared.report).clone()
    }

    /// Hook server state (disabled, listening, port in use, failed).
    pub fn server_status(&self) -> ServerStatus {
        lock(&self.shared.server_status).clone()
    }

    /// Apply changed settings (port, hooks switch, enabled accounts).
    pub fn update_settings(&self, settings: &Settings, accounts: &[Account]) {
        *lock(&self.shared.accounts) = accounts.to_vec();
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.configure_server(settings);
    }

    fn configure_server(&self, settings: &Settings) {
        let wanted = (settings.hooks_enabled, settings.hook_port);
        let mut cfg = lock(&self.server_cfg);
        if *cfg == Some(wanted) {
            return;
        }
        *cfg = Some(wanted);
        let mut slot = lock(&self.server);
        if let Some(old) = slot.take() {
            old.stop();
        }
        let status = if !settings.hooks_enabled {
            ServerStatus::Disabled
        } else {
            match token::load_or_create(&self.paths.hook_token()) {
                Err(e) => {
                    log::warn!("hook server: {e}");
                    ServerStatus::Failed("cannot create the hook token file".into())
                }
                Ok(tok) => {
                    let weak = Arc::downgrade(&self.shared);
                    let sink: server::EventSink = Arc::new(move |ev| {
                        if let Some(shared) = weak.upgrade() {
                            shared.apply(&ev);
                        }
                    });
                    match server::start(settings.hook_port, tok, sink) {
                        Ok(h) => {
                            let port = h.port;
                            *slot = Some(h);
                            ServerStatus::Listening(port)
                        }
                        Err(status) => status,
                    }
                }
            }
        };
        *lock(&self.shared.server_status) = status;
    }

    pub fn stop(self) {}

    fn shutdown(&mut self) {
        self.shared.stopper.stop();
        if let Some(s) = lock(&self.server).take() {
            s.stop();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for ActivityHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

const TICK: Duration = Duration::from_millis(400);

fn claude_thread(shared: Arc<Shared>) {
    let mut watcher = transcript::Watcher::new(Vec::new());
    let mut seen_gen = u64::MAX;
    let probe = win::SystemProbe;
    loop {
        let generation = shared.generation.load(Ordering::SeqCst);
        if generation != seen_gen {
            seen_gen = generation;
            let accounts = lock(&shared.accounts).clone();
            watcher.set_roots(transcript::roots(&shared.env, &accounts));
        }
        let now = now_ms();
        let events = watcher.tick(now);
        let changed = {
            let mut store = lock(&shared.store);
            let mut changed = false;
            for ev in &events {
                changed |= store.apply(ev, now);
            }
            changed |= store.sweep(now, false);
            changed |= store.acknowledge_seen(&probe, now, false);
            changed
        };
        if changed {
            shared.publish();
        }
        if shared.stopper.wait(TICK) {
            return;
        }
    }
}

fn others_thread(shared: Arc<Shared>) {
    let mut poller = others::OtherPoller::new(shared.env.clone());
    let mut seen_gen = u64::MAX;
    loop {
        let generation = shared.generation.load(Ordering::SeqCst);
        if generation != seen_gen {
            seen_gen = generation;
            let mut kinds: Vec<ProviderKind> = Vec::new();
            for a in lock(&shared.accounts).iter().filter(|a| a.enabled) {
                if a.provider != ProviderKind::Claude && !kinds.contains(&a.provider) {
                    kinds.push(a.provider.clone());
                }
            }
            poller.set_providers(kinds);
        }
        let items = poller.poll(now_ms());
        let changed = {
            let mut cur = lock(&shared.others);
            if *cur != items {
                *cur = items;
                true
            } else {
                false
            }
        };
        if changed {
            shared.publish();
        }
        if shared.stopper.wait(others::POLL_EVERY) {
            return;
        }
    }
}

/// Start the hook server, watcher and pollers.
pub fn start(
    paths: &Paths,
    env: &Env,
    settings: &Settings,
    accounts: &[Account],
    on_change: Box<dyn Fn(ActivityReport) + Send + Sync>,
) -> Result<ActivityHandle> {
    let shared = Arc::new(Shared {
        env: env.clone(),
        store: Mutex::new(SessionStore::new()),
        others: Mutex::new(Vec::new()),
        report: Mutex::new(ActivityReport {
            at: now_ms().div_euclid(1000),
            items: Vec::new(),
        }),
        publish_lock: Mutex::new(()),
        on_change,
        accounts: Mutex::new(accounts.to_vec()),
        generation: AtomicU64::new(0),
        server_status: Mutex::new(ServerStatus::Disabled),
        stopper: Stopper::default(),
    });
    let mut handle = ActivityHandle {
        shared: shared.clone(),
        paths: paths.clone(),
        threads: Vec::new(),
        server: Mutex::new(None),
        server_cfg: Mutex::new(None),
    };
    // The helper reads the token file; make sure it exists from the first start.
    if let Err(e) = token::load_or_create(&paths.hook_token()) {
        log::warn!("activity: {e}");
    }
    handle.configure_server(settings);
    let s1 = shared.clone();
    let t1 = std::thread::Builder::new()
        .name("codenotch-activity-claude".into())
        .spawn(move || claude_thread(s1))
        .map_err(|e| Error::io("spawn activity thread", e))?;
    handle.threads.push(t1);
    let s2 = shared;
    let t2 = std::thread::Builder::new()
        .name("codenotch-activity-others".into())
        .spawn(move || others_thread(s2))
        .map_err(|e| Error::io("spawn activity thread", e))?;
    handle.threads.push(t2);
    Ok(handle)
}

/// `codenotch-hook.exe` next to the running executable (where the installer expects it).
pub fn default_hook_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = if cfg!(windows) {
        "codenotch-hook.exe"
    } else {
        "codenotch-hook"
    };
    Some(exe.with_file_name(name))
}

/// Claude Code profile folders the installer works on (`~\.claude`, `~\.claude-*`,
/// `CLAUDE_CONFIG_DIR`; existing folders only).
pub fn claude_profile_dirs(env: &Env) -> Vec<PathBuf> {
    installer::claude_profile_dirs(env)
}

fn for_each_profile(
    dirs: &[PathBuf],
    what: &str,
    mut f: impl FnMut(&Path) -> Result<bool>,
) -> Result<()> {
    if dirs.is_empty() {
        return Err(Error::unsupported("no Claude Code profile folder found"));
    }
    let mut first_err: Option<Error> = None;
    for dir in dirs {
        match f(dir) {
            Ok(changed) => log::info!(
                "hooks {what}: {} ({})",
                dir.display(),
                if changed { "updated" } else { "unchanged" }
            ),
            Err(e) => {
                log::warn!("hooks {what}: {e}");
                first_err.get_or_insert(e);
            }
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// Merge our hook entries into `settings.json` of every Claude profile folder (see
/// [`claude_profile_dirs`]); a backup is kept the first time a file is changed. `hook_exe` =
/// absolute path of `codenotch-hook.exe`.
pub fn install_hooks(env: &Env, hook_exe: &Path) -> Result<()> {
    install_hooks_in(&claude_profile_dirs(env), hook_exe)
}

/// [`install_hooks`] for explicit profile folders (e.g. accounts with a custom `profilePath`).
pub fn install_hooks_in(profile_dirs: &[PathBuf], hook_exe: &Path) -> Result<()> {
    installer::check_exe(hook_exe)?;
    for_each_profile(profile_dirs, "install", |d| {
        installer::install_in(d, hook_exe)
    })
}

/// Remove only our entries (marker `codenotch-hook` or legacy names).
pub fn remove_hooks(env: &Env) -> Result<()> {
    remove_hooks_in(&claude_profile_dirs(env))
}

/// [`remove_hooks`] for explicit profile folders.
pub fn remove_hooks_in(profile_dirs: &[PathBuf]) -> Result<()> {
    for_each_profile(profile_dirs, "remove", installer::remove_in)
}

/// Whether our hooks are present (on every event) in at least one Claude profile's settings.
pub fn hooks_installed(env: &Env) -> bool {
    claude_profile_dirs(env)
        .iter()
        .any(|d| installer::installed_in(d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::hooks::HookEvent;
    use sessions::Source;

    #[test]
    fn empty_report_is_idle() {
        let env = Env::for_test(Path::new("."));
        let a = Account::new(ProviderKind::Claude, "c");
        assert_eq!(
            ActivityReport::default().for_account(&a, &env).0,
            ActivityState::Idle
        );
    }

    fn session(id: &str, state: ActivityState, since: i64) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            title: id.into(),
            state,
            since,
            ..SessionInfo::default()
        }
    }

    #[test]
    fn for_account_matches_profile_folders() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let default = Account::new(ProviderKind::Claude, "default");
        let mut work = Account::new(ProviderKind::Claude, "work");
        work.profile_path = env.home.join(".claude-work").to_string_lossy().into_owned();
        let codex = Account::new(ProviderKind::Codex, "x");
        let report = ActivityReport {
            at: 0,
            items: vec![
                ProviderActivity {
                    provider: ProviderKind::Claude,
                    profile_dir: Some(PathBuf::from(
                        env.home
                            .join(".CLAUDE")
                            .to_string_lossy()
                            .replace('\\', "/")
                            + "/",
                    )),
                    state: ActivityState::Working,
                    sessions: vec![session("a", ActivityState::Working, 5)],
                },
                ProviderActivity {
                    provider: ProviderKind::Claude,
                    profile_dir: Some(env.home.join(".claude-work")),
                    state: ActivityState::Waiting,
                    sessions: vec![session("b", ActivityState::Waiting, 1)],
                },
                ProviderActivity {
                    provider: ProviderKind::Claude,
                    profile_dir: None,
                    state: ActivityState::Done,
                    sessions: vec![session("desk", ActivityState::Done, 9)],
                },
                ProviderActivity {
                    provider: ProviderKind::Codex,
                    profile_dir: None,
                    state: ActivityState::Working,
                    sessions: vec![session("c", ActivityState::Working, 1)],
                },
            ],
        };
        let (st, ss) = report.for_account(&default, &env);
        assert_eq!(st, ActivityState::Working);
        assert_eq!(
            ss.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["a", "desk"]
        );
        let (st, ss) = report.for_account(&work, &env);
        assert_eq!(st, ActivityState::Waiting);
        assert_eq!(
            ss.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["b", "desk"]
        );
        assert_eq!(report.for_account(&codex, &env).0, ActivityState::Working);
        assert_eq!(
            report
                .for_account(&Account::new(ProviderKind::Cursor, "c"), &env)
                .0,
            ActivityState::Idle
        );
    }

    #[test]
    fn claude_items_group_by_profile() {
        let mut store = SessionStore::new();
        let mut e = SessionEvent::new(HookEvent::Running, Source::Hook, "s1");
        e.profile_dir = Some(PathBuf::from(r"C:\u\.claude"));
        store.apply(&e, 1000);
        let mut e2 = SessionEvent::new(HookEvent::Attention, Source::Hook, "s2");
        e2.profile_dir = Some(PathBuf::from(r"c:/u/.claude"));
        store.apply(&e2, 2000);
        store.apply(
            &SessionEvent::new(HookEvent::Done, Source::Watch, "s3"),
            3000,
        );
        let items = claude_items(&store);
        assert_eq!(items.len(), 2);
        let with_profile = items.iter().find(|i| i.profile_dir.is_some()).unwrap();
        assert_eq!(with_profile.state, ActivityState::Waiting);
        assert_eq!(with_profile.sessions[0].id, "s2", "waiting first");
    }

    #[test]
    fn start_stop_promptly_with_port_in_use_reported() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::at(d.path().join("data"));
        let env = Env::for_test(d.path());
        // Occupy an ephemeral port so the server cannot bind it.
        let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = blocker.local_addr().unwrap().port();
        let settings = Settings {
            hooks_enabled: true,
            hook_port: port,
            ..Settings::default()
        };
        let h = start(&paths, &env, &settings, &[], Box::new(|_| {})).unwrap();
        assert_eq!(h.server_status(), ServerStatus::PortInUse(port));
        assert!(paths.hook_token().is_file());
        // Switching to an ephemeral free port and back off.
        let mut s2 = settings.clone();
        s2.hook_port = 0;
        h.update_settings(&s2, &[]);
        assert!(matches!(h.server_status(), ServerStatus::Listening(p) if p != 0));
        s2.hooks_enabled = false;
        h.update_settings(&s2, &[]);
        assert_eq!(h.server_status(), ServerStatus::Disabled);
        let t0 = std::time::Instant::now();
        h.stop();
        assert!(t0.elapsed() < Duration::from_secs(1), "stop must be prompt");
    }

    #[test]
    fn installer_entry_points_on_temp_profiles() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let exe = d.path().join("bin").join("codenotch-hook.exe");
        assert!(install_hooks(&env, &exe).is_err(), "no profile yet");
        assert!(!hooks_installed(&env));
        std::fs::create_dir_all(env.home.join(".claude")).unwrap();
        std::fs::create_dir_all(env.home.join(".claude-work")).unwrap();
        std::fs::write(
            env.home.join(".claude-work").join("settings.json"),
            "{ broken",
        )
        .unwrap();
        assert!(
            install_hooks(&env, &exe).is_err(),
            "broken profile reported"
        );
        assert!(
            hooks_installed(&env),
            "the valid profile was still installed"
        );
        assert_eq!(
            std::fs::read_to_string(env.home.join(".claude-work").join("settings.json")).unwrap(),
            "{ broken"
        );
        std::fs::write(env.home.join(".claude-work").join("settings.json"), "{}").unwrap();
        install_hooks(&env, &exe).unwrap();
        remove_hooks(&env).unwrap();
        assert!(!hooks_installed(&env));
        assert!(install_hooks(&env, Path::new("relative.exe")).is_err());
    }
}
