//! French for the engine's notices and update-check reasons. Keys are the English texts.
//! Notices are translated when emitted ([`tr`]); `UpdateState::Failed` reasons stay English in
//! the snapshot and are translated by the UI through the registered table (the app calls
//! `codenotch_core::i18n::register_fr(codenotch_engine::FR)` at start-up).

use codenotch_core::i18n::{tr_in, Lang};

/// Engine table (no key repeated from core's table).
pub static FR: &[(&str, &str)] = &[
    // --- notices ---
    (
        "Only web links can be opened.",
        "Seuls les liens web peuvent être ouverts.",
    ),
    ("Unknown account.", "Compte inconnu."),
    ("No account to preview.", "Aucun compte à prévisualiser."),
    ("Test message sent.", "Message de test envoyé."),
    ("Hooks installed.", "Hooks installés."),
    ("Hooks removed.", "Hooks retirés."),
    ("Hooks not installed:", "Hooks non installés :"),
    (
        "Hooks not fully removed:",
        "Hooks pas entièrement retirés :",
    ),
    (
        "No Claude Code profile folder found.",
        "Aucun dossier de profil Claude Code trouvé.",
    ),
    (
        "codenotch-hook.exe was not found next to CodeNotch.",
        "codenotch-hook.exe est introuvable à côté de CodeNotch.",
    ),
    (
        "Settings not saved: an account has no id.",
        "Réglages non enregistrés : un compte n'a pas d'identifiant.",
    ),
    (
        "Settings not saved: two accounts share the same id.",
        "Réglages non enregistrés : deux comptes ont le même identifiant.",
    ),
    (
        "Hooks are already being updated; try again in a moment.",
        "Les hooks sont déjà en cours de mise à jour ; réessayez dans un instant.",
    ),
    (
        "The last refresh did not finish.",
        "La dernière actualisation ne s'est pas terminée.",
    ),
    (
        "Rate limited, retrying in {n}s",
        "Limité, nouvel essai dans {n} s",
    ),
    // --- Teams sync (notices and status) ---
    ("Turn on Team sync first.", "Activez d'abord la synchronisation d'équipe."),
    ("Detach this device first.", "Détachez d'abord cet appareil."),
    (
        "Read the invite again before connecting.",
        "Relisez l'invitation avant de vous connecter.",
    ),
    (
        "This device is not connected to a team.",
        "Cet appareil n'est connecté à aucune équipe.",
    ),
    ("Connected to the team.", "Connecté à l'équipe."),
    ("Device detached from the team.", "Appareil détaché de l'équipe."),
    ("Declaration sent to the team.", "Déclaration envoyée à l'équipe."),
    (
        "Your data on the team server is being erased.",
        "Vos données sur le serveur de l'équipe sont en cours d'effacement.",
    ),
    (
        "Set the Teams server address first.",
        "Renseignez d'abord l'adresse du serveur Teams.",
    ),
    ("Invalid Teams server address.", "Adresse du serveur Teams invalide."),
    (
        "This invite link is for another server.",
        "Ce lien d'invitation vise un autre serveur.",
    ),
    ("Invalid invite link.", "Lien d'invitation invalide."),
    (
        "This invite link is invalid or has expired.",
        "Ce lien d'invitation est invalide ou a expiré.",
    ),
    (
        "Cannot reach the Teams server.",
        "Impossible de joindre le serveur Teams.",
    ),
    (
        "The Teams server could not handle the request.",
        "Le serveur Teams n'a pas pu traiter la demande.",
    ),
    ("Check the account and the price.", "Vérifiez le compte et le prix."),
    (
        "The device token could not be saved.",
        "Le jeton de l'appareil n'a pas pu être enregistré.",
    ),
    (
        "Teams sync could not start",
        "La synchronisation d'équipe n'a pas pu démarrer",
    ),
    (
        "The Teams server address changed. Detach this device to pair again.",
        "L'adresse du serveur Teams a changé. Détachez cet appareil pour l'appairer de nouveau.",
    ),
    ("Device revoked by the team", "Appareil révoqué par l'équipe"),
    ("Sync paused, retrying", "Synchronisation en pause, nouvel essai"),
    (
        "Update CodeNotch to keep syncing",
        "Mettez CodeNotch à jour pour continuer la synchronisation",
    ),
    (
        "This server is not a CodeNotch Teams server",
        "Ce serveur n'est pas un serveur CodeNotch Teams",
    ),
    (
        "The team changed what it sees. Review and accept to resume.",
        "L'équipe a changé ce qu'elle voit. Relisez et acceptez pour reprendre.",
    ),
    (
        "Cannot save Teams sync data",
        "Impossible d'enregistrer les données de synchronisation d'équipe",
    ),
    // --- update notification ---
    (
        "CodeNotch {version} is available",
        "CodeNotch {version} est disponible",
    ),
    (
        "Open CodeNotch to install it.",
        "Ouvrez CodeNotch pour l'installer.",
    ),
    // --- update reasons (UpdateState::Failed) ---
    (
        "No update information published yet.",
        "Aucune information de mise à jour publiée pour l'instant.",
    ),
    (
        "The update address must start with https://.",
        "L'adresse de mise à jour doit commencer par https://.",
    ),
    (
        "No Windows package in the update information.",
        "Aucun paquet Windows dans les informations de mise à jour.",
    ),
    (
        "The download does not have the expected size.",
        "Le téléchargement n'a pas la taille attendue.",
    ),
    (
        "The download is corrupted (checksum mismatch).",
        "Le téléchargement est corrompu (somme de contrôle différente).",
    ),
    (
        "The update package is not valid.",
        "Le paquet de mise à jour n'est pas valide.",
    ),
    (
        "Self-update is off for development builds (running from a target folder).",
        "La mise à jour automatique est désactivée pour les builds de développement (lancé depuis un dossier target).",
    ),
    (
        "The install folder cannot be found.",
        "Le dossier d'installation est introuvable.",
    ),
    (
        "The update could not be installed; the current version was kept.",
        "La mise à jour n'a pas pu être installée ; la version actuelle a été conservée.",
    ),
    (
        "The new version could not be started; the current version was restored.",
        "La nouvelle version n'a pas pu démarrer ; la version actuelle a été rétablie.",
    ),
    (
        "The last update did not take effect.",
        "La dernière mise à jour n'a pas pris effet.",
    ),
    (
        "GitHub rate limit reached, try again later.",
        "Limite de requêtes GitHub atteinte, réessayez plus tard.",
    ),
    ("unreadable answer", "réponse illisible"),
    ("no version in the answer", "aucune version dans la réponse"),
    (
        "the check could not start",
        "la vérification n'a pas pu démarrer",
    ),
];

/// Translate an engine key (engine table first, then core's and the registered tables).
pub(crate) fn tr(lang: Lang, key: &str) -> &str {
    tr_in(lang, key, FR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_well_formed_and_does_not_repeat_core() {
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
            assert!(seen.insert(*k), "duplicate key {k}");
            assert!(!v.trim().is_empty(), "{k}");
            assert_eq!(ph(k), ph(v), "{k}");
            assert!(
                !codenotch_core::i18n::FR.iter().any(|(c, _)| c == k),
                "{k} is already in core"
            );
        }
        for key in crate::update::MESSAGES {
            assert!(FR.iter().any(|(k, _)| *k == key), "{key}");
        }
        assert_eq!(tr(Lang::Fr, "Hooks installed."), "Hooks installés.");
        assert_eq!(tr(Lang::En, "Hooks installed."), "Hooks installed.");
    }
}
