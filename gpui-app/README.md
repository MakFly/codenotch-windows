# CodeNotch — GPUI prototype

Experimental native rewrite of the notch with [GPUI](https://gpui.rs), Zed's GPU UI framework.
No WebView: the provider code from `src-tauri/src` (`model`, `providers`, `claude`, `codex`,
`gemini`, `grok`, `opencode`, `login`, `alerts`) is mounted as-is with `#[path]`, so both apps
read quotas the same way. Provider logos come from the same `src/lib/glyphs.json`.

```bash
cargo run --manifest-path gpui-app/Cargo.toml
```

Add `-- --settings` to open the settings window at launch.

## What works

- Notch rail on any edge of the primary display (clear of the taskbar), one usage ring per enabled account,
  with the provider's logo.
- Hover or click a ring to open the usage popover (windows, reset countdown, forecast, refresh).
- Auto-hide: folded to a peek tab, revealed on hover, folded again 240 ms after the pointer
  leaves both the rail and the popover; » folds it by hand.
- Hidden in game mode and, when the setting asks for it, while an app is full screen.
- Tray icon (`tray-icon` crate): Settings, game mode toggle and Quit, in the chosen language.
- Text fields (`input.rs`, from GPUI's input example): IME, clipboard, selection, masked mode.
- Settings window with a Cursor-style frameless header (logo, context pill, caption buttons):
  - Accounts: add or edit (provider, name, profile folder, JSON URL, token kept in Windows
    Credential Manager, environment variable), sign in through the official CLI, enable or
    disable, remove (with confirmation), detect local CLI profiles.
  - Notch: show, fold automatically, edge, hide in full screen, game mode, poll interval.
  - Language: French or English, applied at once to the notch, the window and the tray menu;
    the welcome screen can be shown again from here.
- Welcome screen on first launch (no account yet, as in the Tauri app): CLI install and
  sign-in state for each provider, sign in or add a detected session, language choice.
  - Alerts: notifications, warning and critical levels, urgent alerts in full screen,
    limit forecast and its lead time, quota-back notice, Discord webhook and Telegram bot
    setup (secrets in Credential Manager, chat detection, test message; Telegram sending
    stays off behind the same flag as the Tauri app).
- Polling with the same scheduling rules as the Tauri backend (provider backoff, Claude floor).
- Configuration: saved to `%APPDATA%\dev.kev.smashtoken\config.gpui.json`, first copied from
  the Tauri app's `config.json`, which is never written; without either, local CLI profiles
  are discovered as on first launch.

## Not yet ported

Alert delivery (Windows,
Telegram, Discord); fold animation, Escape to fold, drag to move; updater. Text fields have
no Tab navigation or undo yet.
