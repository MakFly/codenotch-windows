//! Other providers' working state. Polled every 2 s; SQLite databases are
//! opened read-only and re-queried only when the database or its `-wal` file changed.
//!
//! * Codex desktop: `thread_history_1.sqlite` turns `inProgress` (fresh = last item ≤ 10 min old
//!   or started ≤ 2 min ago); approval/permission/request_user item → waiting. Names from
//!   `state_5.sqlite`. Otherwise the newest rollout's tail (tool ≤ 10 min, thinking < 120 s,
//!   assistant message < 4 s, aborted/complete → idle).
//! * OpenCode: `session_v2` rows not idle, not archived, updated ≤ 2 min ago → working. No
//!   "waiting" state is derived.
//! * Cursor: `composerHeaders` (pending actions/plan → waiting, `unfinishedRunAt` → working).
//! * Antigravity: a brain `transcript.jsonl` written ≤ 45 s ago → working.

use crate::ProviderActivity;
use codenotch_core::model::{ActivityState, SessionInfo};
use codenotch_core::{Env, ProviderKind};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const POLL_EVERY: Duration = Duration::from_secs(2);
const CODEX_ITEM_FRESH_MS: i64 = 10 * 60_000;
const CODEX_START_FRESH_MS: i64 = 2 * 60_000;
const CODEX_TOOL_MS: i64 = 10 * 60_000;
const CODEX_THINK_MS: i64 = 120_000;
const CODEX_MSG_MS: i64 = 4_000;
const ROLLOUT_LOCATE_EVERY_MS: i64 = 30_000;
pub const OPENCODE_ACTIVE_MS: i64 = 120_000;
pub const ANTIGRAVITY_ACTIVE_MS: i64 = 45_000;

fn mtime_ms(path: &Path) -> Option<(i64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let t = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some((t, m.len()))
}

fn open_ro(path: &Path) -> Option<Connection> {
    if !path.is_file() {
        return None;
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let _ = conn.busy_timeout(Duration::from_millis(50));
    Some(conn)
}

/// (mtime ms, size) of the database and of its `-wal` file.
type DbSig = (Option<(i64, u64)>, Option<(i64, u64)>);

/// Read-only connection + cached query result, refreshed when the file signature changes.
struct DbCache<T> {
    path: PathBuf,
    conn: Option<Connection>,
    sig: Option<DbSig>,
    rows: Vec<T>,
}

impl<T: Clone> DbCache<T> {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            conn: None,
            sig: None,
            rows: Vec::new(),
        }
    }
    fn set_path(&mut self, path: PathBuf) {
        if path != self.path {
            *self = Self::new(path);
        }
    }
    fn signature(&self) -> DbSig {
        let mut wal = self.path.as_os_str().to_owned();
        wal.push("-wal");
        (mtime_ms(&self.path), mtime_ms(Path::new(&wal)))
    }
    fn refresh(&mut self, query: impl FnOnce(&Connection) -> Option<Vec<T>>) -> Vec<T> {
        let sig = self.signature();
        if sig.0.is_none() {
            self.conn = None;
            self.sig = None;
            self.rows.clear();
            return Vec::new();
        }
        if self.sig == Some(sig) {
            return self.rows.clone();
        }
        if self.conn.is_none() {
            self.conn = open_ro(&self.path);
        }
        let Some(conn) = &self.conn else {
            return Vec::new();
        };
        match query(conn) {
            Some(rows) => {
                self.rows = rows;
                self.sig = Some(sig);
            }
            None => {
                // Schema missing or busy: silence, retry on the next change.
                self.rows.clear();
                self.sig = Some(sig);
                self.conn = None;
            }
        }
        self.rows.clone()
    }
}

#[derive(Debug, Clone, PartialEq)]
struct CodexTurn {
    thread_id: String,
    started_ms: i64,
    last_ms: i64,
    waiting: bool,
    name: String,
}

#[derive(Debug, Clone, PartialEq)]
struct OpenCodeRow {
    id: String,
    title: String,
    model: Option<String>,
    cost: Option<f64>,
    updated_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
struct CursorRow {
    id: String,
    name: String,
    waiting: bool,
    since_ms: i64,
    detail: String,
}

/// Which step a Codex rollout's tail is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexStep {
    Tool,
    Thinking,
    AssistantMessage,
    Ended,
}

