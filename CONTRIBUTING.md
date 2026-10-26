# Contributing

Bug reports and pull requests are welcome. This project targets Windows and
is a native Rust app built with [GPUI](https://www.gpui.rs). The app lives in
`gpui-app/`; it shares the provider connectors in `src-tauri/src/`, the provider
glyphs and the English strings in `src/lib/` with the 0.1.x Tauri app, which
stays in the repository for reference.

1. Fork and clone the repository.
2. Install the prerequisites listed in the README (Rust stable with the MSVC
   toolchain, Visual Studio Build Tools with **Desktop development with C++**).
3. Run `cargo run -- --settings` from `gpui-app/`.
4. Keep changes focused and describe the behavior being fixed.

Before submitting, from `gpui-app/`:

```powershell
cargo test
cargo build --release
```

Use demo data for screenshots. Include both English and French translations
when adding UI text, and check keyboard navigation.

Never include credentials, account configuration, personal usage screenshots
or provider responses containing identifiers in an issue or pull request.
Describe the provider, app version, Windows version and reproduction steps.
Do not send inference requests just to test a usage connector.

Contributions are made under the project's MIT license. Preserve third-party
attribution and add notices when introducing adapted code or assets.
