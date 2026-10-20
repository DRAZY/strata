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
//! 2. Commit, for every file: the claimed file itself is rewritten with the
//!    old bytes (so it keeps its permissions, owner, extended attributes and
//!    other hard links), then moved back to its name with a no-replace
//!    rename (a file that appeared there meanwhile is never replaced), and
//!    the recovery copy goes. If a step fails here, the files after it are
//!    put back too and the report says which file is in which state.
//!
//! Each staging folder's marker holds a journal (what's being undone, the
//! file, its destination and how far it got), updated at every step, so a
//! crash leaves folders that Review Changes and Notes can explain and put
//! back (`leftover`, `put_back_leftover`). Nothing a user wrote is ever
//! deleted.

use crate::claim::{self, Moved, imp};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// The old bytes, in the staging folder until the file has them again.
const TEMP: &str = ".strata-restore";
/// The marker is rewritten through this name (never half-written).
const MARKER_NEW: &str = ".strata-claim.new";

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
    /// The file matched `unchanged`: it only has to go back.
    untouched: bool,
}

fn marker_text(stamp: &str, journal: &serde_json::Value) -> Vec<u8> {
    format!("strata-claim {stamp}\n{journal}\n").into_bytes()
}

impl Prepared {
    fn staged(&self, name: &str) -> PathBuf {
        self.path.parent().unwrap_or(Path::new("")).join(&self.stage_name).join(name)
    }

    fn stage(&self) -> &imp::Dir {
        self.stage.as_ref().expect("open until finished")
    }

    fn stage_path(&self) -> PathBuf {
        self.path.parent().unwrap_or(Path::new("")).join(&self.stage_name)
    }

    /// Record how far this file has got, in the marker (written whole, then renamed over it).
    fn phase(&mut self, phase: &str) {
        self.journal["phase"] = phase.into();
        let stamp = self.stage_name.trim_start_matches(claim::STAGING).to_owned();
        let text = marker_text(&stamp, &self.journal);
        let _ = imp::remove_file_at(self.stage(), MARKER_NEW);
        if imp::write_new(self.stage(), MARKER_NEW, &text).is_ok() && imp::rename_replace(self.stage(), MARKER_NEW, claim::MARKER).is_ok() {
            self.marker = text;
        }
    }

    /// The emptied staging folder goes (its handle first, then the marker);
    /// if it can't, its marker stays so a scan still recognizes it. True if it went.
    fn finish(mut self) -> bool {
        if self.has_temp {
            let _ = imp::remove_file_at(self.stage(), TEMP);
        }
        drop(self.stage.take());
        let _ = imp::remove_in(&self.parent, &self.stage_name, claim::MARKER);
        if imp::remove_empty_dir(&self.parent, &self.stage_name).is_err() {
            let _ = imp::write_in(&self.parent, &self.stage_name, claim::MARKER, &self.marker);
            return false;
        }
        true
    }

