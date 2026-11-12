//! Secret storage in the Windows Credential Manager.

use crate::win::{wide, win_err};
use codenotch_core::secrets::{SecretKey, SecretStore};
use codenotch_core::{Error, Result};
use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
use windows_sys::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_MAX_CREDENTIAL_BLOB_SIZE,
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};

/// User name stored with each credential (informational).
const USER_NAME: &str = "CodeNotch";

/// Generic credentials (`CRED_TYPE_GENERIC`, persist `LOCAL_MACHINE` = this user on this PC,
/// not roamed) named `SecretKey::target_name()` (`CodeNotch/telegram-token`, ...), user name
/// "CodeNotch", value as UTF-8 bytes. `get` of a missing credential is `None`; `delete` of a
/// missing one is Ok. Never logs values. Thread-safe (no state).
#[derive(Debug, Default, Clone, Copy)]
pub struct CredentialStore;

impl CredentialStore {
    pub fn new() -> Self {
        Self
    }
}

impl SecretStore for CredentialStore {
    fn get(&self, key: &SecretKey) -> Option<String> {
        read_target(&key.target_name())
    }
    fn set(&self, key: &SecretKey, value: &str) -> Result<()> {
        write_target(&key.target_name(), value)
    }
    fn delete(&self, key: &SecretKey) -> Result<()> {
        delete_target(&key.target_name())
    }
}

/// Owns a CREDENTIALW returned by CredReadW; freed with CredFree.
struct CredGuard(*mut CREDENTIALW);

impl Drop for CredGuard {
    fn drop(&mut self) {
        // SAFETY: pointer returned by CredReadW, freed exactly once.
        unsafe { CredFree(self.0.cast()) };
    }
}

pub(crate) fn read_target(target: &str) -> Option<String> {
    let name = wide(target);
    let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: valid NUL-terminated name and out pointer.
    let ok = unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) };
    if ok == 0 || cred.is_null() {
        let code = crate::win::last_error();
        if code != ERROR_NOT_FOUND {
            log::warn!("credential read failed (Win32 error {code})");
        }
        return None;
    }
    let guard = CredGuard(cred);
    // SAFETY: CredReadW succeeded, so `cred` points to a valid CREDENTIALW whose blob has
    // CredentialBlobSize bytes (or is null when the size is 0).
    let bytes = unsafe {
        let c = &*guard.0;
        if c.CredentialBlob.is_null() || c.CredentialBlobSize == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
        }
    };
    drop(guard);
    let value = match String::from_utf8(bytes) {
        Ok(v) => v,
        // Written by another tool as UTF-16: accept it rather than losing the secret.
        Err(e) => {
            let b = e.into_bytes();
            let units: Vec<u16> = b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            String::from_utf16(&units).ok()?
        }
    };
    (!value.is_empty()).then_some(value)
}

pub(crate) fn write_target(target: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return delete_target(target);
    }
    if value.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        return Err(Error::other("secret too long for the Credential Manager"));
    }
    let mut name = wide(target);
    let mut user = wide(USER_NAME);
    let mut blob = value.as_bytes().to_vec();
    // SAFETY: CREDENTIALW is plain data; zero is valid (null pointers, zero sizes).
    let mut cred: CREDENTIALW = unsafe { std::mem::zeroed() };
    cred.Type = CRED_TYPE_GENERIC;
    cred.TargetName = name.as_mut_ptr();
    cred.UserName = user.as_mut_ptr();
    cred.CredentialBlobSize = blob.len() as u32;
    cred.CredentialBlob = blob.as_mut_ptr();
    cred.Persist = CRED_PERSIST_LOCAL_MACHINE;
    // SAFETY: every pointer in `cred` refers to a live buffer for the duration of the call.
    let ok = unsafe { CredWriteW(&cred, 0) };
    let code = crate::win::last_error();
    blob.iter_mut().for_each(|b| *b = 0);
    if ok == 0 {
        return Err(win_err("cannot save the secret", code));
    }
    Ok(())
}

pub(crate) fn delete_target(target: &str) -> Result<()> {
    let name = wide(target);
    // SAFETY: valid NUL-terminated name.
    let ok = unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) };
    if ok == 0 {
        let code = crate::win::last_error();
        if code != ERROR_NOT_FOUND {
            return Err(win_err("cannot delete the secret", code));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::test_util::random_token;

    /// Only a random `CodeNotchTest/<random>` target, deleted at the end.
    #[test]
    fn round_trip_with_a_throwaway_target() {
        let target = format!("CodeNotchTest/{}", random_token());
        let result = std::panic::catch_unwind(|| {
            assert_eq!(read_target(&target), None);
            write_target(&target, "tok-é-123").unwrap();
            assert_eq!(read_target(&target).as_deref(), Some("tok-é-123"));
            write_target(&target, "second").unwrap();
            assert_eq!(read_target(&target).as_deref(), Some("second"));
            delete_target(&target).unwrap();
            assert_eq!(read_target(&target), None);
            delete_target(&target).unwrap();
            // Empty value = delete.
            write_target(&target, "x").unwrap();
            write_target(&target, "").unwrap();
            assert_eq!(read_target(&target), None);
            assert!(write_target(&target, &"x".repeat(4000)).is_err());
            assert_eq!(read_target(&target), None);
        });
        let _ = delete_target(&target);
        if let Err(p) = result {
            std::panic::resume_unwind(p);
        }
    }

    #[test]
    fn store_maps_keys_to_core_target_names() {
        // No Credential Manager access here: only the naming contract.
        assert_eq!(
            SecretKey::TelegramToken.target_name(),
            "CodeNotch/telegram-token"
        );
        assert_eq!(
            SecretKey::Account("a1".into()).target_name(),
            "CodeNotch/account/a1"
        );
    }
}
