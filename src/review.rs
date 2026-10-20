//! Review Changes: the files that differ from a base (the last commit, or a
//! snapshot taken at Start Review), and the edits that keep or revert one
//! change. The list is worked out on a background thread with the `git`
//! command line, which only reads the repository and the files as they are
//! (see `git::git_io`); nothing here writes to either.
//!
//! Why not `git status` / `git diff`: comparing the working tree through Git
//! can run filter programs named in the repository's config, and Strata
//! never lets a repository run programs. Instead the files are hashed with
//! `git hash-object --no-filters`, compared with the blob ids of the base
//! commit, and only the files that differ are read and diffed. What each
//! file looked like (size, modification time) is remembered, so a rescan
//! only hashes files that changed since the last one.
//!
//! A snapshot is the commit at HEAD when the review started plus a copy of
//! the files that already differed from it then. It lives in memory: nothing
//! is written to the repository (no stash, no temporary commit) or to disk,
//! and commits the agent makes during the review don't move the base.

use crate::diff::{self, Hunk};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Line counts are skipped for files bigger than this (either side).
const MAX_COUNT_BYTES: u64 = 8 << 20;
/// Files bigger than this aren't read at all (shown as changed, no counts).
const MAX_READ_BYTES: u64 = 64 << 20;
/// Stop listing the working tree past this many files.
const MAX_FILES: usize = 200_000;
/// A snapshot copies at most this many bytes of already-changed files.
const MAX_SNAPSHOT_BYTES: usize = 512 << 20;
/// Paths per `git hash-object` call, and blobs per `git cat-file` call.
const BATCH: usize = 500;
/// Time allowed for one batched git call.
const BATCH_TIMEOUT: Duration = Duration::from_secs(60);
/// Output allowed from one batched `git cat-file`.
const BATCH_BYTES: usize = 256 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Modified,
    Added,
    Deleted,
    Renamed,
}

impl Status {
    pub fn letter(self) -> &'static str {
        match self {
            Status::Modified => "M",
            Status::Added => "A",
            Status::Deleted => "D",
            Status::Renamed => "R",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Modified => "Modified",
            Status::Added => "New",
            Status::Deleted => "Deleted",
            Status::Renamed => "Renamed",
        }
    }
}

/// One file that differs from the base.
#[derive(Clone, Debug, PartialEq)]
pub struct FileChange {
    /// Where the file is (for a deleted file, where it was).
    pub path: PathBuf,
    /// The same, relative to the repository's top folder, with `/`.
    pub rel: String,
    /// A renamed file's old path and relative path.
    pub old: Option<(PathBuf, String)>,
    pub status: Status,
    /// Lines (added, removed); None for binary or very large files.
    pub lines: Option<(usize, usize)>,
    pub binary: bool,
    /// Fingerprint of the file's bytes (of the base's, for a deleted file).
    pub hash: u64,
    /// Every change in it was kept: it matches the text kept for it.
    pub kept: bool,
}

/// What the changes are measured against.
#[derive(Clone, Default)]
pub enum Base {
    /// The last commit, wherever HEAD points now.
    #[default]
    Head,
    Snapshot(Arc<Snapshot>),
}

/// The project when Start Review was chosen.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub top: PathBuf,
    /// HEAD then; None on a branch with no commits yet.
    pub commit: Option<String>,
    /// Files that already differed from `commit`, by relative path: their
    /// bytes then, or None if they didn't exist.
    pub files: HashMap<String, Option<Arc<Vec<u8>>>>,
}

/// One finished scan.
#[derive(Clone, Debug, Default)]
pub struct Scan {
    /// The repository's top folder (spelled like the folder asked about).
    pub top: PathBuf,
    /// The commit compared against.
    pub commit: Option<String>,
    pub files: Vec<FileChange>,
    pub took: Duration,
}

pub enum Outcome {
    /// A scan for request `generation`.
    Scanned(u64, Result<Scan, String>),
    Snapshot(Result<Snapshot, String>),
}

// ---------------------------------------------------------------------------
// Keep and revert
// ---------------------------------------------------------------------------

/// (byte, char) offsets where each line starts, split like a buffer (at
/// `\n`, `\r\n` or a lone `\r`, see `compare::buffer_lines`), then the text's
/// end: one more entry than there are lines.
fn line_starts(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let mut out = vec![(0, 0)];
    let (mut i, mut chars) = (0, 0);
    while i < b.len() {
        let len = match (b[i], b.get(i + 1)) {
            (b'\r', Some(b'\n')) => 2,
            (b'\r' | b'\n', _) => 1,
            _ => 0,
        };
        if len == 0 {
            // Count a char at each UTF-8 lead byte.
            if b[i] & 0xC0 != 0x80 {
                chars += 1;
            }
            i += 1;
            continue;
        }
        i += len;
        chars += len;
        out.push((i, chars));
    }
    out.push((b.len(), chars));
    out
}

