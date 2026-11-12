//! Blocking HTTP behind a trait so providers can be tested without the network.
//!
//! * [`UreqHttp`]: the real client (ureq 2, Windows SChannel TLS via native-tls, proxies from
//!   `HTTPS_PROXY`/`HTTP_PROXY`, no redirects, 20 s timeout, 2 MiB body cap).
//! * [`FakeHttp`]: test double with canned responses matched by URL prefix; records requests.
//!
//! Never log a request's headers: they carry bearer tokens. `HttpRequest`'s `Debug` redacts them.

use std::sync::Mutex;
use std::time::Duration;

/// Largest body read from any response.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Default request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// An outgoing request. Build with [`HttpRequest::get`] / [`HttpRequest::post`].
#[derive(Clone, PartialEq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }
    pub fn post(url: impl Into<String>) -> Self {
        Self {
            method: Method::Post,
            ..Self::get(url)
        }
    }
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    /// `Authorization: Bearer <token>`.
    pub fn bearer(self, token: &str) -> Self {
        self.header("Authorization", format!("Bearer {token}"))
    }
    /// JSON body + `Content-Type: application/json`.
    pub fn json(mut self, value: &serde_json::Value) -> Self {
        self.body = Some(value.to_string().into_bytes());
        self.header("Content-Type", "application/json")
    }
    /// Form body (`application/x-www-form-urlencoded`), already encoded by the caller.
    pub fn form(mut self, encoded: impl Into<String>) -> Self {
        self.body = Some(encoded.into().into_bytes());
        self.header("Content-Type", "application/x-www-form-urlencoded")
    }
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    /// Value of a request header (case-insensitive).
    pub fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(k, _)| k.as_str()).collect();
        // The path can be a secret (Telegram bot token, Discord webhook) and the query may hold
        // keys: print scheme + host (+ port) only.
        let url = url_origin(&self.url);
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &url)
            .field("headers", &names)
            .field("body_len", &self.body.as_ref().map(|b| b.len()))
            .finish()
    }
}

/// `scheme://host[:port]` of `url`: no user info, path, query or fragment. Safe to show or log
/// (error messages, `doctor`).
pub fn url_origin(url: &str) -> String {
    let url = url.trim();
    let (scheme, rest) = match url.find("://") {
        Some(i) => (&url[..i], &url[i + 3..]),
        None => ("", url),
    };
    let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    if scheme.is_empty() {
        host.to_string()
    } else {
        format!("{scheme}://{host}")
    }
}

/// A received response (any status).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> crate::Result<T> {
        Ok(serde_json::from_slice(&self.body)?)
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    /// `Retry-After` as seconds from now: integer seconds or an HTTP date (relative to `now`).
    pub fn retry_after_secs(&self, now: i64) -> Option<i64> {
        let raw = self.header("retry-after")?.trim();
        if let Ok(secs) = raw.parse::<i64>() {
            return Some(secs.max(0));
        }
        chrono::DateTime::parse_from_rfc2822(raw)
            .ok()
            .map(|t| (t.timestamp() - now).max(0))
    }
}

