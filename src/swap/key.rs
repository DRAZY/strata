//! This installation's key for sealing undo journals (see `swap`).
//!
//! The key is only worth anything if nobody else knows it, so a key file is
//! used only when it's a plain file (never through a link), exactly 32
//! bytes, and private: on Unix owned by this user with no group or other
//! permissions, on Windows made with a DACL for its owner only. A new key
//! is written whole to a temporary file, flushed, and linked into place
//! without replacing anything, so Strata starting several times at once
//! settles on one key and never reads half of one. A key that can't be
//! stored and read back the same isn't used: journaled undo is refused
//! instead (a journal sealed with a key that's gone can't be checked later).

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// The key's file name in the config folder.
pub const FILE: &str = "claim.key";

/// The key in `dir`, made there first if there's none. None: there's no
/// key that can be trusted (see the module notes).
pub fn load_from(dir: &Path) -> Option<[u8; 32]> {
    let path = dir.join(FILE);
    match read(&path) {
        Ok(k) => return Some(k),
        // Something is there but isn't a key we can trust: never replaced.
        Err(e) if e.kind() != io::ErrorKind::NotFound => return None,
        Err(_) => {}
    }
    crate::private::create_dir_all(dir).ok()?;
    let k = super::random_bytes();
    let temp = dir.join(format!(
        "{FILE}.{}-{}.new",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
    ));
    let made = write_new(&temp, &k);
    let installed = made.and_then(|()| install(&temp, &path));
    let _ = std::fs::remove_file(&temp);
    sync_dir(dir);
    match installed {
        // Ours, or one another Strata installed meanwhile: whichever is there now.
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    read(&path).ok()
}

/// A key file's 32 bytes, if it can be trusted.
fn read(path: &Path) -> io::Result<[u8; 32]> {
    let mut f = open_plain(path)?;
    let meta = f.metadata()?;
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "not a private 32-byte key file");
    if !meta.is_file() || meta.len() != 32 || !private(&meta) {
        return Err(bad());
    }
    let mut k = [0u8; 32];
    f.read_exact(&mut k)?;
    // Nothing more (it didn't grow after the check).
    if f.read(&mut [0u8; 1])? != 0 {
        return Err(bad());
    }
    Ok(k)
}

#[cfg(unix)]
fn open_plain(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    // O_NONBLOCK: a FIFO planted at the name can't hang startup.
    std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)
}

#[cfg(unix)]
fn private(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid has no preconditions.
    meta.uid() == unsafe { libc::getuid() } && meta.mode() & 0o077 == 0
}

#[cfg(unix)]
fn write_new(path: &Path, k: &[u8; 32]) -> io::Result<()> {
    use std::io::Write;
    let mut f = crate::private::options().write(true).create_new(true).open(path)?;
    f.write_all(k)?;
    f.sync_all()
}

/// Link the finished temporary file in at `to`; AlreadyExists if it's taken.
#[cfg(unix)]
fn install(temp: &Path, to: &Path) -> io::Result<()> {
    std::fs::hard_link(temp, to)
}

#[cfg(unix)]
fn sync_dir(dir: &Path) {
    let _ = File::open(dir).and_then(|d| d.sync_all());
}

#[cfg(windows)]
fn open_plain(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT};
    let f = std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
    if f.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a link"));
    }
    Ok(f)
}

/// Made with an owner-only DACL (see `write_new`); per-user `%APPDATA%`
/// keeps other accounts out of the folder as well.
#[cfg(windows)]
fn private(_: &std::fs::Metadata) -> bool {
    true
}

/// A new file whose DACL lets only its owner in (SDDL `D:P(A;;FA;;;OW)`:
/// protected, nothing inherited, full access for the owner).
#[cfg(windows)]
fn write_new(path: &Path, k: &[u8; 32]) -> io::Result<()> {
    use std::io::Write;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL};
    let sddl: Vec<u16> = "D:P(A;;FA;;;OW)".encode_utf16().chain([0]).collect();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: a NUL-terminated string and an out pointer; freed below.
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: valid string and attributes; CREATE_NEW never opens an existing file.
    let h = unsafe { CreateFileW(wide.as_ptr(), GENERIC_WRITE, 0, &sa, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, std::ptr::null_mut()) };
    let err = io::Error::last_os_error();
    // SAFETY: allocated by the conversion above.
    unsafe { LocalFree(sd) };
    if h == INVALID_HANDLE_VALUE {
        return Err(err);
    }
    // SAFETY: a handle we own.
    let mut f = unsafe { File::from_raw_handle(h) };
    f.write_all(k)?;
    f.sync_all()
}

