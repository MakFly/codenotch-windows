# CodeNotch for Windows

A notch on the edge of the Windows desktop that shows how much of each AI account quota is used.

This tree is a **reconstruction** of [MakFly/codenotch-windows](https://github.com/MakFly/codenotch-windows) 0.2.1. The original repository and its git history are gone. Version 0.3.0 keeps the same configuration file and the same GPUI shell, and reads Claude and Codex again. It does not pretend to be the old binary.

The visual reference remains [CodeNotch by Vinz](https://github.com/vinzdg/codenotch). No code is copied from that project.

## What 0.3.0 does

- Loads and saves `%APPDATA%\dev.kev.smashtoken\config.gpui.json`.
- Draws a small black notch. Hover shows the quota windows. Click opens settings.
- Reads Claude (`~/.claude/.credentials.json` → the official OAuth usage endpoint) and Codex (`auth.json` → the ChatGPT usage endpoint).
- Reads a custom HTTPS usage endpoint, or HTTP on localhost, in the JSON shape below.
- Hides the notch in full screen and in game mode.
- Tray menu → **Quit CodeNotch**.

A failed or missing reading stays a dash or a short error. It is never replaced with zero.

Grok sessions are detected and left unread: the old billing call is not part of this rebuild. Telegram, Discord, the signed updater, in-app OAuth, Gemini, Copilot and Ollama are not reconnected.

## Run

Install Rust stable with the MSVC toolchain.

```powershell
cd codenotch-windows\gpui-app
cargo run -- --settings
cargo run -- --once
```

`--once` prints one reading per enabled account and exits. It does not print tokens.

## Custom JSON endpoint

```json
{
  "windows": [
    { "label": "Monthly quota", "usedPercent": 42, "resetsAt": "2026-11-01T00:00:00Z" }
  ]
}
```

Set the account `endpoint` to that URL. An optional bearer token is read from the environment variable named in `secretEnv`.

## License

MIT. See [LICENSE](LICENSE).
