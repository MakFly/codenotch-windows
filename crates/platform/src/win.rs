//! Small Win32 helpers shared by the modules: UTF-16 strings, owned handles, errors.

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, WIN32_ERROR};

/// NUL-terminated UTF-16 copy of `s`.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// NUL-terminated UTF-16 copy of an `OsStr` (paths keep unpaired surrogates intact).
pub(crate) fn wide_os(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// UTF-16 buffer up to the first NUL (or the whole buffer).
pub(crate) fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub(crate) fn last_error() -> WIN32_ERROR {
    // SAFETY: GetLastError has no preconditions.
    unsafe { GetLastError() }
}

/// A kernel handle closed on drop. Never holds INVALID_HANDLE_VALUE or null.
pub(crate) struct OwnedHandle(HANDLE);

// SAFETY: kernel object handles (mutex, event, process) are usable from any thread.
unsafe impl Send for OwnedHandle {}
// SAFETY: as above; the handle value is only read.
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    /// Wrap a handle returned by a Win32 call; `None` for null / INVALID_HANDLE_VALUE.
    pub(crate) fn new(h: HANDLE) -> Option<Self> {
        if h.is_null() || h as isize == -1 {
            None
        } else {
            Some(Self(h))
        }
    }
    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is valid and owned by us (checked in `new`); closed exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// "Win32 error 5" style message (no system text: it is localised and may be long).
pub(crate) fn win_err(context: &str, code: WIN32_ERROR) -> codenotch_core::Error {
    codenotch_core::Error::other(format!("{context} (Win32 error {code})"))
}

#[cfg(test)]
pub(crate) mod test_util {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    /// A random 16-hex-digit token for throwaway registry keys / credential targets.
    pub(crate) fn random_token() -> String {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_u32(std::process::id());
        format!("{:016x}", h.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_round_trip() {
        let w = wide("Réglages…");
        assert_eq!(*w.last().unwrap(), 0);
        assert_eq!(from_wide(&w), "Réglages…");
        assert_eq!(from_wide(&[65, 66]), "AB");
    }

    #[test]
    fn owned_handle_rejects_invalid() {
        assert!(OwnedHandle::new(std::ptr::null_mut()).is_none());
        assert!(OwnedHandle::new(-1isize as HANDLE).is_none());
    }
}
