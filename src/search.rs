//! Find in files: a background, parallel walk of the "Where" folders
//! (respecting .gitignore via ripgrep's `ignore` crate) streaming hits to
//! the UI. Files open with unsaved changes are searched as they are in
//! their tab, not on disk.

use crate::encoding::{self, Encoding};
use crate::find::Query;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::{WalkBuilder, WalkState};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Stop after this many hits; the UI says the results are truncated.
pub const HIT_LIMIT: usize = 10_000;
/// Skip files larger than this (logs, dumps, bundles).
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// Longest preview shown for a hit line.
const PREVIEW_CHARS: usize = 160;

#[derive(Debug, Clone)]
pub struct LineHit {
    /// 0-based line number.
    pub line: usize,
    /// Match position in the full line, in chars.
    pub cols: Range<usize>,
    /// Trimmed line for display, and the match within it (chars).
    pub preview: String,
    pub preview_cols: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct FileHits {
    pub path: PathBuf,
    pub hits: Vec<LineHit>,
    /// Size and mtime of the file when it was searched; None when the text
    /// came from an open tab with unsaved changes.
    pub stamp: Option<Stamp>,
}

/// What a file looked like on disk, to notice it changing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub mtime: Option<SystemTime>,
}

impl Stamp {
    /// Follows symlinks.
    pub fn of(path: &Path) -> std::io::Result<Stamp> {
        let m = std::fs::metadata(path)?;
        Ok(Stamp { len: m.len(), mtime: m.modified().ok() })
    }
}

/// A text file as find and replace in files see it.
pub struct TextFile {
    pub bytes: Vec<u8>,
    pub text: String,
    pub encoding: Encoding,
    /// Some bytes didn't decode (they read as U+FFFD).
    pub lossy: bool,
    pub stamp: Stamp,
}

/// Why a file isn't searched or rewritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unreadable {
    TooLarge,
    Binary,
    Io(String),
}

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unreadable::TooLarge => write!(f, "larger than {} MB", MAX_FILE_BYTES / (1024 * 1024)),
            Unreadable::Binary => f.write_str("binary file"),
            Unreadable::Io(e) => write!(f, "can't read: {e}"),
        }
    }
}

/// Read and decode a file the way opening it in a tab would (encoding
/// detection, BOM dropped). Binary files (a NUL byte near the start, unless
/// the text is UTF-16) and files over [`MAX_FILE_BYTES`] are refused.
pub fn read_text(path: &Path) -> Result<TextFile, Unreadable> {
    let io = |e: std::io::Error| Unreadable::Io(e.to_string());
    let stamp = Stamp::of(path).map_err(io)?;
    if stamp.len > MAX_FILE_BYTES {
        return Err(Unreadable::TooLarge);
    }
    let bytes = std::fs::read(path).map_err(io)?;
    let (text, encoding, lossy) = encoding::decode_detect(&bytes);
    let utf16 = [Encoding::UTF16LE, Encoding::UTF16LE_BOM, Encoding::UTF16BE, Encoding::UTF16BE_BOM].contains(&encoding);
    // Same binary heuristic as git/ripgrep.
    if !utf16 && bytes[..bytes.len().min(8192)].contains(&0) {
        return Err(Unreadable::Binary);
    }
    let text = text.into_owned();
    Ok(TextFile { bytes, text, encoding, lossy, stamp })
}

/// Lines of `text` as (byte offset, content without its line break). Matches
/// never span lines; the split is the same as `str::lines`.
pub fn lines_with_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut at = 0;
    text.split_inclusive('\n').map(move |seg| {
        let start = at;
        at += seg.len();
        let content = seg.strip_suffix('\n').map_or(seg, |s| s.strip_suffix('\r').unwrap_or(s));
        (start, content)
    })
}

/// Where to search: the parsed "Where" field.
#[derive(Default, Clone)]
pub struct Scope {
    pub folders: Vec<PathBuf>,
    /// Single files (`<open files>` or a file path), searched even if ignored.
    pub files: Vec<PathBuf>,
    include: Vec<String>,
    exclude: Vec<String>,
    /// Unsaved text of open tabs, searched instead of the file on disk.
    pub buffers: HashMap<PathBuf, Arc<String>>,
}

impl Scope {
    #[cfg(test)]
    pub fn folder(root: &Path) -> Self {
        Self::folders(vec![root.to_owned()])
    }

    pub fn folders(roots: Vec<PathBuf>) -> Self {
        Scope { folders: roots, ..Default::default() }
    }

