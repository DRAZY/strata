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
use crate::encoding::Encoding;
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
/// Bytes of base blobs one scan reads at most (see `Cache::blob_budget`).
const BLOB_BUDGET: usize = 512 << 20;

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
    /// Why Strata can't show or put back this file, when it can't (a Git
    /// filter, a link): it's listed, without Keep or Revert.
    pub blocked: Option<String>,
    /// On disk, but Git filters it, so it can't be compared (see `Scan::filtered`).
    pub filtered: bool,
    /// Git checks this file out with `\r\n` where the base has `\n`
    /// (`core.autocrlf`, `eol=crlf`): the base reads that way too.
    pub crlf: bool,
    /// A rename to another spelling of the same file on this disk (letter
    /// case, or the Unicode form of an accent, on macOS and Windows): one
    /// file, never two to delete or restore separately.
    pub alias: bool,
    /// Fingerprint of the file's bytes (of the base's, for a deleted file;
    /// of its size and times, for one too big to read).
    pub hash: u64,
    /// Every change in it was kept: it matches the text kept for it.
    pub kept: bool,
    /// What the file looked like on disk when listed (None: not there).
    /// Trashing it checks it still does.
    pub stat: Option<Stat>,
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
    /// Files on disk that Git filters (LFS and the like), not compared.
    pub filtered: usize,
    /// The base commit's files (relative path → blob id).
    pub tracked: Arc<HashMap<String, String>>,
    pub took: Duration,
}

pub enum Outcome {
    /// A scan for request `generation`.
    Scanned(u64, Result<Scan, String>),
    Snapshot(Result<Snapshot, String>),
    /// A review tab's base: its file, and the bytes (None: not in the base).
    Base(PathBuf, Option<Vec<u8>>),
    /// A file moved to the Trash, or why not.
    Trashed(PathBuf, Result<(), TrashRefused>),
}

/// Why a file wasn't moved to the Trash.
#[derive(Debug, PartialEq)]
pub enum TrashRefused {
    /// It changed (or was replaced) since it was listed.
    Changed,
    /// It isn't really inside the repository (a link on the way).
    Outside,
    Failed(String),
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

/// Line `i`'s text without its break.
fn line_text<'a>(s: &'a str, starts: &[(usize, usize)], i: usize) -> &'a str {
    let (start, end) = (starts[i].0, starts[i + 1].0);
    let line = &s[start..end];
    line.strip_suffix("\r\n").or_else(|| line.strip_suffix('\n')).or_else(|| line.strip_suffix('\r')).unwrap_or(line)
}

/// The edit that makes lines `dl` of `dst` read as lines `sl` of `src`:
/// the (byte, char) ranges of `dst` to replace and the text from `src`.
fn splice(dst: &str, src: &str, mut dl: Range<usize>, mut sl: Range<usize>) -> (Range<usize>, Range<usize>, String) {
    let (sd, ss) = (line_starts(dst), line_starts(src));
    let (nd, ns) = (sd.len() - 1, ss.len() - 1);
    loop {
        // A block running to the end of both texts swaps the line break before
        // it too (when both sides have one), so the final newline goes with it.
        let to_end = dl.end >= nd && sl.end >= ns && dl.start > 0 && sl.start > 0;
        let (db, dc) = span(dst, &sd, &dl, to_end);
        let (sb, _) = span(src, &ss, &sl, to_end);
        let with = &src[sb];
        // A lone \r set down before a \n (or a \n after a \r) would join the
        // two into one line break and lose a line: take in the neighboring
        // line too, when it reads the same on both sides.
        let (d, w) = (dst.as_bytes(), with.as_bytes());
        let left_of_end = w.last().or_else(|| db.start.checked_sub(1).map(|k| &d[k]));
        let after = left_of_end == Some(&b'\r') && d.get(db.end) == Some(&b'\n');
        let right_of_start = w.first().or_else(|| d.get(db.end));
        let before = db.start > 0 && d[db.start - 1] == b'\r' && right_of_start == Some(&b'\n');
        if after && dl.end < nd && sl.end < ns && (dl.end + 1 == nd) == (sl.end + 1 == ns) && line_text(dst, &sd, dl.end) == line_text(src, &ss, sl.end) {
            (dl.end, sl.end) = (dl.end + 1, sl.end + 1);
        } else if before && dl.start > 0 && sl.start > 0 && line_text(dst, &sd, dl.start - 1) == line_text(src, &ss, sl.start - 1) {
            (dl.start, sl.start) = (dl.start - 1, sl.start - 1);
        } else {
            // No neighbor to take in (the start or end of a text): write that
            // one break as \r\n instead, which keeps the lines apart.
            let mut with = with.to_owned();
            if before && with.starts_with('\n') {
                with.insert(0, '\r');
            }
            if after && with.ends_with('\r') {
                with.push('\n');
            }
            return (db, dc, with);
        }
    }
}

/// The edit that turns lines `h.b` of `cur` back into lines `h.a` of `base`:
/// a char range of `cur` and its replacement, exactly the base's text with
/// its own line breaks. `h` must come from diffing these two texts.
pub fn revert_edit(base: &str, cur: &str, h: &Hunk) -> (Range<usize>, String) {
    let (_, chars, with) = splice(cur, base, h.b.clone(), h.a.clone());
    (chars, with)
}

/// The base with hunk `h` kept: lines `h.a` of `base` replaced by lines
/// `h.b` of `cur`, so that change no longer shows.
pub fn keep_edit(base: &str, cur: &str, h: &Hunk) -> String {
    let (bytes, _, with) = splice(base, cur, h.a.clone(), h.b.clone());
    let mut out = String::with_capacity(base.len() + with.len());
    out.push_str(&base[..bytes.start]);
    out.push_str(&with);
    out.push_str(&base[bytes.end..]);
    out
}

/// The base as Git would check it out: `\n` written as `\r\n` when the
/// repository says so for this file (see `FileChange::crlf`).
pub fn as_checked_out(base: String, crlf: bool) -> String {
    if crlf && !base.contains('\r') && base.contains('\n') { base.replace('\n', "\r\n") } else { base }
}

/// The base's bytes as text for a tab that reads its file as `doc`: in the
/// tab's own encoding when both are legacy text and the base reads cleanly
/// in it, else in the encoding the base's bytes show (a BOM, UTF-16, UTF-8,
/// or a guess among the legacy ones). Err when they don't decode cleanly:
/// putting replacement characters back would corrupt the file.
pub fn decode_base(bytes: &[u8], doc: Encoding) -> Result<(String, Encoding), String> {
    if bytes.is_empty() {
        return Ok((String::new(), doc));
    }
    let d = crate::encoding::detect(bytes);
    if d.guess.is_some() && !doc.is_unicode() {
        let (text, lossy) = crate::encoding::decode(bytes, doc);
        if !lossy {
            return Ok((text.into_owned(), doc));
        }
    }
    if d.lossy {
        return Err("The earlier version isn't clean text in any encoding Strata knows, so it can't be shown or put back".into());
    }
    Ok((d.text.into_owned(), d.encoding))
}

