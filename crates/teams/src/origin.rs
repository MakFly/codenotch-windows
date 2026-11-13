//! Teams origin policy and invite links.
//!
//! The device token and invite codes are only ever sent to ONE origin: the admitted origin,
//! which is `teamsEndpoint` when the user set it, else the official origin compiled into the
//! app (see [`codenotch_core::Settings::teams_origin`]). Rules:
//!
//! * `https://host[:port]`, or `http://` to `localhost`, `127.0.0.1` or `[::1]` only;
//! * no user info, query or fragment; an origin has no path;
//! * an invite link whose origin is not the admitted origin is refused before any request.
//!
//! Redirects are never followed (the HTTP client does not follow them; a 3xx is an error).

use std::fmt;

use codenotch_core::Settings;

/// Why an origin or an invite link is refused. Carries no part of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginError {
    /// No `teamsEndpoint` and no official origin in this build.
    NotConfigured,
    /// Not an `https://` origin (or `http://` to localhost), or it has user info, a path, a
    /// query or a fragment.
    Invalid,
    /// The invite link points to another server than the admitted origin.
    NotAdmitted,
    /// The invite link or code is malformed.
    BadInvite,
}

impl fmt::Display for OriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            OriginError::NotConfigured => "no Teams server configured",
            OriginError::Invalid => "invalid Teams server address",
            OriginError::NotAdmitted => "invite link for another server",
            OriginError::BadInvite => "invalid invite link",
        })
    }
}

impl std::error::Error for OriginError {}

/// A checked origin, normalised to `scheme://host[:port]` (lower case, default port dropped).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin(String);

impl Origin {
    /// Parse an origin under the policy above. A single trailing `/` is tolerated.
    pub fn parse(text: &str) -> Result<Origin, OriginError> {
        let (origin, path) = split_url(text)?;
        if !path.is_empty() && path != "/" {
            return Err(OriginError::Invalid);
        }
        Ok(origin)
    }

    /// The origin Teams may talk to under `settings`.
    pub fn admitted(settings: &Settings) -> Result<Origin, OriginError> {
        Origin::parse(settings.teams_origin().ok_or(OriginError::NotConfigured)?)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `origin + path` (`path` starts with `/`).
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.0)
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An invite code (secret until used): `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct InviteCode(String);

impl fmt::Debug for InviteCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InviteCode(***)")
    }
}

impl InviteCode {
    /// Read an invite: a link `https://<admitted origin>/j/<code>` or the bare code. A link to
    /// any other origin is refused ([`OriginError::NotAdmitted`]).
    pub fn parse(input: &str, admitted: &Origin) -> Result<InviteCode, OriginError> {
        let input = input.trim();
        let code = if input.contains("://") {
            let (origin, path) = split_url(input).map_err(|e| match e {
                OriginError::Invalid => OriginError::BadInvite,
                other => other,
            })?;
            if &origin != admitted {
                return Err(OriginError::NotAdmitted);
            }
            path.strip_prefix("/j/").ok_or(OriginError::BadInvite)?
        } else {
            input
        };
        let ok = (8..=128).contains(&code.len())
            && code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if ok {
            Ok(InviteCode(code.to_string()))
        } else {
            Err(OriginError::BadInvite)
        }
    }

