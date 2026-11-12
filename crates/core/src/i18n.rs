//! Translations. Keys ARE the English strings; `tr(Lang::En, k) == k`.
//!
//! * `tr(lang, "Weekly limit")` → "Limite hebdomadaire" in French.
//! * Placeholders use `{name}` and are filled by [`trf`].
//! * A key without a French entry falls back to English (never panics, never empty).
//! * Crates may ship their own French strings without editing this file: keep a
//!   `static FR: &[(&str, &str)]` in your crate and either call [`tr_in`] or register it once at
//!   start-up with [`register_fr`].

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Lang {
    #[default]
    En,
    Fr,
}

impl Lang {
    /// "fr", "fr-FR", "fr_CA" … → Fr; anything else (including "auto") → En.
    /// (Resolving "auto" from the OS locale is the platform crate's job.)
    pub fn from_code(code: &str) -> Lang {
        let c = code.trim().to_ascii_lowercase();
        if c == "fr" || c.starts_with("fr-") || c.starts_with("fr_") {
            Lang::Fr
        } else {
            Lang::En
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Fr => "fr",
        }
    }
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Fr];
    /// Name of the language in itself.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Fr => "Français",
        }
    }
}

/// Translate an English key.
pub fn tr(lang: Lang, key: &str) -> &str {
    match lang {
        Lang::En => key,
        Lang::Fr => lookup_fr(key).unwrap_or(key),
    }
}

/// Translate a window label (`LimitWindow::label_key`). Like [`tr`], plus labels built at fetch
/// time around a model name — "Weekly limit (Fable)" — through their template key
/// "Weekly limit ({model})" when the exact label has no entry.
pub fn tr_label(lang: Lang, key: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if lang == Lang::En {
        return Cow::Borrowed(key);
    }
    if let Some(v) = lookup_fr(key) {
        return Cow::Borrowed(v);
    }
    if let Some((base, rest)) = key.rsplit_once(" (") {
        if let Some(model) = rest.strip_suffix(')').filter(|m| !m.is_empty()) {
            let template = format!("{base} ({{model}})");
            if let Some(v) = lookup_fr(&template) {
                return Cow::Owned(fill(v, &[("model", model)]));
            }
        }
    }
    Cow::Borrowed(key)
}

/// Translate with a crate-local table consulted first.
pub fn tr_in<'a>(
    lang: Lang,
    key: &'a str,
    local_fr: &'static [(&'static str, &'static str)],
) -> &'a str {
    if lang == Lang::Fr {
        if let Some((_, fr)) = local_fr.iter().find(|(k, _)| *k == key) {
            return fr;
        }
    }
    tr(lang, key)
}

/// Translate and fill `{name}` placeholders: `trf(lang, "Resets in {m} min", &[("m", "5")])`.
pub fn trf(lang: Lang, key: &str, args: &[(&str, &str)]) -> String {
    fill(tr(lang, key), args)
}

/// Fill `{name}` placeholders in an already translated string.
pub fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

static REGISTERED: RwLock<Vec<&'static [(&'static str, &'static str)]>> = RwLock::new(Vec::new());

/// Add a French table for [`tr`] (call once at start-up; later tables win over earlier ones,
/// the core table wins over all).
pub fn register_fr(table: &'static [(&'static str, &'static str)]) {
    if let Ok(mut v) = REGISTERED.write() {
        if !v.iter().any(|t| std::ptr::eq(*t, table)) {
            v.push(table);
        }
    }
}

