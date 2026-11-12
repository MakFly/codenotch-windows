//! Logic of `codenotch-hook.exe <event>` (data-spec §9.1 "Hook helper protocol").
//!
//! Claude Code runs the helper on every hook event. It reads its stdin (the hook JSON, at most
//! `MAX_PAYLOAD` bytes forwarded, read for at most [`STDIN_BUDGET`]), the port from
//! `config.gpui.json` (`settings.hookPort`, default 48666) and the token from
//! `Paths::hook_token()`, then POSTs `event_path(event, ppid)` to 127.0.0.1 with a 300 ms
//! connect / 700 ms I/O timeout. It never prints, never blocks Claude Code for long and always
//! exits 0. It does not start CodeNotch when nothing listens.

use codenotch_core::hooks::{event_path, HookEvent, DEFAULT_PORT, MAX_PAYLOAD, TOKEN_HEADER};
use codenotch_core::{Config, Paths};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
pub const IO_TIMEOUT: Duration = Duration::from_millis(700);
/// Longest time spent reading stdin.
pub const STDIN_BUDGET: Duration = Duration::from_millis(1_500);

/// Port from `config.gpui.json` (`settings.hookPort`), default [`DEFAULT_PORT`].
pub fn read_port(paths: &Paths) -> u16 {
    std::fs::read_to_string(paths.config())
        .ok()
        .and_then(|t| Config::parse(&t).ok())
        .map(|c| c.settings.hook_port)
        .filter(|p| *p != 0)
        .unwrap_or(DEFAULT_PORT)
}

/// Token file content, trimmed (`None` when missing or empty).
pub fn read_token(paths: &Paths) -> Option<String> {
    std::fs::read_to_string(paths.hook_token())
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty() && t.len() <= 256 && t.bytes().all(|b| b.is_ascii_graphic()))
}

/// Read at most `cap` bytes from `reader` within `budget`; the rest is drained (discarded) while
/// time remains so the writer does not see a broken pipe. Never blocks longer than `budget`.
pub fn read_capped<R: Read + Send + 'static>(
    mut reader: R,
    cap: usize,
    budget: Duration,
) -> Vec<u8> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let spawned = std::thread::Builder::new()
        .name("stdin".into())
        .spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            return;
                        }
                    }
                }
            }
        });
    if spawned.is_err() {
        return Vec::new();
    }
    let deadline = Instant::now() + budget;
    let mut out = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(chunk) => {
                let room = cap.saturating_sub(out.len());
                out.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            Err(_) => break, // EOF (sender dropped) or out of time
        }
    }
    out
}

