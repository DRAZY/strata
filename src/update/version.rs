//! Semantic versions (semver.org 2.0) and their order, for "is this release
//! newer than me?". Build metadata (`+…`) is accepted and ignored, as the
//! spec says; a leading `v` (from a git tag) is allowed.

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Prerelease identifiers (`rc.1` → ["rc", 1]); empty for a release.
    pub pre: Vec<Ident>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ident {
    Num(u64),
    Alpha(String),
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        let s = s.split_once('+').map_or(s, |(v, build)| if valid_build(build) { v } else { "\u{0}" });
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let mut nums = core.split('.').map(number);
        let (major, minor, patch) = (nums.next()??, nums.next()??, nums.next()??);
        if nums.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(p) => p.split('.').map(ident).collect::<Option<Vec<_>>>()?,
        };
        Some(Self { major, minor, patch, pre })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

/// A numeric part: digits, no leading zero.
fn number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0')) {
        return None;
    }
    s.parse().ok()
}

fn ident(s: &str) -> Option<Ident> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return None;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return number(s).map(Ident::Num);
    }
    Some(Ident::Alpha(s.to_owned()))
}

fn valid_build(s: &str) -> bool {
    s.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Ident::Num(a), Ident::Num(b)) => a.cmp(b),
            (Ident::Num(_), Ident::Alpha(_)) => Ordering::Less,
            (Ident::Alpha(_), Ident::Num(_)) => Ordering::Greater,
            (Ident::Alpha(a), Ident::Alpha(b)) => a.as_bytes().cmp(b.as_bytes()),
        }
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            // A prerelease comes before its release.
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => self.pre.cmp(&other.pre),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (i, p) in self.pre.iter().enumerate() {
            f.write_str(if i == 0 { "-" } else { "." })?;
            match p {
                Ident::Num(n) => write!(f, "{n}")?,
                Ident::Alpha(s) => f.write_str(s)?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    #[test]
    fn parses_and_prints() {
        assert_eq!(v("1.2.3").to_string(), "1.2.3");
        assert_eq!(v("v1.0.0-rc.1").to_string(), "1.0.0-rc.1");
        assert_eq!(v("1.0.0-alpha+build.5"), v("1.0.0-alpha"));
        assert!(v("1.0.0-rc.1").is_prerelease() && !v("1.0.0").is_prerelease());
        assert_eq!(v(env!("CARGO_PKG_VERSION")).to_string(), env!("CARGO_PKG_VERSION"));
        for bad in ["", "1", "1.2", "1.2.3.4", "01.2.3", "1.2.3-", "1.2.3-01", "1.2.3-a..b", "1.2.x", "1.2.3+", "1.2.3+a..b", "1.2.3-ä", " v", "-1.2.3"] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn semver_precedence() {
        // The order in semver.org §11, plus the release after its candidates.
        let order = ["1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0", "1.0.1", "1.1.0-rc.1", "1.1.0", "2.0.0", "10.0.0"];
        for w in order.windows(2) {
            assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        }
        assert!(v("0.9.10") > v("0.9.9"), "numeric, not string, order");
        assert_eq!(v("1.0.0+a").cmp(&v("1.0.0+b")), Ordering::Equal);
    }
}
