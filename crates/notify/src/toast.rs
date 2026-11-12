//! On-screen Windows notifications ("toasts").
//!
//! The backend is a small trait, [`ToastSink`], so tests inject a [`RecordingToast`] and never
//! show anything on the desktop. The real backend, [`WinToast`], uses WinRT
//! (`Windows.UI.Notifications`) through the `windows` crate.
//!
//! An unpackaged desktop app needs an AppUserModelID (AUMID) that Windows knows about before it
//! may show toasts. [`WinToast`] registers one per user, on first use, the way Microsoft
//! documents for unpackaged apps: a key `HKCU\Software\Classes\AppUserModelId\<AUMID>` with a
//! `DisplayName` (and optionally an `IconUri`). No admin rights, no shortcut, nothing outside
//! that one key. It never runs in tests (tests never build a `WinToast`).

use codenotch_core::{Error, Result};
use std::sync::Mutex;

/// AppUserModelID used for CodeNotch toasts (also the registry key name).
pub const APP_USER_MODEL_ID: &str = "dev.kev.CodeNotch";

/// Something that can put a notification on screen. Implementations must be thread-safe and
/// must return an error (never fail silently) when the notification could not be shown.
pub trait ToastSink: Send + Sync {
    /// Show `title` and `body` (body may contain `\n`). Returns once the request is handed to
    /// the OS (does not wait for the user).
    fn show(&self, title: &str, body: &str) -> Result<()>;
}

/// Test double: records every toast; optionally fails with a fixed message.
#[derive(Debug, Default)]
pub struct RecordingToast {
    shown: Mutex<Vec<(String, String)>>,
    fail_with: Option<String>,
}

