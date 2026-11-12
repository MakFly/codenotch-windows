//! Codex (data-spec §3): `auth.json` (never refreshed or written), `GET wham/usage`, then the
//! recovery paths: a native `codex.exe app-server` and the newest rollout snapshot.

use crate::i18n::{t, tf};
use crate::util::{
    self, capitalise, carry_or, flexible_time, jwt_payload, num, path_dirs, set_flags, short, text,
};
use codenotch_core::http::HttpRequest;
use codenotch_core::model::{LimitWindow, ProviderKind, Reading, ReadingStatus, WindowKind};
use codenotch_core::provider::{Env, FetchContext, LocalIdentity, Provider};
use codenotch_core::{Account, Error, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The app-server conversation is abandoned (and the child killed) after this.
pub const APP_SERVER_TIMEOUT: Duration = Duration::from_secs(20);
/// After a failed app-server attempt, it is not tried again for this long (in memory).
pub const APP_SERVER_REST_SECS: i64 = 30 * 60;
/// A rollout snapshot older than this is shown as stale.
pub const ROLLOUT_FRESH_SECS: i64 = 5 * 60;
const ROLLOUT_TAIL_BYTES: u64 = 256 * 1024;
/// Rollout files tried, newest first, before giving up.
const ROLLOUT_CANDIDATES: usize = 8;

pub(crate) const NOTE_EXPIRED: &str = "Codex sign-in expired — open Codex once to refresh it";
pub(crate) const NOTE_REJECTED: &str = "Codex rejected its sign-in — sign in to Codex again";
pub(crate) const NOTE_NO_WINDOWS: &str = "Codex reported no usage windows";
pub(crate) const NOTE_NO_SNAPSHOT: &str = "Codex has not recorded a usage snapshot yet";
pub(crate) const NOTE_NOT_SIGNED_IN: &str = "Codex is not signed in";
pub(crate) const NOTE_FROM_LAST_RUN: &str = "from last Codex run";
pub(crate) const LABEL_LONGER: &str = "Longer window";
pub(crate) const GROUP_CODE_REVIEW: &str = "Code review";
// Templates translated at fetch time (`crate::i18n::tf`).
pub(crate) const TPL_RATE_LIMITED: &str = "Rate limited — retrying in {wait}s";
pub(crate) const TPL_LIVE_FAILED: &str = "Live read failed ({e})";
pub(crate) const TPL_LIVE_HTTP: &str = "Live read failed (HTTP {code})";
pub(crate) const TPL_PLAN_FROM_LAST_RUN: &str = "{plan} · from last Codex run";

// ---------------------------------------------------------------------------------------------
// auth.json
// ---------------------------------------------------------------------------------------------

/// Codex credential. `Debug` hides the token and account id.
pub(crate) struct CodexAuth {
    access_token: String,
    account_id: String,
    pub expired: bool,
    pub plan: Option<String>,
    pub email: Option<String>,
}

impl std::fmt::Debug for CodexAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexAuth")
            .field("expired", &self.expired)
            .field("plan", &self.plan)
            .finish()
    }
}

/// Plan claim of an id_token payload (nested `https://api.openai.com/auth` object or flat key).
fn jwt_plan(payload: &Value) -> Option<String> {
    text(
        payload
            .get("https://api.openai.com/auth")
            .and_then(|a| a.get("chatgpt_plan_type")),
    )
    .or_else(|| text(payload.get("https://api.openai.com/auth.chatgpt_plan_type")))
}

pub(crate) fn parse_auth(body: &str, now: i64) -> Option<CodexAuth> {
    let v: Value = serde_json::from_str(body).ok()?;
    let tokens = v.get("tokens")?;
    let access_token = text(tokens.get("access_token"))?;
    let account_id = text(tokens.get("account_id"))?;
    let access = jwt_payload(&access_token);
    let expired = access
        .as_ref()
        .and_then(|p| num(p.get("exp")))
        .is_some_and(|exp| (exp as i64) <= now);
    let id = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .and_then(jwt_payload);
    let plan = id
        .as_ref()
        .and_then(jwt_plan)
        .or_else(|| access.as_ref().and_then(jwt_plan));
    let email = id.as_ref().and_then(|p| text(p.get("email")));
    Some(CodexAuth {
        access_token,
        account_id,
        expired,
        plan,
        email,
    })
}

