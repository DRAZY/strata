//! Goto Anything's file list: a background, parallel walk of the open folder
//! (respecting .gitignore via the `ignore` crate) streaming relative paths.

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
    Batch(Vec<String>),
    Done { truncated: bool },
}

#[derive(Default)]
pub struct FileIndex {
    root: PathBuf,
    paths: Vec<String>,
    ready: bool,
    truncated: bool,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
}

impl FileIndex {
    /// Re-list `root` from scratch, cancelling any walk in progress.
    pub fn start(&mut self, root: &Path, ctx: Option<egui::Context>) {
        self.stop();
        self.root = root.to_owned();
        self.paths.clear();
        self.ready = false;
        self.truncated = false;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.cancel = Arc::new(AtomicBool::new(false));
        let (cancel, root) = (self.cancel.clone(), root.to_owned());
        std::thread::spawn(move || walk(&root, &tx, &cancel, ctx.as_ref()));
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
                Msg::Batch(batch) => self.paths.extend(batch),
                Msg::Done { truncated } => {
                    self.truncated = truncated;
                    self.ready = true;
                    self.paths.sort_unstable();
                }
            }
        }
        if self.ready {
            self.rx = None;
        }
        changed
    }

    /// Paths relative to the root with `/` separators. Unordered while the
    /// walk runs, sorted once it's ready.
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    pub fn root(&self) -> &Path {
        &self.root
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
    buf: Vec<String>,
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

fn walk(root: &Path, tx: &Sender<Msg>, cancel: &AtomicBool, ctx: Option<&egui::Context>) {
    let (count, truncated) = (&AtomicUsize::new(0), &AtomicBool::new(false));
    WalkBuilder::new(root)
        .require_git(false) // honor .gitignore even outside a git repo
        .filter_entry(|e| e.file_name() != ".git")
        .build_parallel()
        .run(|| {
            let mut batch = Batcher { buf: Vec::with_capacity(BATCH), tx: tx.clone(), ctx };
            Box::new(move |entry| {
                if cancel.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else { return WalkState::Continue };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
                let Some(rel) = relative(root, entry.path()) else { return WalkState::Continue };
                if count.fetch_add(1, Ordering::Relaxed) >= FILE_LIMIT {
                    truncated.store(true, Ordering::Relaxed);
                    return WalkState::Quit;
                }
                batch.buf.push(rel);
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

    fn index(root: &Path) -> FileIndex {
        let mut f = FileIndex::default();
        f.start(root, None);
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
        assert_eq!(f.paths(), ["README.md", "src/main.rs", "src/nested/deep.rs"]);
        assert!(!f.is_truncated());
        assert_eq!(f.root(), root);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn restart_replaces_the_previous_listing() {
        let (a, b) = (temp_dir("a"), temp_dir("b"));
        fs::write(a.join("one.txt"), "").unwrap();
        fs::write(b.join("two.txt"), "").unwrap();
        let mut f = index(&a);
        assert_eq!(f.paths(), ["one.txt"]);
        f.start(&b, None);
        assert!(!f.is_ready());
        while !f.is_ready() {
            f.poll();
            std::thread::yield_now();
        }
        assert_eq!(f.paths(), ["two.txt"]);
        assert!(!f.poll());
        fs::remove_dir_all(a).ok();
        fs::remove_dir_all(b).ok();
    }
}