/// Where lines `lines` of a text sit, as (byte, char) ranges. A block that
/// runs to the end of both texts also takes the line break before it, so
/// adding or removing the final newline is part of the change.
fn span(s: &str, starts: &[(usize, usize)], lines: &Range<usize>, to_end: bool) -> (Range<usize>, Range<usize>) {
    let n = starts.len() - 1;
    let (mut b0, mut c0) = starts[lines.start.min(n)];
    let (b1, c1) = starts[lines.end.min(n)];
    if to_end && lines.start > 0 {
        // From the end of the previous line's text: before its break, if it has one.
        let k = lines.start.min(n);
        (b0, c0) = starts[k];
        if k < n {
            let brk = if s.as_bytes()[..b0].ends_with(b"\r\n") { 2 } else { 1 };
            (b0, c0) = (b0 - brk, c0 - brk);
        }
    }
    (b0..b1, c0..c1)
}

/// The edit that turns lines `h.b` of `cur` back into lines `h.a` of `base`:
/// a char range of `cur` and its replacement, exactly the base's text with
/// its own line breaks. `h` must come from diffing these two texts.
pub fn revert_edit(base: &str, cur: &str, h: &Hunk) -> (Range<usize>, String) {
    let (sa, sb) = (line_starts(base), line_starts(cur));
    let to_end = h.a.end >= sa.len() - 1 && h.b.end >= sb.len() - 1;
    let (a, _) = span(base, &sa, &h.a, to_end);
    let (_, b) = span(cur, &sb, &h.b, to_end);
    (b, base[a].to_owned())
}

/// The base with hunk `h` kept: lines `h.a` of `base` replaced by lines
/// `h.b` of `cur`, so that change no longer shows.
pub fn keep_edit(base: &str, cur: &str, h: &Hunk) -> String {
    let (sa, sb) = (line_starts(base), line_starts(cur));
    let to_end = h.a.end >= sa.len() - 1 && h.b.end >= sb.len() - 1;
    let (a, _) = span(base, &sa, &h.a, to_end);
    let (b, _) = span(cur, &sb, &h.b, to_end);
    let mut out = String::with_capacity(base.len() + b.len());
    out.push_str(&base[..a.start]);
    out.push_str(&cur[b]);
    out.push_str(&base[a.end..]);
    out
}

/// The base as the file would be checked out: a blob stored with `\n` in a
/// repository that writes `\r\n` files (`core.autocrlf`, `.gitattributes`
/// `eol=crlf`) reads with `\r\n` when the file itself uses them.
pub fn as_checked_out(base: String, file_uses_crlf: bool) -> String {
    if file_uses_crlf && !base.contains('\r') && base.contains('\n') { base.replace('\n', "\r\n") } else { base }
}

// ---------------------------------------------------------------------------
// Comparing files
// ---------------------------------------------------------------------------

fn fingerprint(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Git's test for binary: a NUL byte near the start.
fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8000)].contains(&0)
}

/// Same bytes, or the same text with `\n` checked out as `\r\n`.
fn same_content(base: &[u8], work: &[u8]) -> bool {
    if base == work {
        return true;
    }
    if base.contains(&b'\r') || work.len() <= base.len() {
        return false;
    }
    let mut w = work.iter();
    for &c in base {
        if c == b'\n' && w.next() != Some(&b'\r') {
            return false;
        }
        if w.next() != Some(&c) {
            return false;
        }
    }
    w.next().is_none()
}

/// Lines (added, removed) turning `base` into `work`; None when either is
/// binary or too big to count.
fn count_lines(base: &[u8], work: &[u8]) -> Option<(usize, usize)> {
    if is_binary(base) || is_binary(work) || base.len() as u64 > MAX_COUNT_BYTES || work.len() as u64 > MAX_COUNT_BYTES {
        return None;
    }
    let (a, _, _) = crate::encoding::decode_detect(base);
    let (b, _, _) = crate::encoding::decode_detect(work);
    let hunks = diff::diff_lines(&crate::compare::buffer_lines(&a), &crate::compare::buffer_lines(&b), false);
    let (_, removed, added) = diff::stats(&hunks);
    Some((added, removed))
}

