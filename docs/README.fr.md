# CodeNotch pour Windows

[English](../README.md) · [Télécharger la version 0.2.1](https://github.com/MakFly/codenotch-windows/releases/tag/v0.2.1)

Une encoche discrète pour suivre les quotas de vos comptes IA sur Windows.
Survolez un compte pour voir sa consommation et ses échéances de remise à
zéro. Les compteurs de tokens apparaissent lorsque le fournisseur les publie.

Adaptation Windows indépendante inspirée de
[CodeNotch de Vinz](https://github.com/vinzdg/codenotch), écrite en Rust avec
[GPUI](https://www.gpui.rs), le framework d'interface natif de l'éditeur Zed.
Depuis la 0.2.0, WebView2 n'est plus nécessaire. Ce projet n'est pas une
application officielle des fournisseurs concernés.

![Détail des quotas au survol](images/notch.png)

## Installer

1. Téléchargez `CodeNotch-0.2.1-windows-x64-setup.exe` depuis les releases.
2. Lancez l'installateur, puis CodeNotch depuis le menu Démarrer.
3. Survolez la languette au bord droit ; les réglages s'ouvrent depuis l'encoche ou l'icône près de l'horloge.
4. Pour le français : **Appearance → Interface language → Français**.

L'application s'installe pour votre compte Windows, avec raccourcis et
désinstallation depuis les applications installées de Windows.
Windows 10/11 x64 est requis.

**Vous avez la 0.1.x ?** Rien à faire : la mise à jour intégrée propose la nouvelle version,
l'installe par-dessus et conserve vos comptes et réglages.

Une version portable reste disponible : décompressez
`CodeNotch-0.2.1-windows-x64.zip` et lancez `CodeNotch.exe`.

L'installateur et l'application ne sont pas signés. SmartScreen peut afficher
une alerte et Smart App Control peut bloquer leur exécution selon la configuration
de sécurité. Voir les [explications de Microsoft](https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation).
Pas de démarrage automatique. Les mises à jour sont intégrées : CodeNotch vérifie
au lancement puis deux fois par jour, vérifie la signature du nouvel installateur
et l'installe en un clic depuis les réglages. Les empreintes des téléchargements
figurent dans `SHA256SUMS.txt`.

## Utiliser

- Codex / ChatGPT, Claude Code, Gemini, Copilot, Ollama et endpoints JSON personnalisés.
- Plusieurs profils par fournisseur ; détection locale des profils Codex, Claude et Gemini.
- Quatre bords possibles (le déplacement par la poignée n'est pas encore porté dans l'app native).
- Détails au survol et repli automatique au départ de la souris.
- Masquage en plein écran et mode jeu manuel pour les jeux en fenêtre.
- Réglages séparés, formulaires en modale et alertes de seuil (notifications Windows, Discord).
- Anglais par défaut, français disponible et choix enregistré.
- Page **Accueil** : la fenêtre principale de chaque compte, ses échéances et des raccourcis.
- Consommation CPU, RAM et GPU de l'ordinateur en temps réel dans l'en-tête des réglages.

![Page d’accueil des réglages](images/home.png)

Au premier lancement, un écran de bienvenue s’ouvre pour connecter vos abonnements.

![Écran de bienvenue](images/welcome.png)

Dans **Réglages → Comptes → Connexion rapide**, cliquez sur **Se connecter**
à côté de Codex / ChatGPT, Claude, Gemini, Grok ou OpenCode, puis terminez dans le navigateur.
Codex se connecte sans terminal : CodeNotch obtient la page OAuth ChatGPT
officielle via le Codex App Server et l'ouvre ; un code d'appareil s'affiche si
le port de retour est occupé. Claude, Gemini, Grok et OpenCode ouvrent leur connexion CLI
officielle dans une petite fenêtre, car Anthropic interdit aux applications
tierces de gérer elles-mêmes la connexion Claude.ai. CodeNotch détecte ensuite
la session et ajoute le compte. La détection couvre aussi PATH enregistré, npm,
pnpm, bun, Volta, Scoop, WinGet, Chocolatey et les copies fournies avec les
applications de bureau Codex et Claude. CodeNotch ne voit jamais le code
d'autorisation ni les jetons échangés.

Vous pouvez aussi vous connecter depuis les outils des fournisseurs. Une clé API ne donne
pas automatiquement accès aux quotas d'un abonnement. Une session Claude
expirée peut être renouvelée par le CLI installé sans envoyer de prompt ; si
cela échoue, utilisez **Reconnecter** ou `claude auth login`.

Les endpoints Claude, Gemini et Copilot sont internes et peuvent changer.
CodeNotch ne renouvelle jamais la session Gemini : relancez Gemini CLI quand elle expire. Ollama
affiche les modèles chargés, sans inventer de consommation de tokens.
Copilot, Ollama et les adaptateurs personnalisés nécessitent davantage de
validation sur différents comptes.

## Données et confidentialité

Un quota représente un pourcentage **consommé**, pas des tokens restants.
Une mesure absente reste un tiret ; une mesure ancienne est signalée.
Les relevés restent en mémoire pendant la session, pas entre lancements.
Les délais des réponses HTTP 429 sont respectés ; Claude est interrogé au
maximum une fois toutes les deux minutes.

Pas de serveur CodeNotch ni de télémétrie. Les sessions restent dans les
profils locaux des fournisseurs. Les secrets ajoutés utilisent le Gestionnaire
d'identification Windows, service `SmashToken`. Configuration :
`%APPDATA%\dev.kev.smashtoken\config.gpui.json` ; au premier lancement, la 0.2.0
repart du `config.json` des versions précédentes, sans jamais le modifier.

Fermer les réglages laisse le suivi actif. Pour quitter : icône CodeNotch près
de l'horloge → **Quitter**. N'incluez aucun secret dans un ticket public.

## Développer

Prérequis : Rust stable MSVC et Visual Studio Build Tools avec
« Développement Desktop en C++ ». L'application est dans `gpui-app/`.

```powershell
git clone https://github.com/MakFly/codenotch-windows.git
cd codenotch-windows\gpui-app
cargo run -- --settings
```

Binaire de release (`cargo build --release`) :
`gpui-app/target/release/codenotch-gpui.exe`.
La publication d'une release, le format des endpoints JSON personnalisés et les
instructions de contribution figurent dans le [README anglais](../README.md)
et [CONTRIBUTING.md](../CONTRIBUTING.md).

## Licence et crédits

[MIT](../LICENSE). Référence principale :
[CodeNotch de Vinz](https://github.com/vinzdg/codenotch).
[Coucou](https://github.com/Louis-CFM/coucou) a inspiré le choix initial de Tauri,
sans inclusion de son code ni de ses médias.
Voir [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).
