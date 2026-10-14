//! Owner-only files in the config folder. The session holds the text of
//! unsaved tabs, logs hold paths, crash reports hold backtraces; none of that
//! is for other accounts on the machine. On Unix the config folder and the
//! folders Strata keeps in it are created 0700 and the files Strata writes
//! there 0600, and folders left readable by an older Strata are fixed at
//! startup. On Windows the per-user `%APPDATA%` folder's ACL already keeps
//! other users out, so these are plain creates there.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// Folders in the config folder that hold what Strata itself writes (files in
/// them are made owner-only too). `backups.recovered-*` folders count as well.
const DATA_DIRS: &[&str] = &["backups", "Logs", "Crashes", "Print"];
/// Folders of the user's own files (snippets, syntaxes...): the folder is made
/// owner-only, the files in it are left as they are.
const USER_DIRS: &[&str] = &["Snippets", "build_systems", "Syntaxes", "Themes"];
/// Files Strata keeps at the top of the config folder.
const FILES: &[&str] = &["settings.json", "keybindings.json", "session.json", "session.json.tmp", "session.lock", "strata.port"];

/// `create_dir_all`, with every folder it creates owner-only (folders that
/// already exist are left to `harden`).
pub fn create_dir_all(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

/// Options for a new owner-only file (0600 on Unix when it's created).
pub fn options() -> OpenOptions {
    #[allow(unused_mut)]
    let mut o = OpenOptions::new();
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o
}

/// `File::create`, owner-only (an existing file is made owner-only too).
pub fn create(path: &Path) -> io::Result<File> {
    let f = options().write(true).create(true).truncate(true).open(path)?;
    restrict(path, false);
    Ok(f)
}

/// `fs::write`, owner-only.
pub fn write(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    io::Write::write_all(&mut create(path)?, contents.as_ref())
}

/// Make an existing file (0600) or folder (0700) owner-only. Quietly does
/// nothing for symlinks, for what we don't own, and off Unix.
pub fn restrict(path: &Path, dir: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(meta) = std::fs::symlink_metadata(path) else { return };
        if meta.file_type().is_symlink() || meta.is_dir() != dir {
            return;
        }
        let want = if dir { 0o700 } else { 0o600 };
        let mode = meta.permissions().mode() & 0o777;
        // Only take permissions away (an owner's x bit on a file stays).
        let new = mode & (want | 0o100);
        if new != mode {
            // Fails (EPERM) for files we don't own; that's fine.
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(new));
        }
    }
    #[cfg(not(unix))]
    let _ = (path, dir);
}

/// At startup: make the config folder, the folders Strata keeps in it and
/// the files it writes owner-only. `whole` also restricts `config` itself;
/// pass false when the folder was chosen with `STRATA_CONFIG_DIR` (it might
/// be a shared folder that isn't Strata's alone).
pub fn harden(config: &Path, whole: bool) {
    if whole {
        restrict(config, true);
    }
    for name in FILES {
        restrict(&config.join(name), false);
    }
    for name in USER_DIRS {
        restrict(&config.join(name), true);
    }
    let Ok(entries) = std::fs::read_dir(config) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if DATA_DIRS.contains(&name.as_str()) || name.starts_with("backups.recovered-") {
            harden_tree(&e.path(), 0);
        }
    }
}

