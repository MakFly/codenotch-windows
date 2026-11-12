//! The update manifest (`update.json`): what the latest version is, where its Windows package
//! is and whether the running version must update.
//!
//! ```json
//! { "version": "1.1.0", "min_version": "1.0.0", "published_at": "2026-11-12",
//!   "notes": "…", "notes_url": "https://github.com/…/releases/tag/v1.1.0",
//!   "windows_x64": { "url": "https://github.com/…/CodeNotch-1.1.0.zip",
//!                    "sha256": "…", "size": 12345678 } }
//! ```
//! One channel per file; unknown fields are ignored.

use super::version::Version;
use super::{MSG_NO_PACKAGE, MSG_NO_VERSION, MSG_UNREADABLE};
use serde::Deserialize;

/// Longest release notes kept (characters).
const NOTES_MAX: usize = 2000;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub min_version: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub notes_url: Option<String>,
    #[serde(default)]
    pub windows_x64: Option<Asset>,
}

/// The Windows package: a zip holding `codenotch.exe` and `codenotch-hook.exe`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Asset {
    pub url: String,
    /// Lower- or upper-case hex SHA-256 of the zip.
    pub sha256: String,
    /// Exact size of the zip in bytes.
    pub size: u64,
}

impl Asset {
    /// The SHA-256 as 32 bytes, `None` when it is not 64 hex digits.
    pub fn sha256_bytes(&self) -> Option<[u8; 32]> {
        let hex = self.sha256.trim();
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(out)
    }
}

/// A newer version on offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// Normalised (`v` prefix dropped).
    pub version: String,
    pub notes: String,
    pub notes_url: Option<String>,
    /// The running version is below `min_version`.
    pub forced: bool,
    pub asset: Asset,
}

/// Result of comparing a manifest with the running version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    UpToDate,
    Available(Offer),
}

/// Parse manifest bytes (UTF-8, a BOM is ignored).
pub fn parse(bytes: &[u8]) -> Result<Manifest, String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| MSG_UNREADABLE.to_string())?;
    if !value.get("version").is_some_and(|v| v.is_string()) {
        return Err(MSG_NO_VERSION.to_string());
    }
    serde_json::from_value(value).map_err(|_| MSG_UNREADABLE.to_string())
}

fn short(text: &str) -> String {
    text.chars().take(40).collect()
}

