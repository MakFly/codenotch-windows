//! Claude session state machine. Pure: every method takes `now` (unix ms).

use codenotch_core::hooks::HookEvent;
use codenotch_core::model::{ActivityState, SessionInfo};
use std::collections::HashMap;
use std::path::PathBuf;

/// Store capacity; the session heard from longest ago is evicted.
pub const MAX_SESSIONS: usize = 200;
/// Watcher events are ignored for a session whose last hook event is younger than this.
pub const HOOK_PRIORITY_MS: i64 = 5 * 60_000;
/// Running with no event for this long → idle.
pub const RUNNING_STALE_MS: i64 = 30 * 60_000;
/// Idle sessions are removed after this.
pub const IDLE_EXPIRY_MS: i64 = 10 * 60_000;
/// Done / waiting sessions are removed after this.
pub const DONE_EXPIRY_MS: i64 = 24 * 3_600_000;
/// Sweep period.
pub const SWEEP_EVERY_MS: i64 = 30_000;
/// "Seen clears it" check period (only while a session is done).
pub const SEEN_EVERY_MS: i64 = 1_500;

const CWD_MAX: usize = 1024;
const PROMPT_MAX: usize = 120;
const ATTN_MAX: usize = 200;
const CMD_MAX: usize = 60;

/// Where an event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Hook,
    Watch,
}

/// One normalised event (hook POST or watcher push).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEvent {
    pub kind: HookEvent,
    pub source: Source,
    pub session_id: String,
    pub cwd: Option<String>,
    pub prompt: Option<String>,
    pub message: Option<String>,
    pub tool: Option<String>,
    pub command: Option<String>,
    pub model: Option<String>,
    /// Claude profile folder the session belongs to, when known.
    pub profile_dir: Option<PathBuf>,
    /// Process id given by the hook (0 = unknown).
    pub ppid: u32,
}