/// Read a file of at most `MAX_READ_BYTES`.
fn read_small(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_READ_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

/// Where a file's base comes from.
#[derive(Clone)]
enum Src {
    Blob(String),
    Bytes(Arc<Vec<u8>>),
}

impl Src {
    fn key(&self) -> u64 {
        match self {
            Src::Blob(oid) => fingerprint(oid.as_bytes()),
            Src::Bytes(b) => fingerprint(b) ^ 0x5bd1_e995,
        }
    }
}

/// What a file looked like on disk.
type Stat = (u64, Option<SystemTime>);

/// Remembered between scans.
#[derive(Default)]
struct Cache {
    /// The base commit's files: relative path → blob id.
    tree: Option<(String, Arc<HashMap<String, String>>)>,
    /// Per file: its stat and base when last looked at, and what was found
    /// (None: same as the base).
    seen: HashMap<String, (Option<Stat>, u64, Option<FileChange>)>,
}

/// The top folder of the repository holding `folder`, spelled like
/// `folder` (git's own spelling resolves symlinks such as macOS's /var).
fn top_of(folder: &Path) -> Option<PathBuf> {
    let dir = crate::git::work_dir(folder)?;
    let out = crate::git::git(dir, &["rev-parse", "--show-cdup"])?;
    let up = String::from_utf8_lossy(&out).trim().to_owned();
    let mut top = dir.to_path_buf();
    for part in up.split('/').filter(|p| !p.is_empty()) {
        if part == ".." {
            top.pop();
        }
    }
    Some(top)
}

fn head_commit(top: &Path) -> Option<String> {
    crate::git::git_line(top, &["rev-parse", "-q", "--verify", "HEAD^{commit}"])
}

/// Blob ids of `commit`'s files (symlinks and submodules left out).
fn read_tree(top: &Path, commit: &str) -> Result<HashMap<String, String>, String> {
    let out = crate::git::git_io(top, &["ls-tree", "-r", "-z", "--full-tree", commit], None, BATCH_BYTES, BATCH_TIMEOUT).ok_or("Could not list the files in the last commit")?;
    if out.len() > BATCH_BYTES {
        return Err("The repository has too many files to review".into());
    }
    let mut files = HashMap::new();
    for rec in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        // "<mode> <type> <id>\t<path>"
        let Some(tab) = rec.iter().position(|&b| b == b'\t') else { continue };
        let (head, path) = (String::from_utf8_lossy(&rec[..tab]), String::from_utf8_lossy(&rec[tab + 1..]));
        let mut parts = head.split(' ');
        let (Some(mode), Some(kind), Some(id)) = (parts.next(), parts.next(), parts.next()) else { continue };
        if kind == "blob" && mode != "120000" {
            files.insert(path.into_owned(), id.to_owned());
        }
    }
    Ok(files)
}

/// Blob contents by id (`git cat-file --batch`), in batches; blobs that
/// can't be read (too big) are left out.
fn read_blobs(top: &Path, ids: &[String]) -> HashMap<String, Arc<Vec<u8>>> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(BATCH) {
        let input = chunk.iter().flat_map(|id| [id.as_bytes(), b"\n"]).flatten().copied().collect();
        let got = crate::git::git_io(top, &["cat-file", "--batch"], Some(input), BATCH_BYTES, BATCH_TIMEOUT).filter(|o| o.len() <= BATCH_BYTES);
        match got {
            Some(bytes) => parse_batch(&bytes, &mut out),
            // Too much at once: one at a time, skipping the huge ones.
            None => {
                for id in chunk {
                    if let Some(b) = crate::git::git_io(top, &["cat-file", "blob", id], None, MAX_READ_BYTES as usize, BATCH_TIMEOUT).filter(|b| b.len() as u64 <= MAX_READ_BYTES) {
                        out.insert(id.clone(), Arc::new(b));
                    }
                }
            }
        }
    }
    out
}

/// `git cat-file --batch` output: "<id> <type> <size>\n<bytes>\n" per object
/// ("<id> missing\n" for an unknown one).
fn parse_batch(mut bytes: &[u8], out: &mut HashMap<String, Arc<Vec<u8>>>) {
    while let Some(nl) = bytes.iter().position(|&b| b == b'\n') {
        let header = String::from_utf8_lossy(&bytes[..nl]).into_owned();
        bytes = &bytes[nl + 1..];
        let parts: Vec<&str> = header.split(' ').collect();
        let [id, _, size] = parts[..] else { continue };
        let Ok(size) = size.parse::<usize>() else { return };
        if bytes.len() < size {
            return;
        }
        out.insert(id.to_owned(), Arc::new(bytes[..size].to_vec()));
        bytes = bytes.get(size + 1..).unwrap_or_default();
    }
}

/// Git's blob ids for working files, hashed as they are on disk (no filters).
fn hash_files(top: &Path, rels: &[&str]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for chunk in rels.chunks(BATCH) {
        let input: Vec<u8> = chunk.iter().flat_map(|r| [r.as_bytes(), b"\n"]).flatten().copied().collect();
        let Some(ids) = crate::git::git_io(top, &["hash-object", "--no-filters", "--stdin-paths"], Some(input), 1 << 20, BATCH_TIMEOUT) else { continue };
        let ids = String::from_utf8_lossy(&ids);
        let ids: Vec<&str> = ids.lines().collect();
        if ids.len() == chunk.len() {
            out.extend(chunk.iter().zip(ids).map(|(r, id)| ((*r).to_owned(), id.to_owned())));
        }
    }
    out
}

/// The working tree's files: relative path → stat. Respects .gitignore
/// like Git does, and leaves out `.git` and nested repositories.
fn walk(top: &Path) -> Result<HashMap<String, Stat>, String> {
    let mut files = HashMap::new();
    let walker = ignore::WalkBuilder::new(top)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| {
            let dir = e.file_type().is_some_and(|t| t.is_dir());
            !(dir && (e.file_name() == ".git" || (e.depth() > 0 && e.path().join(".git").exists())))
        })
        .build();
    for e in walker.flatten() {
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(rel) = rel_of(top, e.path()) else { continue };
        let meta = e.metadata().ok();
        files.insert(rel, (meta.as_ref().map_or(0, |m| m.len()), meta.and_then(|m| m.modified().ok())));
        if files.len() > MAX_FILES {
            return Err(format!("Over {MAX_FILES} files: too many to review"));
        }
    }
    Ok(files)
}

