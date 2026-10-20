//! Installed plugin packages: copying a folder or a zip into Strata's config
//! folder (`Plugins/<name>`), the package's identity (a SHA-256 over every
//! file's path, mode and bytes, which consent is tied to), and removal.
//!
//! An install is staged next to the installed copies and swapped in by
//! renames, so a failed or interrupted install leaves the old version (or
//! nothing) rather than half of one. Links are refused everywhere: a package
//! can't point outside itself.

use super::manifest::{self, Manifest};
use sha2::Digest;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The folder of installed plugins, in the config folder.
pub const DIR: &str = "Plugins";
/// Plugins' own data (`storagePath`), kept when a plugin is updated.
pub const DATA_DIR: &str = "PluginData";
pub const MAX_FILES: usize = 1000;
pub const MAX_FILE_BYTES: u64 = 16 << 20;
pub const MAX_TOTAL_BYTES: u64 = 64 << 20;
pub const MAX_DEPTH: usize = 16;
/// Left out when copying a folder: a checkout's history and Finder's notes.
const SKIPPED: &[&str] = &[".git", ".DS_Store"];

/// The parts of a relative path that stays inside its folder: no root, no
/// drive, no `.` or `..`, no empty parts. `/` and `\` both separate.
pub fn relative_parts(path: &str) -> Option<Vec<String>> {
    if path.is_empty() || path.len() > 1024 {
        return None;
    }
    let parts: Vec<&str> = path.split(['/', '\\']).collect();
    if parts.iter().any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains(':') || p.contains('\0')) {
        return None;
    }
    Some(parts.into_iter().map(str::to_owned).collect())
}

/// The parts of a manifest's `run.command` path, which may start with `./`.
pub fn command_parts(command: &str) -> Option<Vec<String>> {
    relative_parts(command.strip_prefix("./").or_else(|| command.strip_prefix(".\\")).unwrap_or(command))
}

/// One file of a package, in memory.
pub struct File {
    pub parts: Vec<String>,
    pub data: Vec<u8>,
    pub executable: bool,
}

/// Where installed copies live.
pub fn plugins_dir(config: &Path) -> PathBuf {
    config.join(DIR)
}

/// The SHA-256 of a package folder as hex: every folder and file, in
/// sorted order, with its path, whether it's executable, its length and its
/// bytes. Any change to the package (a file added, removed, renamed, edited
/// or made executable) changes it. Links and odd files make it an error.
#[cfg(test)]
pub fn hash_dir(dir: &Path) -> Result<String, String> {
    hash_and_manifest(dir).map(|(sha, _)| sha)
}

/// An installed package's identity and its manifest, read from the very
/// bytes that were hashed (so the permissions asked for are the ones the
/// identity covers, even if the folder changes meanwhile).
pub fn identity(dir: &Path) -> Result<(String, Manifest), String> {
    let (sha, bytes) = hash_and_manifest(dir)?;
    let bytes = bytes.ok_or_else(|| format!("there's no {} in it", manifest::FILE))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{} isn't UTF-8 text", manifest::FILE))?;
    Ok((sha, manifest::parse(&text)?))
}

