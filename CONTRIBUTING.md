# Contributing

Bug reports and pull requests are welcome. This project targets Windows and
uses Bun, Tauri 2, Rust, React and TypeScript.

1. Fork and clone the repository.
2. Install the prerequisites listed in the README.
3. Run `bun install --frozen-lockfile` and `bun run desktop`.
4. Keep changes focused and describe the behavior being fixed.

Before submitting:

```powershell
bun run test
bun run test:native
bun run build
```

For UI changes, run `bun run dev` in another terminal, install Chromium with
`bunx playwright install chromium`, then run `bun run test:ui`. Use demo data
for screenshots. Include both English and French translations when adding UI
text, reuse the shared Select component, and check keyboard navigation.

Never include credentials, account configuration, personal usage screenshots
or provider responses containing identifiers in an issue or pull request.
Describe the provider, app version, Windows version and reproduction steps.
Do not send inference requests just to test a usage connector.

Contributions are made under the project's MIT license. Preserve third-party
attribution and add notices when introducing adapted code or assets.
