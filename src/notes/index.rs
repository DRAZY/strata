//! The vault index: every file in a vault folder (for resolving links) and,
//! for each note, its links, tags, aliases, headings and block ids. Built on
//! a background thread; the window only ever swaps in a finished snapshot,
//! so typing never waits for it.
//!
//! Rescans are incremental: the folder is walked again (names and sizes
//! only) and just the notes whose size or modification time changed are
//! read again. That's how edits from Obsidian or a sync tool show up.

use super::config::Config;
use super::parse;
use super::resolve::{Files, is_note};
use crate::search::Stamp;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// Files listed per vault, at most.
pub const FILE_LIMIT: usize = 200_000;
/// Notes larger than this are listed (links to them work) but not read.
const NOTE_BYTES: u64 = 4 << 20;
/// Longest line kept as a link's context.
const CONTEXT_CHARS: usize = 240;

#[derive(Clone, Debug, PartialEq)]
pub struct LinkInfo {
    pub path: String,
    pub subpath: Option<String>,
    pub embed: bool,
    pub markdown: bool,
    /// 0-based line, and the line's text (shortened) for backlinks.
    pub line: u32,
    pub context: Box<str>,
}

/// What the index keeps of a note.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteInfo {
    pub links: Vec<LinkInfo>,
    /// (tag as written, line); frontmatter tags are on line 0.
    pub tags: Vec<(String, u32)>,
    pub aliases: Vec<String>,
    pub headings: Vec<String>,
    pub blocks: Vec<String>,
}