/// `path` relative to `top`, with `/`.
fn rel_of(top: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(top).ok()?;
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn stat(path: &Path) -> Option<Stat> {
    let m = std::fs::symlink_metadata(path).ok().filter(|m| m.is_file())?;
    Some((m.len(), m.modified().ok()))
}

/// Find the files under `folder`'s repository that differ from `base`.
/// `kept` holds texts the user kept per file (see `FileChange::kept`).
fn scan(folder: &Path, base: &Base, kept: &HashMap<PathBuf, Arc<String>>, cache: &mut Cache) -> Result<Scan, String> {
    let started = Instant::now();
    let snapshot = match base {
        Base::Snapshot(s) => Some(s.clone()),
        Base::Head => None,
    };
    let top = match &snapshot {
        Some(s) => s.top.clone(),
        None => top_of(folder).ok_or("Not in a Git repository: Review Changes compares files with Git")?,
    };
    let commit = match &snapshot {
        Some(s) => s.commit.clone(),
        None => head_commit(&top),
    };
    let tree = match &commit {
        Some(c) => match &cache.tree {
            Some((have, tree)) if have == c => tree.clone(),
            _ => {
                let tree = Arc::new(read_tree(&top, c)?);
                cache.tree = Some((c.clone(), tree.clone()));
                tree
            }
        },
        None => Arc::default(),
    };
    let mut work = walk(&top)?;
    // Tracked files the walk skipped (ignored, yet committed) still count.
    for rel in tree.keys() {
        if !work.contains_key(rel)
            && let Some(s) = stat(&top.join(rel))
        {
            work.insert(rel.clone(), s);
        }
    }
    let mut paths: HashSet<&String> = work.keys().collect();
    paths.extend(tree.keys());
    if let Some(s) = &snapshot {
        paths.extend(s.files.keys());
    }
    let src_of = |rel: &str| -> Option<Src> {
        if let Some(s) = &snapshot
            && let Some(entry) = s.files.get(rel)
        {
            return entry.clone().map(Src::Bytes);
        }
        tree.get(rel).map(|id| Src::Blob(id.clone()))
    };

    // Reuse what's known; hash the rest that could be unchanged.
    let mut found: Vec<FileChange> = Vec::new();
    let mut todo: Vec<(String, Option<Stat>, Option<Src>)> = Vec::new();
    let mut seen = HashMap::with_capacity(paths.len());
    for rel in paths {
        let st = work.get(rel).copied();
        let src = src_of(rel);
        if st.is_none() && src.is_none() {
            continue;
        }
        let key = src.as_ref().map_or(0, Src::key);
        match cache.seen.remove(rel) {
            Some((s, k, result)) if s == st && k == key && st.is_none_or(|(_, m)| m.is_some()) => {
                found.extend(result.clone());
                seen.insert(rel.clone(), (st, key, result));
            }
            _ => todo.push((rel.clone(), st, src)),
        }
    }
    let to_hash: Vec<&str> = todo.iter().filter(|(r, st, src)| st.is_some() && matches!(src, Some(Src::Blob(_))) && !r.contains('\n')).map(|(r, _, _)| r.as_str()).collect();
    let ids = hash_files(&top, &to_hash);
    let mut need_blobs = Vec::new();
    let mut slow = Vec::new();
    for (rel, st, src) in todo {
        let key = src.as_ref().map_or(0, Src::key);
        if let (Some(Src::Blob(id)), Some(_)) = (&src, st)
            && ids.get(&rel) == Some(id)
        {
            seen.insert(rel, (st, key, None));
            continue;
        }
        if let Some(Src::Blob(id)) = &src {
            need_blobs.push(id.clone());
        }
        slow.push((rel, st, src, key));
    }
    let blobs = read_blobs(&top, &need_blobs);
    for (rel, st, src, key) in slow {
        let path = top.join(&rel);
        let base_bytes = match &src {
            Some(Src::Blob(id)) => blobs.get(id).cloned(),
            Some(Src::Bytes(b)) => Some(b.clone()),
            None => None,
        };
        let work_bytes = st.and_then(|_| read_small(&path));
        let change = match (&src, st) {
            (None, Some(_)) => {
                let w = work_bytes.as_deref();
                Some(FileChange {
                    path,
                    rel: rel.clone(),
                    old: None,
                    status: Status::Added,
                    lines: w.and_then(|w| count_lines(b"", w)),
                    binary: w.is_some_and(is_binary),
                    hash: w.map_or(1, fingerprint),
                    kept: false,
                })
            }
            (Some(_), None) => {
                let b = base_bytes.as_deref().map(Vec::as_slice);
                Some(FileChange {
                    path,
                    rel: rel.clone(),
                    old: None,
                    status: Status::Deleted,
                    lines: b.and_then(|b| count_lines(b, b"")),
                    binary: b.is_some_and(is_binary),
                    hash: b.map_or(1, fingerprint),
                    kept: false,
                })
            }
            (Some(_), Some(_)) => match (base_bytes.as_deref().map(Vec::as_slice), work_bytes.as_deref()) {
                (Some(b), Some(w)) if same_content(b, w) => None,
                (b, w) => Some(FileChange {
                    path,
                    rel: rel.clone(),
                    old: None,
                    status: Status::Modified,
                    lines: b.zip(w).and_then(|(b, w)| count_lines(b, w)),
                    binary: b.is_some_and(is_binary) || w.is_some_and(is_binary),
                    hash: w.map_or(1, fingerprint),
                    kept: false,
                }),
            },
            (None, None) => None,
        };
        found.extend(change.clone());
        seen.insert(rel, (st, key, change));
    }
    cache.seen = seen;

    pair_renames(&mut found);
    // Changes the user kept: a file that now reads exactly as kept is done.
    for f in &mut found {
        if let Some(text) = kept.get(&f.path) {
            f.kept = match read_small(&f.path) {
                Some(bytes) => *crate::encoding::decode_detect(&bytes).0 == ***text,
                None => f.status == Status::Deleted && text.is_empty(),
            };
        }
    }
    found.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(Scan { top, commit, files: found, took: started.elapsed() })
}

/// A deleted file and a new one with the same bytes are one renamed file.
fn pair_renames(found: &mut Vec<FileChange>) {
    let mut gone: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, f) in found.iter().enumerate() {
        if f.status == Status::Deleted && f.hash != 1 {
            gone.entry(f.hash).or_default().push(i);
        }
    }
    let mut drop = HashSet::new();
    for i in 0..found.len() {
        if found[i].status != Status::Added {
            continue;
        }
        let Some(j) = gone.get_mut(&found[i].hash).and_then(Vec::pop) else { continue };
        let old = (found[j].path.clone(), found[j].rel.clone());
        let f = &mut found[i];
        f.status = Status::Renamed;
        f.old = Some(old);
        f.lines = (!f.binary).then_some((0, 0));
        drop.insert(j);
    }
    let mut k = 0;
    found.retain(|_| {
        k += 1;
        !drop.contains(&(k - 1))
    });
}

