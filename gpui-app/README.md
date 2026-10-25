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
- Hover or click a ring to open the usage popover (windows, reset countdown, forecast, refresh).
- Auto-hide: folded to a peek tab, revealed on hover, folded again 240 ms after the pointer
  leaves both the rail and the popover; » folds it by hand.
- Hidden in game mode and, when the setting asks for it, while an app is full screen.
- Tray icon (`tray-icon` crate): game mode toggle and Quit; Settings is greyed out until ported.
- Polling with the same scheduling rules as the Tauri backend (provider backoff, Claude floor).
- Reads the Tauri app's `%APPDATA%\dev.kev.smashtoken\config.json`, never writes it; without
  one, local CLI profiles are discovered as on first launch.

## Not yet ported

Settings and welcome screens, account login, saving game mode (it is in memory only),
fold animation, Escape to fold, drag to move, alert delivery (Windows, Telegram, Discord),
updater, provider logos.
