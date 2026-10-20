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
/// The marker file in each staging folder, and how its contents start:
/// only folders with it are Strata's (a user's own `.strata-trash-x` isn't).
pub const MARKER: &str = ".strata-claim";
const MARKER_TAG: &str = "strata-claim";

/// `dir` is a staging folder Strata made (it has a valid marker).
pub fn is_staging(dir: &Path) -> bool {
    std::fs::read(dir.join(MARKER)).is_ok_and(|b| b.starts_with(MARKER_TAG.as_bytes()))
}

/// How a no-replace move went.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Moved {
    Done,
    /// Nothing moved (the new name was taken, or the old one gone).
    Not,
    /// The file now has both names: linked to the new one, but the old one
    /// couldn't be removed.
    #[cfg_attr(windows, allow(dead_code))]
    Both,
}

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
    if *name == MARKER {
        return Err(Refused::Failed(format!("Strata keeps files named {MARKER} for itself")));
    }
    if old.is_some_and(|o| std::fs::symlink_metadata(o).is_ok()) {
        return Err(Refused::OldBack);
    }
    hook(Step::BeforeClaim);
    // The folder, reached from the top without following any link (and,
    // on Windows, held so it can't be swapped until this is done).
    let parent = imp::open_parent(top, dirs).map_err(|_| Refused::Outside)?;
    let stamp = format!("{}-{}", std::process::id(), SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    let stage_name = format!("{STAGING}{stamp}");
    imp::make_dir(&parent, &stage_name).map_err(|e| Refused::Failed(e.to_string()))?;
    let stage = match imp::open_dir(&parent, &stage_name) {
        Ok(s) => s,
        Err(e) => {
            let _ = imp::remove_empty_dir(&parent, &stage_name);
            return Err(Refused::Failed(e.to_string()));
        }
    };
    // The marker says this folder is Strata's (a scan reports it if left over).
    if let Err(e) = imp::write_new(&stage, MARKER, format!("{MARKER_TAG} {stamp}\n").as_bytes()) {
        let _ = imp::remove_empty_dir(&parent, &stage_name);
        return Err(Refused::Failed(e.to_string()));
    }
    let done_with_stage = || {
        let _ = imp::remove_file_at(&stage, MARKER);
        let _ = imp::remove_empty_dir(&parent, &stage_name);
    };
    let staged_path = path.parent().unwrap_or(top).join(&stage_name).join(name);
    match imp::move_noreplace(&parent, name, &stage, name) {
        Moved::Done => {}
        Moved::Not => {
            done_with_stage();
            return Err(Refused::Changed);
        }
        // Linked into the staging folder but still at its name: the extra
        // link goes (only if it's really the same file), else say where.
        Moved::Both => {
            if imp::drop_extra_link(&stage, name, &parent, name) {
                done_with_stage();
                return Err(Refused::Failed("it couldn't be set aside".into()));
            }
            return Err(Refused::KeptAt(staged_path));
        }
    }
    hook(Step::Claimed);
    // Not the listed file: put it back, or say where it is.
    let put_back = |why: Refused| -> Refused {
        match imp::move_noreplace(&stage, name, &parent, name) {
            Moved::Done => {
                done_with_stage();
                why
            }
            Moved::Both if imp::drop_extra_link(&parent, name, &stage, name) => {
                done_with_stage();
                why
            }
            Moved::Not | Moved::Both => Refused::KeptAt(staged_path.clone()),
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
    done_with_stage();
    Ok(())
}

#[cfg(unix)]
mod imp {
    use std::ffi::{CString, OsStr};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use super::Moved;

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

    /// Rename, never replacing anything: `Not` if the new name is taken.
    pub fn move_noreplace(from: &Dir, a: &str, to: &Dir, b: &str) -> Moved {
        let (Ok(ca), Ok(cb)) = (c(a), c(b)) else { return Moved::Not };
        #[cfg(target_os = "macos")]
        // SAFETY: as above.
        return if unsafe { libc::renameatx_np(from.as_raw_fd(), ca.as_ptr(), to.as_raw_fd(), cb.as_ptr(), libc::RENAME_EXCL) } == 0 { Moved::Done } else { Moved::Not };
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            // SAFETY: as above.
            if unsafe { libc::renameat2(from.as_raw_fd(), ca.as_ptr(), to.as_raw_fd(), cb.as_ptr(), libc::RENAME_NOREPLACE) } == 0 {
                return Moved::Done;
            }
            // An old kernel or a file system without it: links instead.
            match io::Error::last_os_error().raw_os_error() {
                Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP) => move_by_link(from, a, to, b),
                _ => Moved::Not,
            }
        }
        #[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
        move_by_link(from, a, to, b)
    }

    /// A no-replace move with links: a hard link fails if the new name is
    /// taken; then the old name goes. If it can't, the file has both names.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub fn move_by_link(from: &Dir, a: &str, to: &Dir, b: &str) -> Moved {
        let (Ok(ca), Ok(cb)) = (c(a), c(b)) else { return Moved::Not };
        // SAFETY: as above.
        if unsafe { libc::linkat(from.as_raw_fd(), ca.as_ptr(), to.as_raw_fd(), cb.as_ptr(), 0) } != 0 {
            return Moved::Not;
        }
        // SAFETY: as above.
        if unsafe { libc::unlinkat(from.as_raw_fd(), ca.as_ptr(), 0) } != 0 {
            return Moved::Both;
        }
        Moved::Done
    }

    /// `dir/name` and `other/other_name` are the same file (not following links).
    pub fn same_file_at(dir: &Dir, name: &str, other: &Dir, other_name: &str) -> bool {
        let id = |d: &Dir, n: &str| -> Option<(u64, u64)> {
            let n = c(n).ok()?;
            // SAFETY: zeroed plain data, filled in by the call.
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: a valid descriptor, C string and out pointer.
            (unsafe { libc::fstatat(d.as_raw_fd(), n.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } == 0).then_some((st.st_dev as u64, st.st_ino as u64))
        };
        matches!((id(dir, name), id(other, other_name)), (Some(x), Some(y)) if x == y)
    }

    /// `extra_dir/extra` is a second name of the file at `kept_dir/kept`:
    /// remove that name only (the file stays). True if done.
    pub fn drop_extra_link(extra_dir: &Dir, extra: &str, kept_dir: &Dir, kept: &str) -> bool {
        same_file_at(extra_dir, extra, kept_dir, kept) && remove_file_at(extra_dir, extra).is_ok()
    }

    /// Remove a name (one link of a file Strata made or holds elsewhere).
    pub fn remove_file_at(dir: &Dir, name: &str) -> io::Result<()> {
        let name = c(name)?;
        // SAFETY: as above.
        ok(unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) })
    }

    /// A new file with `bytes`, failing if the name exists.
    pub fn write_new(dir: &Dir, name: &str, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let name = c(name)?;
        // SAFETY: as above.
        let f = fd(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600 as libc::c_uint) })?;
        std::fs::File::from(f).write_all(bytes)
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
    use super::Moved;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FileAttributeTagInfo, GetFileInformationByHandle, GetFileInformationByHandleEx, MoveFileExW,
    };

    /// A folder by its path, with open handles on it and every folder above
    /// it up to the top. The handles don't share delete access, and Windows
    /// won't rename, move or replace a folder while such a handle is open: so
    /// none of these folders can be swapped for a junction or link while the
    /// operation runs, and the path-based calls below stay on them.
    pub struct Dir {
        path: PathBuf,
        _held: Vec<Arc<std::fs::File>>,
    }

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain([0]).collect()
    }

    /// Open a folder without following a reparse point, holding it against
    /// renames; Err unless it's a plain folder.
    fn pin(path: &Path, check: bool) -> io::Result<std::fs::File> {
        const FILE_READ_ATTRIBUTES: u32 = 0x80;
        const SYNCHRONIZE: u32 = 0x0010_0000;
        let f = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES | SYNCHRONIZE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        if check {
            // SAFETY: plain data, filled in by the call.
            let mut tag: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
            // SAFETY: a valid handle and an out buffer of the stated size.
            let got = unsafe { GetFileInformationByHandleEx(f.as_raw_handle() as _, FileAttributeTagInfo, (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(), std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32) };
            if got == 0 {
                return Err(io::Error::last_os_error());
            }
            if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                return Err(io::Error::other("not a plain folder"));
            }
        }
        Ok(f)
    }

    /// The top folder (trusted as given, but held too), then each folder
    /// below it, none of which may be a junction or link.
    pub fn open_parent(top: &Path, dirs: &[&str]) -> io::Result<Dir> {
        let mut dir = Dir { path: top.to_path_buf(), _held: vec![Arc::new(pin(top, false)?)] };
        for d in dirs {
            dir = open_dir(&dir, d)?;
        }
        Ok(dir)
    }

    pub fn open_dir(dir: &Dir, name: &str) -> io::Result<Dir> {
        let path = dir.path.join(name);
        let mut held = dir._held.clone();
        held.push(Arc::new(pin(&path, true)?));
        Ok(Dir { path, _held: held })
    }

    pub fn make_dir(dir: &Dir, name: &str) -> io::Result<()> {
        std::fs::create_dir(dir.path.join(name))
    }

    pub fn remove_empty_dir(dir: &Dir, name: &str) -> io::Result<()> {
        std::fs::remove_dir(dir.path.join(name))
    }

    /// MoveFileExW without MOVEFILE_REPLACE_EXISTING: `Not` if the name is taken.
    pub fn move_noreplace(from: &Dir, a: &str, to: &Dir, b: &str) -> Moved {
        let (a, b) = (wide(&from.path.join(a)), wide(&to.path.join(b)));
        // SAFETY: NUL-terminated wide strings that outlive the call.
        if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 0) } == 0 { Moved::Not } else { Moved::Done }
    }

    pub fn drop_extra_link(extra_dir: &Dir, extra: &str, kept_dir: &Dir, kept: &str) -> bool {
        let id = |d: &Dir, n: &str| open_file(&d.path.join(n)).ok().and_then(|f| id_of(&f));
        matches!((id(extra_dir, extra), id(kept_dir, kept)), (Some(x), Some(y)) if x == y) && std::fs::remove_file(extra_dir.path.join(extra)).is_ok()
    }

    pub fn remove_file_at(dir: &Dir, name: &str) -> io::Result<()> {
        std::fs::remove_file(dir.path.join(name))
    }

    pub fn write_new(dir: &Dir, name: &str, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        std::fs::OpenOptions::new().write(true).create_new(true).open(dir.path.join(name))?.write_all(bytes)
    }

    pub fn open_file_at(dir: &Dir, name: &str) -> io::Result<std::fs::File> {
        open_file(&dir.path.join(name))
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
        dir.path.canonicalize()
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::Moved;
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
    pub fn move_noreplace(_: &Dir, _: &str, _: &Dir, _: &str) -> Moved {
        Moved::Not
    }
    pub fn drop_extra_link(_: &Dir, _: &str, _: &Dir, _: &str) -> bool {
        false
    }
    pub fn remove_file_at(_: &Dir, _: &str) -> io::Result<()> {
        no()
    }
    pub fn write_new(_: &Dir, _: &str, _: &[u8]) -> io::Result<()> {
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

    /// The link-based move (old Linux kernels, other systems) never replaces
    /// a name, and when the old name can't be removed the file keeps both:
    /// the extra name goes only after checking it's the same file.
    #[cfg(unix)]
    #[test]
    fn moving_by_links_never_replaces_and_never_loses() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("links");
        std::fs::create_dir_all(dir.join("to")).unwrap();
        std::fs::write(dir.join("sub/a.txt"), "a\n").unwrap();
        std::fs::write(dir.join("to/a.txt"), "taken\n").unwrap();
        let (from, to) = (imp::open_parent(&dir, &["sub"]).unwrap(), imp::open_parent(&dir, &["to"]).unwrap());
        assert_eq!(imp::move_by_link(&from, "a.txt", &to, "a.txt"), Moved::Not);
        assert_eq!(std::fs::read_to_string(dir.join("to/a.txt")).unwrap(), "taken\n");
        assert_eq!(imp::move_by_link(&from, "a.txt", &to, "b.txt"), Moved::Done);
        assert!(!dir.join("sub/a.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.join("to/b.txt")).unwrap(), "a\n");
        // The old name can't go (its folder is read-only): both names stay.
        std::fs::write(dir.join("sub/c.txt"), "c\n").unwrap();
        std::fs::set_permissions(dir.join("sub"), std::fs::Permissions::from_mode(0o555)).unwrap();
        let moved = imp::move_by_link(&from, "c.txt", &to, "c.txt");
        std::fs::set_permissions(dir.join("sub"), std::fs::Permissions::from_mode(0o755)).unwrap();
        // (Root may remove it anyway.)
        // SAFETY: no arguments, no side effects.
        assert!(moved == Moved::Both || unsafe { libc::geteuid() } == 0, "{moved:?}");
        if moved == Moved::Both {
            assert!(dir.join("sub/c.txt").exists() && dir.join("to/c.txt").exists());
            // A different file under the extra name is never removed.
            assert!(!imp::drop_extra_link(&to, "b.txt", &from, "c.txt"));
            assert!(imp::drop_extra_link(&to, "c.txt", &from, "c.txt"));
            assert!(!dir.join("to/c.txt").exists());
            assert_eq!(std::fs::read_to_string(dir.join("sub/c.txt")).unwrap(), "c\n");
        }
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