// ---------------------------------------------------------------------------
// Comparing files
// ---------------------------------------------------------------------------

fn fingerprint(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Git's test for binary, a NUL byte near the start, except that UTF-16
/// text (which is full of NULs) counts as text.
fn is_binary(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(8000)];
    head.contains(&0) && !crate::encoding::detect(&bytes[..bytes.len().min(1 << 16) & !1]).encoding.is_utf16()
}

/// Same bytes, or (when Git checks this file out with `\r\n`) the same text
/// with each `\n` written as `\r\n`.
fn same_content(base: &[u8], work: &[u8], crlf: bool) -> bool {
    if base == work {
        return true;
    }
    if !crlf || base.contains(&b'\r') || work.len() <= base.len() {
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
    let meta = std::fs::symlink_metadata(path).ok().filter(|m| m.is_file())?;
    if meta.len() > MAX_READ_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

/// What a file looked like on disk. `change` folds in the inode and the
/// status-change time where there are such things, so a rewrite of the same
/// size within one modification-time tick still shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Stat {
    len: u64,
    mtime: Option<SystemTime>,
    change: u64,
    /// Which file it is (device and inode), where the system says; 0 elsewhere.
    id: (u64, u64),
}

fn stat_of(m: &std::fs::Metadata) -> Stat {
    #[cfg(unix)]
    let change = {
        use std::os::unix::fs::MetadataExt;
        m.ino() ^ (m.ctime() as u64).rotate_left(32) ^ m.ctime_nsec() as u64
    };
    #[cfg(unix)]
    let id = {
        use std::os::unix::fs::MetadataExt;
        (m.dev(), m.ino())
    };
    #[cfg(not(unix))]
    let (change, id) = (0, (0, 0));
    Stat { len: m.len(), mtime: m.modified().ok(), change, id }
}

/// A regular file's stat (not a link, not a folder).
fn stat(path: &Path) -> Option<Stat> {
    std::fs::symlink_metadata(path).ok().filter(|m| m.is_file()).map(|m| stat_of(&m))
}

/// Fingerprint of a file: of its bytes, or of its stat for one too big to read.
fn hash_of(bytes: Option<&[u8]>, st: Option<Stat>) -> u64 {
    match (bytes, st) {
        (Some(b), _) => fingerprint(b),
        (None, Some(s)) => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        }
        (None, None) => 1,
    }
}

/// The fingerprint a scan gives the file at `path` as it is now (see `FileChange::hash`).
pub fn file_hash(path: &Path) -> u64 {
    hash_of(read_small(path).as_deref(), stat(path))
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

/// A commit, its files and the paths of those with non-UTF-8 names (see `read_tree`).
type CachedTree = (String, Arc<HashMap<String, String>>, Arc<Vec<PathBuf>>);

/// Remembered between scans of one repository.
struct Cache {
    /// The repository this is about: anything else starts afresh.
    top: Option<PathBuf>,
    /// The base commit's files: relative path → blob id.
    tree: Option<CachedTree>,
    /// Per file: its stat and base when last looked at, and what was found
    /// (None: same as the base).
    seen: HashMap<String, (Option<Stat>, u64, Option<FileChange>)>,
    /// Bytes of base blobs one scan may read; past it, files are listed
    /// without line counts (and read when opened).
    blob_budget: usize,
    /// Another system config file for the line-ending settings (tests).
    system_config: Option<PathBuf>,
    /// Files hashed by the last scan (tests check rescans skip unchanged ones).
    hashed: usize,
}

impl Default for Cache {
    fn default() -> Self {
        Self { top: None, tree: None, seen: HashMap::new(), blob_budget: BLOB_BUDGET, system_config: None, hashed: 0 }
    }
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

/// A commit's files: relative path → blob id (symlinks and submodules left
/// out), and the paths of those whose names aren't valid UTF-8.
type Tree = (HashMap<String, String>, Vec<PathBuf>);

fn read_tree(top: &Path, commit: &str) -> Result<Tree, String> {
    let out = crate::git::git_io(top, &["ls-tree", "-r", "-z", "--full-tree", commit], None, BATCH_BYTES, BATCH_TIMEOUT).ok_or("Could not list the files in the last commit")?;
    if out.len() > BATCH_BYTES {
        return Err("The repository has too many files to review".into());
    }
    let (mut files, mut bad) = (HashMap::new(), Vec::new());
    for rec in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        // "<mode> <type> <id>\t<path>"
        let Some(tab) = rec.iter().position(|&b| b == b'\t') else { continue };
        let head = String::from_utf8_lossy(&rec[..tab]);
        let mut parts = head.split(' ');
        let (Some(mode), Some(kind), Some(id)) = (parts.next(), parts.next(), parts.next()) else { continue };
        if kind != "blob" || mode == "120000" {
            continue;
        }
        // Names are bytes: one that isn't UTF-8 never gets a lossy key.
        match std::str::from_utf8(&rec[tab + 1..]) {
            Ok(path) => {
                files.insert(path.to_owned(), id.to_owned());
            }
            Err(_) => bad.push(path_from_bytes(top, &rec[tab + 1..])),
        }
    }
    Ok((files, bad))
}

/// Blob contents by id (`git cat-file --batch`); blobs that can't be read
/// (too big) are left out.
fn read_blobs(top: &Path, ids: &[String]) -> HashMap<String, Arc<Vec<u8>>> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return out;
    }
    let input = ids.iter().flat_map(|id| [id.as_bytes(), b"\n"]).flatten().copied().collect();
    let got = crate::git::git_io(top, &["cat-file", "--batch"], Some(input), BATCH_BYTES, BATCH_TIMEOUT).filter(|o| o.len() <= BATCH_BYTES);
    match got {
        Some(bytes) => parse_batch(&bytes, &mut out),
        // Too much at once: one at a time, skipping the huge ones.
        None => {
            for id in ids {
                if let Some(b) = crate::git::git_io(top, &["cat-file", "blob", id], None, MAX_READ_BYTES as usize, BATCH_TIMEOUT).filter(|b| b.len() as u64 <= MAX_READ_BYTES) {
                    out.insert(id.clone(), Arc::new(b));
                }
            }
        }
    }
    out
}

