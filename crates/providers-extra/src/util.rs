//! Small helpers shared by every provider of this crate: JSON/time parsing, read-only SQLite,
//! carry-over readings, 429 back-off, PATH lookup and query encoding.

use codenotch_core::model::{Reading, ReadingStatus};
use codenotch_core::provider::{Env, FetchContext};
use codenotch_core::{Account, HttpResponse};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) const FIVE_HOURS: i64 = 5 * 3600;
pub(crate) const SEVEN_DAYS: i64 = 7 * 86_400;
/// Request timeout used by every provider of this crate (data-spec §5: 15 s).
pub(crate) const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// `User-Agent` for the endpoints that want ours (OpenCode, GLM).
pub(crate) const USER_AGENT: &str = concat!("codenotch/", env!("CARGO_PKG_VERSION"), " (Windows)");
/// Env var that disables every subprocess launched by this crate when set to `1`.
pub(crate) const NO_SPAWN_ENV: &str = "CODENOTCH_NO_SPAWN";
/// Our data folder name under `%APPDATA%` (read only from this crate).
pub(crate) const DATA_DIR_ENV: &str = codenotch_core::paths::DATA_DIR_ENV;

// ---------------------------------------------------------------------------------------------
// JSON / text
// ---------------------------------------------------------------------------------------------

/// Trimmed, non-empty string value.
pub(crate) fn json_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Finite number value.
pub(crate) fn json_f64(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|n| n.is_finite())
}

/// Read and parse a JSON file; any failure is `None`.
pub(crate) fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// RFC 3339 → unix seconds.
pub(crate) fn rfc3339_secs(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|d| d.timestamp())
}

/// RFC 3339 string value → unix seconds.
pub(crate) fn json_rfc3339(v: Option<&Value>) -> Option<i64> {
    v.and_then(Value::as_str).and_then(rfc3339_secs)
}

/// Epoch number in seconds or milliseconds (values above `ms_above` are milliseconds) → seconds.
/// Zero / negative / non-finite = unknown.
pub(crate) fn epoch_secs(n: f64, ms_above: f64) -> Option<i64> {
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    let secs = if n > ms_above { n / 1000.0 } else { n };
    (secs < 1e13).then_some(secs as i64)
}

/// First letter upper-cased.
pub(crate) fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Length of the calendar month that ends at `reset` (UTC): `reset − (reset − 1 month)`.
pub(crate) fn calendar_month_secs(reset: i64) -> Option<i64> {
    let end = chrono::DateTime::<chrono::Utc>::from_timestamp(reset, 0)?;
    let start = end.checked_sub_months(chrono::Months::new(1))?;
    let len = (end - start).num_seconds();
    (len > 0).then_some(len)
}

/// `end − start` when both are known and positive.
pub(crate) fn span(start: Option<i64>, end: Option<i64>) -> Option<i64> {
    let len = end? - start?;
    (len > 0).then_some(len)
}

/// Short, secret-scrubbed message for `Reading::error` / notes.
pub(crate) fn short(msg: &str) -> String {
    let mut t = codenotch_core::logging::redact(msg.trim());
    if t.chars().count() > 160 {
        t = t.chars().take(160).collect::<String>() + "…";
    }
    t
}

/// Percent-encode a query value (RFC 3986 unreserved characters pass through).
pub(crate) fn query_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `url?k=v&k2=v2` with encoded values.
pub(crate) fn with_query(url: &str, query: &[(&str, &str)]) -> String {
    if query.is_empty() {
        return url.to_string();
    }
    let q: Vec<String> = query
        .iter()
        .map(|(k, v)| format!("{}={}", query_encode(k), query_encode(v)))
        .collect();
    format!("{url}?{}", q.join("&"))
}

// ---------------------------------------------------------------------------------------------
// Environment / files
// ---------------------------------------------------------------------------------------------

/// `CODENOTCH_NO_SPAWN=1` disables every subprocess.
pub(crate) fn spawn_disabled(env: &Env) -> bool {
    env.var(NO_SPAWN_ENV).is_some_and(|v| v.trim() == "1")
}

/// Absolute `PATH` entries (relative ones are ignored: they depend on the working directory).
pub(crate) fn path_dirs(env: &Env) -> Vec<PathBuf> {
    env.var("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .filter(|d| d.is_absolute())
                .collect()
        })
        .unwrap_or_default()
}

