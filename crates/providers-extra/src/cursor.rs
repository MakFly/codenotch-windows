//! Cursor (data-spec §5.1).
//!
//! The editor's own sign-in, borrowed read-only from VS Code's global state database
//! `%APPDATA%\Cursor\User\globalStorage\state.vscdb` (`ItemTable(key, value)`):
//! `cursorAuth/accessToken` + `cursorAuth/stripeMembershipAuthId` form the session cookie
//! `WorkosCursorSessionToken=<authId>::<token>`; `cursorAuth/stripeMembershipType` is the plan and
//! `cursorAuth/cachedEmail` the identity shown on the card. Re-read on every fetch (the editor
//! rotates the token). Since `cursor.storage.authTokenEncryptionAtRest`, the token is stored as
//! `cursor-safe-storage:v1:<base64>`: an Electron safeStorage value, decrypted in memory with the
//! key in Cursor's `Local State` (DPAPI, current user; see `codenotch_providers::safe_storage`).
//! `GET https://cursor.com/api/usage-summary` answers:
//! `{billingCycleStart, billingCycleEnd, membershipType, isUnlimited,
//!   individualUsage:{plan:{totalPercentUsed, apiPercentUsed}, onDemand:{enabled, used, limit}}}`.

use crate::i18n::tf;
use crate::util;
use codenotch_core::i18n::Lang;
use codenotch_core::model::{LimitWindow, Reading, ReadingStatus, WindowKind};
use codenotch_core::provider::{Env, FetchContext, LocalIdentity};
use codenotch_core::{Account, HttpRequest};
use codenotch_providers::safe_storage::SafeStorage;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) const ENDPOINT: &str = "https://cursor.com/api/usage-summary";
pub(crate) const NOTE_SIGN_IN: &str = "Sign in to Cursor (the editor) to see usage.";
pub(crate) const NOTE_REJECTED: &str = "Cursor session was rejected — sign in again in the editor";
pub(crate) const NOTE_RATE_LIMITED: &str = "Cursor is rate limiting; the last reading stands";
pub(crate) const NOTE_BAD_JSON: &str = "Cursor answered with unreadable JSON";
pub(crate) const NOTE_UNDECRYPTABLE: &str =
    "Cursor's saved session could not be decrypted — reopen Cursor, or sign in again";
/// Marker of a token Cursor encrypted at rest; the rest is a base64 Electron safeStorage value.
const ENCRYPTED_MARKER: &str = "cursor-safe-storage:";
const ENCRYPTED_V1: &str = "cursor-safe-storage:v1:";
pub(crate) const LABEL_ON_DEMAND: &str = "On demand";
// Templates translated at fetch time.
pub(crate) const TPL_UNLIMITED: &str = "Unlimited on the {plan} plan — nothing to meter";
pub(crate) const TPL_NOTHING: &str = "The {plan} plan has nothing for Cursor to meter yet";

/// The state database: the account's `profile_path` (the file itself, a folder holding it, or a
/// Cursor user-data folder), else the default location.
pub(crate) fn db_path(env: &Env, account: Option<&Account>) -> PathBuf {
    if let Some(p) = account
        .map(|a| a.profile_path.trim())
        .filter(|p| !p.is_empty())
    {
        let p = PathBuf::from(p);
        if p.is_file() {
            return p;
        }
        let direct = p.join("state.vscdb");
        if direct.is_file() {
            return direct;
        }
        return p.join("User").join("globalStorage").join("state.vscdb");
    }
    env.appdata
        .join("Cursor")
        .join("User")
        .join("globalStorage")
        .join("state.vscdb")
}

struct Creds {
    /// Cookie forms to try, in order (see [`cookie_forms`]).
    cookies: Vec<String>,
    plan: Option<String>,
    email: Option<String>,
}

