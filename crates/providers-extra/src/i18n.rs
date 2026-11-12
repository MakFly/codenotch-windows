//! French strings of this crate (keys are the exact English texts, see `codenotch_core::i18n`).
//!
//! * Fixed messages stay English in the `Reading`; the UI translates them at display time once
//!   the app has registered [`FR`] with `codenotch_core::i18n::register_fr`.
//! * Messages with a variable part are built at fetch time from an English template with
//!   `{placeholders}` ([`tf`]), so they reach the UI already in the user's language.
//! * The 429 marker kept in `Reading::error` (`util::RATE_MARK`) is parsed back and is never
//!   translated.

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
    // --- window labels ---
    ("On demand", "À la demande"),
    ("Chat requests", "Requêtes de chat"),
    ("Completions", "Complétions"),
    ("5-hour Limit", "Limite 5 h"),
    ("5h limit", "Limite 5 h"),
    ("Weekly", "Hebdomadaire"),
    ("Weekly Limit", "Limite hebdomadaire"),
    ("MCP (1 month)", "MCP (1 mois)"),
    ("Usage", "Consommation"),
    ("Requests today", "Requêtes aujourd'hui"),
    ("Requests today · no limit published", "Requêtes aujourd'hui · aucune limite publiée"),
    ("Plan usage", "Usage du forfait"),
    ("Total usage", "Usage total"),
    ("Add-on credits", "Crédits additionnels"),
    ("Organization package", "Forfait de l'organisation"),
    // --- shared templates (fetch time) ---
    ("Rate limited — retrying in {wait}s", "Limité — nouvel essai dans {wait} s"),
    ("Live read failed ({e})", "Échec de la lecture en direct ({e})"),
    ("Live read failed (HTTP {code})", "Échec de la lecture en direct (HTTP {code})"),
    // --- Cursor ---
    ("Sign in to Cursor (the editor) to see usage.", "Connectez-vous à Cursor (l'éditeur) pour voir votre consommation."),
    ("Cursor session was rejected — sign in again in the editor", "Session Cursor refusée — reconnectez-vous dans l'éditeur"),
    ("Cursor is rate limiting; the last reading stands", "Cursor limite les requêtes ; le dernier relevé est conservé"),
    ("Cursor answered with unreadable JSON", "Cursor a répondu avec un JSON illisible"),
    (
        "Cursor's saved session could not be decrypted — reopen Cursor, or sign in again",
        "La session Cursor enregistrée n'a pas pu être déchiffrée — rouvrez Cursor ou reconnectez-vous",
    ),
    ("Unlimited on the {plan} plan — nothing to meter", "Illimité avec l'offre {plan} — rien à mesurer"),
    ("The {plan} plan has nothing for Cursor to meter yet", "L'offre {plan} n'a encore rien que Cursor puisse mesurer"),
    // --- GitHub Copilot ---
    (
        "Run gh auth login — CodeNotch borrows the GitHub CLI's session.",
        "Lancez gh auth login — CodeNotch emprunte la session de la CLI GitHub.",
    ),
    ("GitHub CLI token read skipped (CODENOTCH_NO_SPAWN=1)", "Lecture du jeton de la CLI GitHub ignorée (CODENOTCH_NO_SPAWN=1)"),
    ("GitHub session was rejected — run gh auth login again", "Session GitHub refusée — relancez gh auth login"),
    ("GitHub is rate limiting; the last reading stands", "GitHub limite les requêtes ; le dernier relevé est conservé"),
    ("GitHub answered with unreadable JSON", "GitHub a répondu avec un JSON illisible"),
    ("GitHub Copilot returned no quota snapshots", "GitHub Copilot n'a renvoyé aucun relevé de quota"),
    ("GitHub Copilot reported no metered quotas", "GitHub Copilot n'a signalé aucun quota mesuré"),
    // --- OpenCode ---
    ("Run opencode auth login to sign in to OpenCode.", "Lancez opencode auth login pour vous connecter à OpenCode."),
    ("No OpenCode Go subscription on this account", "Aucun abonnement OpenCode Go sur ce compte"),
    (
        "OpenCode's sign-in has expired — open OpenCode to renew it",
        "La connexion OpenCode a expiré — ouvrez OpenCode pour la renouveler",
    ),
    ("OpenCode rejected the sign-in — run opencode auth login", "OpenCode a refusé la connexion — lancez opencode auth login"),
    (
        "OpenCode rejected the opencode-go key, or it has no Go plan",
        "OpenCode a refusé la clé opencode-go, ou elle n'a pas d'offre Go",
    ),
    ("The Go plan reported no usage windows", "L'offre Go n'a signalé aucune fenêtre d'usage"),
    // --- Command Code ---
    ("Sign in with the Command Code app to read your usage", "Connectez-vous avec l'app Command Code pour lire votre consommation"),
    ("Command Code billing", "Facturation Command Code"),
    ("Command Code has nothing metered on this account yet", "Command Code ne mesure encore rien sur ce compte"),
    (
        "Command Code rejected its sign-in; sign in again in the Command Code app",
        "Command Code a refusé la connexion ; reconnectez-vous dans l'app Command Code",
    ),
    ("Command Code is rate limited; retrying shortly", "Command Code est limité ; nouvel essai sous peu"),
    ("Command Code read failed ({e})", "Échec de la lecture Command Code ({e})"),
    // --- GLM / z.ai ---
    (
        "ZCode keeps its z.ai key encrypted, which CodeNotch does not read: set GLM_API_KEY or add glm.json",
        "ZCode chiffre sa clé z.ai, que CodeNotch ne lit pas : définissez GLM_API_KEY ou ajoutez glm.json",
    ),
    (
        "No usable Z.ai key found (ZCode, OpenCode or a glm.json key file)",
        "Aucune clé Z.ai utilisable trouvée (ZCode, OpenCode ou un fichier de clé glm.json)",
    ),
    ("Z.ai rejected the key — renew it in the tool that holds it", "Z.ai a refusé la clé — renouvelez-la dans l'outil qui la détient"),
    ("Z.ai answered with unreadable JSON", "Z.ai a répondu avec un JSON illisible"),
    ("Z.ai monitor refused the request ({code})", "Le moniteur Z.ai a refusé la requête ({code})"),
    ("The plan reported no usage windows", "L'offre n'a signalé aucune fenêtre d'usage"),
    // --- Antigravity ---
    ("Open Antigravity to read its quota", "Ouvrez Antigravity pour lire son quota"),
    ("Antigravity is closed — last reading kept", "Antigravity est fermé — dernier relevé conservé"),
    ("via Antigravity CLI — last reading kept", "via la CLI Antigravity — dernier relevé conservé"),
    ("Antigravity CLI read skipped (CODENOTCH_NO_SPAWN=1)", "Lecture par la CLI Antigravity ignorée (CODENOTCH_NO_SPAWN=1)"),
    (
        "Antigravity CLI reads are limited to one every 5 minutes",
        "Les lectures par la CLI Antigravity sont limitées à une toutes les 5 minutes",
    ),
    ("the CLI did not return a quota report", "la CLI n'a pas renvoyé de rapport de quota"),
    ("the CLI returned no recognised quota windows", "la CLI n'a renvoyé aucune fenêtre de quota reconnue"),
    // --- Qoder ---
    (
        "Qoder needs its usage sidecar: put qoder-usage.cjs in the qoder-sdk folder of the CodeNotch data folder and run npm install @qoder-ai/qoder-agent-sdk there",
        "Qoder a besoin de son sidecar d'usage : placez qoder-usage.cjs dans le dossier qoder-sdk du dossier de données CodeNotch et lancez-y npm install @qoder-ai/qoder-agent-sdk",
    ),
    ("Qoder needs Node.js: install it or set CODENOTCH_QODER_NODE", "Qoder a besoin de Node.js : installez-le ou définissez CODENOTCH_QODER_NODE"),
    ("Qoder read skipped (CODENOTCH_NO_SPAWN=1)", "Lecture Qoder ignorée (CODENOTCH_NO_SPAWN=1)"),
    ("the Qoder sidecar printed no JSON", "le sidecar Qoder n'a affiché aucun JSON"),
    ("qodercli is not signed in — run qodercli login", "qodercli n'est pas connecté — lancez qodercli login"),
    (
        "The Qoder SDK is not installed next to the sidecar — run npm install @qoder-ai/qoder-agent-sdk in the qoder-sdk folder",
        "Le SDK Qoder n'est pas installé à côté du sidecar — lancez npm install @qoder-ai/qoder-agent-sdk dans le dossier qoder-sdk",
    ),
    ("getUsageInfo returned nothing — sign in to qodercli", "getUsageInfo n'a rien renvoyé — connectez-vous à qodercli"),
    ("Qoder sidecar error ({code})", "Erreur du sidecar Qoder ({code})"),
    ("Qoder sidecar exited with {code}", "Le sidecar Qoder s'est arrêté avec le code {code}"),
    ("Qoder sidecar: {e}", "Sidecar Qoder : {e}"),
    ("{n} {unit} left", "{n} {unit} restants"),
    ("quota exceeded", "quota dépassé"),
    ("{note} — the plan publishes no quota bucket to meter", "{note} — l'offre ne publie aucun quota à mesurer"),
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
        use crate::{antigravity, commandcode, copilot, cursor, glm, opencode, qoder};
        let fixed = [
            cursor::NOTE_SIGN_IN,
            cursor::NOTE_REJECTED,
            cursor::NOTE_RATE_LIMITED,
            cursor::NOTE_BAD_JSON,
            cursor::NOTE_UNDECRYPTABLE,
            cursor::LABEL_ON_DEMAND,
            copilot::NOTE_SIGN_IN,
            copilot::NOTE_NO_SPAWN,
            copilot::NOTE_REJECTED,
            copilot::NOTE_RATE_LIMITED,
            copilot::NOTE_BAD_JSON,
            copilot::NOTE_NO_SNAPSHOTS,
            copilot::NOTE_NOTHING_METERED,
            opencode::NOTE_SIGN_IN,
            opencode::NO_GO,
            opencode::NOTE_EXPIRED,
            opencode::NOTE_OAUTH_REJECTED,
            opencode::NOTE_KEY_REJECTED,
            opencode::NOTE_NO_WINDOWS,
            opencode::LABEL_ROLLING,
            commandcode::NOTE_SIGN_IN,
            commandcode::NOTE_BILLING,
            commandcode::NOTE_NOTHING_METERED,
            commandcode::NOTE_REJECTED,
            commandcode::NOTE_RATE_LIMITED,
            commandcode::LABEL_FIVE_HOUR,
            glm::NOTE_SEALED,
            glm::NOTE_NO_KEY,
            glm::NOTE_REJECTED,
            glm::NOTE_BAD_JSON,
            glm::NOTE_NO_WINDOWS,
            glm::LABEL_WEEKLY,
            glm::LABEL_MCP,
            glm::LABEL_USAGE,
            antigravity::NOTE_OPEN_APP,
            antigravity::NOTE_CLOSED,
            antigravity::NOTE_CLI_KEPT,
            antigravity::NOTE_NO_SPAWN,
            antigravity::NOTE_CLI_SPACING,
            antigravity::ERR_NO_REPORT,
            antigravity::ERR_NO_WINDOWS,
            antigravity::LABEL_REQUESTS,
            "Requests today",
            "Weekly Limit",
            "5-hour Limit",
            qoder::NOTE_NO_SIDECAR,
            qoder::NOTE_NO_NODE,
            qoder::NOTE_NO_SPAWN,
            qoder::ERR_NO_JSON,
            qoder::NOTE_NEEDS_AUTH,
            qoder::NOTE_SDK_MISSING,
            qoder::NOTE_NOTHING,
            qoder::NOTE_EXCEEDED,
            qoder::LABEL_PLAN,
            qoder::LABEL_TOTAL,
            qoder::LABEL_ADDON,
            qoder::LABEL_ORG,
            "Chat requests",
            "Completions",
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
            crate::util::TPL_RATE_LIMITED,
            crate::util::TPL_LIVE_FAILED,
            crate::util::TPL_LIVE_HTTP,
            cursor::TPL_UNLIMITED,
            cursor::TPL_NOTHING,
            commandcode::TPL_FAILED,
            glm::TPL_REFUSED,
            qoder::TPL_SIDECAR,
            qoder::TPL_EXITED,
            qoder::TPL_SIDECAR_ERROR,
            qoder::TPL_LEFT,
            qoder::TPL_NO_BUCKET,
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
            tf(Lang::En, "Live read failed ({e})", &[("e", "boom")]),
            "Live read failed (boom)"
        );
        assert_eq!(
            tf(Lang::Fr, "Live read failed ({e})", &[("e", "boom")]),
            "Échec de la lecture en direct (boom)"
        );
        assert_eq!(
            t(Lang::Fr, "Not installed"),
            "Non installé",
            "core fallback"
        );
    }
}