/// First `<dir>\<exe>` on `PATH` that is a file.
pub(crate) fn find_on_path(env: &Env, exe: &str) -> Option<PathBuf> {
    path_dirs(env)
        .into_iter()
        .map(|d| d.join(exe))
        .find(|p| p.is_file())
}

/// Our own data folder (`CODENOTCH_DATA_DIR`, else `%APPDATA%\dev.kev.smashtoken`). Read only.
pub(crate) fn data_dir(env: &Env) -> PathBuf {
    match env.var(DATA_DIR_ENV) {
        Some(d) => PathBuf::from(d),
        None => env.appdata.join(codenotch_core::paths::APP_DIR_NAME),
    }
}

/// Token of an account: the env var it names in `secretEnv`, then the token saved for it in the
/// secret store (`SecretKey::Account(id)`, see `core::secrets::resolve_secret_with`).
pub(crate) fn account_secret(
    account: &Account,
    env: &Env,
    store: Option<&dyn codenotch_core::secrets::SecretStore>,
) -> Option<String> {
    use codenotch_core::secrets::{resolve_secret_with, NoSecrets, SecretKey};
    let config = codenotch_core::Config {
        accounts: vec![account.clone()],
        ..Default::default()
    };
    resolve_secret_with(
        &SecretKey::Account(account.id.clone()),
        Some(&config),
        env,
        store.unwrap_or(&NoSecrets),
    )
    .map(|v| v.trim().to_string())
    .filter(|v| !v.is_empty())
}

// ---------------------------------------------------------------------------------------------
// SQLite (read only)
// ---------------------------------------------------------------------------------------------

