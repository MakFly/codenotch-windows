//! French strings of this crate (keys are the exact English texts, see `codenotch_core::i18n`).
//!
//! * Fixed messages stay English in the `Reading`; the UI translates them at display time once
//!   the app has registered [`FR`] with `codenotch_core::i18n::register_fr`.
//! * Messages with a variable part are built here at fetch time from an English template with
//!   `{placeholders}` ([`tf`]), so they reach the UI already in the user's language.

use codenotch_core::i18n::{fill, tr_in, Lang};

/// Translate a key with this crate's table (core's table and registered ones as fallback).
pub(crate) fn t(lang: Lang, key: &str) -> &str {
    tr_in(lang, key, FR)
}

/// Translate a template and fill its `{name}` placeholders. English = the template filled.
pub(crate) fn tf(lang: Lang, key: &str, args: &[(&str, &str)]) -> String {
    fill(tr_in(lang, key, FR), args)
}

/// French table of this crate. Keys unique, none repeated from core's table (tests check).
pub static FR: &[(&str, &str)] = &[
    // --- window labels / groups ---
    ("Weekly limit (model-scoped)", "Limite hebdomadaire (par modèle)"),
    ("Extra usage", "Usage supplémentaire"),
    ("Longer window", "Fenêtre longue"),
    ("Code review", "Revue de code"),
    // --- Claude ---
    ("No Claude Code credential found", "Aucun identifiant Claude Code trouvé"),
    (
        "Credential expired — run claude once in a terminal to renew it",
        "Identifiant expiré — lancez claude une fois dans un terminal pour le renouveler",
    ),
    ("Credential rejected (switched accounts?)", "Identifiant refusé (changement de compte ?)"),
    (
        "Claude HTTP 403: access denied. Check network or account access; sign-in may still be valid.",
        "Claude HTTP 403 : accès refusé. Vérifiez le réseau ou l'accès au compte ; la connexion est peut-être encore valide.",
    ),
    ("Claude reported no usage windows", "Claude n'a signalé aucune fenêtre d'usage"),
    ("Rate limited, retrying in {wait}s", "Limité, nouvel essai dans {wait} s"),
    // Claude sign-in (ClaudeAuthState::message and claude_sign_in errors)
    ("Complete sign-in in the browser or terminal window.", "Terminez la connexion dans le navigateur ou la fenêtre du terminal."),
    ("Sign-in complete. Refreshing usage...", "Connexion réussie. Actualisation de la consommation…"),
    ("Sign-in cancelled, failed or timed out. Try again.", "Connexion annulée, échouée ou expirée. Réessayez."),
    (
        "Claude Code CLI not found. Install the standalone CLI first.",
        "CLI Claude Code introuvable. Installez d'abord la CLI autonome.",
    ),
    ("Claude sign-in or renewal is already running.", "Une connexion ou un renouvellement Claude est déjà en cours."),
    ("Unable to open Claude sign-in window.", "Impossible d'ouvrir la fenêtre de connexion Claude."),
    ("process launch is disabled (CODENOTCH_NO_SPAWN=1)", "lancement de processus désactivé (CODENOTCH_NO_SPAWN=1)"),
    // --- Codex ---
    ("Codex sign-in expired — open Codex once to refresh it", "Connexion Codex expirée — ouvrez Codex une fois pour la rafraîchir"),
    ("Codex rejected its sign-in — sign in to Codex again", "Codex a refusé la connexion — reconnectez-vous à Codex"),
    ("Codex reported no usage windows", "Codex n'a signalé aucune fenêtre d'usage"),
    ("Codex has not recorded a usage snapshot yet", "Codex n'a pas encore enregistré de relevé d'usage"),
    ("Codex is not signed in", "Codex n'est pas connecté"),
    ("Rate limited — retrying in {wait}s", "Limité — nouvel essai dans {wait} s"),
    ("Live read failed ({e})", "Échec de la lecture en direct ({e})"),
    ("Live read failed (HTTP {code})", "Échec de la lecture en direct (HTTP {code})"),
    ("from last Codex run", "depuis la dernière exécution de Codex"),
    ("{plan} · from last Codex run", "{plan} · depuis la dernière exécution de Codex"),
    // --- Grok ---
    (
        "Run grok login — it signs in and refreshes the token this reads.",
        "Lancez grok login — il vous connecte et rafraîchit le jeton lu ici.",
    ),
    ("Grok session was rejected — run grok login again", "Session Grok refusée — relancez grok login"),
    ("Grok is rate limiting; the last reading stands", "Grok limite les requêtes ; le dernier relevé est conservé"),
    ("Grok has nothing metered on this account yet", "Grok ne mesure encore rien sur ce compte"),
    ("Grok returned no billing config", "Grok n'a renvoyé aucune configuration de facturation"),
    // --- custom endpoint ---
    ("No endpoint configured", "Aucun point d'accès configuré"),
    (
        "Endpoint must use HTTPS (plain HTTP only to localhost)",
        "Le point d'accès doit utiliser HTTPS (HTTP simple uniquement vers localhost)",
    ),
    ("Environment variable {var} is not set", "La variable d'environnement {var} n'est pas définie"),
    ("endpoint returned no \"windows\" list", "le point d'accès n'a renvoyé aucune liste \"windows\""),
    ("endpoint returned an unexpected JSON shape", "le point d'accès a renvoyé un JSON de forme inattendue"),
    ("endpoint returned no usable window", "le point d'accès n'a renvoyé aucune fenêtre exploitable"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn placeholders(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s
            .split('{')
            .skip(1)
            .filter_map(|p| p.split('}').next().map(String::from))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn table_is_well_formed_and_disjoint_from_core() {
        let core: HashSet<&str> = codenotch_core::i18n::FR.iter().map(|(k, _)| *k).collect();
        let mut seen = HashSet::new();
        for (k, v) in FR {
            assert!(!k.is_empty(), "empty key");
            assert!(seen.insert(*k), "duplicate key {k}");
            assert!(!v.trim().is_empty(), "empty value for {k}");
            assert_ne!(k, v, "useless entry {k}");
            assert_eq!(
                placeholders(k),
                placeholders(v),
                "placeholders differ for {k}"
            );
            assert!(!core.contains(k), "{k} is already in core's table");
        }
    }

    #[test]
    fn every_fixed_message_has_a_french_entry() {
        use crate::{claude, codex, custom, grok};
        let fixed = [
            claude::NOTE_NO_CREDENTIAL,
            claude::NOTE_EXPIRED,
            claude::NOTE_REJECTED,
            claude::NOTE_403,
            claude::NOTE_NO_WINDOWS,
            claude::MSG_SIGN_IN_PENDING,
            claude::MSG_SIGN_IN_DONE,
            claude::MSG_SIGN_IN_FAILED,
            claude::MSG_CLI_NOT_FOUND,
            claude::MSG_AUTH_BUSY,
            claude::MSG_SIGN_IN_WINDOW,
            claude::MSG_NO_SPAWN,
            claude::LABEL_SCOPED,
            claude::LABEL_EXTRA_USAGE,
            codex::NOTE_EXPIRED,
            codex::NOTE_REJECTED,
            codex::NOTE_NO_WINDOWS,
            codex::NOTE_NO_SNAPSHOT,
            codex::NOTE_NOT_SIGNED_IN,
            codex::NOTE_FROM_LAST_RUN,
            codex::LABEL_LONGER,
            codex::GROUP_CODE_REVIEW,
            grok::NOTE_SIGNED_OUT,
            grok::NOTE_REJECTED,
            grok::NOTE_RATE_LIMITED,
            grok::NOTE_NOTHING_METERED,
            grok::NOTE_NO_CONFIG,
            custom::NOTE_NO_ENDPOINT,
            custom::NOTE_BAD_ENDPOINT,
            custom::ERR_NO_LIST,
            custom::ERR_SHAPE,
            custom::ERR_NO_WINDOW,
        ];
        for key in fixed {
            assert!(
                FR.iter().any(|(k, _)| *k == key),
                "no French entry for {key:?}"
            );
            assert_ne!(tr_in(Lang::Fr, key, FR), key);
            assert_eq!(tr_in(Lang::En, key, FR), key);
        }
        // Templates used at fetch time.
        for key in [
            claude::TPL_RATE_LIMITED,
            codex::TPL_RATE_LIMITED,
            codex::TPL_LIVE_FAILED,
            codex::TPL_LIVE_HTTP,
            codex::TPL_PLAN_FROM_LAST_RUN,
            custom::TPL_ENV_UNSET,
        ] {
            assert!(
                FR.iter().any(|(k, _)| *k == key),
                "no French entry for template {key:?}"
            );
        }
    }

    #[test]
    fn english_fill_is_identity() {
        assert_eq!(
            tf(
                Lang::En,
                "Rate limited, retrying in {wait}s",
                &[("wait", "60")]
            ),
            "Rate limited, retrying in 60s"
        );
        assert_eq!(
            tf(
                Lang::Fr,
                "Rate limited, retrying in {wait}s",
                &[("wait", "60")]
            ),
            "Limité, nouvel essai dans 60 s"
        );
        assert_eq!(
            t(Lang::Fr, "Rate limited, trying again soon"),
            "Limité, nouvel essai bientôt",
            "core fallback"
        );
    }
}