/// Start Review: HEAD now, plus a copy of every file that differs from it.
fn take_snapshot(folder: &Path, cache: &mut Cache) -> Result<Snapshot, String> {
    let scan = scan(folder, &Base::Head, &HashMap::new(), cache)?;
    let mut files = HashMap::new();
    let mut total = 0;
    for f in &scan.files {
        if let Some((_, old)) = &f.old {
            files.insert(old.clone(), None);
        }
        if f.status == Status::Deleted {
            files.insert(f.rel.clone(), None);
            continue;
        }
        let bytes = std::fs::read(&f.path).map_err(|e| format!("Could not read {}: {e}", f.rel))?;
        total += bytes.len();
        if total > MAX_SNAPSHOT_BYTES {
            return Err(format!("Over {} MB of files already changed: commit or stash them first, or review against the last commit", MAX_SNAPSHOT_BYTES >> 20));
        }
        files.insert(f.rel.clone(), Some(Arc::new(bytes)));
    }
    Ok(Snapshot { top: scan.top, commit: scan.commit, files })
}

/// The base's bytes for `rel` (None: the file isn't in the base). `commit`
/// is the one the list was made against.
pub fn base_bytes(top: &Path, base: &Base, commit: Option<&str>, rel: &str) -> Option<Vec<u8>> {
    if let Base::Snapshot(s) = base
        && let Some(entry) = s.files.get(rel)
    {
        return entry.as_ref().map(|b| b.to_vec());
    }
    let commit = commit?;
    let out = crate::git::git_io(top, &["cat-file", "blob", &format!("{commit}:{rel}")], None, MAX_READ_BYTES as usize, BATCH_TIMEOUT)?;
    (out.len() as u64 <= MAX_READ_BYTES).then_some(out)
}

// ---------------------------------------------------------------------------
// Background scanner
// ---------------------------------------------------------------------------

struct Job {
    folder: PathBuf,
    base: Base,
    kept: HashMap<PathBuf, Arc<String>>,
    generation: u64,
}

#[derive(Default)]
struct State {
    /// Only the newest scan request matters.
    scan: Option<Job>,
    snapshot: Option<PathBuf>,
    quit: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

/// Scans on one worker thread; the UI asks and polls, never waits.
pub struct Scanner {
    shared: Arc<Shared>,
    rx: Receiver<Outcome>,
}

impl Scanner {
    /// `ctx` gets a repaint request whenever a result is ready.
    pub fn new(ctx: Option<egui::Context>) -> Self {
        let shared = Arc::new(Shared { state: Mutex::default(), wake: Condvar::new() });
        let (tx, rx) = channel();
        let worker = shared.clone();
        std::thread::Builder::new().name("review".into()).spawn(move || work(&worker, &tx, ctx.as_ref())).expect("spawn review thread");
        Self { shared, rx }
    }