fn read_auth(dir: &Path, now: i64) -> Option<CodexAuth> {
    std::fs::read_to_string(dir.join("auth.json"))
        .ok()
        .and_then(|t| parse_auth(&t, now))
}

/// E-mail and plan of an `auth.json`, from the id_token's payload (the access token's as a
/// fallback for the plan). Only the payload is decoded; nothing else of the tokens is kept.
pub(crate) fn auth_identity(body: &str) -> LocalIdentity {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return LocalIdentity::default();
    };
    let tokens = v.get("tokens");
    let payload = |name: &str| {
        tokens
            .and_then(|t| t.get(name))
            .and_then(Value::as_str)
            .and_then(jwt_payload)
    };
    let id = payload("id_token");
    let plan = id
        .as_ref()
        .and_then(jwt_plan)
        .or_else(|| payload("access_token").as_ref().and_then(jwt_plan));
    LocalIdentity {
        identity: id.as_ref().and_then(|p| text(p.get("email"))),
        plan: plan.map(|p| plan_label(&p)),
    }
}

// ---------------------------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------------------------

/// A Codex profile folder: `default` (`~\.codex`), `<id>` of `~\.codex-<id>`, or the folder
/// `CODEX_HOME` names.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CodexProfile {
    pub id: String,
    pub dir: PathBuf,
}

fn valid_profile_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && id != "default"
}

/// `~\.codex` when Codex is present at all (its `auth.json`, its `sessions` or an executable),
/// then every `~\.codex-<id>` folder holding an `auth.json` (sorted), then the `CODEX_HOME`
/// folder when it is another one holding an `auth.json`.
pub(crate) fn codex_profiles(env: &Env) -> Vec<CodexProfile> {
    let mut out = Vec::new();
    let default = env.home_path(".codex");
    if default.join("auth.json").is_file()
        || default.join("sessions").is_dir()
        || find_codex(env, false).is_some()
    {
        out.push(CodexProfile {
            id: "default".into(),
            dir: default,
        });
    }
    let mut extra = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&env.home) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_prefix(".codex-") else {
                continue;
            };
            let dir = entry.path();
            if valid_profile_id(id) && dir.join("auth.json").is_file() {
                extra.push(CodexProfile {
                    id: id.to_string(),
                    dir,
                });
            }
        }
    }
    extra.sort_by(|a, b| a.id.cmp(&b.id));
    out.extend(extra);
    if let Some(dir) = env.var("CODEX_HOME").map(PathBuf::from) {
        let listed = util::same_path(&dir, &env.home_path(".codex"))
            || out.iter().any(|p| util::same_path(&p.dir, &dir));
        if !listed && dir.join("auth.json").is_file() {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = name.trim_start_matches('.');
            let id: String = name
                .strip_prefix("codex-")
                .unwrap_or(name)
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                .take(64)
                .collect();
            out.push(CodexProfile {
                id: if valid_profile_id(&id) && id != "codex" {
                    id
                } else {
                    "home".into()
                },
                dir,
            });
        }
    }
    out
}

/// "plus" → "Plus", "prolite" stays "Prolite".
fn plan_label(plan: &str) -> String {
    capitalise(&plan.replace('_', " "))
}

// ---------------------------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------------------------

/// Label and kind from the window length in minutes (data-spec §3.3; a 5-hour window is the
/// canonical "Current session").
fn label_for(minutes: Option<i64>, primary: bool) -> (String, WindowKind) {
    let Some(m) = minutes.filter(|m| *m > 0) else {
        return if primary {
            ("Current session".into(), WindowKind::Session)
        } else {
            (LABEL_LONGER.into(), WindowKind::Other)
        };
    };
    if m == 300 {
        return ("Current session".into(), WindowKind::Session);
    }
    if m < 60 {
        return (format!("{m}m limit"), WindowKind::Session);
    }
    if m < 1440 {
        return (
            format!("{}h limit", (m as f64 / 60.0).round() as i64),
            WindowKind::Session,
        );
    }
    match (m as f64 / 1440.0).round() as i64 {
        7 => ("Weekly limit".into(), WindowKind::Weekly),
        28..=31 => ("Monthly limit".into(), WindowKind::Monthly),
        d => (format!("{d}d limit"), WindowKind::Other),
    }
}

