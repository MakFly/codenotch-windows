//! Alert texts (English keys, crate-local French table).
//!
//! Formats (EN / FR):
//! * Warn / Critical — title `"{name}: {window} at {n}%"`; body `"{Warning|Critical} · {n}% used · {left}% left"`
//!   + `"\nResets in 2 h 14 min"` when the reset time is known.
//! * Forecast — title `"{name}: {window} will run out in about {m} min"`; body `"{n}% used · {left}% left"`
//!   + reset line.
//! * Reset — title `"{name}: {window} renewed"`; body `"Quota available · {n}%"` (n = remaining)
//!   + `"\nNext reset in …"` when known.
//! * Test — title `"Test message from CodeNotch"`; body `"If you can read this, {channel} alerts work."`

use codenotch_core::format::{percent, reset_in, used_line};
use codenotch_core::i18n::{fill, tr_in, Lang};
use codenotch_core::model::{Alert, AlertKind, LimitWindow};

/// Crate-local French strings (used through [`t`]).
pub static FR: &[(&str, &str)] = &[
    (
        "{name}: {window} renewed",
        "{name} : {window} — quota renouvelé",
    ),
    ("Resets {when}", "Réinitialisation {when}"),
    ("Next reset {when}", "Prochaine réinitialisation {when}"),
    (
        "If you can read this, {channel} alerts work.",
        "Si vous lisez ceci, les alertes {channel} fonctionnent.",
    ),
    // errors shown by the settings window (send_test) and logged by deliver
    (
        "Telegram bot token is missing. Add it in Settings or set {var}.",
        "Le jeton du bot Telegram est manquant. Ajoutez-le dans les Réglages ou définissez {var}.",
    ),
    (
        "Telegram chat id is missing.",
        "L'identifiant de discussion Telegram est manquant.",
    ),
    (
        "Telegram bot token looks invalid.",
        "Le jeton du bot Telegram semble invalide.",
    ),
    (
        "Telegram chat id looks invalid.",
        "L'identifiant de discussion Telegram semble invalide.",
    ),
    (
        "Telegram rejected the bot token.",
        "Telegram a refusé le jeton du bot.",
    ),
    (
        "Telegram refused the message: {reason}",
        "Telegram a refusé le message : {reason}",
    ),
    (
        "Discord webhook URL is missing. Add it in Settings or set {var}.",
        "L'URL du webhook Discord est manquante. Ajoutez-la dans les Réglages ou définissez {var}.",
    ),
    (
        "This is not a Discord webhook URL.",
        "Ce n'est pas une URL de webhook Discord.",
    ),
    (
        "Discord rejected the webhook (deleted or wrong URL).",
        "Discord a refusé le webhook (supprimé ou URL erronée).",
    ),
    (
        "Discord refused the message: {reason}",
        "Discord a refusé le message : {reason}",
    ),
    (
        "{channel} is rate limiting: try again in {s} s.",
        "{channel} limite les envois : réessayez dans {s} s.",
    ),
    (
        "{channel} answered with HTTP {code}.",
        "{channel} a répondu HTTP {code}.",
    ),
    (
        "Could not reach {channel}: {reason}",
        "Impossible de joindre {channel} : {reason}",
    ),
    ("timed out", "délai dépassé"),
    ("no connection", "pas de connexion"),
    ("TLS error", "erreur TLS"),
    ("unexpected error", "erreur inattendue"),
    (
        "Windows notifications are not available.",
        "Les notifications Windows ne sont pas disponibles.",
    ),
    (
        "Windows notification failed: {reason}",
        "La notification Windows a échoué : {reason}",
    ),
];

/// Translate with the crate table first, then core's.
pub fn t(lang: Lang, key: &str) -> &str {
    tr_in(lang, key, FR)
}

/// Translate and fill placeholders.
pub fn tf(lang: Lang, key: &str, args: &[(&str, &str)]) -> String {
    fill(t(lang, key), args)
}