    /// Queue a scan of `folder`'s repository against `base` (replacing any
    /// queued one); its result carries `generation`.
    pub fn scan(&self, folder: &Path, base: Base, kept: HashMap<PathBuf, Arc<String>>, generation: u64) {
        let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        st.scan = Some(Job { folder: folder.to_owned(), base, kept, generation });
        self.shared.wake.notify_one();
    }

    /// Queue Start Review's snapshot of `folder`'s repository.
    pub fn snapshot(&self, folder: &Path) {
        let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        st.snapshot = Some(folder.to_owned());
        self.shared.wake.notify_one();
    }

    pub fn poll(&self) -> Option<Outcome> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner()).quit = true;
        self.shared.wake.notify_one();
    }
}

fn work(shared: &Shared, tx: &Sender<Outcome>, ctx: Option<&egui::Context>) {
    let mut cache = Cache::default();
    let mut top: Option<PathBuf> = None;
    loop {
        let (snap, job) = {
            let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while st.scan.is_none() && st.snapshot.is_none() && !st.quit {
                st = shared.wake.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            if st.quit {
                return;
            }
            // A snapshot first: a scan queued with it may depend on it.
            match st.snapshot.take() {
                Some(s) => (Some(s), None),
                None => (None, st.scan.take()),
            }
        };
        let outcome = if let Some(folder) = snap {
            Outcome::Snapshot(take_snapshot(&folder, &mut cache))
        } else if let Some(job) = job {
            let result = scan(&job.folder, &job.base, &job.kept, &mut cache);
            // Another repository: what was remembered is no use.
            if let Ok(s) = &result
                && top.as_ref() != Some(&s.top)
            {
                if top.is_some() {
                    cache = Cache::default();
                }
                top = Some(s.top.clone());
            }
            Outcome::Scanned(job.generation, result)
        } else {
            continue;
        };
        if tx.send(outcome).is_err() {
            return;
        }
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Apply `revert_edit` for every hunk, last first (as the UI would one at a time).
    fn revert_all(base: &str, cur: &str) -> String {
        let hunks = diff::diff_lines(&crate::compare::buffer_lines(base), &crate::compare::buffer_lines(cur), false);
        let mut text = cur.to_owned();
        for h in hunks.iter().rev() {
            let (r, with) = revert_edit(base, cur, h);
            let start: usize = text.char_indices().nth(r.start).map_or(text.len(), |(b, _)| b);
            let end: usize = text.char_indices().nth(r.end).map_or(text.len(), |(b, _)| b);
            text.replace_range(start..end, &with);
        }
        text
    }

    fn keep_all(base: &str, cur: &str) -> String {
        let hunks = diff::diff_lines(&crate::compare::buffer_lines(base), &crate::compare::buffer_lines(cur), false);
        let mut b = base.to_owned();
        // Keep from the last hunk so earlier line numbers in `b` stay valid.
        for h in hunks.iter().rev() {
            b = keep_edit(&b, cur, h);
        }
        b
    }

    const CASES: &[(&str, &str)] = &[
        ("a\nb\nc\n", "a\nB\nc\n"),
        ("a\nb\nc\n", "a\nc\n"),
        ("a\nc\n", "a\nb\nc\n"),
        ("a\nb\n", "a\nb"),
        ("a\nb", "a\nb\n"),
        ("a\nb", "a\nc"),
        ("", "new\nfile\n"),
        ("old\nfile\n", ""),
        ("x", "y"),
        ("one\r\ntwo\r\nthree\r\n", "one\r\nTWO\r\nthree\r\nfour\r\n"),
        ("one\r\ntwo\r\n", "zero\r\none\r\n"),
        ("one\r\ntwo\r\nthree", "one\r\nthree\r\n"),
        ("mac\rold\rlines\r", "mac\rnew\rlines\r"),
        ("é\nü\nß\n", "é\nÜ\nß\n日本\n"),
        ("1\n2\n3\n4\n5\n6\n7\n8\n", "0\n1\n3\n4\nfive\n6\n7\n7.5\n8"),
        ("}\n}\n}\n", "}\nx\n}\n}\n"),
    ];

    #[test]
    fn reverting_every_hunk_gives_back_the_base_exactly() {
        for (base, cur) in CASES {
            assert_eq!(revert_all(base, cur), *base, "{base:?} ← {cur:?}");
        }
    }

    #[test]
    fn keeping_every_hunk_gives_the_current_text() {
        for (base, cur) in CASES {
            assert_eq!(keep_all(base, cur), *cur, "{base:?} → {cur:?}");
        }
    }

    #[test]
    fn one_hunk_reverts_alone_and_keeps_crlf() {
        let base = "one\r\ntwo\r\nthree\r\nfour\r\n";
        let cur = "ONE\r\ntwo\r\nthree\r\nFOUR\r\nfive\r\n";
        let hunks = diff::diff_lines(&crate::compare::buffer_lines(base), &crate::compare::buffer_lines(cur), false);
        assert_eq!(hunks.len(), 2);
        let (r, with) = revert_edit(base, cur, &hunks[1]);
        assert_eq!(with, "four\r\n");
        assert_eq!(&cur[r.clone()], "FOUR\r\nfive\r\n");
        let mut text = cur.to_owned();
        text.replace_range(r, &with);
        assert_eq!(text, "ONE\r\ntwo\r\nthree\r\nfour\r\n");
        // Keeping the first leaves only the second to review.
        let kept = keep_edit(base, cur, &hunks[0]);
        assert_eq!(kept, "ONE\r\ntwo\r\nthree\r\nfour\r\n");
    }

    #[test]
    fn checked_out_crlf() {
        assert_eq!(as_checked_out("a\nb\n".into(), true), "a\r\nb\r\n");
        assert_eq!(as_checked_out("a\nb\n".into(), false), "a\nb\n");
        assert_eq!(as_checked_out("a\r\nb\n".into(), true), "a\r\nb\n");
        assert!(same_content(b"a\nb\n", b"a\r\nb\r\n"));
        assert!(same_content(b"a\nb", b"a\r\nb"));
        assert!(!same_content(b"a\nb\n", b"a\r\nb\n"));
        assert!(!same_content(b"a\r\nb\r\n", b"a\nb\n"));
        assert!(!same_content(b"a\nb\n", b"a\r\nc\r\n"));
    }

    #[test]
    fn line_counts() {
        assert_eq!(count_lines(b"a\nb\n", b"a\nB\nc\n"), Some((2, 1)));
        assert_eq!(count_lines(b"", b"x\ny\n"), Some((2, 0)));
        assert_eq!(count_lines(b"x\ny\n", b""), Some((0, 2)));
        assert_eq!(count_lines(b"x\0y", b"x"), None);
    }

    // ---- against real repositories ----

    pub(crate) fn run(dir: &Path, args: &[&str]) {
        let mut all = vec!["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"];
        all.extend_from_slice(args);
        assert!(crate::git::git_io(dir, &all, None, 1 << 20, Duration::from_secs(120)).is_some(), "git {args:?}");
    }

    /// A repository with a few committed files; None without git.
    pub(crate) fn temp_repo(tag: &str) -> Option<PathBuf> {
        let dir = std::env::temp_dir().join(format!("strata-review-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).ok()?;
        crate::git::git(&dir, &["init", "-q", "-b", "main"])?;
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    one();\n    two();\n}\n").unwrap();
        std::fs::write(dir.join("README.md"), "# Title\n\nText.\n").unwrap();
        std::fs::write(dir.join("crlf.txt"), "a\r\nb\r\nc\r\n").unwrap();
        std::fs::write(dir.join("gone.txt"), "bye\n").unwrap();
        std::fs::write(dir.join("moved.txt"), "same content\nhere\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        run(&dir, &["add", "."]);
        run(&dir, &["commit", "-q", "-m", "init"]);
        Some(dir)
    }

    type Listed = Vec<(String, Status, Option<(usize, usize)>)>;

    fn scan_of(dir: &Path, base: &Base, cache: &mut Cache) -> Listed {
        let s = scan(dir, base, &HashMap::new(), cache).unwrap();
        s.files.iter().map(|f| (f.rel.clone(), f.status, f.lines)).collect()
    }

    #[test]
    fn statuses_against_head() {
        let Some(dir) = temp_repo("status") else { return };
        let mut cache = Cache::default();
        assert_eq!(scan_of(&dir, &Base::Head, &mut cache), vec![], "a clean tree");
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    one();\n    three();\n}\n").unwrap();
        std::fs::write(dir.join("src/new.rs"), "pub fn x() {}\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::rename(dir.join("moved.txt"), dir.join("src/moved.txt")).unwrap();
        std::fs::write(dir.join("crlf.txt"), "a\r\nB\r\nc\r\n").unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target/out.o"), "ignored").unwrap();
        let got = scan_of(&dir, &Base::Head, &mut cache);
        assert_eq!(
            got,
            vec![
                ("crlf.txt".into(), Status::Modified, Some((1, 1))),
                ("gone.txt".into(), Status::Deleted, Some((0, 1))),
                ("src/main.rs".into(), Status::Modified, Some((1, 1))),
                ("src/moved.txt".into(), Status::Renamed, Some((0, 0))),
                ("src/new.rs".into(), Status::Added, Some((1, 0))),
            ]
        );
        let s = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        let moved = s.files.iter().find(|f| f.status == Status::Renamed).unwrap();
        assert_eq!(moved.old.as_ref().unwrap().1, "moved.txt");
        assert_eq!(moved.path, dir.join("src/moved.txt"));
        // A file put back exactly as committed drops out; the cache notices
        // the new size and time.
        std::fs::write(dir.join("crlf.txt"), "a\r\nb\r\nc\r\n").unwrap();
        let got = scan_of(&dir, &Base::Head, &mut cache);
        assert!(!got.iter().any(|f| f.0 == "crlf.txt"), "{got:?}");
        // Committing moves the HEAD base.
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "agent"]);
        assert_eq!(scan_of(&dir, &Base::Head, &mut cache), vec![]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checked_out_crlf_is_not_a_change() {
        let Some(dir) = temp_repo("autocrlf") else { return };
        // As core.autocrlf would write it: the blob has \n, the file \r\n.
        std::fs::write(dir.join("README.md"), "# Title\r\n\r\nText.\r\n").unwrap();
        let mut cache = Cache::default();
        assert_eq!(scan_of(&dir, &Base::Head, &mut cache), vec![]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_base_shows_only_later_changes() {
        let Some(dir) = temp_repo("snapshot") else { return };
        // Already changed before the review starts.
        std::fs::write(dir.join("README.md"), "# Title\n\nMy own edit.\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "mine\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        let mut cache = Cache::default();
        let snap = Arc::new(take_snapshot(&dir, &mut cache).unwrap());
        assert_eq!(snap.files.len(), 3);
        assert_eq!(snap.files["gone.txt"], None);
        assert_eq!(snap.files["notes.txt"].as_deref().map(|b| b.as_slice()), Some(&b"mine\n"[..]));
        let base = Base::Snapshot(snap.clone());
        assert_eq!(scan_of(&dir, &base, &mut cache), vec![], "nothing since the snapshot");

        // The agent edits a file I'd changed, one I hadn't, adds one, commits.
        std::fs::write(dir.join("README.md"), "# Title\n\nMy own edit.\nAgent line.\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    one();\n}\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        std::fs::write(dir.join("gone.txt"), "back\n").unwrap();
        std::fs::write(dir.join("agent.txt"), "hi\n").unwrap();
        run(&dir, &["add", "src/main.rs"]);
        run(&dir, &["commit", "-q", "-m", "agent commit"]);
        let got = scan_of(&dir, &base, &mut cache);
        assert_eq!(
            got,
            vec![
                ("README.md".into(), Status::Modified, Some((1, 0))),
                ("agent.txt".into(), Status::Added, Some((1, 0))),
                ("gone.txt".into(), Status::Added, Some((1, 0))),
                ("notes.txt".into(), Status::Modified, Some((0, 1))),
                ("src/main.rs".into(), Status::Modified, Some((0, 1))),
            ]
        );
        // The base text for the hunk view.
        let top = scan(&dir, &base, &HashMap::new(), &mut cache).unwrap().top;
        assert_eq!(base_bytes(&top, &base, snap.commit.as_deref(), "README.md").unwrap(), b"# Title\n\nMy own edit.\n");
        assert_eq!(base_bytes(&top, &base, snap.commit.as_deref(), "src/main.rs").unwrap(), b"fn main() {\n    one();\n    two();\n}\n");
        assert_eq!(base_bytes(&top, &base, snap.commit.as_deref(), "gone.txt"), None);
        assert_eq!(base_bytes(&top, &base, snap.commit.as_deref(), "agent.txt"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kept_text_marks_a_file_done() {
        let Some(dir) = temp_repo("kept") else { return };
        std::fs::write(dir.join("README.md"), "# Title\n\nNew text.\n").unwrap();
        let mut cache = Cache::default();
        let mut kept = HashMap::new();
        kept.insert(dir.join("README.md"), Arc::new("# Title\n\nNew text.\n".to_owned()));
        let s = scan(&dir, &Base::Head, &kept, &mut cache).unwrap();
        assert!(s.files[0].kept);
        std::fs::write(dir.join("README.md"), "# Title\n\nNewer text.\n").unwrap();
        let s = scan(&dir, &Base::Head, &kept, &mut cache).unwrap();
        assert!(!s.files[0].kept, "changed again after keeping");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn many_changed_files_scan_quickly_and_rescan_from_the_cache() {
        let Some(dir) = temp_repo("many") else { return };
        std::fs::create_dir_all(dir.join("generation")).unwrap();
        for i in 0..2000 {
            std::fs::write(dir.join(format!("generation/f{i}.txt")), format!("line {i}\n")).unwrap();
        }
        let mut cache = Cache::default();
        let s = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert_eq!(s.files.len(), 2000);
        let t = Instant::now();
        let again = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert_eq!(again.files.len(), 2000);
        assert!(t.elapsed() < Duration::from_secs(5), "rescan took {:?}", t.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outside_a_repository() {
        let dir = std::env::temp_dir().join(format!("strata-review-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // The temp folder might itself be inside a repository; only check it doesn't hang or panic.
        let _ = scan(&dir, &Base::Head, &HashMap::new(), &mut Cache::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scanner_thread_answers() {
        let Some(dir) = temp_repo("thread") else { return };
        std::fs::write(dir.join("README.md"), "changed\n").unwrap();
        let s = Scanner::new(None);
        s.scan(&dir, Base::Head, HashMap::new(), 7);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(Outcome::Scanned(generation, r)) = s.poll() {
                assert_eq!(generation, 7);
                assert_eq!(r.unwrap().files.len(), 1);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