    /// Parse Sublime's "Where" syntax: comma-separated folders or files
    /// (relative paths are looked up in each of `roots`), `<open folders>`,
    /// `<open files>`, include globs like `*.rs` and exclude globs like
    /// `-*.min.js` or `-target/`. A glob without a `/` matches file names
    /// (excludes also match folder names); one with a `/` matches the path
    /// relative to the searched folder. Only globs (or an empty field)
    /// searches every folder in `roots`, or with none, like Sublime, the
    /// open files.
    pub fn parse(spec: &str, roots: &[PathBuf], open_files: &[PathBuf]) -> Result<Scope, String> {
        let mut scope = Scope::default();
        let mut named_place = false;
        for token in spec.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            if let Some(glob) = token.strip_prefix('-') {
                compile(&[glob.to_owned()])?;
                scope.exclude.push(glob.to_owned());
            } else if token == "<open folders>" || token == "<project>" {
                named_place = true;
                scope.folders.extend(roots.iter().cloned());
            } else if token == "<open files>" {
                named_place = true;
                scope.files.extend(open_files.iter().cloned());
            } else if token.contains(['*', '?', '[', '{']) {
                compile(&[token.to_owned()])?;
                scope.include.push(token.to_owned());
            } else {
                named_place = true;
                let path = expand_path(token, roots);
                if path.is_dir() {
                    scope.folders.push(path);
                } else if path.is_file() {
                    scope.files.push(path);
                } else {
                    return Err(format!("No such folder: {token}"));
                }
            }
        }
        if !named_place {
            if roots.is_empty() {
                if open_files.is_empty() {
                    return Err("Nothing to search: open a folder (File ▸ Open Folder…) or a file, or list folders in Where".into());
                }
                scope.files.extend(open_files.iter().cloned());
            } else {
                scope.folders.extend(roots.iter().cloned());
            }
        }
        if scope.folders.is_empty() && scope.files.is_empty() {
            return Err("Nothing to search in (no open files)".into());
        }
        Ok(scope)
    }
}

/// A Where path: `~/…` is in the home folder, and a relative path is in the
/// first of `roots` that has it (else the first).
fn expand_path(token: &str, roots: &[PathBuf]) -> PathBuf {
    if let Some(rest) = token.strip_prefix("~/").or_else(|| token.strip_prefix("~\\"))
        && let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    {
        return PathBuf::from(home).join(rest);
    }
    let p = PathBuf::from(token);
    if p.is_absolute() || roots.is_empty() {
        return p;
    }
    roots.iter().map(|r| r.join(&p)).find(|q| q.exists()).unwrap_or_else(|| roots[0].join(&p))
}

fn compile(globs: &[String]) -> Result<Option<GlobSet>, String> {
    if globs.is_empty() {
        return Ok(None);
    }
    let mut set = GlobSetBuilder::new();
    for g in globs {
        let g = g.trim_end_matches(['/', '\\']);
        let glob = GlobBuilder::new(g).literal_separator(false).backslash_escape(false).build().map_err(|e| format!("Bad pattern {g}: {}", e.kind()))?;
        set.add(glob);
    }
    set.build().map(Some).map_err(|e| e.to_string())
}

/// Include/exclude matching, shared by the walker threads.
struct Filter {
    folders: Vec<PathBuf>,
    include_names: Option<GlobSet>,
    include_paths: Option<GlobSet>,
    exclude_names: Option<GlobSet>,
    exclude_paths: Option<GlobSet>,
}

impl Filter {
    fn new(scope: &Scope) -> Result<Self, String> {
        let split = |globs: &[String]| -> (Vec<String>, Vec<String>) { globs.iter().cloned().partition(|g| !g.trim_end_matches(['/', '\\']).contains(['/', '\\'])) };
        let (inc_n, inc_p) = split(&scope.include);
        let (exc_n, exc_p) = split(&scope.exclude);
        Ok(Filter {
            folders: scope.folders.clone(),
            include_names: compile(&inc_n)?,
            include_paths: compile(&inc_p)?,
            exclude_names: compile(&exc_n)?,
            exclude_paths: compile(&exc_p)?,
        })
    }

