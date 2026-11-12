//! Antigravity (provider id `gemini` is an alias).
//!
//! Sources, chosen per fetch:
//! * **Official CLI** (`%LOCALAPPDATA%\agy\bin\agy.exe`, or `agy.exe` on an absolute PATH entry):
//!   `agy --sandbox --print-timeout 30s --print /usage` in a hidden ConPTY inside a kill-on-close
//!   job, 70 s timeout, ≤ 64 KB output, ANSI/CR stripped, auto-update off. Rows
//!   `<Family> <Lane> Limit Remaining <pct>% <reset>` (used = 100 − pct). The CLI is heavy: it runs
//!   on the first fetch of the process and on a manual refresh, at most once per 5 min, never on
//!   the timer (a timer fetch returns the previous reading).
//! * **Legacy**, when the official CLI is absent:
//!   1. the IDE's local language server (`language_server*` or `agy.exe --hub`, found with a
//!      hidden PowerShell CIM query + `netstat`): `POST https://127.0.0.1:<port>/exa.language_server_pb.
//!      LanguageServerService/RetrieveUserQuotaSummary`, header `x-codeium-csrf-token`, body
//!      `{"forceRefresh":true}`; self-signed certificate accepted for 127.0.0.1 only;
//!   2. the bundled `~\.gemini\bin\agy.exe`, under the same CLI rules;
//!   3. a percentage read before is kept (stale) rather than degraded to a count;
//!   4. today's `source == "MODEL"` steps in `~\.gemini\antigravity*\brain\*\.system_generated\
//!      logs\transcript.jsonl`: a count window (`requests`), no percentage.
//!
//! Not implemented on purpose: the Windows Credential Manager path (`gemini:antigravity` →
//! cloudcode-pa `loadCodeAssist` / `retrieveUserQuotaSummary`): it reads another product's token
//! out of an OS-protected vault.

use crate::util;
use codenotch_core::config::{AntigravityLimit, AntigravityModel};
use codenotch_core::model::{LimitWindow, Reading, ReadingStatus, WindowKind};
use codenotch_core::provider::{Env, FetchContext};
use codenotch_core::{Account, HttpClient, HttpError, HttpRequest, HttpResponse};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Minimum spacing of CLI runs (successes and failures alike).
pub(crate) const CLI_TTL_SECS: i64 = 300;
const CLI_TIMEOUT: Duration = Duration::from_secs(70);
const CLI_MAX_OUTPUT: usize = 64 * 1024;
const CLI_ARGS: [&str; 5] = ["--sandbox", "--print-timeout", "30s", "--print", "/usage"];
const NO_AUTO_UPDATE: (&str, &str) = ("AGY_CLI_DISABLE_AUTO_UPDATE", "true");
pub(crate) const LS_SERVICE: &str =
    "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const CSRF_HEADER: &str = "x-codeium-csrf-token";
/// Also a marker read back from the previous reading's note: stays English (display-time only).
const VIA_CLI: &str = "via Antigravity CLI";
pub(crate) const NOTE_CLI_KEPT: &str = "via Antigravity CLI — last reading kept";
pub(crate) const NOTE_OPEN_APP: &str = "Open Antigravity to read its quota";
pub(crate) const NOTE_CLOSED: &str = "Antigravity is closed — last reading kept";
pub(crate) const NOTE_NO_SPAWN: &str = "Antigravity CLI read skipped (CODENOTCH_NO_SPAWN=1)";
pub(crate) const NOTE_CLI_SPACING: &str =
    "Antigravity CLI reads are limited to one every 5 minutes";
pub(crate) const ERR_NO_REPORT: &str = "the CLI did not return a quota report";
pub(crate) const ERR_NO_WINDOWS: &str = "the CLI returned no recognised quota windows";
pub(crate) const LABEL_REQUESTS: &str = "Requests today · no limit published";

/// Last CLI attempt per account id (unix secs), this process.
static CLI_ATTEMPTS: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);
/// Last local-server endpoint that answered (its port changes on every IDE launch).
static ENDPOINT: Mutex<Option<Endpoint>> = Mutex::new(None);

// ---------------------------------------------------------------------------------------------
// Locations
// ---------------------------------------------------------------------------------------------

/// The official standalone CLI.
pub(crate) fn official_agy(env: &Env) -> Option<PathBuf> {
    let p = env.local_appdata.join("agy").join("bin").join("agy.exe");
    if p.is_file() {
        return Some(p);
    }
    util::find_on_path(env, "agy.exe")
}

/// The copy the IDE / VS Code extension installs.
pub(crate) fn bundled_agy(env: &Env) -> Option<PathBuf> {
    Some(env.home.join(".gemini").join("bin").join("agy.exe")).filter(|p| p.is_file())
}

/// Every `~\.gemini\antigravity*` state folder, sorted.
pub(crate) fn state_roots(env: &Env) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(env.home.join(".gemini")) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("antigravity"))
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

pub(crate) fn installed(env: &Env) -> bool {
    official_agy(env).is_some() || bundled_agy(env).is_some() || !state_roots(env).is_empty()
}

// ---------------------------------------------------------------------------------------------
// Lanes
// ---------------------------------------------------------------------------------------------

/// "5-hour Limit" / "Weekly Limit" from `gemini-5h`-style ids or CLI labels.
pub(crate) fn lane_name(id: &str) -> Option<&'static str> {
    let id = id.to_lowercase();
    if id.contains("weekly") {
        Some("Weekly Limit")
    } else if ["5h", "five hour", "hourly"].iter().any(|k| id.contains(k)) {
        Some("5-hour Limit")
    } else {
        None
    }
}

