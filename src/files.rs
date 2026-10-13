//! Goto Anything's file list: a background, parallel walk of the open
//! folders (respecting .gitignore via the `ignore` crate) streaming paths.

use ignore::{WalkBuilder, WalkState};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;

/// Stop listing past this many files; `is_truncated` reports it.
pub const FILE_LIMIT: usize = 500_000;
/// Paths per message from a walker thread.
const BATCH: usize = 2048;

enum Msg {
    Batch(Vec<Entry>),
    Done { truncated: bool },
}

/// A listed file: its label and where to find it.
struct Entry {
    /// Path relative to its folder with `/` separators, after the folder's
    /// name when there are several folders ("app/src/main.rs").
    label: String,
    /// Index of the folder in `roots`.
    root: u32,
    /// Bytes of `label` before the relative path (the folder name and `/`).
    skip: u32,
}

#[derive(Default)]
pub struct FileIndex {
    roots: Vec<PathBuf>,
    entries: Vec<Entry>,
    ready: bool,
    truncated: bool,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
}

impl FileIndex {
    /// Re-list `roots` from scratch, cancelling any walk in progress.
    pub fn start(&mut self, roots: &[PathBuf], ctx: Option<egui::Context>) {
        self.stop();
        self.roots = roots.to_vec();
        self.entries.clear();
        self.ready = false;
        self.truncated = false;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.cancel = Arc::new(AtomicBool::new(false));
        let (cancel, roots) = (self.cancel.clone(), roots.to_vec());
        std::thread::spawn(move || walk(&roots, &tx, &cancel, ctx.as_ref()));
    }

    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.rx = None;
    }

    /// Drain streamed paths. Call once per frame; true if anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut changed = false;
        while let Ok(msg) = rx.try_recv() {
            changed = true;
            match msg {
                Msg::Batch(batch) => self.entries.extend(batch),
                Msg::Done { truncated } => {
                    self.truncated = truncated;
                    self.ready = true;
                    self.entries.sort_unstable_by(|a, b| a.label.cmp(&b.label));
                    // A folder inside another is walked twice.
                    self.entries.dedup_by(|a, b| a.label == b.label && a.root == b.root);
                }
            }
        }
        if self.ready {
            self.rx = None;
        }
        changed
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// (label, full path) of every file: labels are paths relative to the
    /// folder with `/` separators, after the folder's name when there are
    /// several. Unordered while the walk runs, sorted once it's ready.
    pub fn entries(&self) -> impl Iterator<Item = (&str, PathBuf)> {
        self.entries.iter().map(|e| (e.label.as_str(), self.roots[e.root as usize].join(&e.label[e.skip as usize..])))
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }
}

/// One walker thread's pending paths; flushed when full and when dropped.
struct Batcher<'a> {
    buf: Vec<Entry>,
    tx: Sender<Msg>,
    ctx: Option<&'a egui::Context>,
}

impl Batcher<'_> {
    fn flush(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let _ = self.tx.send(Msg::Batch(std::mem::take(&mut self.buf)));
        if let Some(ctx) = self.ctx {
            ctx.request_repaint();
        }
    }
}

impl Drop for Batcher<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