/// Open another application's SQLite database **read only**: `mode=ro` first (sees a value the
/// owner just wrote to its WAL), then `immutable=1` (works once the owner has exited and its
/// `-shm` is gone). `probe` is a query that must succeed for the connection to count.
pub(crate) fn open_sqlite_ro(path: &Path, probe: &str) -> Option<rusqlite::Connection> {
    use rusqlite::OpenFlags;
    if !path.is_file() {
        return None;
    }
    let works = |c: &rusqlite::Connection| {
        c.prepare(probe)
            .and_then(|mut s| s.query([]).map(|_| ()))
            .is_ok()
    };
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if let Ok(c) = rusqlite::Connection::open_with_flags(path, flags) {
        if works(&c) {
            return Some(c);
        }
    }
    let c = rusqlite::Connection::open_with_flags(
        sqlite_immutable_uri(path),
        flags | OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()?;
    works(&c).then_some(c)
}

/// `file:///C:/x/y.db?immutable=1` with the URI-special characters escaped.
pub(crate) fn sqlite_immutable_uri(path: &Path) -> String {
    let p = path
        .to_string_lossy()
        .replace('%', "%25")
        .replace('\\', "/")
        .replace('#', "%23")
        .replace('?', "%3F");
    format!("file:///{}?immutable=1", p.trim_start_matches('/'))
}

// ---------------------------------------------------------------------------------------------
// Readings
// ---------------------------------------------------------------------------------------------

/// Previous reading worth carrying over (it had figures).
fn carried(ctx: &FetchContext) -> Option<&Reading> {
    ctx.previous
        .as_ref()
        .filter(|p| !p.windows.is_empty() && p.shows_figures())
}

/// A failed attempt: keep the previous numbers marked `Stale` (their original `fetched_at`), or an
/// `Error` reading when there is nothing to keep. "Stale beats invented."
pub(crate) fn failed(account: &Account, ctx: &FetchContext, message: &str) -> Reading {
    let message = short(message);
    match carried(ctx) {
        Some(prev) => {
            let mut r = prev.clone();
            r.account_id = account.id.clone();
            r.status = ReadingStatus::Stale;
            r.error = Some(message);
            r
        }
        None => Reading::error(account, ctx.now, message),
    }
}

/// 429 marker kept in `Reading::error` so the next attempt knows how many came in a row.
/// Parsed back by [`previous_429s`]: never translated.
const RATE_MARK: &str = "rate limited (429 x";

// Templates shared by several providers, translated at fetch time (`crate::i18n::tf`).
pub(crate) const TPL_RATE_LIMITED: &str = "Rate limited — retrying in {wait}s";
pub(crate) const TPL_LIVE_FAILED: &str = "Live read failed ({e})";
pub(crate) const TPL_LIVE_HTTP: &str = "Live read failed (HTTP {code})";

/// "Rate limited — retrying in {wait}s" in the user's language.
pub(crate) fn retrying_note(ctx: &FetchContext, wait: i64) -> String {
    crate::i18n::tf(
        ctx.settings.lang(),
        TPL_RATE_LIMITED,
        &[("wait", &wait.to_string())],
    )
}

/// "Live read failed ({e})" in the user's language (`e` stays as is).
pub(crate) fn live_failed(ctx: &FetchContext, e: &str) -> String {
    crate::i18n::tf(ctx.settings.lang(), TPL_LIVE_FAILED, &[("e", e)])
}

/// "Live read failed (HTTP {code})" in the user's language.
pub(crate) fn live_http(ctx: &FetchContext, code: u16) -> String {
    crate::i18n::tf(
        ctx.settings.lang(),
        TPL_LIVE_HTTP,
        &[("code", &code.to_string())],
    )
}

/// Consecutive 429s before this one, read back from the previous reading.
fn previous_429s(ctx: &FetchContext) -> u32 {
    let Some(prev) = ctx.previous.as_ref() else {
        return 0;
    };
    if !matches!(prev.status, ReadingStatus::RateLimited { .. }) {
        return 0;
    }
    prev.error
        .as_deref()
        .and_then(|e| e.strip_prefix(RATE_MARK))
        .and_then(|rest| rest.split(')').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(1)
}

/// Back-off rule used by OpenCode and GLM: 60·2^(n−1) s capped at 900 s, at least Retry-After.
/// A manual refresh restarts the count (`reset_on_manual`). Returns (wait secs, n).
pub(crate) fn exponential_backoff(
    ctx: &FetchContext,
    retry_after: Option<i64>,
    reset_on_manual: bool,
) -> (i64, u32) {
    let before = if reset_on_manual && ctx.manual {
        0
    } else {
        previous_429s(ctx)
    };
    let n = before.saturating_add(1);
    let exp = 60i64.saturating_mul(1 << (n - 1).min(4));
    let wait = exp.clamp(60, 900).max(retry_after.unwrap_or(0));
    (wait, n)
}

/// A rate-limited reading that keeps the previous numbers (and their age) when there are some.
pub(crate) fn rate_limited(
    account: &Account,
    ctx: &FetchContext,
    wait_secs: i64,
    n: u32,
    note: &str,
) -> Reading {
    let until = ctx.now + wait_secs.max(1);
    let mut r = Reading::rate_limited(account, ctx.now, Some(until));
    if let Some(prev) = carried(ctx) {
        r.windows = prev.windows.clone();
        r.fetched_at = prev.fetched_at;
        r.identity = prev.identity.clone();
        r.plan = prev.plan.clone();
        r.headline_window = prev.headline_window.clone();
        r.weekly_window = prev.weekly_window.clone();
    }
    r.note = Some(note.to_string());
    r.error = Some(format!("{RATE_MARK}{n})"));
    r
}

/// `Retry-After` of a response, in seconds.
pub(crate) fn retry_after(resp: &HttpResponse, now: i64) -> Option<i64> {
    resp.retry_after_secs(now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::{LimitWindow, ProviderKind, WindowKind};
    use codenotch_core::FakeHttp;
    use std::sync::Arc;

    fn ctx(previous: Option<Reading>, manual: bool) -> FetchContext {
        let mut c = FetchContext::new(
            1_000,
            Arc::new(FakeHttp::new()),
            Env::for_test(Path::new("Z:/none")),
        );
        c.previous = previous;
        c.manual = manual;
        c
    }

    #[test]
    fn account_secret_env_then_store() {
        use codenotch_core::secrets::{MemorySecrets, SecretKey, SecretStore};
        let mut a = Account::new(ProviderKind::Glm, "z");
        let env = Env::for_test(Path::new("Z:/none"));
        let store = MemorySecrets::new();
        assert_eq!(account_secret(&a, &env, Some(&store)), None);
        store
            .set(&SecretKey::Account(a.id.clone()), " saved-key ")
            .unwrap();
        assert_eq!(account_secret(&a, &env, None), None);
        assert_eq!(
            account_secret(&a, &env, Some(&store)).as_deref(),
            Some("saved-key")
        );
        a.secret_env = "MY_GLM".into();
        assert_eq!(
            account_secret(&a, &env, Some(&store)).as_deref(),
            Some("saved-key")
        );
        let env = env.with_var("MY_GLM", "env-key");
        assert_eq!(
            account_secret(&a, &env, Some(&store)).as_deref(),
            Some("env-key")
        );
    }

    #[test]
    fn calendar_month_lengths() {
        let t = |s: &str| rfc3339_secs(s).unwrap();
        assert_eq!(
            calendar_month_secs(t("2026-10-01T00:00:00Z")),
            Some(30 * 86_400)
        );
        assert_eq!(
            calendar_month_secs(t("2024-03-01T00:00:00Z")),
            Some(29 * 86_400)
        );
        assert_eq!(
            calendar_month_secs(t("2026-01-03T13:09:45Z")),
            Some(31 * 86_400)
        );
    }

    #[test]
    fn epoch_units() {
        assert_eq!(epoch_secs(1_790_812_800.0, 1e10), Some(1_790_812_800));
        assert_eq!(epoch_secs(1_790_812_800_000.0, 1e10), Some(1_790_812_800));
        assert_eq!(epoch_secs(0.0, 1e10), None);
        assert_eq!(epoch_secs(-5.0, 1e10), None);
        assert_eq!(epoch_secs(f64::NAN, 1e10), None);
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(
            with_query(
                "https://h/p",
                &[("orgId", "o 1"), ("since", "2026-09-13T15:21:27.000Z")]
            ),
            "https://h/p?orgId=o%201&since=2026-09-13T15%3A21%3A27.000Z"
        );
        assert_eq!(with_query("https://h/p", &[]), "https://h/p");
    }

    #[test]
    fn immutable_uri_escapes() {
        let uri = sqlite_immutable_uri(Path::new(r"C:\a b\x#1?.db"));
        assert_eq!(uri, "file:///C:/a b/x%231%3F.db?immutable=1");
    }

    #[test]
    fn sqlite_is_opened_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let c = rusqlite::Connection::open(&path).unwrap();
            c.execute_batch("CREATE TABLE ItemTable(key TEXT, value TEXT); INSERT INTO ItemTable VALUES('k','v');").unwrap();
        }
        let c = open_sqlite_ro(&path, "SELECT 1 FROM ItemTable LIMIT 1").unwrap();
        assert!(
            c.execute("INSERT INTO ItemTable VALUES('a','b')", [])
                .is_err(),
            "must be read only"
        );
        assert!(open_sqlite_ro(&dir.path().join("missing.db"), "SELECT 1").is_none());
        assert!(open_sqlite_ro(&path, "SELECT 1 FROM nope").is_none());
    }

    #[test]
    fn failure_keeps_previous_numbers_stale() {
        let a = Account::new(ProviderKind::Cursor, "c");
        assert_eq!(
            failed(&a, &ctx(None, false), "boom").status,
            ReadingStatus::Error
        );
        let prev = Reading::fresh(
            &a,
            500,
            vec![LimitWindow::new("included", "Included usage", WindowKind::Monthly).used(10.0)],
        );
        let r = failed(&a, &ctx(Some(prev), false), "boom");
        assert_eq!(r.status, ReadingStatus::Stale);
        assert_eq!(r.fetched_at, 500);
        assert_eq!(r.windows.len(), 1);
        // A signed-out previous reading is never revived.
        let r = failed(&a, &ctx(Some(Reading::signed_out(&a, 1)), false), "boom");
        assert_eq!(r.status, ReadingStatus::Error);
    }

    #[test]
    fn backoff_doubles_caps_and_manual_resets() {
        let a = Account::new(ProviderKind::Glm, "g");
        let c = ctx(None, false);
        assert_eq!(exponential_backoff(&c, None, true), (60, 1));
        assert_eq!(exponential_backoff(&c, Some(300), true), (300, 1));
        let mut prev = rate_limited(&a, &c, 60, 1, "x");
        for (expected, n) in [(120, 2), (240, 3), (480, 4), (900, 5), (900, 6)] {
            let c2 = ctx(Some(prev.clone()), false);
            let (wait, got_n) = exponential_backoff(&c2, None, true);
            assert_eq!((wait, got_n), (expected, n));
            prev = rate_limited(&a, &c2, wait, got_n, "x");
            assert_eq!(
                prev.status,
                ReadingStatus::RateLimited {
                    until: Some(1_000 + wait)
                }
            );
        }
        assert_eq!(
            exponential_backoff(&ctx(Some(prev.clone()), true), None, true),
            (60, 1)
        );
        assert_eq!(
            exponential_backoff(&ctx(Some(prev), true), None, false).1,
            7
        );
    }
}