/// OS UI language used for `language: "auto"` (0 = En, 1 = Fr).
static SYSTEM_LANG: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Set the language `"auto"` resolves to (the app calls this once at start-up with
/// `codenotch_platform::system_lang()`). Default: English.
pub fn set_system_lang(lang: Lang) {
    SYSTEM_LANG.store(
        u8::from(lang == Lang::Fr),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// The language `"auto"` resolves to (see [`set_system_lang`]).
pub fn system_lang() -> Lang {
    if SYSTEM_LANG.load(std::sync::atomic::Ordering::Relaxed) == 1 {
        Lang::Fr
    } else {
        Lang::En
    }
}

fn core_fr() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| FR.iter().copied().collect())
}

fn lookup_fr(key: &str) -> Option<&'static str> {
    if let Some(v) = core_fr().get(key) {
        return Some(v);
    }
    let regs = REGISTERED.read().ok()?;
    regs.iter()
        .rev()
        .find_map(|t| t.iter().find(|(k, _)| *k == key).map(|(_, v)| *v))
}

/// The core French table. Grouped by screen; keep keys unique (a test checks).
pub static FR: &[(&str, &str)] = &[
    // --- window labels (LimitWindow::label_key) ---
    ("Current session", "Session en cours"),
    ("Weekly limit", "Limite hebdomadaire"),
    ("Weekly limit (Opus)", "Limite hebdomadaire (Opus)"),
    ("Weekly limit (Sonnet)", "Limite hebdomadaire (Sonnet)"),
    ("Weekly limit ({model})", "Limite hebdomadaire ({model})"),
    ("Monthly limit", "Limite mensuelle"),
    ("5-Hour Limit", "Limite 5 h"),
    ("Daily limit", "Limite quotidienne"),
    ("Spend limit", "Limite de dépenses"),
    ("Credits", "Crédits"),
    ("Premium requests", "Requêtes premium"),
    ("Included usage", "Usage inclus"),
    ("API usage", "Usage API"),
    ("Quota", "Quota"),
    // --- statuses ---
    (
        "Waiting for first reading…",
        "En attente du premier relevé…",
    ),
    (
        "Waiting for the first reading…",
        "En attente du premier relevé…",
    ),
    ("last reading is old", "dernier relevé ancien"),
    ("not signed in", "non connecté"),
    ("not installed", "non installé"),
    ("Not installed", "Non installé"),
    ("nothing to report", "rien à signaler"),
    ("not reachable", "injoignable"),
    (
        "rate limited, trying again soon",
        "limité, nouvel essai bientôt",
    ),
    (
        "Rate limited, trying again soon",
        "Limité, nouvel essai bientôt",
    ),
    (
        "Last known usage — not current.",
        "Dernière consommation connue — pas à jour.",
    ),
    ("Last reported reset", "Dernière réinitialisation annoncée"),
    ("Updated {age}", "Mis à jour {age}"),
    // --- sign-in hints (ProviderKind::sign_in_hint) ---
    (
        "Sign in to Claude Code to see usage.",
        "Connectez-vous à Claude Code pour voir votre consommation.",
    ),
    (
        "Sign in to Codex to see usage.",
        "Connectez-vous à Codex pour voir votre consommation.",
    ),
    (
        "Sign in to Cursor to see usage.",
        "Connectez-vous à Cursor pour voir votre consommation.",
    ),
    (
        "Run grok login to see usage.",
        "Lancez grok login pour voir votre consommation.",
    ),
    (
        "Run gh auth login to see usage.",
        "Lancez gh auth login pour voir votre consommation.",
    ),
    (
        "Run opencode auth login to see usage.",
        "Lancez opencode auth login pour voir votre consommation.",
    ),
    (
        "Sign in with the Command Code app to read your usage.",
        "Connectez-vous avec l'app Command Code pour lire votre consommation.",
    ),
    (
        "Sign in to Antigravity to see usage.",
        "Connectez-vous à Antigravity pour voir votre consommation.",
    ),
    (
        "Sign in to qodercli to see usage.",
        "Connectez-vous à qodercli pour voir votre consommation.",
    ),
    (
        "Add your z.ai API key to see usage.",
        "Ajoutez votre clé API z.ai pour voir votre consommation.",
    ),
    (
        "Check the endpoint settings.",
        "Vérifiez les réglages du point d'accès.",
    ),
    // --- card ---
    ("{name} Usage", "Consommation {name}"),
    (
        "{used}% used · {left}% left",
        "{used} % utilisés · {left} % restants",
    ),
    (
        "{left}% left · {used}% used",
        "{left} % restants · {used} % utilisés",
    ),
    ("{v}% deficit", "{v} % de déficit"),
    ("~{n} requests today", "~{n} requêtes aujourd'hui"),
    ("~1 request today", "~1 requête aujourd'hui"),
    ("no requests today", "aucune requête aujourd'hui"),
    ("and {n} more", "et {n} de plus"),
    ("Sign in", "Se connecter"),
    ("Signing in...", "Connexion…"),
    ("Refresh", "Actualiser"),
    ("Working", "En cours"),
    ("Waiting", "En attente"),
    ("needs your input", "attend votre réponse"),
    ("Done", "Terminé"),
    ("Idle", "Inactif"),
    // --- time formats (format.rs) ---
    ("Resetting…", "Réinitialisation…"),
    ("Resets in {m} min", "Réinit. dans {m} min"),
    ("Resets at {t}", "Réinit. à {t}"),
    ("Resets {d} {t}", "Réinit. {d} {t}"),
    ("Resets {date}", "Réinit. le {date}"),
    ("in {d}", "dans {d}"),
    ("on {date}", "le {date}"),
    ("at {t}", "à {t}"),
    ("around {t}", "vers {t}"),
    ("now", "maintenant"),
    // --- forecast (format.rs) ---
    ("At this pace: {what}", "À ce rythme : {what}"),
    ("runs out {when}", "épuisé {when}"),
    ("≈ {p}% at reset", "≈ {p} % au reset"),
    ("{text} (estimate)", "{text} (estimation)"),
    ("just now", "à l'instant"),
    ("{d} ago", "il y a {d}"),
    // --- tray / menus ---
    ("Refresh all", "Tout actualiser"),
    ("Refresh now", "Actualiser maintenant"),
    ("Settings…", "Réglages…"),
    ("Quit CodeNotch", "Quitter CodeNotch"),
    ("Open {host}", "Ouvrir {host}"),
    ("Keep open", "Garder ouverte"),
    // --- reset card ---
    ("{provider} renewed", "{provider} renouvelé"),
    ("{window} renewed", "{window} renouvelée"),
    ("Quota available · {n}%", "Quota disponible · {n} %"),
    ("Dismiss", "Fermer"),
    // --- alerts ---
    ("{name}: {window} at {n}%", "{name} : {window} à {n} %"),
    (
        "{name}: {window} will run out in about {m} min",
        "{name} : {window} sera épuisée dans environ {m} min",
    ),
    ("Warning", "Attention"),
    ("Critical", "Critique"),
    (
        "Test message from CodeNotch",
        "Message de test de CodeNotch",
    ),
    ("Test message sent", "Message de test envoyé"),
    // --- settings: navigation & sections ---
    ("Settings", "Réglages"),
    ("Accounts", "Comptes"),
    ("Appearance", "Apparence"),
    ("General", "Général"),
    ("Notifications", "Notifications"),
    ("Analytics", "Statistiques"),
    ("Close", "Fermer"),
    ("Connected", "Connectés"),
    ("Not connected", "Non connectés"),
    ("Taskbar", "Barre des tâches"),
    ("Notch", "Encoche"),
    ("Indicators", "Indicateurs"),
    ("Usage Limits", "Limites d'usage"),
    ("App", "Application"),
    ("Saved", "Enregistré"),
    // --- settings: controls ---
    ("Show", "Affichage"),
    ("Always show", "Toujours afficher"),
    ("Show on hover", "Afficher au survol"),
    ("Hide", "Masquer"),
    (
        "Fold for full-screen apps",
        "Replier pour les apps plein écran",
    ),
    ("Adaptive pill", "Pilule adaptative"),
    ("Size", "Taille"),
    ("Small", "Petite"),
    ("Medium", "Moyenne"),
    ("Large", "Grande"),
    ("Theme", "Thème"),
    ("System", "Système"),
    ("Light", "Clair"),
    ("Dark", "Sombre"),
    ("Weekly ring", "Anneau hebdomadaire"),
    ("Off", "Désactivé"),
    ("Inside", "Intérieur"),
    ("Outside", "Extérieur"),
    ("Dashed weekly ring", "Anneau hebdomadaire en pointillés"),
    (
        "Show remaining instead of used",
        "Afficher le restant plutôt que l'utilisé",
    ),
    ("Show usage pace", "Afficher le rythme de consommation"),
    ("Edge", "Bord"),
    ("Left", "Gauche"),
    ("Right", "Droite"),
    ("Top", "Haut"),
    ("Bottom", "Bas"),
    ("Screen", "Écran"),
    ("Main", "Principal"),
    ("Recentre", "Recentrer"),
    (
        "The notch is back in the middle",
        "L'encoche est revenue au centre",
    ),
    ("Show rings in", "Afficher les anneaux dans"),
    ("Taskbar, left", "Barre des tâches, à gauche"),
    ("Taskbar, right", "Barre des tâches, à droite"),
    ("Colour transition", "Transition de couleur"),
    ("Hard step", "Palier"),
    ("Colour ramp", "Dégradé"),
    ("Watch limit", "Seuil d'attention"),
    ("Critical limit", "Seuil critique"),
    ("Reset to defaults", "Valeurs par défaut"),
    ("Language", "Langue"),
    ("Follow system", "Suivre le système"),
    ("Refresh every", "Actualiser toutes les"),
    ("Game mode", "Mode jeu"),
    (
        "Open CodeNotch at login",
        "Ouvrir CodeNotch à l'ouverture de session",
    ),
    ("Global shortcut", "Raccourci global"),
    (
        "Tray shows usage",
        "L'icône de la barre affiche la consommation",
    ),
    ("Version", "Version"),
    ("Updates", "Mises à jour"),
    ("Check for updates", "Rechercher des mises à jour"),
    ("Update", "Mettre à jour"),
    ("Download installer", "Télécharger l'installateur"),
    ("Checking…", "Vérification…"),
    ("Installing…", "Installation…"),
    ("Up to date", "À jour"),
    ("Reset notifications", "Notifications de réinitialisation"),
    ("Reset cards", "Cartes de réinitialisation"),
    ("Notification sound", "Son de notification"),
    (
        "Preview the reset card",
        "Aperçu de la carte de réinitialisation",
    ),
    ("Preview card", "Aperçu"),
    ("Desktop notifications", "Notifications du bureau"),
    ("Forecast alerts", "Alertes de prévision"),
    (
        "Alerts in full screen when critical",
        "Alertes en plein écran si critique",
    ),
    ("Telegram", "Telegram"),
    ("Discord", "Discord"),
    ("Chat id", "Identifiant de discussion"),
    ("Send a test", "Envoyer un test"),
    ("Data folder", "Dossier des données"),
    ("Open folder", "Ouvrir le dossier"),
    ("Add a Claude account", "Ajouter un compte Claude"),
    ("Add…", "Ajouter…"),
    ("Add an account", "Ajouter un compte"),
    ("Remove", "Supprimer"),
    ("Name", "Nom"),
    ("Profile folder", "Dossier du profil"),
    ("Endpoint URL", "URL du point d'accès"),
    (
        "Token environment variable",
        "Variable d'environnement du jeton",
    ),
    (
        "Let Claude Code notify CodeNotch",
        "Laisser Claude Code prévenir CodeNotch",
    ),
    ("Install hooks", "Installer les hooks"),
    ("Remove hooks", "Retirer les hooks"),
    ("Notch reads", "L'encoche lit"),
    ("Model data", "Données des modèles"),
    ("Automatic", "Automatique"),
    (
        "At least one provider stays on the notch",
        "Au moins un fournisseur reste sur l'encoche",
    ),
    // --- onboarding ---
    ("Welcome to CodeNotch", "Bienvenue dans CodeNotch"),
    (
        "We found these accounts on this PC.",
        "Nous avons trouvé ces comptes sur ce PC.",
    ),
    (
        "No account found yet.",
        "Aucun compte trouvé pour l'instant.",
    ),
    ("Continue", "Continuer"),
    ("Skip", "Passer"),
    // --- analytics ---
    ("Tokens", "Jetons"),
    ("Cost", "Coût"),
    ("Input", "Entrée"),
    ("Output", "Sortie"),
    ("Cache read", "Lecture cache"),
    ("Cache write", "Écriture cache"),
    ("Last {n} days", "{n} derniers jours"),
    ("By model", "Par modèle"),
    ("By day", "Par jour"),
    ("No data yet", "Pas encore de données"),
    // --- misc ---
    ("Quit", "Quitter"),
    ("Cancel", "Annuler"),
    ("Save", "Enregistrer"),
    ("Error", "Erreur"),
    ("Unavailable", "Indisponible"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_identity_and_french_translates() {
        assert_eq!(tr(Lang::En, "Weekly limit"), "Weekly limit");
        assert_eq!(tr(Lang::Fr, "Weekly limit"), "Limite hebdomadaire");
        assert_eq!(tr(Lang::Fr, "no such key"), "no such key");
    }

    #[test]
    fn placeholders() {
        assert_eq!(
            trf(Lang::Fr, "Resets in {m} min", &[("m", "5")]),
            "Réinit. dans 5 min"
        );
        assert_eq!(
            trf(Lang::En, "{name} Usage", &[("name", "Claude")]),
            "Claude Usage"
        );
    }

    #[test]
    fn table_has_unique_non_empty_keys_and_same_placeholders() {
        let mut seen = std::collections::HashSet::new();
        for (k, v) in FR {
            assert!(seen.insert(*k), "duplicate key {k}");
            assert!(!v.is_empty(), "empty value for {k}");
            let ph = |s: &str| {
                let mut v: Vec<String> = s
                    .split('{')
                    .skip(1)
                    .filter_map(|p| p.split('}').next().map(String::from))
                    .collect();
                v.sort();
                v
            };
            assert_eq!(ph(k), ph(v), "placeholders differ for {k}");
        }
    }

    #[test]
    fn model_scoped_labels() {
        assert_eq!(
            tr_label(Lang::Fr, "Weekly limit (Fable)"),
            "Limite hebdomadaire (Fable)"
        );
        assert_eq!(
            tr_label(Lang::Fr, "Weekly limit (Opus)"),
            "Limite hebdomadaire (Opus)"
        );
        assert_eq!(tr_label(Lang::Fr, "Weekly limit"), "Limite hebdomadaire");
        assert_eq!(
            tr_label(Lang::En, "Weekly limit (Fable)"),
            "Weekly limit (Fable)"
        );
        // no template for this prefix: unchanged
        assert_eq!(tr_label(Lang::Fr, "Extra thing (x)"), "Extra thing (x)");
    }

    #[test]
    fn lang_codes() {
        assert_eq!(Lang::from_code("fr"), Lang::Fr);
        assert_eq!(Lang::from_code("FR-ca"), Lang::Fr);
        assert_eq!(Lang::from_code("auto"), Lang::En);
        assert_eq!(Lang::Fr.code(), "fr");
    }

    #[test]
    fn local_and_registered_tables() {
        static LOCAL: &[(&str, &str)] = &[("Only here", "Seulement ici")];
        assert_eq!(tr_in(Lang::Fr, "Only here", LOCAL), "Seulement ici");
        assert_eq!(tr_in(Lang::En, "Only here", LOCAL), "Only here");
        static REG: &[(&str, &str)] = &[("Registered key", "Clé enregistrée")];
        register_fr(REG);
        assert_eq!(tr(Lang::Fr, "Registered key"), "Clé enregistrée");
    }
}