/// Transport-level failure (no HTTP status received).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    Timeout,
    /// DNS / connect / proxy failure.
    Connect(String),
    Tls(String),
    /// Body larger than [`MAX_BODY_BYTES`].
    TooLarge,
    /// URL rejected before sending (bad scheme, unparsable).
    BadUrl(String),
    Other(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Timeout => f.write_str("request timed out"),
            HttpError::Connect(m) => write!(f, "cannot connect: {m}"),
            HttpError::Tls(m) => write!(f, "TLS error: {m}"),
            HttpError::TooLarge => f.write_str("response is larger than 2 MB"),
            HttpError::BadUrl(m) => write!(f, "bad URL: {m}"),
            HttpError::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for HttpError {}

/// Blocking HTTP client. Implementations must be thread-safe; a non-2xx status is
/// `Ok(HttpResponse)`, only transport failures are `Err`.
pub trait HttpClient: Send + Sync {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError>;

    /// Stream a large response body into `sink` (no [`MAX_BODY_BYTES`] cap; at most
    /// `max_bytes`, else [`HttpError::TooLarge`]), calling `progress(received, total)` as it
    /// arrives (`total` from `Content-Length` when known). Only a 2xx body goes to `sink` and the
    /// returned response then has an empty body; any other status comes back like [`send`]
    /// (capped body, nothing written). Redirects are not followed.
    ///
    /// The default implementation goes through [`send`] (fine for test doubles).
    ///
    /// [`send`]: HttpClient::send
    fn download(
        &self,
        request: &HttpRequest,
        sink: &mut dyn std::io::Write,
        max_bytes: u64,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<HttpResponse, HttpError> {
        let response = self.send(request)?;
        if !response.is_success() {
            return Ok(response);
        }
        let n = response.body.len() as u64;
        if n > max_bytes {
            return Err(HttpError::TooLarge);
        }
        sink.write_all(&response.body)
            .map_err(|e| HttpError::Other(format!("cannot write the download: {}", e.kind())))?;
        progress(n, Some(n));
        Ok(HttpResponse {
            body: Vec::new(),
            ..response
        })
    }
}

/// The custom-endpoint URL rule: `https://` anywhere, `http://` only to localhost — exactly
/// `localhost`, `127.0.0.1` or `[::1]` (any case, optional numeric port), with no user-info
/// (`@`) in the authority. Other loopback spellings (`127.1`, `0x7f000001`, `localhost.`,
/// `[0:0:0:0:0:0:0:1]`, percent-escapes) are rejected rather than interpreted.
pub fn allowed_endpoint(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("https://") {
        let authority = authority_of(rest);
        let host = authority.rsplit('@').next().unwrap_or("");
        return !host.is_empty() && !host.starts_with(':');
    }
    if let Some(rest) = lower.strip_prefix("http://") {
        let authority = authority_of(rest);
        if authority.contains('@') {
            return false;
        }
        let Some((host, port)) = split_host_port(authority) else {
            return false;
        };
        let port_ok = port.is_none_or(|p| {
            p.is_empty()
                || (p.len() <= 5
                    && p.chars().all(|c| c.is_ascii_digit())
                    && p.parse::<u32>().is_ok_and(|n| n <= 65535))
        });
        return port_ok && matches!(host, "localhost" | "127.0.0.1" | "[::1]");
    }
    false
}

/// Authority part of a URL after `scheme://` (up to the first `/`, `?`, `#` or `\`).
fn authority_of(rest: &str) -> &str {
    rest.split(['/', '?', '#', '\\']).next().unwrap_or("")
}

/// `host` (IPv6 literals keep their brackets) and the optional port text of an authority
/// without user-info. `None` when malformed (unclosed bracket, junk after `]`, several `:`).
fn split_host_port(authority: &str) -> Option<(&str, Option<&str>)> {
    if authority.starts_with('[') {
        let close = authority.find(']')?;
        let (host, after) = authority.split_at(close + 1);
        return match after.strip_prefix(':') {
            Some(port) => Some((host, Some(port))),
            None if after.is_empty() => Some((host, None)),
            None => None,
        };
    }
    match authority.split_once(':') {
        Some((host, port)) if !port.contains(':') => Some((host, Some(port))),
        Some(_) => None,
        None => Some((authority, None)),
    }
}

/// Lower-case host of `url` without brackets, port, user-info or trailing dot ("" if none).
fn request_host(url: &str) -> String {
    let url = url.trim();
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = authority_of(rest).rsplit('@').next().unwrap_or("");
    let host = match split_host_port(authority) {
        Some((h, _)) => h,
        None => authority,
    };
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// Loopback for routing purposes (broad on purpose: a loopback request never goes to a proxy).
fn is_loopback_host(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback() || ip.is_unspecified(),
        Err(_) => false,
    }
}

/// `true` when a `NO_PROXY` list (comma/semicolon separated) excludes `host`: `*`, an exact
/// host or IP, a domain suffix (`example.com`, `.example.com`, `*.example.com`), or a prefix
/// wildcard (`10.*`). A `:port` on an entry is ignored.
fn no_proxy_matches(host: &str, no_proxy: &str) -> bool {
    no_proxy
        .split([',', ';'])
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .any(|entry| {
            let entry = entry.to_ascii_lowercase();
            if entry == "*" {
                return true;
            }
            let entry = entry.trim_start_matches('[');
            let entry = match entry.find(']') {
                Some(i) => &entry[..i],
                None if entry.matches(':').count() == 1 => entry.split(':').next().unwrap_or(""),
                None => entry,
            };
            let entry = entry.trim_end_matches('.');
            if let Some(prefix) = entry.strip_suffix('*') {
                return !prefix.is_empty() && host.starts_with(prefix);
            }
            let suffix = entry.trim_start_matches('*').trim_start_matches('.');
            !suffix.is_empty() && (host == suffix || host.ends_with(&format!(".{suffix}")))
        })
}

/// Which agent a request uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// Never through a proxy: loopback hosts and `NO_PROXY` matches.
    Direct,
    /// The environment proxy, if any is configured.
    Proxy,
}

/// Route for `url` given the `NO_PROXY` value (ureq 2 ignores `NO_PROXY` itself).
fn route_for(url: &str, no_proxy: &str) -> Route {
    let host = request_host(url);
    if host.is_empty() || is_loopback_host(&host) || no_proxy_matches(&host, no_proxy) {
        Route::Direct
    } else {
        Route::Proxy
    }
}

/// Map a ureq transport error to an [`HttpError`] whose text holds only the error kind and the
/// request's origin (`scheme://host[:port]`): never the transport error's own text, which
/// starts with the full URL (path, query and user-info can be secrets).
fn transport_error(t: &ureq::Transport, url: &str) -> HttpError {
    let origin = url_origin(url);
    let timed_out = {
        let mut source = std::error::Error::source(t);
        let mut found = t.message().is_some_and(|m| m.contains("timed out"));
        while let Some(err) = source {
            if let Some(io) = err.downcast_ref::<std::io::Error>() {
                found |= matches!(
                    io.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                );
            }
            source = err.source();
        }
        found
    };
    let kind = t.kind();
    let msg = format!("{kind} ({origin})");
    match kind {
        _ if timed_out => HttpError::Timeout,
        ureq::ErrorKind::ConnectionFailed if t.message() == Some("native_tls connect failed") => {
            HttpError::Tls(msg)
        }
        ureq::ErrorKind::Dns
        | ureq::ErrorKind::ConnectionFailed
        | ureq::ErrorKind::ProxyConnect => HttpError::Connect(msg),
        ureq::ErrorKind::InvalidUrl | ureq::ErrorKind::UnknownScheme => HttpError::BadUrl(msg),
        _ => HttpError::Other(msg),
    }
}

// ---------------------------------------------------------------------------------------------
// UreqHttp
// ---------------------------------------------------------------------------------------------

/// Real client: ureq 2 + native-tls (SChannel), proxy from environment, no redirects.
/// Loopback hosts and hosts listed in `NO_PROXY`/`no_proxy` always go direct (ureq 2 does
/// not read `NO_PROXY`).
pub struct UreqHttp {
    /// Proxy from `ALL_PROXY`/`HTTPS_PROXY`/`HTTP_PROXY` when set.
    agent: ureq::Agent,
    /// Never uses a proxy.
    direct: ureq::Agent,
    no_proxy: String,
}

impl UreqHttp {
    pub fn new() -> Self {
        let tls = match native_tls::TlsConnector::new() {
            Ok(connector) => Some(std::sync::Arc::new(connector)),
            Err(err) => {
                log::error!("native-tls init failed: {err}");
                None
            }
        };
        let build = |from_env: bool| {
            let mut builder = ureq::AgentBuilder::new()
                .redirects(0)
                .timeout_connect(Duration::from_secs(10))
                .try_proxy_from_env(from_env)
                .user_agent(concat!("CodeNotch-Windows/", env!("CARGO_PKG_VERSION")));
            if let Some(connector) = &tls {
                builder = builder.tls_connector(connector.clone());
            }
            builder.build()
        };
        let no_proxy = std::env::var("NO_PROXY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| std::env::var("no_proxy").ok())
            .unwrap_or_default();
        Self {
            agent: build(true),
            direct: build(false),
            no_proxy,
        }
    }

    fn agent_for(&self, url: &str) -> &ureq::Agent {
        match route_for(url, &self.no_proxy) {
            Route::Direct => &self.direct,
            Route::Proxy => &self.agent,
        }
    }
}

impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient for UreqHttp {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        use std::io::Read;
        let method = match req.method {
            Method::Get => "GET",
            Method::Post => "POST",
        };
        let mut request = self
            .agent_for(&req.url)
            .request(method, &req.url)
            .timeout(req.timeout);
        for (k, v) in &req.headers {
            request = request.set(k, v);
        }
        let result = match &req.body {
            Some(body) => request.send_bytes(body),
            None => request.call(),
        };
        let response = match result {
            Ok(resp) => resp,
            Err(ureq::Error::Status(_, resp)) => resp,
            Err(ureq::Error::Transport(t)) => return Err(transport_error(&t, &req.url)),
        };
        let status = response.status();
        let headers = response
            .headers_names()
            .into_iter()
            .filter_map(|name| {
                response
                    .header(&name)
                    .map(|v| (name.clone(), v.to_string()))
            })
            .collect();
        let mut body = Vec::new();
        response
            .into_reader()
            .take(MAX_BODY_BYTES as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => HttpError::Timeout,
                kind => HttpError::Other(format!(
                    "cannot read the response: {kind} ({})",
                    url_origin(&req.url)
                )),
            })?;
        if body.len() > MAX_BODY_BYTES {
            return Err(HttpError::TooLarge);
        }
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    fn download(
        &self,
        req: &HttpRequest,
        sink: &mut dyn std::io::Write,
        max_bytes: u64,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<HttpResponse, HttpError> {
        use std::io::Read;
        let mut request = self
            .agent_for(&req.url)
            .request("GET", &req.url)
            .timeout(req.timeout);
        for (k, v) in &req.headers {
            request = request.set(k, v);
        }
        let response = match request.call() {
            Ok(resp) => resp,
            Err(ureq::Error::Status(_, resp)) => resp,
            Err(ureq::Error::Transport(t)) => return Err(transport_error(&t, &req.url)),
        };
        let status = response.status();
        let headers: Vec<(String, String)> = response
            .headers_names()
            .into_iter()
            .filter_map(|name| {
                response
                    .header(&name)
                    .map(|v| (name.clone(), v.to_string()))
            })
            .collect();
        let read_err = |e: std::io::Error| match e.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => HttpError::Timeout,
            kind => HttpError::Other(format!(
                "cannot read the response: {kind} ({})",
                url_origin(&req.url)
            )),
        };
        if !(200..300).contains(&status) {
            let mut body = Vec::new();
            response
                .into_reader()
                .take(MAX_BODY_BYTES as u64)
                .read_to_end(&mut body)
                .map_err(read_err)?;
            return Ok(HttpResponse {
                status,
                headers,
                body,
            });
        }
        let total = response
            .header("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok());
        if total.is_some_and(|t| t > max_bytes) {
            return Err(HttpError::TooLarge);
        }
        let mut reader = response.into_reader().take(max_bytes + 1);
        let mut buf = vec![0u8; 64 * 1024];
        let mut received: u64 = 0;
        progress(0, total);
        loop {
            let n = reader.read(&mut buf).map_err(read_err)?;
            if n == 0 {
                break;
            }
            received += n as u64;
            if received > max_bytes {
                return Err(HttpError::TooLarge);
            }
            sink.write_all(&buf[..n]).map_err(|e| {
                HttpError::Other(format!("cannot write the download: {}", e.kind()))
            })?;
            progress(received, total);
        }
        Ok(HttpResponse {
            status,
            headers,
            body: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// FakeHttp
// ---------------------------------------------------------------------------------------------

/// Test double. Routes are matched by URL prefix, first registered wins; unmatched URLs return
/// `Err(HttpError::Connect("no fake route"))`. Every request is recorded.
#[derive(Default)]
pub struct FakeHttp {
    routes: Mutex<Vec<(String, Result<HttpResponse, HttpError>)>>,
    log: Mutex<Vec<HttpRequest>>,
}

impl FakeHttp {
    pub fn new() -> Self {
        Self::default()
    }
    /// Answer requests whose URL starts with `prefix` with `status` + `body`.
    pub fn on(self, prefix: &str, status: u16, body: &str) -> Self {
        self.on_response(prefix, HttpResponse::new(status, body.as_bytes().to_vec()))
    }
    pub fn on_response(self, prefix: &str, response: HttpResponse) -> Self {
        self.routes
            .lock()
            .unwrap()
            .push((prefix.to_string(), Ok(response)));
        self
    }
    pub fn on_error(self, prefix: &str, error: HttpError) -> Self {
        self.routes
            .lock()
            .unwrap()
            .push((prefix.to_string(), Err(error)));
        self
    }
    /// Requests received so far (tests only: they may contain test tokens).
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.log.lock().unwrap().clone()
    }
}

impl HttpClient for FakeHttp {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.log.lock().unwrap().push(request.clone());
        let routes = self.routes.lock().unwrap();
        routes
            .iter()
            .find(|(prefix, _)| request.url.starts_with(prefix.as_str()))
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| Err(HttpError::Connect("no fake route".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_routes_and_records() {
        let http = FakeHttp::new().on("https://api.example.com/usage", 200, r#"{"ok":true}"#);
        let resp = http
            .send(&HttpRequest::get("https://api.example.com/usage?x=1").bearer("t0k"))
            .unwrap();
        assert!(resp.is_success());
        assert_eq!(resp.json::<serde_json::Value>().unwrap()["ok"], true);
        assert!(http.send(&HttpRequest::get("https://other")).is_err());
        assert_eq!(http.requests().len(), 2);
        assert_eq!(
            http.requests()[0].header_value("authorization"),
            Some("Bearer t0k")
        );
    }

    #[test]
    fn request_debug_hides_header_values_and_query() {
        let req = HttpRequest::get("https://h/p?key=SECRET").bearer("SECRET");
        let dbg = format!("{req:?}");
        assert!(!dbg.contains("SECRET"));
        assert!(dbg.contains("Authorization"));
    }

    #[test]
    fn request_debug_hides_path_and_user_info() {
        let req = HttpRequest::post("https://api.telegram.org/bot123:SECRET/sendMessage?x=1#f");
        let dbg = format!("{req:?}");
        assert!(!dbg.contains("SECRET") && !dbg.contains("sendMessage") && !dbg.contains("bot"));
        assert!(dbg.contains("\"https://api.telegram.org\"") && dbg.contains("Post"));
        let dbg = format!(
            "{:?}",
            HttpRequest::get("https://discord.com/api/webhooks/1/SECRET")
        );
        assert!(
            !dbg.contains("SECRET")
                && !dbg.contains("webhooks")
                && dbg.contains("https://discord.com")
        );
        let dbg = format!(
            "{:?}",
            HttpRequest::get("http://user:SECRET@127.0.0.1:8080/p")
        );
        assert!(
            !dbg.contains("SECRET")
                && !dbg.contains("user")
                && dbg.contains("http://127.0.0.1:8080")
        );
        assert_eq!(url_origin("localhost:9/x"), "localhost:9");
    }

    #[test]
    fn retry_after_parses_seconds_and_dates() {
        let r = HttpResponse::new(429, "").with_header("Retry-After", "120");
        assert_eq!(r.retry_after_secs(0), Some(120));
        let r =
            HttpResponse::new(429, "").with_header("retry-after", "Thu, 01 Jan 1970 00:10:00 GMT");
        assert_eq!(r.retry_after_secs(0), Some(600));
        assert_eq!(HttpResponse::new(429, "").retry_after_secs(0), None);
    }

    #[test]
    fn endpoint_scheme_rules() {
        assert!(allowed_endpoint("https://example.com/usage"));
        assert!(allowed_endpoint("http://127.0.0.1:9/usage"));
        assert!(allowed_endpoint("http://localhost/usage"));
        assert!(allowed_endpoint("http://[::1]:8080/x"));
        assert!(!allowed_endpoint("http://example.com/usage"));
        assert!(!allowed_endpoint("http://localhost.evil.com/usage"));
        assert!(!allowed_endpoint("file:///tmp/usage.json"));
    }

    #[test]
    fn endpoint_rule_accepts_only_exact_loopback_spellings() {
        for ok in [
            "http://localhost",
            "http://localhost/",
            "http://LOCALHOST:8080/usage",
            "http://127.0.0.1",
            "http://127.0.0.1:65535/usage?x=1#f",
            "http://127.0.0.1:/usage",
            "http://[::1]",
            "http://[::1]:9/x",
            "HTTP://[::1]/x",
            "  http://localhost:3000/usage  ",
            "https://example.com",
            "https://user:pw@example.com/usage",
            "https://[2001:db8::1]:443/x",
        ] {
            assert!(allowed_endpoint(ok), "should accept {ok:?}");
        }
        for bad in [
            "http://127.0.0.1:1@evil.example/usage",
            "http://[::1]@evil.example/",
            "http://localhost@evil.example/",
            "http://user:pw@localhost/usage",
            "http://localhost:80@evil.example:80/",
            "http://evil.example#@localhost/",
            "http://evil.example?@localhost/",
            "http://127.1/usage",
            "http://0x7f000001/usage",
            "http://2130706433/usage",
            "http://0177.0.0.1/usage",
            "http://127.0.0.2/usage",
            "http://localhost./usage",
            "http://localhost../usage",
            "http://localhost.evil.com/usage",
            "http://evil.localhost/usage",
            "http://[0:0:0:0:0:0:0:1]/usage",
            "http://[::ffff:127.0.0.1]/usage",
            "http://[::1/usage",
            "http://[::1]x/usage",
            "http://[::1]:80:80/usage",
            "http://localhost:80:80/usage",
            "http://localhost:99999/usage",
            "http://localhost:8o/usage",
            "http://%6cocalhost/usage",
            "http://local\thost/usage",
            "http://127.0.0.1\t@evil.example/",
            "http://127.0.0.1 @evil.example/",
            "http://0.0.0.0/usage",
            "http://",
            "http:///localhost",
            "http:\\\\localhost\\usage",
            "https://",
            "https:///path",
            "https://user@/path",
            "ftp://localhost/x",
            "localhost/usage",
            "",
        ] {
            assert!(!allowed_endpoint(bad), "should reject {bad:?}");
        }
    }

    #[test]
    fn loopback_and_no_proxy_hosts_go_direct() {
        let np = "localhost,127.0.0.1,::1,corp.example,.internal.test,*.wild.test,10.*,proxy-free.test:8080";
        for url in [
            "http://127.0.0.1:8080/usage",
            "http://localhost/usage",
            "http://LOCALHOST./usage",
            "http://[::1]:9/x",
            "http://127.0.0.5/x",
            "http://user:pw@127.0.0.1:8080/x",
            "https://corp.example/x",
            "https://api.corp.example/x",
            "https://a.internal.test/x",
            "https://b.wild.test/x",
            "http://10.1.2.3/x",
            "https://proxy-free.test/x",
        ] {
            assert_eq!(route_for(url, np), Route::Direct, "{url}");
        }
        // Loopback never goes to the proxy, even with an empty NO_PROXY.
        assert_eq!(route_for("http://127.0.0.1:8080/usage", ""), Route::Direct);
        assert_eq!(route_for("http://localhost:8080/usage", ""), Route::Direct);
        assert_eq!(route_for("http://[::1]/usage", ""), Route::Direct);
        for url in [
            "https://api.anthropic.com/api/oauth/usage",
            "https://notcorp.example/x",
            "https://corp.example.evil.com/x",
            "http://127.0.0.1@evil.example/x",
            "https://110.1.2.3/x",
        ] {
            assert_eq!(route_for(url, np), Route::Proxy, "{url}");
        }
        assert_eq!(route_for("https://anything.example/x", "*"), Route::Direct);
    }

    #[test]
    fn transport_errors_carry_only_the_origin() {
        let http = UreqHttp::new();
        // Nothing listens on port 1: connection refused, loopback only (no network).
        let url = "http://alice:pw-S3CRET@127.0.0.1:1/v1/S3CRETPATH/usage?apikey=S3CRETQ&api_key=S3CRETQ2";
        let err = http
            .send(&HttpRequest::get(url).timeout(Duration::from_secs(10)))
            .unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(
            !text.contains("S3CRET") && !text.contains("alice") && !text.contains("usage"),
            "{text}"
        );
        assert!(text.contains("http://127.0.0.1:1"), "{text}");
        // Unparsable URL: rejected before any connection.
        let err = http
            .send(&HttpRequest::get(
                "http://bob:S3CRET@[zz/k/S3CRET?token=S3CRET",
            ))
            .unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(!text.contains("S3CRET") && !text.contains("bob"), "{text}");
    }
}
