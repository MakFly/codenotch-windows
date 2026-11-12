//! Self-update.
//!
//! * [`manifest`] — the `update.json` manifest ([`DEFAULT_UPDATE_MANIFEST_URL`] by default,
//!   setting `updateManifestUrl`, env `CODENOTCH_UPDATE_URL`) and the version comparison.
//! * [`fetch`] — https only (redirects followed), local files only for tests / debug builds /
//!   `CODENOTCH_UPDATE_URL`.
//! * [`stage`] — download to `<data dir>\updates\<version>\`, size + SHA-256 check, unpack
//!   `codenotch.exe` + `codenotch-hook.exe` ([`zip`]).
//! * [`swap`] — rename the running exes to `.old`, move the new ones in, roll back on failure.
//! * [`service`] — the [`Updater`] thread tying it together (modes, idle install, relaunch).
//!
//! Nothing from a download is ever run except our own exe, once it replaced the old one.
//!
//! [`DEFAULT_UPDATE_MANIFEST_URL`]: codenotch_core::DEFAULT_UPDATE_MANIFEST_URL

pub mod fetch;
pub mod manifest;
pub mod service;
pub mod stage;
pub mod swap;
pub mod version;
pub mod zip;

pub use manifest::{Asset, Manifest, Offer, Outcome};
pub use service::{relaunch_args, spawn_relaunch, Relaunch, Report, Updater, UpdaterSetup};
pub use swap::Retry;
pub use version::Version;

/// `<owner>/<repo>` of the workspace `repository` URL.
pub const UPDATE_REPO: &str = "MakFly/codenotch-windows";

/// Manifest URL override (tests, local dry runs). Also allows `file://` URLs and paths.
pub const UPDATE_URL_ENV: &str = "CODENOTCH_UPDATE_URL";
/// `1` = allow self-update of an exe running from a cargo `target\` folder.
pub const ALLOW_DEV_ENV: &str = "CODENOTCH_UPDATE_ALLOW_DEV";
/// Debug builds only: the version the updater believes it runs (end-to-end dry runs). Never
/// passed on to the relaunched exe.
pub const PRETEND_VERSION_ENV: &str = "CODENOTCH_UPDATE_PRETEND_VERSION";

// `UpdateState::Failed { message }` holds the reason (English key, French in
// `crate::i18n::FR`); the UI shows it translated.
pub(crate) const MSG_NO_MANIFEST: &str = "No update information published yet.";
pub(crate) const MSG_RATE_LIMIT: &str = "GitHub rate limit reached, try again later.";
pub(crate) const MSG_UNREADABLE: &str = "unreadable answer";
pub(crate) const MSG_NO_VERSION: &str = "no version in the answer";
pub(crate) const MSG_NOT_STARTED: &str = "the check could not start";
pub(crate) const MSG_BAD_URL: &str = "The update address must start with https://.";
pub(crate) const MSG_NO_PACKAGE: &str = "No Windows package in the update information.";
pub(crate) const MSG_DOWNLOAD_SIZE: &str = "The download does not have the expected size.";
pub(crate) const MSG_CHECKSUM: &str = "The download is corrupted (checksum mismatch).";
pub(crate) const MSG_BAD_PACKAGE: &str = "The update package is not valid.";
pub(crate) const MSG_DEV_BUILD: &str =
    "Self-update is off for development builds (running from a target folder).";
pub(crate) const MSG_NO_INSTALL_DIR: &str = "The install folder cannot be found.";
pub(crate) const MSG_INSTALL_FAILED: &str =
    "The update could not be installed; the current version was kept.";
pub(crate) const MSG_RESTART_FAILED: &str =
    "The new version could not be started; the current version was restored.";
pub(crate) const MSG_NOT_TAKEN: &str = "The last update did not take effect.";

/// Every failure reason above (for the translation table test).
#[cfg(test)]
pub(crate) const MESSAGES: [&str; 15] = [
    MSG_NO_MANIFEST,
    MSG_RATE_LIMIT,
    MSG_UNREADABLE,
    MSG_NO_VERSION,
    MSG_NOT_STARTED,
    MSG_BAD_URL,
    MSG_NO_PACKAGE,
    MSG_DOWNLOAD_SIZE,
    MSG_CHECKSUM,
    MSG_BAD_PACKAGE,
    MSG_DEV_BUILD,
    MSG_NO_INSTALL_DIR,
    MSG_INSTALL_FAILED,
    MSG_RESTART_FAILED,
    MSG_NOT_TAKEN,
];

/// A short, practically unique token for file names (time + pid).
pub(crate) fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", std::process::id(), nanos & 0xFFFF_FFFF)
}

/// The version the updater compares with: the build's, or (debug builds only) the
/// [`PRETEND_VERSION_ENV`] override.
pub fn current_version(env: &codenotch_core::Env) -> String {
    if cfg!(debug_assertions) {
        if let Some(v) = env
            .var(PRETEND_VERSION_ENV)
            .filter(|v| Version::parse(v).is_some())
        {
            log::warn!("update: pretending to run version {v} ({PRETEND_VERSION_ENV})");
            return v;
        }
    }
    codenotch_core::VERSION.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_matches_the_workspace_repository() {
        let repo = env!("CARGO_PKG_REPOSITORY");
        assert!(
            repo.trim_end_matches('/')
                .ends_with(&format!("github.com/{UPDATE_REPO}")),
            "{repo}"
        );
        assert!(codenotch_core::DEFAULT_UPDATE_MANIFEST_URL
            .starts_with(&format!("https://raw.githubusercontent.com/{UPDATE_REPO}/")));
    }

    #[test]
    fn the_repository_manifest_is_valid() {
        // `update.json` at the repo root is what the default URL serves once pushed.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../update.json");
        let bytes = std::fs::read(path).expect("update.json at the repo root");
        let m = manifest::parse(&bytes).unwrap();
        assert!(Version::parse(&m.version).is_some());
        assert_eq!(
            manifest::evaluate(&m, &m.version),
            Ok(Outcome::UpToDate),
            "the manifest's own version is up to date"
        );
        let asset = m.windows_x64.expect("windows_x64");
        assert!(asset.url.starts_with("https://"));
    }

    #[test]
    fn pretend_version_only_when_valid() {
        let env = codenotch_core::Env::for_test(std::path::Path::new("."));
        assert_eq!(current_version(&env), codenotch_core::VERSION);
        let env = env.with_var(PRETEND_VERSION_ENV, "0.9.0");
        assert_eq!(current_version(&env), "0.9.0", "debug test build");
        let env = codenotch_core::Env::for_test(std::path::Path::new("."))
            .with_var(PRETEND_VERSION_ENV, "nope");
        assert_eq!(current_version(&env), codenotch_core::VERSION);
    }
}