impl SessionEvent {
    pub fn new(kind: HookEvent, source: Source, session_id: impl Into<String>) -> Self {
        let id: String = session_id.into();
        Self {
            kind,
            source,
            session_id: if id.trim().is_empty() {
                "unknown".into()
            } else {
                id
            },
            cwd: None,
            prompt: None,
            message: None,
            tool: None,
            command: None,
            model: None,
            profile_dir: None,
            ppid: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub state: ActivityState,
    /// When the current state began (ms).
    pub since: i64,
    /// When the current run began (ms).
    pub started: i64,
    /// Frozen run length when done (ms).
    pub total: Option<i64>,
    pub prompt: Option<String>,
    pub attn: Option<String>,
    pub last_tool: Option<String>,
    pub ppid: u32,
    pub profile_dir: Option<PathBuf>,
    /// Last event of any source (ms).
    pub last_event: i64,
    /// Last hook event (ms).
    pub last_hook: Option<i64>,
}

impl Session {
    pub fn info(&self) -> SessionInfo {
        let detail = match self.state {
            ActivityState::Waiting => self.attn.clone().or_else(|| Some("Waiting on you".into())),
            ActivityState::Working => self.last_tool.clone().or_else(|| self.prompt.clone()),
            _ => self.prompt.clone(),
        };
        SessionInfo {
            id: self.id.clone(),
            title: self.title.clone(),
            cwd: self.cwd.clone(),
            model: self.model.clone(),
            state: self.state,
            since: self.since.div_euclid(1000),
            cost: None,
            detail,
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        let mut out: String = t.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn clean(v: &Option<String>, max: usize) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| truncate(s, max))
}

/// `<last cwd component> · <first 4 chars of id>`
pub fn title_for(cwd: Option<&str>, id: &str) -> String {
    let leaf = cwd
        .map(|c| c.trim_end_matches(['\\', '/']))
        .and_then(|c| c.rsplit(['\\', '/']).next())
        .filter(|l| !l.is_empty())
        .unwrap_or("Claude");
    let short: String = id.chars().take(4).collect();
    format!("{leaf} · {short}")
}

/// Process view used by "seen clears it".
pub trait ForegroundProbe {
    /// Foreground window's process id and the parent/name table, taken together.
    fn snapshot(&self) -> Option<ProcessView>;
}

/// Foreground pid + `pid → (parent pid, exe name)`.
#[derive(Debug, Clone, Default)]
pub struct ProcessView {
    pub foreground: u32,
    pub procs: HashMap<u32, (u32, String)>,
}

impl ProcessView {
    fn chain_contains(&self, start: u32, target: u32) -> bool {
        let mut pid = start;
        for _ in 0..64 {
            if pid == 0 {
                return false;
            }
            if pid == target {
                return true;
            }
            match self.procs.get(&pid) {
                Some((parent, _)) if *parent != pid => pid = *parent,
                _ => return false,
            }
        }
        false
    }
    fn foreground_name(&self) -> String {
        self.procs
            .get(&self.foreground)
            .map(|(_, n)| n.to_ascii_lowercase())
            .unwrap_or_default()
    }
}

/// All Claude sessions.
#[derive(Debug, Default)]
pub struct SessionStore {
    sessions: HashMap<String, Session>,
    last_sweep: i64,
    last_seen_check: i64,
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Session> {
        self.sessions.values()
    }

    /// Apply one event. Returns true when something visible changed.
    pub fn apply(&mut self, ev: &SessionEvent, now: i64) -> bool {
        if ev.kind == HookEvent::Ping {
            return false;
        }
        if ev.source == Source::Watch {
            if let Some(s) = self.sessions.get(&ev.session_id) {
                if s.last_hook.is_some_and(|h| now - h < HOOK_PRIORITY_MS) {
                    return false;
                }
            }
        }
        if ev.kind == HookEvent::SessionEnd {
            return self.sessions.remove(&ev.session_id).is_some();
        }
        let is_new = !self.sessions.contains_key(&ev.session_id);
        if is_new {
            self.evict_for_insert();
            let cwd = clean(&ev.cwd, CWD_MAX);
            self.sessions.insert(
                ev.session_id.clone(),
                Session {
                    id: ev.session_id.clone(),
                    title: title_for(cwd.as_deref(), &ev.session_id),
                    cwd,
                    model: None,
                    state: ActivityState::Idle,
                    since: now,
                    started: now,
                    total: None,
                    prompt: None,
                    attn: None,
                    last_tool: None,
                    ppid: 0,
                    profile_dir: None,
                    last_event: now,
                    last_hook: None,
                },
            );
        }
        let Some(s) = self.sessions.get_mut(&ev.session_id) else {
            return false;
        };
        let before = (
            s.state,
            s.last_tool.clone(),
            s.attn.clone(),
            s.prompt.clone(),
            s.model.clone(),
            s.cwd.clone(),
        );

        s.last_event = now;
        if ev.source == Source::Hook {
            s.last_hook = Some(now);
        }
        if ev.ppid != 0 {
            s.ppid = ev.ppid;
        }
        if ev.profile_dir.is_some() {
            s.profile_dir = ev.profile_dir.clone();
        }
        if let Some(cwd) = clean(&ev.cwd, CWD_MAX) {
            if s.cwd.as_deref() != Some(cwd.as_str()) {
                s.title = title_for(Some(&cwd), &s.id);
                s.cwd = Some(cwd);
            }
        }
        if let Some(m) = clean(&ev.model, 80) {
            s.model = Some(m);
        }

        let set_state = |s: &mut Session, st: ActivityState| {
            if s.state != st {
                s.state = st;
                s.since = now;
            }
        };
        match ev.kind {
            HookEvent::SessionStart => {
                if s.state != ActivityState::Working {
                    set_state(s, ActivityState::Idle);
                    s.attn = None;
                }
            }
            HookEvent::Running => {
                if s.state != ActivityState::Working {
                    s.started = now;
                    s.total = None;
                }
                set_state(s, ActivityState::Working);
                s.attn = None;
                if let Some(p) = clean(&ev.prompt, PROMPT_MAX) {
                    s.prompt = Some(p);
                    s.last_tool = None;
                }
                if let Some(tool) = clean(&ev.tool, 40) {
                    s.last_tool = Some(match clean(&ev.command, CMD_MAX) {
                        Some(cmd) => format!("{tool}: {cmd}"),
                        None => tool,
                    });
                }
            }
            HookEvent::Attention => {
                set_state(s, ActivityState::Waiting);
                s.attn = clean(&ev.message, ATTN_MAX).or_else(|| Some("Waiting on you".into()));
            }
            HookEvent::Done => {
                if s.state != ActivityState::Done {
                    s.total = Some(now - s.started);
                }
                set_state(s, ActivityState::Done);
                s.attn = None;
                s.last_tool = None;
            }
            HookEvent::SessionEnd | HookEvent::Ping => {}
        }
        is_new
            || before
                != (
                    s.state,
                    s.last_tool.clone(),
                    s.attn.clone(),
                    s.prompt.clone(),
                    s.model.clone(),
                    s.cwd.clone(),
                )
    }

    fn evict_for_insert(&mut self) {
        while self.sessions.len() >= MAX_SESSIONS {
            let Some(oldest) = self
                .sessions
                .values()
                .min_by_key(|s| s.last_event)
                .map(|s| s.id.clone())
            else {
                return;
            };
            self.sessions.remove(&oldest);
        }
    }

    /// Timers (sweep). Runs at most every [`SWEEP_EVERY_MS`] unless `force`.
    pub fn sweep(&mut self, now: i64, force: bool) -> bool {
        if !force && now - self.last_sweep < SWEEP_EVERY_MS {
            return false;
        }
        self.last_sweep = now;
        let mut changed = false;
        for s in self.sessions.values_mut() {
            if s.state == ActivityState::Working && now - s.last_event >= RUNNING_STALE_MS {
                s.state = ActivityState::Idle;
                s.since = now;
                s.last_tool = None;
                changed = true;
            }
        }
        let before = self.sessions.len();
        self.sessions.retain(|_, s| match s.state {
            ActivityState::Idle => now - s.since.max(s.last_event) < IDLE_EXPIRY_MS,
            ActivityState::Done | ActivityState::Waiting => now - s.since < DONE_EXPIRY_MS,
            ActivityState::Working => true,
        });
        changed || self.sessions.len() != before
    }

    pub fn any_done(&self) -> bool {
        self.sessions
            .values()
            .any(|s| s.state == ActivityState::Done)
    }

    /// "Seen clears it": done → idle when the user is looking at the session's window.
    /// Runs at most every [`SEEN_EVERY_MS`] unless `force`, and only while a session is done.
    pub fn acknowledge_seen(&mut self, probe: &dyn ForegroundProbe, now: i64, force: bool) -> bool {
        if !self.any_done() || (!force && now - self.last_seen_check < SEEN_EVERY_MS) {
            return false;
        }
        self.last_seen_check = now;
        let Some(view) = probe.snapshot() else {
            return false;
        };
        if view.foreground == 0 {
            return false;
        }
        let fg_name = view.foreground_name();
        let name_says_claude = fg_name.contains("claude") && !fg_name.contains("codenotch");
        let mut changed = false;
        for s in self
            .sessions
            .values_mut()
            .filter(|s| s.state == ActivityState::Done)
        {
            let seen = if s.ppid == 0 {
                name_says_claude
            } else {
                view.chain_contains(s.ppid, view.foreground)
            };
            if seen {
                s.state = ActivityState::Idle;
                s.since = now;
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ActivityState::*;

    fn ev(kind: HookEvent, id: &str) -> SessionEvent {
        let mut e = SessionEvent::new(kind, Source::Hook, id);
        e.cwd = Some(r"C:\work\proj".into());
        e
    }
    fn watch(kind: HookEvent, id: &str) -> SessionEvent {
        SessionEvent::new(kind, Source::Watch, id)
    }

    struct Probe(Option<ProcessView>);
    impl ForegroundProbe for Probe {
        fn snapshot(&self) -> Option<ProcessView> {
            self.0.clone()
        }
    }
    fn view(fg: u32, procs: &[(u32, u32, &str)]) -> Probe {
        Probe(Some(ProcessView {
            foreground: fg,
            procs: procs
                .iter()
                .map(|(p, pp, n)| (*p, (*pp, n.to_string())))
                .collect(),
        }))
    }

    #[test]
    fn new_session_title_and_initial_idle() {
        let mut st = SessionStore::new();
        assert!(st.apply(&ev(HookEvent::SessionStart, "abcdef"), 1000));
        let s = st.get("abcdef").unwrap();
        assert_eq!(s.state, Idle);
        assert_eq!(s.title, "proj · abcd");
        assert_eq!(title_for(None, "xy"), "Claude · xy");
        assert_eq!(title_for(Some("/a/b/"), "1234567"), "b · 1234");
        assert_eq!(
            SessionEvent::new(HookEvent::Running, Source::Hook, " ").session_id,
            "unknown"
        );
    }

    #[test]
    fn every_transition() {
        let mut st = SessionStore::new();
        let mut run = ev(HookEvent::Running, "s");
        run.prompt = Some("x".repeat(300));
        assert!(st.apply(&run, 1_000));
        let s = st.get("s").unwrap();
        assert_eq!((s.state, s.started, s.since), (Working, 1_000, 1_000));
        assert_eq!(s.prompt.as_ref().unwrap().chars().count(), PROMPT_MAX);

        // Tool use keeps the run start, records the tool.
        let mut tool = ev(HookEvent::Running, "s");
        tool.tool = Some("Bash".into());
        tool.command = Some("cargo build ".to_string() + &"y".repeat(100));
        assert!(st.apply(&tool, 2_000));
        let s = st.get("s").unwrap();
        assert_eq!(s.started, 1_000);
        assert!(s
            .last_tool
            .as_ref()
            .unwrap()
            .starts_with("Bash: cargo build"));
        assert_eq!(s.info().detail, s.last_tool.clone());
        // Same event again: no visible change.
        assert!(!st.apply(&tool, 2_500));

        // session_start while running keeps running.
        st.apply(&ev(HookEvent::SessionStart, "s"), 2_600);
        assert_eq!(st.get("s").unwrap().state, Working);

        let mut att = ev(HookEvent::Attention, "s");
        att.message = Some("Claude needs your permission to use Bash".into());
        assert!(st.apply(&att, 3_000));
        let s = st.get("s").unwrap();
        assert_eq!(s.state, Waiting);
        assert_eq!(
            s.info().detail.as_deref(),
            Some("Claude needs your permission to use Bash")
        );
        assert_eq!(s.info().since, 3);

        // Back to running clears attn and starts a new run (it was not running).
        assert!(st.apply(&ev(HookEvent::Running, "s"), 4_000));
        let s = st.get("s").unwrap();
        assert_eq!((s.state, s.attn.clone(), s.started), (Working, None, 4_000));

        assert!(st.apply(&ev(HookEvent::Done, "s"), 10_000));
        let s = st.get("s").unwrap();
        assert_eq!(
            (s.state, s.total, s.attn.clone()),
            (Done, Some(6_000), None)
        );

        // session_start on a done session → idle.
        assert!(st.apply(&ev(HookEvent::SessionStart, "s"), 11_000));
        assert_eq!(st.get("s").unwrap().state, Idle);

        assert!(st.apply(&ev(HookEvent::SessionEnd, "s"), 12_000));
        assert!(st.get("s").is_none());
        assert!(!st.apply(&ev(HookEvent::SessionEnd, "s"), 12_000));
        assert!(!st.apply(&ev(HookEvent::Ping, "s"), 12_000));
    }

    #[test]
    fn watcher_yields_to_recent_hooks() {
        let mut st = SessionStore::new();
        st.apply(&ev(HookEvent::Running, "s"), 0);
        assert!(!st.apply(&watch(HookEvent::Done, "s"), HOOK_PRIORITY_MS - 1));
        assert_eq!(st.get("s").unwrap().state, Working);
        assert!(st.apply(&watch(HookEvent::Done, "s"), HOOK_PRIORITY_MS));
        assert_eq!(st.get("s").unwrap().state, Done);
        // Watch-only sessions are driven by the watcher.
        assert!(st.apply(&watch(HookEvent::Running, "w"), 0));
        assert!(st.apply(&watch(HookEvent::Attention, "w"), 1));
        assert_eq!(st.get("w").unwrap().state, Waiting);
    }

    #[test]
    fn sweep_timers() {
        let mut st = SessionStore::new();
        st.apply(&ev(HookEvent::Running, "run"), 0);
        st.apply(&ev(HookEvent::SessionStart, "idle"), 0);
        st.apply(&ev(HookEvent::Done, "done"), 0);
        st.apply(&ev(HookEvent::Attention, "att"), 0);

        assert!(!st.sweep(SWEEP_EVERY_MS - 1, false), "rate limited");
        st.sweep(IDLE_EXPIRY_MS - 1, true);
        assert!(st.get("idle").is_some());
        st.sweep(IDLE_EXPIRY_MS, true);
        assert!(st.get("idle").is_none(), "idle removed after 10 min");
        assert_eq!(st.get("run").unwrap().state, Working);

        st.sweep(RUNNING_STALE_MS, true);
        assert_eq!(
            st.get("run").unwrap().state,
            Idle,
            "running → idle after 30 min silence"
        );
        st.sweep(RUNNING_STALE_MS + IDLE_EXPIRY_MS, true);
        assert!(st.get("run").is_none());

        st.sweep(DONE_EXPIRY_MS - 1, true);
        assert!(st.get("done").is_some() && st.get("att").is_some());
        st.sweep(DONE_EXPIRY_MS, true);
        assert!(st.get("done").is_none() && st.get("att").is_none());
    }

    #[test]
    fn capacity_evicts_least_recent() {
        let mut st = SessionStore::new();
        for i in 0..MAX_SESSIONS {
            st.apply(
                &ev(HookEvent::SessionStart, &format!("s{i}")),
                i as i64 + 10,
            );
        }
        st.apply(&ev(HookEvent::Running, "s0"), 10_000); // s0 refreshed, s1 is now oldest
        st.apply(&ev(HookEvent::Running, "new"), 10_001);
        assert_eq!(st.len(), MAX_SESSIONS);
        assert!(st.get("s0").is_some() && st.get("s1").is_none() && st.get("new").is_some());
    }

    #[test]
    fn seen_clears_done_via_ancestry_or_name() {
        let mut st = SessionStore::new();
        let mut a = ev(HookEvent::Done, "a");
        a.ppid = 300; // claude.exe, child of pwsh 200, child of terminal 100
        st.apply(&a, 0);
        st.apply(&watch(HookEvent::Done, "b"), 0); // no ppid

        // Foreground elsewhere: nothing.
        let other = view(
            999,
            &[
                (300, 200, "claude.exe"),
                (200, 100, "pwsh.exe"),
                (100, 1, "WindowsTerminal.exe"),
                (999, 1, "chrome.exe"),
            ],
        );
        assert!(!st.acknowledge_seen(&other, 10_000, true));
        // Rate limit.
        let term = view(
            100,
            &[
                (300, 200, "claude.exe"),
                (200, 100, "pwsh.exe"),
                (100, 1, "WindowsTerminal.exe"),
            ],
        );
        assert!(!st.acknowledge_seen(&term, 10_000 + SEEN_EVERY_MS - 1, false));
        assert!(st.acknowledge_seen(&term, 10_000 + SEEN_EVERY_MS, false));
        assert_eq!(st.get("a").unwrap().state, Idle);
        assert_eq!(
            st.get("b").unwrap().state,
            Done,
            "ppid 0 needs a claude foreground"
        );

        let codenotch = view(5, &[(5, 1, "codenotch.exe")]);
        assert!(!st.acknowledge_seen(&codenotch, 20_000, true));
        let desktop = view(7, &[(7, 1, "Claude.exe")]);
        assert!(st.acknowledge_seen(&desktop, 30_000, true));
        assert_eq!(st.get("b").unwrap().state, Idle);
        // Nothing done → probe not consulted.
        assert!(!st.acknowledge_seen(&Probe(None), 40_000, true));
    }

    #[test]
    fn ancestry_walk_survives_cycles() {
        let v = ProcessView {
            foreground: 9,
            procs: [(1, (2, "a".into())), (2, (1, "b".into()))]
                .into_iter()
                .collect(),
        };
        assert!(!v.chain_contains(1, 9));
        assert!(v.chain_contains(2, 1));
    }
}