    fn relative<'a>(&self, path: &'a Path) -> &'a Path {
        self.folders.iter().find_map(|f| path.strip_prefix(f).ok()).unwrap_or(path)
    }

    fn excluded(&self, path: &Path) -> bool {
        let name_hit = self.exclude_names.as_ref().is_some_and(|g| path.file_name().is_some_and(|n| g.is_match(n)));
        name_hit || self.exclude_paths.as_ref().is_some_and(|g| g.is_match(self.relative(path)))
    }

    fn wanted_file(&self, path: &Path) -> bool {
        if self.excluded(path) {
            return false;
        }
        if self.include_names.is_none() && self.include_paths.is_none() {
            return true;
        }
        self.include_names.as_ref().is_some_and(|g| path.file_name().is_some_and(|n| g.is_match(n)))
            || self.include_paths.as_ref().is_some_and(|g| g.is_match(self.relative(path)))
    }
}

enum Msg {
    File(FileHits),
    Done { truncated: bool },
}

#[derive(Default)]
pub struct Search {
    /// Sorted by path, even while results stream in.
    pub results: Vec<FileHits>,
    pub running: bool,
    pub truncated: bool,
    pub error: Option<String>,
    /// The query and "Where" text the current results are for.
    pub searched: Option<(Query, String)>,
    /// Bumped by every new search (or failed one), so stepping through
    /// results can tell they were replaced.
    pub generation: u64,
    files_scanned: Arc<AtomicUsize>,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
}

impl Search {
    /// Search the folder `root`.
    #[cfg(test)]
    pub fn start(&mut self, root: &Path, q: &Query, ctx: Option<egui::Context>) {
        self.start_in(Scope::folder(root), "", q, ctx);
    }

    /// Search `scope`; `spec` is the "Where" text it came from.
    pub fn start_in(&mut self, scope: Scope, spec: &str, q: &Query, ctx: Option<egui::Context>) {
        self.stop();
        self.generation += 1;
        self.results.clear();
        self.truncated = false;
        self.error = None;
        self.searched = None;
        self.files_scanned = Arc::new(AtomicUsize::new(0));
        if q.text.is_empty() {
            return;
        }
        let re = match q.build() {
            Ok(re) => re,
            Err(e) => return self.fail(e.to_string().lines().last().unwrap_or("invalid regex").trim().to_owned()),
        };
        let filter = match Filter::new(&scope) {
            Ok(f) => f,
            Err(e) => return self.fail(e),
        };
        self.searched = Some((q.clone(), spec.to_owned()));
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.running = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        let (cancel, scanned) = (self.cancel.clone(), self.files_scanned.clone());
        std::thread::spawn(move || walk(&scope, filter, &re, &tx, &cancel, &scanned, ctx.as_ref()));
    }

    /// Show `msg` instead of results.
    pub fn fail(&mut self, msg: String) {
        self.stop();
        self.generation += 1;
        self.results.clear();
        self.searched = None;
        self.error = Some(msg);
    }

    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.rx = None;
        self.running = false;
    }

    pub fn files_scanned(&self) -> usize {
        self.files_scanned.load(Ordering::Relaxed)
    }

    pub fn total_hits(&self) -> usize {
        self.results.iter().map(|f| f.hits.len()).sum()
    }

    /// Drain streamed results. Call once per frame.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::File(f) => {
                    let i = self.results.partition_point(|r| r.path < f.path);
                    self.results.insert(i, f);
                }
                Msg::Done { truncated } => {
                    self.truncated = truncated;
                    self.running = false;
                }
            }
        }
        if !self.running {
            self.rx = None;
        }
    }

    /// Block until the search finishes (tests and dev hooks).
    pub fn wait(&mut self) {
        while self.running {
            self.poll();
            std::thread::yield_now();
        }
    }
}