/// A data folder and everything in it (symlinks are never followed).
fn harden_tree(dir: &Path, depth: usize) {
    restrict(dir, true);
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        match e.file_type() {
            Ok(t) if t.is_dir() => harden_tree(&e.path(), depth + 1),
            Ok(t) if t.is_file() => restrict(&e.path(), false),
            _ => {}
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(p: &Path) -> u32 {
        std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("strata-private-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn new_folders_and_files_are_owner_only() {
        let d = scratch("new");
        let deep = d.join("a/b");
        create_dir_all(&deep).unwrap();
        assert_eq!(mode(&d), 0o700);
        assert_eq!(mode(&d.join("a")), 0o700);
        assert_eq!(mode(&deep), 0o700);
        write(&deep.join("f.txt"), "secret").unwrap();
        assert_eq!(mode(&deep.join("f.txt")), 0o600);
        // Rewriting a file that was world-readable makes it owner-only.
        std::fs::set_permissions(deep.join("f.txt"), std::fs::Permissions::from_mode(0o644)).unwrap();
        write(&deep.join("f.txt"), "again").unwrap();
        assert_eq!(mode(&deep.join("f.txt")), 0o600);
        let mut f = options().create(true).append(true).open(deep.join("log")).unwrap();
        std::io::Write::write_all(&mut f, b"x").unwrap();
        assert_eq!(mode(&deep.join("log")), 0o600);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn session_logs_and_crash_files_are_owner_only() {
        let d = scratch("session");
        let session = crate::session::Session { tabs: vec![crate::session::TabState { backup: Some("1-0.txt".into()), ..Default::default() }], ..Default::default() };
        session.save(&d, &[("1-0.txt".into(), ropey::Rope::from_str("unsaved secret"))]).unwrap();
        assert_eq!(mode(&d), 0o700);
        assert_eq!(mode(&d.join("session.json")), 0o600);
        assert_eq!(mode(&crate::session::Session::backups(&d)), 0o700);
        assert_eq!(mode(&crate::session::Session::backups(&d).join("1-0.txt")), 0o600);
        crate::session::Settings::default().save(&d).unwrap();
        assert_eq!(mode(&crate::session::Settings::path(&d)), 0o600);
        let rec = crate::crash::write_recovered(&d, &[("a".into(), "text".into())]).unwrap();
        assert_eq!(mode(&rec), 0o700);
        for e in std::fs::read_dir(&rec).unwrap().flatten() {
            assert_eq!(mode(&e.path()), 0o600);
        }
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn startup_fixes_an_open_config_folder() {
        let d = scratch("old");
        for sub in ["backups", "Logs", "Crashes", "backups.recovered-1", "Snippets"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
            std::fs::set_permissions(d.join(sub), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let files = ["session.json", "settings.json", "keybindings.json", "backups/1-0.txt", "Logs/strata.log", "Crashes/strata-crash-x.txt", "backups.recovered-1/01 a.txt"];
        for f in files {
            std::fs::write(d.join(f), "x").unwrap();
            std::fs::set_permissions(d.join(f), std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        std::fs::write(d.join("Snippets/mine.sublime-snippet"), "x").unwrap();
        std::fs::set_permissions(d.join("Snippets/mine.sublime-snippet"), std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(d.join("unrelated.txt"), "x").unwrap();
        std::fs::set_permissions(d.join("unrelated.txt"), std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A symlink in a data folder is left alone, and so is what it points to.
        let outside = scratch("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("t"), "x").unwrap();
        std::fs::set_permissions(outside.join("t"), std::fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(outside.join("t"), d.join("Logs/link")).unwrap();

        harden(&d, false);
        assert_eq!(mode(&d), 0o755, "an overridden config folder itself is left alone");
        harden(&d, true);
        assert_eq!(mode(&d), 0o700);
        for sub in ["backups", "Logs", "Crashes", "backups.recovered-1", "Snippets"] {
            assert_eq!(mode(&d.join(sub)), 0o700, "{sub}");
        }
        for f in files {
            assert_eq!(mode(&d.join(f)), 0o600, "{f}");
        }
        assert_eq!(mode(&d.join("Snippets/mine.sublime-snippet")), 0o644, "the user's own files keep their modes");
        assert_eq!(mode(&d.join("unrelated.txt")), 0o644, "files Strata doesn't know are left alone");
        assert_eq!(mode(&outside.join("t")), 0o644, "symlinks aren't followed");
        std::fs::remove_dir_all(d).ok();
        std::fs::remove_dir_all(outside).ok();
    }
}
