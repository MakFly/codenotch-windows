//! Opening URLs and folders (ShellExecuteW), system proxy and UI language.

use crate::reg::Key;
use crate::win::{wide, wide_os};
use codenotch_core::{Error, Lang, Result};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
use windows_sys::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

// ---------------------------------------------------------------------------------------------
// URLs
// ---------------------------------------------------------------------------------------------

/// Accept only `http://` / `https://` URLs with a host, no user-info, no whitespace, control
/// characters, quotes or backslashes. Returns the trimmed URL.
pub fn checked_url(url: &str) -> Result<&str> {
    let url = url.trim();
    let bad = || Error::other("only http and https links can be opened");
    let (scheme, rest) = url.split_once("://").ok_or_else(bad)?;
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return Err(bad());
    }
    if url
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '\\' | '<' | '>' | '`'))
    {
        return Err(bad());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || authority.starts_with(':') {
        return Err(bad());
    }
    Ok(url)
}

/// Run ShellExecuteW("open", target) on a short-lived STA thread (the shell may use COM) and
/// wait up to 10 s for it to return.
fn shell_open(target: Vec<u16>) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("codenotch-shell".into())
        .spawn(move || {
            // SAFETY: initialise COM on this new thread; balanced by CoUninitialize below.
            let hr = unsafe {
                CoInitializeEx(
                    std::ptr::null(),
                    (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
                )
            };
            let verb = wide("open");
            // SAFETY: NUL-terminated verb and target that outlive the call; no parameters.
            let r = unsafe {
                ShellExecuteW(
                    std::ptr::null_mut(),
                    verb.as_ptr(),
                    target.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    SW_SHOWNORMAL,
                )
            } as isize;
            if hr >= 0 {
                // SAFETY: matches the successful CoInitializeEx above.
                unsafe { CoUninitialize() };
            }
            let _ = tx.send(r);
        })
        .map_err(|e| Error::io("cannot start the shell thread", e))?;
    match rx.recv_timeout(Duration::from_secs(10)) {
        // ShellExecuteW: values above 32 mean success.
        Ok(r) if r > 32 => Ok(()),
        Ok(r) => Err(Error::other(format!(
            "the system could not open it (code {r})"
        ))),
        // Still launching (slow handler): assume it will appear.
        Err(_) => Ok(()),
    }
}

pub(crate) fn open_url(url: &str) -> Result<()> {
    let url = checked_url(url)?;
    shell_open(wide(url))
}

/// Folder to show for `path`: the folder itself, or the parent of a file (files are never
/// opened/executed).
pub fn folder_for(path: &Path) -> Result<&Path> {
    let meta = std::fs::metadata(path).map_err(|e| Error::io("cannot open folder", e))?;
    if meta.is_dir() {
        Ok(path)
    } else {
        path.parent()
            .ok_or_else(|| Error::other("cannot open folder"))
    }
}

pub(crate) fn open_path(path: &Path) -> Result<()> {
    let folder = folder_for(path)?;
    shell_open(wide_os(folder.as_os_str()))
}

// ---------------------------------------------------------------------------------------------
// Proxy
// ---------------------------------------------------------------------------------------------

const PROXY_VARS: [&str; 4] = ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"];
const INTERNET_SETTINGS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";

/// Proxy URL from WinINet settings (data-spec §0): `ProxyServer` is `host:port` or
/// `http=..;https=..` (the `https=` entry wins, then `http=`); `http://` is prefixed when no
/// scheme is given. `None` when disabled or unusable.
pub fn proxy_from_settings(enabled: bool, server: &str) -> Option<String> {
    if !enabled {
        return None;
    }
    let server = server.trim();
    if server.is_empty() {
        return None;
    }
    let pick = if server.contains('=') {
        let entries: Vec<(String, &str)> = server
            .split(';')
            .filter_map(|e| e.split_once('='))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim()))
            .filter(|(_, v)| !v.is_empty())
            .collect();
        let find = |k: &str| entries.iter().find(|(key, _)| key == k).map(|(_, v)| *v);
        find("https").or_else(|| find("http"))?
    } else {
        server.split(';').next().unwrap_or("").trim()
    };
    if pick.is_empty() {
        return None;
    }
    Some(if pick.contains("://") {
        pick.to_string()
    } else {
        format!("http://{pick}")
    })
}

/// `NO_PROXY` value from `ProxyOverride` ("<local>;*.corp.example") plus loopback.
pub fn no_proxy_from_override(over: &str) -> String {
    let mut list = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    for item in over
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "<local>")
    {
        let item = item
            .trim_start_matches("*.")
            .trim_start_matches('*')
            .to_string();
        if !item.is_empty() && !list.contains(&item) {
            list.push(item);
        }
    }
    list.join(",")
}