fn walk(scope: &Scope, filter: Filter, re: &Regex, tx: &Sender<Msg>, cancel: &AtomicBool, scanned: &AtomicUsize, ctx: Option<&egui::Context>) {
    let hits = AtomicUsize::new(0);
    let truncated = AtomicBool::new(false);
    // Several places could list the same file twice.
    let overlap = scope.folders.len() + scope.files.len() > 1;
    let seen = Mutex::new(HashSet::new());
    let buffers: HashMap<PathBuf, Arc<String>> = scope
        .buffers
        .iter()
        .flat_map(|(p, t)| std::iter::once((p.clone(), t.clone())).chain(std::fs::canonicalize(p).ok().map(|c| (c, t.clone()))))
        .collect();
    // Search one file; false = stop the walk.
    let visit = |path: &Path| -> bool {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        // Resolving symlinks costs a few syscalls per file: only when it matters.
        let key = (overlap || !buffers.is_empty()).then(|| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()));
        if overlap && let Some(key) = &key
            && !seen.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone())
        {
            return true;
        }
        scanned.fetch_add(1, Ordering::Relaxed);
        let file = match buffers.get(path).or_else(|| key.as_ref().and_then(|k| buffers.get(k))) {
            Some(text) => search_text(path, text, re, None),
            None => search_file(path, re),
        };
        let Some(file) = file else { return true };
        let total = hits.fetch_add(file.hits.len(), Ordering::Relaxed) + file.hits.len();
        let _ = tx.send(Msg::File(file));
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
        if total >= HIT_LIMIT {
            truncated.store(true, Ordering::Relaxed);
            return false;
        }
        true
    };
    let mut go = true;
    for f in &scope.files {
        if go && f.is_file() && filter.wanted_file(f) {
            go = visit(f);
        }
    }
    if go && let Some((first, rest)) = scope.folders.split_first() {
        let mut builder = WalkBuilder::new(first);
        for f in rest {
            builder.add(f);
        }
        let filter = Arc::new(filter);
        let pruner = filter.clone();
        builder
            .require_git(false) // honor .gitignore even outside a git repo
            .filter_entry(move |e| e.depth() == 0 || !e.file_type().is_some_and(|t| t.is_dir()) || !pruner.excluded(e.path()))
            .build_parallel()
            .run(|| {
                Box::new(|entry| {
                    let Ok(entry) = entry else { return WalkState::Continue };
                    if !entry.file_type().is_some_and(|t| t.is_file()) || !filter.wanted_file(entry.path()) {
                        return if cancel.load(Ordering::Relaxed) { WalkState::Quit } else { WalkState::Continue };
                    }
                    if visit(entry.path()) { WalkState::Continue } else { WalkState::Quit }
                })
            });
    }
    let _ = tx.send(Msg::Done { truncated: truncated.load(Ordering::Relaxed) });
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
}

fn search_file(path: &Path, re: &Regex) -> Option<FileHits> {
    let file = read_text(path).ok()?;
    search_text(path, &file.text, re, Some(file.stamp))
}

fn search_text(path: &Path, text: &str, re: &Regex, stamp: Option<Stamp>) -> Option<FileHits> {
    let mut hits = Vec::new();
    for (line, (_, content)) in lines_with_offsets(text).enumerate() {
        for m in re.find_iter(content) {
            if m.start() == m.end() {
                continue;
            }
            let start = content[..m.start()].chars().count();
            let end = start + m.as_str().chars().count();
            let (preview, preview_cols) = preview(content, start..end);
            hits.push(LineHit { line, cols: start..end, preview, preview_cols });
        }
    }
    (!hits.is_empty()).then(|| FileHits { path: path.to_owned(), hits, stamp })
}