/// Last meaningful rollout entry and its timestamp (ms, 0 = unknown).
pub fn codex_last_step(text: &str) -> Option<(CodexStep, i64)> {
    for line in text.lines().rev().filter(|l| !l.trim().is_empty()) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let ts = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.timestamp_millis())
            .unwrap_or(0);
        let payload = v.get("payload").cloned().unwrap_or(Value::Null);
        let pt = payload.get("type").and_then(Value::as_str).unwrap_or("");
        let step = match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "turn_context" => Some(CodexStep::Thinking),
            "response_item" => match pt {
                "function_call" | "local_shell_call" | "custom_tool_call" | "web_search_call" => {
                    Some(CodexStep::Tool)
                }
                "function_call_output" | "custom_tool_call_output" | "reasoning" => {
                    Some(CodexStep::Thinking)
                }
                "message" => match payload.get("role").and_then(Value::as_str) {
                    Some("assistant") => Some(CodexStep::AssistantMessage),
                    Some("user") => Some(CodexStep::Thinking),
                    _ => None,
                },
                _ => None,
            },
            "event_msg" => match pt {
                "turn_aborted" | "task_complete" => Some(CodexStep::Ended),
                "task_started"
                | "item_started"
                | "exec_command_begin"
                | "user_message"
                | "agent_reasoning"
                | "agent_reasoning_raw_content" => Some(CodexStep::Thinking),
                "agent_message" => Some(CodexStep::AssistantMessage),
                _ => None,
            },
            _ => None,
        };
        if let Some(s) = step {
            return Some((s, ts));
        }
    }
    None
}

/// Whether a rollout step at `at` is still busy at `now`.
pub fn codex_step_busy(step: CodexStep, at: i64, now: i64) -> bool {
    let quiet = now - at;
    match step {
        CodexStep::Tool => quiet <= CODEX_TOOL_MS,
        CodexStep::Thinking => quiet < CODEX_THINK_MS,
        CodexStep::AssistantMessage => quiet < CODEX_MSG_MS,
        CodexStep::Ended => false,
    }
}