    /// The code itself (to put in a request body).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// `url` → (origin, path). Refuses user info, query, fragment, backslashes, whitespace and
/// control characters, and `http://` to anything but localhost.
fn split_url(url: &str) -> Result<(Origin, &str), OriginError> {
    let url = url.trim();
    let bad = |c: char| c.is_control() || c.is_whitespace() || matches!(c, '?' | '#' | '\\' | '@');
    if url.is_empty() || url.chars().any(bad) {
        return Err(OriginError::Invalid);
    }
    let (scheme, rest) = url.split_once("://").ok_or(OriginError::Invalid)?;
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let authority = authority.to_ascii_lowercase();
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (inner, after) = v6.split_once(']').ok_or(OriginError::Invalid)?;
            let port = match after {
                "" => None,
                p => Some(p.strip_prefix(':').ok_or(OriginError::Invalid)?),
            };
            (format!("[{inner}]"), port)
        }
        None => match authority.split_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (authority.clone(), None),
        },
    };
    let host_ok = !host.is_empty()
        && (host.starts_with('[')
            || host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.'));
    if !host_ok {
        return Err(OriginError::Invalid);
    }
    let port: Option<u16> = match port {
        None => None,
        Some(p) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => {
            Some(p.parse().map_err(|_| OriginError::Invalid)?)
        }
        Some(_) => return Err(OriginError::Invalid),
    };
    let default_port = match scheme.as_str() {
        "https" => 443,
        "http" if matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]") => 80,
        _ => return Err(OriginError::Invalid),
    };
    let origin = match port {
        Some(p) if p != default_port => format!("{scheme}://{host}:{p}"),
        _ => format!("{scheme}://{host}"),
    };
    Ok((Origin(origin), path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_follow_the_policy() {
        for (input, out) in [
            ("https://teams.example.test", "https://teams.example.test"),
            (
                "HTTPS://Teams.Example.Test:443/",
                "https://teams.example.test",
            ),
            (
                "https://teams.example.test:8443",
                "https://teams.example.test:8443",
            ),
            ("http://localhost:8080", "http://localhost:8080"),
            ("http://127.0.0.1", "http://127.0.0.1"),
            ("http://[::1]:3000", "http://[::1]:3000"),
        ] {
            assert_eq!(Origin::parse(input).unwrap().as_str(), out, "{input}");
        }
        for input in [
            "",
            "teams.example.test",
            "ftp://teams.example.test",
            "http://teams.example.test",
            "http://127.1",
            "http://localhost.:80",
            "https://user:pw@teams.example.test",
            "https://@teams.example.test",
            "https://teams.example.test/path",
            "https://teams.example.test/?q=1",
            "https://teams.example.test#frag",
            "https://teams.example.test:99999",
            "https://teams.example.test:",
            "https://teams example.test",
            "https://teams.example.test\\x",
            "https://",
        ] {
            assert!(Origin::parse(input).is_err(), "{input}");
        }
    }

    #[test]
    fn admitted_origin_needs_an_endpoint_while_the_official_one_is_empty() {
        let mut s = Settings::default();
        assert_eq!(Origin::admitted(&s), Err(OriginError::NotConfigured));
        s.teams_endpoint = " http://localhost:8080 ".into();
        assert_eq!(
            Origin::admitted(&s).unwrap().url("/v1/events"),
            "http://localhost:8080/v1/events"
        );
        s.teams_endpoint = "http://evil.example".into();
        assert_eq!(Origin::admitted(&s), Err(OriginError::Invalid));
    }

    #[test]
    fn invites_only_for_the_admitted_origin() {
        let admitted = Origin::parse("https://teams.example.test").unwrap();
        let code =
            InviteCode::parse("https://teams.example.test/j/AbC_123-xyz", &admitted).unwrap();
        assert_eq!(code.expose(), "AbC_123-xyz");
        assert!(!format!("{code:?}").contains("AbC"));
        assert_eq!(
            InviteCode::parse("AbC_123-xyz", &admitted)
                .unwrap()
                .expose(),
            "AbC_123-xyz"
        );
        assert_eq!(
            InviteCode::parse("https://other.example.test/j/AbC_123-xyz", &admitted),
            Err(OriginError::NotAdmitted)
        );
        for bad in [
            "https://teams.example.test/j/AbC_123-xyz?x=1",
            "https://teams.example.test/j/AbC_123-xyz#f",
            "https://u@teams.example.test/j/AbC_123-xyz",
            "https://teams.example.test/x/AbC_123-xyz",
            "https://teams.example.test/j/short",
            "https://teams.example.test/j/AbC_123-xyz/more",
            "code with spaces",
        ] {
            assert!(InviteCode::parse(bad, &admitted).is_err(), "{bad}");
        }
    }
}
