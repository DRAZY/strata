//! `workspace/readFile` and `workspace/writeFile`: files under the folders
//! open in the side bar, and nothing else. A path is matched to an open
//! folder by its spelling (absolute, no `.` or `..`), then reached from that
//! folder one name at a time without following any link or junction
//! (`claim`), so a link inside a folder can't lead a plugin outside it, and
//! a folder swapped for a link while the plugin works doesn't either.
//! Writes go to a new file renamed over the old one: a link at the file's
//! name is refused, never written through. Git's own folders are off limits
//! (a hook written there would run on the next commit), judged by the
//! folder's real path too, so another spelling of `.git` doesn't get in.

use super::api::{IO_ERROR, MAX_TEXT, OUTSIDE_WORKSPACE, RpcError, TOO_LARGE};
use crate::claim::imp;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

fn is_git(name: &str) -> bool {
    name.eq_ignore_ascii_case(".git")
}

/// The open folder `path` is in, and its names below that folder.
fn locate(roots: &[PathBuf], path: &Path) -> Result<(PathBuf, Vec<String>), RpcError> {
    let outside = || RpcError::new(OUTSIDE_WORKSPACE, format!("{} isn't inside a folder open in Strata", path.display()));
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) {
        return Err(outside());
    }
    // The innermost folder wins when one open folder is inside another.
    let root = roots.iter().filter(|r| path.starts_with(r) && path != r.as_path()).max_by_key(|r| r.components().count()).ok_or_else(outside)?;
    let parts: Vec<String> = path
        .strip_prefix(root)
        .map_err(|_| outside())?
        .components()
        .map(|c| match c {
            Component::Normal(s) => s.to_str().map(str::to_owned),
            _ => None,
        })
        .collect::<Option<_>>()
        .ok_or_else(outside)?;
    if parts.iter().any(|p| is_git(p)) {
        return Err(RpcError::new(OUTSIDE_WORKSPACE, "plugins can't read or write inside .git folders"));
    }
    // Windows reads `.git.`, `.git ` and `GIT~1` as `.git`, and `name:x` as
    // a hidden stream of `name`: names it would change aren't taken at all.
    #[cfg(windows)]
    if let Some(p) = parts.iter().find(|p| !super::zip::portable_name(p) || p.split('~').nth(1).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))) {
        return Err(RpcError::new(OUTSIDE_WORKSPACE, format!("\"{p}\" isn't a name plugins may use on Windows (a short name, a stream, or one Windows would change)")));
    }
    Ok((root.clone(), parts))
}

/// How many names (hard links) the open file has. One elsewhere on the disk
/// would let a plugin read or change a file outside the open folders.
fn links(file: &std::fs::File) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        file.metadata().map_or(u64::MAX, |m| m.nlink())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
        // SAFETY: plain data, filled in by the call.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a valid handle and a valid out pointer.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
            return u64::MAX;
        }
        info.nNumberOfLinks as u64
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        u64::MAX
    }
}

fn hard_linked(path: &Path) -> RpcError {
    RpcError::new(OUTSIDE_WORKSPACE, format!("{} has other names (hard links), maybe outside the open folders, so plugins can't read or write it", path.display()))
}

/// The folder holding the file, reached without following links, and not
/// inside a `.git` folder by its real path.
fn open_dir(root: &Path, dirs: &[&str], path: &Path) -> Result<imp::Dir, RpcError> {
    let dir = imp::open_parent(root, dirs).map_err(|_| RpcError::new(OUTSIDE_WORKSPACE, format!("{}: a folder on the way doesn't exist or is a link", path.display())))?;
    let real = imp::real_path(&dir).map_err(|e| RpcError::new(IO_ERROR, format!("couldn't check where {} is: {e}", path.display())))?;
    if real.components().any(|c| is_git(&c.as_os_str().to_string_lossy())) {
        return Err(RpcError::new(OUTSIDE_WORKSPACE, "plugins can't read or write inside .git folders"));
    }
    Ok(dir)
}

/// The text of a UTF-8 file under an open folder.
pub fn read(roots: &[PathBuf], path: &Path) -> Result<String, RpcError> {
    let (root, parts) = locate(roots, path)?;
    let (name, dirs) = parts.split_last().expect("a path below a folder has a name");
    let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
    let dir = open_dir(&root, &dirs, path)?;
    let file = match imp::open_file_at(&dir, name) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(RpcError::new(IO_ERROR, format!("{} doesn't exist", path.display()))),
        // O_NOFOLLOW on a link: ELOOP (or EMLINK on some systems).
        Err(_) => return Err(RpcError::new(OUTSIDE_WORKSPACE, format!("{} is a link, which plugins can't read through", path.display()))),
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err(RpcError::new(IO_ERROR, format!("{} isn't a file that can be read", path.display())));
    }
    if links(&file) > 1 {
        return Err(hard_linked(path));
    }
    let mut bytes = Vec::new();
    (&file).take(MAX_TEXT as u64 + 1).read_to_end(&mut bytes).map_err(|e| RpcError::new(IO_ERROR, format!("couldn't read {}: {e}", path.display())))?;
    if bytes.len() > MAX_TEXT {
        return Err(RpcError::new(TOO_LARGE, format!("{} is larger than 16 MB", path.display())));
    }
    String::from_utf8(bytes).map_err(|_| RpcError::new(IO_ERROR, format!("{} isn't UTF-8 text", path.display())))
}

