//! The updates folder (`<data dir>\updates`): one sub-folder per downloaded version.
//!
//! ```text
//! updates\
//!   1.1.0\
//!     package.zip.part   while downloading (deleted once unpacked or on any error)
//!     files\codenotch.exe, files\codenotch-hook.exe   unpacked, verified package
//!     ready.json         {"version","sha256"}: written last, marks the folder complete
//!   pending.json         {"from","to"}: an install was started (read at the next start)
//! ```

use super::fetch;
use super::manifest::Offer;
use super::swap::{APP_EXE, PACKAGE_FILES};
use super::version::Version;
use super::zip;
use super::{MSG_BAD_PACKAGE, MSG_CHECKSUM, MSG_DOWNLOAD_SIZE};
use codenotch_core::HttpClient;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Largest package accepted.
pub const MAX_PACKAGE: u64 = 256 * 1024 * 1024;
const READY: &str = "ready.json";
const PENDING: &str = "pending.json";
const PART: &str = "package.zip.part";
const FILES: &str = "files";

/// A complete, verified, unpacked package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    pub version: String,
    pub sha256: String,
    /// Folder holding the exes (not serialised: derived from the location).
    #[serde(skip)]
    pub files: PathBuf,
}

/// An install in progress: written just before the swap, read at the next start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub from: String,
    pub to: String,
}

/// Folder of `version` (only `[0-9A-Za-z.+-]` kept, so a manifest cannot point elsewhere).
pub fn version_dir(work: &Path, version: &str) -> PathBuf {
    let safe: String = version
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        .collect();
    let safe = safe.trim_matches('.');
    work.join(if safe.is_empty() { "unknown" } else { safe })
}

fn read_ready(dir: &Path) -> Option<Staged> {
    let text = std::fs::read(dir.join(READY)).ok()?;
    let mut staged: Staged = serde_json::from_slice(&text).ok()?;
    staged.files = dir.join(FILES);
    staged.files.join(APP_EXE).is_file().then_some(staged)
}

/// The newest complete package of a version above `current`, if any.
pub fn find_staged(work: &Path, current: &str) -> Option<Staged> {
    let mine = Version::parse(current)?;
    std::fs::read_dir(work)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| read_ready(&e.path()))
        .filter_map(|s| Some((Version::parse(&s.version)?, s)))
        .filter(|(v, _)| *v > mine)
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, s)| s)
}

/// Delete every version folder except `keep`'s, and stray files (not `pending.json`).
pub fn cleanup(work: &Path, keep: Option<&str>) {
    let keep = keep.map(|v| version_dir(work, v));
    let Ok(read) = std::fs::read_dir(work) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if Some(&path) == keep.as_ref() || entry.file_name() == PENDING {
            continue;
        }
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(err) = removed {
            log::debug!("update: cannot remove {}: {}", path.display(), err.kind());
        }
    }
}

pub fn write_pending(work: &Path, pending: &Pending) -> std::io::Result<()> {
    std::fs::create_dir_all(work)?;
    let text = serde_json::to_vec(pending).map_err(std::io::Error::other)?;
    std::fs::write(work.join(PENDING), text)
}

pub fn clear_pending(work: &Path) {
    let _ = std::fs::remove_file(work.join(PENDING));
}

/// Read and delete the pending marker.
pub fn take_pending(work: &Path) -> Option<Pending> {
    let path = work.join(PENDING);
    let text = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_slice(&text).ok()
}