/// Blob sizes by id (`git cat-file --batch-check`: no contents read).
fn blob_sizes(top: &Path, ids: &[String]) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return out;
    }
    let input = ids.iter().flat_map(|id| [id.as_bytes(), b"\n"]).flatten().copied().collect();
    let Some(raw) = crate::git::git_io(top, &["cat-file", "--batch-check"], Some(input), BATCH_BYTES, BATCH_TIMEOUT) else { return out };
    for line in String::from_utf8_lossy(&raw).lines() {
        // "<id> <type> <size>", or "<id> missing"
        let parts: Vec<&str> = line.split(' ').collect();
        if let [id, "blob", size] = parts[..]
            && let Ok(n) = size.parse()
        {
            out.insert(id.to_owned(), n);
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

/// What `.gitattributes` and `core.autocrlf` say about a file.
#[derive(Clone, Debug, Default, PartialEq)]
struct Attrs {
    /// Checked out with `\r\n` for `\n`.
    crlf: bool,
    /// A filter or working-tree encoding: the file on disk isn't the blob's
    /// bytes, and Strata doesn't run filters to find out what it should be.
    filter: Option<String>,
}

/// Attributes for `rels` (`git check-attr`, which only reads the attribute
/// files and config; it runs nothing).
fn attributes(top: &Path, rels: &[&str], system: Option<&Path>) -> HashMap<String, Attrs> {
    let mut out: HashMap<String, Attrs> = HashMap::new();
    if rels.is_empty() {
        return out;
    }
    // As Git sees them, system config included (Git for Windows sets
    // core.autocrlf=true there), in any of Git's spellings of true.
    let config = |key: &str| crate::git::config_value(top, key, system).map(|v| v.to_ascii_lowercase());
    let autocrlf = config("core.autocrlf").is_some_and(|v| matches!(v.as_str(), "true" | "yes" | "on" | "1"));
    let eol_crlf = config("core.eol").as_deref() == Some("crlf");
    for chunk in rels.chunks(BATCH) {
        let input: Vec<u8> = chunk.iter().flat_map(|r| [r.as_bytes(), b"\0"]).flatten().copied().collect();
        let args = ["check-attr", "-z", "--stdin", "eol", "text", "filter", "working-tree-encoding"];
        let Some(raw) = crate::git::git_io(top, &args, Some(input), BATCH_BYTES, BATCH_TIMEOUT) else { continue };
        let fields: Vec<String> = raw.split(|&b| b == 0).map(|f| String::from_utf8_lossy(f).into_owned()).collect();
        let mut values: HashMap<&str, HashMap<&str, &str>> = HashMap::new();
        for [path, attr, value] in fields.as_chunks::<3>().0 {
            values.entry(path.as_str()).or_default().insert(attr.as_str(), value.as_str());
        }
        for (path, v) in values {
            let get = |k: &str| v.get(k).copied().unwrap_or("unspecified");
            let crlf = match get("eol") {
                "crlf" => true,
                "lf" => false,
                _ => match get("text") {
                    "unset" => false,
                    // core.eol applies to files marked as text (or text=auto).
                    "set" | "auto" => autocrlf || eol_crlf,
                    _ => autocrlf,
                },
            };
            let named = |k: &str| (!matches!(get(k), "unspecified" | "unset")).then(|| format!("{k}={}", get(k)));
            out.insert(path.to_owned(), Attrs { crlf, filter: named("filter").or_else(|| named("working-tree-encoding")) });
        }
    }
    out
}

/// The working tree's files: relative path → stat. Respects .gitignore
/// like Git does, and leaves out `.git`, nested repositories and links.
fn walk(top: &Path, bad: &mut Vec<PathBuf>) -> Result<HashMap<String, Stat>, String> {
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
        let Some(rel) = rel_of(top, e.path()) else {
            bad.push(e.into_path());
            continue;
        };
        let Ok(meta) = e.metadata() else { continue };
        files.insert(rel, stat_of(&meta));
        if files.len() > MAX_FILES {
            return Err(format!("Over {MAX_FILES} files: too many to review"));
        }
    }
    Ok(files)
}

/// `path` relative to `top`, with `/`. None for a name that isn't valid
/// UTF-8: a lossy spelling could match another, real file.
fn rel_of(top: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(top).ok()?;
    let parts: Vec<&str> = rel.components().map(|c| c.as_os_str().to_str()).collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// A path from git's bytes, exactly (Unix names are bytes, not text).
fn path_from_bytes(top: &Path, bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        top.join(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    top.join(String::from_utf8_lossy(bytes).as_ref())
}

/// Files whose names aren't valid UTF-8 are listed, never acted on.
const BAD_NAME: &str = "Its name isn't valid UTF-8, so Strata leaves it alone: use Git to look at it or put it back.";

/// Some part of `rel`, from `top` down, is a symbolic link (or, on Windows,
/// a junction): opening the path would reach somewhere else.
fn link_in(top: &Path, rel: &str) -> bool {
    let mut p = top.to_path_buf();
    for part in rel.split('/') {
        p.push(part);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    // Junctions aren't links to `is_symlink`: what exists must resolve inside too.
    let mut existing = top.join(rel);
    while !existing.exists() {
        if !existing.pop() {
            return false;
        }
    }
    match (existing.canonicalize(), top.canonicalize()) {
        (Ok(real), Ok(top)) => !real.starts_with(top),
        _ => true,
    }
}

/// `path` is really inside `top`: below it by name, with no `..` and no link
/// on the way. Review only opens, reverts or trashes such paths.
pub fn inside(top: &Path, path: &Path) -> bool {
    match rel_of(top, path) {
        Some(rel) => !rel.split('/').any(|p| p == ".." || p == ".") && !link_in(top, &rel),
        None => false,
    }
}

/// `a` and `b` are the same file on disk (one file under two spellings, or
/// a hard link): same device and inode where there are such, else the same
/// real path as the system resolves it.
pub fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(x), Ok(y)) = (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
            return (x.dev(), x.ino()) == (y.dev(), y.ino());
        }
        false
    }
    #[cfg(not(unix))]
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// A file in `tracked` (relative paths in the base) that is the same file
/// on disk as `path`, under another spelling. Before trashing a "new" file:
/// if it's really a tracked one, it's the only copy.
pub fn tracked_twin<'a>(top: &Path, path: &Path, tracked: impl Iterator<Item = &'a String>) -> Option<String> {
    let rel = rel_of(top, path)?;
    let parent = path.parent()?;
    let dir = |r: &str| r.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_owned());
    // Tracked files in the same folder, which may be spelled differently too:
    // the folders are compared by identity, once each.
    let mut folders: HashMap<String, bool> = HashMap::new();
    tracked
        .filter(|t| **t != rel)
        .filter(|t| *folders.entry(dir(t)).or_insert_with_key(|d| same_file(&top.join(d), parent)))
        .find(|t| same_file(&top.join(t.as_str()), path))
        .cloned()
}

