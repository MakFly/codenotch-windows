//! Minimal per-user registry access (HKCU only) with an RAII key.

use crate::win::{from_wide, wide, win_err};
use codenotch_core::Result;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_READ, KEY_WRITE, REG_BINARY, REG_DWORD,
    REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
};

/// An open `HKCU\<path>` key, closed on drop.
pub(crate) struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened by RegOpenKeyExW/RegCreateKeyExW and is closed once.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

impl Key {
    /// Open an existing key for reading; `None` when it does not exist (or cannot be read).
    pub(crate) fn open_read(path: &str) -> Option<Key> {
        let p = wide(path);
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: `p` is NUL-terminated and outlives the call; `h` is a valid out pointer.
        let rc = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                p.as_ptr(),
                0,
                KEY_READ | KEY_QUERY_VALUE,
                &mut h,
            )
        };
        (rc == ERROR_SUCCESS).then_some(Key(h))
    }

    /// Open for writing, creating the key when missing.
    pub(crate) fn create(path: &str) -> Result<Key> {
        let p = wide(path);
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated path; null class/security; `h` is a valid out pointer.
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                p.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                std::ptr::null(),
                &mut h,
                std::ptr::null_mut(),
            )
        };
        if rc == ERROR_SUCCESS {
            Ok(Key(h))
        } else {
            Err(win_err("cannot open registry key", rc))
        }
    }

    /// Raw value bytes and type; `None` when the value is missing.
    fn raw(&self, name: &str) -> Option<(REG_VALUE_TYPE, Vec<u8>)> {
        let n = wide(name);
        let mut kind: REG_VALUE_TYPE = 0;
        let mut size: u32 = 0;
        // SAFETY: size query with a null data pointer; all pointers valid for the call.
        let rc = unsafe {
            RegQueryValueExW(
                self.0,
                n.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        // SAFETY: `buf` has `size` writable bytes; `size` is updated with the bytes written.
        let rc = unsafe {
            RegQueryValueExW(
                self.0,
                n.as_ptr(),
                std::ptr::null(),
                &mut kind,
                buf.as_mut_ptr(),
                &mut size,
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        buf.truncate(size as usize);
        Some((kind, buf))
    }

    /// REG_SZ / REG_EXPAND_SZ value (not expanded).
    pub(crate) fn get_string(&self, name: &str) -> Option<String> {
        let (kind, bytes) = self.raw(name)?;
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            return None;
        }
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        Some(from_wide(&units))
    }

    pub(crate) fn get_dword(&self, name: &str) -> Option<u32> {
        let (kind, bytes) = self.raw(name)?;
        if kind != REG_DWORD || bytes.len() < 4 {
            return None;
        }
        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn get_binary(&self, name: &str) -> Option<Vec<u8>> {
        let (kind, bytes) = self.raw(name)?;
        (kind == REG_BINARY).then_some(bytes)
    }

    pub(crate) fn set_string(&self, name: &str, value: &str) -> Result<()> {
        let n = wide(name);
        let v = wide(value);
        let bytes = (v.len() * 2) as u32;
        // SAFETY: `v` holds `bytes` bytes of UTF-16 including the NUL terminator.
        let rc = unsafe { RegSetValueExW(self.0, n.as_ptr(), 0, REG_SZ, v.as_ptr().cast(), bytes) };
        if rc == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(win_err("cannot write registry value", rc))
        }
    }

    #[cfg(test)]
    pub(crate) fn set_binary(&self, name: &str, value: &[u8]) -> Result<()> {
        let n = wide(name);
        // SAFETY: `value` is a valid slice of `len` bytes.
        let rc = unsafe {
            RegSetValueExW(
                self.0,
                n.as_ptr(),
                0,
                REG_BINARY,
                value.as_ptr(),
                value.len() as u32,
            )
        };
        if rc == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(win_err("cannot write registry value", rc))
        }
    }

    /// Delete a value; a missing value is Ok.
    pub(crate) fn delete_value(&self, name: &str) -> Result<()> {
        let n = wide(name);
        // SAFETY: valid key and NUL-terminated name.
        let rc = unsafe { RegDeleteValueW(self.0, n.as_ptr()) };
        if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(win_err("cannot delete registry value", rc))
        }
    }
}

/// Delete `HKCU\<path>` and everything under it (tests only clean up their own throwaway key).
#[cfg(test)]
pub(crate) fn delete_tree(path: &str) -> Result<()> {
    use windows_sys::Win32::System::Registry::RegDeleteTreeW;
    let p = wide(path);
    // SAFETY: valid NUL-terminated path under HKCU.
    let rc = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, p.as_ptr()) };
    if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(win_err("cannot delete registry key", rc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::test_util::random_token;

    #[test]
    fn throwaway_key_round_trip() {
        let path = format!("Software\\CodeNotchTest\\{}", random_token());
        let key = Key::create(&path).expect("create");
        key.set_string("s", "\"C:\\x y\\a.exe\" --silent").unwrap();
        key.set_binary("b", &[3, 0, 0]).unwrap();
        assert_eq!(
            key.get_string("s").as_deref(),
            Some("\"C:\\x y\\a.exe\" --silent")
        );
        assert_eq!(key.get_binary("b"), Some(vec![3, 0, 0]));
        assert_eq!(key.get_dword("s"), None);
        assert_eq!(key.get_string("missing"), None);
        key.delete_value("s").unwrap();
        key.delete_value("s").unwrap();
        assert_eq!(key.get_string("s"), None);
        drop(key);
        delete_tree(&path).unwrap();
        assert!(Key::open_read(&path).is_none());
        // Remove the (now empty) parent too when nothing else is under it.
        let _ = delete_tree_if_empty("Software\\CodeNotchTest");
    }

    fn delete_tree_if_empty(path: &str) -> Result<()> {
        use windows_sys::Win32::System::Registry::RegDeleteKeyW;
        let p = wide(path);
        // SAFETY: RegDeleteKeyW only deletes a key without subkeys; valid path.
        unsafe {
            RegDeleteKeyW(HKEY_CURRENT_USER, p.as_ptr());
        }
        Ok(())
    }
}
