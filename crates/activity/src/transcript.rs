//! Claude transcript watcher: the fallback when hooks do not fire (desktop app,
//! hooks not installed). Polls file sizes/mtimes; no file-system notification dependency.
//!
//! * roots: `<profile>\projects` of every enabled Claude account, plus the desktop app's
//!   `local-agent-mode-sessions` folders (Roaming and MSIX `Packages\…\LocalCache\Roaming`);
//! * files: `*.jsonl`, not `audit.jsonl`, nothing under `subagents` (desktop roots: only under a
//!   `.claude` / `.claude-*` folder);
//! * a changed file is ingested at most every 800 ms (last 256 KiB, last 80 lines) and pushes
//!   `running`; after a quiet period that depends on the last entry it pushes `done` or
//!   `attention` once (2.5 s / 20 s / 2.5 s / 75 s / 5 min);
//! * the tree is re-walked every 4 s for new files modified within the last 10 minutes; files
//!   already quiet when a root is first scanned are seeded silently (no fake "done" at start-up).

use crate::sessions::{SessionEvent, Source};
use codenotch_core::hooks::HookEvent;
use codenotch_core::{Account, Env, ProviderKind};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const INGEST_MIN_GAP_MS: i64 = 800;
pub const TAIL_BYTES: u64 = 256 * 1024;
pub const TAIL_LINES: usize = 80;
pub const WALK_EVERY_MS: i64 = 4_000;
pub const RECENT_MS: i64 = 10 * 60_000;
const MAX_DEPTH: usize = 10;
const MAX_ENTRIES_PER_WALK: usize = 50_000;
const PROMPT_MAX: usize = 120;

/// A watched folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    pub path: PathBuf,
    /// Claude profile folder sessions under this root belong to (`None` = desktop app).
    pub profile: Option<PathBuf>,
    pub desktop: bool,
}

/// Roots for the current accounts (only enabled Claude accounts; desktop roots when there is at
/// least one).
pub fn roots(env: &Env, accounts: &[Account]) -> Vec<Root> {
    let mut out: Vec<Root> = Vec::new();
    for a in accounts
        .iter()
        .filter(|a| a.enabled && a.provider == ProviderKind::Claude)
    {
        let profile = env.profile_dir(a, ".claude");
        if out.iter().any(|r| {
            r.profile
                .as_ref()
                .is_some_and(|p| crate::same_path(p, &profile))
        }) {
            continue;
        }
        out.push(Root {
            path: profile.join("projects"),
            profile: Some(profile),
            desktop: false,
        });
    }
    if out.is_empty() {
        return out;
    }
    out.push(Root {
        path: env.appdata.join("Claude").join("local-agent-mode-sessions"),
        profile: None,
        desktop: true,
    });
    if let Ok(rd) = std::fs::read_dir(env.local_appdata.join("Packages")) {
        let mut pkgs: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().to_ascii_lowercase();
                n.contains("claude") || n.contains("anthropic")
            })
            .map(|e| {
                e.path()
                    .join("LocalCache")
                    .join("Roaming")
                    .join("Claude")
                    .join("local-agent-mode-sessions")
            })
            .collect();
        pkgs.sort();
        out.extend(pkgs.into_iter().map(|path| Root {
            path,
            profile: None,
            desktop: true,
        }));
    }
    out
}

/// What the last user/assistant entry of a transcript says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastKind {
    AssistantText,
    AssistantTool,
    UserInterrupt,
    User,
    Other,
}

impl LastKind {
    /// Quiet time after which the terminal event is pushed.
    pub fn quiet_ms(self) -> i64 {
        match self {
            LastKind::AssistantText | LastKind::UserInterrupt => 2_500,
            LastKind::AssistantTool => 20_000,
            LastKind::User => 75_000,
            LastKind::Other => 5 * 60_000,
        }
    }
    pub fn terminal(self) -> HookEvent {
        match self {
            LastKind::AssistantTool => HookEvent::Attention,
            _ => HookEvent::Done,
        }
    }
}

/// Parsed transcript tail.
#[derive(Debug, Clone, PartialEq)]
pub struct Tail {
    pub kind: LastKind,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
}

fn text_blocks(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            let texts: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            (!texts.is_empty()).then(|| texts.join(" "))
        }
        _ => None,
    }
}

fn has_block(content: &Value, kind: &str) -> bool {
    content.as_array().is_some_and(|bs| {
        bs.iter()
            .any(|b| b.get("type").and_then(Value::as_str) == Some(kind))
    })
}

