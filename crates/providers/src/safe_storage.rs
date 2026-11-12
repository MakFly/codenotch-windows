//! Electron `safeStorage` on Windows, read-only.
//!
//! Electron apps (the Claude desktop app, Cursor) keep secrets encrypted at rest with a per-app
//! AES-256 key. The key sits in the app's `Local State` JSON as `os_crypt.encrypted_key` =
//! base64(`DPAPI` + CryptProtectData(key)), protected for the current Windows user. Each value is
//! `v10` (or `v11`) + 12-byte nonce + AES-256-GCM ciphertext + 16-byte tag.
//!
//! We only ever decrypt, in memory, for the signed-in Windows user; nothing is written back and
//! neither the key nor a plaintext is ever logged (`Debug` hides the key).

use crate::util::base64url_decode;
use serde_json::Value;
use std::path::Path;

const KEY_PREFIX: &[u8] = b"DPAPI";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

/// An app's `safeStorage` key.
#[derive(Clone)]
pub struct SafeStorage {
    key: [u8; 32],
}

impl std::fmt::Debug for SafeStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SafeStorage(<key hidden>)")
    }
}

impl Drop for SafeStorage {
    fn drop(&mut self) {
        self.key.iter_mut().for_each(|b| *b = 0);
    }
}

impl SafeStorage {
    /// A raw 32-byte key (tests, or a key obtained elsewhere).
    pub fn from_key(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// The key of the app whose `Local State` file is `path`. `None` when the file is missing,
    /// has no DPAPI-wrapped key, or DPAPI refuses it (another Windows user's file).
    pub fn from_local_state(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        let wrapped = base64url_decode(v.get("os_crypt")?.get("encrypted_key")?.as_str()?)?;
        let protected = wrapped.strip_prefix(KEY_PREFIX)?;
        let key = sys::dpapi_unprotect(protected)?;
        let key: [u8; 32] = key.as_slice().try_into().ok()?;
        Some(Self { key })
    }

    /// Decrypt one `v10`/`v11` value. `None` on any format or authentication failure.
    pub fn decrypt(&self, blob: &[u8]) -> Option<Vec<u8>> {
        let rest = blob
            .strip_prefix(b"v10")
            .or_else(|| blob.strip_prefix(b"v11"))?;
        if rest.len() < NONCE_LEN + TAG_LEN {
            return None;
        }
        let (nonce, rest) = rest.split_at(NONCE_LEN);
        let (cipher, tag) = rest.split_at(rest.len() - TAG_LEN);
        sys::aes_gcm(&self.key, nonce, cipher, tag)
    }

    /// Decrypt a base64 value into UTF-8 text.
    pub fn decrypt_base64(&self, text: &str) -> Option<String> {
        let plain = self.decrypt(&base64url_decode(text)?)?;
        String::from_utf8(plain).ok()
    }

    /// Encrypt `plain` as a `v10` value with `nonce` (fixtures for tests only).
    #[doc(hidden)]
    pub fn encrypt_for_test(&self, plain: &[u8], nonce: [u8; NONCE_LEN]) -> Vec<u8> {
        let mut tag = [0u8; TAG_LEN];
        let cipher =
            sys::aes_gcm_encrypt(&self.key, &nonce, plain, &mut tag).expect("AES-GCM encrypt");
        let mut out = b"v10".to_vec();
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&cipher);
        out.extend_from_slice(&tag);
        out
    }

    /// A `Local State` body wrapping this key with DPAPI for the current user (tests only).
    #[doc(hidden)]
    pub fn local_state_for_test(&self) -> String {
        let mut wrapped = KEY_PREFIX.to_vec();
        wrapped.extend(sys::dpapi_protect(&self.key).expect("DPAPI protect"));
        serde_json::json!({"os_crypt": {"encrypted_key": base64_encode(&wrapped)}}).to_string()
    }
}

