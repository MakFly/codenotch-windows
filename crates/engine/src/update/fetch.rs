//! Reading the manifest and downloading the package: `https://` through the [`HttpClient`]
//! (redirects followed, never to a non-https URL), or a local file (`file://` URL or plain
//! path) when allowed — tests, debug builds and an explicit `CODENOTCH_UPDATE_URL`.

use super::{MSG_BAD_URL, MSG_NO_MANIFEST, MSG_RATE_LIMIT};
use codenotch_core::{HttpClient, HttpRequest, HttpResponse};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

/// Redirects followed at most (GitHub release assets redirect once).
const MAX_REDIRECTS: usize = 5;
/// Manifest request timeout.
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Whole-download timeout.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Where a URL points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Https(String),
    File(PathBuf),
}

/// Classify `url`: `https://…`, or (only when `allow_file`) `file://…` / an absolute path.
pub fn classify(url: &str, allow_file: bool) -> Result<Source, String> {
    let url = url.trim();
    if url.chars().any(|c| c.is_control()) || url.is_empty() {
        return Err(MSG_BAD_URL.into());
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        let host = codenotch_core::http::url_origin(url);
        return if host.len() > "https://".len() {
            Ok(Source::Https(url.to_string()))
        } else {
            Err(MSG_BAD_URL.into())
        };
    }
    if !allow_file {
        return Err(MSG_BAD_URL.into());
    }
    if lower.starts_with("file://") {
        let rest = &url["file://".len()..];
        // file:///C:/x → C:/x ; file://C:/x → C:/x
        let rest = rest
            .strip_prefix('/')
            .filter(|r| has_drive(r))
            .unwrap_or(rest);
        let path = percent_decode(rest);
        return if has_drive(&path) || path.starts_with("\\\\") {
            Ok(Source::File(PathBuf::from(path.replace('/', "\\"))))
        } else {
            Err(MSG_BAD_URL.into())
        };
    }
    let path = PathBuf::from(url);
    if path.is_absolute() || has_drive(url) {
        Ok(Source::File(path))
    } else {
        Err(MSG_BAD_URL.into())
    }
}

fn has_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn status_error(response: &HttpResponse) -> String {
    match response.status {
        404 => MSG_NO_MANIFEST.into(),
        403 | 429 => MSG_RATE_LIMIT.into(),
        code => format!("HTTP {code}"),
    }
}

/// The next URL of a redirect response, `None` when it is not one. Errors when the target is
/// not https.
fn redirect_target(response: &HttpResponse, from: &str) -> Option<Result<String, String>> {
    if !matches!(response.status, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let location = response.header("location")?.trim().to_string();
    let next = if location.starts_with('/') && !location.starts_with("//") {
        format!("{}{location}", codenotch_core::http::url_origin(from))
    } else {
        location
    };
    Some(if next.to_ascii_lowercase().starts_with("https://") {
        Ok(next)
    } else {
        Err(MSG_BAD_URL.to_string())
    })
}

/// Call `send` for `url`, then for each redirect target.
fn follow(
    url: &str,
    mut send: impl FnMut(&str) -> Result<HttpResponse, String>,
) -> Result<HttpResponse, String> {
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let response = send(&current)?;
        match redirect_target(&response, &current) {
            None => return Ok(response),
            Some(Ok(next)) => {
                log::debug!(
                    "update: redirected to {}",
                    codenotch_core::http::url_origin(&next)
                );
                current = next;
            }
            Some(Err(e)) => return Err(e),
        }
    }
    Err("too many redirects".into())
}

/// The manifest bytes at `url` (at most 2 MB).
pub fn get_manifest(http: &dyn HttpClient, url: &str, allow_file: bool) -> Result<Vec<u8>, String> {
    match classify(url, allow_file)? {
        Source::File(path) => {
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .and_then(|f| {
                    f.take(codenotch_core::http::MAX_BODY_BYTES as u64)
                        .read_to_end(&mut bytes)
                })
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::NotFound => MSG_NO_MANIFEST.to_string(),
                    kind => format!("cannot read the update file: {kind}"),
                })?;
            Ok(bytes)
        }
        Source::Https(url) => {
            let response = follow(&url, |u| {
                let request = HttpRequest::get(u)
                    .header("Accept", "application/json")
                    .header("Cache-Control", "no-cache")
                    .timeout(MANIFEST_TIMEOUT);
                http.send(&request).map_err(|e| e.to_string())
            })?;
            if !response.is_success() {
                return Err(status_error(&response));
            }
            Ok(response.body)
        }
    }
}

/// Download `url` into `sink` (at most `max_bytes`), reporting `(received, total)`. Returns
/// the number of bytes written.
pub fn download(
    http: &dyn HttpClient,
    url: &str,
    allow_file: bool,
    sink: &mut dyn Write,
    max_bytes: u64,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<u64, String> {
    match classify(url, allow_file)? {
        Source::File(path) => {
            let mut file = std::fs::File::open(&path)
                .map_err(|e| format!("cannot read the update package: {}", e.kind()))?;
            let total = file.metadata().ok().map(|m| m.len());
            if total.is_some_and(|t| t > max_bytes) {
                return Err(super::MSG_DOWNLOAD_SIZE.into());
            }
            let mut buf = vec![0u8; 64 * 1024];
            let mut done = 0u64;
            progress(0, total);
            loop {
                let n = file
                    .read(&mut buf)
                    .map_err(|e| format!("cannot read the update package: {}", e.kind()))?;
                if n == 0 {
                    break;
                }
                done += n as u64;
                if done > max_bytes {
                    return Err(super::MSG_DOWNLOAD_SIZE.into());
                }
                sink.write_all(&buf[..n])
                    .map_err(|e| format!("cannot write the download: {}", e.kind()))?;
                progress(done, total);
            }
            Ok(done)
        }
        Source::Https(url) => {
            let mut written = 0u64;
            let response = {
                let mut counting = CountingWriter {
                    inner: sink,
                    count: &mut written,
                };
                follow(&url, |u| {
                    let request = HttpRequest::get(u)
                        .header("Accept", "application/octet-stream")
                        .timeout(DOWNLOAD_TIMEOUT);
                    http.download(&request, &mut counting, max_bytes, progress)
                        .map_err(|e| match e {
                            codenotch_core::HttpError::TooLarge => {
                                super::MSG_DOWNLOAD_SIZE.to_string()
                            }
                            other => other.to_string(),
                        })
                })?
            };
            if !response.is_success() {
                return Err(match response.status {
                    404 => "the update package was not found (HTTP 404)".to_string(),
                    _ => status_error(&response),
                });
            }
            Ok(written)
        }
    }
}

struct CountingWriter<'a> {
    inner: &'a mut dyn Write,
    count: &'a mut u64,
}