/// If no proxy env var is set, copy the WinINet proxy (HKCU Internet Settings) into
/// `HTTPS_PROXY`/`HTTP_PROXY` (and `NO_PROXY` when unset). Call early in `main`, before any
/// thread starts.
pub(crate) fn apply_system_proxy_env() {
    if PROXY_VARS
        .iter()
        .any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()))
    {
        return;
    }
    let Some(key) = Key::open_read(INTERNET_SETTINGS) else {
        return;
    };
    let enabled = key.get_dword("ProxyEnable").unwrap_or(0) == 1;
    let server = key.get_string("ProxyServer").unwrap_or_default();
    let Some(proxy) = proxy_from_settings(enabled, &server) else {
        return;
    };
    std::env::set_var("HTTPS_PROXY", &proxy);
    std::env::set_var("HTTP_PROXY", &proxy);
    if std::env::var_os("NO_PROXY").is_none() && std::env::var_os("no_proxy").is_none() {
        let over = key.get_string("ProxyOverride").unwrap_or_default();
        std::env::set_var("NO_PROXY", no_proxy_from_override(&over));
    }
    // Host only: a proxy URL may carry credentials.
    let host = proxy
        .split("://")
        .nth(1)
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("");
    log::info!("using the system proxy {host}");
}

// ---------------------------------------------------------------------------------------------
// Language
// ---------------------------------------------------------------------------------------------

/// Primary language id of a Windows LANGID.
pub fn lang_from_langid(langid: u16) -> Lang {
    const LANG_FRENCH: u16 = 0x0C;
    if langid & 0x3FF == LANG_FRENCH {
        Lang::Fr
    } else {
        Lang::En
    }
}

pub(crate) fn system_lang() -> Lang {
    // SAFETY: no preconditions.
    let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    lang_from_langid(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_scheme_filter() {
        assert_eq!(
            checked_url(" https://claude.ai/settings/usage ").unwrap(),
            "https://claude.ai/settings/usage"
        );
        assert!(checked_url("HTTP://example.com").is_ok());
        assert!(checked_url("https://chatgpt.com/#settings/Account").is_ok());
        assert!(checked_url("https://grok.com/?_s=usage").is_ok());
        assert!(checked_url("https://localhost:8080/x").is_ok());
        for bad in [
            "file:///C:/Windows/System32/calc.exe",
            "C:\\Windows\\notepad.exe",
            "javascript:alert(1)",
            "ms-settings:privacy",
            "mailto:a@b.c",
            "https://",
            "https:///path",
            "https://user:pw@evil.example/",
            "https://a b.com",
            "https://example.com/\"onload",
            "https:\\\\server\\share",
            "https://exa\\mple.com",
            "ftp://example.com",
            "http//example.com",
            "",
        ] {
            assert!(checked_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn folder_for_files_is_the_parent() {
        let dir = std::env::temp_dir();
        assert_eq!(folder_for(&dir).unwrap(), dir.as_path());
        let exe = std::env::current_exe().unwrap();
        assert_eq!(folder_for(&exe).unwrap(), exe.parent().unwrap());
        assert!(folder_for(Path::new("Z:\\definitely\\missing\\path")).is_err());
    }

    #[test]
    fn proxy_parsing() {
        assert_eq!(proxy_from_settings(false, "proxy:8080"), None);
        assert_eq!(proxy_from_settings(true, ""), None);
        assert_eq!(
            proxy_from_settings(true, "proxy.corp:8080").as_deref(),
            Some("http://proxy.corp:8080")
        );
        assert_eq!(
            proxy_from_settings(true, "http://p:3128").as_deref(),
            Some("http://p:3128")
        );
        assert_eq!(
            proxy_from_settings(true, "http=h1:80;https=h2:443;ftp=f:21").as_deref(),
            Some("http://h2:443")
        );
        assert_eq!(
            proxy_from_settings(true, "ftp=f:21;http=h1:80").as_deref(),
            Some("http://h1:80")
        );
        assert_eq!(proxy_from_settings(true, "socks=s:1080"), None);
        assert_eq!(
            proxy_from_settings(true, "HTTPS=h2:443").as_deref(),
            Some("http://h2:443")
        );
    }

    #[test]
    fn no_proxy_list() {
        assert_eq!(no_proxy_from_override(""), "localhost,127.0.0.1,::1");
        assert_eq!(
            no_proxy_from_override("<local>;*.corp.example;10.*"),
            "localhost,127.0.0.1,::1,corp.example,10.*"
        );
    }

    #[test]
    fn langid_mapping() {
        assert_eq!(lang_from_langid(0x040C), Lang::Fr); // fr-FR
        assert_eq!(lang_from_langid(0x0C0C), Lang::Fr); // fr-CA
        assert_eq!(lang_from_langid(0x0409), Lang::En);
        assert_eq!(lang_from_langid(0x0407), Lang::En); // de-DE → English UI
        let _ = system_lang();
    }
}