/// Newest `rollout-*.jsonl` in the three most recent day folders of `<codex>\sessions`.
fn newest_rollout(codex_home: &Path) -> Option<PathBuf> {
    fn dirs_desc(p: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by(|a, b| b.cmp(a));
        v
    }
    let mut days = Vec::new();
    'outer: for y in dirs_desc(&codex_home.join("sessions")) {
        for m in dirs_desc(&y) {
            for d in dirs_desc(&m) {
                days.push(d);
                if days.len() >= 3 {
                    break 'outer;
                }
            }
        }
    }
    days.iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
        })
        .filter_map(|p| mtime_ms(&p).map(|(t, _)| (t, p)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

#[derive(Default)]
struct RolloutCache {
    path: Option<PathBuf>,
    located_at: Option<i64>,
    sig: Option<(i64, u64)>,
    step: Option<(CodexStep, i64)>,
}

/// Polls every non-Claude provider that has an enabled account.
pub struct OtherPoller {
    env: Env,
    providers: Vec<ProviderKind>,
    codex_turns: DbCache<CodexTurn>,
    codex_names_path: PathBuf,
    rollout: RolloutCache,
    opencode: DbCache<OpenCodeRow>,
    cursor: DbCache<CursorRow>,
}

impl OtherPoller {
    pub fn new(env: Env) -> Self {
        let codex = codex_home(&env);
        Self {
            codex_turns: DbCache::new(codex.join("thread_history_1.sqlite")),
            codex_names_path: codex.join("state_5.sqlite"),
            rollout: RolloutCache::default(),
            opencode: DbCache::new(opencode_db(&env)),
            cursor: DbCache::new(
                env.appdata
                    .join("Cursor")
                    .join("User")
                    .join("globalStorage")
                    .join("state.vscdb"),
            ),
            providers: Vec::new(),
            env,
        }
    }

    pub fn set_providers(&mut self, providers: Vec<ProviderKind>) {
        self.providers = providers;
    }

    pub fn poll(&mut self, now: i64) -> Vec<ProviderActivity> {
        let mut out = Vec::new();
        let providers = self.providers.clone();
        for p in providers {
            let sessions = match p {
                ProviderKind::Codex => self.codex(now),
                ProviderKind::OpenCode => self.opencode(now),
                ProviderKind::Cursor => self.cursor(),
                ProviderKind::Antigravity => antigravity(&self.env, now),
                _ => continue,
            };
            if sessions.is_empty() {
                continue;
            }
            out.push(ProviderActivity {
                provider: p,
                profile_dir: None,
                state: ActivityState::aggregate(sessions.iter().map(|s| s.state)),
                sessions,
            });
        }
        out
    }

    fn codex(&mut self, now: i64) -> Vec<SessionInfo> {
        let names_path = self.codex_names_path.clone();
        let turns = self
            .codex_turns
            .refresh(|conn| query_codex_turns(conn, &names_path));
        let live: Vec<SessionInfo> = turns
            .into_iter()
            .filter(|t| {
                now - t.last_ms <= CODEX_ITEM_FRESH_MS || now - t.started_ms <= CODEX_START_FRESH_MS
            })
            .map(|t| SessionInfo {
                id: t.thread_id,
                title: t.name,
                cwd: None,
                model: None,
                state: if t.waiting {
                    ActivityState::Waiting
                } else {
                    ActivityState::Working
                },
                since: t.started_ms.div_euclid(1000),
                cost: None,
                detail: Some(
                    if t.waiting {
                        "Needs your input"
                    } else {
                        "Working"
                    }
                    .into(),
                ),
            })
            .collect();
        if !live.is_empty() {
            return live;
        }
        // CLI / extension: rollout tail.
        let r = &mut self.rollout;
        if r.path.is_none()
            || r.located_at
                .is_none_or(|t| now - t >= ROLLOUT_LOCATE_EVERY_MS)
        {
            r.located_at = Some(now);
            let found = newest_rollout(&codex_home(&self.env));
            if found != r.path {
                r.path = found;
                r.sig = None;
                r.step = None;
            }
        }
        let Some(path) = r.path.clone() else {
            return Vec::new();
        };
        let sig = mtime_ms(&path);
        if sig != r.sig {
            r.sig = sig;
            r.step = crate::transcript::read_tail(&path)
                .and_then(|t| codex_last_step(&t))
                .map(|(s, ts)| (s, ts.max(sig.map(|s| s.0).unwrap_or(0))));
        }
        match r.step {
            Some((step, at)) if codex_step_busy(step, at, now) => vec![SessionInfo {
                id: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                title: "Codex".into(),
                state: ActivityState::Working,
                since: at.div_euclid(1000),
                detail: Some("Working".into()),
                ..SessionInfo::default()
            }],
            _ => Vec::new(),
        }
    }

    fn opencode(&mut self, now: i64) -> Vec<SessionInfo> {
        self.opencode.set_path(opencode_db(&self.env));
        let rows = self.opencode.refresh(|conn| query_opencode(conn, now));
        rows.into_iter()
            .filter(|r| now - r.updated_ms <= OPENCODE_ACTIVE_MS)
            .map(|r| SessionInfo {
                id: r.id,
                title: if r.title.trim().is_empty() {
                    "OpenCode".into()
                } else {
                    r.title
                },
                cwd: None,
                model: r.model,
                state: ActivityState::Working,
                since: r.updated_ms.div_euclid(1000),
                cost: r.cost,
                detail: Some("Working".into()),
            })
            .collect()
    }

    fn cursor(&mut self) -> Vec<SessionInfo> {
        self.cursor
            .refresh(query_cursor)
            .into_iter()
            .map(|r| SessionInfo {
                id: r.id,
                title: r.name,
                state: if r.waiting {
                    ActivityState::Waiting
                } else {
                    ActivityState::Working
                },
                since: r.since_ms.div_euclid(1000),
                detail: Some(r.detail),
                ..SessionInfo::default()
            })
            .collect()
    }
}

pub fn codex_home(env: &Env) -> PathBuf {
    env.var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".codex"))
}

pub fn opencode_db(env: &Env) -> PathBuf {
    let data = env
        .var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".local").join("share"));
    data.join("opencode").join("opencode.db")
}

/// Seconds or milliseconds → milliseconds.
fn to_ms(v: &rusqlite::types::Value) -> i64 {
    match v {
        rusqlite::types::Value::Integer(i) => {
            if *i > 10_000_000_000 {
                *i
            } else {
                i * 1000
            }
        }
        rusqlite::types::Value::Real(f) => (if *f > 1e10 { *f } else { f * 1000.0 }) as i64,
        _ => 0,
    }
}