    /// Put the claimed file back at its name: Unchanged, or where it stays.
    fn put_back(self) -> State {
        match imp::move_noreplace(self.stage(), &self.name, &self.parent, &self.name) {
            Moved::Done => {
                self.finish();
                State::Unchanged
            }
            Moved::Both if imp::drop_extra_link(&self.parent, &self.name, self.stage(), &self.name) => {
                self.finish();
                State::Unchanged
            }
            _ => State::KeptAt(self.staged(&self.name)),
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

/// Claim and check one file. Err: why not (and where it is if it couldn't go back).
fn prepare(op: &str, item: &Item, i: usize, total: usize, hook: &mut dyn FnMut(Step)) -> Result<Prepared, (String, Option<State>)> {
    let fail = |why: &str| (why.to_owned(), None);
    let (dirs, name) = parts(&item.top, &item.path).ok_or_else(|| fail("isn't inside its folder"))?;
    let install = item.install_as.file_name().and_then(|n| n.to_str()).map(str::to_owned).ok_or_else(|| fail("has no usable name"))?;
    if item.install_as.parent() != item.path.parent() || name == claim::MARKER || name == TEMP || name == MARKER_NEW {
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
    let mut p = Prepared { index: i, path: item.path.clone(), parent, stage: Some(stage), stage_name, marker, journal, name, install, bytes, has_temp: false, untouched: false };
    if let Err(e) = imp::write_new(p.stage(), claim::MARKER, &p.marker) {
        p.finish();
        return Err((format!("couldn't set it aside: {e}"), None));
    }
    if let Some(bytes) = &item.original {
        if let Err(e) = imp::write_new_mode(p.stage(), TEMP, bytes, 0o600) {
            p.finish();
            return Err((format!("couldn't write its old text: {e}"), None));
        }
        p.has_temp = true;
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
            p.phase(PREPARED);
            return Ok(p);
        }
        return Err(("changed since the change".into(), Some(p.put_back())));
    }
    // It's rewritten in place: it must be writable (a read-only file is refused here, before anything changes).
    if item.original.is_some() && imp::open_write_at(p.stage(), &p.name).is_err() {
        return Err(("can't be written (it's read-only)".into(), Some(p.put_back())));
    }
    p.phase(PREPARED);
    Ok(p)
}

/// The claimed file gets its old bytes back, in place.
fn rewrite(p: &Prepared) -> std::io::Result<()> {
    let bytes = p.bytes.as_deref().ok_or(std::io::ErrorKind::InvalidInput)?;
    let mut f = imp::open_write_at(p.stage(), &p.name)?;
    f.set_len(0)?;
    f.write_all(bytes)?;
    f.sync_all()
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
        p.phase(REWRITING);
        if let Err(e) = rewrite(&p) {
            // The recovery copy has the old bytes; the journal says to finish from it.
            let at = p.staged(&p.name);
            return Err((State::KeptAt(at), format!("couldn't write it back ({e}); its old text is kept beside it")));
        }
        p.phase(REWRITTEN);
        hook(Step::Rewritten(p.index));
    } else {
        p.phase(MOVING);
    }
    match imp::move_noreplace(p.stage(), &p.name, &p.parent, &p.install) {
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

/// A staging folder an interrupted undo left, as its journal tells it.
#[derive(Clone, Debug, PartialEq)]
pub struct Leftover {
    pub stage: PathBuf,
    /// "Undo Replace in Files", "Undo Rename".
    pub op: String,
    /// Where the file was, and where the undo was putting it.
    pub source: PathBuf,
    pub dest: PathBuf,
    pub phase: String,
}

impl Leftover {
    /// What happened, and what Put Back does, in a sentence or two.
    pub fn describe(&self) -> String {
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

fn journal_of(stage: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(stage.join(claim::MARKER)).ok()?;
    let (head, json) = text.split_once('\n')?;
    if !head.starts_with("strata-claim") {
        return None;
    }
    serde_json::from_str(json.trim()).ok()
}

/// The undo `stage` (a staging folder) was part of, if it was one.
pub fn leftover(stage: &Path) -> Option<Leftover> {
    let j = journal_of(stage)?;
    let s = |k: &str| j.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    Some(Leftover { stage: stage.to_owned(), op: s("op")?, source: PathBuf::from(s("source")?), dest: PathBuf::from(s("dest")?), phase: s("phase")? })
}

/// Put back what an interrupted undo left in `stage`, as its journal says.
/// Returns where the file went.
pub fn put_back_leftover(stage: &Path) -> Result<PathBuf, String> {
    let j = journal_of(stage).ok_or("its journal can't be read")?;
    let left = leftover(stage).ok_or("its journal can't be read")?;
    let name = j.get("name").and_then(|v| v.as_str()).ok_or("its journal names no file")?.to_owned();
    let (parent_path, stage_name) = (stage.parent().ok_or("no folder")?, stage.file_name().and_then(|n| n.to_str()).ok_or("no name")?);
    let parent = imp::open_parent(parent_path, &[]).map_err(|e| e.to_string())?;
    let st = imp::open_dir(&parent, stage_name).map_err(|e| e.to_string())?;
    let claimed = imp::open_file_at(&st, &name).is_ok();
    let target = match left.phase.as_str() {
        REWRITING | REWRITTEN => &left.dest,
        _ => &left.source,
    };
    let target_name = target.file_name().and_then(|n| n.to_str()).ok_or("no name")?.to_owned();
    if target.parent() != Some(parent_path) {
        return Err("its journal points elsewhere".into());
    }
    if claimed {
        if left.phase == REWRITING {
            // Finish the rewrite from the recovery copy, if it's whole.
            let want = j.get("original").and_then(|v| v.as_array()).and_then(|a| Some((a.first()?.as_u64()?, a.get(1)?.as_u64()?)));
            let mut copy = imp::open_file_at(&st, TEMP).map_err(|_| "its old text isn't there".to_owned())?;
            if want.is_none_or(|w| !matches(&mut copy, w)) {
                return Err("the copy of its old text isn't whole".into());
            }
            let mut bytes = Vec::new();
            let mut copy = imp::open_file_at(&st, TEMP).map_err(|e| e.to_string())?;
            std::io::Read::read_to_end(&mut copy, &mut bytes).map_err(|e| e.to_string())?;
            let mut f = imp::open_write_at(&st, &name).map_err(|e| e.to_string())?;
            f.set_len(0).and_then(|()| f.write_all(&bytes)).and_then(|()| f.sync_all()).map_err(|e| e.to_string())?;
        }
        match imp::move_noreplace(&st, &name, &parent, &target_name) {
            Moved::Done => {}
            _ => return Err(format!("“{target_name}” is taken: move it away first")),
        }
    }
    let _ = imp::remove_file_at(&st, TEMP);
    let _ = imp::remove_file_at(&st, MARKER_NEW);
    let _ = imp::remove_file_at(&st, claim::MARKER);
    drop(st);
    let _ = imp::remove_empty_dir(&parent, stage_name);
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
            put_back_leftover(stage).unwrap();
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
            put_back_leftover(&stage).unwrap();
        }
        assert_eq!(fs::read_to_string(&b).unwrap(), "old b");
        assert_eq!(fs::read_to_string(&c).unwrap(), "new c");
        assert!(stages(&d).is_empty());
        // Cut off mid-write: the file is finished from the recovery copy.
        let e = d.join("e.txt");
        crash_at(&[item(&d, &e, "old e, the whole of it", "new e")], Step::BeforeInstall(0));
        let stage = stages(&d).pop().unwrap();
        let marker = fs::read_to_string(stage.join(claim::MARKER)).unwrap().replace("\"prepared\"", "\"rewriting\"");
        fs::write(stage.join(claim::MARKER), marker).unwrap();
        fs::write(stage.join("e.txt"), "old e, the wh").unwrap();
        assert!(leftover(&stage).unwrap().describe().contains("finishes that"));
        assert_eq!(put_back_leftover(&stage).unwrap(), e);
        assert_eq!(fs::read_to_string(&e).unwrap(), "old e, the whole of it");
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