/// "85" for 85 %, "99.6" … (no `%` sign: the templates carry it).
fn number(p: f64) -> String {
    percent(Some(p)).trim_end_matches('%').to_string()
}

fn reset_line(lang: Lang, key: &str, resets_at: Option<i64>, now: i64) -> Option<String> {
    let at = resets_at?;
    if at <= now {
        return None;
    }
    let when = reset_in(lang, Some(at), now)?;
    Some(tf(lang, key, &[("when", &when)]))
}

fn join(first: String, second: Option<String>) -> String {
    match second {
        Some(s) => format!("{first}\n{s}"),
        None => first,
    }
}

/// Warn / Critical alert. `window.used_percent` must be known.
pub fn threshold_alert(
    kind: AlertKind,
    account_id: &str,
    name: &str,
    window: &LimitWindow,
    lang: Lang,
    now: i64,
) -> Alert {
    let used = window.used_percent;
    let label = t(lang, &window.label_key);
    let n = used
        .map(number)
        .unwrap_or_else(|| codenotch_core::format::DASH.to_string());
    let title = tf(
        lang,
        "{name}: {window} at {n}%",
        &[("name", name), ("window", label), ("n", &n)],
    );
    let severity = if kind == AlertKind::Critical {
        t(lang, "Critical")
    } else {
        t(lang, "Warning")
    };
    let first = format!("{severity} · {}", used_line(lang, used, false));
    let body = join(
        first,
        reset_line(lang, "Resets {when}", window.resets_at, now),
    );
    Alert {
        kind,
        account_id: account_id.to_string(),
        window_id: window.id.clone(),
        used_percent: used,
        at: now,
        title,
        body,
    }
}

/// Forecast alert.
pub fn forecast_alert(
    account_id: &str,
    name: &str,
    window: &LimitWindow,
    minutes_left: i64,
    lang: Lang,
    now: i64,
) -> Alert {
    let label = t(lang, &window.label_key);
    let m = minutes_left.max(1).to_string();
    let title = tf(
        lang,
        "{name}: {window} will run out in about {m} min",
        &[("name", name), ("window", label), ("m", &m)],
    );
    let body = join(
        used_line(lang, window.used_percent, false),
        reset_line(lang, "Resets {when}", window.resets_at, now),
    );
    Alert {
        kind: AlertKind::Forecast { minutes_left },
        account_id: account_id.to_string(),
        window_id: window.id.clone(),
        used_percent: window.used_percent,
        at: now,
        title,
        body,
    }
}

/// Reset alert.
#[allow(clippy::too_many_arguments)]
pub fn reset_alert(
    account_id: &str,
    name: &str,
    window_id: &str,
    label_key: &str,
    used_percent: Option<f64>,
    next_reset_at: Option<i64>,
    lang: Lang,
    now: i64,
) -> Alert {
    let label = t(lang, label_key);
    let title = tf(
        lang,
        "{name}: {window} renewed",
        &[("name", name), ("window", label)],
    );
    let first = match used_percent {
        Some(u) => tf(lang, "Quota available · {n}%", &[("n", &number(100.0 - u))]),
        None => String::new(),
    };
    let second = reset_line(lang, "Next reset {when}", next_reset_at, now);
    let body = match (first.is_empty(), second) {
        (true, Some(s)) => s,
        (true, None) => String::new(),
        (false, s) => join(first, s),
    };
    Alert {
        kind: AlertKind::Reset,
        account_id: account_id.to_string(),
        window_id: window_id.to_string(),
        used_percent,
        at: now,
        title,
        body,
    }
}