fn change(path: PathBuf, rel: &str, status: Status) -> FileChange {
    FileChange { path, rel: rel.to_owned(), old: None, status, lines: None, binary: false, blocked: None, filtered: false, crlf: false, alias: false, hash: 1, kept: false, stat: None }
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
    // Another repository: nothing remembered applies.
    if cache.top.as_ref() != Some(&top) {
        *cache = Cache { top: Some(top.clone()), blob_budget: cache.blob_budget, system_config: cache.system_config.take(), ..Cache::default() };
    }
    let commit = match &snapshot {
        Some(s) => s.commit.clone(),
        None => head_commit(&top),
    };
    let (tree, tree_bad) = match &commit {
        Some(c) => match &cache.tree {
            Some((have, tree, bad)) if have == c => (tree.clone(), bad.clone()),
            _ => {
                let (files, bad) = read_tree(&top, c)?;
                let (tree, bad) = (Arc::new(files), Arc::new(bad));
                cache.tree = Some((c.clone(), tree.clone(), bad.clone()));
                (tree, bad)
            }
        },
        None => (Arc::default(), Arc::default()),
    };
    let mut bad_names: Vec<PathBuf> = tree_bad.to_vec();
    let mut work = walk(&top, &mut bad_names)?;
    bad_names.sort();
    bad_names.dedup();
    let mut in_base: Vec<&String> = tree.keys().collect();
    if let Some(s) = &snapshot {
        in_base.extend(s.files.iter().filter(|(_, b)| b.is_some()).map(|(r, _)| r));
    }
    // Files in the base the walk didn't list: a link now (never followed),
    // another spelling of a file the walk did list (letter case, Unicode
    // form: the disk finds the same file under both names), or ignored yet
    // committed (still counts). Which file a name reaches is decided by its
    // identity, not by comparing names.
    let by_id: HashMap<(u64, u64), &String> = work.iter().filter(|(_, s)| s.id != (0, 0)).map(|(r, s)| (s.id, r)).collect();
    let lower: HashMap<String, &String> = work.keys().map(|r| (r.to_lowercase(), r)).collect();
    let mut links = HashSet::new();
    let mut case_pairs = Vec::new();
    let mut extra = Vec::new();
    for rel in in_base {
        if work.contains_key(rel) {
            continue;
        }
        if link_in(&top, rel) {
            links.insert(rel.clone());
            continue;
        }
        let Some(s) = stat(&top.join(rel)) else { continue };
        let other = match by_id.get(&s.id) {
            Some(other) if s.id != (0, 0) => Some((*other).clone()),
            // No file ids here: the same file is what the system resolves both names to.
            _ => lower.get(&rel.to_lowercase()).filter(|o| same_file(&top.join(rel), &top.join(o))).map(|o| (*o).clone()),
        };
        match other {
            Some(other) if other != *rel => case_pairs.push((rel.clone(), other)),
            _ => extra.push((rel.clone(), s)),
        }
    }
    work.extend(extra);
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
    let mut found: Vec<FileChange> = bad_names
        .into_iter()
        .map(|path| {
            let rel = path.strip_prefix(&top).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            let mut f = change(path, &rel, Status::Modified);
            f.blocked = Some(BAD_NAME.into());
            f
        })
        .collect();
    let mut todo: Vec<(String, Option<Stat>, Option<Src>)> = Vec::new();
    let mut seen = HashMap::with_capacity(paths.len());
    for rel in paths {
        if links.contains(rel) {
            let mut f = change(top.join(rel), rel, Status::Modified);
            f.blocked = Some("Replaced by a link to somewhere else: Strata won't follow it. Use Git to look at it or put it back.".into());
            found.push(f);
            continue;
        }
        let st = work.get(rel).copied();
        let src = src_of(rel);
        if st.is_none() && src.is_none() {
            continue;
        }
        let key = src.as_ref().map_or(0, Src::key);
        match cache.seen.remove(rel) {
            Some((s, k, result)) if s == st && k == key && st.is_none_or(|s| s.mtime.is_some()) => {
                found.extend(result.clone());
                seen.insert(rel.clone(), (st, key, result));
            }
            _ => todo.push((rel.clone(), st, src)),
        }
    }
    let to_hash: Vec<&str> = todo.iter().filter(|(r, st, src)| st.is_some() && matches!(src, Some(Src::Blob(_))) && !r.contains('\n')).map(|(r, _, _)| r.as_str()).collect();
    cache.hashed = to_hash.len();
    let ids = hash_files(&top, &to_hash);
    let mut slow = Vec::new();
    for (rel, st, src) in todo {
        let key = src.as_ref().map_or(0, Src::key);
        if let (Some(Src::Blob(id)), Some(_)) = (&src, st)
            && ids.get(&rel) == Some(id)
        {
            seen.insert(rel, (st, key, None));
            continue;
        }
        slow.push((rel, st, src, key));
    }
    let blob_rels: Vec<&str> = slow.iter().filter(|s| matches!(s.2, Some(Src::Blob(_)))).map(|s| s.0.as_str()).collect();
    let attrs = attributes(&top, &blob_rels, cache.system_config.as_deref());
    // Base blobs a chunk at a time, within the scan's budget.
    let mut budget = cache.blob_budget;
    for chunk in slow.chunks(BATCH) {
        let attr = |rel: &str| attrs.get(rel).cloned().unwrap_or_default();
        let want: Vec<String> = chunk
            .iter()
            .filter_map(|(rel, _, src, _)| match src {
                Some(Src::Blob(id)) if attr(rel).filter.is_none() => Some(id.clone()),
                _ => None,
            })
            .collect();
        // Sizes first (cheap), then only the blobs that fit what's left.
        let sizes = blob_sizes(&top, &want);
        let want: Vec<String> = want
            .into_iter()
            .filter(|id| match sizes.get(id) {
                Some(&n) if n <= budget && n as u64 <= MAX_READ_BYTES => {
                    budget -= n;
                    true
                }
                _ => false,
            })
            .collect();
        let blobs = read_blobs(&top, &want);
        for (rel, st, src, key) in chunk {
            let path = top.join(rel);
            let a = attr(rel);
            let base_bytes = match src {
                Some(Src::Blob(id)) => blobs.get(id).cloned(),
                Some(Src::Bytes(b)) => Some(b.clone()),
                None => None,
            };
            let blob_filter = a.filter.clone().filter(|_| matches!(src, Some(Src::Blob(_))));
            let work_bytes = match (st, &blob_filter) {
                (Some(_), None) => read_small(&path),
                _ => None,
            };
            let w = work_bytes.as_deref();
            let b = base_bytes.as_deref().map(Vec::as_slice);
            let mut f = match (src, st) {
                (None, Some(_)) => {
                    let mut f = change(path, rel, Status::Added);
                    f.lines = w.and_then(|w| count_lines(b"", w));
                    f.binary = w.is_some_and(is_binary);
                    f.hash = hash_of(w, *st);
                    Some(f)
                }
                (Some(_), None) => {
                    let mut f = change(path, rel, Status::Deleted);
                    f.lines = b.and_then(|b| count_lines(b, b""));
                    f.binary = b.is_some_and(is_binary);
                    f.hash = b.map_or(1, fingerprint);
                    Some(f)
                }
                (Some(_), Some(_)) if blob_filter.is_some() => {
                    let mut f = change(path, rel, Status::Modified);
                    f.filtered = true;
                    f.hash = hash_of(None, *st);
                    Some(f)
                }
                (Some(_), Some(_)) => match (b, w) {
                    (Some(b), Some(w)) if same_content(b, w, a.crlf) => None,
                    (b, w) => {
                        let mut f = change(path, rel, Status::Modified);
                        f.lines = b.zip(w).and_then(|(b, w)| count_lines(b, w));
                        f.binary = b.is_some_and(is_binary) || w.is_some_and(is_binary);
                        f.hash = hash_of(w, *st);
                        Some(f)
                    }
                },
                (None, None) => None,
            };
            if let Some(f) = &mut f {
                f.crlf = a.crlf;
                f.stat = *st;
                if let Some(name) = &blob_filter {
                    f.blocked = Some(format!("Git converts this file ({name}), and Strata doesn't run Git's filters: use Git to look at it or put it back."));
                }
            }
            found.extend(f.clone());
            // Remembered even when the blob wasn't read (too big, past the
            // budget): a huge file isn't hashed again until it changes; it
            // just stays without line counts.
            seen.insert(rel.clone(), (*st, *key, f));
        }
    }
    cache.seen = seen;

    // A rename that only changed letter case is one file on a
    // case-insensitive disk: never two to delete or restore separately.
    for (old, new) in case_pairs {
        let (Some(i), Some(j)) = (found.iter().position(|f| f.rel == old && f.status == Status::Deleted), found.iter().position(|f| f.rel == new && f.status == Status::Added)) else { continue };
        let (old_path, same) = (found[i].path.clone(), found[i].hash == found[j].hash);
        let f = &mut found[j];
        f.status = Status::Renamed;
        f.old = Some((old_path, old));
        f.alias = true;
        f.lines = (same && !f.binary).then_some((0, 0));
        found.remove(i);
    }
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
    // Files on disk that Git filters can't be compared: counted, not listed.
    let before = found.len();
    found.retain(|f| !(f.filtered && f.status == Status::Modified));
    let filtered = before - found.len();
    found.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(Scan { top, commit, files: found, filtered, tracked: tree.clone(), took: started.elapsed() })
}