/// Window length from a recognised cadence word; a model name alone is no evidence.
pub(crate) fn lane_seconds(raw: &str) -> Option<i64> {
    let n = raw.trim().to_lowercase().replace('_', "-");
    let v = n.strip_suffix(" remaining").unwrap_or(&n);
    let v = v.strip_suffix(" limit").unwrap_or(v);
    if v == "weekly" || v.ends_with("-weekly") || v.ends_with(" weekly") {
        return Some(util::SEVEN_DAYS);
    }
    let five = [
        "session",
        "5h",
        "5-hour",
        "five hour",
        "five-hour",
        "hourly",
    ]
    .iter()
    .any(|c| v == *c || v.strip_suffix(c).is_some_and(|p| p.ends_with(['-', ' '])));
    five.then_some(util::FIVE_HOURS)
}

fn kind_for(len: Option<i64>) -> WindowKind {
    match len {
        Some(util::FIVE_HOURS) => WindowKind::Session,
        Some(util::SEVEN_DAYS) => WindowKind::Weekly,
        _ => WindowKind::Other,
    }
}

/// Groups in source order; inside a group the 5-hour lane before the weekly one.
pub(crate) fn order_lanes(windows: &mut [LimitWindow]) {
    let mut groups: Vec<Option<String>> = Vec::new();
    for w in windows.iter() {
        if !groups.contains(&w.group) {
            groups.push(w.group.clone());
        }
    }
    let rank = |w: &LimitWindow| match lane_name(&w.id) {
        Some("5-hour Limit") => 0,
        Some(_) => 1,
        None => 2,
    };
    windows.sort_by_key(|w| (groups.iter().position(|g| *g == w.group), rank(w)));
}

// ---------------------------------------------------------------------------------------------
// Lane picker
// ---------------------------------------------------------------------------------------------

fn family_of(w: &LimitWindow) -> &'static str {
    let id = w.id.to_lowercase();
    if id.starts_with("gemini") {
        "gemini"
    } else if id.starts_with("3p") || id.starts_with("claude") {
        "3p"
    } else {
        ""
    }
}

fn lane_text(w: &LimitWindow) -> String {
    format!("{} {}", w.id, w.label_key).to_lowercase()
}

fn is_five_hour(w: &LimitWindow) -> bool {
    let t = lane_text(w);
    [
        "5h",
        "5-hour",
        "five hour",
        "five-hour",
        "hourly",
        "session",
    ]
    .iter()
    .any(|k| t.contains(k))
}

fn is_weekly(w: &LimitWindow) -> bool {
    lane_text(w).contains("weekly")
}

/// Highest used percent (count windows and unknown percents excluded); ties → lower id.
fn tightest<'a>(lanes: impl Iterator<Item = &'a LimitWindow>) -> Option<&'a LimitWindow> {
    lanes
        .filter(|w| w.count.is_none())
        .filter_map(|w| w.used_percent.map(|p| (w, p)))
        .fold(
            None,
            |best: Option<(&LimitWindow, f64)>, (w, p)| match best {
                Some((b, bp)) if bp > p || (bp == p && b.id <= w.id) => Some((b, bp)),
                _ => Some((w, p)),
            },
        )
        .map(|(w, _)| w)
}

/// See [`crate::antigravity_headline`].
pub(crate) fn headline(
    windows: &[LimitWindow],
    limit: AntigravityLimit,
    model: AntigravityModel,
) -> (Option<String>, Option<String>) {
    let fam = match model {
        AntigravityModel::Gemini => "gemini",
        AntigravityModel::ThirdParty => "3p",
    };
    let family: Vec<&LimitWindow> = windows.iter().filter(|w| family_of(w) == fam).collect();
    let lanes: Vec<&LimitWindow> = if family.is_empty() {
        windows.iter().collect()
    } else {
        family
    };
    let chosen = match limit {
        AntigravityLimit::Automatic => None,
        AntigravityLimit::FiveHour => tightest(lanes.iter().copied().filter(|w| is_five_hour(w))),
        AntigravityLimit::Weekly => tightest(lanes.iter().copied().filter(|w| is_weekly(w))),
    };
    let head = chosen
        .or_else(|| {
            tightest(
                lanes
                    .iter()
                    .copied()
                    .filter(|w| w.used_percent.is_some_and(|p| p < 100.0)),
            )
        })
        .or_else(|| tightest(lanes.iter().copied()))
        .or_else(|| lanes.first().copied())
        .map(|w| w.id.clone());
    let weekly = tightest(lanes.iter().copied().filter(|w| is_weekly(w)))
        .map(|w| w.id.clone())
        .filter(|id| Some(id) != head.as_ref());
    (head, weekly)
}

fn with_lanes(mut r: Reading, ctx: &FetchContext) -> Reading {
    let (h, w) = headline(
        &r.windows,
        ctx.settings.antigravity_limit,
        ctx.settings.antigravity_model,
    );
    r.headline_window = h;
    r.weekly_window = w;
    r
}

// ---------------------------------------------------------------------------------------------
// CLI output
// ---------------------------------------------------------------------------------------------

/// Strip ANSI escapes (CSI, OSC, two-byte) and normalise CR / CRLF to LF.
pub(crate) fn sanitize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    for n in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(&n) if ('\x40'..='\x5f').contains(&n) => {
                    chars.next();
                }
                _ => {}
            },
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    out.push('\n');
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// `±HH` or `±HHMM` (≤ 14 h).
fn numeric_offset(zone: &str) -> Option<chrono::FixedOffset> {
    let (sign, digits) = match zone.strip_prefix('+') {
        Some(d) => (1, d),
        None => (-1, zone.strip_prefix('-')?),
    };
    if !matches!(digits.len(), 2 | 4) || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h: i32 = digits[..2].parse().ok()?;
    let m: i32 = if digits.len() == 4 {
        digits[2..].parse().ok()?
    } else {
        0
    };
    if h > 14 || m > 59 || (h == 14 && m != 0) {
        return None;
    }
    chrono::FixedOffset::east_opt(sign * (h * 3600 + m * 60))
}