/// Compare `manifest` with `current` (strict semver). A newer version needs a Windows package;
/// `forced` when `current < min_version`.
pub fn evaluate(manifest: &Manifest, current: &str) -> Result<Outcome, String> {
    let latest = Version::parse(&manifest.version)
        .ok_or_else(|| format!("unknown version {}", short(&manifest.version)))?;
    let mine =
        Version::parse(current).ok_or_else(|| format!("unknown version {}", short(current)))?;
    let min = match manifest
        .min_version
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        Some(m) => Some(Version::parse(m).ok_or_else(|| format!("unknown version {}", short(m)))?),
        None => None,
    };
    if latest <= mine {
        return Ok(Outcome::UpToDate);
    }
    let asset = manifest
        .windows_x64
        .clone()
        .ok_or_else(|| MSG_NO_PACKAGE.to_string())?;
    let notes: String = manifest
        .notes
        .as_deref()
        .unwrap_or("")
        .trim()
        .chars()
        .take(NOTES_MAX)
        .collect();
    let notes_url = manifest
        .notes_url
        .as_deref()
        .map(str::trim)
        .filter(|u| u.to_ascii_lowercase().starts_with("https://"))
        .map(str::to_string);
    Ok(Outcome::Available(Offer {
        version: latest.to_string(),
        notes,
        notes_url,
        forced: min.is_some_and(|m| mine < m),
        asset,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    fn manifest(version: &str, min: Option<&str>) -> String {
        let min = min.map_or(String::new(), |m| format!(r#""min_version":"{m}","#));
        format!(
            r#"{{"version":"{version}",{min}"published_at":"2026-11-12","notes":"Faster.","notes_url":"https://github.com/MakFly/codenotch-windows/releases/tag/v{version}",
               "windows_x64":{{"url":"https://example.com/CodeNotch-{version}.zip","sha256":"{SHA}","size":1234}},
               "future_field":{{"x":1}},"linux":{{}}}}"#
        )
    }

    #[test]
    fn parses_and_ignores_unknown_fields() {
        let m = parse(manifest("1.1.0", Some("1.0.0")).as_bytes()).unwrap();
        assert_eq!(m.version, "1.1.0");
        assert_eq!(m.min_version.as_deref(), Some("1.0.0"));
        assert_eq!(m.published_at.as_deref(), Some("2026-11-12"));
        let asset = m.windows_x64.unwrap();
        assert_eq!(asset.size, 1234);
        assert_eq!(asset.sha256_bytes().unwrap()[0], 0x9f);
        // BOM, minimal manifest.
        let m = parse(b"\xEF\xBB\xBF{\"version\":\"2.0.0\"}").unwrap();
        assert!(m.windows_x64.is_none() && m.notes.is_none());
        // Errors.
        assert_eq!(parse(b"not json").unwrap_err(), MSG_UNREADABLE);
        assert_eq!(parse(b"{\"tag\":\"1.0\"}").unwrap_err(), MSG_NO_VERSION);
        assert_eq!(parse(b"{\"version\":3}").unwrap_err(), MSG_NO_VERSION);
        assert_eq!(
            parse(br#"{"version":"1.0.0","windows_x64":{"url":"x"}}"#).unwrap_err(),
            MSG_UNREADABLE
        );
    }

    #[test]
    fn version_compare_and_forced() {
        let m = |v: &str, min: Option<&str>| parse(manifest(v, min).as_bytes()).unwrap();
        assert_eq!(evaluate(&m("1.0.0", None), "1.0.0"), Ok(Outcome::UpToDate));
        assert_eq!(evaluate(&m("1.0.0", None), "1.2.0"), Ok(Outcome::UpToDate));
        assert_eq!(
            evaluate(&m("1.0.0", None), "1.0.1-rc.1"),
            Ok(Outcome::UpToDate),
            "a pre-release of a later version is newer"
        );
        let Ok(Outcome::Available(offer)) = evaluate(&m("v1.1.0", Some("1.0.0")), "1.0.0") else {
            panic!("available")
        };
        assert_eq!(offer.version, "1.1.0");
        assert!(!offer.forced, "1.0.0 is not below min 1.0.0");
        assert_eq!(offer.notes, "Faster.");
        assert!(offer.notes_url.unwrap().starts_with("https://github.com/"));
        let Ok(Outcome::Available(offer)) = evaluate(&m("1.1.0", Some("1.0.1")), "1.0.0") else {
            panic!("available")
        };
        assert!(offer.forced);
        // Release beats its own pre-release.
        assert!(matches!(
            evaluate(&m("1.1.0", None), "1.1.0-rc.2"),
            Ok(Outcome::Available(_))
        ));
        // min above latest with nothing newer: nothing to install.
        assert_eq!(
            evaluate(&m("1.0.0", Some("9.0.0")), "1.0.0"),
            Ok(Outcome::UpToDate)
        );
    }

    #[test]
    fn bad_versions_and_missing_package() {
        let bad = parse(br#"{"version":"latest"}"#).unwrap();
        assert!(evaluate(&bad, "1.0.0")
            .unwrap_err()
            .contains("unknown version"));
        let m = parse(manifest("1.1.0", Some("one")).as_bytes()).unwrap();
        assert!(evaluate(&m, "1.0.0").is_err());
        let no_pkg = parse(br#"{"version":"1.1.0"}"#).unwrap();
        assert_eq!(evaluate(&no_pkg, "1.0.0").unwrap_err(), MSG_NO_PACKAGE);
        // …but an up-to-date manifest needs no package.
        assert_eq!(evaluate(&no_pkg, "1.1.0"), Ok(Outcome::UpToDate));
    }

    #[test]
    fn notes_url_must_be_https_and_sha_must_be_hex() {
        let m = parse(
            br#"{"version":"2.0.0","notes_url":"javascript:alert(1)",
                 "windows_x64":{"url":"https://e/x.zip","sha256":"abc","size":1}}"#,
        )
        .unwrap();
        let Ok(Outcome::Available(offer)) = evaluate(&m, "1.0.0") else {
            panic!()
        };
        assert_eq!(offer.notes_url, None);
        assert_eq!(offer.asset.sha256_bytes(), None);
    }
}