/// Standard base64 with padding (fixtures).
#[doc(hidden)]
pub fn base64_encode(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(windows)]
mod sys {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        BCryptCloseAlgorithmProvider, BCryptDecrypt, BCryptDestroyKey, BCryptEncrypt,
        BCryptGenerateSymmetricKey, BCryptOpenAlgorithmProvider, BCryptSetProperty,
        CryptProtectData, CryptUnprotectData, BCRYPT_AES_ALGORITHM, BCRYPT_ALG_HANDLE,
        BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO, BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO_VERSION,
        BCRYPT_CHAINING_MODE, BCRYPT_CHAIN_MODE_GCM, BCRYPT_KEY_HANDLE, CRYPTPROTECT_UI_FORBIDDEN,
        CRYPT_INTEGER_BLOB,
    };

    fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(data.len()).ok()?,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: `input` points at `data` for the call; DPAPI allocates `output` with
        // LocalAlloc, which we copy and free exactly once.
        unsafe {
            let ok = if protect {
                CryptProtectData(
                    &input,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if ok == 0 || output.pbData.is_null() {
                return None;
            }
            let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            if !protect {
                std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
            }
            LocalFree(output.pbData.cast());
            Some(out)
        }
    }

    pub(super) fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
        dpapi(data, false)
    }

    pub(super) fn dpapi_protect(data: &[u8]) -> Option<Vec<u8>> {
        dpapi(data, true)
    }

    /// An AES-GCM key handle, closed on drop.
    struct GcmKey {
        alg: BCRYPT_ALG_HANDLE,
        key: BCRYPT_KEY_HANDLE,
    }

    impl Drop for GcmKey {
        fn drop(&mut self) {
            // SAFETY: handles opened by `GcmKey::new`, released once.
            unsafe {
                if !self.key.is_null() {
                    BCryptDestroyKey(self.key);
                }
                if !self.alg.is_null() {
                    BCryptCloseAlgorithmProvider(self.alg, 0);
                }
            }
        }
    }

    impl GcmKey {
        fn new(key: &[u8; 32]) -> Option<Self> {
            let mut k = GcmKey {
                alg: std::ptr::null_mut(),
                key: std::ptr::null_mut(),
            };
            // SAFETY: out-pointers are valid locals; the mode string is a static wide string
            // whose byte length includes its terminator.
            unsafe {
                if BCryptOpenAlgorithmProvider(
                    &mut k.alg,
                    BCRYPT_AES_ALGORITHM,
                    std::ptr::null(),
                    0,
                ) != 0
                {
                    return None;
                }
                let mode = BCRYPT_CHAIN_MODE_GCM;
                let mut len = 0usize;
                while *mode.add(len) != 0 {
                    len += 1;
                }
                if BCryptSetProperty(
                    k.alg,
                    BCRYPT_CHAINING_MODE,
                    mode.cast(),
                    ((len + 1) * 2) as u32,
                    0,
                ) != 0
                {
                    return None;
                }
                if BCryptGenerateSymmetricKey(
                    k.alg,
                    &mut k.key,
                    std::ptr::null_mut(),
                    0,
                    key.as_ptr(),
                    32,
                    0,
                ) != 0
                {
                    return None;
                }
            }
            Some(k)
        }
    }

    fn mode_info(nonce: &[u8], tag: &mut [u8]) -> BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO {
        BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO {
            cbSize: std::mem::size_of::<BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO>() as u32,
            dwInfoVersion: BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO_VERSION,
            pbNonce: nonce.as_ptr() as *mut u8,
            cbNonce: nonce.len() as u32,
            pbTag: tag.as_mut_ptr(),
            cbTag: tag.len() as u32,
            ..Default::default()
        }
    }

    /// AES-256-GCM decrypt; `None` when the tag does not authenticate.
    pub(super) fn aes_gcm(
        key: &[u8; 32],
        nonce: &[u8],
        cipher: &[u8],
        tag: &[u8],
    ) -> Option<Vec<u8>> {
        let k = GcmKey::new(key)?;
        let mut tag = tag.to_vec();
        let info = mode_info(nonce, &mut tag);
        let mut out = vec![0u8; cipher.len()];
        let mut written = 0u32;
        // SAFETY: buffers are sized by their lengths; `info` borrows `nonce`/`tag` for the call.
        let status = unsafe {
            BCryptDecrypt(
                k.key,
                cipher.as_ptr(),
                u32::try_from(cipher.len()).ok()?,
                (&info as *const BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO).cast(),
                std::ptr::null_mut(),
                0,
                out.as_mut_ptr(),
                out.len() as u32,
                &mut written,
                0,
            )
        };
        if status != 0 {
            return None;
        }
        out.truncate(written as usize);
        Some(out)
    }

    pub(super) fn aes_gcm_encrypt(
        key: &[u8; 32],
        nonce: &[u8],
        plain: &[u8],
        tag: &mut [u8],
    ) -> Option<Vec<u8>> {
        let k = GcmKey::new(key)?;
        let info = mode_info(nonce, tag);
        let mut out = vec![0u8; plain.len()];
        let mut written = 0u32;
        // SAFETY: as in `aes_gcm`; BCrypt writes the tag through `info.pbTag`.
        let status = unsafe {
            BCryptEncrypt(
                k.key,
                plain.as_ptr(),
                u32::try_from(plain.len()).ok()?,
                (&info as *const BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO).cast(),
                std::ptr::null_mut(),
                0,
                out.as_mut_ptr(),
                out.len() as u32,
                &mut written,
                0,
            )
        };
        if status != 0 {
            return None;
        }
        out.truncate(written as usize);
        Some(out)
    }
}