impl NoteInfo {
    pub fn from_text(text: &str) -> NoteInfo {
        let note = parse::parse(text);
        let lines: Vec<&str> = parse::lines(text).map(|(_, l)| l).collect();
        let context = |line: usize| -> Box<str> {
            let t = lines.get(line).map_or("", |l| l.trim());
            if t.chars().count() > CONTEXT_CHARS { format!("{}…", t.chars().take(CONTEXT_CHARS).collect::<String>()).into() } else { t.into() }
        };
        NoteInfo {
            links: note
                .links
                .iter()
                .map(|l| LinkInfo {
                    path: l.path.clone(),
                    subpath: l.subpath.clone(),
                    embed: l.embed,
                    markdown: l.kind == parse::LinkKind::Markdown,
                    line: l.line as u32,
                    context: context(l.line),
                })
                .collect(),
            tags: note.fm_tags.iter().map(|t| (t.clone(), 0)).chain(note.tags.iter().map(|t| (t.name.clone(), t.line as u32))).collect(),
            aliases: note.aliases,
            headings: note.headings.into_iter().map(|h| h.text).collect(),
            blocks: note.blocks.into_iter().map(|(id, _)| id).collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub stamp: Option<Stamp>,
    /// Notes only (None for attachments, and for notes too big to read).
    pub note: Option<Arc<NoteInfo>>,
}

/// A tag and where it's used: (note, line).
#[derive(Clone, Debug)]
pub struct TagUses {
    pub name: String,
    pub uses: Vec<(u32, u32)>,
}

/// One vault folder, as last scanned.
pub struct Vault {
    pub root: PathBuf,
    pub config: Config,
    pub files: Files,
    /// Same order as `files`.
    pub entries: Vec<Entry>,
    /// For each file, the links to it: (linking note, index in its `links`).
    backlinks: HashMap<u32, Vec<(u32, u32)>>,
    /// Sorted by name, ignoring case; tags differing only in case are one tag.
    tags: Vec<TagUses>,
    /// Conflict copies by original, and the reverse.
    copies: HashMap<u32, Vec<u32>>,
    copy_of: HashMap<u32, u32>,
    pub truncated: bool,
    /// Some note has a `[[wikilink]]`: the folder is used for notes.
    pub wikilinks: bool,
    /// Staging folders an interrupted undo left (see `swap::leftover`).
    pub leftovers: Vec<PathBuf>,
}

impl Vault {
    /// An empty vault (before the first scan finishes).
    pub fn empty(root: PathBuf) -> Vault {
        let config = Config::read(&root);
        Vault { root, config, files: Files::default(), entries: Vec::new(), backlinks: HashMap::new(), tags: Vec::new(), copies: HashMap::new(), copy_of: HashMap::new(), truncated: false, wikilinks: false, leftovers: Vec::new() }
    }

    /// `path` relative to the vault folder, with `/`.
    pub fn rel(&self, path: &Path) -> Option<String> {
        let rest = path.strip_prefix(&self.root).ok()?;
        let parts: Vec<String> = rest.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        (!parts.is_empty()).then(|| parts.join("/"))
    }

    /// The full path of vault-relative `rel`.
    pub fn abs(&self, rel: &str) -> PathBuf {
        rel.split('/').fold(self.root.clone(), |p, part| p.join(part))
    }

    pub fn path_of(&self, i: u32) -> PathBuf {
        self.abs(self.files.path(i))
    }

    pub fn note(&self, i: u32) -> Option<&NoteInfo> {
        self.entries.get(i as usize)?.note.as_deref()
    }

    pub fn find(&self, path: &Path) -> Option<u32> {
        self.files.find(&self.rel(path)?)
    }

    /// What `link` in the note at `from` points to.
    pub fn resolve(&self, link: &str, from: &Path) -> Option<u32> {
        let rel = self.rel(from);
        self.files.resolve(link, rel.as_deref())
    }

    pub fn backlinks(&self, i: u32) -> &[(u32, u32)] {
        self.backlinks.get(&i).map_or(&[], Vec::as_slice)
    }

    pub fn tags(&self) -> &[TagUses] {
        &self.tags
    }

    /// Conflict copies of file `i`.
    pub fn copies(&self, i: u32) -> &[u32] {
        self.copies.get(&i).map_or(&[], Vec::as_slice)
    }

    /// The file `i` is a conflict copy of.
    pub fn original_of(&self, i: u32) -> Option<u32> {
        self.copy_of.get(&i).copied()
    }

    /// Make the lookups from `files` and `entries`. With `reuse` (the
    /// previous snapshot of the same files, and which notes were read again)
    /// only those notes' links are resolved again: resolving every link is
    /// most of a rescan's work in a big vault.
    fn finish(root: PathBuf, config: Config, paths: Vec<String>, entries: Vec<Entry>, truncated: bool, reuse: Option<(&Vault, &[bool])>) -> Vault {
        let files = Files::new(paths);
        let mut backlinks: HashMap<u32, Vec<(u32, u32)>> = match reuse {
            Some((prev, changed)) => prev.backlinks.iter().map(|(&t, v)| (t, v.iter().filter(|(s, _)| !changed[*s as usize]).copied().collect())).collect(),
            None => HashMap::new(),
        };
        let mut tags: HashMap<String, TagUses> = HashMap::new();
        let mut wikilinks = false;
        for (i, e) in entries.iter().enumerate() {
            let Some(note) = &e.note else { continue };
            wikilinks |= note.links.iter().any(|l| !l.markdown);
            for (name, line) in &note.tags {
                tags.entry(name.to_lowercase()).or_insert_with(|| TagUses { name: name.clone(), uses: Vec::new() }).uses.push((i as u32, *line));
            }
            if reuse.is_some_and(|(_, changed)| !changed[i]) {
                continue;
            }
            let from = files.path(i as u32);
            for (k, l) in note.links.iter().enumerate() {
                // A note's links to its own headings aren't backlinks.
                if l.path.is_empty() {
                    continue;
                }
                if let Some(t) = files.resolve(&l.path, Some(from)) {
                    backlinks.entry(t).or_default().push((i as u32, k as u32));
                }
            }
        }
        if reuse.is_some() {
            // Same order as a full scan: by linking note, then link.
            backlinks.retain(|_, v| !v.is_empty());
            backlinks.values_mut().for_each(|v| v.sort_unstable());
        }
        let mut tags: Vec<TagUses> = tags.into_values().collect();
        tags.sort_by_key(|t| t.name.to_lowercase());
        let (mut copies, mut copy_of): (HashMap<u32, Vec<u32>>, HashMap<u32, u32>) = Default::default();
        for (i, rel) in files.paths().iter().enumerate() {
            let (dir, name) = rel.rsplit_once('/').map_or(("", rel.as_str()), |(d, n)| (d, n));
            let Some(original) = super::conflicts::original_name(name) else { continue };
            let original = if dir.is_empty() { original } else { format!("{dir}/{original}") };
            if let Some(o) = files.find(&original).filter(|&o| o != i as u32) {
                copies.entry(o).or_default().push(i as u32);
                copy_of.insert(i as u32, o);
            }
        }
        Vault { root, config, files, entries, backlinks, tags, copies, copy_of, truncated, wikilinks, leftovers: Vec::new() }
    }
}

/// What a scan found.
pub enum Outcome {
    New(Box<Vault>),
    /// Nothing changed since the snapshot it started from.
    Unchanged,
    Cancelled,
}

/// Walk `root` and read the notes that changed since `prev`.
pub fn scan(root: &Path, prev: Option<&Vault>, cancel: &AtomicBool) -> Outcome {
    let config = Config::read(root);
    // Obsidian shows every file but dot-folders (.obsidian, .trash, .git);
    // in other folders, what .gitignore ignores is left out too.
    let git = !config.obsidian;
    // Staging folders an interrupted undo left are noted (and not entered).
    let left = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let found_left = left.clone();
    let walk = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(git)
        .git_global(git)
        .git_exclude(git)
        .ignore(git)
        .parents(git)
        .require_git(false)
        .follow_links(false)
        .filter_entry(move |e| {
            let name = e.file_name().to_string_lossy();
            if e.depth() == 0 || !name.starts_with('.') {
                return true;
            }
            if e.file_type().is_some_and(|t| t.is_dir()) && name.starts_with(crate::claim::STAGING) && crate::swap::leftover(e.path()).is_some() {
                found_left.lock().unwrap_or_else(|p| p.into_inner()).push(e.path().to_path_buf());
            }
            false
        })
        .build();
    let mut found: Vec<(String, PathBuf, Option<Stamp>)> = Vec::new();
    let mut truncated = false;
    for entry in walk.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return Outcome::Cancelled;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if found.len() >= FILE_LIMIT {
            truncated = true;
            break;
        }
        let path = entry.into_path();
        let Ok(rest) = path.strip_prefix(root) else { continue };
        let rel = rest.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
        let stamp = Stamp::of(&path).ok();
        found.push((rel, path, stamp));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    let leftovers_now = left.lock().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(p) = prev
        && p.config == config
        && p.truncated == truncated
        && { let mut l = leftovers_now.clone(); l.sort(); l == p.leftovers }
        && p.files.len() == found.len()
        && found.iter().zip(p.files.paths()).zip(&p.entries).all(|((f, rel), e)| f.0 == *rel && f.2 == e.stamp)
    {
        return Outcome::Unchanged;
    }
    let previous: HashMap<&str, &Entry> = prev.map(|p| p.files.paths().iter().map(String::as_str).zip(&p.entries).collect()).unwrap_or_default();
    let mut entries: Vec<Entry> = Vec::with_capacity(found.len());
    let mut to_read: Vec<usize> = Vec::new();
    for (i, (rel, _, stamp)) in found.iter().enumerate() {
        let kept = previous.get(rel.as_str()).filter(|e| e.stamp == *stamp && stamp.is_some());
        let note = kept.and_then(|e| e.note.clone());
        if is_note(rel) && kept.is_none() && stamp.is_some_and(|s| s.len <= NOTE_BYTES) {
            to_read.push(i);
        }
        entries.push(Entry { stamp: *stamp, note });
    }
    // Reading is most of the work on a first scan: spread it over a few threads.
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8).min(to_read.len().max(1));
    let chunk = to_read.len().div_ceil(threads).max(1);
    let read: Vec<(usize, Option<NoteInfo>)> = std::thread::scope(|s| {
        let handles: Vec<_> = to_read
            .chunks(chunk)
            .map(|part| {
                let found = &found;
                s.spawn(move || {
                    part.iter()
                        .map_while(|&i| {
                            if cancel.load(Ordering::Relaxed) {
                                return None;
                            }
                            let info = crate::search::read_text(&found[i].1).ok().map(|f| NoteInfo::from_text(&f.text));
                            Some((i, info))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    if cancel.load(Ordering::Relaxed) {
        return Outcome::Cancelled;
    }
    let mut changed = vec![false; entries.len()];
    for (i, info) in read {
        entries[i].note = info.map(Arc::new);
        changed[i] = true;
    }
    let paths: Vec<String> = found.into_iter().map(|f| f.0).collect();
    let reuse = prev.filter(|p| p.config == config && p.files.paths() == paths.as_slice()).map(|p| (p, changed.as_slice()));
    let mut vault = Vault::finish(root.to_owned(), config, paths, entries, truncated, reuse);
    vault.leftovers = std::mem::take(&mut *left.lock().unwrap_or_else(|p| p.into_inner()));
    vault.leftovers.sort();
    Outcome::New(Box::new(vault))
}

struct Job {
    rx: Receiver<Outcome>,
    cancel: Arc<AtomicBool>,
    /// Scan again when this one ends (something changed meanwhile).
    again: bool,
}

/// The vaults a window uses, kept up to date in the background.
#[derive(Default)]
pub struct Index {
    vaults: HashMap<PathBuf, Arc<Vault>>,
    jobs: HashMap<PathBuf, Job>,
    /// Vaults scanned at least once.
    ready: std::collections::HashSet<PathBuf>,
    /// Bumped whenever a snapshot is swapped in.
    generation: u64,
}

impl Drop for Index {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Index {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn vault(&self, root: &Path) -> Option<&Arc<Vault>> {
        self.vaults.get(root)
    }

    /// The vault holding `path` (the innermost, if vault folders nest).
    pub fn vault_for(&self, path: &Path) -> Option<&Arc<Vault>> {
        self.vaults.iter().filter(|(r, _)| path.starts_with(r)).max_by_key(|(r, _)| r.components().count()).map(|(_, v)| v)
    }

    pub fn roots(&self) -> impl Iterator<Item = &PathBuf> {
        self.vaults.keys()
    }

    /// A scan of `root` is running.
    pub fn scanning(&self, root: &Path) -> bool {
        self.jobs.contains_key(root)
    }

    /// `root` has been scanned at least once (its snapshot is real, not empty).
    pub fn ready(&self, root: &Path) -> bool {
        self.ready.contains(root)
    }

    /// Scan `root` (again): it starts now, or right after the one running.
    pub fn request(&mut self, root: &Path, ctx: Option<&egui::Context>) {
        if let Some(job) = self.jobs.get_mut(root) {
            job.again = true;
            return;
        }
        self.vaults.entry(root.to_owned()).or_insert_with(|| Arc::new(Vault::empty(root.to_owned())));
        let prev = self.vaults.get(root).cloned();
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (root_owned, stop, ctx) = (root.to_owned(), cancel.clone(), ctx.cloned());
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let outcome = scan(&root_owned, prev.as_deref(), &stop);
            if let Outcome::New(v) = &outcome {
                log::info!("indexed {} ({} files) in {:?}", root_owned.display(), v.files.len(), started.elapsed());
            }
            let _ = tx.send(outcome);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        self.jobs.insert(root.to_owned(), Job { rx, cancel, again: false });
    }

    /// Take finished scans. True if a snapshot changed.
    pub fn poll(&mut self, ctx: Option<&egui::Context>) -> bool {
        let mut changed = false;
        let mut restart = Vec::new();
        self.jobs.retain(|root, job| match job.rx.try_recv() {
            Ok(outcome) => {
                if !matches!(outcome, Outcome::Cancelled) {
                    self.ready.insert(root.clone());
                }
                if let Outcome::New(v) = outcome {
                    self.vaults.insert(root.clone(), Arc::from(v));
                    changed = true;
                }
                if job.again {
                    restart.push(root.clone());
                }
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
        });
        for root in restart {
            self.request(&root, ctx);
        }
        if changed {
            self.generation += 1;
        }
        changed
    }

    /// Forget vaults that aren't among `roots` any more.
    pub fn retain(&mut self, roots: &[PathBuf]) {
        self.vaults.retain(|r, _| roots.contains(r));
        self.ready.retain(|r| roots.contains(r));
        self.jobs.retain(|r, job| {
            let keep = roots.contains(r);
            if !keep {
                job.cancel.store(true, Ordering::Relaxed);
            }
            keep
        });
    }

    /// Block until no scan is running (tests).
    #[cfg(test)]
    pub fn wait(&mut self) {
        while !self.jobs.is_empty() {
            self.poll(None);
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn temp_vault(tag: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-notes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, text) in files {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn scanned(root: &Path, prev: Option<&Vault>) -> Option<Vault> {
        match scan(root, prev, &AtomicBool::new(false)) {
            Outcome::New(v) => Some(*v),
            _ => None,
        }
    }

    #[test]
    fn backlinks_tags_and_conflict_copies() {
        let root = temp_vault(
            "index",
            &[
                ("A.md", "---\ntags: [project]\n---\nLinks to [[B]] and [[Sub/C|see C]] #Topic\n"),
                ("B.md", "Back to [[A]]. Also [b](Sub/C.md) and [[Nowhere]] #topic/sub #TOPIC\n"),
                ("Sub/C.md", "# C\nSelf link [[#C]] ![[pic.png]]\n"),
                ("Sub/pic.png", "png"),
                ("B (conflict).md", "older B\n"),
                (".obsidian/app.json", "{}"),
                (".trash/Old.md", "[[A]]"),
            ],
        );
        let v = scanned(&root, None).unwrap();
        assert!(v.config.obsidian);
        assert_eq!(v.files.paths(), ["A.md", "B (conflict).md", "B.md", "Sub/C.md", "Sub/pic.png"], "dot folders are left out");
        let id = |rel: &str| v.files.find(rel).unwrap();
        let from = |list: &[(u32, u32)]| list.iter().map(|&(n, _)| v.files.path(n).to_owned()).collect::<Vec<_>>();
        assert_eq!(from(v.backlinks(id("A.md"))), ["B.md"]);
        assert_eq!(from(v.backlinks(id("Sub/C.md"))), ["A.md", "B.md"], "wikilinks and Markdown links");
        assert_eq!(from(v.backlinks(id("Sub/pic.png"))), ["Sub/C.md"], "embeds count");
        let (n, k) = v.backlinks(id("A.md"))[0];
        let link = &v.note(n).unwrap().links[k as usize];
        assert_eq!((link.line, &*link.context), (0, "Back to [[A]]. Also [b](Sub/C.md) and [[Nowhere]] #topic/sub #TOPIC"));
        let tags: Vec<(&str, usize)> = v.tags().iter().map(|t| (t.name.as_str(), t.uses.len())).collect();
        assert_eq!(tags, [("project", 1), ("Topic", 2), ("topic/sub", 1)], "case-insensitive, like Obsidian");
        assert_eq!(v.copies(id("B.md")), [id("B (conflict).md")]);
        assert_eq!(v.original_of(id("B (conflict).md")), Some(id("B.md")));
        assert_eq!(v.rel(&root.join("Sub/C.md")).as_deref(), Some("Sub/C.md"));
        assert_eq!(v.abs("Sub/C.md"), root.join("Sub").join("C.md"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rescans_reuse_unchanged_notes_and_see_outside_edits() {
        let root = temp_vault("rescan", &[("A.md", "[[B]]\n"), ("B.md", "plain\n")]);
        let v1 = scanned(&root, None).unwrap();
        assert!(matches!(scan(&root, Some(&v1), &AtomicBool::new(false)), Outcome::Unchanged));
        // Another app edits B (a different size, so the change shows even
        // where mtimes are coarse) and adds C.
        std::fs::write(root.join("B.md"), "now links [[A]] and [[C]]\n").unwrap();
        std::fs::write(root.join("C.md"), "new\n").unwrap();
        let v2 = scanned(&root, Some(&v1)).unwrap();
        // Only B changes, the files stay the same: its links are resolved again, the rest reused.
        std::fs::write(root.join("B.md"), "links [[C]] only, and more text\n").unwrap();
        let v3 = scanned(&root, Some(&v2)).unwrap();
        let full = scanned(&root, None).unwrap();
        for (k, rel) in full.files.paths().iter().enumerate() {
            assert_eq!(v3.backlinks(k as u32), full.backlinks(k as u32), "{rel}");
        }
        assert!(v3.backlinks(v3.files.find("A.md").unwrap()).is_empty());
        let a = v1.files.find("A.md").unwrap();
        assert!(Arc::ptr_eq(v1.entries[a as usize].note.as_ref().unwrap(), v2.entries[v2.files.find("A.md").unwrap() as usize].note.as_ref().unwrap()), "A wasn't read again");
        assert_eq!(v2.backlinks(v2.files.find("A.md").unwrap()).len(), 1);
        assert_eq!(v2.backlinks(v2.files.find("C.md").unwrap()).len(), 1);
        assert_eq!(v3.backlinks(v3.files.find("C.md").unwrap()).len(), 1);
        // The background index swaps the snapshot in.
        let mut index = Index::default();
        index.request(&root, None);
        index.wait();
        let g = index.generation();
        assert_eq!(index.vault_for(&root.join("A.md")).unwrap().files.len(), 3);
        std::fs::remove_file(root.join("C.md")).unwrap();
        index.request(&root, None);
        index.wait();
        assert_eq!(index.generation(), g + 1);
        assert_eq!(index.vault(&root).unwrap().files.len(), 2);
        index.retain(&[]);
        assert!(index.vault(&root).is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn plain_folders_respect_gitignore() {
        let root = temp_vault("plain", &[("doc.md", "x"), ("node_modules/pkg/readme.md", "x"), (".gitignore", "node_modules/\n")]);
        let v = scanned(&root, None).unwrap();
        assert!(!v.config.obsidian);
        assert_eq!(v.files.paths(), ["doc.md"]);
        std::fs::remove_dir_all(root).ok();
    }
}