fn query_codex_turns(conn: &Connection, names_path: &Path) -> Option<Vec<CodexTurn>> {
    let mut stmt = conn
        .prepare("SELECT thread_id, started_at FROM thread_turns WHERE status = 'inProgress' ORDER BY started_at DESC LIMIT 8")
        .ok()?;
    let rows: Vec<(String, rusqlite::types::Value)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok()?
        .flatten()
        .collect();
    let names = if rows.is_empty() {
        None
    } else {
        open_ro(names_path)
    };
    let mut out = Vec::new();
    for (thread_id, started) in rows {
        let started_ms = to_ms(&started);
        let (last_ms, item_type): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT created_at_ms, item_type FROM thread_items WHERE thread_id = ?1 ORDER BY created_at_ms DESC LIMIT 1",
                [&thread_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or((None, None));
        let lt = item_type.unwrap_or_default().to_ascii_lowercase();
        let name = names
            .as_ref()
            .and_then(|c| {
                c.query_row(
                    "SELECT COALESCE(title,''), COALESCE(first_user_message,''), COALESCE(agent_nickname,'') FROM threads WHERE id = ?1",
                    [&thread_id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
                )
                .ok()
            })
            .map(|(title, first, nick)| {
                if !title.trim().is_empty() {
                    title
                } else if !first.trim().is_empty() {
                    first.chars().take(40).collect()
                } else if !nick.trim().is_empty() {
                    format!("Agent {nick}")
                } else {
                    String::new()
                }
            })
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Codex".into());
        out.push(CodexTurn {
            thread_id,
            started_ms,
            last_ms: last_ms.unwrap_or(started_ms),
            waiting: lt.contains("approval")
                || lt.contains("permission")
                || lt.contains("request_user"),
            name,
        });
    }
    Some(out)
}

fn query_opencode(conn: &Connection, now: i64) -> Option<Vec<OpenCodeRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, title, model, cost, time_updated FROM session_v2 \
             WHERE time_idle IS NULL AND time_archived IS NULL AND time_updated >= ?1 \
             ORDER BY time_updated DESC LIMIT 6",
        )
        .ok()?;
    let text = |v: rusqlite::types::Value| match v {
        rusqlite::types::Value::Text(s) if !s.trim().is_empty() => Some(s),
        _ => None,
    };
    let rows = stmt
        .query_map([now - OPENCODE_ACTIVE_MS], |r| {
            Ok(OpenCodeRow {
                id: text(r.get(0)?).unwrap_or_default(),
                title: text(r.get(1)?).unwrap_or_default(),
                model: text(r.get(2)?),
                cost: r.get::<_, Option<f64>>(3).ok().flatten(),
                updated_ms: r.get(4)?,
            })
        })
        .ok()?;
    Some(rows.flatten().collect())
}

fn query_cursor(conn: &Connection) -> Option<Vec<CursorRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT value FROM composerHeaders WHERE isArchived = 0 ORDER BY recency DESC LIMIT 40",
        )
        .ok()?;
    let values: Vec<String> = stmt.query_map([], |r| r.get(0)).ok()?.flatten().collect();
    Some(values.iter().filter_map(|j| cursor_row(j)).collect())
}

fn cursor_row(json: &str) -> Option<CursorRow> {
    let v: Value = serde_json::from_str(json).ok()?;
    let id = v.get("composerId")?.as_str()?.to_string();
    let flag = |k: &str| v.get(k).and_then(Value::as_bool) == Some(true);
    let waiting = flag("hasBlockingPendingActions") || flag("hasPendingPlan");
    let running = v.get("unfinishedRunAt").and_then(Value::as_f64);
    if !waiting && running.is_none() {
        return None;
    }
    let since = running
        .or_else(|| v.get("lastUpdatedAt").and_then(Value::as_f64))
        .or_else(|| v.get("createdAt").and_then(Value::as_f64))
        .unwrap_or(0.0) as i64;
    Some(CursorRow {
        id,
        name: v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Untitled chat")
            .to_string(),
        waiting,
        since_ms: since,
        detail: if waiting {
            "Needs your input".into()
        } else {
            v.get("subtitle")
                .and_then(Value::as_str)
                .unwrap_or("Working")
                .to_string()
        },
    })
}

