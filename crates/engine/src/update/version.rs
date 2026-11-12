//! Semantic versions (strict semver 2.0 precedence).

use std::cmp::Ordering;

/// A semantic version (`v` prefix and build metadata accepted; build metadata ignored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Pre-release identifiers (`rc.1` → `["rc", "1"]`); empty = release.
    pub pre: Vec<String>,
}

impl Version {
    /// `1.2.3`, `v1.2.3-rc.1+build`, `1.2` (patch 0). `None` when not a version.
    pub fn parse(text: &str) -> Option<Version> {
        let t = text.trim();
        let t = t.strip_prefix(['v', 'V']).unwrap_or(t);
        let t = t.split('+').next().unwrap_or(t);
        let (core, pre) = match t.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (t, None),
        };
        let mut parts = core.split('.');
        let num = |s: Option<&str>| -> Option<u64> {
            let s = s?;
            (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.parse().ok())
                .flatten()
        };
        let major = num(parts.next())?;
        let minor = match parts.next() {
            Some(m) => num(Some(m))?,
            None => 0,
        };
        let patch = match parts.next() {
            Some(p) => num(Some(p))?,
            None => 0,
        };
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(p) => {
                let ids: Vec<String> = p.split('.').map(str::to_string).collect();
                if ids.iter().any(|i| {
                    i.is_empty() || !i.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                }) {
                    return None;
                }
                ids
            }
        };
        Some(Version {
            major,
            minor,
            patch,
            pre,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    /// Semver 2.0 precedence.
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => cmp_pre(&self.pre, &other.pre),
            })
    }
}

fn cmp_pre(a: &[String], b: &[String]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let nx = x
            .bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| x.parse::<u64>().ok())
            .flatten();
        let ny = y
            .bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| y.parse::<u64>().ok())
            .flatten();
        let o = match (nx, ny) {
            (Some(p), Some(q)) => p.cmp(&q),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => x.cmp(y),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_versions() {
        assert_eq!(
            v("v1.2.3"),
            Version {
                major: 1,
                minor: 2,
                patch: 3,
                pre: vec![]
            }
        );
        assert_eq!(v("1.2").patch, 0);
        assert_eq!(v("1.0.0-rc.1+build.5").pre, ["rc", "1"]);
        assert_eq!(v("V2.0.0-beta").to_string(), "2.0.0-beta");
        for bad in [
            "",
            "v",
            "x1.0",
            "1.0.0.0",
            "1..0",
            "1.a.0",
            "1.0.0-",
            "1.0.0-a..b",
            "1.0.0-é",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn semver_precedence() {
        // The spec's own example chain.
        let chain = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "1.10.0",
            "2.0.0",
        ];
        for pair in chain.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert_eq!(v("1.0.0+a").cmp(&v("1.0.0+b")), Ordering::Equal);
        assert!(v("1.9.0") < v("1.10.0"), "numeric, not lexical");
    }
}
