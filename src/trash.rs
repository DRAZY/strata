//! Moving files and folders to the Trash (Recycle Bin on Windows), where
//! they can be put back, rather than deleting them outright.

use std::path::Path;

/// Move `path` (a file or a whole folder) to the Trash.
pub fn move_to_trash(path: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path).is_err() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} doesn't exist", path.display())));
    }
    imp::trash(path)
}

/// What the Trash is called here, for menus and messages.
pub fn trash_name() -> &'static str {
    if cfg!(windows) { "Recycle Bin" } else { "Trash" }
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    use std::path::Path;

    pub fn trash(path: &Path) -> std::io::Result<()> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, None).map_err(|e| std::io::Error::other(e.localizedDescription().to_string()))
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::UI::Shell::{FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW};

    pub fn trash(path: &Path) -> std::io::Result<()> {
        // The shell wants an absolute path in its ordinary spelling (it
        // refuses `\\?\C:\…`, which `canonicalize` gives), ending in two
        // NULs (it's a list).
        let path = crate::paths::plain(std::path::absolute(path)?);
        let from: Vec<u16> = path.as_os_str().encode_wide().chain([0, 0]).collect();
        let mut op = SHFILEOPSTRUCTW {
            wFunc: FO_DELETE,
            pFrom: from.as_ptr(),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
            ..Default::default()
        };
        // SAFETY: `op` is fully initialized and `from` outlives the call.
        let code = unsafe { SHFileOperationW(&mut op) };
        if code != 0 {
            return Err(std::io::Error::other(format!("the Recycle Bin refused it (error {code:#x})")));
        }
        if op.fAnyOperationsAborted != 0 {
            return Err(std::io::Error::other("cancelled"));
        }
        Ok(())
    }
}

/// The freedesktop.org Trash: ~/.local/share/Trash, with a .trashinfo
/// file saying where each item came from (so file managers can restore it).
#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use std::path::{Path, PathBuf};

    fn trash_dir() -> std::io::Result<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .ok_or_else(|| std::io::Error::other("no home folder"))?;
        Ok(data.join("Trash"))
    }

    /// `2026-10-14T09:30:00`, in UTC (the spec allows any time zone's local time).
    fn now() -> String {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
        let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
        // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
    }

    pub fn trash(path: &Path) -> std::io::Result<()> {
        let path = std::path::absolute(path)?;
        let dir = trash_dir()?;
        let (files, info) = (dir.join("files"), dir.join("info"));
        std::fs::create_dir_all(&files)?;
        std::fs::create_dir_all(&info)?;
        let name = path.file_name().ok_or_else(|| std::io::Error::other("can't trash a root folder"))?.to_string_lossy().into_owned();
        // A free name in the trash: "name", "name.2", "name.3"…
        let (target, info_file) = (1..)
            .map(|n| if n == 1 { name.clone() } else { format!("{name}.{n}") })
            .map(|n| (files.join(&n), info.join(format!("{n}.trashinfo"))))
            .find(|(t, i)| !t.exists() && !i.exists())
            .expect("a free name");
        let escaped: String = path.to_string_lossy().bytes().map(|b| if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
        std::fs::write(&info_file, format!("[Trash Info]\nPath={escaped}\nDeletionDate={}\n", now()))?;
        // Only within one file system; elsewhere, say so rather than copy and delete.
        std::fs::rename(&path, &target).inspect_err(|_| {
            let _ = std::fs::remove_file(&info_file);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_are_an_error() {
        let p = std::env::temp_dir().join(format!("strata-no-such-file-{}", std::process::id()));
        assert_eq!(move_to_trash(&p).unwrap_err().kind(), std::io::ErrorKind::NotFound);
    }

    /// Really moves a file to the Trash (run with --ignored).
    #[test]
    #[ignore]
    fn a_file_goes_to_the_trash() {
        let p = std::env::temp_dir().join(format!("strata-trash-test-{}.txt", std::process::id()));
        std::fs::write(&p, "bye").unwrap();
        move_to_trash(&p).unwrap();
        assert!(!p.exists());
    }
}
