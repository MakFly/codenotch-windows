# CodeNotch for Windows

A small notch pinned to an edge of the Windows desktop. It shows, as rings, how much of each AI
subscription quota is used: Claude, Codex, Grok, Cursor, GitHub Copilot, OpenCode, Command Code,
z.ai (GLM), Antigravity, Qoder, and any custom JSON endpoint.

Version 1.0.0. Windows only. Written in Rust with [GPUI](https://crates.io/crates/gpui) 0.2.2.

## Origin

The owner's original repository, `MakFly/codenotch-windows` (last version 0.2.1), was deleted and
cannot be recovered. A small partial reconstruction, 0.3.0, existed for a short time; it is kept as
the git tag `backup-0.3.0`. Version 1.0.0 is a complete rebuild from scratch, done on 2026-11-12.
The visual design and behaviour follow [CodeNotch by Vinz](https://github.com/vinzdg/codenotch)
(MIT), including its Windows port. This project is a clean re-implementation in GPUI: no source
code was copied from it.

## Status

Read this section before relying on the app. 1.0.0 was assembled and tested without ever being
shown on screen.

What was checked:

- The workspace builds. `cargo test --workspace` passes 561 unit and integration tests. 6 more are
  ignored by default: three that read the real machine (config file, transcripts, live taskbar) and
  three documentation examples. `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- The headless modes `--version`, `doctor` and `--once` run.
- The settings file round trip (`config.gpui.json`) was checked against the owner's existing file
  with the ignored test `real_config_round_trips_if_present`: every key and value survives.
- The analytics indexer was run on real Claude Code and Codex transcripts.

Providers, as read against real accounts on the build machine:

| Provider | How it was verified |
|---|---|
| Codex | Live reading returned. |
| Grok | Live reading returned. |
| Claude | Not read live. The saved token was expired and automatic renewal was disabled during testing. Unit tests with simulated responses only. |
| Cursor | Tried live. Cursor's API rejected the session. Unit tests with simulated responses only. |
| GitHub Copilot | The token read was skipped. Unit tests with simulated responses only. |
| OpenCode | Not installed on the build machine. Unit tests with simulated responses only. |
| Command Code | Not installed on the build machine. Unit tests with simulated responses only. |
| z.ai (GLM) | Not installed on the build machine. Unit tests with simulated responses only. |
| Antigravity | Not installed on the build machine. Unit tests with simulated responses only. |
| Qoder | Not installed on the build machine. Unit tests with simulated responses only. |
| Custom JSON endpoint | Unit tests with simulated responses only. |

Not verified at all:

- **The GUI has never been shown on screen.** Notch rendering, the hover card, the settings window,
  the tray icon and menu, the global shortcut, taskbar mode, Windows notifications (toasts) and
  reset cards are unverified visually.
- No real Telegram message, Discord message or Windows toast was sent.
- The Claude Code hooks were never tried against a real Claude Code session.

Not included:

- An installer, and code signing. Self-update replaces the two exes in place, checked against the
  SHA-256 published in the update manifest (see [Updates](#updates)).
- Two credential paths that would require decrypting another product's vault: sealed ZCode keys
  for GLM (`enc:v1:` values are skipped) and Antigravity's token in Windows Credential Manager.

Analytics: GPT-6-family Codex models have no built-in price. Their cost is left out and the totals
are shown as a lower bound. Prices can be supplied in a [`pricing.json`](#pricingjson).

## Features

- A notch on the left, right, top or bottom edge of a chosen monitor, with one ring cell per
  account. Optional second ring for the weekly window. Small, medium or large.
- Three display modes: always open, a slim tab that opens on hover, or hidden (tray only).
- Hover card per account with every quota window, its reset time and the account's status.
- Settings window (Accounts, Appearance, Notifications, Analytics, General) and first-run
  onboarding. English and French. Its look follows the shadcn/ui `dashboard-01` block: a
  collapsible sidebar (icon rail with tooltips, Ctrl+B, automatic under 900 px) next to an inset
  content panel whose header bar carries the page title; the Statistics page has KPI cards with
  trend badges, a period toggle group and an interactive area chart.
- Tray icon with a short menu: "Refresh all", "Settings…", "Game mode" (a check mark shows the
  switch; a click toggles it) and "Quit CodeNotch". The tooltip can list the percentages.
- Forecast per quota window on the hover card and the Accounts page: when the window will run out
  at your current pace, or how much will be used at the reset. It uses the window's history over
  its whole period and, for Claude and Codex, your usual hour-by-hour activity from the local
  transcripts, so nights and weekends count for little. Switch: Appearance > "Show the forecast".
- Global shortcut (for example `Ctrl+Alt+N`) to show or hide the notch.
- Alerts when a window crosses the watch or critical threshold, forecast alerts when a window is
  projected to run out soon, and reset alerts when a window renews. Delivered as Windows
  notifications, to Telegram and to Discord.
- Reset cards: an on-screen card next to the notch when a window renews, with an optional sound.
- Token and cost analytics from local Claude Code and Codex transcripts.
- "Working" and "waiting on you" states from Claude Code hooks, plus busy detection for some other
  tools.
- Optional taskbar mode: the rings drawn in the free space of the Windows taskbar instead of the
  floating notch.
- Folds away while a full-screen app or a game is in front.
- Open at login (per-user, no admin rights).

## Requirements

- Windows. The build machine was Windows 11. Other versions were not tested.
- To build: Rust with the MSVC toolchain. 1.0.0 was built with Rust 1.99.1.
- Optional: GNU Make for Windows, for the `make` targets below.
- Each provider needs its own tool installed and signed in (see [Providers](#providers)). Qoder also
  needs Node.js.

## Build and run

The workspace has two executables: `codenotch.exe` (the app) and `codenotch-hook.exe` (the small
helper Claude Code calls). **`codenotch-hook.exe` must sit in the same folder as `codenotch.exe`.**
The app only looks for it there.

`cargo build` alone builds only the app (it is the workspace's default member). Build the whole
workspace, or name both packages:

```powershell
cargo build --workspace
cargo build --release -p codenotch -p codenotch-hook
```

Debug builds land in `target\debug\`, release builds in `target\release\`.

### First run: demo mode

Start with fake data first. Demo mode opens the notch on a built-in snapshot that shows every
state. It starts no engine, contacts no network and takes no single-instance lock. It still creates
the data folder and writes the log file.

```powershell
cargo build --workspace
target\debug\codenotch.exe --demo
```

### The real app

```powershell
target\debug\codenotch.exe
```

On the first start with no accounts, the onboarding opens inside the settings window. Providers
found on the machine are added as accounts automatically (existing accounts are never removed or
renamed by discovery).

A second launch does not start a second app: it asks the running one to open its settings and
exits.

To try the real data path without touching your real data folder, point `CODENOTCH_DATA_DIR` at
another folder (see [Environment variables](#environment-variables)).

### With make

A `Makefile` at the root wraps the same commands. It needs GNU Make 4.x for Windows, cargo and
PowerShell 5.1 or later. `make` alone prints the list of targets.

| Target | What it does |
|---|---|
| `make help` | List the targets and variables (default target). |
| `make build` | Debug build of the whole workspace (app and hook in `target\debug`). |
| `make release` | Release build of `codenotch.exe` and `codenotch-hook.exe`. |
| `make hook` | Debug build of `codenotch-hook.exe` only. |
| `make check` | `cargo check --workspace`. |
| `make demo` | Debug build, then `--demo`. |
| `make run` | Debug build, then start the real app. |
| `make dev` | Watch mode: build, start the debug app, then on every save under `crates\`, `Cargo.toml`, `Cargo.lock` or `.cargo\` stop it, rebuild and start it again. A failed build leaves it stopped until the next save. Refuses to start while a `codenotch.exe` from `target\` runs (`make stop`). Ctrl+C to stop. No extra tool needed. |
| `make run-release` | Release build, then start the real app. |
| `make settings` | Debug build, then start with the settings window open. |
| `make once` | Debug build, then `--once`. |
| `make once-safe` | Same as `once`, with `CODENOTCH_NO_SPAWN=1`. |
| `make doctor` | Debug build, then `doctor`. |
| `make version` | Debug build, then `--version`. |
| `make test` | `cargo test --workspace`. |
| `make test-crate CRATE=<package>` | `cargo test -p <package>`. |
| `make lint` | `cargo clippy --workspace --all-targets -- -D warnings`. |
| `make fmt` | `cargo fmt --all` (rewrites files). |
| `make fmt-check` | `cargo fmt --all -- --check`. |
| `make ci` | `fmt-check`, `lint` and `test`. |
| `make dist` | Release build, then `dist\CodeNotch-<version>\` and a `.zip` with both executables, `LICENSE`, `README.md` and the glyph `NOTICE.md`. |
| `make update-manifest` | Rewrite `update.json` from `dist\CodeNotch-<version>.zip` (version, size, SHA-256, release URLs, date). Run `make dist` first. See [Publishing a release](#publishing-a-release). |
| `make install` | Release build, then copy both executables to `%LOCALAPPDATA%\Programs\CodeNotch`. No shortcut, registry or autostart entry is created. |
| `make uninstall` | Remove the installed executables (and the folder if it is empty). The data folder, registry and autostart are not touched. |
| `make stop` | Kill every running `codenotch.exe`. A running exe locks its file and makes the next build fail to link. |
| `make logs` | Show the end of the log (`TAIL=50`, `FOLLOW=1` to keep following). |
| `make data-dir` | Print the data folder path. |
| `make clean` | `cargo clean` and remove `dist\`. |
| `make size` | Print the size of the two release executables. |
| `make loc` | Lines of Rust per crate. |

Variables: `ARGS=...` adds arguments to the run targets, `SAFE=1` sets `CODENOTCH_NO_SPAWN=1` for
them, `INSTALL_DIR=` changes the install folder, `CARGO=` the cargo executable.

```powershell
make demo
make dev ARGS=--demo SAFE=1
make once ARGS=--help
make test-crate CRATE=codenotch-core
```

## Command line

```text
codenotch [--settings | --demo | --once | doctor | --version]
```

| Argument | What it does |
|---|---|
| (none) | Start the notch. A second launch asks the first one to open its settings. |
| `--silent` | Same as no argument. This is what "Open at login" registers. |
| `--settings` | Start and open the settings window. |
| `--demo` | GUI on fake data. No engine, no network, no single-instance lock (the updater runs only when `CODENOTCH_UPDATE_URL` is set). |
| `--once` | Fetch every enabled account once, print one line each, exit. |
| `doctor` (or `--doctor`) | Print what the app sees on this machine, exit. |
| `--version` (or `-V`) | Print `codenotch 1.0.0`, exit. |
| `--help` (or `-h`) | Print the version and the usage line above, exit. |
| `--updated-from=<version>`, `--wait-pid=<pid>` | Set by the self-updater when it starts the new version: wait for the old process to exit, then show "Updated to …". Not for manual use. |

If several are given, the first match in this order wins: version, help, doctor, once, demo,
settings. Release builds have no console window; the CLI modes attach to the console they were
started from.

### `--once`

Prints one line per enabled account:

```text
<account name> (<provider>): <window> <percent> (<reset countdown>) · <window> <percent> ...
```

When a reading has no figures, the line carries the provider's note or error instead. A stale
reading gets its age in brackets at the end. With no enabled account it prints `no enabled
account`. Tokens are never printed; messages go through the same redaction as the log.

`--once` also runs provider discovery (the result is not saved), fetches in parallel and stops
after 130 seconds at most. It writes nothing to the data folder and opens no log file. It can start
helper programs (for example `codex.exe app-server`, `gh auth token`, the Antigravity CLI); set
`CODENOTCH_NO_SPAWN=1` to prevent that.

```powershell
target\release\codenotch.exe --once
```

### `doctor`

Read-only report. It creates nothing and prints presence only, never a secret value:

- version and executable path;
- data folder (and whether `CODENOTCH_DATA_DIR` set it);
- `config.gpui.json`: OK or unreadable, number of accounts, language, refresh period, onboarding;
- `state.json` present or absent, number of `history.jsonl` samples, log path;
- each provider: detected on this PC (file checks only) and number of configured accounts;
- each account: profile folder found or missing, credential file found (Claude, Codex), endpoint
  host (custom), token variable set or not and whether a token is stored in Credential Manager,
  last reading status and age, active back-off;
- Telegram token and Discord webhook: present or absent;
- hook helper present or missing next to the exe, hooks installed, Claude profile folders, hook
  port and token file;
- open at login, and the global shortcut.

```powershell
target\release\codenotch.exe doctor
```

## Where data lives

The data folder is `%APPDATA%\dev.kev.smashtoken`, the folder the 0.2 and 0.3 versions already
used. `CODENOTCH_DATA_DIR` replaces it when set and not empty.

| File | Written by | What it holds |
|---|---|---|
| `config.gpui.json` | app | Accounts and settings (see [Settings reference](#settings-reference)). Saved atomically (temp file, then rename). Unknown keys are kept. |
| `config.gpui.unreadable.json` | app | A copy of `config.gpui.json` made when it cannot be parsed. The app then starts on defaults. |
| `state.json` | app | Last reading per account, rate-limit back-off deadlines and alert bookkeeping (which alerts were already sent). |
| `history.jsonl` | app | One line per quota window per fresh reading: `{"a":"<account id>","l":"<window label>","t":<unix secs>,"u":<percent used>,"r":<reset time or null>}`. Pruned to `historyHours`, except samples of a window whose period has not ended yet (kept up to 32 days, thinned to one per 10 minutes) so weekly and monthly forecasts see their whole period. |
| `analytics-cache.json` | app | Incremental index of the transcripts: per file, bytes already read, model, working directory and hourly token counts. No message content. |
| `auth_token` | app | The hook server token: 64 hex characters, created when the hook server first starts. |
| `logs\codenotch.log` | app | The log. Rotated at 1 MiB to `codenotch.1.log` … `codenotch.3.log`. |
| `pricing.json` | you (optional) | Model price overrides for analytics. |
| `glm.json` | you (optional) | A z.ai key file (see [Providers](#providers)). |
| `glyphs\<provider id>.svg` or `.png` | you (optional) | Replaces the built-in provider mark. |
| `qoder-sdk\` | you (optional) | The Qoder sidecar and its Node SDK. |

For `glm.json` and `qoder-sdk\`, the 0.x folder `%APPDATA%\codenotch\` is also searched.

An existing `config.gpui.json` and `history.jsonl` from 0.2.x or 0.3.0 keep working. Every key
those versions wrote keeps its name and meaning, unknown keys survive a save, and a bad value for
a choice setting falls back to its default instead of failing the whole file. Old history lines
with localised labels ("5 h", "7 jours") are mapped to the current labels. A 0.3 file without
`showMode` shows the pill on hover, like a fresh install (or hides it when `widget` is `false`).

Outside the data folder, the app writes only these:

- Windows Credential Manager, generic credentials `CodeNotch/telegram-token`,
  `CodeNotch/discord-webhook` and `CodeNotch/account/<account id>`, when you save a secret in the
  settings window.
- `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `CodeNotch` = `"<exe>" --silent`,
  when "Open CodeNotch at login" is on. Turning it on also clears a "disabled" flag that Task
  Manager may have set for that value under `…\Explorer\StartupApproved\Run`.
- `HKCU\Software\Classes\AppUserModelId\dev.kev.CodeNotch`, registered on first use so Windows
  accepts the app's notifications.
- Claude Code's `settings.json`, only when you install or remove the hooks (see
  [Claude Code hooks](#claude-code-hooks)).
- `~\.claude-<n>\`, only when you use "Add a Claude account". The folder is removed again if the
  sign-in does not leave a credential.

## Providers

Most providers borrow a sign-in that the provider's own tool already holds. For the custom
endpoint, GitHub Copilot, OpenCode, Command Code and z.ai, an account can also carry its own token:
from the environment variable named in its `secretEnv`, else from the token stored for that account
in Credential Manager (`CodeNotch/account/<account id>`). In the table, "the account's token" means
that pair. The settings window stores such a token only in the "add a custom endpoint" form; for
the other providers, set `secretEnv` in `config.gpui.json` by hand.

| Provider | Credential read from | Request | What is shown |
|---|---|---|---|
| Claude | `<profile>\.credentials.json`, then `<profile>\credentials.json`. Profiles found: `~\.claude` and every `~\.claude-<id>` holding a credential; or the account's `profilePath`. | `GET https://api.anthropic.com/api/oauth/usage` | Current session, weekly limit, weekly per model (Opus, Sonnet, others), spend limit, extra usage. |
| Codex | `<profile>\auth.json` (`~\.codex`, or the account's `profilePath`). | `GET https://chatgpt.com/backend-api/wham/usage`. Fallbacks: a hidden `codex.exe app-server`, then the newest rollout file's last rate-limit snapshot. | Current session and weekly limit (labels follow the window length), plus Spark and Code review groups when present. |
| Grok | `~\.grok\auth.json`; only entries issued by `https://auth.x.ai` are ever sent. | `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits` | Credits per product. |
| Cursor | `%APPDATA%\Cursor\User\globalStorage\state.vscdb` (the editor's sign-in), opened read-only and re-read on every fetch. | `GET https://cursor.com/api/usage-summary` | Plan usage, API usage, on-demand spend. |
| GitHub Copilot | First found: the account's token, `GH_TOKEN`, `GITHUB_TOKEN`, `oauth_token` in GitHub CLI's `hosts.yml`, then `gh auth token --hostname github.com` run hidden. | `GET https://api.github.com/copilot_internal/user` | Premium requests and other metered quotas. Unlimited quotas draw nothing. |
| OpenCode | The account's token (used as a Go API key), else OpenCode's data folder (`$XDG_DATA_HOME\opencode`, then `~\.local\share\opencode`): `auth.json` and `opencode.db`, read-only. | `GET https://opencode.ai/zen/go/v1/usage` (Go API key) or `GET https://opencode.ai/inference/go/v1/usage` (OAuth). | OpenCode Go rolling (5-hour), weekly and monthly windows. |
| Command Code | The account's token, `COMMAND_CODE_API_KEY`, else `apiKey` in `~\.commandcode\auth.json`. | Four `GET`s under `https://api.commandcode.ai/alpha`. | Monthly credits, plus 5-hour and weekly windows when the plan has them. |
| z.ai (GLM) | First found: the account's token, `GLM_API_KEY`, `glm.json` in the data folder, Claude Code `~\.claude\settings.json` (only when its base URL is a z.ai or bigmodel.cn host), ZCode `~\.zcode\v2\config.json` and plaintext values of `credentials.json`, OpenCode `auth.json`. | `GET {https://api.z.ai or https://open.bigmodel.cn}/api/monitor/usage/quota/limit` | Session, weekly and monthly MCP windows. |
| Antigravity | No account token is read. The official CLI (`%LOCALAPPDATA%\agy\bin\agy.exe`) is run hidden with `/usage`; without it, the IDE's local language server on `https://127.0.0.1:<port>`, then the bundled `~\.gemini\bin\agy.exe`, then today's request count from local transcripts. | CLI, or loopback only. | 5-hour and weekly lanes for Gemini models or for Claude/GPT models (settings `antigravityLimit`, `antigravityModel`). The CLI runs at most once per 5 minutes, on the first fetch and on manual refresh. |
| Qoder | The signed-in `qodercli` session, through a Node sidecar that uses the official `@qoder-ai/qoder-agent-sdk`. Qoder Desktop's vault is never touched. | Made by the SDK, not by CodeNotch. | Plan usage, add-on credits, organisation package, total. |
| Custom | The account's token (optional). | `GET <endpoint>` | The windows the endpoint declares. |

### Read-only rule

CodeNotch never writes, refreshes or deletes another tool's credential files. SQLite databases are
opened read-only. Tokens never go into a reading, the log or the config file.

Two providers may start their own vendor CLI when a token is about to expire, and that CLI may
then renew its own file:

- Claude: when the token expires within 4 minutes, `claude -p` is run hidden (the standalone CLI
  only, not the desktop app's copy), at most every 10 minutes with a growing back-off, killed
  after 30 seconds.
- Grok: `grok models` is run hidden.

Codex's `auth.json` is never refreshed. No child process is started at all when
`CODENOTCH_NO_SPAWN=1`.

### Signing in

- Claude: the account's sign-in button opens a PowerShell window that runs
  `claude auth login --claudeai` for that profile. "Add a Claude account" creates the next free
  `~\.claude-<n>` folder and signs it in the same way. The standalone Claude Code CLI must be
  installed.
- Other providers: sign in with the provider's own tool (`grok login`, `gh auth login`,
  `opencode auth login`, `qodercli login`, the Cursor editor, the Command Code app). The card shows
  what to run when a provider is signed out.

### Qoder setup

The sidecar is not installed by the app. Copy `crates\providers-extra\assets\qoder-usage.cjs` into
`<data folder>\qoder-sdk\` and install the SDK there:

```powershell
cd "$env:APPDATA\dev.kev.smashtoken\qoder-sdk"
npm install @qoder-ai/qoder-agent-sdk
```

Sidecar search order: `CODENOTCH_QODER_SIDECAR`, `<data folder>\qoder-sdk\`,
`%APPDATA%\codenotch\qoder-sdk\`, next to the exe, `<exe folder>\resources\`. Node.js comes from
`CODENOTCH_QODER_NODE`, else `node.exe` on `PATH`.

### Custom JSON endpoint

The URL must use HTTPS. Plain HTTP is allowed only to localhost. The response:

```json
{
  "windows": [
    { "id": "quota", "label": "Monthly quota", "usedPercent": 42,
      "resetsAt": "2026-11-01T00:00:00Z", "windowSeconds": 2592000 }
  ]
}
```

- Used share: `usedPercent`, `used_percent` or `utilization` (a value up to 1 is read as a
  fraction), or `used` with `limit`.
- Reset: `resetsAt`, `resets_at`, `reset_at` or `resetAt`, as RFC 3339, unix seconds or
  milliseconds.
- Optional: `id` (default: derived from the label), `label` or `name`, `windowSeconds`, `count`,
  `detail`, `group`.
- A top-level array of windows is accepted too. A body without any usable window is an error.

The bearer token is optional. It comes from the environment variable named in the account's
`secretEnv`, else from the token typed in the "add a custom endpoint" form (stored in Credential
Manager). When `secretEnv` names a variable that is not set and nothing is stored, the reading is an
error that names the variable.

## Alerts

All alerts are governed by the `notifications` switch (Settings → Notifications → Desktop
notifications), except reset cards, which follow `resetCards` alone.

- **Threshold alerts**: a window crosses the watch threshold (`alertPercent`, default 80) or the
  critical threshold (`criticalPercent`, default 95) upward between two fresh readings. Never from a
  first, stale, restored, signed-out or failed reading. One alert per window, kind and reset period,
  remembered across restarts in `state.json`.
- **Forecast alerts** (`forecastAlerts`): the last hour of history projects the window to run out
  within `forecastMinutes` (default 30) and before its own reset.
- **Reset alerts** (`resetAlerts`) and **reset cards** (`resetCards`, `resetSound`): when a window
  renews.
- While a full-screen app or game is in front (or `gameMode` is on), on-screen output is held back,
  except critical alerts when `criticalInFullscreen` is on. Telegram and Discord are not affected by
  full screen.

### Telegram and Discord

Settings → Notifications has a Telegram section (switch, chat id, bot token, test button) and a
Discord section (switch, webhook URL, test button).

- The **bot token** and the **webhook URL** are stored in Windows Credential Manager
  (`CodeNotch/telegram-token`, `CodeNotch/discord-webhook`). The field is cleared after saving.
- Or set them as environment variables. A variable wins over the stored value.

| Secret | Environment variable |
|---|---|
| Telegram bot token | `CODENOTCH_TELEGRAM_TOKEN` |
| Discord webhook URL | `CODENOTCH_DISCORD_WEBHOOK` |

- The Telegram **chat id** is stored in `config.gpui.json` (`telegramChatId`). It is hidden from
  debug output and logs.
- Accepted formats: bot token `<digits>:<at least 20 letters, digits, _ or ->`; chat id a number
  (negative for groups) or `@channelname`; webhook
  `https://discord.com/api/webhooks/<id>/<token>` (also `discordapp.com`, `ptb.discord.com`,
  `canary.discord.com`, an optional `/vN` after `/api`).
- Each request has an 8-second timeout and one retry on a transient failure. Error messages never
  contain the token, chat id or webhook URL.

## Claude Code hooks

Hooks let Claude Code tell CodeNotch when a session is working or waiting for you. Install them in
Settings → Accounts → Claude Code → "Install hooks".

Installing:

- works on every Claude Code profile folder that exists: `~\.claude`, every `~\.claude-*`,
  `CLAUDE_CONFIG_DIR`, and the profile folder of any Claude account set in the config;
- merges entries into that folder's `settings.json` (created if missing) for the events
  `SessionStart`, `UserPromptSubmit`, `PreToolUse` (matcher `*`), `PostToolUse` (matcher `*`),
  `Notification`, `Stop` and `SessionEnd`. Each entry runs
  `"<folder of codenotch.exe>\codenotch-hook.exe" <event>` with a 5-second timeout;
- keeps everything else exactly as it was, including other tools' hooks on the same events;
- is idempotent: nothing is written when the entries are already there;
- writes atomically and, the first time it changes an existing file, keeps a copy as
  `settings.json.codenotch.bak` (never overwritten afterwards);
- never writes a file that is not valid JSON or whose `hooks` has an unexpected shape;
- fails with a notice when `codenotch-hook.exe` is not next to `codenotch.exe`.

Removing: the same button reads "Remove hooks" once installed. It deletes only commands containing
`codenotch-hook` (or the legacy names `eatbean-hook`, `pacman-hook`). A matcher group, an event or
the `hooks` object is removed only when that removal emptied it. You can also restore
`settings.json.codenotch.bak` by hand.

How it works: the hook server listens on `127.0.0.1:<hookPort>` (default 48666) while
`hooksEnabled` is on. `codenotch-hook.exe` reads the hook's JSON from stdin (at most 256 KiB), the
port from `config.gpui.json` and the token from `auth_token`, then posts the event with a 300 ms
connect and 700 ms I/O timeout. It never prints, always exits 0 and does not start CodeNotch when
nothing listens.

Without hooks, Claude Code activity is still guessed from its transcript files. Busy state is also
read for Codex, Cursor and OpenCode (from their local SQLite databases, read-only) and for
Antigravity (from transcript modification times).

## Analytics

The analytics report behind Settings → Analytics covers `analyticsDays` days (default 7): tokens
and cost per provider, model, project, day and hour, in USD and EUR (`usdToEur`), and a comparison
with plan prices (`planPricesEur`).

Sources, read-only:

- Claude Code: `<profile>\projects\**\*.jsonl` for `CLAUDE_CONFIG_DIR` or `~\.claude`, and every
  enabled Claude account's profile folder.
- Codex: `<profile>\sessions\**\rollout-*.jsonl` and `<profile>\archived_sessions\…` for
  `CODEX_HOME` or `~\.codex`, and every enabled Codex account's profile folder.

Only usage numbers, model ids, working directories and timestamps are read, never message content.
The index in `analytics-cache.json` is incremental: unchanged files are skipped and grown files are
resumed. The engine runs it 10 seconds after start, every 15 minutes, and on "Refresh".

### pricing.json

Built-in prices cover Anthropic Claude models and the OpenAI GPT-5 family. A model without a price
is never priced at 0: its tokens are counted as unpriced and left out of the cost. Add or override
prices in `<data folder>\pricing.json`:

```json
{
  "models": {
    "gpt-6-luna":      { "input": 1.25, "output": 10.0, "cacheRead": 0.125, "cacheWrite": 1.25 },
    "claude-opus-5-5": { "input": 4.0, "output": 20.0, "cacheRead": 0.2, "cacheWrite": 5.0, "cacheWrite1h": 8.0 },
    "claude-opus-4-1": null
  }
}
```

- Keys are model ids. They are normalised like transcript ids (lower case; prefixes such as
  `anthropic.` or `openai/`, date suffixes such as `-20250514` or `@20250514`, Bedrock `-v1:0` and
  `[1m]` tags removed), then matched exactly: `gpt-5` never prices `gpt-5-pro`.
- Every rate is USD per million tokens. `input`, `output`, `cacheRead` and `cacheWrite` (5-minute
  cache writes) are required. `cacheWrite1h` is optional and defaults to `cacheWrite`. The
  snake_case spellings `cache_read`, `cache_write`, `cache_write_1h` are accepted too.
- An entry missing a required rate is ignored (and logged).
- `null` removes a built-in price.
- An unreadable file is ignored (and logged); the built-in table is used alone.

## Taskbar mode

Settings → Appearance → Taskbar → "Show rings in": Notch, Taskbar left, or Taskbar right (setting
`taskbarMode`: `off`, `left`, `right`).

In taskbar mode the floating notch is hidden and the same cells are drawn as a strip over the free
space of the primary taskbar. Hover a ring for its card, click to refresh it, right-click for
Settings. The strip hides when there is no room, the taskbar is hidden or an app is full screen.

Limits, from the code:

- Only the primary taskbar, horizontal, at the bottom or top of the primary monitor. Vertical,
  secondary and third-party taskbars are not supported; the strip hides.
- The free space is found with UI Automation. Nothing is injected into Explorer and no Explorer
  window is changed. Explorer's layout is not a public API, so a future Windows build may break the
  measurement; the strip then hides after 6 seconds.
- Left-to-right taskbars only.

Taskbar mode was never seen on screen (see [Status](#status)).

## Updates

CodeNotch updates itself from a small JSON **manifest**. By default it reads
`https://raw.githubusercontent.com/MakFly/codenotch-windows/main/update.json` (the
[`update.json`](update.json) file at the root of this repository); Settings → General →
"Update address" (`updateManifestUrl`) points it elsewhere, for example another repository.

**When.** About 20 seconds after start, then every 6 hours, unless `updateMode` is `off`.
"Check for updates" in Settings → General checks at once in every mode (a toast says
"Checking…", then the result).

**What happens with a newer version** (`updateMode`, Settings → General → "New versions"):

| Mode | Behaviour |
|---|---|
| `notify` (default) | A toast says "CodeNotch 1.1.0 is available" with **Update** and **Notes** (once per version and session), plus a Windows notification when notifications are on and no full-screen app or game mode is in front. **Update** downloads, installs and restarts: "Downloading… 42 %", "Restarting…", then "Updated to 1.1.0" in the new version. |
| `auto` | The package is downloaded in the background and installed when CodeNotch is idle (no Claude Code / Codex session working or waiting, settings window not in use), or at the next start. |
| `off` | No automatic check; the button still works. |

A running version below the manifest's `min_version` is a **required** update: downloaded and
installed at once, in every mode, under a toast that cannot be dismissed ("Required update to
1.1.0… 42 %").

The toasts appear in the settings window when it is open, otherwise in a small transparent window
at the top centre of the notch's screen (under the pill when the notch is on the top edge). It is
click-through outside the toasts and exists only while toasts are shown.

**How the install works.**

1. The zip is downloaded to `<data folder>\updates\<version>\` (https only; redirects are
   followed, never to a non-https address). Its size and SHA-256 must match the manifest, or it is
   deleted and the update fails.
2. `codenotch.exe` and `codenotch-hook.exe` are unpacked from it by file name (the paths inside
   the zip are ignored); nothing else from the zip is kept, and nothing is run.
3. Windows cannot overwrite a running exe but can rename it: each installed exe is renamed
   `<name>.old` and the new one moved into the install folder (the folder of the running
   `codenotch.exe`, for example `%LOCALAPPDATA%\Programs\CodeNotch` after `make install`, or an
   unzipped `dist` folder). A rename is retried for 2 seconds (a Claude Code hook may be starting
   `codenotch-hook.exe`). Any failure puts the old files back.
4. The new `codenotch.exe` is started with `--updated-from=<old version> --wait-pid=<old pid>`; it
   waits for the old process to exit (and release the single-instance lock), deletes the `.old`
   files and the downloaded package, and shows "Updated to …". If it cannot be started, the old
   files are put back. If a restart does not run the expected version, that version is reported
   ("The last update did not take effect.") and never installed automatically again.

An exe running from a cargo `target\` folder never updates itself ("Self-update is off for
development builds"); `CODENOTCH_UPDATE_ALLOW_DEV=1` allows it.

### Manifest format

```json
{
  "version": "1.1.0",
  "min_version": "1.0.0",
  "published_at": "2026-11-12",
  "notes": "Short release notes.",
  "notes_url": "https://github.com/MakFly/codenotch-windows/releases/tag/v1.1.0",
  "windows_x64": {
    "url": "https://github.com/MakFly/codenotch-windows/releases/download/v1.1.0/CodeNotch-1.1.0.zip",
    "sha256": "<64 hex digits: SHA-256 of the zip>",
    "size": 12345678
  }
}
```

| Field | Meaning |
|---|---|
| `version` | Latest version (semantic versioning; a leading `v` is accepted). Compared with strict semver precedence: `1.1.0-rc.1` < `1.1.0` < `1.10.0`. Required. |
| `min_version` | Optional. A running version below it updates without asking. |
| `published_at`, `notes` | Optional, informational. |
| `notes_url` | Optional release page for the **Notes** button (https only). |
| `windows_x64` | The package: a zip holding `codenotch.exe` (required) and `codenotch-hook.exe`, as `make dist` builds it. `url` (https), `sha256` (hex), `size` (exact bytes). Needed only when `version` is newer than the running one. |

One file per channel; unknown fields are ignored.

### Publishing a release

1. Set the new version in `Cargo.toml` (`[workspace.package] version`), update `CHANGELOG.md`.
2. `make dist` → `dist\CodeNotch-<version>.zip`.
3. Create the GitHub release `v<version>` and attach that zip as `CodeNotch-<version>.zip`.
4. `make update-manifest` rewrites `update.json` from the zip: `version`, `size`, `sha256`, the
   release URLs and today's date. It keeps `min_version` from the current file (the new version
   when there is none) and the notes when the version did not change. Edit `notes` and, to force
   everyone up, `min_version` by hand. Without make: `Get-FileHash -Algorithm SHA256
   dist\CodeNotch-<version>.zip` and `(Get-Item dist\CodeNotch-<version>.zip).Length`.
5. Commit and push `update.json` to `main` (raw.githubusercontent.com may serve the old file for a
   few minutes). Publish the zip **before** the manifest: a manifest pointing at a missing file
   makes every update fail.

The `update.json` in this repository describes 1.0.0 with a placeholder SHA-256 and size: a 1.0.0
app reads it as up to date and never downloads it. `make update-manifest` fills in the real values.

### Testing an update locally

`CODENOTCH_UPDATE_URL` overrides the manifest address and also accepts `file:///C:/…/update.json`
or a plain path (local files are otherwise accepted only by debug builds), with a `file:///` zip URL
inside. In a **debug** build, `CODENOTCH_UPDATE_PRETEND_VERSION=0.9.0` makes the updater believe it
runs 0.9.0 (it is not passed to the restarted exe). With `--demo`, the updater runs only when
`CODENOTCH_UPDATE_URL` is set. A dry run: copy `codenotch.exe` and `codenotch-hook.exe` to a scratch
folder, zip a second copy as `CodeNotch-1.0.0/…`, write a manifest with its SHA-256 and size (add
`"min_version": "1.0.0"` to make it install without a click), then start the copy with
`CODENOTCH_DATA_DIR`, `CODENOTCH_UPDATE_URL`, `CODENOTCH_UPDATE_PRETEND_VERSION=0.9.0` and `--demo`.

## Settings reference

All settings live under `settings` in `config.gpui.json`. Keys are camelCase.

### Keys from 0.2 and 0.3 (names frozen)

| Key | Meaning | Default |
|---|---|---|
| `language` | UI language: `en` or `fr`. `auto`, `system` or empty follow the Windows UI language. Anything else falls back to English. | `"en"` |
| `theme` | `system`, `light` or `dark`. Applies to the notch, its cards, the reset cards and the settings window. | `"system"` |
| `pollSeconds` | Automatic refresh period in seconds, clamped to 30–300. | `60` |
| `notifications` | Master switch for threshold, forecast and reset alerts. | `true` |
| `alertPercent` | Watch threshold in percent used (also the "watch" colour), clamped to 1–99. | `80` |
| `widget` | Legacy "show the notch". `false` means hidden when `showMode` is absent. | `true` |
| `widgetPosition` | Screen edge: `left`, `right`, `top`, `bottom`. | `"right"` |
| `autoHide` | Legacy 0.3 "expand on hover". Kept for round trip; not used. | `true` |
| `hideInFullscreen` | Legacy 0.3 "hide in full screen". Kept for round trip; `foldForFullScreen` is used instead. | `true` |
| `gameMode` | Hide everything and stop non-critical alerts. | `false` |
| `widgetPlacement` | `{ "monitor": "<device name, empty = primary>", "x": 1.0, "y": 0.5 }`. `y` is the position along the edge, 0–1. `x` is legacy and not used. | `{"monitor":"","x":1.0,"y":0.5}` |
| `onboardingDone` | Whether the first-run onboarding was finished. Absent: existing accounts count as onboarded. | absent |
| `criticalPercent` | Critical threshold in percent used, clamped to (watch + 1)–100. | `95` |
| `forecastAlerts` | Forecast alerts on or off. | `true` |
| `forecastMinutes` | Forecast lead time in minutes. | `30` |
| `resetAlerts` | Desktop notification when a window resets. | `true` |
| `criticalInFullscreen` | Let critical alerts through while a full-screen app is in front. | `true` |
| `telegramEnabled` | Send alerts to Telegram. | `false` |
| `telegramChatId` | Telegram chat id (the bot token is not stored here). | `""` |
| `discordEnabled` | Send alerts to Discord. | `false` |
| `launchAtStartup` | Open CodeNotch at login. | `false` |
| `trayShowsUsage` | Tray tooltip lists the percentages. | `true` |
| `hotkey` | Global shortcut, for example `"Ctrl+Alt+N"`. `null` = none. | `null` |
| `historyHours` | Hours of history kept in `history.jsonl`. | `24` |
| `analyticsDays` | Analytics period in days. | `7` |
| `usdToEur` | Conversion rate for costs in EUR. | `0.92` |
| `planPricesEur` | Plan prices in EUR per month. Keys: `"<Provider> <plan>"` (`"Claude Max"`) or the provider name alone. Values: a number or a numeric string. Readers also accept an account id, account name, bare plan name or provider id. | `{}` |

### Keys added in 1.0 (all optional)

| Key | Meaning | Default |
|---|---|---|
| `notchSize` | `small` (0.8×), `medium` (1.0×), `large` (1.25×). | `"medium"` |
| `showMode` | `always`, `hover` (folded tab until hovered) or `hidden` (no pill; the tray icon stays and reset cards still appear). Absent (0.3 file): `hover`, or `hidden` when `widget` is `false`. | `"hover"` on a fresh install |
| `foldForFullScreen` | Fold the pill while a full-screen app is in front of its monitor. | `true` |
| `adaptivePill` | The folded pill turns dark or light from the brightness behind it. | `false` |
| `notchAlwaysDark` | Legacy "notch always dark". Kept for round trip; ignored: the notch and its cards follow `theme`. | `true` |
| `weeklyRing` | Second ring for the weekly window: `off`, `inside`, `outside`. | `"off"` |
| `weeklyRingDashed` | Draw the weekly ring dashed. | `false` |
| `showRemaining` | Show remaining instead of used (the colour still grades the used share). | `false` |
| `showUsagePace` | Show the usage pace figure on the card. | `false` |
| `showForecast` | Show the exhaustion forecast on the card and the Accounts page. | `true` |
| `colorTransition` | Ring colour change: `hard_step` or `ramp`. | `"hard_step"` |
| `taskbarMode` | `off`, `left`, `right` (see [Taskbar mode](#taskbar-mode)). | `"off"` |
| `resetCards` | On-screen card when a window renews. | `true` |
| `resetSound` | Play the system sound with the reset card. | `true` |
| `providerOrder` | Provider ids in display order, completed with the default order. Empty = default. | `[]` |
| `hiddenAccounts` | Account ids that are fetched but not drawn on the notch. | `[]` |
| `hooksEnabled` | Claude Code hooks installed; the hook server runs while on. | `false` |
| `hookPort` | Hook server port on 127.0.0.1. | `48666` |
| `updateMode` | `notify`, `auto` or `off` (see [Updates](#updates)). Absent (file from 1.0 or older): `off` when `updateChecks` is `false`, else `notify`. | `"notify"` |
| `updateManifestUrl` | Update manifest address (https). Absent or empty: the official `update.json`. | absent |
| `updateChecks` | Legacy switch, superseded by `updateMode` and kept in sync with it (`false` exactly when the mode is `off`). | `true` |
| `antigravityLimit` | Antigravity lane: `automatic`, `5h`, `weekly`. | `"automatic"` |
| `antigravityModel` | Antigravity model family: `gemini` or `3p` (Claude and GPT models). | `"gemini"` |
| `settingsSidebarCollapsed` | Settings window: navigation sidebar collapsed to its icon rail (toggle button or Ctrl+B). A window narrower than 900 px shows the rail without changing this key. | `false` |

Default provider order: `claude`, `codex`, `glm`, `opencode`, `commandcode`, `cursor`, `grok`,
`copilot`, `antigravity`, `qoder`, `custom`.

### Account keys

Each entry of `accounts`:

| Key | Meaning |
|---|---|
| `id` | Stable unique id. |
| `name` | Name shown to you. |
| `provider` | `claude`, `codex`, `glm`, `opencode`, `commandcode`, `cursor`, `grok`, `copilot`, `antigravity`, `qoder`, `custom`. `gemini` is read as Antigravity, `zai` and `z.ai` as GLM. An unknown value is kept as it is. |
| `enabled` | Disabled accounts are neither fetched nor shown. |
| `profilePath` | The provider's profile folder. Empty = the provider's default folder. |
| `endpoint` | Custom JSON endpoint URL. Any account with an endpoint is read as a custom endpoint. |
| `secretEnv` | The **name** of an environment variable holding the token, never the token itself. |
| `hasSecret` | Informational flag. |

## Environment variables

| Variable | Effect |
|---|---|
| `CODENOTCH_DATA_DIR` | Use this folder instead of `%APPDATA%\dev.kev.smashtoken`. Also gives the instance lock its own name, so a test copy can run beside the installed one. Read by `codenotch-hook.exe` too. |
| `CODENOTCH_NO_SPAWN` | `1` = start no child process: no Claude/Grok renewal, no sign-in window, no `codex.exe app-server`, no `gh`, no Antigravity CLI, no Qoder sidecar, no PowerShell or `netstat` probes. |
| `CODENOTCH_NOTCH_NO_REGION` | Any value other than `0`: the notch window is not clipped to its visible shapes (no window region). For diagnosing the notch window. |
| `CODENOTCH_TELEGRAM_TOKEN` | Telegram bot token. Wins over Credential Manager. |
| `CODENOTCH_DISCORD_WEBHOOK` | Discord webhook URL. Wins over Credential Manager. |
| `CODENOTCH_QODER_SIDECAR` | Path of `qoder-usage.cjs`, searched first. |
| `CODENOTCH_QODER_NODE` | Path of `node.exe` for the Qoder sidecar. |
| `CODENOTCH_UPDATE_URL` | Update manifest address, instead of the setting. Also accepts `file://` URLs and paths (tests, local dry runs). With `--demo`, starts the updater. |
| `CODENOTCH_UPDATE_ALLOW_DEV` | `1` = allow self-update of an exe running from a cargo `target\` folder. |
| `CODENOTCH_UPDATE_PRETEND_VERSION` | Debug builds only: the version the updater compares with (dry runs). Not passed to the restarted exe. |
| `CODENOTCH_WRITE_ICON` | Tests only: `1` makes one test rewrite `crates\app\assets\codenotch.ico`. |
| `CLAUDE_CONFIG_DIR` | An extra Claude Code profile folder for the hooks installer, and the default Claude folder for analytics. The Claude provider itself does not read it. |
| `CODEX_HOME` | The Codex folder for analytics and activity, instead of `~\.codex`. The Codex provider itself uses `~\.codex` or the account's `profilePath`. |
| `XDG_DATA_HOME` | OpenCode's data folder parent. |
| `GH_CONFIG_DIR`, `XDG_CONFIG_HOME` | Where GitHub CLI's `hosts.yml` is looked for. |
| `GH_TOKEN`, `GITHUB_TOKEN` | GitHub token for Copilot. |
| `GLM_API_KEY` | z.ai key. |
| `COMMAND_CODE_API_KEY` | Command Code key. |
| `HTTPS_PROXY`, `HTTP_PROXY` (and lower-case), `NO_PROXY` | Proxy. When none is set, the app copies the Windows (WinINet) proxy into them at start, and sets `NO_PROXY` to loopback plus the Windows bypass list. |
| any name in an account's `secretEnv` | That account's token. |

`PATH`, `ProgramFiles`, `ProgramFiles(x86)` and `SystemRoot` are read to find provider CLIs and
system tools. `CODENOTCH_CLAUDE_CLI` is set by the app for its own sign-in window; it is not an
input.

## Troubleshooting

- **Log**: `<data folder>\logs\codenotch.log` (`make logs` shows its end). Debug builds log at debug
  level, release builds at info level. Lines are redacted (bearer tokens, `token=` style values,
  Telegram bot tokens, long letter-and-digit runs).
- **What does the app see?** Run `codenotch.exe doctor`. It is read-only and prints no secrets.
- **Readings without side effects**: run `codenotch.exe --once` with `CODENOTCH_NO_SPAWN=1`.
- **Try with a clean data folder**: set `CODENOTCH_DATA_DIR` to an empty folder.
- **Notch drawing or mouse problems**: start with `CODENOTCH_NOTCH_NO_REGION=1` to see whether the
  window region is involved.
- **Build fails with "Access is denied (os error 5)"**: a running `codenotch.exe` locks its file.
  Quit it from the tray or run `make stop`.
- **Hooks not installed: codenotch-hook.exe was not found**: build the hook
  (`cargo build -p codenotch-hook`, or `--release`) so it sits next to `codenotch.exe`.
- **Config unreadable**: the app starts on defaults and keeps a copy as
  `config.gpui.unreadable.json`. The original stays in place until the next save.
- **Hook server port taken by another program**: 1.0.0 does not show the hook server's state in the
  UI, in `doctor` or in the log. If another program holds port 48666, quit CodeNotch, set `hookPort`
  in `config.gpui.json` to a free port and start it again. The helper reads the port from the
  config on every call, so the hooks do not need to be reinstalled.

```powershell
$env:CODENOTCH_NO_SPAWN = "1"
target\release\codenotch.exe --once
```

## Workspace layout

| Crate (folder) | Package | Role |
|---|---|---|
| `crates/core` | `codenotch-core` | Shared model, config, paths, history, state, secrets API, HTTP client, i18n, formatting, logging. No GPUI, no Win32. |
| `crates/providers` | `codenotch-providers` | Claude, Codex, Grok and the custom JSON endpoint. |
| `crates/providers-extra` | `codenotch-providers-extra` | Cursor, Copilot, OpenCode, Command Code, GLM, Antigravity, Qoder. |
| `crates/notify` | `codenotch-notify` | Threshold, forecast and reset alerts; Windows toasts, Telegram, Discord. |
| `crates/analytics` | `codenotch-analytics` | Token and cost analytics from local Claude and Codex transcripts. |
| `crates/activity` | `codenotch-activity` | Hook server, transcript watcher, hooks installer, busy state. |
| `crates/hook` | `codenotch-hook` | `codenotch-hook.exe`, the helper Claude Code calls. |
| `crates/platform` | `codenotch-platform` | Win32 services: full-screen and game detection, autostart, hotkey, tray, single instance, Credential Manager, backdrop, taskbar measurement. |
| `crates/ui-kit` | `codenotch-ui-kit` | GPUI shared kit: UI model, theme, ring painter, controls, overlay windows. |
| `crates/ui-notch` | `codenotch-ui-notch` | Notch pill, hover card, reset card, taskbar strip, provider marks. |
| `crates/ui-settings` | `codenotch-ui-settings` | Settings window, onboarding, analytics view. |
| `crates/engine` | `codenotch-engine` | Scheduler, back-off, persistence, self-update (`update/`); turns providers, activity and alerts into snapshots. |
| `crates/app` | `codenotch` | `codenotch.exe`: CLI and wiring. |

Build or test one crate by its package name:

```powershell
cargo build -p codenotch-hook
cargo test -p codenotch-core
```

Manual checks against the real machine (read-only, aggregate output only, not part of the normal
test run):

```powershell
cargo test -p codenotch-core -- --ignored real_config
cargo test -p codenotch-analytics --test real_sanity -- --ignored --nocapture
cargo run -p codenotch-providers-extra --example live_check
```

## Privacy and security

What the code does:

- **Hosts contacted.** Each provider's own usage endpoint, with that provider's credential only:
  `api.anthropic.com`, `chatgpt.com`, `cli-chat-proxy.grok.com`, `cursor.com`, `api.github.com`,
  `opencode.ai` (plus the OpenCode console URL from its stored metadata, HTTPS only),
  `api.commandcode.ai`, `api.z.ai` or `open.bigmodel.cn`, and your custom endpoint. Alerts go to
  `api.telegram.org` and your Discord webhook host when you turn them on. The update check reads
  the manifest (`raw.githubusercontent.com` by default) and, when you update, downloads the package
  from the address it gives (GitHub release assets by default). Antigravity is read through its CLI or a loopback address; the Qoder SDK makes
  its own requests.
- **Nothing else leaves the machine.** Analytics, history, state and activity stay local.
  Transcript message content is never read.
- **Hook server**: bound to `127.0.0.1` only. It accepts only `POST /event` with the token from
  `auth_token` (compared in constant time), rejects non-loopback `Host` and `Origin` headers and
  cross-site browser requests, caps headers at 16 KiB and bodies at 256 KiB.
- **Secrets** (Telegram token, Discord webhook, account tokens) are kept in Windows Credential
  Manager or read from environment variables. They are never written to `config.gpui.json`,
  `state.json`, history, snapshots or logs, and their `Debug` output is `***`. The config stores only
  the name of a token's environment variable. The Telegram chat id is the exception: it is stored in
  the config, and hidden from debug output and logs.
- **Logs** are redacted before writing. `doctor` prints presence only.
- **Child processes** (provider CLIs, `gh`, PowerShell, `netstat`, Node) run hidden, with a timeout,
  inside a kill-on-close job where the code says so, and not at all with `CODENOTCH_NO_SPAWN=1`.
- **Links** opened from the app must be `http` or `https`. "Open data folder" shows a folder and
  never runs a file.
- Grok sessions are sent only when issued by `https://auth.x.ai`. The Antigravity loopback client
  accepts a self-signed certificate for `127.0.0.1` only.

## Credits and licence

- Design and behaviour after [CodeNotch by Vinz](https://github.com/vinzdg/codenotch) (MIT),
  including its Windows port. No code was copied.
- Provider marks: SVG files from [lobe-icons](https://github.com/lobehub/lobe-icons)
  (`@lobehub/icons-static-svg`, MIT License, © LobeHub). Their notice and the trademark note are in
  [`crates/ui-notch/assets/glyphs/NOTICE.md`](crates/ui-notch/assets/glyphs/NOTICE.md). The marks
  are trademarks of their owners and identify the product whose usage is shown.
- CodeNotch for Windows is released under the MIT License. See [LICENSE](LICENSE).
