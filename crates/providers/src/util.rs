//! Shared helpers: JSON/time parsing, JWT payloads, carry-over readings, CLI lookup and
//! hidden child processes.

use codenotch_core::model::{Reading, ReadingStatus};
use codenotch_core::provider::{Env, FetchContext};
use codenotch_core::Account;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// `CREATE_NO_WINDOW`: the child gets no console window.
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `CREATE_NEW_CONSOLE`: the child gets its own visible console (interactive sign-in only).
pub(crate) const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
/// Environment variable that disables every child process of this crate when set to `1`.
pub const NO_SPAWN_ENV: &str = "CODENOTCH_NO_SPAWN";

/// May this crate start child processes? (`CODENOTCH_NO_SPAWN=1` disables it.)
pub(crate) fn spawn_allowed(env: &Env) -> bool {
    env.var(NO_SPAWN_ENV).as_deref().map(str::trim) != Some("1")
}

// ---------------------------------------------------------------------------------------------
// JSON / time
// ---------------------------------------------------------------------------------------------

/// A finite number from a JSON number or a numeric string.
pub(crate) fn num(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    let n = match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    n.is_finite().then_some(n)
}

/// A non-empty trimmed string.
pub(crate) fn text(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// RFC 3339 string → unix seconds.
pub(crate) fn rfc3339_secs(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|d| d.timestamp())
}

/// Epoch number → unix seconds: values above 10^11 are milliseconds.
pub(crate) fn epoch_secs(n: f64) -> Option<i64> {
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    let secs = if n > 1e11 { n / 1000.0 } else { n };
    Some(secs.round() as i64)
}

/// A time as RFC 3339 text, unix seconds or unix milliseconds (number or digit string).
pub(crate) fn flexible_time(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            if s.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
                return s.parse::<f64>().ok().and_then(epoch_secs);
            }
            rfc3339_secs(s)
        }
        Value::Number(n) => n.as_f64().and_then(epoch_secs),
        _ => None,
    }
}

/// Decode base64url (padding optional). `None` on any invalid character.
pub(crate) fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        })
    }
    let bytes: Vec<u8> = input.trim().bytes().filter(|&b| b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in bytes {
        acc = (acc << 6) | val(b)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

/// The (unverified) JSON payload of a JWT.
pub(crate) fn jwt_payload(token: &str) -> Option<Value> {
    let middle = token.split('.').nth(1)?;
    let bytes = base64url_decode(middle)?;
    serde_json::from_slice(&bytes).ok()
}

/// First letter upper-cased.
pub(crate) fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `weekly_foo_bar` → `Weekly foo bar`.
pub(crate) fn humanise(kind: &str) -> String {
    capitalise(&kind.replace(['_', '-'], " "))
}

/// `GrokBuild` → `Grok Build`.
pub(crate) fn split_camel(s: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        out.push(c);
    }
    out
}

/// Short, single-line, secret-free text for an error note.
pub(crate) fn short(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut t = codenotch_core::logging::redact(flat.trim());
    if t.chars().count() > 160 {
        t = t.chars().take(160).collect();
    }
    t
}

// ---------------------------------------------------------------------------------------------
// Readings
// ---------------------------------------------------------------------------------------------

/// The previous reading's windows, carried over with a new status / note / error. `fetched_at`
/// stays the time the numbers were taken, so age-based staleness keeps working. `None` when
/// there is nothing to carry.
pub(crate) fn carry_over(
    account: &Account,
    ctx: &FetchContext,
    status: ReadingStatus,
    note: Option<String>,
    error: Option<String>,
) -> Option<Reading> {
    let prev = ctx.previous.as_ref()?;
    if prev.account_id != account.id || prev.windows.is_empty() || prev.fetched_at <= 0 {
        return None;
    }
    let mut r = prev.clone();
    r.status = status;
    r.note = note;
    r.error = error;
    Some(r)
}

/// Carry the previous windows with `status`, or build a reading with no windows.
pub(crate) fn carry_or(
    account: &Account,
    ctx: &FetchContext,
    status: ReadingStatus,
    note: Option<String>,
    error: Option<String>,
) -> Reading {
    carry_over(account, ctx, status.clone(), note.clone(), error.clone()).unwrap_or_else(|| {
        let mut r = Reading::new(account, ctx.now);
        r.status = status;
        r.note = note;
        r.error = error;
        r
    })
}

// ---------------------------------------------------------------------------------------------
// Executables and processes
// ---------------------------------------------------------------------------------------------

/// Same folder, ignoring case, slash style and a trailing separator (Windows paths).
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// The directories of the `PATH` variable seen through `env` (tests have none).
pub(crate) fn path_dirs(env: &Env) -> Vec<PathBuf> {
    match env.var("PATH") {
        Some(p) => std::env::split_paths(&p)
            .filter(|d| !d.as_os_str().is_empty())
            .collect(),
        None => Vec::new(),
    }
}

/// First existing file `<dir>\<name>` over `dirs` × `names` (dirs outer).
pub(crate) fn first_file(
    dirs: &[PathBuf],
    names: &[&str],
    skip: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file() && !skip(p))
}