/// The line with leading whitespace trimmed, windowed around long matches.
fn preview(line: &str, cols: Range<usize>) -> (String, Range<usize>) {
    let chars: Vec<char> = line.chars().collect();
    let indent = chars.iter().take_while(|c| c.is_whitespace()).count().min(cols.start);
    let from = if cols.start - indent > PREVIEW_CHARS / 2 { cols.start - PREVIEW_CHARS / 3 } else { indent };
    let to = (from + PREVIEW_CHARS).min(chars.len()).max(cols.end.min(chars.len()));
    let mut s: String = chars[from..to].iter().collect();
    let mut offset = 0;
    if from > indent {
        s.insert(0, '…');
        offset = 1;
    }
    if to < chars.len() {
        s.push('…');
    }
    (s, cols.start - from + offset..cols.end - from + offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_tree() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-search-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/nested")).unwrap();
        fs::write(dir.join("src/a.rs"), "fn alpha() {}\n    let beta = alpha();\n").unwrap();
        fs::write(dir.join("src/nested/b.txt"), "no match\nalpha again alpha\n").unwrap();
        fs::write(dir.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(dir.join("ignored.txt"), "alpha\n").unwrap();
        fs::write(dir.join("bin.dat"), b"alpha\0\x01\x02").unwrap();
        dir
    }

    fn run(root: &Path, q: &Query) -> Search {
        let mut s = Search::default();
        s.start(root, q, None);
        s.wait();
        s
    }

    fn run_where(root: &Path, spec: &str, open: &[PathBuf], q: &Query) -> Result<Vec<String>, String> {
        let scope = Scope::parse(spec, &[root.to_owned()], open)?;
        let mut s = Search::default();
        s.start_in(scope, spec, q, None);
        s.wait();
        if let Some(e) = s.error {
            return Err(e);
        }
        Ok(s.results.iter().map(|f| f.path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/")).collect())
    }

    #[test]
    fn finds_across_files_respecting_gitignore_and_binary() {
        let root = temp_tree();
        let s = run(&root, &Query::literal("alpha", false));
        let files: Vec<_> = s.results.iter().map(|f| f.path.strip_prefix(&root).unwrap().to_owned()).collect();
        assert_eq!(files, vec![PathBuf::from("src/a.rs"), PathBuf::from("src/nested/b.txt")]);
        assert_eq!(s.total_hits(), 4);
        let hit = &s.results[0].hits[1];
        assert_eq!((hit.line, hit.cols.clone()), (1, 15..20));
        assert_eq!(hit.preview, "let beta = alpha();");
        assert_eq!(&hit.preview[hit.preview_cols.clone()], "alpha");
        assert!(s.files_scanned() >= 2);
        assert!(s.results[0].stamp.is_some());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn where_folders_includes_and_excludes() {
        let root = temp_tree();
        fs::write(root.join("src/app.min.js"), "alpha").unwrap();
        fs::write(root.join("src/nested/c.rs"), "alpha").unwrap();
        let q = Query::literal("alpha", false);
        assert_eq!(run_where(&root, "*.rs", &[], &q).unwrap(), ["src/a.rs", "src/nested/c.rs"]);
        assert_eq!(run_where(&root, "-*.min.js", &[], &q).unwrap(), ["src/a.rs", "src/nested/b.txt", "src/nested/c.rs"]);
        assert_eq!(run_where(&root, "src/nested, -*.txt", &[], &q).unwrap(), ["src/nested/c.rs"]);
        assert_eq!(run_where(&root, "-nested/", &[], &q).unwrap(), ["src/a.rs", "src/app.min.js"]);
        assert_eq!(run_where(&root, "-src/nested/*", &[], &q).unwrap(), ["src/a.rs", "src/app.min.js"]);
        assert_eq!(run_where(&root, "<open folders>, *.js", &[], &q).unwrap(), ["src/app.min.js"]);
        // <open files> searches just those, even when gitignored.
        assert_eq!(run_where(&root, "<open files>", &[root.join("ignored.txt")], &q).unwrap(), ["ignored.txt"]);
        assert!(run_where(&root, "nope", &[], &q).unwrap_err().contains("No such folder"));
        assert!(Scope::parse("", &[], &[]).is_err());
        // No folder: the open files, like Sublime.
        let open = [root.join("src/a.rs")];
        assert_eq!(Scope::parse("", &[], &open).unwrap().files, open);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unsaved_buffers_are_searched_instead_of_disk() {
        let root = temp_tree();
        let mut scope = Scope::folder(&root);
        scope.buffers.insert(root.join("src/a.rs"), Arc::new("nothing here\nalpha\n".into()));
        let mut s = Search::default();
        s.start_in(scope, "", &Query::literal("alpha", false), None);
        s.wait();
        let a = s.results.iter().find(|f| f.path.ends_with("a.rs")).unwrap();
        assert_eq!((a.hits.len(), a.hits[0].line, a.stamp), (1, 1, None));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn utf16_files_are_searched_not_skipped_as_binary() {
        let root = temp_tree();
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("x alpha\r\n".encode_utf16().flat_map(u16::to_le_bytes));
        fs::write(root.join("wide.txt"), bytes).unwrap();
        let s = run(&root, &Query::literal("alpha", false));
        let wide = s.results.iter().find(|f| f.path.ends_with("wide.txt")).expect("UTF-16 file searched");
        assert_eq!(wide.hits[0].cols, 2..7);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn lines_split_like_str_lines() {
        let text = "a\r\nb\nc\rd\n\ne";
        let ours: Vec<&str> = lines_with_offsets(text).map(|(_, l)| l).collect();
        assert_eq!(ours, text.lines().collect::<Vec<_>>());
        assert_eq!(lines_with_offsets(text).map(|(o, _)| o).collect::<Vec<_>>(), [0, 3, 5, 9, 10]);
    }

    #[test]
    fn bad_regex_reports_error() {
        let root = temp_tree();
        let q = Query { text: "(".into(), regex: true, ..Default::default() };
        let s = run(&root, &q);
        assert!(s.error.is_some());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn long_lines_preview_around_the_match() {
        let line = format!("{}needle{}", "x".repeat(500), "y".repeat(500));
        let (p, cols) = preview(&line, 500..506);
        assert!(p.chars().count() <= PREVIEW_CHARS + 2);
        assert_eq!(p.chars().skip(cols.start).take(6).collect::<String>(), "needle");
    }
}
