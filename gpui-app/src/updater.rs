//! Signed in-app updates, compatible with the Tauri builds: the same release feed, the same
//! minisign key, and the same NSIS installer flags (`/P /UPDATE /R`), so 0.1.x installs move
//! to this app and this app updates itself afterwards.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;

const FEED: &str = "https://github.com/MakFly/codenotch-windows/releases/latest/download/latest.json";
/// The updater public key from `src-tauri/tauri.conf.json`.
const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEU5OTZDMDU4N0JCNERDMTIKUldRUzNMUjdXTUNXNlZhS0Q3QkNwblVTaHcrVGFvSmZhT2xWazlNNnpCSHNmbmZVTDloR1lGM1MK";
/// The identifier the installer's shortcuts carry, which Windows needs to show a toast.
pub const APP_ID: &str = "dev.kev.smashtoken";

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    url: String,
    signature: String,
}

#[derive(Deserialize)]
struct Feed {
    version: String,
    platforms: std::collections::HashMap<String, Download>,
}

#[derive(Deserialize)]
struct Download {
    url: String,
    signature: String,
}

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// "0.2.0" as numbers; a pre-release suffix is ignored.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0)))
}

/// Asks the release feed for a newer build. Runs on the tokio runtime.
pub async fn check() -> Result<Option<Release>, String> {
    let error = "Impossible de vérifier les mises à jour. Vérifie la connexion, puis réessaie.";
    let feed: Feed = reqwest::Client::new()
        .get(FEED)
        .header("User-Agent", concat!("CodeNotch/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|_| error)?
        .json()
        .await
        .map_err(|_| error)?;
    if parse(&feed.version).zip(parse(current())).is_none_or(|(new, old)| new <= old) {
        return Ok(None);
    }
    let download = feed
        .platforms
        .get("windows-x86_64-nsis")
        .or_else(|| feed.platforms.get("windows-x86_64"))
        .ok_or(error)?;
    Ok(Some(Release { version: feed.version, url: download.url.clone(), signature: download.signature.clone() }))
}

/// Downloads the installer and checks its signature, including the version it was signed
/// for, so a tampered feed cannot pair a new number with an older build.
pub async fn download(release: Release) -> Result<std::path::PathBuf, String> {
    let error = "Échec du téléchargement ou de la vérification de la mise à jour.";
    let bytes = reqwest::Client::new()
        .get(&release.url)
        .header("User-Agent", concat!("CodeNotch/", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|_| error)?
        .bytes()
        .await
        .map_err(|_| error)?;
    verify(&bytes, &release).map_err(|_| error)?;
    let path = std::env::temp_dir().join(format!("CodeNotch-{}-setup.exe", release.version));
    tokio::fs::write(&path, &bytes).await.map_err(|_| error)?;
    Ok(path)
}

fn verify(bytes: &[u8], release: &Release) -> Result<(), ()> {
    let text = |b64: &str| STANDARD.decode(b64).ok().and_then(|b| String::from_utf8(b).ok()).ok_or(());
    let key = minisign_verify::PublicKey::decode(&text(PUBLIC_KEY)?).map_err(|_| ())?;
    let signature = minisign_verify::Signature::decode(&text(&release.signature)?).map_err(|_| ())?;
    key.verify(bytes, &signature, true).map_err(|_| ())?;
    let signed = signature.trusted_comment().split('\t').find_map(|f| f.strip_prefix("version:"));
    match signed.and_then(parse) {
        Some(version) if Some(version) == parse(&release.version) => Ok(()),
        _ => Err(()),
    }
}

/// Starts the installer the way the Tauri updater did: passive, as an update, relaunching
/// CodeNotch when done. The caller quits right after so the files can be replaced.
#[cfg(windows)]
pub fn run_installer(path: &std::path::Path) -> Result<(), String> {
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOW};
    let wide = |s: &std::ffi::OsStr| {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>()
    };
    let (verb, file, args) = (wide("open".as_ref()), wide(path.as_os_str()), wide("/P /UPDATE /R".as_ref()));
    let result = unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), args.as_ptr(), std::ptr::null(), SW_SHOW)
    };
    if result as isize <= 32 {
        return Err("Impossible de lancer l’installation de la mise à jour.".into());
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run_installer(_: &std::path::Path) -> Result<(), String> {
    Err("Impossible de lancer l’installation de la mise à jour.".into())
}

/// A Windows notification, once per version. Installed builds have a Start menu shortcut
/// carrying `APP_ID`; a build run from elsewhere borrows PowerShell's so the toast still shows.
#[cfg(windows)]
pub fn notify(version: &str, french: bool) {
    let installed = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("uninstall.exe").exists()))
        .unwrap_or(false);
    let id = if installed { APP_ID } else { tauri_winrt_notification::Toast::POWERSHELL_APP_ID };
    let (title, body) = if french {
        (format!("CodeNotch {version} est disponible"), "Ouvre les réglages pour mettre à jour.")
    } else {
        (format!("CodeNotch {version} is available"), "Open Settings to update.")
    };
    let _ = tauri_winrt_notification::Toast::new(id).title(&title).text1(body).show();
}

#[cfg(not(windows))]
pub fn notify(_: &str, _: bool) {}
