//! Real paths as other programs read them. On Windows `canonicalize` gives
//! `\\?\C:\…` and `\\?\UNC\server\…` (verbatim paths), which the shell
//! (the Recycle Bin), Git, language servers and settings.json don't take
//! or don't match: those get the ordinary spelling.

use std::path::{Path, PathBuf};

/// Windows' `\\?\C:\…` and `\\?\UNC\server\…` as ordinary paths. Other
/// paths (including other verbatim ones, which have no ordinary spelling)
/// are unchanged.
pub fn plain(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p,
    }
}

/// `path` with links and `..` resolved, spelled as `plain` does.
pub fn canonical(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(plain)
}

/// Set a file's or folder's modification time (tests that age things).
/// Windows opens a folder only with backup semantics, and changes its time
/// only through a handle opened for writing.
#[cfg(test)]
pub fn set_modified(path: &Path, when: std::time::SystemTime) {
    let mut o = std::fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        o.write(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    #[cfg(not(windows))]
    o.read(true);
    o.open(path).and_then(|f| f.set_modified(when)).unwrap_or_else(|e| panic!("couldn't set the time of {}: {e}", path.display()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_verbatim_paths_become_plain() {
        assert_eq!(plain(PathBuf::from(r"\\?\C:\Users\me\Notes")), PathBuf::from(r"C:\Users\me\Notes"));
        assert_eq!(plain(PathBuf::from(r"\\?\UNC\server\share\Notes")), PathBuf::from(r"\\server\share\Notes"));
        assert_eq!(plain(PathBuf::from(r"\\?\Volume{1234}\x")), PathBuf::from(r"\\?\Volume{1234}\x"));
        assert_eq!(plain(PathBuf::from("/Users/me/Notes")), PathBuf::from("/Users/me/Notes"));
    }

    #[test]
    fn folders_and_files_can_be_aged() {
        let dir = std::env::temp_dir().join(format!("strata-paths-age-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f"), "x").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        for p in [dir.join("f"), dir.clone()] {
            set_modified(&p, old);
            let age = std::fs::metadata(&p).unwrap().modified().unwrap().elapsed().unwrap();
            assert!(age.as_secs() >= 7000, "{}: {age:?}", p.display());
        }
        std::fs::remove_dir_all(dir).ok();
    }
}