/// Writes to a file and hashes what it writes.
struct HashWriter<W: Write> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Download `offer`'s package into `work`, check its size and SHA-256, unpack the exes and
/// mark the folder ready. Any failure deletes the version folder.
pub fn stage_offer(
    http: &dyn HttpClient,
    offer: &Offer,
    work: &Path,
    allow_file: bool,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Staged, String> {
    let dir = version_dir(work, &offer.version);
    let result = stage_into(http, offer, &dir, allow_file, progress);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

fn stage_into(
    http: &dyn HttpClient,
    offer: &Offer,
    dir: &Path,
    allow_file: bool,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Staged, String> {
    let asset = &offer.asset;
    let expected = asset.sha256_bytes().ok_or(MSG_BAD_PACKAGE)?;
    if asset.size == 0 || asset.size > MAX_PACKAGE {
        return Err(MSG_BAD_PACKAGE.into());
    }
    if dir.exists() {
        std::fs::remove_dir_all(dir)
            .map_err(|e| format!("cannot prepare the download: {}", e.kind()))?;
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot prepare the download: {}", e.kind()))?;
    let part = dir.join(PART);
    let file = std::fs::File::create(&part)
        .map_err(|e| format!("cannot write the download: {}", e.kind()))?;
    let mut sink = HashWriter {
        inner: std::io::BufWriter::new(file),
        hasher: Sha256::new(),
    };
    let size = fetch::download(
        http, &asset.url, allow_file, &mut sink, asset.size, progress,
    )?;
    sink.flush()
        .map_err(|e| format!("cannot write the download: {}", e.kind()))?;
    let digest = sink.hasher.finalize();
    drop(sink.inner);
    if size != asset.size {
        log::warn!("update: {size} bytes downloaded, {} expected", asset.size);
        return Err(MSG_DOWNLOAD_SIZE.into());
    }
    if digest.as_slice() != expected {
        log::warn!(
            "update: SHA-256 {} does not match the manifest's {}",
            hex(&digest),
            asset.sha256.trim()
        );
        return Err(MSG_CHECKSUM.into());
    }
    let bytes =
        std::fs::read(&part).map_err(|e| format!("cannot read the download: {}", e.kind()))?;
    let files = dir.join(FILES);
    let done = zip::extract_named(&bytes, &PACKAGE_FILES, &files).map_err(|e| {
        log::warn!("update: {e}");
        MSG_BAD_PACKAGE.to_string()
    })?;
    if !done.iter().any(|n| n == APP_EXE) {
        return Err(MSG_BAD_PACKAGE.into());
    }
    for name in &done {
        let head = std::fs::read(files.join(name)).unwrap_or_default();
        if !head.starts_with(b"MZ") {
            log::warn!("update: {name} in the package is not a Windows program");
            return Err(MSG_BAD_PACKAGE.into());
        }
    }
    let _ = std::fs::remove_file(&part);
    let staged = Staged {
        version: offer.version.clone(),
        sha256: hex(&digest),
        files,
    };
    let text = serde_json::to_vec_pretty(&staged).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(READY), text)
        .map_err(|e| format!("cannot write the download: {}", e.kind()))?;
    Ok(staged)
}

/// SHA-256 of `bytes` as lower-case hex (tests, tools).
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Test package: a release-shaped zip with a fake app and hook.
#[cfg(test)]
pub(crate) fn tests_package() -> Vec<u8> {
    zip::build(&[
        ("CodeNotch-1.1.0/codenotch.exe", b"MZ new app", true),
        ("CodeNotch-1.1.0/codenotch-hook.exe", b"MZ new hook", true),
        ("CodeNotch-1.1.0/LICENSE", b"MIT", true),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::manifest::Asset;
    use codenotch_core::{FakeHttp, HttpResponse};

    fn package() -> Vec<u8> {
        tests_package()
    }

    fn offer(url: &str, bytes: &[u8]) -> Offer {
        Offer {
            version: "1.1.0".into(),
            notes: String::new(),
            notes_url: None,
            forced: false,
            asset: Asset {
                url: url.into(),
                sha256: sha256_hex(bytes).to_uppercase(),
                size: bytes.len() as u64,
            },
        }
    }

    const URL: &str = "https://example.com/CodeNotch-1.1.0.zip";

    #[test]
    fn download_verify_and_unpack() {
        let zip = package();
        let http = FakeHttp::new().on_response(URL, HttpResponse::new(200, zip.clone()));
        let work = tempfile::tempdir().unwrap();
        let mut last = (0, None);
        let staged = stage_offer(&http, &offer(URL, &zip), work.path(), false, &mut |d, t| {
            last = (d, t)
        })
        .unwrap();
        assert_eq!(last, (zip.len() as u64, Some(zip.len() as u64)));
        assert_eq!(staged.version, "1.1.0");
        assert_eq!(
            std::fs::read(staged.files.join("codenotch.exe")).unwrap(),
            b"MZ new app"
        );
        assert!(staged.files.join("codenotch-hook.exe").is_file());
        assert!(!staged.files.join("LICENSE").exists());
        assert!(!version_dir(work.path(), "1.1.0").join(PART).exists());
        // Found again at the next start (only when newer than the running version).
        assert_eq!(find_staged(work.path(), "1.0.0"), Some(staged.clone()));
        assert_eq!(find_staged(work.path(), "1.1.0"), None);
        // Cleanup keeps it when asked, deletes it otherwise.
        std::fs::create_dir_all(work.path().join("0.9.0")).unwrap();
        cleanup(work.path(), Some("1.1.0"));
        assert!(!work.path().join("0.9.0").exists());
        assert!(staged.files.is_dir());
        cleanup(work.path(), None);
        assert!(!staged.files.exists());
    }

    #[test]
    fn sha_mismatch_is_refused_and_the_file_deleted() {
        let zip = package();
        let http = FakeHttp::new().on_response(URL, HttpResponse::new(200, zip.clone()));
        let work = tempfile::tempdir().unwrap();
        let mut bad = offer(URL, &zip);
        bad.asset.sha256 = sha256_hex(b"something else");
        let err = stage_offer(&http, &bad, work.path(), false, &mut |_, _| {}).unwrap_err();
        assert_eq!(err, MSG_CHECKSUM);
        assert!(!version_dir(work.path(), "1.1.0").exists(), "nothing kept");
        assert_eq!(find_staged(work.path(), "1.0.0"), None);
    }

    #[test]
    fn size_mismatch_and_bad_packages_are_refused() {
        let zip = package();
        let http = FakeHttp::new()
            .on_response(URL, HttpResponse::new(200, zip.clone()))
            .on("https://example.com/notzip", 200, "MZ not a zip");
        let work = tempfile::tempdir().unwrap();
        let mut bigger = offer(URL, &zip);
        bigger.asset.size += 1;
        assert_eq!(
            stage_offer(&http, &bigger, work.path(), false, &mut |_, _| {}).unwrap_err(),
            MSG_DOWNLOAD_SIZE
        );
        let mut smaller = offer(URL, &zip);
        smaller.asset.size -= 1;
        assert_eq!(
            stage_offer(&http, &smaller, work.path(), false, &mut |_, _| {}).unwrap_err(),
            MSG_DOWNLOAD_SIZE
        );
        let not_zip = offer("https://example.com/notzip", b"MZ not a zip");
        assert_eq!(
            stage_offer(&http, &not_zip, work.path(), false, &mut |_, _| {}).unwrap_err(),
            MSG_BAD_PACKAGE
        );
        // A zip without the app, or whose "exe" is not a program.
        for z in [
            zip::build(&[("codenotch-hook.exe", b"MZ", false)]),
            zip::build(&[("codenotch.exe", b"#!/bin/sh", false)]),
        ] {
            let http = FakeHttp::new().on_response(URL, HttpResponse::new(200, z.clone()));
            assert_eq!(
                stage_offer(&http, &offer(URL, &z), work.path(), false, &mut |_, _| {})
                    .unwrap_err(),
                MSG_BAD_PACKAGE
            );
        }
        // Plain http is never fetched.
        let plain = offer("http://example.com/p.zip", &zip);
        assert!(stage_offer(&http, &plain, work.path(), false, &mut |_, _| {}).is_err());
        let mut bad_sha = offer(URL, &zip);
        bad_sha.asset.sha256 = "zz".into();
        assert_eq!(
            stage_offer(&http, &bad_sha, work.path(), false, &mut |_, _| {}).unwrap_err(),
            MSG_BAD_PACKAGE
        );
        assert_eq!(std::fs::read_dir(work.path()).unwrap().count(), 0);
    }

    #[test]
    fn pending_marker_round_trip_and_version_dir_is_contained() {
        let work = tempfile::tempdir().unwrap();
        let p = Pending {
            from: "1.0.0".into(),
            to: "1.1.0".into(),
        };
        write_pending(work.path(), &p).unwrap();
        cleanup(work.path(), None);
        assert_eq!(take_pending(work.path()), Some(p));
        assert_eq!(take_pending(work.path()), None);
        assert_eq!(
            version_dir(work.path(), "../../Windows"),
            work.path().join("Windows")
        );
        assert_eq!(version_dir(work.path(), ".."), work.path().join("unknown"));
    }
}
