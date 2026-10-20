//! `file://` URIs, which the protocol names documents by.

use std::path::{Path, PathBuf};

/// The URI of an absolute path.
pub fn from_path(path: &Path) -> String {
    from_path_str(&path.to_string_lossy(), cfg!(windows))
}

/// The path a `file://` URI names, or None for other schemes.
pub fn to_path(uri: &str) -> Option<PathBuf> {
    to_path_str(uri, cfg!(windows)).map(PathBuf::from)
}

/// The form paths are compared in: on Windows the drive letter in upper
/// case (servers write `c:` or `C:` as they please).
pub fn normalize(path: &Path) -> PathBuf {
    if cfg!(windows) { PathBuf::from(upper_drive(&path.to_string_lossy())) } else { path.to_owned() }
}

fn upper_drive(p: &str) -> String {
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        let mut s = p.to_owned();
        s[..1].make_ascii_uppercase();
        return s;
    }
    p.to_owned()
}

fn from_path_str(path: &str, windows: bool) -> String {
    let mut p = path.to_owned();
    let mut authority = String::new();
    if windows {
        p = p.replace('\\', "/");
        if let Some(unc) = p.strip_prefix("//") {
            // \\server\share\file → file://server/share/file
            let (host, rest) = unc.split_once('/').unwrap_or((unc, ""));
            authority = host.to_owned();
            p = format!("/{rest}");
        } else if !p.starts_with('/') {
            p = format!("/{p}");
        }
    }
    let mut out = format!("file://{authority}");
    for (i, &b) in p.as_bytes().iter().enumerate() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
            // The drive letter's colon (`/C:/`) stays readable, as most clients write it.
            || (windows && b == b':' && i == 2);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn to_path_str(uri: &str, windows: bool) -> Option<String> {
    let rest = uri.get(..7).filter(|s| s.eq_ignore_ascii_case("file://")).map(|_| &uri[7..])?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = decode(path)?;
    if !windows {
        return Some(path);
    }
    let authority = decode(authority)?;
    if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
        return Some(format!("\\\\{authority}{}", path.replace('/', "\\")));
    }
    // /c:/x → C:\x
    let b = path.as_bytes();
    let path = if b.len() >= 3 && b[0] == b'/' && b[2] == b':' && b[1].is_ascii_alphabetic() { &path[1..] } else { &path[..] };
    Some(upper_drive(&path.replace('/', "\\")))
}

/// Percent-decoding; None if the result isn't UTF-8.
fn decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_paths() {
        let uri = from_path_str("/Users/me/my project/naïve#1.rs", false);
        assert_eq!(uri, "file:///Users/me/my%20project/na%C3%AFve%231.rs");
        assert_eq!(to_path_str(&uri, false).as_deref(), Some("/Users/me/my project/naïve#1.rs"));
        assert_eq!(to_path_str("file:///a/b%3Ac", false).as_deref(), Some("/a/b:c"));
        assert_eq!(to_path_str("FILE:///x", false).as_deref(), Some("/x"));
        assert_eq!(to_path_str("untitled:Untitled-1", false), None);
        assert_eq!(to_path_str("https://example.com/x", false), None);
    }

    #[test]
    fn windows_paths() {
        let uri = from_path_str(r"C:\Users\me\a b\x.rs", true);
        assert_eq!(uri, "file:///C:/Users/me/a%20b/x.rs");
        assert_eq!(to_path_str(&uri, true).as_deref(), Some(r"C:\Users\me\a b\x.rs"));
        // Servers lower the drive letter or encode its colon: the same path either way.
        assert_eq!(to_path_str("file:///c%3A/Users/me/x.rs", true).as_deref(), Some(r"C:\Users\me\x.rs"));
        assert_eq!(to_path_str("file:///c:/Users/me/x.rs", true).as_deref(), Some(r"C:\Users\me\x.rs"));
        // UNC shares.
        let unc = from_path_str(r"\\server\share\dir\x.rs", true);
        assert_eq!(unc, "file://server/share/dir/x.rs");
        assert_eq!(to_path_str(&unc, true).as_deref(), Some(r"\\server\share\dir\x.rs"));
        assert_eq!(upper_drive("d:\\x"), "D:\\x");
    }
}
