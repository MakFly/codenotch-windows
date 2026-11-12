//! Qoder.
//!
//! The read goes through a small Node sidecar, `qoder-usage.cjs`, which uses the official
//! `@qoder-ai/qoder-agent-sdk`: it starts the SDK daemon on the local signed-in `qodercli` session
//! (`qodercliAuth()`), calls the read-only account query `getUsageInfo()`, prints ONE JSON object
//! and exits. Envelope: `{"usage": {...}}` or `{"error": "needsAuth"|"sdk_missing"|…, "message"}`.
//! Qoder Desktop's DPAPI vault (`auth.v1.dat`) is never touched.
//!
//! Sidecar search order: `CODENOTCH_QODER_SIDECAR`, `<data dir>\qoder-sdk\`, the 0.x
//! `%APPDATA%\codenotch\qoder-sdk\`, next to the exe, `<exe dir>\resources\`. Node:
//! `CODENOTCH_QODER_NODE`, else `node.exe` on PATH. Hidden, 120 s timeout, process tree cleaned.
//!
//! `UsageInfo`: `userQuota` / `addOnQuota` `{total, used, remaining, percentage, unit}`,
//! `orgResourcePackage {used, cap, percentage, available}`, `totalUsagePercentage`, `expiresAt`
//! (ms; after 2100 = no renewal), `isQuotaExceeded`, `userType`. The `session` block is ignored.

use crate::i18n::{t, tf};
use crate::util;
use codenotch_core::i18n::Lang;
use codenotch_core::model::{LimitWindow, Reading, ReadingStatus, WindowKind};
use codenotch_core::provider::{Env, FetchContext};
use codenotch_core::Account;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

const RUN_TIMEOUT: Duration = Duration::from_secs(120);
/// `expiresAt` beyond 2100-01-01 is the "never expires" sentinel.
const NO_RENEWAL_AFTER: i64 = 4_102_444_800;
pub(crate) const SIDECAR_NAME: &str = "qoder-usage.cjs";
pub(crate) const NOTE_NO_SIDECAR: &str = "Qoder needs its usage sidecar: put qoder-usage.cjs in the qoder-sdk folder of the CodeNotch data folder and run npm install @qoder-ai/qoder-agent-sdk there";
pub(crate) const NOTE_NO_NODE: &str = "Qoder needs Node.js: install it or set CODENOTCH_QODER_NODE";
pub(crate) const NOTE_NO_SPAWN: &str = "Qoder read skipped (CODENOTCH_NO_SPAWN=1)";
pub(crate) const ERR_NO_JSON: &str = "the Qoder sidecar printed no JSON";
pub(crate) const NOTE_NEEDS_AUTH: &str = "qodercli is not signed in — run qodercli login";
pub(crate) const NOTE_SDK_MISSING: &str =
    "The Qoder SDK is not installed next to the sidecar — run npm install @qoder-ai/qoder-agent-sdk in the qoder-sdk folder";
pub(crate) const NOTE_NOTHING: &str = "getUsageInfo returned nothing — sign in to qodercli";
pub(crate) const NOTE_EXCEEDED: &str = "quota exceeded";
pub(crate) const LABEL_PLAN: &str = "Plan usage";
pub(crate) const LABEL_TOTAL: &str = "Total usage";
pub(crate) const LABEL_ADDON: &str = "Add-on credits";
pub(crate) const LABEL_ORG: &str = "Organization package";
// Templates translated at fetch time (`{e}` and the sidecar's codes stay as they are).
pub(crate) const TPL_SIDECAR: &str = "Qoder sidecar: {e}";
pub(crate) const TPL_EXITED: &str = "Qoder sidecar exited with {code}";
pub(crate) const TPL_SIDECAR_ERROR: &str = "Qoder sidecar error ({code})";
pub(crate) const TPL_LEFT: &str = "{n} {unit} left";
pub(crate) const TPL_NO_BUCKET: &str = "{note} — the plan publishes no quota bucket to meter";

pub(crate) fn qoder_home(env: &Env, account: Option<&Account>) -> PathBuf {
    match account {
        Some(a) => env.profile_dir(a, ".qoder"),
        None => env.home.join(".qoder"),
    }
}

pub(crate) fn installed(env: &Env, account: Option<&Account>) -> bool {
    qoder_home(env, account).is_dir()
}

pub(crate) fn sidecar_candidates(env: &Env) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = env.var("CODENOTCH_QODER_SIDECAR") {
        v.push(PathBuf::from(p));
    }
    v.push(util::data_dir(env).join("qoder-sdk").join(SIDECAR_NAME));
    v.push(
        env.appdata
            .join("codenotch")
            .join("qoder-sdk")
            .join(SIDECAR_NAME),
    );
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(PathBuf::from))
    {
        v.push(dir.join(SIDECAR_NAME));
        v.push(dir.join("resources").join(SIDECAR_NAME));
    }
    v
}

