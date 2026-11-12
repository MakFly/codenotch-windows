//! `codenotch-hook.exe <event>` (owner: agent E)
//!
//! Called by Claude Code hooks. Reads stdin (≤ `codenotch_core::hooks::MAX_PAYLOAD`), reads the
//! port from `config.gpui.json` (`settings.hookPort`, default 48666) and the token from
//! `Paths::hook_token()`, then POSTs `codenotch_core::hooks::event_path(event, ppid)` to
//! 127.0.0.1 with a 300 ms connect / 700 ms I/O timeout. Must never block Claude Code, never
//! print anything, and always exit 0. Built as a console app (Claude Code captures stdio).
//! The logic lives in the library (`codenotch_hook::run`) so it can be tested in-process.

use std::io::IsTerminal;

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(|| {
        let arg = std::env::args().nth(1);
        let stdin = std::io::stdin();
        let piped = !stdin.is_terminal();
        let paths = codenotch_core::Paths::resolve();
        codenotch_hook::run(arg.as_deref(), piped.then_some(stdin), &paths);
    });
    std::process::exit(0);
}