/// Build the raw request bytes.
pub fn build_request(event: HookEvent, ppid: u32, token: Option<&str>, body: &[u8]) -> Vec<u8> {
    let mut head = format!(
        "POST {} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        event_path(event, ppid),
        body.len()
    );
    if let Some(t) = token {
        head.push_str(&format!("{TOKEN_HEADER}: {t}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

/// Send one event and wait for any response byte. Errors are returned for tests; the binary
/// ignores them.
pub fn send(
    port: u16,
    event: HookEvent,
    ppid: u32,
    token: Option<&str>,
    body: &[u8],
) -> std::io::Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(&build_request(event, ppid, token, body))?;
    stream.flush()?;
    let mut byte = [0u8; 1];
    let _ = stream.read(&mut byte)?;
    Ok(())
}

/// Shells that only wrap the hook command: the session's process is their parent.
fn is_wrapper_shell(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "bash.exe" | "sh.exe" | "dash.exe" | "cmd.exe"
    )
}

/// Process id reported as `ppid`: the helper's parent, skipping up to three wrapper shells
/// (Claude Code may run hook commands through `bash`/`cmd`, which exit right after) so the id
/// stays valid for the "seen clears it" ancestry check. 0 when unknown.
pub fn session_pid() -> u32 {
    let table = process_table();
    let mut pid = match table.iter().find(|(p, _, _)| *p == std::process::id()) {
        Some((_, parent, _)) => *parent,
        None => return 0,
    };
    for _ in 0..3 {
        match table.iter().find(|(p, _, _)| *p == pid) {
            Some((_, parent, name)) if is_wrapper_shell(name) && *parent != 0 => pid = *parent,
            _ => break,
        }
    }
    pid
}

#[cfg(windows)]
fn process_table() -> Vec<(u32, u32, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: snapshot handle checked and closed; entry struct sized as required.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            out.push((
                entry.th32ProcessID,
                entry.th32ParentProcessID,
                String::from_utf16_lossy(&entry.szExeFile[..len]),
            ));
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    out
}

#[cfg(not(windows))]
fn process_table() -> Vec<(u32, u32, String)> {
    Vec::new()
}

/// Whole helper run. `stdin` is `None` when it is a console (nothing piped).
pub fn run<R: Read + Send + 'static>(arg: Option<&str>, stdin: Option<R>, paths: &Paths) {
    let Some(event) = HookEvent::from_arg(arg.unwrap_or("ping")) else {
        return;
    };
    let body = stdin
        .map(|r| read_capped(r, MAX_PAYLOAD, STDIN_BUDGET))
        .unwrap_or_default();
    let port = read_port(paths);
    let token = read_token(paths);
    let _ = send(port, event, session_pid(), token.as_deref(), &body);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_layout() {
        let r = String::from_utf8(build_request(HookEvent::Done, 9, Some("tok"), b"{}")).unwrap();
        assert!(r.starts_with("POST /event?e=done&ppid=9 HTTP/1.1\r\nHost: 127.0.0.1\r\n"));
        assert!(r.contains("Content-Length: 2\r\n") && r.contains("X-Codenotch-Token: tok\r\n"));
        assert!(r.ends_with("Connection: close\r\n\r\n{}"));
        let r = String::from_utf8(build_request(HookEvent::Ping, 0, None, b"")).unwrap();
        assert!(!r.contains("X-Codenotch-Token"));
    }

    #[test]
    fn stdin_is_capped() {
        let data = vec![b'x'; MAX_PAYLOAD + 5000];
        let got = read_capped(
            std::io::Cursor::new(data),
            MAX_PAYLOAD,
            Duration::from_secs(5),
        );
        assert_eq!(got.len(), MAX_PAYLOAD);
        let small = read_capped(
            std::io::Cursor::new(b"{\"a\":1}".to_vec()),
            MAX_PAYLOAD,
            Duration::from_secs(5),
        );
        assert_eq!(small, b"{\"a\":1}");
    }

    /// A reader that never ends must not block past the budget.
    #[test]
    fn stdin_never_blocks() {
        struct Endless;
        impl Read for Endless {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                std::thread::sleep(Duration::from_secs(30));
                Ok(0)
            }
        }
        let t0 = Instant::now();
        assert!(read_capped(Endless, 10, Duration::from_millis(100)).is_empty());
        assert!(t0.elapsed() < Duration::from_millis(600));
    }

    #[test]
    fn port_and_token_files() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::at(d.path());
        assert_eq!(read_port(&paths), DEFAULT_PORT);
        assert_eq!(read_token(&paths), None);
        std::fs::write(
            paths.config(),
            r#"{"accounts":[],"settings":{"hookPort":50123}}"#,
        )
        .unwrap();
        assert_eq!(read_port(&paths), 50123);
        std::fs::write(paths.config(), "{ not json").unwrap();
        assert_eq!(read_port(&paths), DEFAULT_PORT);
        std::fs::write(paths.hook_token(), "  abc\r\n").unwrap();
        assert_eq!(read_token(&paths).as_deref(), Some("abc"));
    }

    #[test]
    fn nothing_listening_fails_fast() {
        // Bind then drop to get a port that is (very likely) closed.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let t0 = Instant::now();
        assert!(send(port, HookEvent::Ping, 0, None, b"").is_err());
        assert!(t0.elapsed() < Duration::from_millis(1_500));
    }

    #[cfg(windows)]
    #[test]
    fn session_pid_is_an_ancestor() {
        let pid = session_pid();
        assert_ne!(pid, 0);
        assert_ne!(pid, std::process::id());
        assert!(is_wrapper_shell("BASH.EXE") && !is_wrapper_shell("claude.exe"));
    }
}
