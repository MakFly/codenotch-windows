//! Wire protocol between `codenotch-hook.exe` (called by Claude Code) and the hook server in
//! `codenotch-activity`. Lives in core because the hook binary must not depend on activity.
//!
//! Request (raw HTTP/1.1 over TCP to 127.0.0.1:`hookPort`, connect 300 ms, I/O 700 ms):
//! ```text
//! POST /event?e=<event>&ppid=<pid> HTTP/1.1
//! Host: 127.0.0.1
//! Content-Type: application/json
//! Content-Length: <n>
//! X-Codenotch-Token: <token from <data dir>\auth_token>   (omitted if no token file)
//! Connection: close
//!
//! <stdin of the hook, ≤ 256 KiB>
//! ```

/// Default port (overridden by `settings.hookPort`).
pub const DEFAULT_PORT: u16 = 48666;
/// Header carrying the shared token.
pub const TOKEN_HEADER: &str = "X-Codenotch-Token";
/// Largest stdin payload forwarded.
pub const MAX_PAYLOAD: usize = 256 * 1024;
/// Marker that identifies our entries in `~/.claude/settings.json` (also legacy names).
pub const HOOK_MARKERS: [&str; 3] = ["codenotch-hook", "eatbean-hook", "pacman-hook"];

/// Internal event names (argv[1] of the hook).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    SessionStart,
    /// UserPromptSubmit / PreToolUse / PostToolUse
    Running,
    /// Notification (waiting on the user)
    Attention,
    /// Stop
    Done,
    SessionEnd,
    Ping,
}

impl HookEvent {
    pub const ALL: [HookEvent; 6] = [
        Self::SessionStart,
        Self::Running,
        Self::Attention,
        Self::Done,
        Self::SessionEnd,
        Self::Ping,
    ];

    pub fn as_arg(self) -> &'static str {
        match self {
            Self::SessionStart => "session_start",
            Self::Running => "running",
            Self::Attention => "attention",
            Self::Done => "done",
            Self::SessionEnd => "session_end",
            Self::Ping => "ping",
        }
    }

    pub fn from_arg(arg: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_arg() == arg)
    }

    /// Claude Code hook names that map to this event, with whether they need `"matcher":"*"`.
    pub fn claude_events(self) -> &'static [(&'static str, bool)] {
        match self {
            Self::SessionStart => &[("SessionStart", false)],
            Self::Running => &[
                ("UserPromptSubmit", false),
                ("PreToolUse", true),
                ("PostToolUse", true),
            ],
            Self::Attention => &[("Notification", false)],
            Self::Done => &[("Stop", false)],
            Self::SessionEnd => &[("SessionEnd", false)],
            Self::Ping => &[],
        }
    }
}

/// Request path for an event.
pub fn event_path(event: HookEvent, ppid: u32) -> String {
    format!("/event?e={}&ppid={ppid}", event.as_arg())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_round_trip() {
        for e in HookEvent::ALL {
            assert_eq!(HookEvent::from_arg(e.as_arg()), Some(e));
        }
        assert_eq!(HookEvent::from_arg("nope"), None);
        assert_eq!(event_path(HookEvent::Done, 42), "/event?e=done&ppid=42");
    }
}
