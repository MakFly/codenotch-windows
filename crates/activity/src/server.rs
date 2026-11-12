//! Loopback hook server ("Local server"): a small HTTP/1.1 listener on
//! `127.0.0.1:<hookPort>` for the POSTs of `codenotch-hook.exe`.
//!
//! Checks on `/event`, in order: method POST (405); at most one `Origin`, and it must be a
//! loopback origin; `Sec-Fetch-Site: cross-site` rejected; `Host`, when present, must be loopback
//! (403); token in `X-Codenotch-Token` or `Authorization: Bearer` (constant time, 401); body
//! ≤ `MAX_PAYLOAD` (413). Anything else is 404. Headers are capped at 16 KiB (431).

use crate::sessions::{SessionEvent, Source};
use crate::token::Token;
use codenotch_core::hooks::{HookEvent, MAX_PAYLOAD, TOKEN_HEADER};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAX_HEADER: usize = 16 * 1024;
const IO_TIMEOUT: Duration = Duration::from_millis(1_000);
const REQUEST_DEADLINE: Duration = Duration::from_millis(2_000);
const MAX_CONNECTIONS: usize = 16;
const ACCEPT_POLL: Duration = Duration::from_millis(40);

/// State of the hook server.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ServerStatus {
    /// Hooks switched off in settings.
    #[default]
    Disabled,
    Listening(u16),
    /// Another program (or another CodeNotch) holds the port.
    PortInUse(u16),
    /// Could not start (token file, socket error …). Short, secret-free message.
    Failed(String),
}

/// One parsed request.
#[derive(Debug, Default)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    fn header_all<'a, 'n>(&'a self, name: &'n str) -> impl Iterator<Item = &'a str> + use<'a, 'n> {
        self.headers
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Outcome of reading a request (the status to answer when it is not usable).
#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    Status(u16),
    Io,
}

/// Read one HTTP/1.1 request with header and body caps.
pub fn read_request(stream: &mut impl Read, deadline: Instant) -> Result<Request, ReadError> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEADER {
            return Err(ReadError::Status(431));
        }
        if Instant::now() > deadline {
            return Err(ReadError::Io);
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(ReadError::Io),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    if header_end > MAX_HEADER {
        return Err(ReadError::Status(431));
    }
    let head = std::str::from_utf8(&buf[..header_end]).map_err(|_| ReadError::Status(400))?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(ReadError::Status(400));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(ReadError::Status(400));
    }
    let mut req = Request {
        method: method.to_string(),
        target: target.to_string(),
        ..Request::default()
    };
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            return Err(ReadError::Status(400));
        };
        req.headers
            .push((k.trim().to_string(), v.trim().to_string()));
    }
    if req.header("Transfer-Encoding").is_some() {
        return Err(ReadError::Status(411));
    }
    let len = match req.header("Content-Length") {
        None => 0,
        Some(v) => v.parse::<usize>().map_err(|_| ReadError::Status(400))?,
    };
    if len > MAX_PAYLOAD {
        return Err(ReadError::Status(413));
    }
    let mut body = buf[header_end + 4..].to_vec();
    if body.len() > len {
        body.truncate(len);
    }
    while body.len() < len {
        if Instant::now() > deadline {
            return Err(ReadError::Io);
        }
        let want = (len - body.len()).min(chunk.len());
        match stream.read(&mut chunk[..want]) {
            Ok(0) | Err(_) => return Err(ReadError::Io),
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    req.body = body;
    Ok(req)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    let name = if let Some(rest) = host.strip_prefix('[') {
        match rest.split_once(']') {
            Some((inner, tail)) if tail.is_empty() || valid_port(tail) => return inner == "::1",
            _ => return false,
        }
    } else {
        match host.rsplit_once(':') {
            Some((n, p)) if valid_port(&format!(":{p}")) => n,
            Some(_) => return false,
            None => host,
        }
    };
    name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1"
}

fn valid_port(tail: &str) -> bool {
    tail.strip_prefix(':')
        .is_some_and(|p| !p.is_empty() && p.len() <= 5 && p.parse::<u16>().is_ok())
}

/// `http(s)://` + loopback host (+ port), nothing else.
pub fn is_loopback_origin(origin: &str) -> bool {
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .or_else(|| origin.strip_prefix("HTTP://"))
        .or_else(|| origin.strip_prefix("HTTPS://"));
    match rest {
        Some(r) if !r.contains('/') && !r.contains('@') => is_loopback_host(r),
        _ => false,
    }
}

/// Validate a request; `Ok(())` = accepted.
pub fn check(req: &Request, token: &Token) -> Result<(), u16> {
    let path = req.target.split('?').next().unwrap_or_default();
    if path != "/event" {
        return Err(404);
    }
    if req.method != "POST" {
        return Err(405);
    }
    let origins: Vec<&str> = req.header_all("Origin").collect();
    if origins.len() > 1 || origins.first().is_some_and(|o| !is_loopback_origin(o)) {
        return Err(403);
    }
    if req
        .header("Sec-Fetch-Site")
        .is_some_and(|v| v.eq_ignore_ascii_case("cross-site"))
    {
        return Err(403);
    }
    if req.header("Host").is_some_and(|h| !is_loopback_host(h)) {
        return Err(403);
    }
    let presented = req
        .header(TOKEN_HEADER)
        .map(str::to_string)
        .or_else(|| {
            req.header("Authorization")
                .and_then(|a| {
                    a.strip_prefix("Bearer ")
                        .or_else(|| a.strip_prefix("bearer "))
                })
                .map(str::to_string)
        })
        .unwrap_or_default();
    if !token.matches(&presented) {
        return Err(401);
    }
    Ok(())
}

fn query_param<'a>(target: &'a str, name: &str) -> Option<&'a str> {
    target.split_once('?')?.1.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == name).then_some(v)
    })
}