/// Reset text → unix secs: RFC 3339, `YYYY-MM-DD HH:MM UTC|GMT|Z`, `… ±HH[MM]`, or `… ABBR`
/// (an abbreviation is resolved in the host zone at that date, through `local`).
pub(crate) fn parse_reset_with(
    text: &str,
    local: impl FnOnce(
        &chrono::NaiveDateTime,
    ) -> chrono::LocalResult<chrono::DateTime<chrono::FixedOffset>>,
) -> Result<i64, String> {
    use chrono::{LocalResult, NaiveDateTime};
    let text = text.trim();
    if let Some(t) = util::rfc3339_secs(text) {
        return Ok(t);
    }
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() != 3 {
        return Err("invalid reset time".into());
    }
    let wall = format!("{} {}", parts[0], parts[1]);
    let naive =
        NaiveDateTime::parse_from_str(&wall, "%Y-%m-%d %H:%M").map_err(|_| "invalid reset time")?;
    if naive.format("%Y-%m-%d %H:%M").to_string() != wall {
        return Err("invalid reset time".into());
    }
    let zone = parts[2];
    if matches!(zone, "UTC" | "GMT" | "Z") {
        return Ok(naive.and_utc().timestamp());
    }
    if zone.starts_with(['+', '-']) {
        let off = numeric_offset(zone).ok_or("invalid reset time zone")?;
        return match naive.and_local_timezone(off) {
            LocalResult::Single(dt) => Ok(dt.timestamp()),
            _ => Err("invalid reset time".into()),
        };
    }
    if !zone.bytes().all(|c| c.is_ascii_alphabetic()) {
        return Err("invalid reset time zone".into());
    }
    match local(&naive) {
        LocalResult::Single(dt) => Ok(dt.timestamp()),
        LocalResult::Ambiguous(..) => Err("ambiguous local reset time".into()),
        LocalResult::None => Err("invalid local reset time".into()),
    }
}

fn parse_reset(text: &str) -> Result<i64, String> {
    use chrono::TimeZone;
    parse_reset_with(text, |n| {
        chrono::Local
            .from_local_datetime(n)
            .map(|d| d.fixed_offset())
    })
}

/// `/usage` report → windows. Any malformed row fails the whole report (never a dummy 0 %).
pub(crate) fn parse_quota_with(
    text: &str,
    reset: &dyn Fn(&str) -> Result<i64, String>,
) -> Result<Vec<LimitWindow>, String> {
    let clean = sanitize(text);
    if !clean.lines().any(|l| l.trim() == "Quota:") {
        return Err(ERR_NO_REPORT.into());
    }
    let mut out = Vec::new();
    for line in clean.lines().filter(|l| l.contains("Limit Remaining")) {
        let (left, reset_text) = line.rsplit_once('%').ok_or("invalid quota row")?;
        let (label, pct) = left
            .trim()
            .rsplit_once(char::is_whitespace)
            .ok_or("missing quota percentage")?;
        let remaining: f64 = pct.trim().parse().map_err(|_| "invalid quota percentage")?;
        if !remaining.is_finite() || !(0.0..=100.0).contains(&remaining) {
            return Err("quota percentage out of range".into());
        }
        let at = reset(reset_text)?;
        if at <= 0 {
            return Err("invalid quota reset time".into());
        }
        let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
        let label = label
            .strip_suffix(" Remaining")
            .ok_or("unknown quota label")?
            .to_string();
        let group = [" Weekly Limit", " Five Hour Limit"]
            .iter()
            .find_map(|s| label.strip_suffix(s))
            .map(str::to_string);
        let short = label
            .replace(" Models", "")
            .replace(" models", "")
            .replace(" and ", "/")
            .replace(" Weekly Limit", " · Weekly")
            .replace(" Five Hour Limit", " · 5h");
        let shown = match (lane_name(&label), &group) {
            (Some(l), Some(_)) => l.to_string(),
            _ => short,
        };
        let len = lane_seconds(&label);
        let mut w = LimitWindow::new(label.clone(), shown, kind_for(len))
            .used(100.0 - remaining)
            .resets_at(at);
        w.window_seconds = len;
        w.group = group;
        out.push(w);
    }
    if out.is_empty() {
        return Err(ERR_NO_WINDOWS.into());
    }
    order_lanes(&mut out);
    Ok(out)
}

pub(crate) fn parse_quota(text: &str) -> Result<Vec<LimitWindow>, String> {
    parse_quota_with(text, &parse_reset)
}

/// Run an `agy.exe` and parse its `/usage` report.
fn read_cli(env: &Env, agy: &Path) -> Result<Vec<LimitWindow>, String> {
    #[cfg(windows)]
    {
        let dir = std::env::temp_dir().join("codenotch-agy-work");
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create the CLI working folder ({})", e.kind()))?;
        let raw = crate::process::run_pseudo_console(
            env,
            agy,
            &CLI_ARGS,
            Some(&dir),
            &[NO_AUTO_UPDATE],
            CLI_TIMEOUT,
            CLI_MAX_OUTPUT,
        )
        .map_err(|e| format!("Antigravity CLI: {e}"))?;
        parse_quota(&String::from_utf8_lossy(&raw))
    }
    #[cfg(not(windows))]
    {
        let _ = (
            env,
            agy,
            CLI_ARGS,
            NO_AUTO_UPDATE,
            CLI_TIMEOUT,
            CLI_MAX_OUTPUT,
        );
        Err("the Antigravity CLI runner needs Windows".into())
    }
}

/// Is a CLI run allowed now? First fetch of the process, or a manual refresh, at most once per
/// `CLI_TTL_SECS`, and not while the previous CLI reading is younger than that.
pub(crate) fn cli_due(last_attempt: Option<i64>, ctx: &FetchContext) -> bool {
    let spaced = match last_attempt {
        None => true,
        Some(t) => ctx.manual && ctx.now - t >= CLI_TTL_SECS,
    };
    let recent = ctx.previous.as_ref().is_some_and(|p| {
        !p.windows.is_empty() && p.shows_figures() && p.age(ctx.now) < CLI_TTL_SECS
    });
    spaced && !recent
}