/// One window from used percent (0–100), length (minutes) and reset (unix secs).
fn build_window(
    id: &str,
    primary: bool,
    used: f64,
    minutes: Option<i64>,
    resets_at: Option<i64>,
) -> LimitWindow {
    let (label, kind) = label_for(minutes, primary);
    let mut w = LimitWindow::new(id, label, kind).used(used);
    w.resets_at = resets_at;
    w.window_seconds = minutes.filter(|m| *m > 0).map(|m| m * 60);
    w
}

fn non_negative(v: Option<f64>) -> Option<f64> {
    v.filter(|x| *x >= 0.0)
}

/// `wham/usage` window object: `used_percent`, `limit_window_seconds`, `reset_at` (secs) else
/// `now + reset_after_seconds`.
fn wham_window(v: Option<&Value>, id: &str, primary: bool, now: i64) -> Option<LimitWindow> {
    let v = v.filter(|v| v.is_object())?;
    let used = num(v.get("used_percent"))?;
    let reset = non_negative(num(v.get("reset_at")))
        .map(|s| s as i64)
        .or_else(|| non_negative(num(v.get("reset_after_seconds"))).map(|s| now + s as i64));
    let minutes =
        non_negative(num(v.get("limit_window_seconds"))).map(|s| (s / 60.0).round() as i64);
    Some(build_window(id, primary, used, minutes, reset))
}

fn push_unique(out: &mut Vec<LimitWindow>, w: Option<LimitWindow>) {
    if let Some(w) = w {
        if !out.iter().any(|x| x.id == w.id) {
            out.push(w);
        }
    }
}

/// Parse the body of `GET wham/usage` (data-spec §3.3).
pub fn parse_codex_usage(body: &str, now: i64) -> Result<Vec<LimitWindow>> {
    let v: Value = serde_json::from_str(body)?;
    if !v.is_object() {
        return Err(Error::other("Codex usage: unexpected response shape"));
    }
    let mut out = Vec::new();
    let rl = v.get("rate_limit");
    push_unique(
        &mut out,
        wham_window(
            rl.and_then(|r| r.get("primary_window")),
            "primary",
            true,
            now,
        ),
    );
    push_unique(
        &mut out,
        wham_window(
            rl.and_then(|r| r.get("secondary_window")),
            "secondary",
            false,
            now,
        ),
    );
    if let Some(extra) = v.get("additional_rate_limits").and_then(Value::as_array) {
        let spark = extra.iter().find(|e| {
            ["limit_name", "metered_feature"].iter().any(|k| {
                e.get(*k)
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.to_ascii_lowercase().contains("spark"))
            })
        });
        if let Some(rl) = spark.and_then(|e| e.get("rate_limit")) {
            for (field, id, primary) in [
                ("primary_window", "spark", true),
                ("secondary_window", "spark-secondary", false),
            ] {
                push_unique(
                    &mut out,
                    wham_window(rl.get(field), id, primary, now).map(|w| w.group("Spark")),
                );
            }
        }
    }
    if let Some(rl) = v.get("code_review_rate_limit") {
        for (field, id, primary) in [
            ("primary_window", "code-review", true),
            ("secondary_window", "code-review-secondary", false),
        ] {
            push_unique(
                &mut out,
                wham_window(rl.get(field), id, primary, now).map(|w| w.group(GROUP_CODE_REVIEW)),
            );
        }
    }
    Ok(out)
}

