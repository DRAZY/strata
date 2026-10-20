//! Conflict copies that sync tools leave next to a file when two devices
//! changed it at once, recognized by name:
//!
//! - Syncthing: `note.sync-conflict-20261019-143012-ABCDEF1.md`
//! - Dropbox, Nextcloud, ownCloud: `note (Andre's MacBook's conflicted copy 2026-10-19).md`,
//!   `note (conflicted copy 2026-10-19 143012).md`, `note (Case Conflict).md`
//! - Generic: `note (conflict).md`, `note (conflict 2).md`
//!
//! iCloud's `note 2.md` isn't detected: it looks the same as a note someone
//! named that way.

use std::sync::LazyLock;

static SYNCTHING: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^(.+)\.sync-conflict-\d{8}-\d{6}(?:-[A-Za-z0-9]+)?((?:\.[^.]*)?)$").expect("valid"));
static PARENS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)^(.+?) \(([^()]*conflict[^()]*)\)((?:\.[^.]*)?)$").expect("valid"));

/// If `name` (a file name) is a conflict copy, the name of the file it's a
/// copy of.
pub fn original_name(name: &str) -> Option<String> {
    let caps = SYNCTHING.captures(name).or_else(|| PARENS.captures(name))?;
    let stem = caps.get(1)?.as_str();
    let ext = caps.get(caps.len() - 1).map_or("", |m| m.as_str());
    Some(format!("{stem}{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_sync_tools_conflict_copies() {
        let cases = [
            ("note.sync-conflict-20261019-143012-ABCDEF1.md", "note.md"),
            ("My Note.sync-conflict-20231231-000000.md", "My Note.md"),
            ("archive.tar.sync-conflict-20231231-000000-XYZ.gz", "archive.tar.gz"),
            ("note (conflict).md", "note.md"),
            ("note (Conflict 2).md", "note.md"),
            ("note (Andre's MacBook's conflicted copy).md", "note.md"),
            ("Daily (Andre Hall's conflicted copy 2026-10-19).md", "Daily.md"),
            ("note (conflicted copy 2026-10-19 143012).md", "note.md"),
            ("Readme (Case Conflict).md", "Readme.md"),
            ("Café (conflict).md", "Café.md"),
            ("notes (conflict)", "notes"),
        ];
        for (copy, original) in cases {
            assert_eq!(original_name(copy).as_deref(), Some(original), "{copy}");
        }
        for plain in ["note.md", "note 2.md", "Conflict resolution.md", "My (draft).md", "sync-conflict.md", "(conflict).md", "x.sync-conflict-2026-1.md"] {
            assert_eq!(original_name(plain), None, "{plain}");
        }
    }
}
