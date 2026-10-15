//! Putting a downloaded, verified update in place of the running app.
//!
//! macOS: the download is a zip of Strata.app (signed with Developer ID and
//! notarized, the ticket stapled). It's unpacked with `ditto` into a hidden
//! folder next to the app (same volume, so the swap is two renames), its
//! code signature is checked (valid, Developer ID, our team, accepted by
//! Gatekeeper when this copy is), as are its bundle id and version, and it
//! must answer `--version` with the right one. Then the running app's bundle
//! is renamed to a hidden backup and the new one renamed into its place.
//!
//! Windows: the download is the portable zip. Strata.exe is unpacked with
//! the system's `tar` next to the running exe, must answer `--version`, and
//! takes its place: Windows lets a running exe be renamed (not deleted), so
//! it becomes `Strata.exe.old` until the new version has started.
//!
//! Either way the old app stays as the backup until the new version shows
//! its first window; if it doesn't start, the relaunch helper puts the
//! backup back (see `helper`).

use super::Version;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Folders the updater unpacks into start with this (inside the app's folder).
pub const STAGING_PREFIX: &str = ".strata-update-";
/// Strata's bundle id (scripts/package-macos.sh).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const BUNDLE_ID: &str = "io.github.drazy.strata";
/// The Apple Developer team that signs Strata.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const TEAM_ID: &str = "W966GFT83K";

/// The installed app to replace: the .app bundle on macOS, the exe on Windows.
pub fn locate() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).map_err(|e| format!("couldn't find Strata's own program: {e}"))?;
    target_for(&exe)
}

pub fn target_for(exe: &Path) -> Result<PathBuf, String> {
    if cfg!(target_os = "macos") {
        let bundle = exe.parent().filter(|p| p.ends_with("Contents/MacOS")).and_then(Path::parent).and_then(Path::parent);
        match bundle {
            Some(b) if b.extension().is_some_and(|e| e == "app") => Ok(b.to_path_buf()),
            _ => Err("Strata isn't running from Strata.app, so it can't update itself".into()),
        }
    } else if cfg!(windows) {
        Ok(exe.to_path_buf())
    } else {
        Err("Strata can't update itself on this system yet".into())
    }
}

/// Can the update go here? The folder holding the app must take new files,
/// and macOS mustn't be running the app from a read-only copy.
pub fn check_writable(target: &Path) -> Result<(), String> {
    if target.components().any(|c| c.as_os_str() == "AppTranslocation") {
        return Err("macOS is running Strata from a temporary read-only copy. Move Strata to your Applications folder, open it from there, and try again".into());
    }
    let parent = target.parent().ok_or("the app has no folder")?;
    let probe = parent.join(format!("{STAGING_PREFIX}probe-{}", std::process::id()));
    match std::fs::create_dir(&probe) {
        Ok(()) => {
            let _ = std::fs::remove_dir(&probe);
            Ok(())
        }
        Err(e) => Err(format!("Strata can't write to {} ({e}), where it's installed", parent.display())),
    }
}

