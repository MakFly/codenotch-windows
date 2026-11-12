//! Global hotkey ("Ctrl+Alt+N" style), registered on a private thread with its own message loop.
//!
//! Parsing is case-insensitive, accepts spaces around `+`, English and common French key names
//! (`Maj`, `Espace`, `Entrée`, `Échap`, `Suppr`, `Inser`, `Début`, `Fin`, `PgPréc`, `PgSuiv`,
//! `Haut`/`Bas`/`Gauche`/`Droite`, `AltGr` = Ctrl+Alt). Punctuation keys map to the US-layout
//! virtual keys (`VK_OEM_*`). At least one modifier is required, and Shift alone is refused with
//! a character key (it would swallow capital letters system-wide).

use codenotch_core::{Error, Result};
use std::fmt;
use std::sync::mpsc;
use std::thread::JoinHandle;
use windows_sys::Win32::Foundation::ERROR_HOTKEY_ALREADY_REGISTERED;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_HOTKEY, WM_QUIT, WM_USER,
};

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

/// Parsed hotkey: Win32 modifier flags (`MOD_ALT|MOD_CONTROL|MOD_SHIFT|MOD_WIN`) + virtual key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeySpec {
    pub modifiers: u32,
    pub vk: u32,
}

/// Named keys: (normalised name, vk, canonical display name). Normalised = lower case, no
/// accents, no spaces/dots.
const NAMED_KEYS: &[(&str, u32, &str)] = &[
    ("space", 0x20, "Space"),
    ("espace", 0x20, "Space"),
    ("enter", 0x0D, "Enter"),
    ("return", 0x0D, "Enter"),
    ("entree", 0x0D, "Enter"),
    ("esc", 0x1B, "Esc"),
    ("escape", 0x1B, "Esc"),
    ("echap", 0x1B, "Esc"),
    ("tab", 0x09, "Tab"),
    ("tabulation", 0x09, "Tab"),
    ("backspace", 0x08, "Backspace"),
    ("retourarriere", 0x08, "Backspace"),
    ("delete", 0x2E, "Delete"),
    ("del", 0x2E, "Delete"),
    ("suppr", 0x2E, "Delete"),
    ("supprimer", 0x2E, "Delete"),
    ("insert", 0x2D, "Insert"),
    ("ins", 0x2D, "Insert"),
    ("inser", 0x2D, "Insert"),
    ("inserer", 0x2D, "Insert"),
    ("home", 0x24, "Home"),
    ("debut", 0x24, "Home"),
    ("origine", 0x24, "Home"),
    ("end", 0x23, "End"),
    ("fin", 0x23, "End"),
    ("pageup", 0x21, "PageUp"),
    ("pgup", 0x21, "PageUp"),
    ("pgprec", 0x21, "PageUp"),
    ("pageprec", 0x21, "PageUp"),
    ("pagedown", 0x22, "PageDown"),
    ("pgdn", 0x22, "PageDown"),
    ("pgdown", 0x22, "PageDown"),
    ("pgsuiv", 0x22, "PageDown"),
    ("pagesuiv", 0x22, "PageDown"),
    ("left", 0x25, "Left"),
    ("gauche", 0x25, "Left"),
    ("up", 0x26, "Up"),
    ("haut", 0x26, "Up"),
    ("right", 0x27, "Right"),
    ("droite", 0x27, "Right"),
    ("down", 0x28, "Down"),
    ("bas", 0x28, "Down"),
    ("pause", 0x13, "Pause"),
    ("printscreen", 0x2C, "PrintScreen"),
    ("prtsc", 0x2C, "PrintScreen"),
    ("impr", 0x2C, "PrintScreen"),
    ("imprecran", 0x2C, "PrintScreen"),
    ("plus", 0xBB, "Plus"),
    ("minus", 0xBD, "Minus"),
    ("moins", 0xBD, "Minus"),
    ("comma", 0xBC, ","),
    ("virgule", 0xBC, ","),
    ("period", 0xBE, "."),
    ("point", 0xBE, "."),
    ("numadd", 0x6B, "NumAdd"),
    ("numsubtract", 0x6D, "NumSubtract"),
    ("nummultiply", 0x6A, "NumMultiply"),
    ("numdivide", 0x6F, "NumDivide"),
    ("numdecimal", 0x6E, "NumDecimal"),
];