/// Title and body of a test message for `channel` ("Telegram", "Discord", "Windows").
pub fn test_message(lang: Lang, channel: &str) -> (String, String) {
    let channel = t(lang, channel);
    (
        t(lang, "Test message from CodeNotch").to_string(),
        tf(
            lang,
            "If you can read this, {channel} alerts work.",
            &[("channel", channel)],
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::WindowKind;

    const NOW: i64 = 1_700_000_000;

    fn session(used: f64) -> LimitWindow {
        LimitWindow::new("session", "Current session", WindowKind::Session)
            .used(used)
            .resets_at(NOW + 2 * 3600 + 14 * 60)
    }

    #[test]
    fn french_table_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        let ph = |s: &str| {
            let mut v: Vec<String> = s
                .split('{')
                .skip(1)
                .filter_map(|p| p.split('}').next().map(String::from))
                .collect();
            v.sort();
            v
        };
        for (k, v) in FR {
            assert!(seen.insert(*k), "duplicate {k}");
            assert!(!v.is_empty());
            assert_eq!(ph(k), ph(v), "placeholders differ for {k}");
        }
    }

    #[test]
    fn threshold_texts_en_fr() {
        let a = threshold_alert(
            AlertKind::Warn,
            "acc",
            "Claude",
            &session(85.0),
            Lang::En,
            NOW,
        );
        assert_eq!(a.title, "Claude: Current session at 85%");
        assert_eq!(
            a.body,
            "Warning · 85% used · 15% left\nResets in 2 h 14 min"
        );
        let a = threshold_alert(
            AlertKind::Critical,
            "acc",
            "Claude",
            &session(96.0),
            Lang::Fr,
            NOW,
        );
        assert_eq!(a.title, "Claude : Session en cours à 96 %");
        assert_eq!(
            a.body,
            "Critique · 96 % utilisés · 4 % restants\nRéinitialisation dans 2 h 14 min"
        );
        assert_eq!(a.window_id, "session");
        // unknown reset → one line
        let w = LimitWindow::new("w", "Weekly limit", WindowKind::Weekly).used(80.0);
        let a = threshold_alert(AlertKind::Warn, "acc", "Codex", &w, Lang::Fr, NOW);
        assert_eq!(a.body, "Attention · 80 % utilisés · 20 % restants");
    }

    #[test]
    fn forecast_and_reset_texts() {
        let a = forecast_alert("acc", "Codex", &session(70.0), 25, Lang::En, NOW);
        assert_eq!(
            a.title,
            "Codex: Current session will run out in about 25 min"
        );
        assert_eq!(a.body, "70% used · 30% left\nResets in 2 h 14 min");
        assert_eq!(a.kind, AlertKind::Forecast { minutes_left: 25 });
        let a = forecast_alert("acc", "Codex", &session(70.0), 25, Lang::Fr, NOW);
        assert_eq!(
            a.title,
            "Codex : Session en cours sera épuisée dans environ 25 min"
        );

        let r = reset_alert(
            "acc",
            "Claude",
            "session",
            "Current session",
            Some(2.0),
            Some(NOW + 5 * 3600),
            Lang::En,
            NOW,
        );
        assert_eq!(r.title, "Claude: Current session renewed");
        assert_eq!(r.body, "Quota available · 98%\nNext reset in 5 h");
        let r = reset_alert(
            "acc",
            "Claude",
            "seven_day",
            "Weekly limit",
            Some(0.0),
            None,
            Lang::Fr,
            NOW,
        );
        assert_eq!(r.title, "Claude : Limite hebdomadaire — quota renouvelé");
        assert_eq!(r.body, "Quota disponible · 100 %");
    }

    #[test]
    fn test_texts() {
        let (t1, b1) = test_message(Lang::En, "Telegram");
        assert_eq!(t1, "Test message from CodeNotch");
        assert_eq!(b1, "If you can read this, Telegram alerts work.");
        let (t2, b2) = test_message(Lang::Fr, "Windows");
        assert_eq!(t2, "Message de test de CodeNotch");
        assert_eq!(b2, "Si vous lisez ceci, les alertes Windows fonctionnent.");
    }
}