impl Write for CountingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        *self.count += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::FakeHttp;

    #[test]
    fn url_rules() {
        assert_eq!(
            classify(
                " https://raw.githubusercontent.com/a/b/main/update.json ",
                false
            ),
            Ok(Source::Https(
                "https://raw.githubusercontent.com/a/b/main/update.json".into()
            ))
        );
        for bad in [
            "http://example.com/u.json",
            "ftp://x/y",
            "https://",
            "",
            "update.json",
            "file:///C:/x/update.json",
            r"C:\x\update.json",
        ] {
            assert!(classify(bad, false).is_err(), "{bad}");
        }
        assert_eq!(
            classify("file:///C:/Temp/my%20dir/update.json", true),
            Ok(Source::File(PathBuf::from(r"C:\Temp\my dir\update.json")))
        );
        assert_eq!(
            classify(r"C:\Temp\update.json", true),
            Ok(Source::File(PathBuf::from(r"C:\Temp\update.json")))
        );
        assert!(classify("file://relative/x", true).is_err());
        assert!(classify("http://localhost/x", true).is_err());
    }

    #[test]
    fn manifest_over_https_follows_redirects_but_never_to_http() {
        let http = FakeHttp::new()
            .on_response(
                "https://a.example/update.json",
                HttpResponse::new(302, "").with_header("Location", "https://b.example/u.json"),
            )
            .on("https://b.example/u.json", 200, r#"{"version":"1.0.0"}"#)
            .on_response(
                "https://c.example/",
                HttpResponse::new(301, "").with_header("Location", "http://d.example/u.json"),
            )
            .on("https://e.example/", 404, "nope");
        assert_eq!(
            get_manifest(&http, "https://a.example/update.json", false).unwrap(),
            br#"{"version":"1.0.0"}"#
        );
        assert_eq!(
            get_manifest(&http, "https://c.example/x", false).unwrap_err(),
            MSG_BAD_URL
        );
        assert_eq!(
            get_manifest(&http, "https://e.example/x", false).unwrap_err(),
            MSG_NO_MANIFEST
        );
        assert!(http
            .requests()
            .iter()
            .all(|r| r.header_value("cache-control") == Some("no-cache")));
    }

    #[test]
    fn local_files_when_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.json");
        std::fs::write(&path, b"{}").unwrap();
        let http = FakeHttp::new();
        let url = format!("file:///{}", path.display().to_string().replace('\\', "/"));
        assert_eq!(get_manifest(&http, &url, true).unwrap(), b"{}");
        assert!(get_manifest(&http, &url, false).is_err());
        let missing = dir.path().join("none.json");
        assert_eq!(
            get_manifest(&http, &missing.display().to_string(), true).unwrap_err(),
            MSG_NO_MANIFEST
        );
        assert!(http.requests().is_empty());

        // Download from a file, with progress and a size cap.
        let mut out = Vec::new();
        let mut seen = Vec::new();
        let n = download(&http, &url, true, &mut out, 10, &mut |d, t| {
            seen.push((d, t))
        })
        .unwrap();
        assert_eq!((n, out.as_slice()), (2, b"{}".as_slice()));
        assert_eq!(seen.last(), Some(&(2, Some(2))));
        let mut out = Vec::new();
        assert!(download(&http, &url, true, &mut out, 1, &mut |_, _| {}).is_err());
    }

    #[test]
    fn https_download_through_the_client() {
        let http = FakeHttp::new()
            .on_response(
                "https://github.com/o/r/releases/download/v1/p.zip",
                HttpResponse::new(302, "").with_header(
                    "location",
                    "https://objects.githubusercontent.com/p.zip?sig=1",
                ),
            )
            .on(
                "https://objects.githubusercontent.com/p.zip",
                200,
                "PKZIPDATA",
            )
            .on("https://missing.example/", 404, "");
        let mut out = Vec::new();
        let n = download(
            &http,
            "https://github.com/o/r/releases/download/v1/p.zip",
            false,
            &mut out,
            100,
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(n, 9);
        assert_eq!(out, b"PKZIPDATA");
        let mut out = Vec::new();
        assert!(download(
            &http,
            "https://missing.example/p.zip",
            false,
            &mut out,
            100,
            &mut |_, _| {}
        )
        .unwrap_err()
        .contains("404"));
        assert!(out.is_empty());
        let mut out = Vec::new();
        assert_eq!(
            download(
                &http,
                "https://github.com/o/r/releases/download/v1/p.zip",
                false,
                &mut out,
                3,
                &mut |_, _| {}
            )
            .unwrap_err(),
            super::super::MSG_DOWNLOAD_SIZE
        );
    }
}