fn hash_and_manifest(dir: &Path) -> Result<(String, Option<Vec<u8>>), String> {
    let mut h = sha2::Sha256::new();
    let mut manifest_bytes = None;
    let mut entries = Vec::new();
    walk(dir, &mut Vec::new(), &mut entries, false)?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut total = 0u64;
    let mut buf = vec![0u8; 64 << 10];
    for (rel, kind) in entries {
        let path = rel.iter().fold(dir.to_path_buf(), |p, s| p.join(s));
        let joined = rel.join("/");
        match kind {
            Kind::Dir => {
                h.update(b"D");
                h.update(joined.as_bytes());
                h.update([0]);
            }
            Kind::File { executable } => {
                // Reached again from the top without following a link swapped in since the walk.
                let opened = crate::claim::open_inside(dir, &path).map_err(|_| format!("couldn't read {joined}: it changed, or a link was put in its way"))?;
                let mut f = opened.file;
                let len = f.metadata().map_err(|e| format!("couldn't read {joined}: {e}"))?.len();
                let is_manifest = rel.len() == 1 && rel[0] == manifest::FILE && len <= manifest::MAX_BYTES as u64;
                let mut kept = Vec::new();
                total += len;
                if len > MAX_FILE_BYTES || total > MAX_TOTAL_BYTES {
                    return Err(format!("the package is larger than {} MB", MAX_TOTAL_BYTES >> 20));
                }
                h.update(b"F");
                h.update(joined.as_bytes());
                h.update([0, u8::from(executable)]);
                h.update(len.to_le_bytes());
                let mut read = 0u64;
                loop {
                    let n = f.read(&mut buf).map_err(|e| format!("couldn't read {joined}: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    read += n as u64;
                    h.update(&buf[..n]);
                    if is_manifest {
                        kept.extend_from_slice(&buf[..n]);
                    }
                }
                if read != len {
                    return Err(format!("{joined} changed while it was read"));
                }
                if is_manifest {
                    manifest_bytes = Some(kept);
                }
            }
        }
    }
    Ok((h.finalize().iter().map(|b| format!("{b:02x}")).collect(), manifest_bytes))
}

enum Kind {
    Dir,
    File { executable: bool },
}

/// Every folder and file below `dir` (relative parts). `skip` leaves out
/// `SKIPPED` names (copying a source folder); the installed copy is hashed whole.
fn walk(dir: &Path, rel: &mut Vec<String>, out: &mut Vec<(Vec<String>, Kind)>, skip: bool) -> Result<(), String> {
    if rel.len() > MAX_DEPTH {
        return Err(format!("{} is nested too deeply", rel.join("/")));
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("couldn't list {}: {e}", dir.display()))?;
    for e in entries {
        let e = e.map_err(|e| format!("couldn't list {}: {e}", dir.display()))?;
        let name = e.file_name().into_string().map_err(|n| format!("\"{}\" isn't a valid UTF-8 name", n.to_string_lossy()))?;
        if skip && SKIPPED.contains(&name.as_str()) {
            continue;
        }
        rel.push(name);
        let shown = rel.join("/");
        if !super::zip::portable_name(rel.last().expect("pushed")) {
            return Err(format!("\"{shown}\" has a name that can't be used on every system"));
        }
        let meta = std::fs::symlink_metadata(e.path()).map_err(|e| format!("couldn't read {shown}: {e}"))?;
        let t = meta.file_type();
        if t.is_symlink() || is_reparse_point(&meta) {
            return Err(format!("\"{shown}\" is a link; plugin packages can't contain links"));
        }
        if t.is_dir() {
            out.push((rel.clone(), Kind::Dir));
            walk(&e.path(), rel, out, skip)?;
        } else if t.is_file() {
            out.push((rel.clone(), Kind::File { executable: executable(&meta) }));
        } else {
            return Err(format!("\"{shown}\" isn't a plain file or folder"));
        }
        if out.len() > MAX_FILES {
            return Err(format!("the package has more than {MAX_FILES} files"));
        }
        rel.pop();
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // Junctions aren't symlinks to `file_type`, but lead outside just the same.
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_: &std::fs::Metadata) -> bool {
    false
}

fn executable(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o100 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

/// The files of a source folder, read into memory (within the limits).
pub fn read_folder(dir: &Path) -> Result<Vec<File>, String> {
    let mut entries = Vec::new();
    walk(dir, &mut Vec::new(), &mut entries, true)?;
    let mut files = Vec::new();
    let mut total = 0u64;
    for (rel, kind) in entries {
        let Kind::File { executable } = kind else { continue };
        let path = rel.iter().fold(dir.to_path_buf(), |p, s| p.join(s));
        let shown = rel.join("/");
        // Reached from the top without following a link swapped in since the walk.
        let opened = crate::claim::open_inside(dir, &path).map_err(|_| format!("couldn't read {shown}: it changed, or a link was put in its way"))?;
        let mut f = opened.file;
        let mut data = Vec::new();
        f.by_ref().take(MAX_FILE_BYTES + 1).read_to_end(&mut data).map_err(|e| format!("couldn't read {shown}: {e}"))?;
        if data.len() as u64 > MAX_FILE_BYTES {
            return Err(format!("\"{shown}\" is larger than {} MB", MAX_FILE_BYTES >> 20));
        }
        total += data.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(format!("the package is larger than {} MB", MAX_TOTAL_BYTES >> 20));
        }
        files.push(File { parts: rel, data, executable });
    }
    Ok(files)
}

/// The files of a zip archive (checked by `zip::read`). A zip made by
/// zipping the plugin's folder has that folder at its top: it's taken off.
pub fn read_zip(path: &Path) -> Result<Vec<File>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    if meta.len() > super::zip::MAX_ARCHIVE_BYTES {
        return Err(format!("the zip file is larger than {} MB", super::zip::MAX_ARCHIVE_BYTES >> 20));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    let entries = super::zip::read(&bytes)?;
    let mut files: Vec<File> = entries.into_iter().map(|e| File { parts: e.parts, data: e.data, executable: e.executable }).collect();
    let at_top = files.iter().any(|f| f.parts.len() == 1 && f.parts[0] == manifest::FILE);
    if !at_top {
        let tops: std::collections::BTreeSet<&str> = files.iter().map(|f| f.parts[0].as_str()).collect();
        let wrapped = tops.len() == 1 && files.iter().all(|f| f.parts.len() > 1);
        if wrapped && files.iter().any(|f| f.parts.len() == 2 && f.parts[1] == manifest::FILE) {
            for f in &mut files {
                f.parts.remove(0);
            }
        }
    }
    Ok(files)
}

/// A package checked and written to a staging folder, not installed yet.
pub struct Staged {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// Check `files` (a manifest at the top that parses, a `run` file that's
/// there) and write them to a new staging folder in `Plugins`.
pub fn stage(config: &Path, files: Vec<File>) -> Result<Staged, String> {
    let manifest_file = files.iter().find(|f| f.parts.len() == 1 && f.parts[0] == manifest::FILE).ok_or_else(|| format!("there's no {} at the top of the package", manifest::FILE))?;
    let text = std::str::from_utf8(&manifest_file.data).map_err(|_| format!("{} isn't UTF-8 text", manifest::FILE))?;
    let manifest = manifest::parse(text)?;
    if manifest.run.in_package() {
        let want = command_parts(&manifest.run.command).expect("checked by the manifest");
        if !files.iter().any(|f| f.parts == want) {
            return Err(format!("\"run.command\" is {}, which isn't in the package", manifest.run.command));
        }
    }
    let root = plugins_dir(config);
    crate::private::create_dir_all(&root).map_err(|e| format!("couldn't create {}: {e}", root.display()))?;
    let dir = root.join(format!(".installing-{}", unique()));
    crate::private::create_dir_all(&dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let written = (|| {
        for f in &files {
            let (name, dirs) = f.parts.split_last().expect("paths have a name");
            let mut at = dir.clone();
            for d in dirs {
                at.push(d);
                match std::fs::symlink_metadata(&at) {
                    Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
                    Ok(_) => return Err(format!("{} is both a file and a folder in the package", dirs.join("/"))),
                    Err(_) => crate::private::create_dir_all(&at).map_err(|e| format!("couldn't create {}: {e}", at.display()))?,
                }
            }
            write_new(&at.join(name), &f.data, f.executable).map_err(|e| format!("couldn't write {}: {e}", f.parts.join("/")))?;
        }
        Ok(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok(Staged { dir, manifest })
}

/// A new file (never replacing one), owner-only, executable if asked.
fn write_new(path: &Path, data: &[u8], executable: bool) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, if executable { 0o700 } else { 0o600 });
    #[cfg(not(unix))]
    let _ = executable;
    let mut f = o.open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

fn unique() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{}-{nanos}", std::process::id())
}

/// Swap a staged package in as `Plugins/<name>`, replacing an older copy.
/// Returns the installed folder.
pub fn commit(config: &Path, staged: Staged) -> Result<PathBuf, String> {
    let root = plugins_dir(config);
    let target = root.join(&staged.manifest.name);
    let old = root.join(format!(".removing-{}", unique()));
    let had_old = std::fs::symlink_metadata(&target).is_ok();
    if had_old && let Err(e) = std::fs::rename(&target, &old) {
        let _ = std::fs::remove_dir_all(&staged.dir);
        return Err(format!("couldn't replace the installed copy: {e}"));
    }
    if let Err(e) = std::fs::rename(&staged.dir, &target) {
        // Put the old copy back rather than leave nothing.
        if had_old {
            let _ = std::fs::rename(&old, &target);
        }
        let _ = std::fs::remove_dir_all(&staged.dir);
        return Err(format!("couldn't install: {e}"));
    }
    if had_old {
        remove_tree(&old);
    }
    Ok(target)
}

/// Remove an installed plugin's folder (its data folder stays).
pub fn remove(config: &Path, name: &str) -> Result<(), String> {
    if !manifest::valid_name(name) {
        return Err(format!("\"{name}\" isn't a plugin name"));
    }
    let root = plugins_dir(config);
    let target = root.join(name);
    if std::fs::symlink_metadata(&target).is_err() {
        return Ok(());
    }
    // Renamed out of the way first, so the name is free at once even if
    // deleting the files takes a moment or partly fails.
    let gone = root.join(format!(".removing-{}", unique()));
    std::fs::rename(&target, &gone).map_err(|e| format!("couldn't remove {name}: {e}"))?;
    remove_tree(&gone);
    Ok(())
}

/// `remove_dir_all` (which never follows links); leftovers are tidied at
/// the next start (`tidy`).
fn remove_tree(dir: &Path) {
    if let Err(e) = std::fs::remove_dir_all(dir) {
        log::warn!("couldn't remove {}: {e}", dir.display());
    }
}

/// Staging and removal folders an interrupted install or removal left:
/// not this run's (another window may be installing right now), and only
/// old ones (another Strata may be).
pub fn tidy(config: &Path) {
    let Ok(entries) = std::fs::read_dir(plugins_dir(config)) else { return };
    let ours = format!("-{}-", std::process::id());
    let old = |e: &std::fs::DirEntry| e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 3600);
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if (name.starts_with(".installing-") || name.starts_with(".removing-")) && !name.contains(&ours) && old(&e) {
            remove_tree(&e.path());
        }
    }
}

/// Where approved copies of packages are kept, each named by its SHA-256
/// (in `Plugins`, which `installed` skips since the name starts with a dot).
pub const SNAPSHOTS: &str = ".snapshots";

/// Copy an installed package into a sealed (read-only) snapshot named by
/// its SHA-256, and return (sha, manifest, snapshot). The identity is that
/// of the copy itself, so what's approved is exactly what runs: a plugin
/// always runs from its snapshot, never from the installed folder, which
/// could change between the check and the start (or while the user reads
/// the permission question).
pub fn snapshot(config: &Path, dir: &Path) -> Result<(String, Manifest, PathBuf), String> {
    let root = plugins_dir(config).join(SNAPSHOTS);
    crate::private::create_dir_all(&root).map_err(|e| format!("couldn't create {}: {e}", root.display()))?;
    let tmp = root.join(format!(".copy-{}", unique()));
    let (sha, manifest) = match copy_tree(dir, &tmp).and_then(|_| identity(&tmp)) {
        Ok(r) => r,
        Err(e) => {
            remove_sealed(&tmp);
            return Err(e);
        }
    };
    let target = root.join(&sha);
    if std::fs::symlink_metadata(&target).is_err() {
        seal(&tmp);
        if std::fs::rename(&tmp, &target).is_ok() {
            return Ok((sha, manifest, target));
        }
    }
    // One for this identity is there already (from an earlier start, or
    // another window just now): it must still be exactly that package.
    remove_sealed(&tmp);
    verify_snapshot(&target, &sha)?;
    Ok((sha, manifest, target))
}

/// The snapshot is still the package `sha` names. If not, it's removed
/// (it was changed outside Strata) and the plugin isn't started.
pub fn verify_snapshot(snap: &Path, sha: &str) -> Result<(), String> {
    match hash_and_manifest(snap) {
        Ok((s, _)) if s == sha => Ok(()),
        _ => {
            remove_sealed(snap);
            Err("Strata's approved copy of it was changed outside Strata, so it wasn't started; start it again to check it afresh".into())
        }
    }
}

/// Remove snapshots nothing was granted for (and copies left half made).
pub fn prune_snapshots(config: &Path, keep: &std::collections::HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(plugins_dir(config).join(SNAPSHOTS)) else { return };
    let ours = format!("-{}-", std::process::id());
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let stale_copy = name.starts_with(".copy-") && !name.contains(&ours);
        if stale_copy || (!name.starts_with('.') && !keep.contains(&name)) {
            remove_sealed(&e.path());
        }
    }
}

/// Copy every folder and file of `src` (none may be a link) into `dst`.
fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    let mut entries = Vec::new();
    walk(src, &mut Vec::new(), &mut entries, false)?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    crate::private::create_dir_all(dst).map_err(|e| format!("couldn't create {}: {e}", dst.display()))?;
    let mut total = 0u64;
    for (rel, kind) in entries {
        let to = rel.iter().fold(dst.to_path_buf(), |p, s| p.join(s));
        let shown = rel.join("/");
        match kind {
            Kind::Dir => crate::private::create_dir_all(&to).map_err(|e| format!("couldn't create {shown}: {e}"))?,
            Kind::File { executable } => {
                let from = rel.iter().fold(src.to_path_buf(), |p, s| p.join(s));
                let opened = crate::claim::open_inside(src, &from).map_err(|_| format!("couldn't read {shown}: it changed, or a link was put in its way"))?;
                let mut data = Vec::new();
                (&opened.file).take(MAX_FILE_BYTES + 1).read_to_end(&mut data).map_err(|e| format!("couldn't read {shown}: {e}"))?;
                total += data.len() as u64;
                if data.len() as u64 > MAX_FILE_BYTES || total > MAX_TOTAL_BYTES {
                    return Err(format!("the package is larger than {} MB", MAX_TOTAL_BYTES >> 20));
                }
                write_new(&to, &data, executable).map_err(|e| format!("couldn't copy {shown}: {e}"))?;
            }
        }
    }
    Ok(())
}

/// Make a snapshot read-only: files lose their write bits (Windows: the
/// read-only attribute), folders become read-and-enter only, deepest first.
fn seal(dir: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return };
    if meta.is_dir() {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            seal(&e.path());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if meta.is_dir() { 0o500 } else { meta.permissions().mode() & 0o500 };
        if !meta.file_type().is_symlink() {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode));
        }
    }
    #[cfg(not(unix))]
    if meta.is_file() {
        let mut p = meta.permissions();
        p.set_readonly(true);
        let _ = std::fs::set_permissions(dir, p);
    }
}