/// The session cookie in the form the reference uses (`<authId>::<token>`), then the form the
/// web dashboard sets: the bare user id (the auth id after its `provider|` prefix, e.g.
/// `google-oauth2|user_…`) and the separator URL-encoded.
fn cookie_forms(auth_id: &str, token: &str) -> Vec<String> {
    let mut v = vec![format!("WorkosCursorSessionToken={auth_id}::{token}")];
    let user = auth_id.rsplit('|').next().unwrap_or(auth_id);
    let alt = format!("WorkosCursorSessionToken={user}%3A%3A{token}");
    if !v.contains(&alt) {
        v.push(alt);
    }
    v
}

fn item(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    use rusqlite::types::ValueRef;
    // VS Code stores values as TEXT, some builds as BLOB: accept both.
    conn.query_row("SELECT value FROM ItemTable WHERE key = ?1", [key], |r| {
        Ok(match r.get_ref(0)? {
            ValueRef::Text(t) | ValueRef::Blob(t) => String::from_utf8_lossy(t).into_owned(),
            _ => String::new(),
        })
    })
    .ok()
    .map(|s| s.trim().trim_matches('"').to_string())
    .filter(|s| !s.is_empty())
}

/// Cursor's `Local State` (holds the safeStorage key): the user-data folder three levels above
/// `User\globalStorage\state.vscdb`, else `%APPDATA%\Cursor`.
fn local_state_path(env: &Env, db: &Path) -> PathBuf {
    db.ancestors()
        .nth(3)
        .map(|d| d.join("Local State"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| env.appdata.join("Cursor").join("Local State"))
}

/// The usable token: a plain stored value as is, a `cursor-safe-storage:v1:` value decrypted.
/// `None` when it is encrypted and cannot be decrypted (unknown version, wrong key).
pub(crate) fn plain_token(
    stored: &str,
    key: impl FnOnce() -> Option<SafeStorage>,
) -> Option<String> {
    if let Some(b64) = stored.strip_prefix(ENCRYPTED_V1) {
        return key()?
            .decrypt_base64(b64.trim())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
    }
    if stored.starts_with(ENCRYPTED_MARKER) {
        return None;
    }
    Some(stored.to_string())
}

enum CredsError {
    /// No database or no session in it.
    SignedOut,
    /// A session is stored but could not be decrypted.
    Undecryptable,
}

fn read_creds(env: &Env, path: &Path) -> Result<Creds, CredsError> {
    let conn = util::open_sqlite_ro(path, "SELECT 1 FROM ItemTable LIMIT 1")
        .ok_or(CredsError::SignedOut)?;
    let stored = item(&conn, "cursorAuth/accessToken").ok_or(CredsError::SignedOut)?;
    let auth_id = item(&conn, "cursorAuth/stripeMembershipAuthId").ok_or(CredsError::SignedOut)?;
    let token = plain_token(&stored, || {
        SafeStorage::from_local_state(&local_state_path(env, path))
    })
    .ok_or_else(|| {
        log::warn!("cursor: the stored session is encrypted and could not be decrypted");
        CredsError::Undecryptable
    })?;
    Ok(Creds {
        cookies: cookie_forms(&auth_id, &token),
        plan: item(&conn, "cursorAuth/stripeMembershipType"),
        email: item(&conn, "cursorAuth/cachedEmail"),
    })
}

/// `pro` → "Pro", `free_trial` → "Free trial".
fn plan_label(p: &str) -> String {
    util::capitalize(&p.replace('_', " "))
}

/// The editor's cached e-mail and plan (`cursorAuth/cachedEmail`,
/// `cursorAuth/stripeMembershipType`), signed in or not. The token is never read.
pub(crate) fn local_identity(env: &Env, account: &Account) -> LocalIdentity {
    let Some(conn) = util::open_sqlite_ro(
        &db_path(env, Some(account)),
        "SELECT 1 FROM ItemTable LIMIT 1",
    ) else {
        return LocalIdentity::default();
    };
    LocalIdentity {
        identity: item(&conn, "cursorAuth/cachedEmail"),
        plan: item(&conn, "cursorAuth/stripeMembershipType").map(|p| plan_label(&p)),
    }
}

/// usage-summary → windows, or the reason there is nothing to meter (in `lang`).
pub(crate) fn parse_summary(v: &Value, lang: Lang) -> Result<Vec<LimitWindow>, String> {
    let resets_at = util::json_rfc3339(v.get("billingCycleEnd"));
    let len = util::span(util::json_rfc3339(v.get("billingCycleStart")), resets_at);
    let usage = v.get("individualUsage");
    let plan = usage.and_then(|u| u.get("plan"));
    let decorate = |mut w: LimitWindow| {
        w.resets_at = resets_at;
        w.window_seconds = len;
        w
    };
    let mut out = Vec::new();
    // The dashboard's "Included usage · N% used". 0 is a reading, not a gap.
    if let Some(total) = util::json_f64(plan.and_then(|p| p.get("totalPercentUsed"))) {
        out.push(decorate(
            LimitWindow::new("included", "Included usage", WindowKind::Monthly).used(total),
        ));
    }
    if let Some(api) =
        util::json_f64(plan.and_then(|p| p.get("apiPercentUsed"))).filter(|p| *p > 0.0)
    {
        out.push(decorate(
            LimitWindow::new("api", "API usage", WindowKind::Monthly).used(api),
        ));
    }
    if let Some(od) = usage.and_then(|u| u.get("onDemand")) {
        let enabled = od.get("enabled").and_then(Value::as_bool) == Some(true);
        let limit = util::json_f64(od.get("limit")).filter(|l| *l > 0.0);
        let used = util::json_f64(od.get("used"));
        if let (true, Some(limit), Some(used)) = (enabled, limit, used) {
            out.push(decorate(
                LimitWindow::new("on_demand", LABEL_ON_DEMAND, WindowKind::Monthly)
                    .used(used / limit * 100.0)
                    .detail(format!("${used:.2} of ${limit:.2}")),
            ));
        }
    }
    if !out.is_empty() {
        return Ok(out);
    }
    let membership = v
        .get("membershipType")
        .and_then(Value::as_str)
        .unwrap_or("this");
    let template = if v.get("isUnlimited").and_then(Value::as_bool) == Some(true) {
        TPL_UNLIMITED
    } else {
        TPL_NOTHING
    };
    Err(tf(lang, template, &[("plan", membership)]))
}

pub(crate) fn discover(env: &Env) -> bool {
    db_path(env, None).is_file()
}

pub(crate) fn fetch(account: &Account, ctx: &FetchContext) -> Reading {
    let path = db_path(&ctx.env, Some(account));
    if !path.is_file() {
        return Reading::not_installed(account, ctx.now);
    }
    let creds = match read_creds(&ctx.env, &path) {
        Ok(c) => c,
        Err(CredsError::SignedOut) => {
            return Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, NOTE_SIGN_IN)
        }
        Err(CredsError::Undecryptable) => {
            return Reading::with_status(
                account,
                ctx.now,
                ReadingStatus::Error,
                NOTE_UNDECRYPTABLE,
            );
        }
    };
    let mut resp = None;
    for cookie in &creds.cookies {
        let req = HttpRequest::get(ENDPOINT)
            .header("Cookie", cookie.as_str())
            .header("Accept", "application/json")
            .timeout(util::HTTP_TIMEOUT);
        match ctx.http.send(&req) {
            // A rejected cookie form: try the next one.
            Ok(r) if matches!(r.status, 401 | 403) => resp = Some(r),
            Ok(r) => {
                resp = Some(r);
                break;
            }
            Err(e) => return util::failed(account, ctx, &e.to_string()),
        }
    }
    let Some(resp) = resp else {
        return Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, NOTE_SIGN_IN);
    };
    match resp.status {
        401 | 403 => {
            return Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, NOTE_REJECTED)
        }
        429 => {
            let wait = util::retry_after(&resp, ctx.now).unwrap_or(300).max(60);
            return util::rate_limited(account, ctx, wait, 1, NOTE_RATE_LIMITED);
        }
        s if !resp.is_success() => return util::failed(account, ctx, &format!("HTTP {s}")),
        _ => {}
    }
    let Ok(v) = resp.json::<Value>() else {
        return util::failed(account, ctx, NOTE_BAD_JSON);
    };
    let plan = v
        .get("membershipType")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(creds.plan)
        .map(|p| plan_label(&p));
    let mut reading = match parse_summary(&v, ctx.settings.lang()) {
        Ok(windows) => {
            let mut r = Reading::fresh(account, ctx.now, windows);
            r.note = Some(match &plan {
                Some(p) => format!("{p} · via Cursor"),
                None => "via Cursor".into(),
            });
            r
        }
        Err(note) => Reading::with_status(account, ctx.now, ReadingStatus::Empty, note),
    };
    reading.plan = plan;
    reading.identity = creds.email;
    reading
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::ProviderKind;
    use codenotch_core::FakeHttp;
    use serde_json::json;
    use std::sync::Arc;

    fn make_db(env: &Env, rows: &[(&str, &str)]) {
        let p = db_path(env, None);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let c = rusqlite::Connection::open(&p).unwrap();
        c.execute_batch("CREATE TABLE IF NOT EXISTS ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)").unwrap();
        for (k, v) in rows {
            c.execute("INSERT INTO ItemTable VALUES (?1, ?2)", [k, v])
                .unwrap();
        }
    }

    #[test]
    fn plain_and_encrypted_tokens() {
        use codenotch_providers::safe_storage::base64_encode;
        let key = || Some(SafeStorage::from_key([4u8; 32]));
        assert_eq!(
            plain_token("plain-jwt", || None).as_deref(),
            Some("plain-jwt")
        );
        let blob = SafeStorage::from_key([4u8; 32]).encrypt_for_test(b"the-jwt", [6u8; 12]);
        let stored = format!("cursor-safe-storage:v1:{}", base64_encode(&blob));
        assert_eq!(plain_token(&stored, key).as_deref(), Some("the-jwt"));
        assert_eq!(plain_token(&stored, || None), None, "no key, no token");
        assert_eq!(
            plain_token(&stored, || Some(SafeStorage::from_key([1u8; 32]))),
            None,
            "wrong key"
        );
        assert_eq!(
            plain_token("cursor-safe-storage:v2:abcd", key),
            None,
            "unknown format is never sent as a token"
        );
    }

    #[test]
    fn fetch_decrypts_a_session_encrypted_at_rest() {
        use codenotch_providers::safe_storage::base64_encode;
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let account = Account::new(ProviderKind::Cursor, "Cursor");
        let key = SafeStorage::from_key([2u8; 32]);
        let local_state = env.appdata.join("Cursor").join("Local State");
        std::fs::create_dir_all(local_state.parent().unwrap()).unwrap();
        std::fs::write(&local_state, key.local_state_for_test()).unwrap();
        let blob = key.encrypt_for_test(b"jwt-value", [9u8; 12]);
        let stored = format!("cursor-safe-storage:v1:{}", base64_encode(&blob));
        make_db(
            &env,
            &[
                ("cursorAuth/accessToken", stored.as_str()),
                ("cursorAuth/stripeMembershipAuthId", "google-oauth2|user_01"),
            ],
        );
        let http = Arc::new(FakeHttp::new().on(
            ENDPOINT,
            200,
            r#"{"membershipType":"ultra","individualUsage":{"plan":{"totalPercentUsed":75}}}"#,
        ));
        let r = fetch(&account, &FetchContext::new(1, http.clone(), env.clone()));
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.headline_percent(), Some(75.0));
        assert_eq!(
            http.requests()[0].header_value("cookie"),
            Some("WorkosCursorSessionToken=google-oauth2|user_01::jwt-value")
        );
        // A key that does not open it: an error, and nothing is sent.
        std::fs::write(
            &local_state,
            SafeStorage::from_key([3u8; 32]).local_state_for_test(),
        )
        .unwrap();
        let http = Arc::new(FakeHttp::new());
        let r = fetch(&account, &FetchContext::new(1, http.clone(), env));
        assert_eq!(r.status, ReadingStatus::Error);
        assert_eq!(r.note.as_deref(), Some(NOTE_UNDECRYPTABLE));
        assert!(http.requests().is_empty());
    }

    #[test]
    fn local_identity_reads_plan_and_email_without_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let account = Account::new(ProviderKind::Cursor, "Cursor");
        assert_eq!(local_identity(&env, &account), LocalIdentity::default());
        make_db(
            &env,
            &[
                ("cursorAuth/cachedEmail", "me@example.com"),
                ("cursorAuth/stripeMembershipType", "ultra"),
                ("cursorAuth/stripeSubscriptionStatus", "active"),
            ],
        );
        let local = local_identity(&env, &account);
        assert_eq!(local.identity.as_deref(), Some("me@example.com"));
        assert_eq!(local.plan.as_deref(), Some("Ultra"));
        assert_eq!(plan_label("free_trial"), "Free trial");
    }

    #[test]
    fn summary_windows_share_the_billing_cycle() {
        let ws = parse_summary(&json!({
            "billingCycleStart": "2024-02-24T03:32:15.933Z", "billingCycleEnd": "2024-03-24T04:32:15.933+01:00",
            "individualUsage": {"plan": {"totalPercentUsed": 10, "apiPercentUsed": 20},
                "onDemand": {"enabled": true, "used": 5, "limit": 10}}
        }), Lang::En)
        .unwrap();
        assert_eq!(
            ws.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
            ["included", "api", "on_demand"]
        );
        assert!(ws.iter().all(|w| w.window_seconds == Some(29 * 86_400)));
        assert_eq!(ws[0].used_percent, Some(10.0));
        assert_eq!(ws[2].used_percent, Some(50.0));
        assert_eq!(ws[0].resets_at, util::rfc3339_secs("2024-03-24T03:32:15Z"));
    }

    #[test]
    fn zero_is_a_reading_and_missing_is_nothing() {
        let ws = parse_summary(
            &json!({"individualUsage": {"plan": {"totalPercentUsed": 0, "apiPercentUsed": 0}}}),
            Lang::En,
        )
        .unwrap();
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].used_percent, Some(0.0));
        assert_eq!(ws[0].window_seconds, None);
        let disabled = parse_summary(
            &json!({"individualUsage": {"plan": {"totalPercentUsed": 3},
            "onDemand": {"enabled": false, "used": 5, "limit": 10}}}),
            Lang::En,
        )
        .unwrap();
        assert_eq!(disabled.len(), 1);
        let err = parse_summary(
            &json!({"membershipType": "enterprise", "isUnlimited": true}),
            Lang::En,
        )
        .unwrap_err();
        assert_eq!(err, "Unlimited on the enterprise plan — nothing to meter");
        assert!(parse_summary(&json!({}), Lang::En)
            .unwrap_err()
            .contains("nothing for Cursor to meter"));
        let err = parse_summary(
            &json!({"membershipType": "enterprise", "isUnlimited": true}),
            Lang::Fr,
        )
        .unwrap_err();
        assert_eq!(err, "Illimité avec l'offre enterprise — rien à mesurer");
    }

    #[test]
    fn french_plan_note_at_fetch_time_and_fixed_note_at_display_time() {
        use codenotch_core::i18n::tr_in;
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let account = Account::new(ProviderKind::Cursor, "Cursor");
        make_db(
            &env,
            &[
                ("cursorAuth/accessToken", "tok"),
                ("cursorAuth/stripeMembershipAuthId", "auth"),
            ],
        );
        let http = Arc::new(FakeHttp::new().on(ENDPOINT, 200, r#"{"membershipType":"free"}"#));
        let mut ctx = FetchContext::new(1, http, env.clone());
        ctx.settings.language = "fr".into();
        let r = fetch(&account, &ctx);
        assert_eq!(r.status, ReadingStatus::Empty);
        assert_eq!(
            r.note.as_deref(),
            Some("L'offre free n'a encore rien que Cursor puisse mesurer")
        );
        // A fixed note stays English in the reading; the UI translates it with the table.
        let http = Arc::new(FakeHttp::new().on(ENDPOINT, 401, ""));
        let mut ctx = FetchContext::new(1, http, env);
        ctx.settings.language = "fr".into();
        let note = fetch(&account, &ctx).note.unwrap();
        assert_eq!(note, NOTE_REJECTED);
        assert_eq!(
            tr_in(Lang::Fr, &note, crate::FR),
            "Session Cursor refusée — reconnectez-vous dans l'éditeur"
        );
    }

    #[test]
    fn fetch_reads_the_editor_session_and_parses() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let account = Account::new(ProviderKind::Cursor, "Cursor");
        let http = Arc::new(FakeHttp::new());
        let ctx = FetchContext::new(1_000, http.clone(), env.clone());
        assert_eq!(fetch(&account, &ctx).status, ReadingStatus::NotInstalled);
        assert!(!discover(&env));

        make_db(&env, &[("cursorAuth/cachedEmail", "me@example.com")]);
        assert!(discover(&env));
        assert_eq!(fetch(&account, &ctx).status, ReadingStatus::SignedOut);

        make_db(
            &env,
            &[
                ("cursorAuth/accessToken", "tok"),
                ("cursorAuth/stripeMembershipAuthId", "auth"),
                ("cursorAuth/stripeMembershipType", "pro"),
            ],
        );
        let http = Arc::new(FakeHttp::new().on(
            ENDPOINT,
            200,
            r#"{"membershipType":"pro","individualUsage":{"plan":{"totalPercentUsed":42.5}}}"#,
        ));
        let ctx = FetchContext::new(1_000, http.clone(), env.clone());
        let r = fetch(&account, &ctx);
        assert_eq!(r.status, ReadingStatus::Fresh);
        assert_eq!(r.headline_percent(), Some(42.5));
        assert_eq!(r.plan.as_deref(), Some("Pro"));
        assert_eq!(r.identity.as_deref(), Some("me@example.com"));
        let req = &http.requests()[0];
        assert_eq!(
            req.header_value("cookie"),
            Some("WorkosCursorSessionToken=auth::tok")
        );
        assert!(!format!("{r:?}").contains("tok"), "no token in the reading");

        let http = Arc::new(FakeHttp::new().on(ENDPOINT, 401, ""));
        assert_eq!(
            fetch(&account, &FetchContext::new(1, http.clone(), env.clone())).status,
            ReadingStatus::SignedOut
        );
        assert_eq!(
            http.requests().len(),
            2,
            "both cookie forms are tried before giving up"
        );
        assert_eq!(
            http.requests()[1].header_value("cookie"),
            Some("WorkosCursorSessionToken=auth%3A%3Atok")
        );
        assert_eq!(
            cookie_forms("google-oauth2|user_01", "t"),
            [
                "WorkosCursorSessionToken=google-oauth2|user_01::t",
                "WorkosCursorSessionToken=user_01%3A%3At"
            ]
        );
        let http = Arc::new(FakeHttp::new().on(ENDPOINT, 200, r#"{"membershipType":"free"}"#));
        assert_eq!(
            fetch(&account, &FetchContext::new(1, http, env.clone())).status,
            ReadingStatus::Empty
        );
        let http = Arc::new(FakeHttp::new().on(ENDPOINT, 500, ""));
        let mut ctx = FetchContext::new(2_000, http, env);
        ctx.previous = Some(r);
        let stale = fetch(&account, &ctx);
        assert_eq!(stale.status, ReadingStatus::Stale);
        assert_eq!(stale.fetched_at, 1_000);
    }
}