/// A new, empty folder next to `target` for this update.
pub fn staging_dir(target: &Path) -> Result<PathBuf, String> {
    let parent = target.parent().ok_or("the app has no folder")?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let dir = parent.join(format!("{STAGING_PREFIX}{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Remove updater leftovers next to `target` (an update interrupted by a
/// crash or power cut), except `keep`.
pub fn sweep(target: &Path, keep: Option<&Path>) {
    let Some(parent) = target.parent() else { return };
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if e.file_name().to_string_lossy().starts_with(STAGING_PREFIX) && Some(p.as_path()) != keep {
            remove(&p);
        }
    }
}

/// Where the old app waits while the new one starts.
pub fn backup_path(target: &Path) -> PathBuf {
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Hidden on macOS (a visible "Strata.app.old" would show in Applications).
    let name = if cfg!(target_os = "macos") { format!(".{name}.old") } else { format!("{name}.old") };
    target.with_file_name(name)
}

/// Delete a file or a folder and everything in it.
pub fn remove(path: &Path) -> bool {
    let r = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(_) => return true,
    };
    r.is_ok()
}

/// Put `staged` where `target` is, keeping `target` as `backup`. On failure
/// everything is as it was.
pub fn swap(target: &Path, staged: &Path, backup: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(backup).is_ok() && !remove(backup) {
        return Err(format!("couldn't remove the old backup {}", backup.display()));
    }
    std::fs::rename(target, backup).map_err(|e| format!("couldn't move the current version aside: {e}"))?;
    if let Err(e) = std::fs::rename(staged, target) {
        return match std::fs::rename(backup, target) {
            Ok(()) => Err(format!("couldn't move the new version into place: {e}")),
            Err(e2) => Err(format!("couldn't move the new version into place ({e}), nor the old one back ({e2}); it is at {}", backup.display())),
        };
    }
    Ok(())
}

/// Put the backup back (the new version didn't start).
pub fn restore(target: &Path, backup: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(backup).is_err() {
        return Err(format!("the backup {} is gone", backup.display()));
    }
    let failed = target.with_file_name(format!("{STAGING_PREFIX}failed-{}", std::process::id()));
    if std::fs::symlink_metadata(target).is_ok() {
        std::fs::rename(target, &failed).map_err(|e| format!("couldn't move the new version aside: {e}"))?;
    }
    std::fs::rename(backup, target).map_err(|e| format!("couldn't put the previous version back: {e}"))?;
    remove(&failed);
    Ok(())
}

/// Unpack the downloaded zip into `dir`; returns the app (macOS) or exe
/// (Windows) inside it.
pub fn unpack(zip: &Path, dir: &Path) -> Result<PathBuf, String> {
    let out = dir.join("unpacked");
    std::fs::create_dir(&out).map_err(|e| e.to_string())?;
    let mut cmd = if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let mut c = Command::new(root.join("System32").join("tar.exe"));
        c.arg("-xf").arg(zip).arg("-C").arg(&out);
        c
    } else {
        let mut c = Command::new("/usr/bin/ditto");
        c.arg("-x").arg("-k").arg(zip).arg(&out);
        c
    };
    run_quiet(&mut cmd, Duration::from_secs(300)).map_err(|e| format!("couldn't unpack the update: {e}"))?;
    find_payload(&out)
}

/// The one Strata.app (macOS) or Strata.exe (Windows) in an unpacked zip,
/// at the top or one folder down.
pub fn find_payload(out: &Path) -> Result<PathBuf, String> {
    let wanted = |p: &Path| {
        if cfg!(windows) {
            p.is_file() && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("strata.exe"))
        } else {
            p.is_dir() && p.extension().is_some_and(|e| e == "app")
        }
    };
    let mut found = Vec::new();
    for e in std::fs::read_dir(out).map_err(|e| e.to_string())?.flatten() {
        let p = e.path();
        if wanted(&p) {
            found.push(p);
        } else if p.is_dir() && !e.file_name().to_string_lossy().starts_with("__MACOSX") {
            for e in std::fs::read_dir(&p).map_err(|e| e.to_string())?.flatten() {
                if wanted(&e.path()) {
                    found.push(e.path());
                }
            }
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err("the update doesn't contain Strata".into()),
        _ => Err("the update contains more than one Strata".into()),
    }
}

/// The program inside a staged app (macOS) or the staged exe itself.
pub fn program(app: &Path) -> PathBuf {
    if cfg!(target_os = "macos") { app.join("Contents/MacOS/strata") } else { app.to_path_buf() }
}

/// Check the unpacked app before it replaces this one.
pub fn verify_staged(app: &Path, version: &Version, gatekeeper: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    verify_mac(app, version, gatekeeper)?;
    #[cfg(not(target_os = "macos"))]
    let _ = gatekeeper;
    // It runs, on this machine, and is the version the feed promised.
    let mut cmd = Command::new(program(app));
    cmd.arg("--version");
    let out = run_quiet(&mut cmd, Duration::from_secs(30)).map_err(|e| format!("the new version doesn't run here: {e}"))?;
    let said = out.trim().strip_prefix("strata ").and_then(Version::parse);
    if said.as_ref() != Some(version) {
        return Err(format!("the download says it's {:?}, not {version}", out.trim()));
    }
    Ok(())
}