/// `account/rateLimits/read` result → `primary` / `secondary` (data-spec §3.4). `None` when no
/// codex bucket or no window.
pub(crate) fn parse_app_server_limits(result: &Value) -> Option<Vec<LimitWindow>> {
    let bucket = match result.get("rateLimitsByLimitId").filter(|b| !b.is_null()) {
        Some(by_id) => by_id.get("codex")?,
        None => result.get("rateLimits")?,
    };
    if let Some(id) = bucket.get("limitId").and_then(Value::as_str) {
        if id != "codex" {
            return None;
        }
    }
    let window = |key: &str, id: &str, primary: bool| {
        let w = bucket.get(key).filter(|w| w.is_object())?;
        let used = num(w.get("usedPercent"))?;
        let minutes = non_negative(num(w.get("windowDurationMins"))).map(|m| m as i64);
        let reset = flexible_time(w.get("resetsAt"));
        Some(build_window(id, primary, used, minutes, reset))
    };
    let out: Vec<LimitWindow> = [
        window("primary", "primary", true),
        window("secondary", "secondary", false),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!out.is_empty()).then_some(out)
}

// ---------------------------------------------------------------------------------------------
// Rollout snapshot
// ---------------------------------------------------------------------------------------------

/// A rate-limit snapshot from a rollout line.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RolloutSnapshot {
    pub at: i64,
    pub windows: Vec<LimitWindow>,
    pub plan: Option<String>,
}

fn rollout_window(v: Option<&Value>, id: &str, primary: bool, now: i64) -> Option<LimitWindow> {
    let v = v.filter(|v| v.is_object())?;
    let used = num(v.get("used_percent"))?;
    let reset = non_negative(num(v.get("resets_at")))
        .map(|s| s as i64)
        .or_else(|| non_negative(num(v.get("resets_in_seconds"))).map(|s| now + s as i64));
    let minutes = non_negative(num(v.get("window_minutes"))).map(|m| m as i64);
    Some(build_window(id, primary, used, minutes, reset))
}

/// One rollout line → snapshot (`payload.rate_limits` or top-level `rate_limits`). Lines of
/// another limit id, or without any window, give `None`. `fallback_at` is used when the
/// line has no parsable `timestamp`.
pub(crate) fn parse_rollout_line(
    line: &str,
    now: i64,
    fallback_at: i64,
) -> Option<RolloutSnapshot> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let rl = v
        .get("payload")
        .and_then(|p| p.get("rate_limits"))
        .filter(|r| r.is_object())
        .or_else(|| v.get("rate_limits").filter(|r| r.is_object()))?;
    if let Some(id) = rl
        .get("limit_id")
        .or_else(|| rl.get("limitId"))
        .and_then(Value::as_str)
    {
        if id != "codex" {
            return None;
        }
    }
    let windows: Vec<LimitWindow> = [
        rollout_window(rl.get("primary"), "primary", true, now),
        rollout_window(rl.get("secondary"), "secondary", false, now),
    ]
    .into_iter()
    .flatten()
    .collect();
    if windows.is_empty() {
        return None;
    }
    let at = v
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(util::rfc3339_secs)
        .unwrap_or(fallback_at);
    Some(RolloutSnapshot {
        at,
        windows,
        plan: text(rl.get("plan_type")),
    })
}

fn mtime_secs(path: &Path) -> Option<i64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Newest snapshot in the last 256 KiB of `file`.
pub(crate) fn snapshot_from_file(file: &Path, now: i64) -> Option<RolloutSnapshot> {
    let mut f = std::fs::File::open(file).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(ROLLOUT_TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.take(ROLLOUT_TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let fallback = mtime_secs(file).unwrap_or(now);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // partial first line
    }
    lines
        .iter()
        .rev()
        .filter(|l| l.contains("rate_limits"))
        .find_map(|l| parse_rollout_line(l, now, fallback))
}

fn is_rollout_name(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
}

/// Rollout paths from `state_5.sqlite` (read-only, WAL visible), newest thread first.
pub(crate) fn sqlite_rollouts(profile: &Path) -> Vec<PathBuf> {
    use rusqlite::{Connection, OpenFlags};
    let db = profile.join("state_5.sqlite");
    if !db.is_file() {
        return Vec::new();
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(conn) = Connection::open_with_flags(&db, flags) else {
        return Vec::new();
    };
    let _ = conn.busy_timeout(Duration::from_millis(50));
    let query = |sql: &str| -> rusqlite::Result<Vec<String>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| row.get::<_, Option<String>>(0))?;
        Ok(rows.filter_map(|r| r.ok().flatten()).collect())
    };
    let paths = query(
        "SELECT rollout_path FROM threads WHERE archived = 0 ORDER BY updated_at_ms DESC LIMIT 8",
    )
    .or_else(|_| {
        query(
            "SELECT rollout_path FROM threads WHERE archived = 0 ORDER BY updated_at DESC LIMIT 8",
        )
    })
    .unwrap_or_default();
    paths
        .into_iter()
        .map(|p| {
            let p = PathBuf::from(p);
            if p.is_relative() {
                profile.join(p)
            } else {
                p
            }
        })
        .filter(|p| is_rollout_name(p) && p.is_file())
        .collect()
}

