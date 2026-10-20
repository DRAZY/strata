//! Moving a file Review Changes listed to the Trash, and only that file.
//!
//! Checking a path and then trashing it are two lookups, and an agent (or
//! anything else) can swap the file, or a folder above it, in between. So
//! the file is claimed first, then checked: it's renamed into a private
//! staging folder next to it (through a handle on its folder, opened without
//! following links), the claimed file is compared with what was listed
//! (identity, size, time, every byte), and only then moved on to the Trash.
//! If it isn't the listed file it goes back where it was; if that name has
//! been taken meanwhile it stays in the staging folder and the caller is told
//! where. Nothing is ever deleted.

use std::hash::Hasher;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Staging folders start with this; a scan reports any left over.
pub const STAGING: &str = ".strata-trash-";

/// The file as it was listed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Expected {
    /// Which file (device and inode, or volume and file index); None: unknown.
    pub id: Option<(u64, u64)>,
    pub len: u64,
    pub mtime: Option<SystemTime>,
    /// `fingerprint` of every byte.
    pub hash: u64,
}

/// Why nothing went to the Trash.
#[derive(Debug, PartialEq)]
pub enum Refused {
    /// It isn't the file listed (changed, replaced) or wasn't there.
    Changed,
    /// It isn't really inside the repository: a link (or junction) on the way.
    Outside,
    /// A renamed file's old name exists again.
    OldBack,
    /// It wasn't the listed file and its name was taken meanwhile: it's here.
    KeptAt(PathBuf),
    Failed(String),
}

/// Points between the steps, for tests to change things at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The path was checked; nothing claimed yet.
    BeforeClaim,
    /// Claimed into the staging folder, not checked yet.
    Claimed,
}

/// The fingerprint of `len` bytes from `bytes` (the same for a slice and a stream).
pub fn fingerprint(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write_usize(bytes.len());
    h.write(bytes);
    h.finish()
}

/// `fingerprint` of a whole file, read a piece at a time (any size).
pub fn fingerprint_file(file: &mut std::fs::File) -> Option<u64> {
    let len = file.metadata().ok()?.len() as usize;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write_usize(len);
    let (mut buf, mut read) = (vec![0u8; 1 << 20], 0usize);
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.write(&buf[..n]);
        read += n;
    }
    (read == len).then(|| h.finish())
}

/// Which file `path` is (device and inode; volume and file index on
/// Windows), without following a link at the end.
pub fn file_id(path: &Path) -> Option<(u64, u64)> {
    let file = imp::open_file(path).ok()?;
    imp::id_of(&file)
}

/// Open `path` for reading without following a link at the end.
pub fn open_plain(path: &Path) -> std::io::Result<std::fs::File> {
    imp::open_file(path)
}

/// `claimed` is the listed file, and a renamed file's old name is still free.
fn check(claimed: &mut std::fs::File, expected: &Expected, old: Option<&Path>) -> Result<(), Refused> {
    if old.is_some_and(|o| std::fs::symlink_metadata(o).is_ok()) {
        return Err(Refused::OldBack);
    }
    let Ok(meta) = claimed.metadata() else { return Err(Refused::Changed) };
    let same = meta.is_file()
        && meta.len() == expected.len
        && meta.modified().ok() == expected.mtime
        && (expected.id.is_none() || imp::id_of(claimed) == expected.id)
        && fingerprint_file(claimed) == Some(expected.hash);
    if same { Ok(()) } else { Err(Refused::Changed) }
}

/// Move `path` (inside `top`) to the Trash if it is the file `expected`
/// describes; for a renamed file, `old` (its old path) must still be free.
pub fn trash_listed(top: &Path, path: &Path, old: Option<&Path>, expected: &Expected) -> Result<(), Refused> {
    trash_listed_with(top, path, old, expected, &mut |_| {})
}