/// Claude profile folder of a transcript (`<profile>\projects\<project>\<id>.jsonl`).
pub fn profile_of_transcript(path: &Path) -> Option<PathBuf> {
    if path
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains("local-agent-mode-sessions")
    {
        return None;
    }
    path.ancestors()
        .skip(1)
        .find(|a| {
            a.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("projects"))
        })
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

/// Turn an accepted request into a session event (lenient: a truncated body still yields the
/// members read before the cut).
pub fn event_from_request(req: &Request) -> Option<SessionEvent> {
    let kind = HookEvent::from_arg(query_param(&req.target, "e").unwrap_or("ping"))?;
    let ppid = query_param(&req.target, "ppid")
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);
    let body = crate::json::parse_prefix_object(&req.body).unwrap_or_default();
    let get = |k: &str| body.iter().find(|(n, _)| n == k).map(|(_, v)| v);
    let s = |k: &str| get(k).and_then(|v| v.as_str()).map(str::to_string);
    let mut ev = SessionEvent::new(kind, Source::Hook, s("session_id").unwrap_or_default());
    ev.ppid = ppid;
    ev.cwd = s("cwd");
    ev.prompt = s("prompt");
    ev.message = s("message");
    ev.tool = s("tool_name");
    ev.command = get("tool_input")
        .and_then(|t| t.get("command"))
        .and_then(|c| c.as_str())
        .map(str::to_string);
    ev.model = s("model").or_else(|| {
        get("model")
            .and_then(|m| m.get("display_name").or_else(|| m.get("id")))
            .and_then(|v| v.as_str())
            .map(String::from)
    });
    ev.profile_dir = s("transcript_path").and_then(|p| profile_of_transcript(Path::new(&p)));
    Some(ev)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    }
}

fn respond(stream: &mut TcpStream, status: u16) {
    let body = if status == 200 { "ok" } else { reason(status) };
    let text = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        reason(status),
        body.len()
    );
    let _ = stream.write_all(text.as_bytes());
    let _ = stream.flush();
}

/// Callback receiving accepted events (from server threads).
pub type EventSink = Arc<dyn Fn(SessionEvent) + Send + Sync>;

fn handle(mut stream: TcpStream, token: &Token, sink: &EventSink) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let req = match read_request(&mut stream, Instant::now() + REQUEST_DEADLINE) {
        Ok(r) => r,
        Err(ReadError::Status(s)) => {
            log::debug!("hook server: rejected request ({s})");
            respond(&mut stream, s);
            return;
        }
        Err(ReadError::Io) => return,
    };
    if let Err(status) = check(&req, token) {
        log::debug!("hook server: rejected request ({status})");
        respond(&mut stream, status);
        return;
    }
    let ev = event_from_request(&req);
    respond(&mut stream, 200);
    if let Some(ev) = ev {
        sink(ev);
    }
}

