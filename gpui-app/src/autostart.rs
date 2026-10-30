//! Starting with the Windows session, through the current user's `Run` key. The value name
//! is the one the uninstaller removes (`installer.nsi`).

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const NAME: &str = "CodeNotch Windows";
/// Passed by the `Run` value so a session start never opens the settings.
pub const FLAG: &str = "--autostart";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The command the `Run` value should hold for this executable.
fn command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" {FLAG}", exe.display()))
}

/// The command currently registered, if any.
#[cfg(windows)]
fn registered() -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let (key, name) = (wide(RUN), wide(NAME));
    let mut buffer = [0u16; 2048];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr() as _,
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buffer[..len]))
}

/// Registers or removes this executable in the `Run` key.
#[cfg(windows)]
pub fn set(enabled: bool) -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
    };
    let (key, name) = (wide(RUN), wide(NAME));
    let status = if enabled {
        let value = wide(&command().ok_or("Chemin de CodeNotch introuvable.")?);
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr() as _,
                (value.len() * 2) as u32,
            )
        }
    } else {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) } {
            // Already absent.
            2 => 0,
            status => status,
        }
    };
    if status == 0 {
        Ok(())
    } else {
        Err("Impossible de modifier le démarrage automatique de Windows.".into())
    }
}

/// At launch: a portable copy that moved, or an update into another folder, would leave a
/// `Run` value pointing to a missing executable, so it is rewritten. Debug builds share the
/// configuration with the installed app and leave its value alone.
#[cfg(windows)]
pub fn reconcile(enabled: bool) {
    if enabled && !cfg!(debug_assertions) && registered() != command() {
        let _ = set(true);
    }
}

#[cfg(not(windows))]
pub fn set(_: bool) -> Result<(), String> {
    Err("Windows only.".into())
}

#[cfg(not(windows))]
pub fn reconcile(_: bool) {}