fn numeric_subdirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (!name.is_empty() && name.bytes().all(|b| b.is_ascii_digit())).then(|| (name, e.path()))
        })
        .collect();
    out.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(b.0.cmp(&a.0)));
    out
}

/// Rollout files of the 3 newest `sessions\YYYY\MM\DD` folders, newest mtime first.
pub(crate) fn scanned_rollouts(profile: &Path) -> Vec<PathBuf> {
    let mut days = Vec::new();
    'outer: for (_, y) in numeric_subdirs(&profile.join("sessions")) {
        for (_, m) in numeric_subdirs(&y) {
            for (_, d) in numeric_subdirs(&m) {
                days.push(d);
                if days.len() == 3 {
                    break 'outer;
                }
            }
        }
    }
    let mut files: Vec<(i64, PathBuf)> = days
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|it| it.flatten().map(|e| e.path()))
        .filter(|p| is_rollout_name(p) && p.is_file())
        .map(|p| (mtime_secs(&p).unwrap_or(0), p))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files.into_iter().map(|(_, p)| p).collect()
}

pub(crate) fn rollout_snapshot(profile: &Path, now: i64) -> Option<RolloutSnapshot> {
    let mut candidates = sqlite_rollouts(profile);
    if candidates.is_empty() {
        candidates = scanned_rollouts(profile);
    }
    candidates
        .iter()
        .take(ROLLOUT_CANDIDATES)
        .find_map(|p| snapshot_from_file(p, now))
}

/// Latest rate-limit snapshot from the newest rollout under `profile` (data-spec §3.5):
/// `(snapshot time from the line's own timestamp, windows)`. Any age is returned; the caller
/// marks it stale when older than [`ROLLOUT_FRESH_SECS`]. `None` when nothing is found.
pub fn codex_rollout_windows(profile: &Path, now: i64) -> Option<(i64, Vec<LimitWindow>)> {
    rollout_snapshot(profile, now).map(|s| (s.at, s.windows))
}

// ---------------------------------------------------------------------------------------------
// Executables and the app-server
// ---------------------------------------------------------------------------------------------

fn newest_subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut subs: Vec<(i64, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .map(|p| (mtime_secs(&p).unwrap_or(0), p))
        .collect();
    subs.sort_by_key(|s| std::cmp::Reverse(s.0));
    subs.into_iter().map(|(_, p)| p).collect()
}

fn files_matching(dir: &Path, pred: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| pred(&n.to_ascii_lowercase()))
        })
        .collect();
    v.sort();
    v
}

