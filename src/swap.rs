//! Putting files back as they were (Undo Replace in Files, Undo Rename),
//! all or nothing, and never over something written meanwhile.
//!
//! Checking a file and then writing it are two steps, and a sync tool or
//! another app can write in between. So, as in `claim`, files are claimed
//! before they're checked:
//!
//! 1. Prepare, for every file: its old bytes go into a recovery copy in a
//!    private staging folder next to it; then the current file is claimed
//!    into that folder (a no-replace rename through handles on its folders,
//!    opened without following links) and checked: size and every byte must
//!    be what the change left. If any file fails, every claimed file goes
//!    back where it was and nothing has changed.
//! 2. Commit, for every file: its current bytes are copied aside too, then
//!    the claimed file itself is rewritten with the old bytes (so it keeps
//!    its permissions, owner, extended attributes and other hard links; if
//!    the write fails it's rewritten back from that copy at once), then moved
//!    back to its name with a no-replace rename (a file that appeared there
//!    meanwhile is never replaced). If a step fails here, the files after it
//!    are put back too and the report says which file is in which state.
//!
//! Every name made, renamed or removed is flushed to disk with its folder.
//! Each staging folder's marker holds a journal (what's being undone, the
//! file, its destination, how far it got, the fingerprints to check), sealed
//! with this installation's key and updated at every step: after a crash,
//! Review Changes and Notes explain such folders and put them back
//! (`leftover`, `put_back_leftover`). A folder whose seal doesn't check out
//! was made by something else and is never acted on. Nothing a user wrote
//! is ever deleted.

use crate::claim::{self, Moved, imp};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

/// The old bytes, in the staging folder until the file has them again.
const TEMP: &str = ".strata-restore";
/// The file's bytes just before it was rewritten (to undo a failed rewrite).
const BEFORE: &str = ".strata-before";
/// The marker is rewritten through this name (never half-written).
const MARKER_NEW: &str = ".strata-claim.new";
/// Markers are small; anything bigger isn't one of Strata's.
const MARKER_MAX: u64 = 64 * 1024;

// ---- the journal's seal ---------------------------------------------------------

/// This installation's key for sealing journals: 32 random bytes in an
/// owner-only file in the config folder (made on first use). Tests use a
/// key of their own, in memory.
fn key() -> &'static [u8; 32] {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    KEY.get_or_init(load_key)
}

#[cfg(test)]
fn load_key() -> [u8; 32] {
    random_bytes()
}

#[cfg(not(test))]
fn load_key() -> [u8; 32] {
    let Some(dir) = crate::session::config_dir() else { return random_bytes() };
    let path = dir.join(KEY_FILE);
    let read = |p: &Path| std::fs::read(p).ok().and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok());
    if let Some(k) = read(&path) {
        return k;
    }
    let k = random_bytes();
    let _ = crate::private::create_dir_all(&dir);
    // A new file only: two Strata starting at once settle on one key.
    match crate::private::options().write(true).create_new(true).open(&path) {
        Ok(mut f) => {
            let _ = f.write_all(&k).and_then(|()| f.sync_all());
            k
        }
        Err(_) => read(&path).unwrap_or(k),
    }
}

/// The key's file name in the config folder.
pub const KEY_FILE: &str = "claim.key";

#[cfg(unix)]
fn random_bytes() -> [u8; 32] {
    let mut k = [0u8; 32];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut k)).is_err() {
        k = mixed_bytes();
    }
    k
}

#[cfg(not(unix))]
fn random_bytes() -> [u8; 32] {
    mixed_bytes()
}

/// 256 bits from `RandomState` keys (the OS random source, see `ipc`), hashed.
fn mixed_bytes() -> [u8; 32] {
    use sha2::Digest;
    use std::hash::{BuildHasher, Hasher};
    let mut h = sha2::Sha256::new();
    for i in 0..8u64 {
        let mut s = std::collections::hash_map::RandomState::new().build_hasher();
        s.write_u64(i);
        s.write_u128(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        h.update(s.finish().to_le_bytes());
    }
    h.finalize().into()
}

/// HMAC-SHA256 (RFC 2104) of `msg` under `key`.
fn hmac(key: &[u8; 32], msg: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    let (mut ipad, mut opad) = ([0x36u8; 64], [0x5cu8; 64]);
    for (i, b) in key.iter().enumerate() {
        ipad[i] ^= b;
        opad[i] ^= b;
    }
    let inner: [u8; 32] = sha2::Sha256::new().chain_update(ipad).chain_update(msg).finalize().into();
    sha2::Sha256::new().chain_update(opad).chain_update(inner).finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A marker's text: the claim line, the journal, and their seal.
fn marker_text(stamp: &str, journal: &serde_json::Value) -> Vec<u8> {
    let body = format!("strata-claim {stamp}\n{journal}\n");
    let seal = hex(&hmac(key(), body.as_bytes()));
    format!("{body}{seal}\n").into_bytes()
}

/// A marker's journal, and whether its seal is this installation's.
fn read_marker(stage: &Path) -> Option<(serde_json::Value, bool)> {
    let f = claim::open_plain(&stage.join(claim::MARKER)).ok()?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::Read::take(f, MARKER_MAX), &mut text).ok()?;
    let mut lines = text.split_inclusive('\n');
    let (head, json) = (lines.next()?, lines.next()?);
    if !head.starts_with("strata-claim ") {
        return None;
    }
    let journal: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    let seal = lines.next().map(str::trim).unwrap_or("");
    let body = format!("{head}{json}");
    // Compared whole; a forged seal is as wrong as a missing one.
    let sealed = seal.len() == 64 && seal == hex(&hmac(key(), body.as_bytes()));
    Some((journal, sealed))
}

// ---- restoring ------------------------------------------------------------------

/// One file to put back.
#[derive(Clone, Debug)]
pub struct Item {
    /// Reached from this folder without following links (the vault, or the
    /// file's own folder).
    pub top: PathBuf,
    pub path: PathBuf,
    /// The bytes to put back; None: the file itself moves (a rename back).
    pub original: Option<Arc<Vec<u8>>>,
    /// What the file must be now: (size, `claim::fingerprint`); None: any file.
    pub expected: Option<(u64, u64)>,
    /// If the file is this instead, it's left as it is (nothing to undo).
    pub unchanged: Option<(u64, u64)>,
    /// Where it goes, in the same folder: `path`, or its old name for a rename back.
    pub install_as: PathBuf,
}

impl Item {
    pub fn restore(top: PathBuf, path: PathBuf, original: Arc<Vec<u8>>, expected: (u64, u64)) -> Item {
        Item { top, install_as: path.clone(), path, original: Some(original), expected: Some(expected), unchanged: None }
    }
}

/// What became of a file.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Restored,
    /// Restored, but its emptied staging folder (here) couldn't be removed.
    RestoredWithLeftover(PathBuf),
    /// As it was before the undo started (put back, or nothing to do).
    Unchanged,
    /// Couldn't go back: it's here, in a staging folder (whose journal says what it is).
    KeptAt(PathBuf),
}

