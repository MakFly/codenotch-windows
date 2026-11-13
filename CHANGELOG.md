# Changelog

## Unreleased

### Added

- Team sync (opt-in, off by default): pair this PC with a CodeNotch Teams server from the new
  Settings > Teams page (invite preview and consent, shared accounts, plan payer declaration,
  backfill agreement, send now, detach, erase on the server). Only quota figures and hourly token
  counts of the shared Claude and Codex accounts leave the PC, through one bounded background
  thread; nothing runs while it is off. New crates `teams-schema` and `teams`, settings keys
  `teamsSyncEnabled`, `teamsEndpoint`, `teamsExcludedAccounts`, `--settings-page=teams`, and a
  `teams:` line in `doctor`. The Teams page appears once a team server is configured (official
  origin compiled in or `teamsEndpoint` set); until then `--settings-page=teams` opens the default page.
- One-line PowerShell installer (`install.ps1`): `irm …/install.ps1 | iex` installs or updates to
  `%LOCALAPPDATA%\Programs\CodeNotch` after checking size and SHA-256, adds a Start Menu shortcut
  and starts the app; `CODENOTCH_UNINSTALL=1` removes it again (data kept).

## 0.1.0 — 2026-11-12

A complete rebuild from scratch, and the first release published from this repository. It
replaces the 0.3.0 reconstruction (kept as the first commit of the repository); version numbers
restart at 0.1.0 with the rebuild. Design and behaviour follow CodeNotch by Vinz (MIT); no code was copied
from it.

### Added

- Cargo workspace of 13 crates: core, providers, providers-extra, notify, analytics, activity,
  hook, platform, ui-kit, ui-notch, ui-settings, engine, app.
- Providers: Grok (read again), Cursor, GitHub Copilot, OpenCode (Go plan), Command Code, z.ai
  (GLM), Antigravity, Qoder, alongside Claude, Codex and the custom JSON endpoint.
- Several Claude profiles (`~\.claude-<n>`), "Add a Claude account", and in-app Claude sign-in
  through the Claude Code CLI.
- Automatic discovery of installed providers (adds accounts, never removes or renames).
- Notch: four edges, three sizes, show modes (always, hover, hidden), optional weekly ring,
  remaining-instead-of-used, usage pace, adaptive pill, colour ramp, drag to place, folding in full
  screen and game mode. Hover card per account. Reset cards with optional sound.
- Taskbar mode: rings drawn in the free space of the primary taskbar.
- One app theme (Light, Dark or System) for the notch, its hover card, the reset cards, the
  taskbar strip's card and the settings window. A change in Settings, or a Windows light/dark
  switch under System, applies at once. The taskbar strip's own rings and figures follow the
  taskbar's light/dark mode, for contrast. `notchAlwaysDark` is kept in the file but ignored.
- Settings window (Accounts, Appearance, Notifications, Analytics, General) and first-run
  onboarding. English and French. It opens at 1120 × 900, shrunk to fit smaller screens, and
  centered. Layout after the shadcn/ui `dashboard-01` block: sidebar on the window background
  (app identity, 32 px items, Quit at the bottom), collapsible to a 48 px icon rail with instant
  label tooltips (toggle button, Ctrl+B; 200 ms ease-out, instant with reduced motion; saved as
  `settingsSidebarCollapsed`; automatic under 900 px without touching the setting), and an inset
  content panel (8 px margin, 12 px radius, border) whose 48 px header bar holds the toggle, the
  page title, page actions and the caption buttons.
- Statistics page in the same style: period toggle group (90 / 30 / 7 days; a select when
  narrow), four KPI cards (API-equivalent cost, tokens, monthly projection, plan value) with a
  trend badge and an interpretation footer when the change vs the previous period is reliable,
  the secondary figures in one compact card, and the "Evolution" area chart with its metric as a
  toggle group (subtle y labels, grid at half the border colour). Refresh is in the header bar.
- Tray menu: "Refresh all", "Settings…", "Game mode" (check mark, toggles the switch),
  "Quit CodeNotch"; optional usage tooltip.
- Exhaustion forecast per quota window (hover card and Accounts page): run-out time or percent at
  reset, from the window's history over its whole period, the recent slope and, for Claude and
  Codex, the usual hour-of-week activity. `showForecast` switch (on by default). Samples of
  weekly and monthly windows are kept in `history.jsonl` until their period ends.
- Global shortcut to show or hide the notch. Open at login.
- Alerts: watch and critical thresholds, forecast alerts, reset alerts. Delivery as Windows
  notifications, to Telegram and to Discord, with test buttons. Secrets in Windows Credential
  Manager or in `CODENOTCH_TELEGRAM_TOKEN` / `CODENOTCH_DISCORD_WEBHOOK`.
- Token and cost analytics from local Claude Code and Codex transcripts, with an incremental index
  and an optional `pricing.json`.
- Claude Code hooks: `codenotch-hook.exe`, a loopback hook server with a token, and a merge-only
  installer for `settings.json`. Busy detection for Codex, Cursor, OpenCode and Antigravity.
