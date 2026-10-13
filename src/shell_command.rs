//! Install Shell Command: put a `strata` symlink on the PATH.

#[cfg(unix)]
use std::path::{Path, PathBuf};

/// Symlink `/usr/local/bin/strata` (or `~/.local/bin/strata` when that isn't
/// writable) to this executable. Returns a message for the status bar.
#[cfg(unix)]
pub fn install() -> Result<String, String> {
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).map_err(|e| format!("Install Shell Command: can't find Strata's executable: {e}"))?;
    let system = PathBuf::from("/usr/local/bin");
    let home = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin"));
    let mut tried = Vec::new();
    for dir in std::iter::once(system).chain(home) {
        match link(&exe, &dir) {
            Ok(path) => {
                let on_path = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == dir));
                let hint = if on_path { String::new() } else { format!(" — add {} to your PATH", dir.display()) };
                return Ok(format!("Installed {} → {}{hint}", path.display(), exe.display()));
            }
            Err(e) => tried.push(format!("{}: {e}", dir.display())),
        }
    }
    Err(format!("Install Shell Command failed ({}). Run: sudo ln -sf '{}' /usr/local/bin/strata", tried.join("; "), exe.display()))
}

#[cfg(windows)]
pub fn install() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().map(std::path::Path::display).map(|d| d.to_string()).unwrap_or_default();
    Err(format!("On Windows, add {dir} to your PATH to run strata from a terminal"))
}

/// Create or replace `dir/strata` as a link to `exe`. Never replaces a real file.
#[cfg(unix)]
pub fn link(exe: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("strata");
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::remove_file(&path)?,
        Ok(_) => return Err(std::io::Error::other(format!("{} exists and isn't a link; remove it first", path.display()))),
        Err(_) => {}
    }
    std::os::unix::fs::symlink(exe, &path)?;
    Ok(path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn links_replace_old_links_but_not_files() {
        let d = std::env::temp_dir().join(format!("strata-shellcmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let bin = d.join("bin");
        let path = link(Path::new("/old/strata"), &bin).unwrap();
        let path2 = link(Path::new("/new/strata"), &bin).unwrap();
        assert_eq!(path, path2);
        assert_eq!(std::fs::read_link(&path).unwrap(), PathBuf::from("/new/strata"));
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "mine").unwrap();
        assert!(link(Path::new("/new/strata"), &bin).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
        std::fs::remove_dir_all(d).ok();
    }
}