impl State {
    pub fn restored(&self) -> bool {
        matches!(self, State::Restored | State::RestoredWithLeftover(_))
    }
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Every file restored (or already as it should be).
    Done(Vec<(PathBuf, State)>),
    /// Nothing changed: these files are why (and any that couldn't go back).
    Refused(Vec<(PathBuf, String)>, Vec<(PathBuf, State)>),
    /// A step of the commit failed: each file's state, and the failure.
    Partial(Vec<(PathBuf, State)>, String),
}

/// Points between the steps, for tests to change things at (file index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    BeforeClaim(usize),
    Claimed(usize),
    BeforeInstall(usize),
    /// The claimed file has its old bytes again; not moved back yet.
    Rewritten(usize),
}

/// Phases, as the journal names them.
const CLAIMING: &str = "claiming";
const PREPARED: &str = "prepared";
const REWRITING: &str = "rewriting";
const REWRITTEN: &str = "rewritten";
const MOVING: &str = "moving";

#[cfg(test)]
thread_local! {
    /// Tests: the next in-place rewrite fails after truncating the file.
    pub static FAIL_REWRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct Prepared {
    /// Its index in the items given.
    index: usize,
    path: PathBuf,
    parent: imp::Dir,
    stage: Option<imp::Dir>,
    stage_name: String,
    /// The marker as last written (the journal), to put back if it must stay.
    marker: Vec<u8>,
    journal: serde_json::Value,
    name: String,
    install: String,
    bytes: Option<Arc<Vec<u8>>>,
    has_temp: bool,
    has_before: bool,
    /// The file matched `unchanged`: it only has to go back.
    untouched: bool,
}

impl Prepared {
    fn staged(&self, name: &str) -> PathBuf {
        self.stage_path().join(name)
    }

    fn stage(&self) -> &imp::Dir {
        self.stage.as_ref().expect("open until finished")
    }

    fn stage_path(&self) -> PathBuf {
        self.path.parent().unwrap_or(Path::new("")).join(&self.stage_name)
    }

    /// Record how far this file has got: the sealed marker is written whole
    /// to a new name, flushed, renamed over the old one and the folder
    /// flushed. Err: the journal couldn't be kept, so the next step mustn't start.
    fn phase(&mut self, phase: &str) -> std::io::Result<()> {
        self.journal["phase"] = phase.into();
        let stamp = self.stage_name.trim_start_matches(claim::STAGING).to_owned();
        let text = marker_text(&stamp, &self.journal);
        let _ = imp::remove_file_at(self.stage(), MARKER_NEW);
        imp::write_new_mode(self.stage(), MARKER_NEW, &text, 0o600)?;
        imp::rename_replace(self.stage(), MARKER_NEW, claim::MARKER)?;
        imp::sync_dir(self.stage())?;
        self.marker = text;
        Ok(())
    }

    /// The emptied staging folder goes (its handle first, then the marker);
    /// if it can't, its marker stays so a scan still recognizes it. True if it went.
    fn finish(mut self) -> bool {
        for (has, name) in [(self.has_temp, TEMP), (self.has_before, BEFORE)] {
            if has {
                let _ = imp::remove_file_at(self.stage(), name);
            }
        }
        drop(self.stage.take());
        let _ = imp::remove_in(&self.parent, &self.stage_name, claim::MARKER);
        let gone = imp::remove_empty_dir(&self.parent, &self.stage_name).is_ok();
        if !gone {
            let _ = imp::write_in(&self.parent, &self.stage_name, claim::MARKER, &self.marker);
        }
        let _ = imp::sync_dir(&self.parent);
        gone
    }

    /// Put the claimed file back at its name: Unchanged, or where it stays.
    fn put_back(self) -> State {
        let state = match imp::move_noreplace(self.stage(), &self.name, &self.parent, &self.name) {
            Moved::Done => None,
            Moved::Both if imp::drop_extra_link(&self.parent, &self.name, self.stage(), &self.name) => None,
            _ => Some(State::KeptAt(self.staged(&self.name))),
        };
        let _ = imp::sync_dir(&self.parent);
        match state {
            None => {
                self.finish();
                State::Unchanged
            }
            Some(s) => s,
        }
    }
}

fn parts(top: &Path, path: &Path) -> Option<(Vec<String>, String)> {
    let rel = path.strip_prefix(top).ok()?;
    let parts: Vec<String> = rel.components().map(|c| match c {
        std::path::Component::Normal(s) => s.to_str().map(str::to_owned),
        _ => None,
    }).collect::<Option<_>>()?;
    let (name, dirs) = parts.split_last()?;
    Some((dirs.to_vec(), name.clone()))
}

fn matches(file: &mut std::fs::File, want: (u64, u64)) -> bool {
    file.metadata().is_ok_and(|m| m.is_file() && m.len() == want.0) && claim::fingerprint_file(file) == Some(want.1)
}

fn pair(v: Option<&serde_json::Value>) -> Option<(u64, u64)> {
    let a = v?.as_array()?;
    Some((a.first()?.as_u64()?, a.get(1)?.as_u64()?))
}

/// Copy `from` to the new file `to` in `dir`, streamed, flushed to disk.
fn copy_in(dir: &imp::Dir, from: &str, to: &str) -> std::io::Result<()> {
    let mut src = imp::open_file_at(dir, from)?;
    imp::write_new_mode(dir, to, &[], 0o600)?;
    let mut dst = imp::open_write_at(dir, to)?;
    std::io::copy(&mut src, &mut dst)?;
    dst.sync_all()?;
    imp::sync_dir(dir)
}

/// Rewrite `name` in `dir` in place with `bytes`, flushed.
fn write_in_place(dir: &imp::Dir, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = imp::open_write_at(dir, name)?;
    f.set_len(0)?;
    #[cfg(test)]
    if FAIL_REWRITE.with(|c| c.replace(false)) {
        f.write_all(&bytes[..bytes.len() / 2])?;
        return Err(std::io::Error::other("disk full (test)"));
    }
    f.write_all(bytes)?;
    f.sync_all()
}

/// Claim and check one file. Err: why not (and where it is if it couldn't go back).
fn prepare(op: &str, item: &Item, i: usize, total: usize, hook: &mut dyn FnMut(Step)) -> Result<Prepared, (String, Option<State>)> {
    let fail = |why: &str| (why.to_owned(), None);
    let (dirs, name) = parts(&item.top, &item.path).ok_or_else(|| fail("isn't inside its folder"))?;
    let install = item.install_as.file_name().and_then(|n| n.to_str()).map(str::to_owned).ok_or_else(|| fail("has no usable name"))?;
    if item.install_as.parent() != item.path.parent() || [claim::MARKER, TEMP, BEFORE, MARKER_NEW].contains(&name.as_str()) {
        return Err(fail("can't be put back there"));
    }
    hook(Step::BeforeClaim(i));
    let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
    let parent = imp::open_parent(&item.top, &dirs).map_err(|_| fail("a folder on the way is gone or is a link"))?;
    // A rename back needs its old name free.
    if install != name && imp::open_file_at(&parent, &install).is_ok() {
        return Err((format!("“{install}” exists again"), None));
    }
    let stamp = format!("{}-{}-{i}", std::process::id(), SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    let stage_name = format!("{}{stamp}", claim::STAGING);
    imp::make_dir(&parent, &stage_name).map_err(|e| (format!("couldn't set it aside: {e}"), None))?;
    let stage = match imp::open_dir(&parent, &stage_name) {
        Ok(s) => s,
        Err(e) => {
            let _ = imp::remove_empty_dir(&parent, &stage_name);
            return Err((format!("couldn't set it aside: {e}"), None));
        }
    };
    let journal = serde_json::json!({
        "op": op,
        "source": item.path.to_string_lossy(),
        "dest": item.install_as.to_string_lossy(),
        "name": name,
        "phase": CLAIMING,
        "expected": item.expected.map(|(l, h)| vec![l, h]),
        "original": item.original.as_deref().map(|b| { let (l, h) = identity_of(b); vec![l, h] }),
        "order": i,
        "total": total,
    });
    let marker = marker_text(&stamp, &journal);
    let bytes = item.original.clone();
    let mut p = Prepared { index: i, path: item.path.clone(), parent, stage: Some(stage), stage_name, marker, journal, name, install, bytes, has_temp: false, has_before: false, untouched: false };
    let persisted = imp::write_new_mode(p.stage(), claim::MARKER, &p.marker, 0o600).and_then(|()| imp::sync_dir(p.stage())).and_then(|()| imp::sync_dir(&p.parent));
    if let Err(e) = persisted {
        p.finish();
        return Err((format!("couldn't set it aside: {e}"), None));
    }
    if let Some(bytes) = &item.original {
        p.has_temp = true;
        if let Err(e) = imp::write_new_mode(p.stage(), TEMP, bytes, 0o600).and_then(|()| imp::sync_dir(p.stage())) {
            p.finish();
            return Err((format!("couldn't write its old text: {e}"), None));
        }
    }
    match imp::move_noreplace(&p.parent, &p.name, p.stage(), &p.name) {
        Moved::Done => {}
        Moved::Not => {
            p.finish();
            return Err(fail("it's gone"));
        }
        Moved::Both => {
            if imp::drop_extra_link(p.stage(), &p.name, &p.parent, &p.name) {
                p.finish();
                return Err(fail("it couldn't be set aside"));
            }
            let at = p.staged(&p.name);
            return Err(("it couldn't be set aside".into(), Some(State::KeptAt(at))));
        }
    }
    if let Err(e) = imp::sync_dir(&p.parent).and_then(|()| imp::sync_dir(p.stage())) {
        return Err((format!("couldn't flush it to disk: {e}"), Some(p.put_back())));
    }
    hook(Step::Claimed(i));
    let mut claimed = match imp::open_file_at(p.stage(), &p.name) {
        Ok(f) => f,
        Err(_) => return Err(("changed since the change".into(), Some(p.put_back()))),
    };
    if let Some(want) = item.expected
        && !matches(&mut claimed, want)
    {
        drop(claimed);
        if item.unchanged.is_some_and(|u| imp::open_file_at(p.stage(), &p.name).is_ok_and(|mut f| matches(&mut f, u))) {
            p.untouched = true;
            p.journal["expected"] = item.unchanged.map(|(l, h)| vec![l, h]).into();
        } else {
            return Err(("changed since the change".into(), Some(p.put_back())));
        }
    }
    // It's rewritten in place: it must be writable (a read-only file is refused here, before anything changes).
    if item.original.is_some() && !p.untouched && imp::open_write_at(p.stage(), &p.name).is_err() {
        return Err(("can't be written (it's read-only)".into(), Some(p.put_back())));
    }
    // Which file this is, for recovery to check before it writes anything.
    p.journal["id"] = imp::id_at(p.stage(), &p.name).map(|(a, b)| vec![a, b]).into();
    if let Err(e) = p.phase(PREPARED) {
        return Err((format!("couldn't record its progress: {e}"), Some(p.put_back())));
    }
    Ok(p)
}

/// Install one prepared file. Ok(state), or Err(state, why) when it couldn't.
fn commit(mut p: Prepared, hook: &mut dyn FnMut(Step)) -> Result<State, (State, String)> {
    hook(Step::BeforeInstall(p.index));
    if p.untouched && p.install == p.name {
        return match p.put_back() {
            State::Unchanged => Ok(State::Unchanged),
            s => Err((s, "its name was taken meanwhile".into())),
        };
    }
    let rewriting = p.has_temp && !p.untouched;
    if rewriting {
        // Its current bytes go aside first: a failed rewrite is undone from them.
        if let Err(e) = copy_in(p.stage(), &p.name, BEFORE) {
            let _ = imp::remove_file_at(p.stage(), BEFORE);
            return Err((p.put_back(), format!("couldn't keep a copy of it ({e})")));
        }
        p.has_before = true;
        p.journal["before"] = p.journal["expected"].clone();
        if let Err(e) = p.phase(REWRITING) {
            return Err((p.put_back(), format!("couldn't record its progress ({e})")));
        }
        let bytes = p.bytes.clone().unwrap_or_default();
        if let Err(e) = write_in_place(p.stage(), &p.name, &bytes) {
            // Back as it was, from the copy, for every name the file has.
            let mut kept = Vec::new();
            let back = imp::open_file_at(p.stage(), BEFORE).and_then(|mut f| std::io::Read::read_to_end(&mut f, &mut kept)).and_then(|_| write_in_place(p.stage(), &p.name, &kept));
            return match back {
                Ok(()) => {
                    let _ = p.phase(PREPARED);
                    Err((p.put_back(), format!("couldn't write it back ({e}); it was left as it was")))
                }
                Err(e2) => {
                    let at = p.staged(&p.name);
                    Err((State::KeptAt(at), format!("couldn't write it back ({e}) or restore it ({e2}); both versions are kept beside it")))
                }
            };
        }
        if let Err(e) = p.phase(REWRITTEN) {
            let at = p.staged(&p.name);
            return Err((State::KeptAt(at), format!("couldn't record its progress ({e}); it has its old text, kept beside its copies")));
        }
        hook(Step::Rewritten(p.index));
    } else if let Err(e) = p.phase(MOVING) {
        return Err((p.put_back(), format!("couldn't record its progress ({e})")));
    }
    let moved = imp::move_noreplace(p.stage(), &p.name, &p.parent, &p.install);
    let _ = imp::sync_dir(&p.parent).and_then(|()| imp::sync_dir(p.stage()));
    match moved {
        Moved::Done => {
            let stage = p.stage_path();
            if p.finish() { Ok(State::Restored) } else { Ok(State::RestoredWithLeftover(stage)) }
        }
        _ if !rewriting => {
            // Not changed yet (a rename back): it goes back to its own name.
            let why = format!("“{}” was taken meanwhile", p.install);
            Err((p.put_back(), why))
        }
        _ => {
            // It has its old text again but can't go back: it stays, and its journal says so.
            let at = p.staged(&p.name);
            Err((State::KeptAt(at), format!("“{}” was taken meanwhile", p.install)))
        }
    }
}

/// Put every file in `items` back, all or nothing (`op` names the undo, for the journal).
#[cfg(test)]
pub fn restore_all(items: &[Item]) -> Outcome {
    restore_all_with("Undo", items, &mut |_| {})
}

/// `restore_all`, calling `hook` between the steps.
pub fn restore_all_with(op: &str, items: &[Item], hook: &mut dyn FnMut(Step)) -> Outcome {
    let mut prepared: Vec<Prepared> = Vec::new();
    let mut problems: Vec<(PathBuf, String)> = Vec::new();
    let mut states: Vec<(PathBuf, State)> = Vec::new();
    // Every file is tried, so the answer names all that are in the way.
    for (i, item) in items.iter().enumerate() {
        match prepare(op, item, i, items.len(), hook) {
            Ok(p) => prepared.push(p),
            Err((why, state)) => {
                problems.push((item.path.clone(), why));
                if let Some(s @ State::KeptAt(_)) = state {
                    states.push((item.path.clone(), s));
                }
            }
        }
    }
    if !problems.is_empty() {
        for p in prepared {
            let path = p.path.clone();
            if let s @ State::KeptAt(_) = p.put_back() {
                states.push((path, s));
            }
        }
        return Outcome::Refused(problems, states);
    }
    // A rename back first: if its old name was taken at the last moment,
    // nothing else has changed yet.
    prepared.sort_by_key(|p| p.install == p.name);
    let mut failure = None;
    for p in prepared {
        let path = p.path.clone();
        if failure.is_some() {
            states.push((path, p.put_back()));
            continue;
        }
        match commit(p, hook) {
            Ok(s) => states.push((path, s)),
            Err((s, why)) => {
                failure = Some(format!("{}: {why}", path.display()));
                states.push((path, s));
            }
        }
    }
    match failure {
        None => Outcome::Done(states),
        Some(why) => Outcome::Partial(states, why),
    }
}

// ---- after a crash -------------------------------------------------------------

/// A staging folder with a journal: one an interrupted undo of this
/// installation left (`sealed`), or one something else made.
#[derive(Clone, Debug, PartialEq)]
pub struct Leftover {
    pub stage: PathBuf,
    /// The seal is this installation's: the rest can be acted on.
    pub sealed: bool,
    /// "Undo Replace in Files", "Undo Rename" (only when sealed).
    pub op: String,
    /// Where the file was, and where the undo was putting it.
    pub source: PathBuf,
    pub dest: PathBuf,
    pub phase: String,
}

impl Leftover {
    /// What happened, and what Put Back does, in a sentence or two.
    pub fn describe(&self) -> String {
        if !self.sealed {
            return "Left by another program or another copy of Strata. Strata won't act on it: look inside and move what you need yourself.".into();
        }
        let file = |p: &Path| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
        let (what, put) = match self.phase.as_str() {
            CLAIMING | PREPARED => ("was set aside while its undo was being checked; it's as it was before the undo", format!("puts it back as {}", file(&self.source))),
            REWRITING => ("was being given back its old text", format!("finishes that from the copy kept beside it and puts it back as {}", file(&self.dest))),
            REWRITTEN => ("had its old text back but wasn't moved back yet", format!("puts it back as {}", file(&self.dest))),
            _ => ("was being renamed back", format!("puts it back as {}", file(&self.source))),
        };
        format!("Left by an interrupted {}: {} {what}. Put Back {put}. Nothing was deleted.", self.op, file(&self.source))
    }
}

/// The undo `stage` (a staging folder) was part of, if it has a journal.
pub fn leftover(stage: &Path) -> Option<Leftover> {
    let (j, sealed) = read_marker(stage)?;
    if !sealed {
        return Some(Leftover { stage: stage.to_owned(), sealed, op: String::new(), source: PathBuf::new(), dest: PathBuf::new(), phase: String::new() });
    }
    let s = |k: &str| j.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    Some(Leftover { stage: stage.to_owned(), sealed, op: s("op")?, source: PathBuf::from(s("source")?), dest: PathBuf::from(s("dest")?), phase: s("phase")? })
}

/// One plain file or folder name: no separators, not `.`/`..`, no NUL, no
/// drive or root (and no `:` on Windows, which would name a stream).
fn plain_name(s: &str) -> bool {
    let mut c = Path::new(s).components();
    !s.is_empty()
        && !s.contains(['/', '\\', '\0'])
        && !(cfg!(windows) && s.contains(':'))
        && matches!((c.next(), c.next()), (Some(std::path::Component::Normal(n)), None) if n == std::ffi::OsStr::new(s))
}

/// Put back what an interrupted undo left in `stage`, as its journal says.
/// `top` is the folder being looked at (the repository, the vault): the
/// staging folder must be inside it, and everything is reached from it
/// without following links. Only a journal sealed by this installation is
/// acted on, every name in it must be a plain name in the staging folder's
/// own folder, and the files are checked against the journal's fingerprints
/// before anything is written. Returns where the file went.
pub fn put_back_leftover(top: &Path, stage: &Path) -> Result<PathBuf, String> {
    let (j, sealed) = read_marker(stage).ok_or("its journal can't be read")?;
    if !sealed {
        return Err("it wasn't left by this copy of Strata, so Strata won't act on it".into());
    }
    let left = leftover(stage).ok_or("its journal is incomplete")?;
    let (dirs, stage_name) = parts(top, stage).ok_or("it isn't inside the folder being looked at")?;
    if !stage_name.starts_with(claim::STAGING) || !dirs.iter().all(|d| plain_name(d)) {
        return Err("it isn't one of Strata's staging folders".into());
    }
    let name = j.get("name").and_then(|v| v.as_str()).filter(|n| plain_name(n)).ok_or("its journal names no plain file")?.to_owned();
    let folder = stage.parent().ok_or("no folder")?;
    let own = |p: &Path| p.parent() == Some(folder) && p.file_name().and_then(|n| n.to_str()).is_some_and(plain_name);
    if !own(&left.source) || !own(&left.dest) || [claim::MARKER, TEMP, BEFORE, MARKER_NEW].contains(&name.as_str()) {
        return Err("its journal points outside its own folder".into());
    }
    let target = if matches!(left.phase.as_str(), REWRITING | REWRITTEN) { &left.dest } else { &left.source };
    let target_name = target.file_name().and_then(|n| n.to_str()).ok_or("no name")?.to_owned();
    let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
    let parent = imp::open_parent(top, &dirs).map_err(|_| "a folder on the way is gone or is a link".to_owned())?;
    let st = imp::open_dir(&parent, &stage_name).map_err(|_| "it isn't a plain folder".to_owned())?;
    let claimed = imp::open_file_at(&st, &name).is_ok();
    if claimed {
        // The file must be the one the journal describes before anything is done to it.
        let id = pair(j.get("id"));
        if id.is_some() && imp::id_at(&st, &name) != id {
            return Err("the file in it isn't the one its journal names".into());
        }
        let check = |key: &str| -> Result<(), String> {
            match pair(j.get(key)) {
                Some(want) if imp::open_file_at(&st, &name).is_ok_and(|mut f| matches(&mut f, want)) => Ok(()),
                Some(_) => Err("the file in it has changed since".into()),
                None => Ok(()),
            }
        };
        match left.phase.as_str() {
            PREPARED => check("expected")?,
            REWRITTEN => check("original")?,
            REWRITING => {
                if id.is_none() {
                    return Err("its journal doesn't say which file to finish".into());
                }
                // Finish the rewrite from the recovery copy, if it's whole.
                let want = pair(j.get("original")).ok_or("its journal has no fingerprint for the old text")?;
                let mut bytes = Vec::new();
                let mut copy = imp::open_file_at(&st, TEMP).map_err(|_| "its old text isn't there".to_owned())?;
                std::io::Read::read_to_end(&mut copy, &mut bytes).map_err(|e| e.to_string())?;
                if identity_of(&bytes) != want {
                    return Err("the copy of its old text isn't whole".into());
                }
                write_in_place(&st, &name, &bytes).map_err(|e| e.to_string())?;
            }
            _ => {}
        }
        match imp::move_noreplace(&st, &name, &parent, &target_name) {
            Moved::Done => {}
            _ => return Err(format!("“{target_name}” is taken: move it away first")),
        }
        let _ = imp::sync_dir(&parent).and_then(|()| imp::sync_dir(&st));
    }
    for n in [TEMP, BEFORE, MARKER_NEW, claim::MARKER] {
        let _ = imp::remove_file_at(&st, n);
    }
    drop(st);
    let _ = imp::remove_empty_dir(&parent, &stage_name);
    let _ = imp::sync_dir(&parent);
    Ok(target.clone())
}

/// A file's (size, fingerprint), streamed.
pub fn identity(path: &Path) -> Option<(u64, u64)> {
    let mut f = claim::open_plain(path).ok()?;
    let len = f.metadata().ok()?.len();
    Some((len, claim::fingerprint_file(&mut f)?))
}

/// (size, fingerprint) of bytes in memory.
pub fn identity_of(bytes: &[u8]) -> (u64, u64) {
    (bytes.len() as u64, claim::fingerprint(bytes))
}


// ---- making notes inside a vault, through pinned folders ---------------------

/// Points between checking a new note's place and making it (tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MakeStep {
    BeforeOpen,
}

/// Make the file `rel` (folders from `root`, `/`-separated) with `bytes`,
/// creating missing folders, never following a link and never replacing a
/// file: every folder is opened from the one above it, so a folder swapped
/// for a link meanwhile is refused rather than written through.
#[cfg(test)]
pub fn create_in(root: &Path, rel: &[&str], bytes: &[u8]) -> std::io::Result<()> {
    create_in_with(root, rel, bytes, &mut |_| {})
}

pub fn create_in_with(root: &Path, rel: &[&str], bytes: &[u8], hook: &mut dyn FnMut(MakeStep)) -> std::io::Result<()> {
    let Some((name, dirs)) = rel.split_last() else { return Err(std::io::ErrorKind::InvalidInput.into()) };
    hook(MakeStep::BeforeOpen);
    let mut dir = imp::open_parent(root, &[])?;
    for d in dirs {
        dir = match imp::open_dir(&dir, d) {
            Ok(next) => next,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                imp::make_dir_mode(&dir, d, 0o755)?;
                imp::open_dir(&dir, d)?
            }
            Err(e) => return Err(e),
        };
    }
    imp::write_new_mode(&dir, name, bytes, 0o644)
}