fn node(env: &Env) -> Option<PathBuf> {
    match env.var("CODENOTCH_QODER_NODE") {
        Some(p) => Some(PathBuf::from(p)).filter(|p| p.is_file()),
        None => util::find_on_path(env, "node.exe"),
    }
}

fn num(v: &Value, key: &str) -> Option<f64> {
    util::json_f64(v.get(key))
}

/// A bucket that meters something (an all-zero plan bucket is "nothing", not 0 %).
fn bucket<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get(key).filter(|b| b.is_object()).filter(|b| {
        num(b, "total").unwrap_or(0.0) > 0.0
            || num(b, "used").unwrap_or(0.0) > 0.0
            || num(b, "percentage").unwrap_or(0.0) > 0.0
    })
}

/// Used percent: used / total first (unambiguous), else a percentage that may be points (> 1.5)
/// or a fraction.
fn used_percent(pct: Option<&Value>, used: Option<&Value>, total: Option<&Value>) -> Option<f64> {
    if let (Some(u), Some(t)) = (
        util::json_f64(used),
        util::json_f64(total).filter(|t| *t > 0.0),
    ) {
        return Some(u / t * 100.0);
    }
    let p = util::json_f64(pct)?;
    Some(if p > 1.5 { p } else { p * 100.0 })
}

fn humanise(s: &str) -> String {
    util::capitalize(&s.replace('_', " "))
}

/// `UsageInfo` → (windows, note in `lang`).
pub(crate) fn windows_from(v: &Value, lang: Lang) -> (Vec<LimitWindow>, String) {
    let mut out = Vec::new();
    let renews = util::json_f64(v.get("expiresAt"))
        .and_then(|ms| util::epoch_secs(ms, 1e11))
        .filter(|t| *t < NO_RENEWAL_AFTER);
    let plan = |label: &str, used: f64| {
        let mut w = LimitWindow::new("quota", label, WindowKind::Monthly).used(used);
        w.resets_at = renews;
        w
    };
    match bucket(v, "userQuota") {
        Some(q) => {
            if let Some(u) = used_percent(q.get("percentage"), q.get("used"), q.get("total")) {
                out.push(plan(LABEL_PLAN, u));
            }
        }
        None => {
            if let Some(p) = num(v, "totalUsagePercentage") {
                out.push(plan(LABEL_TOTAL, if p > 1.5 { p } else { p * 100.0 }));
            }
        }
    }
    if let Some(a) = bucket(v, "addOnQuota") {
        if let Some(u) = used_percent(a.get("percentage"), a.get("used"), a.get("total")) {
            out.push(LimitWindow::new("addon", LABEL_ADDON, WindowKind::Other).used(u));
        }
    }
    if let Some(o) = v.get("orgResourcePackage").filter(|o| o.is_object()) {
        if o.get("available").and_then(Value::as_bool).unwrap_or(true) {
            if let Some(u) = used_percent(o.get("percentage"), o.get("used"), o.get("cap")) {
                out.push(LimitWindow::new("org", LABEL_ORG, WindowKind::Other).used(u));
            }
        }
    }
    // A composed note: its parts are localised here (the unit comes from the server, as is).
    let mut bits = vec![humanise(
        v.get("userType").and_then(Value::as_str).unwrap_or("Qoder"),
    )];
    if let Some(q) = ["userQuota", "addOnQuota", "orgResourcePackage"]
        .iter()
        .find_map(|k| bucket(v, k))
    {
        if let Some(rem) = num(q, "remaining") {
            let unit = q.get("unit").and_then(Value::as_str).unwrap_or("credits");
            bits.push(tf(
                lang,
                TPL_LEFT,
                &[("n", &format!("{rem:.0}")), ("unit", unit)],
            ));
        }
    }
    if v.get("isQuotaExceeded").and_then(Value::as_bool) == Some(true) {
        bits.push(t(lang, NOTE_EXCEEDED).to_string());
    }
    (out, bits.join(" · "))
}

/// What the sidecar said.
#[derive(Debug, PartialEq)]
pub(crate) enum SidecarResult {
    Usage(Value),
    SignedOut(String),
    Failed(String),
}