fn truncate(s: &str, max: usize) -> String {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= max {
        t
    } else {
        let mut o: String = t.chars().take(max - 1).collect();
        o.push('…');
        o
    }
}

/// Classify the last lines of a transcript.
pub fn parse_tail(text: &str) -> Tail {
    let mut tail = Tail {
        kind: LastKind::Other,
        session_id: None,
        cwd: None,
        model: None,
        prompt: None,
    };
    let mut kind: Option<LastKind> = None;
    for line in text
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(TAIL_LINES)
    {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if tail.session_id.is_none() {
            tail.session_id = v
                .get("sessionId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from);
        }
        if tail.cwd.is_none() {
            tail.cwd = v
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from);
        }
        let content = v
            .get("message")
            .and_then(|m| m.get("content"))
            .cloned()
            .unwrap_or(Value::Null);
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                if kind.is_none() {
                    kind = Some(if has_block(&content, "tool_use") {
                        LastKind::AssistantTool
                    } else {
                        LastKind::AssistantText
                    });
                }
                if tail.model.is_none() {
                    tail.model = v
                        .get("message")
                        .and_then(|m| m.get("model"))
                        .and_then(Value::as_str)
                        .filter(|m| !m.is_empty() && !m.starts_with('<'))
                        .map(String::from);
                }
            }
            Some("user") => {
                let text = text_blocks(&content);
                if kind.is_none() {
                    let interrupted = text
                        .as_deref()
                        .is_some_and(|t| t.to_ascii_lowercase().contains("[request interrupted"));
                    kind = Some(if interrupted {
                        LastKind::UserInterrupt
                    } else {
                        LastKind::User
                    });
                }
                if tail.prompt.is_none() && !has_block(&content, "tool_result") {
                    tail.prompt = text
                        .filter(|t| !t.trim().is_empty())
                        .map(|t| truncate(&t, PROMPT_MAX));
                }
            }
            _ => {}
        }
    }
    tail.kind = kind.unwrap_or(LastKind::Other);
    tail
}