/// Remove the variables a parent Claude Code session would leak into a child.
pub(crate) fn scrub_claude_env(cmd: &mut Command, extra: &[&str]) {
    for (key, _) in std::env::vars_os() {
        let k = key.to_string_lossy();
        if k == "CLAUDECODE"
            || k.starts_with("CLAUDE_CODE_")
            || extra.iter().any(|e| k.eq_ignore_ascii_case(e))
        {
            cmd.env_remove(key);
        }
    }
}

/// Set Windows creation flags (no-op elsewhere).
pub(crate) fn set_flags(cmd: &mut Command, flags: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(flags);
    }
    #[cfg(not(windows))]
    let _ = (cmd, flags);
}

/// Wait for `child` until `timeout`, polling every `poll`; on timeout kill it and reap it.
/// Returns the exit success, or `None` when it was killed.
pub(crate) fn wait_or_kill(child: &mut Child, timeout: Duration, poll: Duration) -> Option<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.success()),
            Ok(None) => {}
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(poll);
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// Kill-on-close job object: every process put in it (and every descendant started after that)
/// dies when the job is terminated or dropped. `.cmd` shims make the direct child `cmd.exe`, so
/// killing the child alone would leave `node` running hidden.
#[cfg(windows)]
pub(crate) struct KillOnCloseJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl KillOnCloseJob {
    pub(crate) fn new() -> Option<Self> {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // SAFETY: plain Win32 calls with null (default) attributes; the handle is owned by the
        // returned value (closed in Drop, also on the error path below).
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let job = KillOnCloseJob(h);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            (ok != 0).then_some(job)
        }
    }

    /// Put `child` in the job (`false` on failure).
    pub(crate) fn assign(&self, child: &Child) -> bool {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: both handles are valid for the duration of the call.
        unsafe {
            windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
                self.0,
                child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
            ) != 0
        }
    }

    /// Kill every process in the job now.
    pub(crate) fn terminate(&self) {
        // SAFETY: valid job handle.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        // SAFETY: we own the handle; closing the last handle kills what is left in the job.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// [`wait_or_kill`] for a child that may start descendants (CLI renewals through `.cmd` shims):
/// the child is put in a kill-on-close job right after spawn, so on timeout — and when this
/// returns — the whole tree is gone. Without a job (creation refused), falls back to
/// `taskkill /T` on timeout.
pub(crate) fn wait_or_kill_tree(
    env: &Env,
    child: &mut Child,
    timeout: Duration,
    poll: Duration,
) -> Option<bool> {
    #[cfg(windows)]
    {
        let job = KillOnCloseJob::new().filter(|job| job.assign(child));
        let pid = child.id();
        let deadline = Instant::now() + timeout;
        let finished = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status.success()),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(poll),
                _ => break None,
            }
        };
        if finished.is_none() {
            match &job {
                Some(job) => job.terminate(),
                None => kill_tree(env, pid),
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        drop(job);
        finished
    }
    #[cfg(not(windows))]
    {
        let _ = env;
        wait_or_kill(child, timeout, poll)
    }
}