/// Read the file `rel` inside `root`, never following a link on the way.
pub fn read_in(root: &Path, rel: &[&str]) -> std::io::Result<Vec<u8>> {
    let Some((name, dirs)) = rel.split_last() else { return Err(std::io::ErrorKind::InvalidInput.into()) };
    let dir = imp::open_parent(root, dirs)?;
    let mut f = imp::open_file_at(&dir, name)?;
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut f, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("strata-swap-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("sub")).unwrap();
        d
    }

    /// `path` now holds `after`; the undo puts `before` back.
    fn item(top: &Path, path: &Path, before: &str, after: &str) -> Item {
        fs::write(path, after).unwrap();
        Item::restore(top.to_owned(), path.to_owned(), Arc::new(before.as_bytes().to_vec()), identity_of(after.as_bytes()))
    }

    fn staging(dir: &Path) -> Vec<String> {
        fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(claim::STAGING)).collect()
    }

    #[test]
    fn every_file_comes_back_and_nothing_is_left_over() {
        let d = temp("ok");
        let (a, b) = (d.join("a.txt"), d.join("sub/b.txt"));
        let items = [item(&d, &a, "old a\r\n", "new a\r\n"), item(&d, &b, "old b", "new b")];
        let out = restore_all(&items);
        assert!(matches!(out, Outcome::Done(ref s) if s.iter().all(|(_, s)| *s == State::Restored)), "{out:?}");
        assert_eq!(fs::read(&a).unwrap(), b"old a\r\n");
        assert_eq!(fs::read_to_string(&b).unwrap(), "old b");
        assert!(staging(&d).is_empty() && staging(&d.join("sub")).is_empty());
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_write_before_the_claim_refuses_everything() {
        let d = temp("refuse");
        let (a, b) = (d.join("a.txt"), d.join("sub/b.txt"));
        let items = [item(&d, &a, "old a", "new a"), item(&d, &b, "old b", "new b")];
        let out = restore_all_with("Undo", &items, &mut |s| {
            if s == Step::BeforeClaim(1) {
                fs::write(&b, "the sync tool's b").unwrap();
            }
        });
        let Outcome::Refused(why, kept) = out else { panic!("{out:?}") };
        assert_eq!(why.len(), 1);
        assert_eq!(why[0].0, b);
        assert!(kept.is_empty());
        // The first file, already claimed and checked, went back untouched.
        assert_eq!(fs::read_to_string(&a).unwrap(), "new a");
        assert_eq!(fs::read_to_string(&b).unwrap(), "the sync tool's b");
        assert!(staging(&d).is_empty() && staging(&d.join("sub")).is_empty());
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_file_written_at_the_name_after_the_claim_is_never_replaced() {
        let d = temp("taken");
        let a = d.join("a.txt");
        let items = [item(&d, &a, "old", "new")];
        let out = restore_all_with("Undo", &items, &mut |s| {
            if s == Step::BeforeInstall(0) {
                fs::write(&a, "written meanwhile").unwrap();
            }
        });
        let Outcome::Partial(states, why) = out else { panic!("{out:?}") };
        assert!(why.contains("taken"), "{why}");
        assert_eq!(fs::read_to_string(&a).unwrap(), "written meanwhile");
        // The file (with its old text back) couldn't go back: it's kept, said
        // where, and its journal says what it is.
        let State::KeptAt(at) = &states[0].1 else { panic!("{states:?}") };
        assert_eq!(fs::read_to_string(at).unwrap(), "old");
        assert!(claim::is_staging(at.parent().unwrap()), "marked, so a scan reports it");
        let left = leftover(at.parent().unwrap()).unwrap();
        assert_eq!((left.phase.as_str(), left.dest.clone()), ("rewritten", a.clone()));
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_commit_failure_puts_the_rest_back_and_says_which_is_where() {
        let d = temp("partial");
        let (a, b, c) = (d.join("a.txt"), d.join("b.txt"), d.join("c.txt"));
        let items = [item(&d, &a, "old a", "new a"), item(&d, &b, "old b", "new b"), item(&d, &c, "old c", "new c")];
        let out = restore_all_with("Undo", &items, &mut |s| {
            if s == Step::BeforeInstall(1) {
                fs::write(&b, "someone's b").unwrap();
            }
        });
        let Outcome::Partial(states, _) = out else { panic!("{out:?}") };
        assert_eq!(states[0], (a.clone(), State::Restored));
        assert!(matches!(states[1].1, State::KeptAt(_)));
        assert_eq!(states[2], (c.clone(), State::Unchanged));
        assert_eq!(fs::read_to_string(&a).unwrap(), "old a");
        assert_eq!(fs::read_to_string(&b).unwrap(), "someone's b");
        assert_eq!(fs::read_to_string(&c).unwrap(), "new c");
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_rename_back_moves_the_file_and_an_unchanged_one_is_left_alone() {
        let d = temp("rename");
        let (new, old) = (d.join("New.md"), d.join("Old.md"));
        fs::write(&new, "the note, edited since\n").unwrap();
        let back = Item { top: d.clone(), path: new.clone(), original: None, expected: None, unchanged: None, install_as: old.clone() };
        let other = d.join("o.md");
        fs::write(&other, "never saved: as it was\n").unwrap();
        let mut keep = Item::restore(d.clone(), other.clone(), Arc::new(b"old".to_vec()), identity_of(b"new"));
        keep.unchanged = Some(identity_of(b"never saved: as it was\n"));
        let out = restore_all(&[back, keep]);
        assert!(matches!(out, Outcome::Done(_)), "{out:?}");
        assert!(!new.exists());
        assert_eq!(fs::read_to_string(&old).unwrap(), "the note, edited since\n");
        assert_eq!(fs::read_to_string(&other).unwrap(), "never saved: as it was\n");
        // The old name taken again: refused before anything changes, the note stays where it is.
        fs::write(&new, "n").unwrap();
        let back = Item { top: d.clone(), path: new.clone(), original: None, expected: None, unchanged: None, install_as: old.clone() };
        assert!(matches!(restore_all(std::slice::from_ref(&back)), Outcome::Refused(..)));
        assert_eq!(fs::read_to_string(&new).unwrap(), "n");
        // Taken at the very last moment: the rename back goes first, so the
        // other file is put back untouched too.
        fs::remove_file(&old).unwrap();
        let other_item = item(&d, &other, "old o", "new o");
        let out = restore_all_with("Undo", &[other_item, back], &mut |s| {
            if s == Step::BeforeInstall(1) {
                fs::write(&old, "taken").unwrap();
            }
        });
        let Outcome::Partial(states, _) = out else { panic!("{out:?}") };
        assert!(states.iter().all(|(_, s)| *s == State::Unchanged), "{states:?}");
        assert_eq!(fs::read_to_string(&other).unwrap(), "new o");
        assert_eq!(fs::read_to_string(&new).unwrap(), "n");
        fs::remove_dir_all(d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_swapped_for_a_link_is_never_written_through() {
        let d = temp("link");
        let outside = temp("link-out");
        let b = d.join("sub/b.txt");
        let items = [item(&d, &b, "old", "new")];
        let out = restore_all_with("Undo", &items, &mut |s| {
            if s == Step::BeforeClaim(0) {
                fs::remove_dir_all(d.join("sub")).unwrap();
                std::os::unix::fs::symlink(&outside, d.join("sub")).unwrap();
                fs::write(outside.join("b.txt"), "new").unwrap();
            }
        });
        assert!(matches!(out, Outcome::Refused(..)), "{out:?}");
        assert_eq!(fs::read_to_string(outside.join("b.txt")).unwrap(), "new");
        assert!(staging(&outside).is_empty());
        // Making a note: the same refusal.
        let r = create_in_with(&d, &["sub", "n.md"], b"x", &mut |_| {});
        assert!(r.is_err());
        assert!(!outside.join("n.md").exists());
        fs::remove_dir_all(d).ok();
        fs::remove_dir_all(outside).ok();
    }

    /// The restored file is the same file: its mode, its other names (hard
    /// links) and its extended attributes stay.
    #[cfg(unix)]
    #[test]
    fn the_same_file_comes_back_with_its_mode_links_and_attributes() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let d = temp("inode");
        let (secret, tool, linked, sibling) = (d.join("secret.env"), d.join("tool.sh"), d.join("a.md"), d.join("sub/also-a.md"));
        let items = [item(&d, &secret, "TOKEN=old\n", "TOKEN=new\n"), item(&d, &tool, "echo old\n", "echo new\n"), item(&d, &linked, "old a\n", "new a\n")];
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        fs::hard_link(&linked, &sibling).unwrap();
        let ino = fs::metadata(&linked).unwrap().ino();
        #[cfg(target_os = "macos")]
        {
            let (p, name, value) = (std::ffi::CString::new(secret.to_str().unwrap()).unwrap(), c"com.strata.test", b"kept");
            // SAFETY: valid C strings and buffer.
            assert_eq!(unsafe { libc::setxattr(p.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) }, 0);
        }
        let out = restore_all(&items);
        assert!(matches!(out, Outcome::Done(_)), "{out:?}");
        assert_eq!(fs::read_to_string(&secret).unwrap(), "TOKEN=old\n");
        assert_eq!(fs::metadata(&secret).unwrap().permissions().mode() & 0o777, 0o600, "a secret stays private");
        assert_eq!(fs::metadata(&tool).unwrap().permissions().mode() & 0o777, 0o755, "a script stays runnable");
        assert_eq!(fs::metadata(&linked).unwrap().ino(), ino, "the same file");
        assert_eq!(fs::read_to_string(&sibling).unwrap(), "old a\n", "its other name sees the old text too");
        #[cfg(target_os = "macos")]
        {
            let (p, name) = (std::ffi::CString::new(secret.to_str().unwrap()).unwrap(), c"com.strata.test");
            let mut buf = [0u8; 16];
            // SAFETY: valid C strings and buffer of the given size.
            let n = unsafe { libc::getxattr(p.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), 0, 0) };
            assert_eq!(&buf[..n.max(0) as usize], b"kept", "extended attributes stay");
        }
        fs::remove_dir_all(d).ok();
    }

    fn stages(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = staging(dir).into_iter().map(|n| dir.join(n)).collect();
        v.sort();
        v
    }

    /// Restored, but its emptied staging folder couldn't go: said, with where.
    #[test]
    fn a_staging_folder_that_cant_be_removed_is_reported() {
        let d = temp("leftover");
        let a = d.join("a.txt");
        let items = [item(&d, &a, "old", "new")];
        let mut stray = None;
        let out = restore_all_with("Undo", &items, &mut |s| {
            if s == Step::Rewritten(0) {
                let stage = stages(&d).pop().unwrap();
                fs::write(stage.join("stray.txt"), "someone's").unwrap();
                stray = Some(stage);
            }
        });
        let Outcome::Done(states) = out else { panic!("{out:?}") };
        assert_eq!(states[0].1, State::RestoredWithLeftover(stray.clone().unwrap()));
        assert_eq!(fs::read_to_string(&a).unwrap(), "old");
        assert!(claim::is_staging(&stray.unwrap()), "still marked for a scan");
        fs::remove_dir_all(d).ok();
    }

    /// Run a restore that "crashes" (a panic) at `at`: what's left on disk is what a crash leaves.
    fn crash_at(items: &[Item], at: Step) {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            restore_all_with("Undo Replace in Files", items, &mut |s| {
                if s == at {
                    panic!("crash");
                }
            })
        }));
        assert!(r.is_err());
    }

    #[test]
    fn after_a_crash_once_checked_put_back_returns_every_file_as_it_was() {
        let d = temp("crash-prepared");
        let (a, b) = (d.join("a.txt"), d.join("sub/b.txt"));
        let items = [item(&d, &a, "old a", "new a"), item(&d, &b, "old b", "new b")];
        crash_at(&items, Step::BeforeInstall(0));
        assert!(!a.exists() && !b.exists(), "both were set aside when it crashed");
        let left: Vec<PathBuf> = stages(&d).into_iter().chain(stages(&d.join("sub"))).collect();
        assert_eq!(left.len(), 2);
        for stage in &left {
            let l = leftover(stage).unwrap();
            assert_eq!((l.op.as_str(), l.phase.as_str()), ("Undo Replace in Files", "prepared"));
            assert!(l.describe().contains("interrupted Undo Replace in Files") && !l.describe().contains("Trash"), "{}", l.describe());
            put_back_leftover(&d, stage).unwrap();
        }
        assert_eq!(fs::read_to_string(&a).unwrap(), "new a", "exactly as before the undo");
        assert_eq!(fs::read_to_string(&b).unwrap(), "new b");
        assert!(stages(&d).is_empty() && stages(&d.join("sub")).is_empty());
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn after_a_crash_mid_commit_put_back_finishes_or_returns_each_file_by_its_journal() {
        let d = temp("crash-commit");
        let (a, b, c) = (d.join("a.txt"), d.join("b.txt"), d.join("c.txt"));
        let items = [item(&d, &a, "old a", "new a"), item(&d, &b, "old b", "new b"), item(&d, &c, "old c", "new c")];
        crash_at(&items, Step::Rewritten(1));
        // a was done; b had its old text back but wasn't moved; c was only checked.
        assert_eq!(fs::read_to_string(&a).unwrap(), "old a");
        let phases: Vec<String> = stages(&d).iter().map(|s| leftover(s).unwrap().phase).collect();
        assert_eq!(phases.len(), 2);
        assert!(phases.contains(&"rewritten".to_owned()) && phases.contains(&"prepared".to_owned()), "{phases:?}");
        for stage in stages(&d) {
            put_back_leftover(&d, &stage).unwrap();
        }
        assert_eq!(fs::read_to_string(&b).unwrap(), "old b");
        assert_eq!(fs::read_to_string(&c).unwrap(), "new c");
        assert!(stages(&d).is_empty());
        // Cut off mid-write: the file is finished from the recovery copy.
        let e = d.join("e.txt");
        crash_at(&[item(&d, &e, "old e, the whole of it", "new e")], Step::BeforeInstall(0));
        let stage = stages(&d).pop().unwrap();
        reseal(&stage, |j| j["phase"] = "rewriting".into());
        fs::write(stage.join("e.txt"), "old e, the wh").unwrap();
        assert!(leftover(&stage).unwrap().describe().contains("finishes that"));
        assert_eq!(put_back_leftover(&d, &stage).unwrap(), e);
        assert_eq!(fs::read_to_string(&e).unwrap(), "old e, the whole of it");
        fs::remove_dir_all(d).ok();
    }

    /// Rewrite a leftover's journal and seal it again, as Strata would have.
    fn reseal(stage: &Path, change: impl FnOnce(&mut serde_json::Value)) {
        let (mut j, sealed) = read_marker(stage).unwrap();
        assert!(sealed);
        change(&mut j);
        let stamp = stage.file_name().unwrap().to_str().unwrap().trim_start_matches(claim::STAGING).to_owned();
        fs::write(stage.join(claim::MARKER), marker_text(&stamp, &j)).unwrap();
    }

    /// A staging folder in `d` with a journal (as a planted one would be),
    /// holding `x.md` and a recovery copy; `seal` decides how it's sealed.
    fn plant(d: &Path, journal: &serde_json::Value, seal: &str) -> PathBuf {
        let stage = d.join(format!("{}666-1-0", claim::STAGING));
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join(TEMP), "evil\n").unwrap();
        fs::write(stage.join("x.md"), "staged\n").unwrap();
        let body = format!("strata-claim 666-1-0\n{journal}\n");
        let text = match seal {
            "none" => body,
            "forged" => format!("{body}{}\n", hex(&hmac(&[7u8; 32], body.as_bytes()))),
            _ => String::from_utf8(marker_text("666-1-0", journal)).unwrap(),
        };
        fs::write(stage.join(claim::MARKER), text).unwrap();
        stage
    }

    /// A staging folder planted by someone else (a repository, a download)
    /// with a journal naming a file elsewhere: Put Back never touches
    /// anything, whether the journal is unsealed, forged, or sealed (as if
    /// the key had leaked) with names that leave its folder.
    #[test]
    fn a_planted_journal_never_touches_a_file_elsewhere() {
        let d = temp("planted");
        let outside = temp("planted-out");
        let victim = outside.join("document.txt");
        let x = d.join("x.md").to_string_lossy().into_owned();
        let evil = serde_json::json!([5, claim::fingerprint(b"evil\n")]);
        let v = victim.to_string_lossy().into_owned();
        let up = d.join("..").join(outside.file_name().unwrap()).join("document.txt").to_string_lossy().into_owned();
        let cases = [
            ("none", serde_json::json!({"op": "Undo Rename", "source": x, "dest": x, "name": v, "phase": "rewriting", "original": evil})),
            ("forged", serde_json::json!({"op": "Undo Rename", "source": x, "dest": x, "name": "x.md", "phase": "rewriting", "original": evil, "id": [0, 0]})),
            ("sealed", serde_json::json!({"op": "Undo Rename", "source": x, "dest": x, "name": v, "phase": "rewriting", "original": evil})),
            ("sealed", serde_json::json!({"op": "Undo Rename", "source": x, "dest": x, "name": "../document.txt", "phase": "rewriting", "original": evil})),
            ("sealed", serde_json::json!({"op": "Undo Rename", "source": x, "dest": x, "name": "a/x.md", "phase": "rewriting", "original": evil})),
            ("sealed", serde_json::json!({"op": "Undo Rename", "source": x, "dest": x, "name": "a\\x.md", "phase": "rewriting", "original": evil})),
            ("sealed", serde_json::json!({"op": "Undo Rename", "source": x, "dest": v, "name": "x.md", "phase": "rewritten"})),
            ("sealed", serde_json::json!({"op": "Undo Rename", "source": up, "dest": x, "name": "x.md", "phase": "moving"})),
            ("sealed", serde_json::json!({"op": "Undo Rename", "source": x, "dest": x, "name": "x.md", "phase": "rewriting", "original": evil})),
        ];
        for (seal, journal) in &cases {
            fs::write(&victim, "precious\n").unwrap();
            let stage = plant(&d, journal, seal);
            let l = leftover(&stage).unwrap();
            assert_eq!(l.sealed, *seal == "sealed", "{journal}");
            if !l.sealed {
                assert!(l.describe().contains("another program or another copy of Strata"), "{}", l.describe());
            }
            assert!(put_back_leftover(&d, &stage).is_err(), "{seal} {journal}");
            assert_eq!(fs::read_to_string(&victim).unwrap(), "precious\n", "never written: {journal}");
            assert_eq!(fs::read_to_string(stage.join("x.md")).unwrap(), "staged\n", "left as it was: {journal}");
            assert!(!d.join("x.md").exists() && !d.join("document.txt").exists(), "never moved: {journal}");
            fs::remove_dir_all(&stage).unwrap();
        }
        // A sealed journal edited afterwards no longer checks out.
        let fine = serde_json::json!({"op": "Undo Rename", "source": x, "dest": x, "name": "x.md", "phase": "moving"});
        let stage = plant(&d, &fine, "sealed");
        let text = fs::read_to_string(stage.join(claim::MARKER)).unwrap().replace("\"moving\"", "\"prepared\"");
        fs::write(stage.join(claim::MARKER), text).unwrap();
        assert!(!leftover(&stage).unwrap().sealed);
        assert!(put_back_leftover(&d, &stage).is_err());
        assert!(!d.join("x.md").exists());
        // Looked at from a folder it isn't in: refused.
        fs::remove_dir_all(&stage).unwrap();
        let stage = plant(&d, &fine, "sealed");
        assert!(put_back_leftover(&outside, &stage).is_err());
        assert!(!d.join("x.md").exists());
        fs::remove_dir_all(d).ok();
        fs::remove_dir_all(outside).ok();
    }

    /// The journal names the file by identity: one swapped in under its name
    /// (same text, another file) is never written or moved.
    #[test]
    fn put_back_checks_it_is_the_same_file() {
        let d = temp("swapped");
        let a = d.join("a.txt");
        crash_at(&[item(&d, &a, "old a", "new a")], Step::BeforeInstall(0));
        let stage = stages(&d).pop().unwrap();
        fs::remove_file(stage.join("a.txt")).unwrap();
        fs::write(stage.join("a.txt"), "new a").unwrap();
        assert!(put_back_leftover(&d, &stage).unwrap_err().contains("isn't the one"));
        assert!(!a.exists());
        // Rewritten, but changed since: refused too.
        let b = d.join("b.txt");
        crash_at(&[item(&d, &b, "old b", "new b")], Step::Rewritten(0));
        let stage = stages(&d).into_iter().find(|s| s.join("b.txt").exists()).unwrap();
        fs::write(stage.join("b.txt"), "someone else's").unwrap();
        assert!(put_back_leftover(&d, &stage).is_err());
        assert!(!b.exists());
        fs::remove_dir_all(d).ok();
    }

    /// If the journal can't be written, the next step never starts.
    #[test]
    fn a_journal_that_cant_be_kept_stops_before_the_next_step() {
        let d = temp("no-journal");
        let a = d.join("a.txt");
        let out = restore_all_with("Undo", &[item(&d, &a, "old a", "new a")], &mut |s| {
            if s == Step::Claimed(0) {
                // Something in the way of the marker's new name.
                fs::create_dir(stages(&d)[0].join(MARKER_NEW)).unwrap();
            }
        });
        let Outcome::Refused(why, kept) = out else { panic!("{out:?}") };
        assert!(why[0].1.contains("progress"), "{why:?}");
        assert!(kept.is_empty(), "{kept:?}");
        assert_eq!(fs::read_to_string(&a).unwrap(), "new a", "never rewritten");
        fs::remove_dir_all(d).ok();
    }

    /// A rewrite that fails partway is undone at once from the copy taken
    /// just before: every name of the file has its text back.
    #[cfg(unix)]
    #[test]
    fn a_failed_rewrite_puts_the_file_back_for_every_name() {
        let d = temp("failed-rewrite");
        let (a, other) = (d.join("a.txt"), d.join("other-name.txt"));
        fs::write(&a, "new text of a").unwrap();
        fs::hard_link(&a, &other).unwrap();
        FAIL_REWRITE.with(|c| c.set(true));
        let out = restore_all(&[item(&d, &a, "old a", "new text of a")]);
        let Outcome::Partial(states, why) = out else { panic!("{out:?}") };
        assert!(why.contains("left as it was"), "{why}");
        assert_eq!(states[0].1, State::Unchanged);
        assert_eq!(fs::read_to_string(&a).unwrap(), "new text of a");
        assert_eq!(fs::read_to_string(&other).unwrap(), "new text of a", "the other name too");
        assert!(stages(&d).is_empty());
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn notes_are_made_in_new_folders_and_never_replace() {
        let d = temp("make");
        create_in(&d, &["A", "B", "n.md"], b"hello").unwrap();
        assert_eq!(fs::read_to_string(d.join("A/B/n.md")).unwrap(), "hello");
        assert!(create_in(&d, &["A", "B", "n.md"], b"other").is_err());
        assert_eq!(fs::read_to_string(d.join("A/B/n.md")).unwrap(), "hello");
        assert_eq!(read_in(&d, &["A", "B", "n.md"]).unwrap(), b"hello");
        fs::remove_dir_all(d).ok();
    }
}