/// Read the last [`TAIL_BYTES`] of a file (lossy UTF-8, first partial line dropped).
pub fn read_tail(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.take(TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    Some(if start > 0 {
        text.split_once('\n')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default()
    } else {
        text
    })
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn accepted(path: &Path, desktop: bool) -> bool {
    let is_jsonl = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("jsonl"));
    let name_ok = path
        .file_name()
        .is_some_and(|n| !n.eq_ignore_ascii_case("audit.jsonl"));
    let comps: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    let no_sub = !comps.iter().any(|c| c == "subagents");
    let claude_dir = !desktop
        || comps
            .iter()
            .any(|c| c == ".claude" || c.starts_with(".claude-"));
    is_jsonl && name_ok && no_sub && claude_dir
}

#[derive(Debug)]
struct FileState {
    root: usize,
    size: u64,
    mtime: i64,
    dirty: bool,
    last_ingest: i64,
    tail: Option<Tail>,
    pushed: bool,
}

/// Polling watcher. Call [`Watcher::tick`] every ~400 ms.
#[derive(Debug, Default)]
pub struct Watcher {
    roots: Vec<Root>,
    seeded: Vec<bool>,
    files: HashMap<PathBuf, FileState>,
    last_walk: Option<i64>,
}

impl Watcher {
    pub fn new(roots: Vec<Root>) -> Self {
        let mut w = Self::default();
        w.set_roots(roots);
        w
    }

    /// Replace the roots (files of removed roots are forgotten; new roots are seeded).
    pub fn set_roots(&mut self, roots: Vec<Root>) {
        if roots == self.roots {
            return;
        }
        let old = std::mem::take(&mut self.roots);
        let old_seeded = std::mem::take(&mut self.seeded);
        self.seeded = roots
            .iter()
            .map(|r| {
                old.iter()
                    .position(|o| o == r)
                    .map(|i| old_seeded[i])
                    .unwrap_or(false)
            })
            .collect();
        let remap: Vec<Option<usize>> = old
            .iter()
            .map(|o| roots.iter().position(|r| r == o))
            .collect();
        self.files
            .retain(|_, f| remap.get(f.root).copied().flatten().is_some());
        for f in self.files.values_mut() {
            if let Some(Some(n)) = remap.get(f.root) {
                f.root = *n;
            }
        }
        self.roots = roots;
        self.last_walk = None;
    }

    /// One poll. Returns the events to apply to the session store.
    pub fn tick(&mut self, now: i64) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        if self.last_walk.is_none_or(|t| now - t >= WALK_EVERY_MS) {
            self.last_walk = Some(now);
            self.walk(now, &mut events);
        }
        let paths: Vec<PathBuf> = self.files.keys().cloned().collect();
        for path in paths {
            let Some(f) = self.files.get_mut(&path) else {
                continue;
            };
            match std::fs::metadata(&path) {
                Ok(meta) => {
                    let (size, mtime) = (meta.len(), mtime_ms(&meta));
                    if size != f.size || mtime != f.mtime {
                        f.size = size;
                        f.mtime = mtime;
                        f.dirty = true;
                    }
                }
                Err(_) => {
                    self.files.remove(&path);
                    continue;
                }
            }
            if f.dirty && now - f.last_ingest >= INGEST_MIN_GAP_MS {
                f.dirty = false;
                f.last_ingest = now;
                if let Some(text) = read_tail(&path) {
                    let tail = parse_tail(&text);
                    let root = &self.roots[f.root];
                    events.push(event_for(HookEvent::Running, &path, &tail, root));
                    f.tail = Some(tail);
                    f.pushed = false;
                }
            }
            if !f.pushed && !f.dirty {
                if let Some(tail) = &f.tail {
                    if now - f.mtime > tail.kind.quiet_ms() {
                        f.pushed = true;
                        events.push(event_for(
                            tail.kind.terminal(),
                            &path,
                            tail,
                            &self.roots[f.root],
                        ));
                    }
                }
            }
        }
        // Forget files that went quiet long ago (re-discovered by the walk when written again).
        self.files
            .retain(|_, f| !(f.pushed && now - f.mtime > RECENT_MS));
        events
    }

    fn walk(&mut self, now: i64, events: &mut Vec<SessionEvent>) {
        let mut budget = MAX_ENTRIES_PER_WALK;
        for idx in 0..self.roots.len() {
            let root = self.roots[idx].clone();
            let initial = !self.seeded[idx];
            if !root.path.is_dir() {
                continue;
            }
            let mut found = Vec::new();
            walk_dir(&root.path, 0, &mut budget, &mut |p, meta| {
                if accepted(p, root.desktop) && now - mtime_ms(meta) <= RECENT_MS {
                    found.push((p.to_path_buf(), meta.len(), mtime_ms(meta)));
                }
            });
            for (path, size, mtime) in found {
                if self.files.contains_key(&path) {
                    continue;
                }
                let mut st = FileState {
                    root: idx,
                    size,
                    mtime,
                    dirty: !initial,
                    last_ingest: i64::MIN / 2,
                    tail: None,
                    pushed: false,
                };
                if initial {
                    // Seed: learn the tail; only a session that is still active right now is shown.
                    if let Some(text) = read_tail(&path) {
                        let tail = parse_tail(&text);
                        st.last_ingest = now;
                        if now - mtime > tail.kind.quiet_ms() {
                            st.pushed = true;
                        } else {
                            events.push(event_for(HookEvent::Running, &path, &tail, &root));
                        }
                        st.tail = Some(tail);
                    }
                }
                self.files.insert(path, st);
            }
            self.seeded[idx] = true;
        }
    }
}

fn walk_dir(
    dir: &Path,
    depth: usize,
    budget: &mut usize,
    visit: &mut dyn FnMut(&Path, &std::fs::Metadata),
) {
    if depth > MAX_DEPTH || *budget == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let Ok(ft) = entry.file_type() else { continue };
        let path = entry.path();
        if ft.is_dir() {
            if !entry.file_name().eq_ignore_ascii_case("subagents") {
                walk_dir(&path, depth + 1, budget, visit);
            }
        } else if ft.is_file() {
            if let Ok(meta) = entry.metadata() {
                visit(&path, &meta);
            }
        }
    }
}