#[cfg(not(windows))]
mod sys {
    pub(super) fn dpapi_unprotect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }
    pub(super) fn dpapi_protect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }
    pub(super) fn aes_gcm(_: &[u8; 32], _: &[u8], _: &[u8], _: &[u8]) -> Option<Vec<u8>> {
        None
    }
    pub(super) fn aes_gcm_encrypt(
        _: &[u8; 32],
        _: &[u8],
        _: &[u8],
        _: &mut [u8],
    ) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tamper() {
        let s = SafeStorage::from_key([7u8; 32]);
        let blob = s.encrypt_for_test(b"hello safe storage", [1u8; 12]);
        assert_eq!(&blob[..3], b"v10");
        assert_eq!(s.decrypt(&blob).unwrap(), b"hello safe storage");
        assert_eq!(
            s.decrypt_base64(&base64_encode(&blob)).as_deref(),
            Some("hello safe storage")
        );
        let mut bad = blob.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(s.decrypt(&bad).is_none(), "a wrong tag never decrypts");
        assert!(SafeStorage::from_key([8u8; 32]).decrypt(&blob).is_none());
        assert!(s.decrypt(b"v10short").is_none());
        assert!(s.decrypt(b"xyz").is_none());
        assert!(format!("{s:?}").contains("hidden"));
    }

    #[test]
    fn local_state_key_through_dpapi() {
        let dir = tempfile::tempdir().unwrap();
        let s = SafeStorage::from_key([9u8; 32]);
        let path = dir.path().join("Local State");
        std::fs::write(&path, s.local_state_for_test()).unwrap();
        let loaded = SafeStorage::from_local_state(&path).unwrap();
        let blob = s.encrypt_for_test(b"x", [2u8; 12]);
        assert_eq!(loaded.decrypt(&blob).unwrap(), b"x");
        assert!(SafeStorage::from_local_state(&dir.path().join("missing")).is_none());
    }

    #[test]
    fn base64_encode_matches_decoder() {
        for s in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\xff\xef\x00"] {
            assert_eq!(base64url_decode(&base64_encode(s)).unwrap(), s);
        }
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    }
}