/// Single punctuation characters (US layout).
const PUNCT: &[(char, u32)] = &[
    (';', 0xBA),
    ('=', 0xBB),
    (',', 0xBC),
    ('-', 0xBD),
    ('.', 0xBE),
    ('/', 0xBF),
    ('`', 0xC0),
    ('[', 0xDB),
    ('\\', 0xDC),
    (']', 0xDD),
    ('\'', 0xDE),
];

fn normalise(token: &str) -> String {
    token
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace() && *c != '.' && *c != '_' && *c != '-')
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'â' => 'a',
            'î' | 'ï' => 'i',
            'ô' => 'o',
            'û' | 'ù' => 'u',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

fn modifier(token: &str) -> Option<u32> {
    match normalise(token).as_str() {
        "ctrl" | "control" | "ctl" | "controle" | "strg" => Some(MOD_CONTROL),
        "alt" | "option" => Some(MOD_ALT),
        "shift" | "maj" | "majuscule" => Some(MOD_SHIFT),
        "win" | "windows" | "super" | "meta" | "cmd" | "logo" => Some(MOD_WIN),
        "altgr" => Some(MOD_CONTROL | MOD_ALT),
        _ => None,
    }
}

fn key(token: &str) -> Option<u32> {
    let raw = token.trim();
    let mut chars = raw.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase() as u32);
        }
        if let Some((_, vk)) = PUNCT.iter().find(|(p, _)| *p == c) {
            return Some(*vk);
        }
        if c == '+' {
            return Some(0xBB);
        }
    }
    let n = normalise(raw);
    if let Some(num) = n.strip_prefix('f').and_then(|d| d.parse::<u32>().ok()) {
        return (1..=24).contains(&num).then_some(0x70 + num - 1);
    }
    for prefix in ["numpad", "num", "pavnum"] {
        if let Some(d) = n.strip_prefix(prefix).and_then(|d| d.parse::<u32>().ok()) {
            if d <= 9 {
                return Some(0x60 + d);
            }
        }
    }
    NAMED_KEYS
        .iter()
        .find(|(name, _, _)| *name == n)
        .map(|(_, vk, _)| *vk)
}

/// A key that types a character (letters, digits, punctuation, space).
fn is_character_key(vk: u32) -> bool {
    matches!(vk, 0x20 | 0x30..=0x39 | 0x41..=0x5A | 0xBA..=0xC0 | 0xDB..=0xDE)
}

/// Parse "Ctrl+Alt+N", "Win+Shift+F12", "ctrl + maj + espace" … `None` when invalid.
pub fn parse(text: &str) -> Option<HotkeySpec> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // "Ctrl++" means Ctrl and the plus key.
    let (mods_part, key_part) = if let Some(rest) = text.strip_suffix("++") {
        (rest, "+")
    } else {
        match text.rfind('+') {
            Some(i) => (&text[..i], &text[i + 1..]),
            None => ("", text),
        }
    };
    let mut modifiers = 0u32;
    for token in mods_part.split('+') {
        if token.trim().is_empty() {
            if mods_part.trim().is_empty() {
                continue;
            }
            return None;
        }
        modifiers |= modifier(token)?;
    }
    if key_part.trim().is_empty() || modifier(key_part).is_some() {
        return None;
    }
    let vk = key(key_part)?;
    if modifiers == 0 {
        return None;
    }
    if modifiers == MOD_SHIFT && is_character_key(vk) {
        return None;
    }
    Some(HotkeySpec { modifiers, vk })
}

impl fmt::Display for HotkeySpec {
    /// Canonical text: "Ctrl+Alt+Shift+Win+N".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in [
            (MOD_CONTROL, "Ctrl"),
            (MOD_ALT, "Alt"),
            (MOD_SHIFT, "Shift"),
            (MOD_WIN, "Win"),
        ] {
            if self.modifiers & flag != 0 {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&key_name(self.vk))
    }
}

fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).map(String::from).unwrap_or_default(),
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        _ => {
            if let Some((_, _, name)) = NAMED_KEYS.iter().find(|(_, v, _)| *v == vk) {
                return (*name).to_string();
            }
            if let Some((c, _)) = PUNCT.iter().find(|(_, v)| *v == vk) {
                return c.to_string();
            }
            format!("0x{vk:02X}")
        }
    }
}

/// Unregisters on drop (posts WM_QUIT to the hotkey thread and joins it).
pub struct HotkeyHandle {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
    spec: HotkeySpec,
}

impl HotkeyHandle {
    pub fn spec(&self) -> HotkeySpec {
        self.spec
    }
}

impl Drop for HotkeyHandle {
    fn drop(&mut self) {
        // SAFETY: posting to a thread id we own; the queue exists (created before registering).
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

const HOTKEY_ID: i32 = 0x0C0D;

/// Register; `on_press` runs on the hotkey thread (send a Command from it). Fails with a clear
/// message when another app already owns the combination.
pub fn register(spec: HotkeySpec, on_press: Box<dyn Fn() + Send>) -> Result<HotkeyHandle> {
    let (tx, rx) = mpsc::sync_channel::<std::result::Result<u32, u32>>(1);
    let thread = std::thread::Builder::new()
        .name("codenotch-hotkey".into())
        .spawn(move || hotkey_thread(spec, on_press, tx))
        .map_err(|e| Error::io("cannot start the hotkey thread", e))?;
    match rx.recv() {
        Ok(Ok(thread_id)) => {
            log::info!("hotkey {spec} registered");
            Ok(HotkeyHandle {
                thread_id,
                thread: Some(thread),
                spec,
            })
        }
        Ok(Err(code)) => {
            let _ = thread.join();
            if code == ERROR_HOTKEY_ALREADY_REGISTERED {
                Err(Error::other(format!(
                    "{spec} is already used by another app"
                )))
            } else {
                Err(crate::win::win_err(
                    &format!("cannot register {spec}"),
                    code,
                ))
            }
        }
        Err(_) => {
            let _ = thread.join();
            Err(Error::other("hotkey thread stopped"))
        }
    }
}

fn hotkey_thread(
    spec: HotkeySpec,
    on_press: Box<dyn Fn() + Send>,
    tx: mpsc::SyncSender<std::result::Result<u32, u32>>,
) {
    // SAFETY: MSG is plain data; zero is a valid bit pattern.
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    // SAFETY: forces creation of this thread's message queue so PostThreadMessageW works.
    unsafe {
        PeekMessageW(
            &mut msg,
            std::ptr::null_mut(),
            WM_USER,
            WM_USER,
            PM_NOREMOVE,
        )
    };
    // SAFETY: no preconditions.
    let thread_id = unsafe { GetCurrentThreadId() };
    // SAFETY: thread-level hotkey (null hwnd): WM_HOTKEY is posted to this thread's queue.
    let ok = unsafe {
        RegisterHotKey(
            std::ptr::null_mut(),
            HOTKEY_ID,
            spec.modifiers | MOD_NOREPEAT,
            spec.vk,
        )
    } != 0;
    if !ok {
        let _ = tx.send(Err(crate::win::last_error()));
        return;
    }
    if tx.send(Ok(thread_id)).is_err() {
        // SAFETY: unregister what we registered on this thread.
        unsafe { UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID) };
        return;
    }
    // SAFETY: `msg` is a valid out pointer; GetMessageW returns 0 on WM_QUIT, -1 on error.
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        if msg.message == WM_HOTKEY
            && msg.wParam == HOTKEY_ID as usize
            && std::panic::catch_unwind(std::panic::AssertUnwindSafe(&on_press)).is_err()
        {
            log::error!("hotkey callback panicked");
        }
    }
    // SAFETY: same thread that registered it.
    unsafe { UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID) };
    log::info!("hotkey {spec} unregistered");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Option<(u32, u32)> {
        parse(s).map(|h| (h.modifiers, h.vk))
    }

    #[test]
    fn basic_combinations() {
        assert_eq!(p("Ctrl+Alt+N"), Some((MOD_CONTROL | MOD_ALT, 'N' as u32)));
        assert_eq!(p("Win+Shift+F9"), Some((MOD_WIN | MOD_SHIFT, 0x78)));
        assert_eq!(
            p("ctrl + alt + n"),
            Some((MOD_CONTROL | MOD_ALT, 'N' as u32))
        );
        assert_eq!(
            p("CONTROL+SHIFT+1"),
            Some((MOD_CONTROL | MOD_SHIFT, '1' as u32))
        );
        assert_eq!(p("Ctrl+F24"), Some((MOD_CONTROL, 0x87)));
        assert_eq!(p("Alt+Space"), Some((MOD_ALT, 0x20)));
        assert_eq!(p("Ctrl+Alt+Delete"), Some((MOD_CONTROL | MOD_ALT, 0x2E)));
        assert_eq!(p("Ctrl+Num5"), Some((MOD_CONTROL, 0x65)));
        assert_eq!(p("Super+PageUp"), Some((MOD_WIN, 0x21)));
    }

    #[test]
    fn french_names() {
        assert_eq!(p("Ctrl+Maj+N"), Some((MOD_CONTROL | MOD_SHIFT, 'N' as u32)));
        assert_eq!(p("Ctrl+Alt+Espace"), Some((MOD_CONTROL | MOD_ALT, 0x20)));
        assert_eq!(p("Ctrl+Entrée"), Some((MOD_CONTROL, 0x0D)));
        assert_eq!(p("Alt+Échap"), Some((MOD_ALT, 0x1B)));
        assert_eq!(p("Ctrl+Suppr"), Some((MOD_CONTROL, 0x2E)));
        assert_eq!(p("Win+PgPréc"), Some((MOD_WIN, 0x21)));
        assert_eq!(p("Contrôle+Début"), Some((MOD_CONTROL, 0x24)));
        assert_eq!(p("AltGr+E"), Some((MOD_CONTROL | MOD_ALT, 'E' as u32)));
        assert_eq!(p("Maj+F3"), Some((MOD_SHIFT, 0x72)));
    }

    #[test]
    fn punctuation_and_plus() {
        assert_eq!(p("Ctrl++"), Some((MOD_CONTROL, 0xBB)));
        assert_eq!(p("Ctrl+Plus"), Some((MOD_CONTROL, 0xBB)));
        assert_eq!(p("Ctrl+Alt+,"), Some((MOD_CONTROL | MOD_ALT, 0xBC)));
        assert_eq!(p("Ctrl+-"), Some((MOD_CONTROL, 0xBD)));
        assert_eq!(p("Win+/"), Some((MOD_WIN, 0xBF)));
    }

    #[test]
    fn rejects_invalid() {
        for bad in [
            "",
            "   ",
            "N",
            "F9",
            "Ctrl",
            "Ctrl+Alt",
            "Ctrl+",
            "+N",
            "Ctrl++N",
            "Ctrl+Foo",
            "Hyper+N",
            "Ctrl+F25",
            "Ctrl+F0",
            "Ctrl+NN",
            "Shift+N",
            "Maj+1",
            "Shift+Space",
            "Ctrl+Alt+Shift",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn display_is_canonical_and_round_trips() {
        for (input, canon) in [
            ("alt+ctrl+n", "Ctrl+Alt+N"),
            ("shift+win+f9", "Shift+Win+F9"),
            ("Ctrl+Maj+Espace", "Ctrl+Shift+Space"),
            ("Ctrl++", "Ctrl+Plus"),
            ("Ctrl+Alt+,", "Ctrl+Alt+,"),
            ("ctrl+num7", "Ctrl+Num7"),
            ("Win+Échap", "Win+Esc"),
        ] {
            let spec = parse(input).unwrap();
            assert_eq!(spec.to_string(), canon);
            assert_eq!(parse(canon), Some(spec), "{canon}");
        }
    }
}