fn last_attempt(account: &Account) -> Option<i64> {
    CLI_ATTEMPTS
        .lock()
        .ok()?
        .as_ref()?
        .get(&account.id)
        .copied()
}

fn note_attempt(account: &Account, now: i64) {
    if let Ok(mut map) = CLI_ATTEMPTS.lock() {
        map.get_or_insert_with(HashMap::new)
            .insert(account.id.clone(), now);
    }
}

// ---------------------------------------------------------------------------------------------
// Local language server
// ---------------------------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
pub(crate) struct Endpoint {
    pub ports: Vec<u16>,
    csrf: String,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("ports", &self.ports)
            .finish()
    }
}

/// `--flag value` or `--flag=value`.
pub(crate) fn flag_value(line: &str, flag: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    while let Some(part) = parts.next() {
        let value = if part == flag {
            parts.next()?
        } else if let Some(v) = part.strip_prefix(flag).and_then(|r| r.strip_prefix('=')) {
            v
        } else {
            continue;
        };
        let value = value.trim_matches('"');
        return (!value.is_empty()).then(|| value.to_string());
    }
    None
}

/// From `pid<TAB>commandline` lines: the IDE's language server first, else `agy --hub`; only a
/// process carrying a CSRF token.
pub(crate) fn server_in(table: &str) -> Option<(u32, String)> {
    let servers: Vec<&str> = table
        .lines()
        .filter(|l| l.contains("--csrf_token"))
        .collect();
    let line = servers
        .iter()
        .find(|l| l.contains("language_server"))
        .or(servers.first())?;
    let (pid, cmdline) = line.split_once('\t')?;
    Some((
        pid.trim().parse().ok()?,
        flag_value(cmdline, "--csrf_token")?,
    ))
}

/// `netstat -ano -p TCP` → listening ports of `pid`.
pub(crate) fn listening_ports(netstat: &str, pid: u32) -> Vec<u16> {
    let pid = pid.to_string();
    let mut ports: Vec<u16> = netstat
        .lines()
        .filter(|l| l.contains("LISTENING"))
        .filter_map(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            if cols.len() < 5 || cols[4] != pid {
                return None;
            }
            cols[1].rsplit(':').next()?.parse().ok()
        })
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// The process table holds the token (command line); the port is only in the listening table.
fn discover_endpoint(env: &Env) -> Option<Endpoint> {
    if util::spawn_disabled(env) {
        return None;
    }
    let system = PathBuf::from(
        env.var("SystemRoot")
            .unwrap_or_else(|| r"C:\Windows".into()),
    )
    .join("System32");
    let ps = system
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let query = "Get-CimInstance Win32_Process -Filter \"Name LIKE '%language_server%' OR Name = 'agy.exe'\" | \
                 ForEach-Object { \"$($_.ProcessId)`t$($_.CommandLine)\" }";
    let table = crate::process::run_hidden(
        env,
        &ps,
        &["-NoProfile", "-NonInteractive", "-Command", query],
        None,
        Duration::from_secs(20),
        256 * 1024,
    )
    .ok()?;
    let (pid, csrf) = server_in(&String::from_utf8_lossy(&table.stdout))?;
    let netstat = crate::process::run_hidden(
        env,
        &system.join("NETSTAT.EXE"),
        &["-ano", "-p", "TCP"],
        None,
        Duration::from_secs(15),
        1024 * 1024,
    )
    .ok()?;
    let ports = listening_ports(&String::from_utf8_lossy(&netstat.stdout), pid);
    (!ports.is_empty()).then_some(Endpoint { ports, csrf })
}

/// `response.groups[].buckets[]` → windows; the server reports what **remains**.
pub(crate) fn windows_from_bridge(v: &Value) -> Vec<LimitWindow> {
    let mut out = Vec::new();
    let Some(groups) = v.pointer("/response/groups").and_then(Value::as_array) else {
        return out;
    };
    for g in groups {
        let gname = g.get("displayName").and_then(Value::as_str);
        for b in g
            .get("buckets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(rem) =
                util::json_f64(b.get("remainingFraction")).filter(|r| (0.0..=1.0).contains(r))
            else {
                continue;
            };
            let bname = b.get("displayName").and_then(Value::as_str);
            let id = b
                .get("bucketId")
                .and_then(Value::as_str)
                .or(gname)
                .unwrap_or("quota")
                .to_string();
            let len = ["window", "bucketId", "displayName"]
                .iter()
                .map(|k| b.get(*k).and_then(Value::as_str).and_then(lane_seconds))
                .fold(None, |acc: Option<i64>, d| match (acc, d) {
                    (Some(util::SEVEN_DAYS), _) => acc,
                    (_, Some(util::SEVEN_DAYS)) => d,
                    (None, d) => d,
                    (acc, _) => acc,
                });
            let label = lane_name(&id)
                .or(gname)
                .or(bname)
                .unwrap_or("Usage")
                .to_string();
            let mut w = LimitWindow::new(id, label, kind_for(len)).used((1.0 - rem) * 100.0);
            w.resets_at = util::json_rfc3339(b.get("resetTime"));
            w.window_seconds = len;
            w.group = gname.map(str::to_string);
            out.push(w);
        }
    }
    order_lanes(&mut out);
    out
}

/// Try every port of the endpoint; the first that answers with groups wins.
pub(crate) fn bridge_quota(
    http: &dyn HttpClient,
    ep: &Endpoint,
) -> Result<Vec<LimitWindow>, String> {
    let mut last = String::from("no port answered");
    for port in &ep.ports {
        let req = HttpRequest::post(format!("https://127.0.0.1:{port}{LS_SERVICE}"))
            .json(&serde_json::json!({"forceRefresh": true}))
            .header(CSRF_HEADER, ep.csrf.as_str())
            .timeout(Duration::from_secs(10));
        match http.send(&req) {
            Ok(r) if r.is_success() => match r.json::<Value>() {
                Ok(v) => {
                    let w = windows_from_bridge(&v);
                    if !w.is_empty() {
                        return Ok(w);
                    }
                    last = format!("port {port}: no recognisable groups");
                }
                Err(_) => last = format!("port {port}: unreadable JSON"),
            },
            Ok(r) => last = format!("port {port}: HTTP {}", r.status),
            Err(e) => last = format!("port {port}: {e}"),
        }
    }
    Err(last)
}

