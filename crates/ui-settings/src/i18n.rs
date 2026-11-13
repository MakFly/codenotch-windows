//! Strings of this crate. Keys are English; French comes from the crate-local [`FR`] table,
//! then from core's table (`codenotch_core::i18n::FR`), then falls back to English.
//! Neutral, Windows-style wording (infinitive / noun labels, no "tu").

use codenotch_core::i18n::{fill, tr_in};
use codenotch_core::Lang;

/// Translate an English key.
pub fn t(lang: Lang, key: &str) -> &str {
    tr_in(lang, key, FR)
}

/// Translate and fill `{name}` placeholders.
pub fn tf(lang: Lang, key: &str, args: &[(&str, &str)]) -> String {
    fill(t(lang, key), args)
}

/// French strings specific to the settings window, onboarding and analytics.
pub static FR: &[(&str, &str)] = &[
    // --- window & navigation ---
    ("CodeNotch Settings", "Réglages de CodeNotch"),
    ("Quit CodeNotch", "Quitter CodeNotch"),
    ("Saved", "Enregistré"),
    ("Order saved", "Ordre enregistré"),
    ("Undo", "Annuler"),
    ("Remove account…", "Supprimer le compte…"),
    ("Remove “{name}”?", "Supprimer « {name} » ?"),
    (
        "{name} is removed from CodeNotch only. Its sign-in, its files and its profile folder stay on this PC, untouched by CodeNotch.",
        "{name} est retiré de CodeNotch uniquement. Sa connexion, ses fichiers et son dossier de profil restent sur ce PC : CodeNotch n'y touche pas.",
    ),
    (
        "A detected account is not added back automatically: find it under Removed accounts to add it again.",
        "Un compte détecté n'est pas rajouté automatiquement : il reste disponible dans Comptes retirés pour le rajouter.",
    ),
    ("Account removed", "Compte supprimé"),
    ("Account restored", "Compte rétabli"),
    ("Removed accounts", "Comptes retirés"),
    ("Add back", "Rajouter"),
    ("Not detected again automatically.", "Plus détecté automatiquement."),
    ("The last account on the notch cannot be removed", "Le dernier compte de l'encoche ne peut pas être supprimé"),
    ("Refreshing statistics…", "Actualisation des statistiques…"),
    ("Statistics updated", "Statistiques actualisées"),
    ("Toggle sidebar (Ctrl+B)", "Afficher ou masquer la barre latérale (Ctrl+B)"),
    // --- refusals / validation ---
    ("This account no longer exists", "Ce compte n'existe plus"),
    ("Only custom endpoints can be removed", "Seuls les points d'accès personnalisés peuvent être supprimés"),
    ("Enter a name", "Saisir un nom"),
    ("Enter a number", "Saisir un nombre"),
    ("Value out of range", "Valeur hors limites"),
    ("Use an https:// address (http only for localhost)", "Utiliser une adresse https:// (http uniquement pour localhost)"),
    ("Use a format like Ctrl+Alt+N", "Utiliser un format comme Ctrl+Alt+N"),
    ("A chat id is a number (or @channel)", "Un identifiant de discussion est un nombre (ou @canal)"),
    ("A bot token looks like 123456:ABC…", "Un jeton de bot ressemble à 123456:ABC…"),
    ("Paste the https:// webhook URL", "Coller l'URL https:// du webhook"),
    // --- statuses ---
    ("Turned off", "Désactivé"),
    ("Screen {n}", "Écran {n}"),
    ("Not checked yet", "Pas encore vérifié"),
    ("checked {age}", "vérifié {age}"),
    ("Version {v} is available", "La version {v} est disponible"),
    ("Version {v} is required", "La version {v} est obligatoire"),
    ("Downloading version {v}… {n}%", "Téléchargement de la version {v}… {n} %"),
    ("Version {v} is ready to install", "La version {v} est prête à être installée"),
    ("Update failed: {msg}", "Échec de la mise à jour : {msg}"),
    ("hidden from the notch", "masqué sur l'encoche"),
    ("Hooks installed", "Hooks installés"),
    ("Hooks not installed", "Hooks non installés"),
    // --- accounts page ---
    (
        "Enabled accounts appear on the notch in this order. Drag a card by its handle on the left, or use Move up / Move down in its … menu, to reorder them. Accounts of one provider stay together.",
        "Les comptes activés apparaissent sur l'encoche dans cet ordre. Faire glisser une carte par sa poignée à gauche, ou utiliser Monter / Descendre dans son menu …, pour les réordonner. Les comptes d'un même fournisseur restent groupés.",
    ),
    ("Hide from notch", "Masquer sur l'encoche"),
    ("Show on notch", "Afficher sur l'encoche"),
    ("Rename", "Renommer"),
    ("More options", "Plus d'options"),
    ("Move up", "Monter"),
    ("Move down", "Descendre"),
    ("Gemini models", "Modèles Gemini"),
    ("Claude and GPT models", "Modèles Claude et GPT"),
    (
        "Shows when Claude Code is working or waiting for you. Adds small hooks to Claude Code's settings file.",
        "Indique quand Claude Code travaille ou attend une réponse. Ajoute de petits hooks au fichier de réglages de Claude Code.",
    ),
    ("Signs in a second Claude profile in the browser.", "Connecte un second profil Claude dans le navigateur."),
    ("Add a custom endpoint", "Ajouter un point d'accès personnalisé"),
    (
        "A JSON usage endpoint of your own (team proxy, gateway).",
        "Un point d'accès JSON de consommation (proxy d'équipe, passerelle).",
    ),
    ("Add", "Ajouter"),
    ("Bearer token", "Jeton d'accès"),
    ("Bearer token (optional)", "Jeton d'accès (facultatif)"),
    (
        "The token is kept in Windows Credential Manager, never in the config file.",
        "Le jeton est conservé dans le Gestionnaire d'identification de Windows, jamais dans le fichier de configuration.",
    ),
    // --- appearance page ---
    (
        "In the taskbar the rings use its free space: hover one for its card, click to refresh, right-click for Settings. They hide when there is no room, the taskbar is hidden or an app is full screen.",
        "Dans la barre des tâches, les anneaux occupent l'espace libre : survoler un anneau pour sa carte, cliquer pour actualiser, clic droit pour les réglages. Ils se masquent faute de place, quand la barre est masquée ou qu'une app est en plein écran.",
    ),
    ("The pill stays open on the edge of the screen.", "La pilule reste ouverte au bord de l'écran."),
    (
        "A slim tab rests on the edge and opens when the pointer touches it.",
        "Un fin onglet reste au bord et s'ouvre au contact du pointeur.",
    ),
    (
        "No pill. The tray icon stays, and reset cards still appear.",
        "Pas de pilule. L'icône de la barre reste et les cartes de réinitialisation s'affichent toujours.",
    ),
    (
        "Folds the pill at once while a full-screen app is in front. Hovering still opens it.",
        "Replie aussitôt la pilule quand une app plein écran est au premier plan. Le survol l'ouvre toujours.",
    ),
    (
        "The folded pill turns light over dark backgrounds and dark over light ones.",
        "La pilule repliée devient claire sur fond sombre et sombre sur fond clair.",
    ),
    ("Hides the notch and keeps only critical alerts.", "Masque l'encoche et ne garde que les alertes critiques."),
    ("A second, thinner ring for the weekly limit.", "Un second anneau, plus fin, pour la limite hebdomadaire."),
    (
        "Figures and rings count what is left. Colours still follow what is used.",
        "Les chiffres et les anneaux comptent le restant. Les couleurs suivent toujours l'utilisé.",
    ),
    ("Shows how far ahead of an even pace you are.", "Indique l'avance prise sur un rythme régulier."),
    ("Show the forecast", "Afficher la prévision"),
    (
        "Estimates from your recent activity when each limit will run out, or how much will be used at the reset.",
        "Estime d'après votre activité récente quand chaque limite sera épuisée, ou la part consommée à la réinitialisation.",
    ),
    ("{label}: {forecast}", "{label} : {forecast}"),
    ("Position", "Position"),
    (
        "Hold Alt and drag the notch, or use its grip, to move it along the edges.",
        "Maintenir Alt et faire glisser l'encoche, ou utiliser sa poignée, pour la déplacer le long des bords.",
    ),
    (
        "Rings jump from green to amber at the watch limit, then to red at the critical limit.",
        "Les anneaux passent du vert à l'ambre au seuil d'attention, puis au rouge au seuil critique.",
    ),
    (
        "Rings blend from green to amber up to the watch limit, then towards red at 100 %.",
        "Les anneaux passent progressivement du vert à l'ambre jusqu'au seuil d'attention, puis vers le rouge à 100 %.",
    ),
    ("Watch {w} %, critical {c} %", "Attention {w} %, critique {c} %"),
    ("Reset", "Réinitialiser"),
    // --- notifications page ---
    ("Alerts", "Alertes"),
    (
        "Alerts when a limit crosses the watch or critical threshold.",
        "Alerte quand une limite franchit le seuil d'attention ou le seuil critique.",
    ),
    (
        "Lets critical alerts through while a full-screen app or game is in front.",
        "Laisse passer les alertes critiques quand une app plein écran ou un jeu est au premier plan.",
    ),
    (
        "Warns when a limit is projected to run out soon at the current pace.",
        "Prévient quand une limite risque d'être épuisée bientôt au rythme actuel.",
    ),
    ("Warn ahead by", "Prévenir à l'avance de"),
    ("Resets", "Réinitialisations"),
    ("A desktop notification when a limit renews.", "Une notification du bureau quand une limite se renouvelle."),
    ("A card next to the notch when a limit renews.", "Une carte près de l'encoche quand une limite se renouvelle."),
    ("Send alerts to Telegram", "Envoyer les alertes sur Telegram"),
    ("Send alerts to Discord", "Envoyer les alertes sur Discord"),
    ("Bot token", "Jeton du bot"),
    ("Webhook URL", "URL du webhook"),
    ("Paste the bot token", "Coller le jeton du bot"),
    ("Paste the webhook URL", "Coller l'URL du webhook"),
    ("Not set", "Non défini"),
    ("Test", "Tester"),
    (
        "Turn it on and set the chat id and the bot token first.",
        "Activer l'envoi et renseigner d'abord l'identifiant de discussion et le jeton du bot.",
    ),
    ("Turn it on and set the webhook first.", "Activer l'envoi et renseigner d'abord le webhook."),
    (
        "Tokens and webhooks are kept in Windows Credential Manager (or the CODENOTCH_TELEGRAM_TOKEN / CODENOTCH_DISCORD_WEBHOOK variables), never in the config file.",
        "Les jetons et webhooks sont conservés dans le Gestionnaire d'identification de Windows (ou les variables CODENOTCH_TELEGRAM_TOKEN / CODENOTCH_DISCORD_WEBHOOK), jamais dans le fichier de configuration.",
    ),
    // --- analytics page ---
    ("Reading the local transcripts…", "Lecture des transcriptions locales…"),
    (
        "Press Refresh to read the Claude Code and Codex transcripts on this PC. Nothing leaves your computer.",
        "Cliquer sur Actualiser pour lire les transcriptions Claude Code et Codex de ce PC. Rien ne quitte l'ordinateur.",
    ),
    ("No usage in the last {n} days.", "Aucune consommation sur les {n} derniers jours."),
    (
        "These figures cover the last {n} days. Press Refresh to recompute.",
        "Ces chiffres portent sur les {n} derniers jours. Cliquer sur Actualiser pour recalculer.",
    ),
    ("API-equivalent cost", "Coût équivalent API"),
    (
        "{n} tokens from models without a known price are not in the costs.",
        "{n} jetons de modèles sans tarif connu ne sont pas comptés dans les coûts.",
    ),
    (
        "×N compares the API-equivalent cost with your plan price over the same period (above ×1, the plan pays off).",
        "×N compare le coût équivalent API au prix de l'abonnement sur la même période (au-delà de ×1, l'abonnement est rentable).",
    ),
    ("no price", "sans tarif"),
    ("Analytics could not be computed: {msg}", "Statistiques impossibles à calculer : {msg}"),
    ("{tokens} tokens", "{tokens} jetons"),
    ("Plans", "Abonnements"),
    // --- statistics page (KPIs, chart, heatmap, by model, hours) ---
    ("All providers", "Tous les fournisseurs"),
    ("All models", "Tous les modèles"),
    ("Press Refresh to load the detailed statistics.", "Cliquer sur Actualiser pour charger les statistiques détaillées."),
    ("vs previous period", "vs période précédente"),
    ("Trending up this period", "En hausse cette période"),
    ("Trending down this period", "En baisse cette période"),
    ("Stable this period", "Stable sur la période"),
    ("vs the previous {n} days", "vs les {n} jours précédents"),
    ("≥: {n} tokens have no price", "≥ car {n} jetons sans tarif"),
    ("At public API prices", "Au tarif public de l'API"),
    ("Input, output and cache", "Entrées, sorties et cache"),
    ("{eur} per month", "{eur} par mois"),
    ("At the pace of the last {n} days", "Au rythme des {n} derniers jours"),
    ("The plan pays off", "L'abonnement est rentabilisé"),
    ("Below the plan price", "Sous le prix de l'abonnement"),
    ("No plan price yet", "Aucun prix d'abonnement"),
    ("1 day", "1 jour"),
    ("{n} days", "{n} jours"),
    ("Cache hit rate", "Taux de cache"),
    ("Share of the prompt read from the cache", "Part du contexte lue depuis le cache"),
    ("Sessions", "Sessions"),
    ("{tokens} tokens / session", "{tokens} jetons / session"),
    ("Cost per active day", "Coût par jour actif"),
    ("{n} active days out of {total}", "{n} jours actifs sur {total}"),
    ("Pace", "Rythme"),
    ("{tokens} / h", "{tokens} / h"),
    ("{n} active time slots", "{n} créneaux actifs"),
    ("Monthly projection", "Projection mensuelle"),
    ("at the pace of the period", "au rythme de la période"),
    ("Plan value", "Rentabilité abonnement"),
    ("API-equivalent cost vs plan price", "Coût équivalent API / prix de l'abonnement"),
    ("API-equivalent cost vs list price (≈)", "Coût équivalent API / prix catalogue (≈)"),
    ("Plans are per provider", "Les abonnements sont par fournisseur"),
    ("Set a plan price below", "Renseigner un prix d'abonnement ci-dessous"),
    ("Streak", "Série"),
    ("Record: {n}", "Record : {n}"),
    ("Busiest day", "Jour le plus chargé"),
    ("peak around {h}:00", "pointe vers {h} h"),
    ("Evolution", "Évolution"),
    ("Others", "Autres"),
    ("Total", "Total"),
    ("Sessions per day", "Sessions par jour"),
    ("API-equivalent cost per day, by provider", "Coût équivalent API par jour, par fournisseur"),
    ("API-equivalent cost per day, by model", "Coût équivalent API par jour, par modèle"),
    ("Tokens per day, by provider", "Jetons par jour, par fournisseur"),
    ("Tokens per day, by model", "Jetons par jour, par modèle"),
    ("Activity", "Activité"),
    ("Tokens per day over the last 12 months", "Jetons par jour sur les 12 derniers mois"),
    ("Tokens per day over the last {n} weeks", "Jetons par jour sur les {n} dernières semaines"),
    ("No activity", "Aucune activité"),
    ("Less", "Moins"),
    ("More", "Plus"),
    ("{n} active days in the last year", "{n} jours actifs sur l'année"),
    ("Share of the tokens of the period", "Part des jetons de la période"),
    ("{n} other models · {tokens} tokens", "{n} autres modèles · {tokens} jetons"),
    ("No usage for this filter.", "Aucune consommation pour ce filtre."),
    ("Activity hours", "Heures d'activité"),
    ("Busiest on {day} · peak around {h}:00", "Jour le plus actif : {day} · pointe vers {h} h"),
    ("Tokens by weekday and hour over the period", "Jetons par jour de la semaine et par heure sur la période"),
    ("1 US dollar in euros", "1 dollar US en euros"),
    ("Used to show costs in euros.", "Sert à afficher les coûts en euros."),
    ("{n} accounts", "{n} comptes"),
    ("€ / month", "€ / mois"),
    (
        "Monthly price of each plan, to compare with the API-equivalent cost. Leave empty to use the public list price when there is one, enter 0 to leave a plan out.",
        "Prix mensuel de chaque abonnement, à comparer au coût équivalent API. Laisser vide pour utiliser le prix catalogue public quand il existe, saisir 0 pour exclure un abonnement.",
    ),
    (
        "List price {usd} / month (≈ {eur}), used while the field is empty",
        "Prix catalogue {usd} / mois (≈ {eur}), utilisé tant que le champ est vide",
    ),
    ("Left out of the comparison (0)", "Exclu de la comparaison (0)"),
    (
        "≈: plans without a price of yours use their public list price ({date}). Enter 0 to leave a plan out.",
        "≈ : les abonnements sans prix saisi utilisent leur prix catalogue public ({date}). Saisir 0 pour exclure un abonnement.",
    ),
    // --- general page ---
    ("The tray icon tooltip lists the percentages.", "L'info-bulle de l'icône de la barre affiche les pourcentages."),
    (
        "Shows or hides the notch. Example: Ctrl+Alt+N. Leave empty for none.",
        "Affiche ou masque l'encoche. Exemple : Ctrl+Alt+N. Laisser vide pour aucun.",
    ),
    ("Keep history for", "Conserver l'historique"),
    ("New versions", "Nouvelles versions"),
    ("Propose", "Proposer"),
    ("Install automatically", "Installer automatiquement"),
    (
        "Propose: a notification offers each new version. Install automatically: downloaded in the background and installed when CodeNotch is idle. Off: only when you check.",
        "Proposer : une notification propose chaque nouvelle version. Installer automatiquement : téléchargée en arrière-plan et installée quand CodeNotch est inactif. Désactivé : seulement quand vous vérifiez.",
    ),
    ("Install update", "Installer la mise à jour"),
    ("Update address", "Adresse des mises à jour"),
    (
        "Advanced. The update.json file CodeNotch reads. Leave empty for the official one.",
        "Avancé. Le fichier update.json lu par CodeNotch. Laisser vide pour l'adresse officielle.",
    ),
    ("Use an https:// address", "Utiliser une adresse https://"),
    ("Version {v}", "Version {v}"),
    ("Data", "Données"),
    ("Settings, history and logs.", "Réglages, historique et journaux."),
    ("Design after Codenotch by @hivinz_ (vinzdg/codenotch).", "Design inspiré de Codenotch par @hivinz_ (vinzdg/codenotch)."),
    ("Provider marks from lobe-icons (MIT licence).", "Logos des fournisseurs : lobe-icons (licence MIT)."),
    // --- onboarding ---
    (
        "CodeNotch shows how much of your AI subscriptions you have used, in a small notch on the edge of the screen.",
        "CodeNotch affiche la consommation de vos abonnements IA dans une petite encoche au bord de l'écran.",
    ),
    (
        "Sign in to Claude Code, Codex or another supported tool, then add it in Settings › Accounts.",
        "Se connecter à Claude Code, Codex ou un autre outil pris en charge, puis l'ajouter dans Réglages › Comptes.",
    ),
    ("Hold Alt and drag the notch to move it later.", "Maintenir Alt et faire glisser l'encoche pour la déplacer ensuite."),
    ("Step {n} of {total}", "Étape {n} sur {total}"),
    ("Back", "Retour"),
    ("Finish", "Terminer"),
    // --- Teams ---
    (
        "Teams",
        "Équipe",
    ),
    (
        "Paste your invite link",
        "Collez votre lien d'invitation",
    ),
    (
        "Not connected to a team",
        "Non connecté à une équipe",
    ),
    (
        "Connected to {team}",
        "Connecté à {team}",
    ),
    (
        "{n} events waiting",
        "{n} événements en attente",
    ),
    (
        "Sync paused, retrying",
        "Synchronisation en pause, nouvel essai",
    ),
    (
        "Device revoked by the team",
        "Appareil révoqué par l'équipe",
    ),
    (
        "The team changed what it sees. Review and accept to resume.",
        "L'équipe a changé ce qu'elle voit. Relisez et acceptez pour reprendre.",
    ),
    (
        "Cannot save Teams sync data",
        "Impossible d'enregistrer les données de synchronisation d'équipe",
    ),
    (
        "Update CodeNotch to keep syncing",
        "Mettez CodeNotch à jour pour continuer la synchronisation",
    ),
    (
        "This server is not a CodeNotch Teams server",
        "Ce serveur n'est pas un serveur CodeNotch Teams",
    ),
    (
        "Teams sync could not start",
        "La synchronisation d'équipe n'a pas pu démarrer",
    ),
    (
        "The Teams server address changed. Detach this device to pair again.",
        "L'adresse du serveur Teams a changé. Détachez cet appareil pour l'appairer de nouveau.",
    ),
    (
        "Tool, plan, quota percentages and reset times",
        "Outil, forfait, pourcentages de quota et heures de réinitialisation",
    ),
    (
        "Token counts per model and hour",
        "Nombre de tokens par modèle et par heure",
    ),
    (
        "An account reference made on this PC, the app version and queue counters",
        "Une référence de compte créée sur ce PC, la version de l'app et les compteurs de file d'attente",
    ),
    (
        "Prompts, code and model responses",
        "Prompts, code et réponses des modèles",
    ),
    (
        "File paths, folder and repository names",
        "Chemins de fichiers, noms de dossiers et de dépôts",
    ),
    (
        "Sign-in e-mail and provider tokens",
        "E-mail de connexion et jetons des fournisseurs",
    ),
    (
        "Accounts you do not share, and every other tool",
        "Les comptes que vous ne partagez pas, et tous les autres outils",
    ),
    (
        "What leaves this PC",
        "Ce qui quitte ce PC",
    ),
    (
        "What never leaves this PC",
        "Ce qui ne quitte jamais ce PC",
    ),
    (
        "Team sync",
        "Synchronisation d'équipe",
    ),
    (
        "Shares quota and token figures with your team's CodeNotch Teams server. Off by default.",
        "Partage les quotas et le nombre de tokens avec le serveur CodeNotch Teams de votre équipe. Désactivé par défaut.",
    ),
    (
        "Status",
        "État",
    ),
    (
        "Server address",
        "Adresse du serveur",
    ),
    (
        "Your team's server, for example https://teams.example.com. Changing it requires pairing again.",
        "Le serveur de votre équipe, par exemple https://teams.example.com. La changer impose d'appairer de nouveau.",
    ),
    (
        "Last sent: {when}",
        "Dernier envoi : {when}",
    ),
    (
        "Nothing sent yet",
        "Rien d'envoyé pour l'instant",
    ),
    (
        "Send now",
        "Envoyer maintenant",
    ),
    (
        "Include the 45 days before pairing",
        "Inclure les 45 jours avant l'appairage",
    ),
    (
        "A separate agreement: your token counts of the 45 days before pairing are sent too. Turn it off to withdraw it.",
        "Un accord distinct : vos nombres de tokens des 45 jours avant l'appairage sont aussi envoyés. Désactivez-le pour le retirer.",
    ),
    (
        "Connect",
        "Se connecter",
    ),
    (
        "Invite link",
        "Lien d'invitation",
    ),
    (
        "Sent by your team admin. It only goes to the server address above.",
        "Envoyé par l'administrateur de votre équipe. Il n'est transmis qu'à l'adresse du serveur ci-dessus.",
    ),
    (
        "Your team admin sees your usage by name",
        "L'administrateur de votre équipe voit votre usage à votre nom",
    ),
    (
        "Visibility: {v}",
        "Visibilité : {v}",
    ),
    (
        "Server: {origin}",
        "Serveur : {origin}",
    ),
    (
        "Accept and connect",
        "Accepter et se connecter",
    ),
    (
        "Join a team",
        "Rejoindre une équipe",
    ),
    (
        "Privacy",
        "Confidentialité",
    ),
    (
        "Shared accounts",
        "Comptes partagés",
    ),
    (
        "Only Claude and Codex accounts can be shared",
        "Seuls les comptes Claude et Codex peuvent être partagés",
    ),
    (
        "This device",
        "Cet appareil",
    ),
    (
        "No account to share",
        "Aucun compte à partager",
    ),
    (
        "My company",
        "Mon entreprise",
    ),
    (
        "Me, not reimbursed",
        "Moi, sans remboursement",
    ),
    (
        "Declare",
        "Déclarer",
    ),
    (
        "Monthly price you pay.",
        "Prix mensuel que vous payez.",
    ),
    (
        "Your team sees this amount but cannot change it",
        "Votre équipe voit ce montant mais ne peut pas le modifier",
    ),
    (
        "Who pays for this plan?",
        "Qui paie ce forfait ?",
    ),
    (
        "Detach this device",
        "Détacher cet appareil",
    ),
    (
        "This device stops syncing. Its pairing, its queue and its device token are deleted from this PC.",
        "Cet appareil arrête de synchroniser. Son appairage, sa file d'attente et son jeton sont supprimés de ce PC.",
    ),
    (
        "Detach",
        "Détacher",
    ),
    (
        "Delete my data on the server",
        "Supprimer mes données sur le serveur",
    ),
    (
        "Asks the team server to erase everything you sent. Sync goes on from now.",
        "Demande au serveur de l'équipe d'effacer tout ce que vous avez envoyé. La synchronisation continue à partir de maintenant.",
    ),
    (
        "Delete",
        "Supprimer",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

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
    fn table_is_clean() {
        let mut seen = std::collections::HashSet::new();
        for (k, v) in FR {
            assert!(seen.insert(*k), "duplicate key {k}");
            assert!(!v.trim().is_empty(), "empty value for {k}");
            assert_eq!(
                placeholders(k),
                placeholders(v),
                "placeholders differ for {k}"
            );
        }
    }

    #[test]
    fn local_table_wins_and_english_is_identity() {
        assert_eq!(t(Lang::En, "Turned off"), "Turned off");
        assert_eq!(t(Lang::Fr, "Turned off"), "Désactivé");
        // Falls through to core's table.
        assert_eq!(t(Lang::Fr, "Weekly limit"), "Limite hebdomadaire");
        assert_eq!(tf(Lang::Fr, "Screen {n}", &[("n", "2")]), "Écran 2");
    }

    /// Every literal passed to `t(lang, "…")` / `tf(lang, "…", …)` in this crate has a French
    /// entry (here or in core), so a French user never sees an English string in French mode.
    #[test]
    fn every_ui_string_has_french() {
        let sources = [
            include_str!("logic.rs"),
            include_str!("view.rs"),
            include_str!("pages.rs"),
            include_str!("pages_stats.rs"),
            include_str!("pages_teams.rs"),
            include_str!("stats.rs"),
            include_str!("lib.rs"),
        ];
        let core: std::collections::HashSet<&str> =
            codenotch_core::i18n::FR.iter().map(|(k, _)| *k).collect();
        let local: std::collections::HashSet<&str> = FR.iter().map(|(k, _)| *k).collect();
        // Strings that are the same in both languages.
        let same: &[&str] = &[
            "Telegram",
            "Discord",
            "Version",
            "Notifications",
            "English",
            "Français",
            "Antigravity",
            "OK",
        ];
        let mut missing = Vec::new();
        let mut calls = 0;
        for src in sources {
            // rustfmt may split a call over several lines: compare without whitespace runs
            // between `(`, `lang,` and the literal.
            let src = src.replace("(\n", "(").replace(",\n", ", ");
            let src: String = {
                let mut out = String::with_capacity(src.len());
                let mut prev_space = false;
                let mut in_lit = false;
                for c in src.chars() {
                    if c == '"' {
                        in_lit = !in_lit;
                    }
                    let space = c == ' ' && !in_lit;
                    if !(space && prev_space) {
                        out.push(c);
                    }
                    prev_space = space;
                }
                out.replace("( lang", "(lang")
            };
            for marker in ["t(lang, \"", "tf(lang, \""] {
                let mut rest = src.as_str();
                while let Some(pos) = rest.find(marker) {
                    calls += 1;
                    // Skip identifiers that merely end in `t` / `tf` (e.g. `format(lang, "`).
                    let prev = rest[..pos].chars().last();
                    rest = &rest[pos + marker.len()..];
                    if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let Some(end) = rest.find('"') else { break };
                    let key = &rest[..end];
                    if !core.contains(key) && !local.contains(key) && !same.contains(&key) {
                        missing.push(key.to_string());
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(calls > 150, "scanner found only {calls} calls");
        assert!(missing.is_empty(), "no French for: {missing:#?}");
    }

    /// Keys passed through variables (page titles, refusal and validation messages).
    #[test]
    fn computed_keys_have_french() {
        use crate::logic::{FieldError, Refusal};
        use crate::Page;
        let mut keys: Vec<&str> = [
            Page::Onboarding,
            Page::Accounts,
            Page::Appearance,
            Page::Notifications,
            Page::Analytics,
            Page::Teams,
            Page::General,
        ]
        .into_iter()
        .map(crate::view::page_title)
        .collect();
        keys.extend(
            [
                Refusal::LastNotchAccount,
                Refusal::UnknownAccount,
                Refusal::EmptyName,
            ]
            .map(Refusal::message),
        );
        keys.extend(
            [
                FieldError::NotANumber,
                FieldError::OutOfRange,
                FieldError::Url,
                FieldError::HotkeyFormat,
                FieldError::ChatId,
                FieldError::TelegramToken,
                FieldError::DiscordWebhook,
                FieldError::EmptyName,
                FieldError::HttpsOnly,
            ]
            .map(FieldError::message),
        );
        keys.extend(crate::stats::Metric::ALL.map(crate::stats::Metric::label));
        // Teams: status keys from the engine, labels passed through variables.
        keys.extend([
            "Sync paused, retrying",
            "Device revoked by the team",
            "The team changed what it sees. Review and accept to resume.",
            "Cannot save Teams sync data",
            "Update CodeNotch to keep syncing",
            "This server is not a CodeNotch Teams server",
            "Teams sync could not start",
            "The Teams server address changed. Detach this device to pair again.",
            "Detach this device",
            "This device stops syncing. Its pairing, its queue and its device token are deleted from this PC.",
            "Detach",
            "Delete my data on the server",
            "Asks the team server to erase everything you sent. Sync goes on from now.",
            "Delete",
        ]);
        let known = |k: &str| {
            FR.iter().any(|(e, _)| *e == k) || codenotch_core::i18n::FR.iter().any(|(e, _)| *e == k)
        };
        for key in keys {
            assert!(known(key), "no French for {key}");
        }
    }
}
