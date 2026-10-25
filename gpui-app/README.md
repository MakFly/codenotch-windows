# CodeNotch — GPUI prototype

Experimental native rewrite of the notch with [GPUI](https://gpui.rs), Zed's GPU UI framework.
No WebView: the provider code from `src-tauri/src` (`model`, `providers`, `claude`, `codex`,
`gemini`, `grok`, `opencode`, `login`, `alerts`) is mounted as-is with `#[path]`, so both apps
read quotas the same way.

```bash
cargo run --manifest-path gpui-app/Cargo.toml
```

## What works

- Notch rail on the right edge of the primary display, one usage ring per enabled account.
- Click a ring to open the usage popover (windows, reset countdown, forecast, refresh).
- Polling with the same scheduling rules as the Tauri backend (provider backoff, Claude floor).
- Reads the Tauri app's `%APPDATA%\dev.kev.smashtoken\config.json`, never writes it; without
  one, local CLI profiles are discovered as on first launch.

## Not yet ported

Settings and welcome screens, account login, tray icon (right click on the rail quits),
auto-hide / fold, full screen and game mode, drag to move, alert delivery (Windows,
Telegram, Discord), updater, provider logos.
