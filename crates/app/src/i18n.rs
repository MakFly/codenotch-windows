//! French for the app shell's own messages, and registration of the other crates' tables.

use codenotch_core::i18n::{fill, register_fr, tr_in, Lang};

/// App table (no key repeated from core's table).
pub static FR: &[(&str, &str)] = &[
    (
        "The global shortcut {key} is not valid.",
        "Le raccourci global {key} n'est pas valide.",
    ),
    (
        "The global shortcut {key} could not be registered: {error}",
        "Le raccourci global {key} n'a pas pu être enregistré : {error}",
    ),
    // --- update toasts ---
    ("Checking for updates…", "Recherche de mises à jour…"),
    (
        "CodeNotch is up to date ({version}).",
        "CodeNotch est à jour ({version}).",
    ),
    (
        "CodeNotch {version} is available",
        "CodeNotch {version} est disponible",
    ),
    ("Notes", "Notes"),
    ("Downloading… {n}%", "Téléchargement… {n} %"),
    (
        "Required update to {version}…",
        "Mise à jour obligatoire vers {version}…",
    ),
    (
        "Required update to {version}… {n}%",
        "Mise à jour obligatoire vers {version}… {n} %",
    ),
    ("Restarting…", "Redémarrage…"),
    ("Updated to {version}", "Mis à jour vers {version}"),
    (
        "Could not check for updates: {msg}",
        "Impossible de rechercher les mises à jour : {msg}",
    ),
    ("Update failed: {msg}", "Échec de la mise à jour : {msg}"),
];

/// Translate an app key and fill its placeholders.
pub fn tf(lang: Lang, key: &str, args: &[(&str, &str)]) -> String {
    fill(tr_in(lang, key, FR), args)
}

/// Tables whose keys reach `codenotch_core::i18n::tr` from other crates: reading notes/errors
/// (providers), update-check reasons and notices (engine), tray lines (platform).
pub fn tables() -> [&'static [(&'static str, &'static str)]; 4] {
    [
        codenotch_providers::FR,
        codenotch_providers_extra::FR,
        codenotch_engine::FR,
        codenotch_platform::tray::model::FR,
    ]
}

/// Resolve `language: "auto"` from the OS and register the French tables. Call once at
/// start-up, before any thread translates.
pub fn init() {
    codenotch_core::i18n::set_system_lang(codenotch_platform::system_lang());
    for table in tables() {
        register_fr(table);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn registered_tables_agree_with_each_other_and_core() {
        // The same English key must not get two different French texts depending on which
        // table `tr` happens to consult first.
        let mut seen: HashMap<&str, &str> = codenotch_core::i18n::FR.iter().copied().collect();
        for table in tables().into_iter().chain([FR]) {
            let mut local = std::collections::HashSet::new();
            for (k, v) in table {
                assert!(local.insert(*k), "duplicate key in one table: {k}");
                if let Some(prev) = seen.insert(k, v) {
                    assert_eq!(prev, *v, "conflicting French for {k:?}");
                }
            }
        }
    }

    #[test]
    fn app_messages_translate() {
        assert_eq!(
            tf(
                Lang::Fr,
                "The global shortcut {key} is not valid.",
                &[("key", "Ctrl+?")]
            ),
            "Le raccourci global Ctrl+? n'est pas valide."
        );
        assert_eq!(
            tf(
                Lang::En,
                "The global shortcut {key} is not valid.",
                &[("key", "X")]
            ),
            "The global shortcut X is not valid."
        );
    }
}