/// Create or replace a file under an open folder. Its folder must exist.
pub fn write(roots: &[PathBuf], path: &Path, text: &str) -> Result<(), RpcError> {
    if text.len() > MAX_TEXT {
        return Err(RpcError::new(TOO_LARGE, "the text is larger than 16 MB"));
    }
    let (root, parts) = locate(roots, path)?;
    let (name, dirs) = parts.split_last().expect("a path below a folder has a name");
    let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
    let dir = open_dir(&root, &dirs, path)?;
    // What's at the name now: nothing, or a plain file (never a link or a folder).
    #[allow(unused_mut, unused_assignments)]
    let mut mode = 0o644u32;
    match imp::open_file_at(&dir, name) {
        Ok(f) => {
            let meta = f.metadata().map_err(|e| RpcError::new(IO_ERROR, e.to_string()))?;
            if !meta.is_file() {
                return Err(RpcError::new(OUTSIDE_WORKSPACE, format!("{} is a link or a folder, not a file", path.display())));
            }
            if links(&f) > 1 {
                return Err(hard_linked(path));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // The permission bits only: setuid and the like aren't carried onto what a plugin wrote.
                mode = meta.permissions().mode() & 0o777;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        // O_NOFOLLOW on a link: ELOOP (or EMLINK on some systems).
        Err(_) => return Err(RpcError::new(OUTSIDE_WORKSPACE, format!("{} is a link, which plugins can't write through", path.display()))),
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let tmp = format!(".{name}.strata-plugin-{}-{nanos}.tmp", std::process::id());
    imp::write_new_mode(&dir, &tmp, text.as_bytes(), mode).map_err(|e| RpcError::new(IO_ERROR, format!("couldn't write {}: {e}", path.display())))?;
    if let Err(e) = imp::rename_replace(&dir, &tmp, name) {
        let _ = imp::remove_file_at(&dir, &tmp);
        return Err(RpcError::new(IO_ERROR, format!("couldn't write {}: {e}", path.display())));
    }
    let _ = imp::sync_dir(&dir);
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn setup(tag: &str) -> (PathBuf, PathBuf) {
        let base = std::fs::canonicalize(super::super::package::tests::temp(tag)).unwrap();
        let root = base.join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.txt"), "hello").unwrap();
        std::fs::create_dir_all(base.join("secret")).unwrap();
        std::fs::write(base.join("secret/key"), "TOP SECRET").unwrap();
        (base, root)
    }

    #[test]
    fn reads_and_writes_inside_open_folders() {
        let (base, root) = setup("ws-ok");
        let roots = vec![root.clone()];
        assert_eq!(read(&roots, &root.join("src/a.txt")).unwrap(), "hello");
        write(&roots, &root.join("src/new.txt"), "fresh").unwrap();
        assert_eq!(std::fs::read_to_string(root.join("src/new.txt")).unwrap(), "fresh");
        write(&roots, &root.join("src/a.txt"), "replaced").unwrap();
        assert_eq!(std::fs::read_to_string(root.join("src/a.txt")).unwrap(), "replaced");
        let left: Vec<String> = std::fs::read_dir(root.join("src")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(left.iter().all(|n| !n.contains("strata-plugin")), "no temporary files left: {left:?}");
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn keeps_a_files_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (base, root) = setup("ws-mode");
        let script = root.join("run.sh");
        std::fs::write(&script, "old").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o750)).unwrap();
        write(std::slice::from_ref(&root), &script, "new").unwrap();
        assert_eq!(std::fs::metadata(&script).unwrap().permissions().mode() & 0o777, 0o750);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn refuses_paths_outside_the_folders() {
        let (base, root) = setup("ws-out");
        let roots = vec![root.clone()];
        for p in [base.join("secret/key"), root.join("../secret/key"), root.join("src/../../secret/key"), PathBuf::from("src/a.txt"), root.clone(), PathBuf::from("/etc/passwd")] {
            assert_eq!(read(&roots, &p).unwrap_err().code, OUTSIDE_WORKSPACE, "read {}", p.display());
            assert_eq!(write(&roots, &p, "x").unwrap_err().code, OUTSIDE_WORKSPACE, "write {}", p.display());
        }
        assert_eq!(std::fs::read_to_string(base.join("secret/key")).unwrap(), "TOP SECRET");
        assert!(read(&[], &root.join("src/a.txt")).is_err(), "no folders open: nothing");
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn refuses_links_on_the_way_and_at_the_file() {
        let (base, root) = setup("ws-links");
        let roots = vec![root.clone()];
        // A folder link inside the project pointing outside it.
        std::os::unix::fs::symlink(base.join("secret"), root.join("escape")).unwrap();
        assert_eq!(read(&roots, &root.join("escape/key")).unwrap_err().code, OUTSIDE_WORKSPACE);
        assert_eq!(write(&roots, &root.join("escape/key"), "pwned").unwrap_err().code, OUTSIDE_WORKSPACE);
        assert_eq!(write(&roots, &root.join("escape/new"), "pwned").unwrap_err().code, OUTSIDE_WORKSPACE);
        // A file link pointing outside.
        std::os::unix::fs::symlink(base.join("secret/key"), root.join("src/key-link")).unwrap();
        assert_eq!(read(&roots, &root.join("src/key-link")).unwrap_err().code, OUTSIDE_WORKSPACE);
        assert_eq!(write(&roots, &root.join("src/key-link"), "pwned").unwrap_err().code, OUTSIDE_WORKSPACE);
        assert_eq!(std::fs::read_to_string(base.join("secret/key")).unwrap(), "TOP SECRET", "nothing written through a link");
        assert!(std::fs::symlink_metadata(root.join("src/key-link")).unwrap().file_type().is_symlink(), "the link itself is left alone");
        // Git's folder.
        std::fs::create_dir_all(root.join(".git/hooks")).unwrap();
        let e = write(&roots, &root.join(".git/hooks/pre-commit"), "#!/bin/sh\n").unwrap_err();
        assert!(e.message.contains(".git"), "{}", e.message);
        assert!(!root.join(".git/hooks/pre-commit").exists());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn named_pipes_are_refused_without_waiting() {
        let (base, root) = setup("ws-fifo");
        let fifo = root.join("pipe");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a valid path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        assert_eq!(read(std::slice::from_ref(&root), &fifo).unwrap_err().code, IO_ERROR);
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "no writer, no wait");
        assert_eq!(write(std::slice::from_ref(&root), &fifo, "x").unwrap_err().code, OUTSIDE_WORKSPACE, "not replaced either");
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn special_mode_bits_are_not_kept() {
        use std::os::unix::fs::PermissionsExt;
        let (base, root) = setup("ws-suid");
        let f = root.join("tool");
        std::fs::write(&f, "old").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o4755)).unwrap();
        write(std::slice::from_ref(&root), &f, "new").unwrap();
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o7777, 0o755);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn an_open_folder_inside_git_is_still_git() {
        let (base, root) = setup("ws-in-git");
        // The user opened a folder that is itself inside a repository's .git.
        let inner = root.join(".git").join("hooks");
        std::fs::create_dir_all(&inner).unwrap();
        let e = write(std::slice::from_ref(&inner), &inner.join("pre-commit"), "#!/bin/sh\n").unwrap_err();
        assert_eq!(e.code, OUTSIDE_WORKSPACE, "{}", e.message);
        assert!(!inner.join("pre-commit").exists());
        assert!(read(std::slice::from_ref(&inner), &inner.join("pre-commit.sample")).is_err());
        // Another spelling of the name, for case-insensitive disks.
        assert_eq!(write(std::slice::from_ref(&root), &root.join(".GIT/hooks/pre-commit"), "x").unwrap_err().code, OUTSIDE_WORKSPACE);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn hard_linked_files_are_refused() {
        let (base, root) = setup("ws-hardlink");
        let roots = vec![root.clone()];
        // A name inside the folder for a file that also lives outside it.
        std::fs::hard_link(base.join("secret/key"), root.join("src/key")).unwrap();
        let e = read(&roots, &root.join("src/key")).unwrap_err();
        assert_eq!(e.code, OUTSIDE_WORKSPACE, "{}", e.message);
        assert!(e.message.contains("hard link"), "{}", e.message);
        let e = write(&roots, &root.join("src/key"), "pwned").unwrap_err();
        assert_eq!(e.code, OUTSIDE_WORKSPACE, "{}", e.message);
        assert_eq!(std::fs::read_to_string(base.join("secret/key")).unwrap(), "TOP SECRET");
        assert_eq!(std::fs::read_to_string(root.join("src/key")).unwrap(), "TOP SECRET", "the inside name isn't replaced either");
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn limits_and_odd_files() {
        let (base, root) = setup("ws-limits");
        let roots = vec![root.clone()];
        let big = "x".repeat(MAX_TEXT + 1);
        assert_eq!(write(&roots, &root.join("big"), &big).unwrap_err().code, TOO_LARGE);
        std::fs::write(root.join("bin"), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(read(&roots, &root.join("bin")).unwrap_err().code, IO_ERROR);
        assert_eq!(write(&roots, &root.join("src"), "x").unwrap_err().code, OUTSIDE_WORKSPACE, "a folder isn't overwritten");
        assert_eq!(write(&roots, &root.join("nope/new.txt"), "x").unwrap_err().code, OUTSIDE_WORKSPACE, "folders aren't created");
        std::fs::remove_dir_all(base).ok();
    }
}