/// HTTPS client for `https://127.0.0.1:<port>/…` only, accepting the language server's
/// self-signed certificate. Refuses every other URL.
pub(crate) struct LoopbackHttp {
    agent: Option<ureq::Agent>,
}

impl LoopbackHttp {
    pub(crate) fn new() -> Self {
        let agent = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .build()
            .ok()
            .map(|tls| {
                ureq::AgentBuilder::new()
                    .tls_connector(std::sync::Arc::new(tls))
                    .redirects(0)
                    .try_proxy_from_env(false)
                    .timeout_connect(Duration::from_secs(5))
                    .build()
            });
        Self { agent }
    }
}

impl HttpClient for LoopbackHttp {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        use std::io::Read;
        if !req.url.starts_with("https://127.0.0.1:") {
            return Err(HttpError::BadUrl("loopback client: 127.0.0.1 only".into()));
        }
        let agent = self
            .agent
            .as_ref()
            .ok_or_else(|| HttpError::Tls("TLS setup failed".into()))?;
        let method = match req.method {
            codenotch_core::Method::Get => "GET",
            codenotch_core::Method::Post => "POST",
        };
        let mut r = agent.request(method, &req.url).timeout(req.timeout);
        for (k, v) in &req.headers {
            r = r.set(k, v);
        }
        let result = match &req.body {
            Some(b) => r.send_bytes(b),
            None => r.call(),
        };
        let resp = match result {
            Ok(resp) | Err(ureq::Error::Status(_, resp)) => resp,
            Err(ureq::Error::Transport(t)) => {
                return Err(HttpError::Connect(util::short(&t.kind().to_string())))
            }
        };
        let status = resp.status();
        let mut body = Vec::new();
        resp.into_reader()
            .take(codenotch_core::http::MAX_BODY_BYTES as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|e| HttpError::Other(e.kind().to_string()))?;
        if body.len() > codenotch_core::http::MAX_BODY_BYTES {
            return Err(HttpError::TooLarge);
        }
        Ok(HttpResponse {
            status,
            headers: Vec::new(),
            body,
        })
    }
}