/// A Codex executable (data-spec §3.4): the native app's `%LOCALAPPDATA%\OpenAI\Codex\bin\<newest>\codex.exe`,
/// then npm's platform binary, npm vendor folders, `%APPDATA%\npm`, `~\.codex\bin`, PATH. With
/// `exe_only`, `.cmd` wrappers are never returned (the app-server is only launched natively).
pub fn find_codex(env: &Env, exe_only: bool) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for sub in newest_subdirs(&env.local_appdata.join("OpenAI").join("Codex").join("bin")) {
        candidates.push(sub.join("codex.exe"));
    }
    let pkg = env
        .appdata
        .join("npm")
        .join("node_modules")
        .join("@openai")
        .join("codex");
    candidates.extend(files_matching(&pkg.join("bin"), |n| {
        n.starts_with("codex-") && n.contains("windows") && n.ends_with(".exe")
    }));
    if let Ok(entries) = std::fs::read_dir(pkg.join("vendor")) {
        let mut vendors: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        vendors.sort();
        for v in vendors {
            candidates.push(v.join("codex").join("codex.exe"));
            candidates.push(v.join("codex").join("codex.cmd"));
        }
    }
    let mut dirs = vec![env.appdata.join("npm"), env.home.join(".codex").join("bin")];
    dirs.extend(path_dirs(env));
    for d in dirs {
        candidates.push(d.join("codex.exe"));
        candidates.push(d.join("codex.cmd"));
    }
    candidates.into_iter().find(|p| {
        p.is_file()
            && (!exe_only
                || p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("exe")))
    })
}

fn app_server_rest() -> &'static Mutex<HashMap<PathBuf, i64>> {
    static REST: OnceLock<Mutex<HashMap<PathBuf, i64>>> = OnceLock::new();
    REST.get_or_init(|| Mutex::new(HashMap::new()))
}

fn app_server_due(profile: &Path, now: i64) -> bool {
    app_server_rest()
        .lock()
        .map(|m| m.get(profile).is_none_or(|t| *t <= now))
        .unwrap_or(true)
}

fn app_server_rest_until(profile: &Path, until: i64) {
    if let Ok(mut m) = app_server_rest().lock() {
        m.insert(profile.to_path_buf(), until);
    }
}

/// Wait for the JSON-RPC response with `id` on `rx` until `deadline`.
pub(crate) fn wait_response(
    rx: &mpsc::Receiver<String>,
    id: i64,
    deadline: Instant,
) -> Option<Value> {
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let line = rx.recv_timeout(left).ok()?;
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if v.get("id").and_then(Value::as_i64) == Some(id) {
            return Some(v);
        }
    }
}

/// Run `codex.exe app-server` hidden and ask for the rate limits. The child is always killed
/// and reaped before returning; the whole exchange is bounded by [`APP_SERVER_TIMEOUT`].
fn app_server_read(exe: &Path, profile: &Path, env: &Env) -> Option<Vec<LimitWindow>> {
    let mut cmd = Command::new(exe);
    cmd.arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if !util::same_path(profile, &env.home_path(".codex")) {
        cmd.env("CODEX_HOME", profile);
    }
    set_flags(&mut cmd, util::CREATE_NO_WINDOW);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => {
            log::warn!("codex: app-server could not start: {}", err.kind());
            return None;
        }
    };
    let deadline = Instant::now() + APP_SERVER_TIMEOUT;
    let (tx, rx) = mpsc::channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        let _ = std::thread::Builder::new()
            .name("codex-app-server-out".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    match line {
                        Ok(l) => {
                            if tx.send(l).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
    }
    let result = (|| {
        let mut stdin = child.stdin.take()?;
        let version = codenotch_core::VERSION;
        let init = json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"codenotch","version":version}}});
        writeln!(stdin, "{init}").ok()?;
        stdin.flush().ok()?;
        let resp = wait_response(&rx, 1, deadline)?;
        if resp.get("error").is_some_and(|e| !e.is_null()) {
            return None;
        }
        writeln!(stdin, "{}", json!({"method":"initialized","params":{}})).ok()?;
        writeln!(
            stdin,
            "{}",
            json!({"id":2,"method":"account/rateLimits/read"})
        )
        .ok()?;
        stdin.flush().ok()?;
        let resp = wait_response(&rx, 2, deadline)?;
        parse_app_server_limits(resp.get("result")?)
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

// ---------------------------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------------------------

/// Outcome of the live read.
enum Live {
    Ok(Vec<LimitWindow>, Option<String>),
    NoWindows,
    /// 401/403 or no usable `auth.json`.
    Auth(String),
    RateLimited(i64, String),
    Failed(String),
}