/// Sidecar stdout → result. Its free-text `message` is never surfaced (it may carry account
/// details); only the error code is (inside a template in `lang`).
pub(crate) fn interpret(text: &str, lang: Lang) -> SidecarResult {
    let Ok(v) = serde_json::from_str::<Value>(text.trim()) else {
        return SidecarResult::Failed(ERR_NO_JSON.into());
    };
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return match err {
            "needsAuth" => SidecarResult::SignedOut(NOTE_NEEDS_AUTH.into()),
            "sdk_missing" => SidecarResult::SignedOut(NOTE_SDK_MISSING.into()),
            other => SidecarResult::Failed(tf(
                lang,
                TPL_SIDECAR_ERROR,
                &[("code", &util::short(other))],
            )),
        };
    }
    match v.get("usage") {
        None | Some(Value::Null) => SidecarResult::SignedOut(NOTE_NOTHING.into()),
        Some(u) => SidecarResult::Usage(u.clone()),
    }
}

pub(crate) fn fetch(account: &Account, ctx: &FetchContext) -> Reading {
    let env = &ctx.env;
    if !installed(env, Some(account)) {
        return Reading::not_installed(account, ctx.now);
    }
    let Some(script) = sidecar_candidates(env).into_iter().find(|p| p.is_file()) else {
        return Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, NOTE_NO_SIDECAR);
    };
    let Some(node) = node(env) else {
        return Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, NOTE_NO_NODE);
    };
    if util::spawn_disabled(env) {
        return util::failed(account, ctx, NOTE_NO_SPAWN);
    }
    let lang = ctx.settings.lang();
    let script_arg = script.to_string_lossy().into_owned();
    let out = match crate::process::run_hidden(
        env,
        &node,
        &[script_arg.as_str()],
        script.parent(),
        RUN_TIMEOUT,
        1024 * 1024,
    ) {
        Ok(o) => o,
        Err(e) => return util::failed(account, ctx, &tf(lang, TPL_SIDECAR, &[("e", &e)])),
    };
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.success && !text.starts_with('{') {
        let code = out.code.map_or_else(|| "?".to_string(), |c| c.to_string());
        return util::failed(account, ctx, &tf(lang, TPL_EXITED, &[("code", &code)]));
    }
    match interpret(&text, lang) {
        SidecarResult::Usage(u) => {
            let (windows, note) = windows_from(&u, lang);
            if windows.is_empty() {
                return Reading::with_status(
                    account,
                    ctx.now,
                    ReadingStatus::Empty,
                    tf(lang, TPL_NO_BUCKET, &[("note", &note)]),
                );
            }
            let mut r = Reading::fresh(account, ctx.now, windows);
            r.plan = u.get("userType").and_then(Value::as_str).map(humanise);
            r.note = Some(format!("{note} · via Qoder"));
            r
        }
        SidecarResult::SignedOut(note) => {
            Reading::with_status(account, ctx.now, ReadingStatus::SignedOut, note)
        }
        SidecarResult::Failed(e) => util::failed(account, ctx, &e),
    }
}