#[cfg(windows)]
fn install(temp: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let w = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain([0]).collect() };
    let (a, b) = (w(temp), w(to));
    // SAFETY: NUL-terminated strings. Without MOVEFILE_REPLACE_EXISTING it never replaces.
    if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        let e = io::Error::last_os_error();
        // ERROR_ALREADY_EXISTS / ERROR_FILE_EXISTS
        return Err(if matches!(e.raw_os_error(), Some(183 | 80)) { io::Error::from(io::ErrorKind::AlreadyExists) } else { e });
    }
    Ok(())
}

#[cfg(windows)]
fn sync_dir(_: &Path) {
    // MOVEFILE_WRITE_THROUGH flushed the move.
}

#[cfg(not(any(unix, windows)))]
fn open_plain(_: &Path) -> io::Result<File> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}
#[cfg(not(any(unix, windows)))]
fn private(_: &std::fs::Metadata) -> bool {
    false
}
#[cfg(not(any(unix, windows)))]
fn write_new(_: &Path, _: &[u8; 32]) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}
#[cfg(not(any(unix, windows)))]
fn install(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}
#[cfg(not(any(unix, windows)))]
fn sync_dir(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("strata-key-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_new_key_is_made_once_and_read_back() {
        let d = temp("new");
        let k = load_from(&d).expect("made");
        assert_eq!(fs::read(d.join(FILE)).unwrap(), k);
        assert_eq!(load_from(&d), Some(k), "the same key next time");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(d.join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let left: Vec<_> = fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "no temporary files left: {left:?}");
        fs::remove_dir_all(d).ok();
    }

    /// Strata starting several times at once settles on one key.
    #[test]
    fn instances_starting_together_share_one_key() {
        for round in 0..20 {
            let d = temp(&format!("race{round}"));
            let keys: Vec<Option<[u8; 32]>> = std::thread::scope(|s| {
                let hs: Vec<_> = (0..8).map(|_| s.spawn(|| load_from(&d))).collect();
                hs.into_iter().map(|h| h.join().unwrap()).collect()
            });
            let on_disk = fs::read(d.join(FILE)).unwrap();
            for k in keys {
                assert_eq!(k.expect("a key").as_slice(), on_disk.as_slice(), "round {round}");
            }
            fs::remove_dir_all(d).ok();
        }
    }

    /// A key someone else could know or have planted is never used.
    #[cfg(unix)]
    #[test]
    fn a_key_file_that_isnt_private_and_plain_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let d = temp("bad");
        let known = d.join("known");
        fs::write(&known, [7u8; 32]).unwrap();
        fs::set_permissions(&known, fs::Permissions::from_mode(0o600)).unwrap();
        // A link to a key the attacker knows.
        std::os::unix::fs::symlink(&known, d.join(FILE)).unwrap();
        assert_eq!(load_from(&d), None, "never through a link");
        assert_eq!(fs::read(&known).unwrap(), [7u8; 32]);
        fs::remove_file(d.join(FILE)).unwrap();
        // Readable by others.
        fs::write(d.join(FILE), [7u8; 32]).unwrap();
        fs::set_permissions(d.join(FILE), fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load_from(&d), None, "never one others can read");
        // The wrong size.
        fs::write(d.join(FILE), [7u8; 31]).unwrap();
        fs::set_permissions(d.join(FILE), fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(load_from(&d), None, "never one of the wrong size");
        // A folder.
        fs::remove_file(d.join(FILE)).unwrap();
        fs::create_dir(d.join(FILE)).unwrap();
        assert_eq!(load_from(&d), None);
        fs::remove_dir_all(d).ok();
    }

    /// A key that can't be stored isn't used (journals sealed with it
    /// couldn't be checked after a restart anyway).
    #[cfg(unix)]
    #[test]
    fn a_key_that_cant_be_stored_isnt_used() {
        use std::os::unix::fs::PermissionsExt;
        let d = temp("ro");
        fs::set_permissions(&d, fs::Permissions::from_mode(0o500)).unwrap();
        let k = load_from(&d);
        fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(k, None);
        fs::remove_dir_all(d).ok();
    }
}