fn walk(roots: &[PathBuf], tx: &Sender<Msg>, cancel: &AtomicBool, ctx: Option<&egui::Context>) {
    let Some(first) = roots.first() else {
        let _ = tx.send(Msg::Done { truncated: false });
        return;
    };
    let (count, truncated) = (&AtomicUsize::new(0), &AtomicBool::new(false));
    // With several folders, each label starts with its folder's name.
    let prefixes: Vec<String> = if roots.len() > 1 {
        roots.iter().map(|r| format!("{}/", r.file_name().map_or_else(|| r.display().to_string(), |n| n.to_string_lossy().into_owned()))).collect()
    } else {
        vec![String::new()]
    };
    let mut builder = WalkBuilder::new(first);
    for r in &roots[1..] {
        builder.add(r);
    }
    builder
        .require_git(false) // honor .gitignore even outside a git repo
        .filter_entry(|e| e.file_name() != ".git")
        .build_parallel()
        .run(|| {
            let mut batch = Batcher { buf: Vec::with_capacity(BATCH), tx: tx.clone(), ctx };
            let prefixes = &prefixes;
            Box::new(move |entry| {
                if cancel.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else { return WalkState::Continue };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
                // The innermost folder holding the file.
                let Some((root, rel)) = roots.iter().enumerate().filter_map(|(i, r)| Some((i, relative(r, entry.path())?))).min_by_key(|(_, rel)| rel.len()) else {
                    return WalkState::Continue;
                };
                if count.fetch_add(1, Ordering::Relaxed) >= FILE_LIMIT {
                    truncated.store(true, Ordering::Relaxed);
                    return WalkState::Quit;
                }
                let prefix = &prefixes[root.min(prefixes.len() - 1)];
                batch.buf.push(Entry { label: format!("{prefix}{rel}"), root: root as u32, skip: prefix.len() as u32 });
                if batch.buf.len() >= BATCH {
                    batch.flush();
                }
                WalkState::Continue
            })
        });
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    let _ = tx.send(Msg::Done { truncated: truncated.load(Ordering::Relaxed) });
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
}

fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?.to_string_lossy();
    if rel.is_empty() {
        return None;
    }
    Some(if cfg!(windows) { rel.replace('\\', "/") } else { rel.into_owned() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn labels(f: &FileIndex) -> Vec<&str> {
        f.entries.iter().map(|e| e.label.as_str()).collect()
    }

    fn index(root: &Path) -> FileIndex {
        let mut f = FileIndex::default();
        f.start(&[root.to_owned()], None);
        while !f.is_ready() {
            f.poll();
            std::thread::yield_now();
        }
        f
    }

    #[test]
    fn lists_files_respecting_gitignore_hidden_and_git() {
        let root = temp_dir("walk");
        for dir in ["src/nested", "target/debug", ".git/objects", ".hidden"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for (path, body) in [
            (".gitignore", "target/\n*.log\n"),
            ("README.md", ""),
            ("src/main.rs", ""),
            ("src/nested/deep.rs", ""),
            ("src/debug.log", ""),
            ("target/debug/strata", ""),
            (".git/HEAD", ""),
            (".git/objects/abc", ""),
            (".hidden/secret", ""),
            (".env", ""),
        ] {
            fs::write(root.join(path), body).unwrap();
        }
        let f = index(&root);
        assert_eq!(labels(&f), ["README.md", "src/main.rs", "src/nested/deep.rs"]);
        assert!(!f.is_truncated());
        assert_eq!(f.entries().next().unwrap().1, root.join("README.md"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn restart_replaces_the_previous_listing() {
        let (a, b) = (temp_dir("a"), temp_dir("b"));
        fs::write(a.join("one.txt"), "").unwrap();
        fs::write(b.join("two.txt"), "").unwrap();
        let mut f = index(&a);
        assert_eq!(labels(&f), ["one.txt"]);
        f.start(std::slice::from_ref(&b), None);
        assert!(!f.is_ready());
        while !f.is_ready() {
            f.poll();
            std::thread::yield_now();
        }
        assert_eq!(labels(&f), ["two.txt"]);
        assert!(!f.poll());
        fs::remove_dir_all(a).ok();
        fs::remove_dir_all(b).ok();
    }

    #[test]
    fn several_roots_are_listed_under_their_names() {
        let base = temp_dir("roots");
        let (a, b) = (base.join("app"), base.join("lib"));
        fs::create_dir_all(a.join("src")).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("src/main.rs"), "").unwrap();
        fs::write(b.join("util.rs"), "").unwrap();
        let mut f = FileIndex::default();
        f.start(&[a.clone(), b.clone()], None);
        while !f.is_ready() {
            f.poll();
            std::thread::yield_now();
        }
        let entries: Vec<(String, PathBuf)> = f.entries().map(|(l, p)| (l.to_owned(), p)).collect();
        assert_eq!(entries, [("app/src/main.rs".to_owned(), a.join("src/main.rs")), ("lib/util.rs".to_owned(), b.join("util.rs"))]);
        fs::remove_dir_all(base).ok();
    }
}