impl Live {
    fn note(&self) -> Option<&str> {
        match self {
            Live::Ok(..) => None,
            Live::NoWindows => Some(NOTE_NO_WINDOWS),
            Live::Auth(n) | Live::RateLimited(_, n) | Live::Failed(n) => Some(n),
        }
    }
}

fn plan_from_body(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| text(v.get("plan_type")))
}

fn live_read(auth: &CodexAuth, ctx: &FetchContext) -> Live {
    let req = HttpRequest::get(CODEX_USAGE_URL)
        .bearer(&auth.access_token)
        .header("ChatGPT-Account-Id", auth.account_id.as_str())
        .header("Accept", "application/json")
        .header("Cache-Control", "no-cache, no-store")
        .header(
            "User-Agent",
            format!("codenotch/{} (Windows)", codenotch_core::VERSION),
        )
        .timeout(REQUEST_TIMEOUT);
    let lang = ctx.settings.lang();
    let failed = |e: String| Live::Failed(tf(lang, TPL_LIVE_FAILED, &[("e", &e)]));
    let resp = match ctx.http.send(&req) {
        Ok(r) => r,
        Err(err) => return failed(short(&err.to_string())),
    };
    match resp.status {
        200..=299 => {
            let body = resp.text();
            match parse_codex_usage(&body, ctx.now) {
                Ok(w) if w.is_empty() => Live::NoWindows,
                Ok(w) => Live::Ok(w, plan_from_body(&body)),
                Err(err) => failed(short(&format!("parse: {err}"))),
            }
        }
        401 | 403 => Live::Auth(if auth.expired {
            NOTE_EXPIRED.into()
        } else {
            NOTE_REJECTED.into()
        }),
        429 => {
            let wait = resp.retry_after_secs(ctx.now).unwrap_or(0).max(60);
            Live::RateLimited(
                ctx.now + wait,
                tf(lang, TPL_RATE_LIMITED, &[("wait", &wait.to_string())]),
            )
        }
        code => Live::Failed(tf(lang, TPL_LIVE_HTTP, &[("code", &code.to_string())])),
    }
}

/// An active 429 deadline carried by the previous reading.
fn active_backoff(ctx: &FetchContext) -> Option<i64> {
    match ctx.previous.as_ref().map(|r| &r.status) {
        Some(ReadingStatus::RateLimited { until: Some(u) }) if *u > ctx.now => Some(*u),
        _ => None,
    }
}

pub struct CodexProvider;