fn read_bridge(env: &Env) -> Option<Vec<LimitWindow>> {
    let http = LoopbackHttp::new();
    let cached = ENDPOINT.lock().ok().and_then(|e| e.clone());
    if let Some(ep) = cached {
        if let Ok(w) = bridge_quota(&http, &ep) {
            return Some(w);
        }
        if let Ok(mut e) = ENDPOINT.lock() {
            *e = None;
        }
    }
    let ep = discover_endpoint(env)?;
    match bridge_quota(&http, &ep) {
        Ok(w) => {
            if let Ok(mut e) = ENDPOINT.lock() {
                *e = Some(ep);
            }
            Some(w)
        }
        Err(e) => {
            log::debug!("antigravity: local server did not answer ({e})");
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Count fallback
// ---------------------------------------------------------------------------------------------

/// Today's (local day) `source == "MODEL"` steps across every install.
pub(crate) fn requests_in(roots: &[PathBuf], today: chrono::NaiveDate) -> u64 {
    use chrono::TimeZone;
    let mut count = 0;
    for entry in roots
        .iter()
        .filter_map(|r| std::fs::read_dir(r.join("brain")).ok())
        .flat_map(|rd| rd.flatten())
    {
        let p = entry
            .path()
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl");
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        for line in text.lines().filter(|l| l.contains("\"MODEL\"")) {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if v.get("source").and_then(Value::as_str) != Some("MODEL") {
                continue;
            }
            let Some(t) = util::json_rfc3339(v.get("created_at")) else {
                continue;
            };
            if chrono::Local
                .timestamp_opt(t, 0)
                .single()
                .is_some_and(|l| l.date_naive() == today)
            {
                count += 1;
            }
        }
    }
    count
}

fn count_reading(account: &Account, ctx: &FetchContext) -> Reading {
    let today = {
        use chrono::TimeZone;
        chrono::Local
            .timestamp_opt(ctx.now, 0)
            .single()
            .map(|d| d.date_naive())
    };
    let n = today.map(|d| requests_in(&state_roots(&ctx.env), d));
    let mut w = LimitWindow::new("requests", LABEL_REQUESTS, WindowKind::Other).derived();
    w.count = n.map(|n| n as i64);
    let mut r = Reading::fresh(account, ctx.now, vec![w]);
    r.note = Some(NOTE_OPEN_APP.into());
    with_lanes(r, ctx)
}

// ---------------------------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------------------------

fn cli_reading(account: &Account, ctx: &FetchContext, windows: Vec<LimitWindow>) -> Reading {
    let mut r = Reading::fresh(account, ctx.now, windows);
    r.note = Some(VIA_CLI.into());
    with_lanes(r, ctx)
}

/// The previous reading, unchanged except for the lane choice (settings may have changed).
fn keep_previous(account: &Account, ctx: &FetchContext, why: &str) -> Reading {
    match ctx.previous.clone() {
        Some(mut p) => {
            p.account_id = account.id.clone();
            with_lanes(p, ctx)
        }
        None => Reading::error(account, ctx.now, why),
    }
}

pub(crate) fn fetch(account: &Account, ctx: &FetchContext) -> Reading {
    let env = &ctx.env;
    if let Some(agy) = official_agy(env) {
        if !cli_due(last_attempt(account), ctx) {
            return keep_previous(account, ctx, NOTE_CLI_SPACING);
        }
        if util::spawn_disabled(env) {
            return with_lanes(util::failed(account, ctx, NOTE_NO_SPAWN), ctx);
        }
        note_attempt(account, ctx.now);
        return match read_cli(env, &agy) {
            Ok(w) => cli_reading(account, ctx, w),
            Err(e) => {
                log::warn!("antigravity: {e}");
                let mut r = with_lanes(util::failed(account, ctx, &e), ctx);
                if r.status == ReadingStatus::Stale {
                    r.note = Some(NOTE_CLI_KEPT.into());
                }
                r
            }
        };
    }
    if !installed(env) {
        return Reading::not_installed(account, ctx.now);
    }
    // 1. the IDE's local server
    if let Some(w) = read_bridge(env) {
        let mut r = Reading::fresh(account, ctx.now, w);
        r.note = Some("via Antigravity".into());
        return with_lanes(r, ctx);
    }
    // 2. the bundled CLI, when someone asked and it is due
    if let Some(agy) = bundled_agy(env).filter(|_| !util::spawn_disabled(env)) {
        if cli_due(last_attempt(account), ctx) {
            note_attempt(account, ctx.now);
            match read_cli(env, &agy) {
                Ok(w) => return cli_reading(account, ctx, w),
                Err(e) => log::warn!("antigravity: bundled {e}"),
            }
        }
    }
    // 3. a percentage read before beats a count
    if let Some(prev) = ctx.previous.as_ref().filter(|p| {
        p.shows_figures()
            && p.windows
                .iter()
                .any(|w| w.count.is_none() && w.used_percent.is_some())
    }) {
        let mut p = prev.clone();
        p.account_id = account.id.clone();
        if !p.note.as_deref().unwrap_or("").starts_with(VIA_CLI) {
            p.status = ReadingStatus::Stale;
            p.note = Some(NOTE_CLOSED.into());
        }
        return with_lanes(p, ctx);
    }
    // 4. today's request count
    count_reading(account, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::ProviderKind;
    use codenotch_core::FakeHttp;
    use serde_json::json;
    use std::sync::Arc;

    const SAMPLE: &str = "Quota:\nGemini Models Weekly Limit Remaining 94% 2026-09-12T01:47:23Z\nGemini Models Five Hour Limit Remaining 78% 2026-09-10T06:12:31Z\nClaude and GPT models Weekly Limit Remaining 100% 2026-09-17T01:47:23Z\nClaude and GPT models Five Hour Limit Remaining 100% 2026-09-10T08:13:44Z";

    fn utc_only(t: &str) -> Result<i64, String> {
        parse_reset_with(t, |_| panic!("no local lookup expected"))
    }

    #[test]
    fn cli_report_is_grouped_and_ordered() {
        let ws = parse_quota_with(SAMPLE, &utc_only).unwrap();
        assert_eq!(ws.len(), 4);
        assert_eq!(ws[0].id, "Gemini Models Five Hour Limit");
        assert_eq!(ws[0].label_key, "5-hour Limit");
        assert_eq!(ws[0].group.as_deref(), Some("Gemini Models"));
        assert!((ws[0].used_percent.unwrap() - 22.0).abs() < 1e-9);
        assert_eq!(ws[0].window_seconds, Some(util::FIVE_HOURS));
        assert_eq!(ws[0].kind, WindowKind::Session);
        assert_eq!(ws[1].id, "Gemini Models Weekly Limit");
        assert_eq!(ws[1].window_seconds, Some(util::SEVEN_DAYS));
        assert_eq!(ws[3].group.as_deref(), Some("Claude and GPT models"));
        assert_eq!(ws[3].used_percent, Some(0.0));
        assert_eq!(ws[1].resets_at, util::rfc3339_secs("2026-09-12T01:47:23Z"));
    }

    #[test]
    fn cli_report_rejects_bad_input() {
        for bad in [
            "".to_string(),
            "authentication required".into(),
            "Quota:\nunknown".into(),
            SAMPLE.replace("94%", "101%"),
            SAMPLE.replace("94%", "-5%"),
            SAMPLE.replace("94%", "NaN%"),
            SAMPLE.replace("2026-09-12T01:47:23Z", "not-a-date"),
        ] {
            assert!(parse_quota_with(&bad, &utc_only).is_err(), "{bad}");
        }
        let ws = parse_quota_with(
            "Quota:\nGemini Models Monthly Limit Remaining 90% 2026-10-12T01:47:23Z",
            &utc_only,
        )
        .unwrap();
        assert_eq!(
            ws[0].window_seconds, None,
            "unknown cadences are not guessed"
        );
    }

    #[test]
    fn terminal_noise_is_stripped() {
        let raw = "\x1b[?25h\x1b]0;agy\x07\x1b[32mQuota:\x1b[0m\r\nGemini Models Weekly Limit Remaining 94% 2026-09-12T01:47:23Z\r\n";
        assert_eq!(
            sanitize(raw),
            "Quota:\nGemini Models Weekly Limit Remaining 94% 2026-09-12T01:47:23Z\n"
        );
        assert_eq!(
            parse_quota_with(raw, &utc_only).unwrap()[0].label_key,
            "Weekly Limit"
        );
    }

    #[test]
    fn reset_time_forms() {
        let at = |s: &str| util::rfc3339_secs(s).unwrap();
        for (text, want) in [
            ("2026-10-10 22:31 +07", "2026-10-10T15:31:00Z"),
            ("2026-10-10 22:31 -03", "2026-10-11T01:31:00Z"),
            ("2026-10-10 22:31 +0530", "2026-10-10T17:01:00Z"),
            ("2026-10-10 22:31 +14", "2026-10-10T08:31:00Z"),
            ("2026-10-10 11:31 UTC", "2026-10-10T11:31:00Z"),
            (" 2026-10-10 11:31 Z ", "2026-10-10T11:31:00Z"),
            ("2026-10-10T22:31:00+11:00", "2026-10-10T11:31:00Z"),
        ] {
            assert_eq!(utc_only(text).unwrap(), at(want), "{text}");
        }
        for bad in [
            "2026-10-10 9:31 AEDT",
            "2026-10-10 22:31",
            "2026-10-10 22:31 +7",
            "2026-10-10 22:31 +15",
            "2026-10-10 22:31 +1401",
            "2026-10-10 22:31 +07:00",
            "2026-10-10 22:31 UTC+7",
            "not-a-date",
        ] {
            assert!(utc_only(bad).is_err(), "{bad}");
        }
        use chrono::{FixedOffset, LocalResult, TimeZone};
        let aedt = parse_reset_with("2026-10-10 22:31 AEDT", |n| {
            FixedOffset::east_opt(11 * 3600)
                .unwrap()
                .from_local_datetime(n)
        })
        .unwrap();
        assert_eq!(aedt, at("2026-10-10T11:31:00Z"));
        assert!(parse_reset_with("2026-10-04 02:30 AEDT", |_| LocalResult::None).is_err());
    }

    #[test]
    fn lane_seconds_aliases() {
        for l in [
            "session",
            "gemini_5h",
            "3p-hourly",
            "5-hour",
            "Five Hour Limit",
            "Gemini Models Five Hour Limit",
        ] {
            assert_eq!(lane_seconds(l), Some(util::FIVE_HOURS), "{l}");
        }
        for l in [
            "weekly",
            "3p-weekly",
            "Gemini Models Weekly Limit Remaining",
        ] {
            assert_eq!(lane_seconds(l), Some(util::SEVEN_DAYS), "{l}");
        }
        for l in ["requests", "monthly", "biweekly", "5h-preview", "gemini"] {
            assert_eq!(lane_seconds(l), None, "{l}");
        }
    }

    #[test]
    fn bridge_reply_is_flipped_and_ordered() {
        let ws = windows_from_bridge(&json!({"response":{"groups":[
            {"displayName":"Gemini Models","buckets":[
                {"bucketId":"gemini-weekly","remainingFraction":0.97,"resetTime":"2026-09-12T00:00:00Z"},
                {"bucketId":"gemini-5h","remainingFraction":1.0}]},
            {"displayName":"Claude and GPT models","buckets":[
                {"bucketId":"3p-weekly","remainingFraction":1.0},
                {"bucketId":"3p-5h","remainingFraction":0.5},
                {"bucketId":"bad","remainingFraction":1.5}]}]}}));
        let ids: Vec<&str> = ws.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["gemini-5h", "gemini-weekly", "3p-5h", "3p-weekly"]);
        assert!((ws[1].used_percent.unwrap() - 3.0).abs() < 1e-9);
        assert_eq!(ws[1].label_key, "Weekly Limit");
        assert_eq!(ws[2].used_percent, Some(50.0));
        assert_eq!(ws[0].window_seconds, Some(util::FIVE_HOURS));
        assert!(windows_from_bridge(&json!({})).is_empty());
    }

    #[test]
    fn bridge_tries_each_port() {
        let ep = Endpoint {
            ports: vec![1111, 2222],
            csrf: "csrf-x".into(),
        };
        let reply = r#"{"response":{"groups":[{"displayName":"Gemini Models","buckets":[{"bucketId":"gemini-5h","remainingFraction":0.8}]}]}}"#;
        let http = FakeHttp::new().on("https://127.0.0.1:1111/", 404, "").on(
            "https://127.0.0.1:2222/",
            200,
            reply,
        );
        let ws = bridge_quota(&http, &ep).unwrap();
        assert!((ws[0].used_percent.unwrap() - 20.0).abs() < 1e-9);
        let reqs = http.requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[1].header_value("x-codeium-csrf-token"), Some("csrf-x"));
        assert_eq!(
            reqs[1].body.as_deref(),
            Some(br#"{"forceRefresh":true}"#.as_slice())
        );
        assert!(!format!("{ep:?}").contains("csrf-x"));
        assert!(bridge_quota(&FakeHttp::new(), &ep).is_err());
    }

    #[test]
    fn loopback_client_refuses_other_hosts() {
        let http = LoopbackHttp::new();
        assert!(matches!(
            http.send(&HttpRequest::get("https://example.com/x")),
            Err(HttpError::BadUrl(_))
        ));
        assert!(matches!(
            http.send(&HttpRequest::get("https://127.0.0.1.evil.com/x")),
            Err(HttpError::BadUrl(_))
        ));
    }

    #[test]
    fn process_and_port_tables() {
        assert_eq!(
            flag_value("x --csrf_token abc --y", "--csrf_token").as_deref(),
            Some("abc")
        );
        assert_eq!(
            flag_value("x --csrf_token=abc", "--csrf_token").as_deref(),
            Some("abc")
        );
        assert_eq!(flag_value("x --csrf_token_file=abc", "--csrf_token"), None);
        assert_eq!(flag_value("x --csrf_token", "--csrf_token"), None);
        let hub = "9052\tC:\\u\\.gemini\\bin\\agy.exe --hub --csrf_token=hub-token";
        let ide = "4410\tC:\\A\\language_server_windows_x64.exe --csrf_token ide-token --https_server_port 0";
        let cli = "7000\tC:\\agy.exe --print /usage";
        assert_eq!(
            server_in(&format!("{cli}\n{hub}")),
            Some((9052, "hub-token".into()))
        );
        assert_eq!(
            server_in(&format!("{hub}\n{ide}")),
            Some((4410, "ide-token".into()))
        );
        assert_eq!(server_in(cli), None);
        let netstat = "  TCP    127.0.0.1:61001        0.0.0.0:0              LISTENING       4410\n  TCP    127.0.0.1:61002        0.0.0.0:0              LISTENING       4410\n  TCP    127.0.0.1:61002        0.0.0.0:0              LISTENING       4410\n  TCP    127.0.0.1:5000         0.0.0.0:0              LISTENING       9\n  TCP    127.0.0.1:61003        127.0.0.1:1            ESTABLISHED     4410\n";
        assert_eq!(listening_ports(netstat, 4410), vec![61001, 61002]);
    }

    fn lanes() -> Vec<LimitWindow> {
        vec![
            LimitWindow::new("gemini-5h", "5-hour Limit", WindowKind::Session).used(30.0),
            LimitWindow::new("gemini-weekly", "Weekly Limit", WindowKind::Weekly).used(60.0),
            LimitWindow::new("3p-5h", "5-hour Limit", WindowKind::Session).used(100.0),
            LimitWindow::new("3p-weekly", "Weekly Limit", WindowKind::Weekly).used(10.0),
        ]
    }

    #[test]
    fn lane_picker() {
        use AntigravityLimit::*;
        use AntigravityModel::*;
        let ws = lanes();
        assert_eq!(
            headline(&ws, Automatic, Gemini),
            (Some("gemini-weekly".into()), None)
        );
        assert_eq!(
            headline(&ws, FiveHour, Gemini),
            (Some("gemini-5h".into()), Some("gemini-weekly".into()))
        );
        assert_eq!(
            headline(&ws, Weekly, Gemini),
            (Some("gemini-weekly".into()), None)
        );
        // Automatic skips a spent lane while another has room.
        assert_eq!(
            headline(&ws, Automatic, ThirdParty),
            (Some("3p-weekly".into()), None)
        );
        assert_eq!(
            headline(&ws, FiveHour, ThirdParty),
            (Some("3p-5h".into()), Some("3p-weekly".into()))
        );
        // Every lane spent: the tightest overall; ties go to the lower id.
        let spent = vec![
            LimitWindow::new("b", "Weekly Limit", WindowKind::Weekly).used(100.0),
            LimitWindow::new("a", "5-hour Limit", WindowKind::Session).used(100.0),
        ];
        assert_eq!(headline(&spent, Automatic, Gemini).0.as_deref(), Some("a"));
        // CLI ids: families by prefix.
        let cli = parse_quota_with(SAMPLE, &utc_only).unwrap();
        assert_eq!(
            headline(&cli, Automatic, Gemini).0.as_deref(),
            Some("Gemini Models Five Hour Limit")
        );
        assert_eq!(
            headline(&cli, Weekly, ThirdParty).0.as_deref(),
            Some("Claude and GPT models Weekly Limit")
        );
        // A count-only window is still the headline (the ring draws its track).
        let mut count = LimitWindow::new("requests", "Requests today", WindowKind::Other).derived();
        count.count = Some(3);
        assert_eq!(
            headline(&[count], Automatic, Gemini),
            (Some("requests".into()), None)
        );
        assert_eq!(headline(&[], Automatic, Gemini), (None, None));
    }

    #[test]
    fn cli_scheduling_rule() {
        let a = Account::new(ProviderKind::Antigravity, "a");
        let mut ctx = FetchContext::new(
            10_000,
            Arc::new(FakeHttp::new()),
            Env::for_test(Path::new("Z:/x")),
        );
        assert!(cli_due(None, &ctx), "first fetch of the process");
        assert!(
            !cli_due(Some(9_000), &ctx),
            "timer fetches never run the CLI again"
        );
        ctx.manual = true;
        assert!(cli_due(Some(9_000), &ctx), "a click after the TTL");
        assert!(!cli_due(Some(9_800), &ctx), "a click inside the TTL");
        ctx.previous = Some(Reading::fresh(&a, 9_900, lanes()));
        assert!(
            !cli_due(Some(9_000), &ctx),
            "the shown reading is younger than the TTL"
        );
        ctx.previous = Some(Reading::fresh(&a, 9_000, lanes()));
        assert!(cli_due(Some(9_000), &ctx));
    }

    fn trajectory(root: &Path, id: &str, lines: &[String]) {
        let logs = root
            .join("brain")
            .join(id)
            .join(".system_generated")
            .join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("transcript.jsonl"), lines.join("\n")).unwrap();
    }

    fn step(source: &str, at: chrono::DateTime<chrono::Utc>) -> String {
        format!(
            r#"{{"source":"{source}","created_at":"{}"}}"#,
            at.to_rfc3339()
        )
    }

    #[test]
    fn legacy_count_fallback_and_carry() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path()).with_var("CODENOTCH_NO_SPAWN", "1");
        let a = Account::new(ProviderKind::Antigravity, "Antigravity");
        let ctx = FetchContext::new(
            chrono::Utc::now().timestamp(),
            Arc::new(FakeHttp::new()),
            env.clone(),
        );
        assert!(!installed(&env));
        assert_eq!(fetch(&a, &ctx).status, ReadingStatus::NotInstalled);
        let gem = env.home.join(".gemini");
        let now = chrono::Utc::now();
        let old = now - chrono::TimeDelta::days(3);
        trajectory(
            &gem.join("antigravity-ide"),
            "a",
            &[step("MODEL", now), step("USER", now), step("MODEL", old)],
        );
        trajectory(
            &gem.join("antigravity-cli"),
            "b",
            &[step("MODEL", now), "not json".into()],
        );
        std::fs::create_dir_all(gem.join("config")).unwrap();
        assert_eq!(state_roots(&env).len(), 2);
        let r = fetch(&a, &ctx);
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.windows[0].count, Some(2));
        assert_eq!(
            r.windows[0].used_percent, None,
            "a count is never a percentage"
        );
        assert_eq!(r.headline_window.as_deref(), Some("requests"));
        // A percentage read before is kept, stale, instead of the count.
        let mut ctx2 = ctx.clone();
        let mut prev = Reading::fresh(&a, 5, lanes());
        prev.note = Some("via Antigravity".into());
        ctx2.previous = Some(prev);
        let r = fetch(&a, &ctx2);
        assert_eq!(r.status, ReadingStatus::Stale);
        assert_eq!(r.windows.len(), 4);
        assert_eq!(r.headline_window.as_deref(), Some("gemini-weekly"));
    }
}