impl RecordingToast {
    pub fn new() -> Self {
        Self::default()
    }
    /// A sink whose `show` always fails with `message` (still recorded).
    pub fn failing(message: &str) -> Self {
        Self {
            shown: Mutex::new(Vec::new()),
            fail_with: Some(message.to_string()),
        }
    }
    /// `(title, body)` of every toast requested so far.
    pub fn shown(&self) -> Vec<(String, String)> {
        self.shown.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

impl ToastSink for RecordingToast {
    fn show(&self, title: &str, body: &str) -> Result<()> {
        if let Ok(mut v) = self.shown.lock() {
            v.push((title.to_string(), body.to_string()));
        }
        match &self.fail_with {
            Some(msg) => Err(Error::other(msg.clone())),
            None => Ok(()),
        }
    }
}

/// Escape text for an XML text node / attribute.
pub(crate) fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Control characters are not allowed in XML 1.0 (except tab/newline/CR).
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// Toast XML (ToastGeneric: at most three text lines — title + two body lines).
pub(crate) fn toast_xml(title: &str, body: &str) -> String {
    let mut lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let rest = if lines.len() > 2 {
        lines.split_off(1).join(" · ")
    } else {
        String::new()
    };
    if !rest.is_empty() {
        lines.push(&rest);
    }
    let mut xml = String::from(r#"<toast><visual><binding template="ToastGeneric">"#);
    xml.push_str(&format!("<text>{}</text>", xml_escape(title)));
    for line in lines {
        xml.push_str(&format!("<text>{}</text>", xml_escape(line)));
    }
    xml.push_str("</binding></visual></toast>");
    xml
}

#[cfg(windows)]
pub use win::WinToast;

#[cfg(windows)]
mod win {
    use super::{toast_xml, ToastSink, APP_USER_MODEL_ID};
    use codenotch_core::{Error, Result};
    use std::sync::OnceLock;

    /// Real WinRT toast backend. Construct only in the running app (never in tests): the first
    /// `show` registers the AppUserModelID under `HKCU\Software\Classes\AppUserModelId`.
    #[derive(Debug)]
    pub struct WinToast {
        app_id: String,
        display_name: String,
        icon_path: Option<String>,
        registered: OnceLock<std::result::Result<(), String>>,
    }

    impl WinToast {
        /// Toasts under [`APP_USER_MODEL_ID`], shown as "CodeNotch".
        pub fn new() -> Self {
            Self::with_app_id(APP_USER_MODEL_ID, "CodeNotch")
        }
        pub fn with_app_id(app_id: &str, display_name: &str) -> Self {
            Self {
                app_id: app_id.to_string(),
                display_name: display_name.to_string(),
                icon_path: None,
                registered: OnceLock::new(),
            }
        }
        /// Absolute path of a PNG/ICO shown as the app icon in the toast (registry `IconUri`).
        pub fn icon(mut self, path: impl Into<String>) -> Self {
            self.icon_path = Some(path.into());
            self
        }
        pub fn app_id(&self) -> &str {
            &self.app_id
        }
    }

    impl Default for WinToast {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ToastSink for WinToast {
        fn show(&self, title: &str, body: &str) -> Result<()> {
            let reg = self.registered.get_or_init(|| {
                register_app_id(&self.app_id, &self.display_name, self.icon_path.as_deref())
            });
            if let Err(msg) = reg {
                return Err(Error::other(format!(
                    "Windows notifications unavailable: {msg}"
                )));
            }
            show_winrt(&self.app_id, &toast_xml(title, body))
        }
    }

    fn show_winrt(app_id: &str, xml: &str) -> Result<()> {
        use windows::core::HSTRING;
        use windows::Data::Xml::Dom::XmlDocument;
        use windows::UI::Notifications::{
            NotificationSetting, ToastNotification, ToastNotificationManager,
        };

        let fail = |what: &str, e: windows::core::Error| {
            Error::other(format!(
                "Windows notification failed ({what}, 0x{:08X})",
                e.code().0 as u32
            ))
        };
        let doc = XmlDocument::new().map_err(|e| fail("xml", e))?;
        doc.LoadXml(&HSTRING::from(xml))
            .map_err(|e| fail("xml", e))?;
        let toast =
            ToastNotification::CreateToastNotification(&doc).map_err(|e| fail("create", e))?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))
            .map_err(|e| fail("notifier", e))?;
        if let Ok(setting) = notifier.Setting() {
            if setting != NotificationSetting::Enabled {
                return Err(Error::other(
                    "Windows notifications are turned off for CodeNotch (or by Focus / group policy)",
                ));
            }
        }
        notifier.Show(&toast).map_err(|e| fail("show", e))
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Create/refresh `HKCU\Software\Classes\AppUserModelId\<app_id>` (DisplayName, IconUri).
    fn register_app_id(
        app_id: &str,
        display_name: &str,
        icon: Option<&str>,
    ) -> std::result::Result<(), String> {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
            REG_OPTION_NON_VOLATILE, REG_SZ,
        };
        if app_id.is_empty() || app_id.contains(['\\', '/']) {
            return Err("invalid application id".into());
        }
        let subkey = wide(&format!("Software\\Classes\\AppUserModelId\\{app_id}"));
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated UTF-16 strings; out-pointer to a local HKEY.
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            return Err(format!("cannot register the app id (error {rc})"));
        }
        let set = |name: &str, value: &str| -> u32 {
            let name = wide(name);
            let data = wide(value);
            // SAFETY: `key` is open; data is a NUL-terminated UTF-16 buffer of the given size.
            unsafe {
                RegSetValueExW(
                    key,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr().cast::<u8>(),
                    (data.len() * 2) as u32,
                )
            }
        };
        let mut rc = set("DisplayName", display_name);
        if rc == 0 {
            if let Some(icon) = icon.filter(|p| !p.is_empty()) {
                rc = set("IconUri", icon);
            }
        }
        // SAFETY: `key` was opened above and is closed once.
        unsafe {
            RegCloseKey(key);
        }
        if rc != 0 {
            return Err(format!("cannot register the app id (error {rc})"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_is_escaped_and_limited_to_three_lines() {
        let xml = toast_xml("A <b> & \"c\"", "line 1\nline 2\nline 3");
        assert!(xml.contains("<text>A &lt;b&gt; &amp; &quot;c&quot;</text>"));
        assert_eq!(xml.matches("<text>").count(), 3);
        assert!(xml.contains("<text>line 2 · line 3</text>"));
        assert_eq!(xml_escape("a\u{1}b"), "ab");
    }

    #[test]
    fn recorder_records_and_can_fail() {
        let r = RecordingToast::new();
        r.show("t", "b").unwrap();
        assert_eq!(r.shown(), vec![("t".to_string(), "b".to_string())]);
        let f = RecordingToast::failing("nope");
        assert!(f.show("t", "b").is_err());
        assert_eq!(f.shown().len(), 1);
    }
}