/// Signed by Strata's team with Developer ID, intact, and (when this copy is)
/// accepted by Gatekeeper; Strata's bundle id and the expected version.
#[cfg(target_os = "macos")]
fn verify_mac(app: &Path, version: &Version, gatekeeper: bool) -> Result<(), String> {
    let requirement = format!("=anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"{TEAM_ID}\"");
    run_quiet(Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict", "-R", &requirement]).arg(app), Duration::from_secs(120)).map_err(|e| format!("the new version's code signature isn't valid: {e}"))?;
    let info = run_quiet(Command::new("/usr/bin/codesign").args(["-d", "--verbose=2"]).arg(app), Duration::from_secs(60)).map_err(|e| format!("couldn't read the code signature: {e}"))?;
    if !info.lines().any(|l| l.trim() == format!("TeamIdentifier={TEAM_ID}")) {
        return Err("the new version isn't signed by Strata's developer".into());
    }
    if gatekeeper {
        run_quiet(Command::new("/usr/sbin/spctl").args(["--assess", "--type", "execute"]).arg(app), Duration::from_secs(120)).map_err(|e| format!("Gatekeeper doesn't accept the new version: {e}"))?;
    }
    let plist = app.join("Contents/Info.plist");
    let key = |k: &str| run_quiet(Command::new("/usr/bin/plutil").args(["-extract", k, "raw", "-o", "-"]).arg(&plist), Duration::from_secs(10)).map(|s| s.trim().to_owned());
    if key("CFBundleIdentifier").as_deref() != Ok(BUNDLE_ID) {
        return Err("the download isn't Strata".into());
    }
    if key("CFBundleShortVersionString").ok().and_then(|v| Version::parse(&v)).as_ref() != Some(version) {
        return Err(format!("the download isn't Strata {version}"));
    }
    Ok(())
}

/// Is this copy of Strata accepted by Gatekeeper (Developer ID and
/// notarized)? Then the new one must be too.
pub fn self_passes_gatekeeper(target: &Path) -> bool {
    cfg!(target_os = "macos") && run_quiet(Command::new("/usr/sbin/spctl").args(["--assess", "--type", "execute"]).arg(target), Duration::from_secs(60)).is_ok()
}

/// Run a short command; its stdout and stderr together if it succeeded,
/// otherwise an error with what it said. Killed after `timeout`.
pub fn run_quiet(cmd: &mut Command, timeout: Duration) -> Result<String, String> {
    super::fetch::no_window(cmd);
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let (mut out, mut err) = (child.stdout.take(), child.stderr.take());
    let read_out = std::thread::spawn(move || read_some(out.as_mut()));
    let read_err = std::thread::spawn(move || read_some(err.as_mut()));
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(s) => break s,
            None if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("it took too long".into());
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let text = read_out.join().unwrap_or_default() + &read_err.join().unwrap_or_default();
    if status.success() { Ok(text) } else { Err(text.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("it failed").to_owned()) }
}

fn read_some(r: Option<&mut impl std::io::Read>) -> String {
    let mut s = String::new();
    if let Some(r) = r {
        let _ = std::io::Read::read_to_string(&mut std::io::Read::take(&mut *r, 64 * 1024), &mut s);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("strata-install-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A pretend app: a folder with a version file.
    fn app(at: &Path, version: &str) {
        std::fs::create_dir_all(at.join("Contents")).unwrap();
        std::fs::write(at.join("Contents/version"), version).unwrap();
    }

    fn version_of(at: &Path) -> String {
        std::fs::read_to_string(at.join("Contents/version")).unwrap()
    }

    #[test]
    fn swap_keeps_a_backup_and_restore_puts_it_back() {
        let dir = temp("swap");
        let target = dir.join("Strata.app");
        app(&target, "1.0.0");
        let staging = staging_dir(&target).unwrap();
        assert!(staging.file_name().unwrap().to_string_lossy().starts_with(STAGING_PREFIX));
        let staged = staging.join("Strata.app");
        app(&staged, "1.1.0");
        let backup = backup_path(&target);
        assert_eq!(backup.parent(), target.parent(), "same folder, so a rename");
        // A stale backup from before is replaced.
        app(&backup, "0.9.0");
        swap(&target, &staged, &backup).unwrap();
        assert_eq!(version_of(&target), "1.1.0");
        assert_eq!(version_of(&backup), "1.0.0");
        assert!(!staged.exists());
        // The new version didn't start: back to the old one.
        restore(&target, &backup).unwrap();
        assert_eq!(version_of(&target), "1.0.0");
        assert!(!backup.exists());
        assert!(restore(&target, &backup).is_err(), "no backup any more");
        sweep(&target, None);
        assert!(!staging.exists(), "leftovers are swept");
        assert!(target.exists(), "the app isn't");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_failed_swap_changes_nothing() {
        let dir = temp("swapfail");
        let target = dir.join("Strata.app");
        app(&target, "1.0.0");
        let backup = backup_path(&target);
        // Nothing staged: moving it in fails, and the old app comes back.
        let err = swap(&target, &dir.join("missing.app"), &backup).unwrap_err();
        assert!(err.contains("into place"), "{err}");
        assert_eq!(version_of(&target), "1.0.0");
        assert!(!backup.exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn finds_the_app_in_an_unpacked_zip() {
        let dir = temp("payload");
        let name = if cfg!(windows) { "Strata.exe" } else { "Strata.app" };
        let make = |p: &Path| if cfg!(windows) { std::fs::write(p, "MZ").unwrap() } else { std::fs::create_dir_all(p).unwrap() };
        assert!(find_payload(&dir).is_err(), "empty");
        std::fs::create_dir_all(dir.join("__MACOSX")).unwrap();
        // One level down (the Windows zip has a folder) is found.
        let sub = dir.join("Strata-1.1.0-windows-x64");
        std::fs::create_dir_all(&sub).unwrap();
        make(&sub.join(name));
        assert_eq!(find_payload(&dir).unwrap(), sub.join(name));
        make(&dir.join(name));
        assert!(find_payload(&dir).is_err(), "two of them");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_app_is_the_bundle_or_the_exe() {
        if cfg!(target_os = "macos") {
            assert_eq!(target_for(Path::new("/Applications/Strata.app/Contents/MacOS/strata")).unwrap(), Path::new("/Applications/Strata.app"));
            assert!(target_for(Path::new("/Users/me/strata/target/release/strata")).is_err(), "cargo run can't update itself");
        } else if cfg!(windows) {
            assert_eq!(target_for(Path::new(r"C:\Tools\Strata\Strata.exe")).unwrap(), Path::new(r"C:\Tools\Strata\Strata.exe"));
        }
        assert!(check_writable(Path::new("/private/var/folders/x/AppTranslocation/ABC/d/Strata.app")).unwrap_err().contains("Applications"));
        let dir = temp("writable");
        assert!(check_writable(&dir.join("Strata.app")).is_ok());
        assert!(check_writable(&dir.join("missing-folder/Strata.app")).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_folder_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("readonly");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = check_writable(&dir.join("Strata.app"));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        // (root can write anywhere; CI doesn't run tests as root.)
        assert!(r.is_err() || unsafe { libc::geteuid() } == 0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn run_quiet_reports_failures_and_timeouts() {
        if cfg!(windows) {
            return;
        }
        assert_eq!(run_quiet(Command::new("/bin/sh").args(["-c", "echo hi"]), Duration::from_secs(5)).unwrap().trim(), "hi");
        assert_eq!(run_quiet(Command::new("/bin/sh").args(["-c", "echo nope >&2; exit 3"]), Duration::from_secs(5)).unwrap_err(), "nope");
        assert!(run_quiet(Command::new("/bin/sh").args(["-c", "sleep 5"]), Duration::from_millis(200)).unwrap_err().contains("too long"));
    }

    /// The real checks on a real app: this test build isn't one, so a
    /// freshly made ad-hoc-signed bundle must be refused.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_app_not_signed_by_the_team_is_refused() {
        let dir = temp("codesign");
        let app = dir.join("Strata.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::copy("/usr/bin/true", app.join("Contents/MacOS/strata")).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), format!(r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string><key>CFBundleExecutable</key><string>strata</string><key>CFBundleShortVersionString</key><string>9.9.9</string></dict></plist>"#)).unwrap();
        let _ = Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(&app).output();
        let err = verify_staged(&app, &Version::parse("9.9.9").unwrap(), false).unwrap_err();
        assert!(err.contains("signature") || err.contains("signed"), "{err}");
        std::fs::remove_dir_all(dir).ok();
    }
}