/// Kill a whole process tree with `taskkill /T /F` (hidden).
pub(crate) fn kill_tree(env: &Env, pid: u32) {
    let root = env
        .var("SystemRoot")
        .unwrap_or_else(|| "C:\\Windows".into());
    let taskkill = PathBuf::from(root).join("System32").join("taskkill.exe");
    let mut cmd = Command::new(taskkill);
    cmd.args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    set_flags(&mut cmd, CREATE_NO_WINDOW);
    if let Ok(mut c) = cmd.spawn() {
        let _ = wait_or_kill(&mut c, Duration::from_secs(10), Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------------------------------------
// Renewal timing (shared by Claude and Grok)
// ---------------------------------------------------------------------------------------------

/// Renew only when the token expires within this margin (under Claude Code's own 5-min gate).
pub const RENEW_MARGIN_MS: i64 = 4 * 60 * 1000;
/// Wait between two renewal launches.
pub const RENEW_COOLDOWN_MS: i64 = 10 * 60 * 1000;
/// Cap of the doubling wait after launches that did not move the expiry.
pub const RENEW_RETRY_CAP_MS: i64 = 60 * 60 * 1000;
/// A renewal CLI run is killed after this.
pub const RENEW_TIMEOUT: Duration = Duration::from_secs(30);

/// Should a renewal CLI be launched now? Pure (all times in ms).
///
/// * unknown expiry → never;
/// * expiry more than [`RENEW_MARGIN_MS`] away → no;
/// * never attempted → yes;
/// * last attempt was for this same expiry (it did not move) → wait
///   `10 min × 2^min(failures, 16)`, capped at 60 min; otherwise wait 10 min.
pub fn should_renew(
    expires_at_ms: Option<i64>,
    now_ms: i64,
    attempted_for: Option<i64>,
    last_attempt_ms: Option<i64>,
    failures: u32,
) -> bool {
    let Some(exp) = expires_at_ms else {
        return false;
    };
    if exp > now_ms + RENEW_MARGIN_MS {
        return false;
    }
    let Some(last) = last_attempt_ms else {
        return true;
    };
    let wait = if attempted_for == Some(exp) {
        RENEW_COOLDOWN_MS
            .saturating_mul(1i64 << failures.min(16))
            .min(RENEW_RETRY_CAP_MS)
    } else {
        RENEW_COOLDOWN_MS
    };
    now_ms.saturating_sub(last) >= wait
}

/// In-memory renewal bookkeeping for one credential file.
#[derive(Debug, Default, Clone)]
pub(crate) struct RenewState {
    pub attempted_for: Option<i64>,
    pub last_attempt: Option<i64>,
    pub failures: u32,
}

impl RenewState {
    /// Decide and, when due, record the attempt (so the caller then launches).
    pub(crate) fn begin(&mut self, expires_at_ms: Option<i64>, now_ms: i64) -> bool {
        if !should_renew(
            expires_at_ms,
            now_ms,
            self.attempted_for,
            self.last_attempt,
            self.failures,
        ) {
            return false;
        }
        if self.attempted_for != expires_at_ms {
            self.failures = 0;
        }
        self.last_attempt = Some(now_ms);
        self.attempted_for = expires_at_ms;
        self.failures = self.failures.saturating_add(1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `cmd /c outer.cmd` → `cmd /c beat.cmd` (a grandchild appending to a file every second
    /// for at most 60 s): on timeout the grandchild must die with the child.
    #[cfg(windows)]
    #[test]
    fn timeout_kills_the_whole_process_tree() {
        let dir = tempfile::tempdir().unwrap();
        let beat = dir.path().join("beat.txt");
        std::fs::write(
            dir.path().join("beat.cmd"),
            "@echo off\r\nfor /l %%i in (1,1,60) do (\r\n  echo x>>\"%~dp0beat.txt\"\r\n  ping -n 2 127.0.0.1 >nul\r\n)\r\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("outer.cmd"),
            "@cmd /d /c \"%~dp0beat.cmd\"\r\n",
        )
        .unwrap();
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/d", "/c"])
            .arg(dir.path().join("outer.cmd"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        set_flags(&mut cmd, CREATE_NO_WINDOW);
        let mut child = cmd.spawn().unwrap();
        let env = Env::for_test(dir.path());
        // Called right after spawn (as the providers do); the grandchild beats meanwhile.
        let out = wait_or_kill_tree(
            &env,
            &mut child,
            Duration::from_millis(3000),
            Duration::from_millis(50),
        );
        assert_eq!(out, None, "timed out");
        assert!(beat.exists(), "the grandchild ran before the timeout");
        std::thread::sleep(Duration::from_millis(1500));
        let len = std::fs::metadata(&beat).unwrap().len();
        std::thread::sleep(Duration::from_millis(3000));
        assert_eq!(
            std::fs::metadata(&beat).unwrap().len(),
            len,
            "the grandchild is still running"
        );
    }

    #[test]
    fn base64url_round_trip() {
        assert_eq!(base64url_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(base64url_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64url_decode("_-8").unwrap(), vec![0xFF, 0xEF]);
        assert!(base64url_decode("a b").is_none());
    }

    #[test]
    fn jwt_payload_reads_middle_part() {
        // {"exp":100,"email":"a@b.c"}
        let payload = "eyJleHAiOjEwMCwiZW1haWwiOiJhQGIuYyJ9";
        let v = jwt_payload(&format!("h.{payload}.sig")).unwrap();
        assert_eq!(v["exp"], 100);
        assert!(jwt_payload("nodots").is_none());
        assert!(jwt_payload("a.!!!.b").is_none());
    }

    #[test]
    fn flexible_times() {
        assert_eq!(
            flexible_time(Some(&json!("1970-01-01T00:01:40Z"))),
            Some(100)
        );
        assert_eq!(
            flexible_time(Some(&json!("2026-10-02T06:59:59.000000+00:00"))),
            Some(1_790_924_399)
        );
        assert_eq!(
            flexible_time(Some(&json!(1_792_000_000))),
            Some(1_792_000_000)
        );
        assert_eq!(
            flexible_time(Some(&json!(1_792_000_000_000i64))),
            Some(1_792_000_000)
        );
        assert_eq!(
            flexible_time(Some(&json!("1792000000"))),
            Some(1_792_000_000)
        );
        assert_eq!(flexible_time(Some(&json!(-5))), None);
        assert_eq!(flexible_time(Some(&json!("soon"))), None);
        assert_eq!(flexible_time(Some(&json!(null))), None);
        assert_eq!(flexible_time(None), None);
    }

    #[test]
    fn text_helpers() {
        assert_eq!(humanise("weekly_foo_bar"), "Weekly foo bar");
        assert_eq!(split_camel("GrokBuild"), "Grok Build");
        assert_eq!(split_camel("grok"), "grok");
        assert_eq!(capitalise("max"), "Max");
        assert_eq!(num(Some(&json!("42.5"))), Some(42.5));
        assert_eq!(num(Some(&json!(true))), None);
        assert!(short("line\nbreak").find('\n').is_none());
    }

    #[test]
    fn renew_timing_rules() {
        let min = 60_000;
        let now = 1_000 * min;
        // unknown expiry, far expiry
        assert!(!should_renew(None, now, None, None, 0));
        assert!(!should_renew(Some(now + 5 * min), now, None, None, 0));
        // inside the 4-min margin, never tried
        assert!(should_renew(Some(now + 4 * min), now, None, None, 0));
        assert!(should_renew(Some(now - min), now, None, None, 0));
        // tried for another expiry: 10-min cooldown
        assert!(!should_renew(
            Some(now),
            now,
            Some(now - 99),
            Some(now - 9 * min),
            1
        ));
        assert!(should_renew(
            Some(now),
            now,
            Some(now - 99),
            Some(now - 10 * min),
            1
        ));
        // same expiry: doubling (1 failure → 20 min, 2 → 40, 3+ → capped 60)
        assert!(!should_renew(
            Some(now),
            now,
            Some(now),
            Some(now - 19 * min),
            1
        ));
        assert!(should_renew(
            Some(now),
            now,
            Some(now),
            Some(now - 20 * min),
            1
        ));
        assert!(!should_renew(
            Some(now),
            now,
            Some(now),
            Some(now - 39 * min),
            2
        ));
        assert!(!should_renew(
            Some(now),
            now,
            Some(now),
            Some(now - 59 * min),
            30
        ));
        assert!(should_renew(
            Some(now),
            now,
            Some(now),
            Some(now - 60 * min),
            30
        ));
    }

    #[test]
    fn renew_state_records_attempts() {
        let mut s = RenewState::default();
        assert!(s.begin(Some(10), 100));
        assert_eq!(
            (s.attempted_for, s.last_attempt, s.failures),
            (Some(10), Some(100), 1)
        );
        assert!(!s.begin(Some(10), 101), "cool-down");
        // a new expiry resets the failure count
        assert!(s.begin(Some(20), 100 + RENEW_COOLDOWN_MS));
        assert_eq!(s.failures, 1);
    }

    #[test]
    fn path_comparison_ignores_case_and_slashes() {
        assert!(same_path(
            Path::new("C:\\Users\\user\\.claude"),
            Path::new("c:/users/user/.claude/")
        ));
        assert!(!same_path(
            Path::new("C:\\Users\\user\\.claude"),
            Path::new("C:\\Users\\user\\.claude-2")
        ));
    }

    #[test]
    fn no_spawn_switch() {
        let dir = Path::new("Z:/t");
        assert!(spawn_allowed(&Env::for_test(dir)));
        assert!(!spawn_allowed(
            &Env::for_test(dir).with_var(NO_SPAWN_ENV, "1")
        ));
        assert!(spawn_allowed(
            &Env::for_test(dir).with_var(NO_SPAWN_ENV, "0")
        ));
    }
}