/// `trash_listed`, calling `hook` between the steps.
pub fn trash_listed_with(top: &Path, path: &Path, old: Option<&Path>, expected: &Expected, hook: &mut dyn FnMut(Step)) -> Result<(), Refused> {
    let rel = path.strip_prefix(top).map_err(|_| Refused::Outside)?;
    let parts: Vec<&str> = rel.components().map(|c| match c {
        std::path::Component::Normal(s) => s.to_str(),
        _ => None,
    }).collect::<Option<_>>().ok_or(Refused::Outside)?;
    let Some((name, dirs)) = parts.split_last() else { return Err(Refused::Outside) };
    if old.is_some_and(|o| std::fs::symlink_metadata(o).is_ok()) {
        return Err(Refused::OldBack);
    }
    hook(Step::BeforeClaim);
    // The folder, reached from the top without following any link.
    let parent = imp::open_parent(top, dirs).map_err(|_| Refused::Outside)?;
    let stage_name = format!("{STAGING}{}-{}", std::process::id(), SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    imp::make_dir(&parent, &stage_name).map_err(|e| Refused::Failed(e.to_string()))?;
    let stage = match imp::open_dir(&parent, &stage_name) {
        Ok(s) => s,
        Err(e) => {
            let _ = imp::remove_empty_dir(&parent, &stage_name);
            return Err(Refused::Failed(e.to_string()));
        }
    };
    let staged_path = path.parent().unwrap_or(top).join(&stage_name).join(name);
    if imp::move_noreplace(&parent, name, &stage, name).is_err() {
        let _ = imp::remove_empty_dir(&parent, &stage_name);
        return Err(Refused::Changed);
    }
    hook(Step::Claimed);
    // Not the listed file: put it back, or say where it is.
    let put_back = |why: Refused| -> Refused {
        match imp::move_noreplace(&stage, name, &parent, name) {
            Ok(()) => {
                let _ = imp::remove_empty_dir(&parent, &stage_name);
                why
            }
            Err(_) => Refused::KeptAt(staged_path.clone()),
        }
    };
    let checked = match imp::open_file_at(&stage, name) {
        Ok(mut claimed) => check(&mut claimed, expected, old),
        Err(_) => Err(Refused::Changed),
    };
    if let Err(why) = checked {
        return Err(put_back(why));
    }
    // Trash it by the staging folder's real path, which must be inside the top.
    let real = match (imp::real_path(&stage), top.canonicalize()) {
        (Ok(real), Ok(top)) if real.starts_with(&top) => real,
        _ => return Err(put_back(Refused::Outside)),
    };
    if let Err(e) = crate::trash::move_to_trash(&real.join(name)) {
        return Err(put_back(Refused::Failed(e.to_string())));
    }
    let _ = imp::remove_empty_dir(&parent, &stage_name);
    Ok(())
}

#[cfg(unix)]
mod imp {
    use std::ffi::{CString, OsStr};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    pub type Dir = OwnedFd;

    fn c(name: &str) -> io::Result<CString> {
        CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
    }

    fn fd(ret: libc::c_int) -> io::Result<OwnedFd> {
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: a new descriptor we own.
            Ok(unsafe { OwnedFd::from_raw_fd(ret) })
        }
    }

    fn ok(ret: libc::c_int) -> io::Result<()> {
        if ret < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    const DIR_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

    /// The top folder (trusted as given), then each folder below it, none
    /// of which may be a link.
    pub fn open_parent(top: &Path, dirs: &[&str]) -> io::Result<Dir> {
        let top = CString::new(top.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: valid C strings and flags.
        let mut dir = fd(unsafe { libc::open(top.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) })?;
        for d in dirs {
            dir = open_dir(&dir, d)?;
        }
        Ok(dir)
    }

    pub fn open_dir(dir: &Dir, name: &str) -> io::Result<Dir> {
        let name = c(name)?;
        // SAFETY: a valid descriptor and C string.
        fd(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), DIR_FLAGS) })
    }

    pub fn make_dir(dir: &Dir, name: &str) -> io::Result<()> {
        let name = c(name)?;
        // SAFETY: as above.
        ok(unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) })
    }

    /// Removes the folder only if it's empty (never anything in it).
    pub fn remove_empty_dir(dir: &Dir, name: &str) -> io::Result<()> {
        let name = c(name)?;
        // SAFETY: as above.
        ok(unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) })
    }

    /// Rename, failing if the new name is taken (never replacing anything).
    pub fn move_noreplace(from: &Dir, a: &str, to: &Dir, b: &str) -> io::Result<()> {
        let (a, b) = (c(a)?, c(b)?);
        #[cfg(target_os = "macos")]
        // SAFETY: as above.
        return ok(unsafe { libc::renameatx_np(from.as_raw_fd(), a.as_ptr(), to.as_raw_fd(), b.as_ptr(), libc::RENAME_EXCL) });
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        // SAFETY: as above.
        return ok(unsafe { libc::renameat2(from.as_raw_fd(), a.as_ptr(), to.as_raw_fd(), b.as_ptr(), libc::RENAME_NOREPLACE) });
        #[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
        {
            // No exclusive rename here: a hard link fails if the name is
            // taken; then the old name goes (the file stays, under the new one).
            // SAFETY: as above.
            ok(unsafe { libc::linkat(from.as_raw_fd(), a.as_ptr(), to.as_raw_fd(), b.as_ptr(), 0) })?;
            ok(unsafe { libc::unlinkat(from.as_raw_fd(), a.as_ptr(), 0) })
        }
    }

    pub fn open_file_at(dir: &Dir, name: &str) -> io::Result<std::fs::File> {
        let name = c(name)?;
        // SAFETY: as above.
        let f = fd(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC) })?;
        Ok(std::fs::File::from(f))
    }

    pub fn open_file(path: &Path) -> io::Result<std::fs::File> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)
    }

    pub fn id_of(file: &std::fs::File) -> Option<(u64, u64)> {
        use std::os::unix::fs::MetadataExt;
        file.metadata().ok().map(|m| (m.dev(), m.ino()))
    }

    /// Where a folder handle really is.
    pub fn real_path(dir: &Dir) -> io::Result<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            let mut buf = vec![0u8; libc::PATH_MAX as usize];
            // SAFETY: F_GETPATH writes at most PATH_MAX bytes, NUL-terminated.
            ok(unsafe { libc::fcntl(dir.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) })?;
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            Ok(PathBuf::from(OsStr::from_bytes(&buf[..end])))
        }
        #[cfg(not(target_os = "macos"))]
        std::fs::read_link(format!("/proc/self/fd/{}", dir.as_raw_fd()))
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, GetFileInformationByHandle, MoveFileExW};

    /// A folder, by its path (checked to have no link or junction on the way).
    pub type Dir = PathBuf;

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain([0]).collect()
    }

    pub fn open_parent(top: &Path, dirs: &[&str]) -> io::Result<Dir> {
        let mut dir = top.to_path_buf();
        for d in dirs {
            dir = open_dir(&dir, d)?;
        }
        Ok(dir)
    }

    pub fn open_dir(dir: &Dir, name: &str) -> io::Result<Dir> {
        let p = dir.join(name);
        // A link or junction (a reparse point) is never entered.
        let m = std::fs::symlink_metadata(&p)?;
        use std::os::windows::fs::MetadataExt;
        const REPARSE: u32 = 0x400;
        if !m.is_dir() || m.file_attributes() & REPARSE != 0 {
            return Err(io::Error::other("not a plain folder"));
        }
        Ok(p)
    }

    pub fn make_dir(dir: &Dir, name: &str) -> io::Result<()> {
        std::fs::create_dir(dir.join(name))
    }

    pub fn remove_empty_dir(dir: &Dir, name: &str) -> io::Result<()> {
        std::fs::remove_dir(dir.join(name))
    }

    /// MoveFileExW without MOVEFILE_REPLACE_EXISTING: fails if the name is taken.
    pub fn move_noreplace(from: &Dir, a: &str, to: &Dir, b: &str) -> io::Result<()> {
        let (a, b) = (wide(&from.join(a)), wide(&to.join(b)));
        // SAFETY: NUL-terminated wide strings that outlive the call.
        if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 0) } == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    pub fn open_file_at(dir: &Dir, name: &str) -> io::Result<std::fs::File> {
        open_file(&dir.join(name))
    }

    /// Opened without following a link at the end.
    pub fn open_file(path: &Path) -> io::Result<std::fs::File> {
        std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS).open(path)
    }

    /// Volume serial number and file index.
    pub fn id_of(file: &std::fs::File) -> Option<(u64, u64)> {
        // SAFETY: plain data, filled in by the call.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a valid handle and a valid out pointer.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
            return None;
        }
        Some((info.dwVolumeSerialNumber as u64, (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64))
    }

    pub fn real_path(dir: &Dir) -> io::Result<PathBuf> {
        dir.canonicalize()
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::io;
    use std::path::{Path, PathBuf};
    pub type Dir = PathBuf;
    fn no<T>() -> io::Result<T> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
    pub fn open_parent(_: &Path, _: &[&str]) -> io::Result<Dir> {
        no()
    }
    pub fn open_dir(_: &Dir, _: &str) -> io::Result<Dir> {
        no()
    }
    pub fn make_dir(_: &Dir, _: &str) -> io::Result<()> {
        no()
    }
    pub fn remove_empty_dir(_: &Dir, _: &str) -> io::Result<()> {
        no()
    }
    pub fn move_noreplace(_: &Dir, _: &str, _: &Dir, _: &str) -> io::Result<()> {
        no()
    }
    pub fn open_file_at(_: &Dir, _: &str) -> io::Result<std::fs::File> {
        no()
    }
    pub fn open_file(_: &Path) -> io::Result<std::fs::File> {
        no()
    }
    pub fn id_of(_: &std::fs::File) -> Option<(u64, u64)> {
        None
    }
    pub fn real_path(_: &Dir) -> io::Result<PathBuf> {
        no()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-claim-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        dir
    }

    fn expect(path: &Path) -> Expected {
        let m = std::fs::symlink_metadata(path).unwrap();
        Expected { id: file_id(path), len: m.len(), mtime: m.modified().ok(), hash: fingerprint(&std::fs::read(path).unwrap()) }
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(STAGING)).collect()
    }

    #[test]
    fn a_stream_and_a_slice_fingerprint_alike() {
        let dir = temp("fp");
        let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.join("big"), &bytes).unwrap();
        let mut f = std::fs::File::open(dir.join("big")).unwrap();
        assert_eq!(fingerprint_file(&mut f), Some(fingerprint(&bytes)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A replacement swapped in after the path was checked is claimed,
    /// found not to be the listed file, and put back: never trashed.
    #[test]
    fn a_replacement_swapped_in_after_the_check_is_put_back() {
        let dir = temp("swap");
        let file = dir.join("sub/new.txt");
        std::fs::write(&file, "listed\n").unwrap();
        let listed = expect(&file);
        let r = trash_listed_with(&dir, &file, None, &listed, &mut |step| {
            if step == Step::BeforeClaim {
                std::fs::remove_file(&file).unwrap();
                std::fs::write(&file, "the agent's new work\n").unwrap();
            }
        });
        assert_eq!(r, Err(Refused::Changed));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "the agent's new work\n");
        assert!(leftovers(&dir.join("sub")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The folder above the file swapped for a link to somewhere else just
    /// before the claim: nothing outside is touched.
    #[cfg(unix)]
    #[test]
    fn a_folder_swapped_for_a_link_is_never_entered() {
        let dir = temp("dirlink");
        let outside = temp("dirlink-out");
        std::fs::write(outside.join("new.txt"), "outside\n").unwrap();
        let file = dir.join("sub/new.txt");
        std::fs::write(&file, "outside\n").unwrap();
        let listed = expect(&file);
        let r = trash_listed_with(&dir, &file, None, &listed, &mut |step| {
            if step == Step::BeforeClaim {
                std::fs::remove_dir_all(dir.join("sub")).unwrap();
                std::os::unix::fs::symlink(&outside, dir.join("sub")).unwrap();
            }
        });
        assert_eq!(r, Err(Refused::Outside));
        assert_eq!(std::fs::read_to_string(outside.join("new.txt")).unwrap(), "outside\n");
        assert!(leftovers(&outside).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// A renamed file's old name appearing while the Trash was queued blocks it.
    #[test]
    fn the_old_name_coming_back_blocks_the_trash() {
        let dir = temp("old");
        let (file, old) = (dir.join("sub/b.txt"), dir.join("sub/a.txt"));
        std::fs::write(&file, "moved\n").unwrap();
        let listed = expect(&file);
        std::fs::write(&old, "someone's new a.txt\n").unwrap();
        assert_eq!(trash_listed(&dir, &file, Some(&old), &listed), Err(Refused::OldBack));
        // Also when it appears after the claim.
        std::fs::remove_file(&old).unwrap();
        let r = trash_listed_with(&dir, &file, Some(&old), &listed, &mut |step| {
            if step == Step::Claimed {
                std::fs::write(&old, "back again\n").unwrap();
            }
        });
        assert_eq!(r, Err(Refused::OldBack));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "moved\n");
        assert!(leftovers(&dir.join("sub")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Not the listed file, and its name taken meanwhile: it stays in the
    /// staging folder, and the answer says exactly where. Nothing is lost.
    #[test]
    fn when_it_cant_go_back_it_stays_where_the_answer_says() {
        let dir = temp("kept");
        let file = dir.join("sub/new.txt");
        std::fs::write(&file, "listed\n").unwrap();
        let listed = expect(&file);
        let r = trash_listed_with(&dir, &file, None, &listed, &mut |step| match step {
            Step::BeforeClaim => {
                std::fs::remove_file(&file).unwrap();
                std::fs::write(&file, "replacement\n").unwrap();
            }
            Step::Claimed => std::fs::write(&file, "yet another\n").unwrap(),
        });
        let Err(Refused::KeptAt(at)) = r else { panic!("{r:?}") };
        assert_eq!(std::fs::read_to_string(&at).unwrap(), "replacement\n");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "yet another\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The listed file itself goes to the Trash, and no staging folder stays.
    #[test]
    fn the_listed_file_goes_to_the_trash() {
        let dir = temp("ok");
        let file = dir.join("sub/new.txt");
        std::fs::write(&file, "listed\n").unwrap();
        let listed = expect(&file);
        assert_eq!(trash_listed(&dir, &file, None, &listed), Ok(()));
        assert!(!file.exists());
        assert!(leftovers(&dir.join("sub")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
