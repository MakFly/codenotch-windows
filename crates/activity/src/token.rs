//! Hook server token: 32 random bytes as 64 lowercase hex characters in `Paths::hook_token()`.
//! Never logged, never part of `Debug` output.

use codenotch_core::paths::write_atomic;
use codenotch_core::{Error, Result};
use std::path::Path;

/// The shared secret. `Debug` hides the value.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<hidden>)")
    }
}

impl Token {
    /// Constant-time comparison with a presented value.
    pub fn matches(&self, presented: &str) -> bool {
        ct_eq(self.0.as_bytes(), presented.trim().as_bytes())
    }
    #[cfg(test)]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Constant-time equality (time depends only on the lengths).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u64;
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0xff);
        diff |= u64::from(x ^ y);
    }
    std::hint::black_box(diff) == 0
}

fn valid(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Load the token file, creating (or replacing an unusable one with) a fresh random token.
pub fn load_or_create(file: &Path) -> Result<Token> {
    if let Ok(text) = std::fs::read_to_string(file) {
        let t = text.trim();
        if valid(t) {
            return Ok(Token(t.to_string()));
        }
        log::warn!("hook token file unusable; creating a new one");
    }
    let bytes = random_bytes()?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    write_atomic(file, hex.as_bytes()).map_err(|e| Error::io("write hook token", e))?;
    Ok(Token(hex))
}

#[cfg(windows)]
fn random_bytes() -> Result<[u8; 32]> {
    use windows_sys::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    let mut buf = [0u8; 32];
    // SAFETY: valid buffer and length; a null algorithm handle is allowed with the flag.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(Error::other("system random generator failed"));
    }
    Ok(buf)
}

#[cfg(not(windows))]
fn random_bytes() -> Result<[u8; 32]> {
    use std::io::Read;
    let mut buf = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| Error::io("read /dev/urandom", e))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_once_and_reuses() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("auth_token");
        let a = load_or_create(&f).unwrap();
        assert!(valid(a.expose()));
        let b = load_or_create(&f).unwrap();
        assert_eq!(a, b);
        assert_eq!(format!("{a:?}"), "Token(<hidden>)");
        std::fs::write(&f, "short").unwrap();
        let c = load_or_create(&f).unwrap();
        assert_ne!(a, c);
        assert!(valid(c.expose()));
    }

    #[test]
    fn matching_is_exact() {
        let d = tempfile::tempdir().unwrap();
        let t = load_or_create(&d.path().join("t")).unwrap();
        let good = t.expose().to_string();
        assert!(t.matches(&good));
        assert!(t.matches(&format!(" {good}\n")));
        assert!(!t.matches(&good[..63]));
        assert!(!t.matches(&format!("{good}0")));
        assert!(!t.matches(""));
        let mut flipped = good.into_bytes();
        flipped[10] = if flipped[10] == b'a' { b'b' } else { b'a' };
        assert!(!t.matches(std::str::from_utf8(&flipped).unwrap()));
        assert!(ct_eq(b"", b""));
        assert!(!ct_eq(b"a", b""));
    }
}
