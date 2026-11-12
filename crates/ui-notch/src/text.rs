//! Crate-local strings. Keys are English; French comes from [`FR`] first, then core's table.

use codenotch_core::i18n::{fill, tr_in, Lang};

/// French for strings only this crate uses (core's table covers the rest).
pub static FR: &[(&str, &str)] = &[
    (
        "Last reported reset: {t}",
        "Dernière réinitialisation annoncée : {t}",
    ),
    ("Requests today", "Requêtes aujourd'hui"),
    ("Nothing to report", "Rien à signaler"),
    ("Retrying {when}", "Nouvel essai {when}"),
    ("Settings", "Réglages"),
    ("Move the notch", "Déplacer l'encoche"),
    (
        "Next reset: {label} · {when}",
        "Prochaine réinitialisation : {label} · {when}",
    ),
];

pub fn t(lang: Lang, key: &str) -> &str {
    tr_in(lang, key, FR)
}

pub fn tf(lang: Lang, key: &str, args: &[(&str, &str)]) -> String {
    fill(t(lang, key), args)
}

/// "Nov 20, 4:59 PM" / "20 nov. 16:59" (for "Last reported reset").
pub fn datetime(lang: Lang, month: u32, day: u32, hour: u32, minute: u32, clock24: bool) -> String {
    const EN: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const FR_M: [&str; 12] = [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ];
    let i = (month.clamp(1, 12) - 1) as usize;
    let time = codenotch_core::format::clock(hour, minute, clock24);
    match lang {
        Lang::Fr => format!("{day} {} {time}", FR_M[i]),
        Lang::En => format!("{} {day}, {time}", EN[i]),
    }
}

/// Does a Windows short-time pattern ("HH:mm", "h:mm tt") use a 24-hour clock?
pub fn clock24_from_pattern(pattern: &str) -> bool {
    pattern.contains('H')
}

/// Host part of a URL: `https://claude.ai/settings/usage` → `claude.ai`.
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest[..end].trim_start_matches("www.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french_first_from_local_then_core() {
        assert_eq!(
            tf(Lang::Fr, "Last reported reset: {t}", &[("t", "x")]),
            "Dernière réinitialisation annoncée : x"
        );
        assert_eq!(t(Lang::Fr, "Keep open"), "Garder ouverte");
        assert_eq!(t(Lang::En, "Keep open"), "Keep open");
    }

    #[test]
    fn local_table_is_consistent() {
        let mut seen = std::collections::HashSet::new();
        for (k, v) in FR {
            assert!(seen.insert(*k), "duplicate {k}");
            assert!(!v.is_empty());
            assert_eq!(k.matches('{').count(), v.matches('{').count(), "{k}");
        }
    }

    #[test]
    fn dates_hosts_and_clocks() {
        assert_eq!(datetime(Lang::En, 11, 20, 16, 59, false), "Nov 20, 4:59 PM");
        assert_eq!(datetime(Lang::Fr, 11, 20, 16, 59, true), "20 nov. 16:59");
        assert!(clock24_from_pattern("HH:mm"));
        assert!(!clock24_from_pattern("h:mm tt"));
        assert_eq!(host("https://claude.ai/settings/usage"), "claude.ai");
        assert_eq!(host("https://chatgpt.com/#settings/Account"), "chatgpt.com");
        assert_eq!(host("https://www.example.com"), "example.com");
        assert_eq!(host("grok.com?x"), "grok.com");
    }
}