fn antigravity(env: &Env, now: i64) -> Vec<SessionInfo> {
    let Ok(rd) = std::fs::read_dir(env.home.join(".gemini")) else {
        return Vec::new();
    };
    let mut newest: Option<(String, i64)> = None;
    for root in rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("antigravity"))
    {
        let Ok(brains) = std::fs::read_dir(root.path().join("brain")) else {
            continue;
        };
        for b in brains.flatten() {
            let t = b
                .path()
                .join(".system_generated")
                .join("logs")
                .join("transcript.jsonl");
            let Some((m, _)) = mtime_ms(&t) else { continue };
            if newest.as_ref().is_none_or(|(_, n)| m > *n) {
                newest = Some((b.file_name().to_string_lossy().into_owned(), m));
            }
        }
    }
    match newest {
        Some((id, at)) if now - at <= ANTIGRAVITY_ACTIVE_MS => vec![SessionInfo {
            id,
            title: "Antigravity".into(),
            state: ActivityState::Working,
            since: at.div_euclid(1000),
            detail: Some("Working".into()),
            ..SessionInfo::default()
        }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    #[test]
    fn opencode_working_only_for_fresh_live_rows() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let db = opencode_db(&env);
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let now = now_ms();
        {
            let c = Connection::open(&db).unwrap();
            c.execute_batch(&format!(
                "CREATE TABLE session_v2(id TEXT, title TEXT, model TEXT, cost REAL, time_updated INTEGER, time_idle INTEGER, time_archived INTEGER);
                 INSERT INTO session_v2 VALUES ('a','Fix bug','openrouter/x',0.42,{now},NULL,NULL);
                 INSERT INTO session_v2 VALUES ('b','Idle','x',0.1,{now},{now},NULL);
                 INSERT INTO session_v2 VALUES ('c','Archived','x',0.1,{now},NULL,{now});
                 INSERT INTO session_v2 VALUES ('d','Old','x',0.0,{},NULL,NULL);",
                now - 600_000
            ))
            .unwrap();
        }
        let mut p = OtherPoller::new(env);
        p.set_providers(vec![ProviderKind::OpenCode]);
        let acts = p.poll(now);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].state, ActivityState::Working);
        assert_eq!(acts[0].sessions.len(), 1);
        let s = &acts[0].sessions[0];
        assert_eq!(
            (s.id.as_str(), s.title.as_str(), s.cost),
            ("a", "Fix bug", Some(0.42))
        );
        // Ages out without any file change.
        assert!(p.poll(now + OPENCODE_ACTIVE_MS + 1).is_empty());
    }

    #[test]
    fn missing_db_or_table_is_silence() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let mut p = OtherPoller::new(env.clone());
        p.set_providers(vec![
            ProviderKind::OpenCode,
            ProviderKind::Codex,
            ProviderKind::Cursor,
            ProviderKind::Antigravity,
        ]);
        assert!(p.poll(now_ms()).is_empty());
        let db = opencode_db(&env);
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE other(x);")
            .unwrap();
        assert!(p.poll(now_ms()).is_empty());
    }

    #[test]
    fn codex_desktop_turns() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let home = codex_home(&env);
        std::fs::create_dir_all(&home).unwrap();
        let now = now_ms();
        let c = Connection::open(home.join("thread_history_1.sqlite")).unwrap();
        c.execute_batch(&format!(
            "CREATE TABLE thread_turns(thread_id TEXT, started_at INTEGER, status TEXT);
             CREATE TABLE thread_items(thread_id TEXT, created_at_ms INTEGER, item_type TEXT);
             INSERT INTO thread_turns VALUES ('t1', {s}, 'inProgress');
             INSERT INTO thread_turns VALUES ('t2', {s}, 'inProgress');
             INSERT INTO thread_turns VALUES ('t3', {old}, 'inProgress');
             INSERT INTO thread_turns VALUES ('t4', {s}, 'completed');
             INSERT INTO thread_items VALUES ('t1', {now}, 'agentMessage');
             INSERT INTO thread_items VALUES ('t2', {now}, 'execApprovalRequest');
             INSERT INTO thread_items VALUES ('t3', {stale}, 'agentMessage');",
            s = now / 1000 - 30,
            old = now / 1000 - 3600,
            stale = now - 3_000_000
        ))
        .unwrap();
        let n = Connection::open(home.join("state_5.sqlite")).unwrap();
        n.execute_batch(
            "CREATE TABLE threads(id TEXT, title TEXT, first_user_message TEXT, agent_nickname TEXT);
             INSERT INTO threads VALUES ('t1', 'Refactor parser', NULL, NULL);",
        )
        .unwrap();
        let mut p = OtherPoller::new(env);
        p.set_providers(vec![ProviderKind::Codex]);
        let acts = p.poll(now);
        assert_eq!(acts.len(), 1);
        let a = &acts[0];
        assert_eq!(a.provider, ProviderKind::Codex);
        assert_eq!(a.state, ActivityState::Waiting);
        let mut ids: Vec<_> = a
            .sessions
            .iter()
            .map(|s| (s.id.as_str(), s.state, s.title.as_str()))
            .collect();
        ids.sort_by_key(|x| x.0);
        assert_eq!(
            ids,
            vec![
                ("t1", ActivityState::Working, "Refactor parser"),
                ("t2", ActivityState::Waiting, "Codex")
            ]
        );
    }

    #[test]
    fn codex_rollout_steps_and_thresholds() {
        let line = |ty: &str, payload: serde_json::Value| {
            serde_json::json!({"timestamp":"2026-11-12T10:00:00Z","type":ty,"payload":payload})
                .to_string()
        };
        let tool = line("response_item", serde_json::json!({"type":"function_call"}));
        let msg = line(
            "response_item",
            serde_json::json!({"type":"message","role":"assistant"}),
        );
        let done = line("event_msg", serde_json::json!({"type":"task_complete"}));
        let tokens = line("event_msg", serde_json::json!({"type":"token_count"}));
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-12T10:00:00Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            codex_last_step(&[tool.clone(), tokens.clone()].join("\n")),
            Some((CodexStep::Tool, at))
        );
        assert_eq!(
            codex_last_step(&[tool.clone(), msg.clone()].join("\n"))
                .unwrap()
                .0,
            CodexStep::AssistantMessage
        );
        assert_eq!(
            codex_last_step(&[msg, done].join("\n")).unwrap().0,
            CodexStep::Ended
        );
        assert_eq!(codex_last_step(&tokens), None);
        assert!(codex_step_busy(CodexStep::Tool, 0, CODEX_TOOL_MS));
        assert!(!codex_step_busy(CodexStep::Tool, 0, CODEX_TOOL_MS + 1));
        assert!(codex_step_busy(CodexStep::Thinking, 0, 119_999));
        assert!(!codex_step_busy(CodexStep::Thinking, 0, 120_000));
        assert!(codex_step_busy(CodexStep::AssistantMessage, 0, 3_999));
        assert!(!codex_step_busy(CodexStep::AssistantMessage, 0, 4_000));
        assert!(!codex_step_busy(CodexStep::Ended, 0, 0));

        // End to end through the newest rollout file.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let day = codex_home(&env)
            .join("sessions")
            .join("2026")
            .join("11")
            .join("12");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("rollout-a.jsonl"), format!("{tool}\n")).unwrap();
        let mut p = OtherPoller::new(env);
        p.set_providers(vec![ProviderKind::Codex]);
        let now = now_ms();
        assert_eq!(p.poll(now).len(), 1, "fresh tool call = working");
        assert!(p.poll(now + CODEX_TOOL_MS + 60_000).is_empty());
    }

    #[test]
    fn cursor_rows() {
        assert!(
            cursor_row(r#"{"composerId":"c","name":"n"}"#).is_none(),
            "idle chat ignored"
        );
        let w =
            cursor_row(r#"{"composerId":"c","hasPendingPlan":true,"lastUpdatedAt":5000}"#).unwrap();
        assert!(w.waiting && w.since_ms == 5000 && w.name == "Untitled chat");
        let b = cursor_row(r#"{"composerId":"c","unfinishedRunAt":7000,"subtitle":"Editing"}"#)
            .unwrap();
        assert!(!b.waiting && b.detail == "Editing");
        assert!(cursor_row("nope").is_none());
    }

    #[test]
    fn antigravity_recent_transcript() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let logs = env
            .home
            .join(".gemini")
            .join("antigravity")
            .join("brain")
            .join("b1")
            .join(".system_generated")
            .join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("transcript.jsonl"), "{}\n").unwrap();
        let now = now_ms();
        assert_eq!(antigravity(&env, now).len(), 1);
        assert!(antigravity(&env, now + ANTIGRAVITY_ACTIVE_MS + 5_000).is_empty());
    }
}