/// A deleted file and a new one with the same bytes are one renamed file.
fn pair_renames(found: &mut Vec<FileChange>) {
    let mut gone: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, f) in found.iter().enumerate() {
        if f.status == Status::Deleted && f.hash != 1 && f.blocked.is_none() {
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
/// Files Strata can't review (filtered, links) aren't copied.
fn take_snapshot(folder: &Path, cache: &mut Cache) -> Result<Snapshot, String> {
    let scan = scan(folder, &Base::Head, &HashMap::new(), cache)?;
    let mut files = HashMap::new();
    let mut total = 0;
    for f in scan.files.iter().filter(|f| f.blocked.is_none()) {
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

/// The base's bytes for `rel` when they're at hand without running git (a
/// snapshot's copy): Some(None) means "not in the base". None: ask the
/// scanner (`Scanner::base`).
pub fn base_now(base: &Base, rel: &str) -> Option<Option<Vec<u8>>> {
    match base {
        Base::Snapshot(s) => s.files.get(rel).map(|entry| entry.as_ref().map(|b| b.to_vec())),
        Base::Head => None,
    }
}

/// The base's bytes for `rel` (None: the file isn't in the base). `commit`
/// is the one the list was made against. Runs git: off the UI thread.
fn base_bytes(top: &Path, base: &Base, commit: Option<&str>, rel: &str) -> Option<Vec<u8>> {
    if let Some(found) = base_now(base, rel) {
        return found;
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

/// Move one file to the Trash, if it's still what was listed.
struct TrashJob {
    path: PathBuf,
    top: PathBuf,
    listed: Option<Stat>,
    hash: u64,
}

/// The file at `path` is still the one listed: the same file (device,
/// inode, size, times) with the same bytes.
pub fn unchanged_since(path: &Path, listed: Option<Stat>, hash: u64) -> bool {
    listed.is_some() && stat(path) == listed && file_hash(path) == hash
}

/// Move `path` to the Trash, checking right before that it's inside `top`
/// (no link on the way) and still the file listed: an agent may have
/// replaced it with new work since.
fn trash_if_unchanged(top: &Path, path: &Path, listed: Option<Stat>, hash: u64) -> Result<(), TrashRefused> {
    if !inside(top, path) {
        return Err(TrashRefused::Outside);
    }
    if !unchanged_since(path, listed, hash) {
        return Err(TrashRefused::Changed);
    }
    crate::trash::move_to_trash(path).map_err(|e| TrashRefused::Failed(e.to_string()))
}

/// Fetch one file's base for a review tab.
struct BaseJob {
    path: PathBuf,
    top: PathBuf,
    base: Base,
    commit: Option<String>,
    rel: String,
}

#[derive(Default)]
struct State {
    /// Only the newest scan request matters.
    scan: Option<Job>,
    snapshot: Option<PathBuf>,
    bases: Vec<BaseJob>,
    trash: Vec<TrashJob>,
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

    /// Queue reading `rel`'s base (for the review tab of `path`); the answer
    /// is an `Outcome::Base`.
    pub fn base(&self, path: &Path, top: &Path, base: Base, commit: Option<String>, rel: String) {
        let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        st.bases.push(BaseJob { path: path.to_owned(), top: top.to_owned(), base, commit, rel });
        self.shared.wake.notify_one();
    }

    /// Queue moving `path` to the Trash if it's still as listed (`listed`,
    /// `hash` from its `FileChange`); the answer is an `Outcome::Trashed`.
    pub fn trash(&self, path: &Path, top: &Path, listed: Option<Stat>, hash: u64) {
        let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        st.trash.push(TrashJob { path: path.to_owned(), top: top.to_owned(), listed, hash });
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
    loop {
        let (trash, base, snap, job) = {
            let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while st.scan.is_none() && st.snapshot.is_none() && st.bases.is_empty() && st.trash.is_empty() && !st.quit {
                st = shared.wake.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            if st.quit {
                return;
            }
            // The user waits on a Trash or a base; a scan queued with a
            // snapshot may depend on it.
            if !st.trash.is_empty() {
                (Some(st.trash.remove(0)), None, None, None)
            } else if !st.bases.is_empty() {
                (None, Some(st.bases.remove(0)), None, None)
            } else if let Some(s) = st.snapshot.take() {
                (None, None, Some(s), None)
            } else {
                (None, None, None, st.scan.take())
            }
        };
        let outcome = if let Some(t) = trash {
            let result = trash_if_unchanged(&t.top, &t.path, t.listed, t.hash);
            Outcome::Trashed(t.path, result)
        } else if let Some(b) = base {
            let bytes = base_bytes(&b.top, &b.base, b.commit.as_deref(), &b.rel);
            Outcome::Base(b.path, bytes)
        } else if let Some(folder) = snap {
            Outcome::Snapshot(take_snapshot(&folder, &mut cache))
        } else if let Some(job) = job {
            Outcome::Scanned(job.generation, scan(&job.folder, &job.base, &job.kept, &mut cache))
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

    fn hunks(a: &str, b: &str) -> Vec<Hunk> {
        diff::diff_lines(&crate::compare::buffer_lines(a), &crate::compare::buffer_lines(b), false)
    }

    /// Revert one change at a time, diffing again after each, as the UI does.
    fn revert_all(base: &str, cur: &str) -> String {
        let mut text = cur.to_owned();
        for _ in 0..64 {
            let Some(h) = hunks(base, &text).pop() else { break };
            let (r, with) = revert_edit(base, &text, &h);
            let start: usize = text.char_indices().nth(r.start).map_or(text.len(), |(b, _)| b);
            let end: usize = text.char_indices().nth(r.end).map_or(text.len(), |(b, _)| b);
            text.replace_range(start..end, &with);
        }
        text
    }

    /// Keep one change at a time, diffing again after each.
    fn keep_all(base: &str, cur: &str) -> String {
        let mut b = base.to_owned();
        for _ in 0..64 {
            let Some(h) = hunks(&b, cur).pop() else { break };
            b = keep_edit(&b, cur, &h);
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
        assert!(same_content(b"a\nb\n", b"a\r\nb\r\n", true));
        assert!(same_content(b"a\nb", b"a\r\nb", true));
        assert!(!same_content(b"a\nb\n", b"a\r\nb\n", true));
        assert!(!same_content(b"a\r\nb\r\n", b"a\nb\n", true));
        assert!(!same_content(b"a\nb\n", b"a\r\nc\r\n", true));
        // Without autocrlf or eol=crlf, a change of line endings is a change.
        assert!(!same_content(b"a\nb\n", b"a\r\nb\r\n", false));
    }

    #[test]
    fn line_counts() {
        assert_eq!(count_lines(b"a\nb\n", b"a\nB\nc\n"), Some((2, 1)));
        assert_eq!(count_lines(b"", b"x\ny\n"), Some((2, 0)));
        assert_eq!(count_lines(b"x\ny\n", b""), Some((0, 2)));
        assert_eq!(count_lines(b"x\0y", b"x"), None);
    }

    // ---- against real repositories ----

    const GIT_WAIT: Duration = Duration::from_secs(60);

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
        // Without autocrlf, \r\n where the blob has \n is a change...
        std::fs::write(dir.join("README.md"), "# Title\r\n\r\nText.\r\n").unwrap();
        let mut cache = Cache::default();
        assert_eq!(scan_of(&dir, &Base::Head, &mut cache), vec![("README.md".into(), Status::Modified, Some((0, 0)))]);
        // ...with it, that's how Git checks the file out: no change.
        run(&dir, &["config", "core.autocrlf", "true"]);
        let mut cache = Cache::default();
        assert_eq!(scan_of(&dir, &Base::Head, &mut cache), vec![]);
        // And eol=crlf in .gitattributes says the same for its files only.
        run(&dir, &["config", "core.autocrlf", "false"]);
        std::fs::write(dir.join(".gitattributes"), "*.md text eol=crlf\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {\r\n    one();\r\n    two();\r\n}\r\n").unwrap();
        let mut cache = Cache::default();
        let got = scan_of(&dir, &Base::Head, &mut cache);
        assert_eq!(got, vec![(".gitattributes".into(), Status::Added, Some((1, 0))), ("src/main.rs".into(), Status::Modified, Some((0, 0)))]);
        let s = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert!(!s.files[1].crlf);
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
    fn a_lone_cr_next_to_a_lf_never_joins_two_lines() {
        let (base, cur) = ("bc\r\r", "\n\n");
        let sa = hunks(base, cur);
        assert_eq!(sa.len(), 1);
        let (r, with) = revert_edit(base, cur, &sa[0]);
        let mut text = cur.to_owned();
        text.replace_range(r, &with);
        assert_eq!(text, base, "three lines stay three");
        assert_eq!(crate::compare::buffer_lines(&keep_edit(base, cur, &sa[0])).len(), 3);
    }

    /// Random texts made of a few letters and every kind of line break:
    /// reverting every change gives the base's lines, keeping every change
    /// gives the current text's lines (line breaks of unchanged lines may
    /// stay as they were, but no line is ever lost or joined).
    #[test]
    fn keep_and_revert_keep_the_lines_of_random_texts() {
        let mut seed = 0x9e37_79b9_u64;
        let mut rnd = |n: u64| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) % n
        };
        let pieces = ["a", "b", "\n", "\r", "\r\n", "é"];
        for _ in 0..20_000 {
            let text = |rnd: &mut dyn FnMut(u64) -> u64| (0..rnd(12)).map(|_| pieces[rnd(pieces.len() as u64) as usize]).collect::<String>();
            let (base, cur) = (text(&mut rnd), text(&mut rnd));
            let lines = |s: &str| crate::compare::buffer_lines(s).into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert_eq!(lines(&revert_all(&base, &cur)), lines(&base), "revert {base:?} ← {cur:?}");
            assert_eq!(lines(&keep_all(&base, &cur)), lines(&cur), "keep {base:?} → {cur:?}");
            // Each single change: its lines swapped in, every other line where it was.
            let (lb, lc) = (lines(&base), lines(&cur));
            for h in hunks(&base, &cur) {
                let (r, with) = revert_edit(&base, &cur, &h);
                let mut text: Vec<char> = cur.chars().collect();
                text.splice(r, with.chars());
                let mut want = lc.clone();
                want.splice(h.b.clone(), lb[h.a.clone()].iter().cloned());
                assert_eq!(lines(&text.into_iter().collect::<String>()), want, "revert {h:?} of {base:?} ← {cur:?}");
                let mut want = lb.clone();
                want.splice(h.a.clone(), lc[h.b.clone()].iter().cloned());
                assert_eq!(lines(&keep_edit(&base, &cur, &h)), want, "keep {h:?} of {base:?} → {cur:?}");
            }
        }
    }

    /// A file Git filters (LFS and the like) is stored as something else
    /// than what's on disk: never "changed" for that alone, never copied
    /// into a snapshot, and a deleted one can't be put back from the blob.
    #[test]
    fn filtered_files_are_not_compared() {
        let Some(dir) = temp_repo("filter") else { return };
        std::fs::write(dir.join(".gitattributes"), "*.bin filter=lfs\n*.txt16 working-tree-encoding=UTF-16\n").unwrap();
        std::fs::write(dir.join("data.bin"), "version https://git-lfs.github.com/spec/v1\noid sha256:00\nsize 3\n").unwrap();
        run(&dir, &["add", "."]);
        run(&dir, &["commit", "-q", "-m", "lfs"]);
        // What LFS would have checked out: the real content, not the pointer.
        std::fs::write(dir.join("data.bin"), "the real bytes\n").unwrap();
        let mut cache = Cache::default();
        let s = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert!(s.files.is_empty(), "{:?}", s.files);
        assert_eq!(s.filtered, 1);
        let snap = take_snapshot(&dir, &mut cache).unwrap();
        assert!(snap.files.is_empty());
        std::fs::remove_file(dir.join("data.bin")).unwrap();
        let s = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.files[0].status, Status::Deleted);
        assert!(s.files[0].blocked.as_deref().is_some_and(|w| w.contains("filter=lfs")), "{:?}", s.files[0].blocked);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blob_budget_bounds_what_one_scan_reads() {
        let Some(dir) = temp_repo("budget") else { return };
        for n in 0..3 {
            std::fs::write(dir.join(format!("f{n}.txt")), format!("twenty bytes, line {n}\n").repeat(2)).unwrap();
        }
        run(&dir, &["add", "."]);
        run(&dir, &["commit", "-q", "-m", "three"]);
        for n in 0..3 {
            std::fs::write(dir.join(format!("f{n}.txt")), "changed\n").unwrap();
        }
        let mut cache = Cache { blob_budget: 50, ..Cache::default() };
        let s = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert_eq!(s.files.len(), 3, "every changed file is still listed");
        let counted = s.files.iter().filter(|f| f.lines.is_some()).count();
        assert_eq!(counted, 1, "only what fits in 50 bytes was read: {:?}", s.files);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One worker scans whichever repository it's asked about: what it
    /// remembers about one never shows up in another.
    #[test]
    fn the_cache_never_crosses_repositories() {
        let (Some(a), Some(b)) = (temp_repo("cache-a"), temp_repo("cache-b")) else { return };
        // The same committed file in both; changed in A only, then made to
        // look alike on disk (same size and modification time).
        std::fs::write(a.join("README.md"), "# Title\n\nText!\n").unwrap();
        let when = std::fs::metadata(a.join("README.md")).unwrap().modified().unwrap();
        for p in [a.join("README.md"), b.join("README.md")] {
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(when).unwrap();
        }
        let mut cache = Cache::default();
        let in_a = scan(&a, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert_eq!(in_a.files.len(), 1);
        // Where inode and change time can't tell the files apart (Windows),
        // A's entry would look exactly like B's file: make it so here.
        let (_, key, found) = cache.seen.get("README.md").cloned().unwrap();
        cache.seen.insert("README.md".into(), (stat(&b.join("README.md")), key, found));
        let in_b = scan(&b, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert!(in_b.files.iter().all(|f| f.path.starts_with(&b)), "{:?}", in_b.files);
        assert!(in_b.files.is_empty(), "{:?}", in_b.files);
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    /// A rewrite of the same size within the same modification-time tick is
    /// still noticed (the inode or status-change time differs).
    #[cfg(unix)]
    #[test]
    fn a_same_size_rewrite_is_noticed() {
        let Some(dir) = temp_repo("samesize") else { return };
        let file = dir.join("README.md");
        std::fs::write(&file, "# Title\n\nText1\n").unwrap();
        let when = std::fs::metadata(&file).unwrap().modified().unwrap();
        let mut cache = Cache::default();
        let first = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        std::fs::write(&file, "# Title\n\nText2\n").unwrap();
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(when).unwrap();
        let second = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert_ne!(first.files[0].hash, second.files[0].hash);
        assert_eq!(file_hash(&file), second.files[0].hash);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn utf16_text_is_text() {
        let Some(dir) = temp_repo("utf16") else { return };
        let utf16 = |s: &str| [0xFF, 0xFE].into_iter().chain(s.encode_utf16().flat_map(u16::to_le_bytes)).collect::<Vec<u8>>();
        std::fs::write(dir.join("wide.txt"), utf16("one\r\ntwo\r\n")).unwrap();
        run(&dir, &["add", "."]);
        run(&dir, &["commit", "-q", "-m", "wide"]);
        std::fs::write(dir.join("wide.txt"), utf16("one\r\nTWO\r\nthree\r\n")).unwrap();
        let mut cache = Cache::default();
        let s = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert_eq!((s.files[0].binary, s.files[0].lines), (false, Some((2, 1))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decoding_the_base() {
        let sjis = encoding_rs::SHIFT_JIS.encode("日本語のテキストファイルです。これは文字コードの判定のためのテストです。\n").0.into_owned();
        let doc = crate::encoding::Encoding::from_id("shift_jis").unwrap();
        // The tab's own legacy encoding when the base reads cleanly in it.
        assert_eq!(decode_base(&sjis, doc).unwrap().1, doc);
        // A UTF-8 tab: the base's own (guessed) encoding, not mojibake.
        let (text, enc) = decode_base(&sjis, crate::encoding::Encoding::UTF8).unwrap();
        assert_eq!(enc, doc);
        assert!(text.starts_with("日本語"));
        // Bytes that aren't clean text in any encoding: refused.
        assert!(decode_base(b"\xEF\xBB\xBFok \xFF\xFE\xFD", crate::encoding::Encoding::UTF8).is_err());
    }

    /// The re-audit's fuzz: 300,000 random pairs; each single change swaps
    /// exactly its lines, and reverting every change gives the base's lines back.
    #[test]
    fn keep_and_revert_fuzz() {
        let mut seed: u64 = 0x1234_5678;
        let mut rnd = move |n: u64| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) % n
        };
        let alpha = ["a", "b", "c", "\n", "\n", "\r\n", "\r", "é"];
        let mut fails = Vec::new();
        for _ in 0..300_000 {
            let n = rnd(12);
            let base: String = (0..n).map(|_| alpha[rnd(alpha.len() as u64) as usize]).collect();
            let n = rnd(12);
            let cur: String = (0..n).map(|_| alpha[rnd(alpha.len() as u64) as usize]).collect();
            let (bl, cl) = (crate::compare::buffer_lines(&base), crate::compare::buffer_lines(&cur));
            for h in diff::diff_lines(&bl, &cl, false) {
                let (r, with) = revert_edit(&base, &cur, &h);
                let mut t: Vec<char> = cur.chars().collect();
                t.splice(r, with.chars());
                let t: String = t.into_iter().collect();
                let mut want: Vec<&str> = cl[..h.b.start].to_vec();
                want.extend_from_slice(&bl[h.a.clone()]);
                want.extend_from_slice(&cl[h.b.end..]);
                if crate::compare::buffer_lines(&t) != want {
                    fails.push(format!("revert {h:?} {base:?} ← {cur:?}: {t:?}"));
                }
                let kept = keep_edit(&base, &cur, &h);
                let mut want: Vec<&str> = bl[..h.a.start].to_vec();
                want.extend_from_slice(&cl[h.b.clone()]);
                want.extend_from_slice(&bl[h.a.end..]);
                if crate::compare::buffer_lines(&kept) != want {
                    fails.push(format!("keep {h:?} {base:?} → {cur:?}: {kept:?}"));
                }
            }
            // Every line back. (Line breaks of lines that matched all along can
            // differ: no change shows them, and Revert File puts them back too.)
            let text = revert_all(&base, &cur);
            if crate::compare::buffer_lines(&text) != bl {
                fails.push(format!("revert all {base:?} ← {cur:?}: {text:?}"));
            }
        }
        assert!(fails.is_empty(), "{} failures, first: {:?}", fails.len(), &fails[..fails.len().min(5)]);
    }

    /// Git for Windows sets core.autocrlf=true in the system config: a CRLF
    /// checkout isn't a change, and the base reads with CRLF. Any of Git's
    /// spellings of true count, and core.eol=crlf does it for text files.
    #[test]
    fn line_ending_settings_from_the_system_config() {
        let Some(dir) = temp_repo("sysconfig") else { return };
        let sys = dir.with_extension("sysconfig");
        std::fs::write(dir.join("README.md"), "# Title\r\n\r\nText.\r\n").unwrap();
        let with = |config: &str| {
            std::fs::write(&sys, config).unwrap();
            let mut cache = Cache { system_config: Some(sys.clone()), ..Cache::default() };
            scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap().files
        };
        for yes in ["true", "yes", "on", "1", "True"] {
            assert_eq!(with(&format!("[core]\n\tautocrlf = {yes}\n")), vec![], "autocrlf = {yes}");
        }
        let files = with("[core]\n\tautocrlf = input\n");
        assert_eq!((files.len(), files[0].crlf), (1, false));
        // core.eol=crlf: only for files Git treats as text.
        assert_eq!(with("[core]\n\teol = crlf\n").len(), 1, "no text attribute: not converted");
        std::fs::write(dir.join(".gitattributes"), "*.md text=auto\n").unwrap();
        run(&dir, &["add", ".gitattributes"]);
        run(&dir, &["commit", "-q", "-m", "attrs"]);
        assert_eq!(with("[core]\n\teol = crlf\n"), vec![]);
        // A deleted file's base then reads with CRLF.
        std::fs::remove_file(dir.join("README.md")).unwrap();
        let files = with("[core]\n\tautocrlf = true\n");
        assert!(files[0].status == Status::Deleted && files[0].crlf);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&sys);
    }

    /// A file whose base wasn't read (too big, past the budget) is still
    /// remembered: the next scan doesn't hash it again unless it changed.
    #[test]
    fn unread_files_are_not_hashed_again() {
        let Some(dir) = temp_repo("rehash") else { return };
        std::fs::write(dir.join("README.md"), "# Title\n\nChanged.\n").unwrap();
        let mut cache = Cache { blob_budget: 0, ..Cache::default() };
        let first = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert_eq!((first.files.len(), first.files[0].lines), (1, None));
        assert!(cache.hashed > 0);
        let again = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert_eq!((again.files.len(), cache.hashed), (1, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A name that isn't valid UTF-8 (`a\xff.txt`) next to a real file named
    /// with U+FFFD (`a�.txt`, which a lossy spelling of the other would give):
    /// the real file keeps its own base, the other is listed and left alone.
    #[cfg(unix)]
    #[test]
    fn names_that_are_not_utf8_never_stand_for_another_file() {
        use std::os::unix::ffi::OsStrExt;
        let Some(dir) = temp_repo("badname") else { return };
        std::fs::write(dir.join("a\u{FFFD}.txt"), "real\n").unwrap();
        run(&dir, &["add", "."]);
        // The other one only in the index (macOS disks refuse such names).
        let blob = crate::git::git_io(&dir, &["hash-object", "-w", "--stdin"], Some(b"other\n".to_vec()), 1 << 10, GIT_WAIT).unwrap();
        let blob = String::from_utf8(blob).unwrap();
        let mut info = format!("100644 {}\t", blob.trim()).into_bytes();
        info.extend_from_slice(b"a\xff.txt\n");
        assert!(crate::git::git_io(&dir, &["update-index", "--index-info"], Some(info), 1 << 10, GIT_WAIT).is_some());
        run(&dir, &["commit", "-q", "-m", "names"]);
        let mut cache = Cache::default();
        let s = scan(&dir, &Base::Head, &HashMap::new(), &mut cache).unwrap();
        assert!(!s.files.iter().any(|f| f.path == dir.join("a\u{FFFD}.txt")), "the real file is unchanged: {:?}", s.files);
        let bad = s.files.iter().find(|f| f.path.as_os_str().as_bytes().ends_with(b"a\xff.txt")).expect("listed");
        assert_eq!(bad.blocked.as_deref(), Some(BAD_NAME));
        assert!(!inside(&s.top, &bad.path), "never acted on");
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