/// A sidecar for integration to install next to the SDK (`<data dir>\qoder-sdk\qoder-usage.cjs`).
/// Written for this project from the SDK's documented entry points; not run here.
pub const QODER_SIDECAR_SOURCE: &str = include_str!("../assets/qoder-usage.cjs");

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::ProviderKind;
    use codenotch_core::FakeHttp;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn plan_bucket_points_fraction_and_counts() {
        let (w, note) = windows_from(
            &json!({"userType":"Pro","expiresAt":1760000000000u64,
            "userQuota":{"used":40,"total":100,"remaining":60,"percentage":40,"unit":"credits"}}),
            Lang::En,
        );
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].id, "quota");
        assert_eq!(w[0].used_percent, Some(40.0));
        assert_eq!(w[0].resets_at, Some(1_760_000_000));
        assert!(note.contains("60 credits left"), "{note}");
        let (w, _) = windows_from(&json!({"userQuota":{"percentage":0.25}}), Lang::En);
        assert_eq!(w[0].used_percent, Some(25.0));
        let (w, _) = windows_from(
            &json!({"userQuota":{"used":1,"total":100,"percentage":1}}),
            Lang::En,
        );
        assert_eq!(
            w[0].used_percent,
            Some(1.0),
            "1 % with counts is not a full ring"
        );
    }

    #[test]
    fn empty_plan_bucket_yields_to_total() {
        let (w, note) = windows_from(
            &json!({
            "userType":"personal_standard","totalUsagePercentage":96,"expiresAt":253402214400000u64,
            "userQuota":{"total":0,"used":0,"remaining":0,"percentage":0,"unit":"credits"},
            "addOnQuota":{"total":2100,"used":2000,"remaining":100,"percentage":96,"unit":"credits"}}),
            Lang::En,
        );
        assert_eq!(
            w.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
            ["quota", "addon"]
        );
        assert_eq!(w[0].label_key, "Total usage");
        assert_eq!(w[0].used_percent, Some(96.0));
        assert_eq!(w[0].resets_at, None, "year 9999 is no renewal");
        assert!(
            note.contains("Personal standard") && note.contains("100 credits left"),
            "{note}"
        );
    }

    #[test]
    fn french_note_parts_and_sidecar_codes() {
        let (w, note) = windows_from(
            &json!({"userType":"Pro","isQuotaExceeded":true,"userQuota":{"used":40,"total":100,"remaining":60,"unit":"credits"}}),
            Lang::Fr,
        );
        assert_eq!(w[0].label_key, LABEL_PLAN, "labels stay canonical English");
        assert_eq!(note, "Pro · 60 credits restants · quota dépassé");
        let (_, en) = windows_from(
            &json!({"userType":"Pro","isQuotaExceeded":true,"userQuota":{"used":40,"total":100,"remaining":60,"unit":"credits"}}),
            Lang::En,
        );
        assert_eq!(en, "Pro · 60 credits left · quota exceeded");
        assert_eq!(
            interpret(r#"{"error":"daemon_failed","message":"secret"}"#, Lang::Fr),
            SidecarResult::Failed("Erreur du sidecar Qoder (daemon_failed)".into())
        );
        // A fixed message stays English; the UI translates it with the registered table.
        let SidecarResult::SignedOut(m) = interpret(r#"{"error":"needsAuth"}"#, Lang::Fr) else {
            panic!()
        };
        assert_eq!(
            codenotch_core::i18n::tr_in(Lang::Fr, &m, crate::FR),
            "qodercli n'est pas connecté — lancez qodercli login"
        );
    }

    #[test]
    fn missing_numbers_publish_nothing() {
        assert!(windows_from(&json!({}), Lang::En).0.is_empty());
        assert!(
            windows_from(&json!({"userQuota":{"total":0,"used":0}}), Lang::En)
                .0
                .is_empty()
        );
        assert!(windows_from(
            &json!({"orgResourcePackage":{"used":10,"cap":40,"available":false}}),
            Lang::En
        )
        .0
        .is_empty());
        let (w, _) = windows_from(
            &json!({"userQuota":{"percentage":50},"orgResourcePackage":{"used":10,"cap":40}}),
            Lang::En,
        );
        assert_eq!(w[1].id, "org");
        assert_eq!(w[1].used_percent, Some(25.0));
        let (_, note) = windows_from(
            &json!({"totalUsagePercentage":100,"isQuotaExceeded":true}),
            Lang::En,
        );
        assert!(note.contains("quota exceeded"));
    }

    #[test]
    fn sidecar_envelopes() {
        assert!(matches!(
            interpret(r#"{"usage":{"totalUsagePercentage":5}}"#, Lang::En),
            SidecarResult::Usage(_)
        ));
        assert!(
            matches!(interpret(r#"{"error":"needsAuth","message":"no login for a@b.c"}"#, Lang::En), SidecarResult::SignedOut(m) if !m.contains("a@b.c"))
        );
        assert!(matches!(
            interpret(r#"{"error":"sdk_missing","message":"x"}"#, Lang::En),
            SidecarResult::SignedOut(_)
        ));
        assert!(
            matches!(interpret(r#"{"error":"daemon_failed","message":"secret"}"#, Lang::En), SidecarResult::Failed(m) if !m.contains("secret"))
        );
        assert!(matches!(
            interpret(r#"{"usage":null}"#, Lang::En),
            SidecarResult::SignedOut(_)
        ));
        assert!(matches!(
            interpret("garbage", Lang::En),
            SidecarResult::Failed(_)
        ));
    }

    #[test]
    fn fetch_without_install_or_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let a = Account::new(ProviderKind::Qoder, "Qoder");
        let ctx = FetchContext::new(0, Arc::new(FakeHttp::new()), env.clone());
        assert_eq!(fetch(&a, &ctx).status, ReadingStatus::NotInstalled);
        std::fs::create_dir_all(env.home.join(".qoder")).unwrap();
        assert!(installed(&env, None));
        let r = fetch(&a, &ctx);
        assert_eq!(r.status, ReadingStatus::SignedOut);
        assert!(r.note.unwrap().contains("sidecar"));
        let side = dir.path().join("side.cjs");
        std::fs::write(&side, "// test").unwrap();
        let env = env.with_var("CODENOTCH_QODER_SIDECAR", &side.to_string_lossy());
        let r = fetch(&a, &FetchContext::new(0, Arc::new(FakeHttp::new()), env));
        assert_eq!(r.status, ReadingStatus::SignedOut);
        assert!(r.note.unwrap().contains("Node.js"));
        assert!(QODER_SIDECAR_SOURCE.contains("getUsageInfo"));
    }
}