impl CodexProvider {
    fn fetch_inner(&self, account: &Account, ctx: &FetchContext) -> Reading {
        let now = ctx.now;
        let dir = ctx.env.profile_dir(account, ".codex");
        let present = dir.join("auth.json").is_file()
            || dir.join("sessions").is_dir()
            || find_codex(&ctx.env, false).is_some();
        if !present {
            return Reading::not_installed(account, now);
        }
        let auth = read_auth(&dir, now);
        let identity = auth.as_ref().and_then(|a| a.email.clone());
        let jwt_plan = auth.as_ref().and_then(|a| a.plan.clone());
        let decorate = |mut r: Reading, plan: Option<String>| {
            r.identity = identity.clone();
            r.plan = plan.or_else(|| jwt_plan.clone()).map(|p| plan_label(&p));
            r
        };

        let lang = ctx.settings.lang();
        let live = match (active_backoff(ctx), &auth) {
            (Some(until), _) => {
                let wait = until - now;
                Live::RateLimited(
                    until,
                    tf(lang, TPL_RATE_LIMITED, &[("wait", &wait.to_string())]),
                )
            }
            (None, None) => Live::Auth(NOTE_NOT_SIGNED_IN.into()),
            (None, Some(a)) => live_read(a, ctx),
        };
        if let Live::Ok(windows, plan) = live {
            let mut r = decorate(Reading::fresh(account, now, windows), plan);
            r.note = Some(match &r.plan {
                Some(p) => format!("{p} · via Codex"),
                None => "via Codex".into(),
            });
            return r;
        }

        // Recovery 1: the native app-server (never during a 429 back-off).
        let rate_limited = matches!(live, Live::RateLimited(..));
        if !rate_limited && util::spawn_allowed(&ctx.env) && app_server_due(&dir, now) {
            if let Some(exe) = find_codex(&ctx.env, true) {
                match app_server_read(&exe, &dir, &ctx.env) {
                    Some(windows) => {
                        let mut r = decorate(Reading::fresh(account, now, windows), None);
                        r.note = Some("via Codex app-server".into());
                        return r;
                    }
                    None => {
                        log::info!("codex: app-server gave no rate limits; resting 30 min");
                        app_server_rest_until(&dir, now + APP_SERVER_REST_SECS);
                    }
                }
            }
        }

        // Recovery 2: the newest rollout snapshot, dated by its own timestamp.
        if let Some(snap) = rollout_snapshot(&dir, now) {
            let status = match &live {
                Live::RateLimited(until, _) => ReadingStatus::RateLimited {
                    until: Some(*until),
                },
                _ if now - snap.at <= ROLLOUT_FRESH_SECS => ReadingStatus::Fresh,
                _ => ReadingStatus::Stale,
            };
            let mut r = decorate(Reading::fresh(account, snap.at, snap.windows), snap.plan);
            r.status = status;
            let base = match &r.plan {
                Some(p) => tf(lang, TPL_PLAN_FROM_LAST_RUN, &[("plan", p)]),
                None => t(lang, NOTE_FROM_LAST_RUN).to_string(),
            };
            // A composed note cannot be looked up at display time: translate its parts now
            // (fixed notes through the table, variable ones are already localised).
            r.note = Some(match live.note() {
                Some(n) => format!("{} · {base}", t(lang, n)),
                None => base,
            });
            return r;
        }

        let r = match live {
            Live::RateLimited(until, note) => carry_or(
                account,
                ctx,
                ReadingStatus::RateLimited { until: Some(until) },
                Some(note),
                Some("HTTP 429".into()),
            ),
            Live::Auth(note) => carry_or(account, ctx, ReadingStatus::SignedOut, Some(note), None),
            Live::NoWindows => {
                Reading::with_status(account, now, ReadingStatus::Empty, NOTE_NO_WINDOWS)
            }
            Live::Failed(note) => carry_or(
                account,
                ctx,
                ReadingStatus::Error,
                Some(note.clone()),
                Some(note),
            ),
            Live::Ok(..) => {
                Reading::with_status(account, now, ReadingStatus::Empty, NOTE_NO_SNAPSHOT)
            }
        };
        decorate(r, None)
    }
}

impl Provider for CodexProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Codex
    }

    /// The default profile when `~\.codex\auth.json`, `~\.codex\sessions` or a Codex executable
    /// exists, plus every other profile with an `auth.json` (`~\.codex-<id>`, `CODEX_HOME`).
    /// One profile is named "Codex", the others "Codex (<id>)".
    fn discover(&self, env: &Env) -> Vec<Account> {
        let profiles = codex_profiles(env);
        let single = profiles.len() == 1;
        profiles
            .into_iter()
            .map(|p| {
                let name = if single || p.id == "default" {
                    "Codex".to_string()
                } else {
                    format!("Codex ({})", p.id)
                };
                let mut a = Account::new(ProviderKind::Codex, name);
                if p.id != "default" {
                    a.profile_path = p.dir.to_string_lossy().into_owned();
                }
                a
            })
            .collect()
    }

    fn local_identity(&self, account: &Account, env: &Env) -> LocalIdentity {
        std::fs::read_to_string(env.profile_dir(account, ".codex").join("auth.json"))
            .map(|t| auth_identity(&t))
            .unwrap_or_default()
    }

    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading {
        self.fetch_inner(account, ctx)
    }

    /// Loop period (data-spec §3.6). Manual refreshes are not floored for Codex.
    fn min_interval(&self) -> Duration {
        Duration::from_secs(300)
    }
}

#[cfg(test)]
pub(crate) fn reset_app_server_rest(profile: &Path) {
    if let Ok(mut m) = app_server_rest().lock() {
        m.remove(profile);
    }
}