/// A running listener.
pub struct ServerHandle {
    pub port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ServerHandle {
    pub fn stop(mut self) {
        self.shutdown();
    }
    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Bind `127.0.0.1:port` (0 = ephemeral) and serve on a background thread.
pub fn start(port: u16, token: Token, sink: EventSink) -> Result<ServerHandle, ServerStatus> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse || e.raw_os_error() == Some(10013) => {
            log::warn!("hook server: port {port} is already in use; hooks will not be received");
            return Err(ServerStatus::PortInUse(port));
        }
        Err(e) => {
            log::warn!("hook server: cannot listen on port {port}: {e}");
            return Err(ServerStatus::Failed(format!(
                "cannot listen on port {port}"
            )));
        }
    };
    let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    if listener.set_nonblocking(true).is_err() {
        return Err(ServerStatus::Failed("cannot configure the listener".into()));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let token = Arc::new(token);
    let active = Arc::new(AtomicUsize::new(0));
    let thread = std::thread::Builder::new()
        .name("codenotch-hook-server".into())
        .spawn(move || {
            log::info!("hook server listening on 127.0.0.1:{bound}");
            while !stop2.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, peer)) => {
                        if !peer.ip().is_loopback() {
                            continue;
                        }
                        let _ = stream.set_nonblocking(false);
                        if active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                            continue; // dropped: the hook gives up quickly
                        }
                        active.fetch_add(1, Ordering::SeqCst);
                        let (token, sink, active2) = (token.clone(), sink.clone(), active.clone());
                        let spawned = std::thread::Builder::new()
                            .name("codenotch-hook-conn".into())
                            .spawn(move || {
                                handle(stream, &token, &sink);
                                active2.fetch_sub(1, Ordering::SeqCst);
                            });
                        if spawned.is_err() {
                            active.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(ACCEPT_POLL)
                    }
                    Err(_) => std::thread::sleep(ACCEPT_POLL),
                }
            }
            log::info!("hook server stopped");
        })
        .map_err(|_| ServerStatus::Failed("cannot start the hook server thread".into()))?;
    Ok(ServerHandle {
        port: bound,
        stop,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn token() -> (tempfile::TempDir, Token) {
        let d = tempfile::tempdir().unwrap();
        let t = crate::token::load_or_create(&d.path().join("auth_token")).unwrap();
        (d, t)
    }

    fn req(raw: &[u8]) -> Result<Request, ReadError> {
        read_request(
            &mut std::io::Cursor::new(raw.to_vec()),
            Instant::now() + Duration::from_secs(5),
        )
    }

    fn post(token: &str, extra: &str, body: &str) -> Request {
        let raw = format!(
            "POST /event?e=running&ppid=42 HTTP/1.1\r\nHost: 127.0.0.1\r\n{TOKEN_HEADER}: {token}\r\n{extra}Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        req(raw.as_bytes()).unwrap()
    }

    #[test]
    fn parses_and_caps() {
        let r =
            req(b"POST /event?e=done&ppid=7 HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}extra").unwrap();
        assert_eq!(
            (r.method.as_str(), r.target.as_str(), r.body.as_slice()),
            ("POST", "/event?e=done&ppid=7", &b"{}"[..])
        );
        let big = format!(
            "POST /event HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_PAYLOAD + 1
        );
        assert_eq!(req(big.as_bytes()).unwrap_err(), ReadError::Status(413));
        let ok = format!(
            "POST /event HTTP/1.1\r\nContent-Length: {MAX_PAYLOAD}\r\n\r\n{}",
            "x".repeat(MAX_PAYLOAD)
        );
        assert_eq!(req(ok.as_bytes()).unwrap().body.len(), MAX_PAYLOAD);
        let huge_header = format!(
            "GET / HTTP/1.1\r\nX: {}\r\n\r\n",
            "a".repeat(MAX_HEADER + 10)
        );
        assert_eq!(
            req(huge_header.as_bytes()).unwrap_err(),
            ReadError::Status(431)
        );
        assert_eq!(
            req(b"POST /event HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap_err(),
            ReadError::Status(411)
        );
        assert_eq!(req(b"garbage\r\n\r\n").unwrap_err(), ReadError::Status(400));
        assert_eq!(
            req(b"POST /event HTTP/1.1\r\nContent-Length: 10\r\n\r\nshort").unwrap_err(),
            ReadError::Io
        );
    }

    #[test]
    fn checks_in_order() {
        let (_d, t) = token();
        let good = t.expose().to_string();
        assert_eq!(check(&post(&good, "", "{}"), &t), Ok(()));
        assert_eq!(check(&post("wrong", "", "{}"), &t), Err(401));
        assert_eq!(check(&post("", "", "{}"), &t), Err(401));
        let bearer =
            req(format!("POST /event HTTP/1.1\r\nAuthorization: Bearer {good}\r\n\r\n").as_bytes())
                .unwrap();
        assert_eq!(check(&bearer, &t), Ok(()));
        let get = req(format!("GET /event HTTP/1.1\r\n{TOKEN_HEADER}: {good}\r\n\r\n").as_bytes())
            .unwrap();
        assert_eq!(check(&get, &t), Err(405));
        let other = req(b"POST /other HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(check(&other, &t), Err(404));
        assert_eq!(
            check(&post(&good, "Origin: https://evil.example\r\n", ""), &t),
            Err(403)
        );
        assert_eq!(
            check(&post(&good, "Origin: http://localhost:3000\r\n", ""), &t),
            Ok(())
        );
        assert_eq!(check(&post(&good, "Origin: null\r\n", ""), &t), Err(403));
        assert_eq!(
            check(
                &post(
                    &good,
                    "Origin: http://127.0.0.1\r\nOrigin: http://127.0.0.1\r\n",
                    ""
                ),
                &t
            ),
            Err(403)
        );
        assert_eq!(
            check(&post(&good, "Sec-Fetch-Site: cross-site\r\n", ""), &t),
            Err(403)
        );
        // Origin is checked before the token (a browser cannot learn anything from a 401).
        assert_eq!(
            check(&post("wrong", "Origin: http://evil\r\n", ""), &t),
            Err(403)
        );
        let rebind = req(format!(
            "POST /event HTTP/1.1\r\nHost: evil.example:48666\r\n{TOKEN_HEADER}: {good}\r\n\r\n"
        )
        .as_bytes())
        .unwrap();
        assert_eq!(check(&rebind, &t), Err(403));
    }

    #[test]
    fn origins_and_hosts() {
        for ok in [
            "http://127.0.0.1",
            "https://localhost:8443",
            "http://[::1]:1",
            "http://LOCALHOST",
        ] {
            assert!(is_loopback_origin(ok), "{ok}");
        }
        for bad in [
            "http://127.0.0.1.evil.com",
            "http://localhost:99999",
            "ftp://localhost",
            "http://localhost/x",
            "http://a@localhost",
            "http://[::2]",
            "http://localhost:",
        ] {
            assert!(!is_loopback_origin(bad), "{bad}");
        }
    }

    #[test]
    fn event_fields_and_profile() {
        let body = r#"{"session_id":"abc123","transcript_path":"C:\\Users\\u\\.claude-work\\projects\\C--x\\abc123.jsonl","cwd":"C:\\x","tool_name":"Bash","tool_input":{"command":"ls -la"},"model":{"id":"claude-opus","display_name":"Opus"},"tool_response":"cut her"#;
        let r = post("t", "", body);
        let ev = event_from_request(&r).unwrap();
        assert_eq!(ev.kind, HookEvent::Running);
        assert_eq!(ev.ppid, 42);
        assert_eq!(ev.session_id, "abc123");
        assert_eq!(ev.command.as_deref(), Some("ls -la"));
        assert_eq!(ev.model.as_deref(), Some("Opus"));
        assert_eq!(
            ev.profile_dir,
            Some(PathBuf::from(r"C:\Users\u\.claude-work"))
        );
        let bad = req(b"POST /event?e=bogus HTTP/1.1\r\n\r\n").unwrap();
        assert!(event_from_request(&bad).is_none());
        let empty = req(b"POST /event?e=done HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(event_from_request(&empty).unwrap().session_id, "unknown");
        assert_eq!(
            profile_of_transcript(Path::new(
                r"C:\a\Claude\local-agent-mode-sessions\x\.claude\projects\p\s.jsonl"
            )),
            None
        );
    }

    #[test]
    fn live_server_on_ephemeral_port() {
        let (_d, t) = token();
        let good = t.expose().to_string();
        let got: Arc<Mutex<Vec<SessionEvent>>> = Arc::default();
        let g2 = got.clone();
        let h = start(0, t, Arc::new(move |e| g2.lock().unwrap().push(e))).unwrap();
        assert_ne!(h.port, 0);
        let send = |raw: String| {
            let mut s = TcpStream::connect(("127.0.0.1", h.port)).unwrap();
            s.write_all(raw.as_bytes()).unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out
        };
        let body = r#"{"session_id":"s1"}"#;
        let ok = send(format!(
            "POST /event?e=done&ppid=1 HTTP/1.1\r\nHost: 127.0.0.1\r\n{TOKEN_HEADER}: {good}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        let denied = send("POST /event?e=done HTTP/1.1\r\nContent-Length: 0\r\n\r\n".into());
        assert!(denied.starts_with("HTTP/1.1 401"));
        let deadline = Instant::now() + Duration::from_secs(2);
        while got.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got.lock().unwrap().len(), 1);
        // Second bind on the same port reports PortInUse.
        let (_d2, t2) = token();
        match start(h.port, t2, Arc::new(|_| {})) {
            Err(ServerStatus::PortInUse(p)) => assert_eq!(p, h.port),
            Err(other) => panic!("unexpected {other:?}"),
            Ok(_) => panic!("second bind should fail"),
        }
        let t0 = Instant::now();
        h.stop();
        assert!(t0.elapsed() < Duration::from_millis(500));
    }
}
