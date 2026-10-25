# Changelog

## 0.1.3 — 2026-10-25

- Maintenance release to exercise the built-in updater from 0.1.2: the update banner, the green “Update” button in the settings menu, the signed install and the restart. No functional change.

## 0.1.2 — 2026-10-25

- **Alerts before the limit**: pace forecast (“limit in ~25 min”, only when it lands before the reset), graded warning / critical / limit-reached alerts, “quota available again” after a reset, urgent alerts in full screen, faster readings near a limit, and an alert at launch when a window is already past a threshold.
- **Discord alerts** through a channel webhook: coloured embeds with gauge, expected limit and reset times, mentions disabled, webhook URL kept in Windows Credential Manager.
- Settings: **Appearance** and **Alerts** are now separate pages made of collapsible sections (one open at a time, remembered), with shadcn inputs; a green “Update x.y.z” button appears at the bottom of the menu when an update is available; opening the settings (tray, notch, relaunch, first run) brings the window to the front.
- **Telegram alerts** (built but switched off until validated; shown as “coming soon”) through your own bot: token stored in Windows Credential Manager, chat detection after `/start`, test message, and alert cards with gauge, forecast, reset time and a link to the provider usage page.

## 0.1.1 — 2026-10-25

- **Built-in updates**: CodeNotch checks GitHub Releases at launch and twice a day, notifies once per version, and installs the signed installer in one click (Tauri updater, signature and signed version verified).
- **Welcome screen** on first launch: the settings window opens by itself to connect ChatGPT, Claude, Gemini, Grok and OpenCode, with the language choice and a short guide. Existing installs without any account see it once; it can be shown again from Appearance and alerts.
- **Grok** (Grok Build CLI) and **OpenCode** (OpenCode Go) connectors, following T3 Code: detection, sign-in from Quick connect, and quota readings. Grok sessions are renewed through `grok models` without a prompt.
- **Quick connect**: sign in to Codex / ChatGPT, Claude and Gemini from Settings → Accounts, then the account is added automatically; each account dialog also offers **Sign in again**.
- Codex signs in in-app, without a terminal, through the Codex App Server ChatGPT OAuth (device code fallback). Claude and Gemini use their official CLI sign-in.
- Native Gemini connector (Gemini CLI Google session, internal Code Assist quota endpoint). It reads the session Gemini CLI saved and never refreshes it with Gemini CLI’s OAuth client; run Gemini CLI again when it expires.
- Much broader CLI detection: saved PATH, npm (custom prefix), pnpm, bun, Volta, Scoop, WinGet, Chocolatey, and the CLIs bundled with the Codex and Claude desktop apps. Honours `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `GEMINI_CLI_HOME`, and no longer duplicates an account whose profile path was left at the default.
- Vite no longer watches `src-tauri`, which crashed `bun run desktop` on Windows while Cargo was linking.

## 0.1.0 — 2026-10-24

First public Windows release.

- Floating edge notch with per-account quota rings and details on hover.
- Codex / ChatGPT, Claude Code, GitHub Copilot, Ollama and custom JSON connectors.
- Multiple accounts and local profile detection.
- Auto-collapse, four screen edges and saved monitor placement.
- Automatic hiding in full screen and manual game mode.
- Separate settings window with modal account configuration.
- English by default, with persistent English / French selection.
- Windows Credential Manager for secrets added in the app.
- Windows x64 installer for the current user, with shortcuts, uninstaller and WebView2 bootstrapper; portable download also available.
- Installer smoke-tested locally: install, launch and uninstall, with existing configuration preserved.
- Unsigned downloads; no automatic updater.

Subscription quotas and token counts depend on what each provider exposes.
Claude and Copilot use internal endpoints that may change.