- Self-update from a JSON manifest (`update.json` at the repository root by default;
  `updateManifestUrl` to use another address): `version`, `min_version`, `notes`, `notes_url`
  and the Windows zip's `url`, `sha256` and `size`. Checks 20 s after start, every 6 h and on
  demand. `updateMode`: `notify` (default; toast with **Update** / **Notes** and a Windows
  notification), `auto` (background download, install when idle or at the next start) or `off`
  (the button only); a version below `min_version` installs at once under a sticky toast. The zip
  is downloaded to `<data folder>\updates\`, refused unless its size and SHA-256 match, then the
  two exes are swapped in place (running ones renamed `.old`, everything rolled back on failure)
  and the new version restarts (`--updated-from`, `--wait-pid`) and cleans up. Never from a cargo
  `target\` folder unless `CODENOTCH_UPDATE_ALLOW_DEV=1`. `updateChecks` is migrated (`false` →
  `off`) and kept in sync. Settings → General shows the version, the last check, "Check for
  updates", "Install update", the mode and the address. `make update-manifest` writes
  `update.json` from `dist\`.
- A standalone toast window at the top centre of the notch's screen hosts the app's toasts while
  the settings window is closed (click-through around the toasts, gone when empty). Toasts can
  carry two buttons and be non-dismissable.
- CLI: `doctor`, `--demo`, `--once`, `--version`, `--help`, `--silent`, `--settings`.
- Environment variables `CODENOTCH_DATA_DIR`, `CODENOTCH_NO_SPAWN`, `CODENOTCH_NOTCH_NO_REGION`,
  `CODENOTCH_QODER_SIDECAR`, `CODENOTCH_QODER_NODE`, `CODENOTCH_UPDATE_URL`,
  `CODENOTCH_UPDATE_ALLOW_DEV`, `CODENOTCH_UPDATE_PRETEND_VERSION` (debug builds).
- `make dev`: watch mode that rebuilds and restarts the debug app on save (`scripts\dev.ps1`, no
  extra tool).
- Rotating log file in `<data folder>\logs\`, with redaction.
- GitHub Actions: CI (fmt and clippy on Windows; tests run locally with `make ci`) and a
  tag-triggered release workflow that builds the zip with `scripts\dist.ps1` (shared with
  `make dist`), its SHA-256 and `update.json`.
- Provider marks from lobe-icons (MIT), with their notice.
- Toasts in the manner of react-hot-toast (`ui-kit::toast`): top-centre stack (bottom-right
  possible), success / error / loading / custom, in-place update (loading → result), optional
  action button, timers paused on hover, spring enter and fade-out exit (fades only with reduced
  motion). The settings window uses them for saves, engine notices, statistics refreshes and
  reordering; any window can host the app's toaster.
- Accounts: "Remove account…" in each card's menu, behind a confirmation dialog, with a 6 s
  undo. Only CodeNotch forgets the account; a removed detected account is remembered in
  `removedAccounts` so discovery does not add it back, and "Removed accounts" adds it again.

### Changed since 0.3.0

- The settings window comes to the front when opened from the notch, the tray or a second
  launch, instead of only flashing in the taskbar (Windows foreground lock worked around).

- The single `gpui-app` crate is replaced by the workspace. The app package is now `codenotch` in
  `crates/app`.
- `config.gpui.json` keeps every 0.2/0.3 key with its name and meaning. Keys added by the
  rebuild are optional. Unknown keys survive a save. A bad value for a choice setting falls back
  to its default instead of failing the whole file. An unparsable file is copied to `config.gpui.unreadable.json`.
- `history.jsonl` keeps its format. Old localised labels are mapped to the current ones.
- A 0.3 file without `showMode` shows the pill on hover, like a fresh install (hidden when
  `widget` is `false`).
- `analytics-cache.json` keeps the format the 0.3 app left behind, with optional additions.
- New files in the data folder: `state.json`, `auth_token`, `logs\`.

### Not included

- An installer and code signing. Updates are trusted through the manifest's SHA-256 (served over
  https from the repository), not a signature.
- Sealed ZCode keys for GLM (`enc:v1:` values) are skipped, never decrypted.
- Antigravity's token in Windows Credential Manager is not read.

### Known unverified

- The GUI was checked in demo mode only (notch, hover card, settings pages, toasts). The tray,
  hotkey, taskbar mode and reset cards were not checked against real use.
- Read live against real accounts: Codex and Grok only. Claude was not read live (expired saved
  token, automatic renewal disabled during testing). Cursor's session was rejected by its API.
  Copilot's token read was skipped. OpenCode, Command Code, GLM, Antigravity and Qoder were not
  installed; they are covered by unit tests with simulated responses only.
- No real Telegram message, Discord message or Windows toast was sent.
- The hooks were never tried against a real Claude Code session.
- GPT-6-family Codex models have no built-in price; their cost is shown as a lower bound.

What was checked: the workspace builds, the tests pass, clippy is clean, `--version`, `doctor` and
`--once` run, and the analytics indexer ran on real transcripts.

## 0.3.0

Reconstruction after the 0.2.1 repository was lost.

- GPUI notch, settings, tray quit, and the existing `config.gpui.json`.
- Claude and Codex quota readings, plus custom JSON endpoints.
- Grok, Telegram, Discord, the updater, and in-app sign-in are not restored.

## 0.2.1 and earlier

The history of 0.2.1 and earlier versions was lost with the original repository.