fn event_for(kind: HookEvent, path: &Path, tail: &Tail, root: &Root) -> SessionEvent {
    let id = tail
        .session_id
        .clone()
        .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let mut ev = SessionEvent::new(kind, Source::Watch, id);
    ev.cwd = tail.cwd.as_ref().map(|c| {
        if c.to_ascii_lowercase().contains("local-agent-mode-sessions") || root.desktop {
            "Claude desktop".to_string()
        } else {
            c.clone()
        }
    });
    ev.model = tail.model.clone();
    if kind == HookEvent::Running {
        ev.prompt = tail.prompt.clone();
    }
    if kind == HookEvent::Attention {
        ev.message = Some("Waiting for your approval".into());
    }
    ev.profile_dir = root.profile.clone();
    ev
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    const NOW: i64 = 1_800_000_000_000;

    fn user(text: &str) -> String {
        serde_json::json!({"type":"user","sessionId":"sess-1","cwd":"C:\\code\\app","message":{"role":"user","content":text}}).to_string()
    }
    fn asst(tool: bool) -> String {
        let content = if tool {
            serde_json::json!([{"type":"text","text":"Running"},{"type":"tool_use","id":"t","name":"Bash","input":{}}])
        } else {
            serde_json::json!([{"type":"text","text":"All done"}])
        };
        serde_json::json!({"type":"assistant","sessionId":"sess-1","message":{"model":"claude-opus-4","content":content}}).to_string()
    }
    fn tool_result() -> String {
        serde_json::json!({"type":"user","sessionId":"sess-1","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}).to_string()
    }

    #[test]
    fn tail_classification() {
        let t = parse_tail(&[user("fix the bug"), asst(false)].join("\n"));
        assert_eq!(t.kind, LastKind::AssistantText);
        assert_eq!(t.session_id.as_deref(), Some("sess-1"));
        assert_eq!(t.model.as_deref(), Some("claude-opus-4"));
        assert_eq!(t.prompt.as_deref(), Some("fix the bug"));
        assert_eq!(t.cwd.as_deref(), Some("C:\\code\\app"));

        assert_eq!(
            parse_tail(&[user("x"), asst(true)].join("\n")).kind,
            LastKind::AssistantTool
        );
        let r = parse_tail(
            &[
                user("go"),
                asst(true),
                tool_result(),
                "{bad json".into(),
                "".into(),
            ]
            .join("\n"),
        );
        assert_eq!(r.kind, LastKind::User);
        assert_eq!(
            r.prompt.as_deref(),
            Some("go"),
            "tool_result entries are not prompts"
        );
        assert_eq!(
            parse_tail(&user("[Request interrupted by user]")).kind,
            LastKind::UserInterrupt
        );
        assert_eq!(
            parse_tail(&user("fix the interrupt handler")).kind,
            LastKind::User
        );
        assert_eq!(parse_tail(r#"{"type":"summary"}"#).kind, LastKind::Other);
        assert_eq!(
            parse_tail(&user(&"p".repeat(500)))
                .prompt
                .unwrap()
                .chars()
                .count(),
            PROMPT_MAX
        );
    }

    #[test]
    fn thresholds() {
        assert_eq!(LastKind::AssistantText.quiet_ms(), 2_500);
        assert_eq!(LastKind::UserInterrupt.quiet_ms(), 2_500);
        assert_eq!(LastKind::AssistantTool.quiet_ms(), 20_000);
        assert_eq!(LastKind::User.quiet_ms(), 75_000);
        assert_eq!(LastKind::Other.quiet_ms(), 300_000);
        assert_eq!(LastKind::AssistantTool.terminal(), HookEvent::Attention);
        assert_eq!(LastKind::User.terminal(), HookEvent::Done);
    }

    fn write_at(path: &Path, lines: &[String], mtime_ms: i64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, lines.join("\n") + "\n").unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::from_millis(mtime_ms as u64))
            .unwrap();
    }

    fn kinds(evs: &[SessionEvent]) -> Vec<HookEvent> {
        evs.iter().map(|e| e.kind).collect()
    }

    #[test]
    fn watcher_pushes_running_then_quiet_terminal_events() {
        let d = tempfile::tempdir().unwrap();
        let profile = d.path().join(".claude");
        let root = Root {
            path: profile.join("projects"),
            profile: Some(profile.clone()),
            desktop: false,
        };
        // Before the first scan: one quiet old file (seeded silently), one active file.
        let old = root.path.join("p1").join("old.jsonl");
        write_at(&old, &[user("a"), asst(false)], NOW - 60_000);
        let active = root.path.join("p2").join("live.jsonl");
        write_at(&active, &[user("b")], NOW - 1_000);
        // Ignored files.
        write_at(&root.path.join("p2").join("audit.jsonl"), &[user("c")], NOW);
        write_at(
            &root.path.join("p2").join("subagents").join("s.jsonl"),
            &[user("c")],
            NOW,
        );
        write_at(&root.path.join("p2").join("notes.txt"), &[user("c")], NOW);
        write_at(
            &root.path.join("p3").join("ancient.jsonl"),
            &[user("c")],
            NOW - RECENT_MS - 1,
        );

        let mut w = Watcher::new(vec![root]);
        let evs = w.tick(NOW);
        assert_eq!(
            kinds(&evs),
            vec![HookEvent::Running],
            "only the active file shows up"
        );
        assert_eq!(evs[0].session_id, "sess-1");
        assert_eq!(evs[0].profile_dir.as_deref(), Some(profile.as_path()));
        assert_eq!(evs[0].prompt.as_deref(), Some("b"));

        // User entry: done only after 75 s of quiet.
        assert!(w.tick(NOW - 1_000 + 75_000).is_empty());
        assert_eq!(kinds(&w.tick(NOW - 1_000 + 75_001)), vec![HookEvent::Done]);
        assert!(w.tick(NOW + 80_000).is_empty(), "pushed once");

        // The assistant starts a tool: running, then attention after 20 s.
        let t1 = NOW + 100_000;
        write_at(&active, &[user("b"), asst(true)], t1);
        assert_eq!(kinds(&w.tick(t1 + 10)), vec![HookEvent::Running]);
        // Another write within 800 ms is deferred, then ingested.
        write_at(
            &active,
            &[user("b"), asst(true), tool_result(), asst(true)],
            t1 + 300,
        );
        assert!(w.tick(t1 + 400).is_empty());
        assert_eq!(kinds(&w.tick(t1 + 900)), vec![HookEvent::Running]);
        assert!(w.tick(t1 + 300 + 20_000).is_empty());
        let evs = w.tick(t1 + 300 + 20_001);
        assert_eq!(kinds(&evs), vec![HookEvent::Attention]);
        assert!(evs[0].message.is_some());

        // Final answer: done after 2.5 s.
        let t2 = t1 + 60_000;
        write_at(&active, &[user("b"), asst(false)], t2);
        assert_eq!(kinds(&w.tick(t2 + 1)), vec![HookEvent::Running]);
        assert!(w.tick(t2 + 2_500).is_empty());
        assert_eq!(kinds(&w.tick(t2 + 2_501)), vec![HookEvent::Done]);

        // A new file after start-up is not seeded: running, then done.
        let t3 = t2 + 10_000;
        let fresh = d
            .path()
            .join(".claude")
            .join("projects")
            .join("p4")
            .join("new.jsonl");
        write_at(&fresh, &[user("[Request interrupted by user]")], t3 - 5_000);
        let evs = w.tick(t3 + WALK_EVERY_MS);
        assert_eq!(kinds(&evs), vec![HookEvent::Running, HookEvent::Done]);
    }

    #[test]
    fn desktop_roots_need_a_claude_folder_and_relabel_cwd() {
        let d = tempfile::tempdir().unwrap();
        let root = Root {
            path: d.path().join("local-agent-mode-sessions"),
            profile: None,
            desktop: true,
        };
        write_at(&root.path.join("x").join("other.jsonl"), &[user("a")], NOW);
        write_at(
            &root
                .path
                .join("x")
                .join(".claude")
                .join("projects")
                .join("p")
                .join("s.jsonl"),
            &[user("a")],
            NOW,
        );
        let evs = Watcher::new(vec![root]).tick(NOW);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].cwd.as_deref(), Some("Claude desktop"));
        assert_eq!(evs[0].profile_dir, None);
    }

    #[test]
    fn roots_follow_enabled_claude_accounts() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        assert!(roots(&env, &[]).is_empty());
        let mut a = Account::new(ProviderKind::Claude, "a");
        let mut b = Account::new(ProviderKind::Claude, "b");
        b.profile_path = d.path().join("p2").to_string_lossy().into_owned();
        let c = Account::new(ProviderKind::Codex, "c");
        std::fs::create_dir_all(env.local_appdata.join("Packages").join("Claude_abc")).unwrap();
        std::fs::create_dir_all(env.local_appdata.join("Packages").join("Other")).unwrap();
        let r = roots(&env, &[a.clone(), b.clone(), c]);
        assert_eq!(r.len(), 4);
        assert_eq!(r[0].path, env.home.join(".claude").join("projects"));
        assert_eq!(r[1].profile.as_deref(), Some(d.path().join("p2").as_path()));
        assert!(r[2].desktop && r[3].path.to_string_lossy().contains("Claude_abc"));
        a.enabled = false;
        b.enabled = false;
        assert!(roots(&env, &[a, b]).is_empty());
    }

    #[test]
    fn long_files_are_tailed() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("big.jsonl");
        let filler =
            serde_json::json!({"type":"user","message":{"content":"x".repeat(1000)}}).to_string();
        let mut lines: Vec<String> = std::iter::repeat_n(filler, 400).collect();
        lines.push(asst(true));
        write_at(&p, &lines, NOW);
        let text = read_tail(&p).unwrap();
        assert!(text.len() as u64 <= TAIL_BYTES);
        assert_eq!(parse_tail(&text).kind, LastKind::AssistantTool);
    }
}