/// Undo `seal` (so a snapshot can be removed), never following a link.
fn unseal(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if meta.file_type().is_symlink() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() | if meta.is_dir() { 0o700 } else { 0o200 };
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o7777));
    }
    #[cfg(not(unix))]
    {
        let mut p = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        let _ = std::fs::set_permissions(path, p);
    }
    if meta.is_dir() {
        for e in std::fs::read_dir(path).into_iter().flatten().flatten() {
            unseal(&e.path());
        }
    }
}

fn remove_sealed(dir: &Path) {
    unseal(dir);
    if std::fs::symlink_metadata(dir).is_ok() {
        remove_tree(dir);
    }
}

/// An installed plugin as found on disk: its folder and manifest (or why
/// the manifest can't be used).
pub struct Found {
    pub name: String,
    pub dir: PathBuf,
    pub manifest: Result<Manifest, String>,
}

/// Every installed plugin, by name.
pub fn installed(config: &Path) -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir(plugins_dir(config)) else { return Vec::new() };
    let mut out: Vec<Found> = Vec::new();
    for e in entries.flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        if name.starts_with('.') || !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let manifest = read_manifest(&e.path()).and_then(|m| if m.name == name { Ok(m) } else { Err(format!("the folder is named {name}, but the manifest says \"{}\"", m.name)) });
        out.push(Found { name, dir: e.path(), manifest });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Read and check `dir/strata-plugin.json` (not following a link).
pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let mut f = crate::claim::open_plain(&dir.join(manifest::FILE)).map_err(|e| format!("couldn't read {}: {e}", manifest::FILE))?;
    let mut text = String::new();
    f.by_ref().take(manifest::MAX_BYTES as u64 + 1).read_to_string(&mut text).map_err(|e| format!("couldn't read {}: {e}", manifest::FILE))?;
    manifest::parse(&text)
}

/// Make a tree writable again so a test can delete it (snapshots are read-only).
#[cfg(test)]
pub fn unseal_for_tests(dir: &Path) {
    unseal(dir);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-plugins-{tag}-{}", std::process::id()));
        super::unseal_for_tests(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub(crate) const MANIFEST: &str = r#"{"name": "demo", "version": "1.0.0", "api": "strata.plugin/1", "run": {"command": "bin/run.sh"}, "permissions": ["notify"], "contributes": {"commands": [{"id": "go", "title": "Go"}]}}"#;

    fn source(tag: &str) -> PathBuf {
        let src = temp(tag);
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join(manifest::FILE), MANIFEST).unwrap();
        std::fs::write(src.join("bin/run.sh"), "#!/bin/sh\n").unwrap();
        std::fs::create_dir_all(src.join(".git/objects")).unwrap();
        std::fs::write(src.join(".git/HEAD"), "ref").unwrap();
        src
    }

    #[test]
    fn relative_parts_stay_inside() {
        assert_eq!(relative_parts("bin/run.sh"), Some(vec!["bin".into(), "run.sh".into()]));
        assert_eq!(relative_parts("bin\\run.exe"), Some(vec!["bin".into(), "run.exe".into()]));
        for bad in ["", "/abs", "a/../b", "..", "./x", "a//b", "C:/x", "C:x", "a/", "\\\\server\\share"] {
            assert_eq!(relative_parts(bad), None, "{bad}");
        }
    }

    #[test]
    fn install_from_a_folder_then_update_and_remove() {
        let config = temp("install-config");
        let src = source("install-src");
        let staged = stage(&config, read_folder(&src).unwrap()).unwrap();
        assert_eq!(staged.manifest.name, "demo");
        let dir = commit(&config, staged).unwrap();
        assert_eq!(dir, config.join("Plugins/demo"));
        assert!(dir.join("bin/run.sh").is_file());
        assert!(!dir.join(".git").exists(), "a checkout's history isn't copied");
        let first = hash_dir(&dir).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(hash_dir(&dir).unwrap(), first, "stable");
        let found = installed(&config);
        assert_eq!(found.len(), 1);
        assert!(found[0].manifest.is_ok());

        // An update replaces it whole: a file gone in the new version is gone.
        std::fs::write(src.join("extra.txt"), "x").unwrap();
        commit(&config, stage(&config, read_folder(&src).unwrap()).unwrap()).unwrap();
        assert!(dir.join("extra.txt").exists());
        let second = hash_dir(&dir).unwrap();
        assert_ne!(second, first);
        std::fs::remove_file(src.join("extra.txt")).unwrap();
        commit(&config, stage(&config, read_folder(&src).unwrap()).unwrap()).unwrap();
        assert!(!dir.join("extra.txt").exists());
        assert_eq!(hash_dir(&dir).unwrap(), first, "same files, same identity");
        let leftovers: Vec<_> = std::fs::read_dir(config.join("Plugins")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(leftovers, ["demo"], "no staging folders left");

        remove(&config, "demo").unwrap();
        assert!(!dir.exists());
        assert!(installed(&config).is_empty());
        assert!(remove(&config, "../x").is_err());
        for d in [config, src] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn the_hash_sees_every_kind_of_change() {
        let dir = temp("hash");
        std::fs::write(dir.join("a"), "one").unwrap();
        let h0 = hash_dir(&dir).unwrap();
        std::fs::write(dir.join("a"), "two").unwrap();
        let h1 = hash_dir(&dir).unwrap();
        std::fs::rename(dir.join("a"), dir.join("b")).unwrap();
        let h2 = hash_dir(&dir).unwrap();
        std::fs::create_dir(dir.join("empty")).unwrap();
        let h3 = hash_dir(&dir).unwrap();
        let mut all = vec![h0, h1, h2, h3];
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("b"), std::fs::Permissions::from_mode(0o755)).unwrap();
            all.push(hash_dir(&dir).unwrap());
        }
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n, "every change gives a new identity");
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn links_are_refused_in_folders_and_installed_copies() {
        let src = source("links-src");
        std::os::unix::fs::symlink("/etc/passwd", src.join("passwd")).unwrap();
        let e = read_folder(&src).err().unwrap();
        assert!(e.contains("is a link"), "{e}");
        // A link added to an installed copy makes its identity unreadable (it won't start).
        let config = temp("links-config");
        std::fs::remove_file(src.join("passwd")).unwrap();
        let dir = commit(&config, stage(&config, read_folder(&src).unwrap()).unwrap()).unwrap();
        std::os::unix::fs::symlink("/etc", dir.join("bin/etc")).unwrap();
        assert!(hash_dir(&dir).unwrap_err().contains("is a link"));
        for d in [config, src] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn install_from_a_zip_with_or_without_a_top_folder() {
        use super::super::zip::build::{Item, zip};
        let config = temp("zip-config");
        let script = b"#!/bin/sh\n";
        for (tag, prefix) in [("wrapped", "demo-1.0/"), ("flat", "")] {
            let names = [format!("{prefix}{}", manifest::FILE), format!("{prefix}bin/run.sh")];
            let z = zip(&[Item::file(&names[0], MANIFEST.as_bytes()), Item { name: &names[1], data: script, deflate: true, mode: Some(0o100_755), claimed_size: None }]);
            let path = config.join(format!("{tag}.zip"));
            std::fs::write(&path, z).unwrap();
            let files = read_zip(&path).unwrap();
            assert!(files.iter().any(|f| f.parts == ["bin", "run.sh"] && f.executable), "{tag}");
            let dir = commit(&config, stage(&config, files).unwrap()).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(dir.join("bin/run.sh")).unwrap().permissions().mode() & 0o777, 0o700, "executable, owner-only");
                assert_eq!(std::fs::metadata(dir.join(manifest::FILE)).unwrap().permissions().mode() & 0o777, 0o600);
            }
            let _ = dir;
        }
        // A zip whose command isn't in it, or without a manifest.
        let z = zip(&[Item::file(manifest::FILE, MANIFEST.as_bytes())]);
        std::fs::write(config.join("norun.zip"), z).unwrap();
        let e = stage(&config, read_zip(&config.join("norun.zip")).unwrap()).err().unwrap();
        assert!(e.contains("isn't in the package"), "{e}");
        let z = zip(&[Item::file("readme.txt", b"hi")]);
        std::fs::write(config.join("nomanifest.zip"), z).unwrap();
        let e = stage(&config, read_zip(&config.join("nomanifest.zip")).unwrap()).err().unwrap();
        assert!(e.contains("no strata-plugin.json"), "{e}");
        let staging: Vec<_> = std::fs::read_dir(config.join("Plugins")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with('.')).collect();
        assert!(staging.is_empty(), "failed installs leave nothing behind");
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn folders_over_the_limits_are_refused() {
        let src = source("limits");
        std::fs::write(src.join("big.bin"), vec![0u8; (MAX_FILE_BYTES + 1) as usize]).unwrap();
        assert!(read_folder(&src).err().unwrap().contains("larger than"));
        std::fs::remove_file(src.join("big.bin")).unwrap();
        std::fs::create_dir(src.join("many")).unwrap();
        for i in 0..=MAX_FILES {
            std::fs::write(src.join(format!("many/{i}")), "").unwrap();
        }
        assert!(read_folder(&src).err().unwrap().contains("more than"));
        std::fs::remove_dir_all(src).ok();
    }

    #[test]
    fn a_folder_whose_manifest_disagrees_with_its_name_is_listed_as_broken() {
        let config = temp("broken");
        let dir = config.join("Plugins/other");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(manifest::FILE), MANIFEST).unwrap();
        std::fs::create_dir_all(config.join("Plugins/.installing-1")).unwrap();
        let found = installed(&config);
        assert_eq!(found.len(), 1);
        assert!(found[0].manifest.as_ref().unwrap_err().contains("folder is named other"));
        std::fs::create_dir_all(config.join("Plugins/.installing-2")).unwrap();
        // One left two hours ago; one another Strata may be using now.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        std::fs::File::open(config.join("Plugins/.installing-1")).unwrap().set_modified(old).unwrap();
        tidy(&config);
        assert!(!config.join("Plugins/.installing-1").exists());
        assert!(config.join("Plugins/.installing-2").exists(), "a fresh one is left alone");
        std::fs::remove_dir_all(config).ok();
    }
}
