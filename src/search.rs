//! Find in files: a background, parallel walk of a folder (respecting
//! .gitignore via ripgrep's `ignore` crate) streaming hits to the UI.

use crate::find::Query;
use ignore::{WalkBuilder, WalkState};
use regex::Regex;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;

/// Stop after this many hits; the UI says the results are truncated.
pub const HIT_LIMIT: usize = 10_000;
/// Skip files larger than this (logs, dumps, bundles).
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
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
    files_scanned: Arc<AtomicUsize>,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
}

impl Search {
    pub fn start(&mut self, root: &Path, q: &Query, ctx: Option<egui::Context>) {
        self.stop();
        self.results.clear();
        self.truncated = false;
        self.error = None;
        if q.text.is_empty() {
            return;
        }
        let re = match q.build() {
            Ok(re) => re,
            Err(e) => {
                self.error = Some(e.to_string().lines().last().unwrap_or("invalid regex").trim().to_owned());
                return;
            }
        };
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.running = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        self.files_scanned = Arc::new(AtomicUsize::new(0));
        let (cancel, scanned, root) = (self.cancel.clone(), self.files_scanned.clone(), root.to_owned());
        std::thread::spawn(move || walk(&root, &re, &tx, &cancel, &scanned, ctx.as_ref()));
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
}

fn walk(root: &Path, re: &Regex, tx: &Sender<Msg>, cancel: &AtomicBool, scanned: &AtomicUsize, ctx: Option<&egui::Context>) {
    let hits = AtomicUsize::new(0);
    let truncated = AtomicBool::new(false);
    WalkBuilder::new(root)
        .require_git(false) // honor .gitignore even outside a git repo
        .build_parallel()
        .run(|| {
            Box::new(|entry| {
                if cancel.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else { return WalkState::Continue };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
                scanned.fetch_add(1, Ordering::Relaxed);
                let Some(file) = search_file(entry.path(), re) else { return WalkState::Continue };
                let total = hits.fetch_add(file.hits.len(), Ordering::Relaxed) + file.hits.len();
                let _ = tx.send(Msg::File(file));
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
                if total >= HIT_LIMIT {
                    truncated.store(true, Ordering::Relaxed);
                    return WalkState::Quit;
                }
                WalkState::Continue
            })
        });
    let _ = tx.send(Msg::Done { truncated: truncated.load(Ordering::Relaxed) });
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
}

fn search_file(path: &Path, re: &Regex) -> Option<FileHits> {
    if std::fs::metadata(path).ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    // Same binary heuristic as git/ripgrep: a NUL byte near the start.
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut hits = Vec::new();
    for (line, content) in text.lines().enumerate() {
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
    (!hits.is_empty()).then(|| FileHits { path: path.to_owned(), hits })
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
        while s.running {
            s.poll();
            std::thread::yield_now();
        }
        s
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
        fs::remove_dir_all(root).ok();
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
