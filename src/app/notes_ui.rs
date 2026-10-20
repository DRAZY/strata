//! Notes mode, the window half (see `crate::notes`): keeping the vault index
//! fresh, marking links and tags in the editor, following links (⌘/Ctrl-click,
//! Notes: Follow Link, the preview), `[[` completion, the Notes commands and
//! link updates after a rename. The backlinks panel, the dialogs and the
//! preview's embeds are in `notes_panel`.

use super::StrataApp;
use super::notes_panel::{Dialog, EmbedCache, LinkPlan, PlanFile};
use crate::commands::Cmd;
use crate::notes::config::{Config, NewNotes};
use crate::notes::index::{Index, Vault};
use crate::notes::marks::Mark;
use crate::notes::rename::{Rename, Rewriter};
use crate::notes::resolve::{is_note, without_md};
use crate::notes::{self, dates, parse};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Seconds between rescans of the vaults in use while the window has focus
/// (how soon edits from Obsidian or a sync tool show up).
const RESCAN_SECS: f64 = 3.0;
/// Notes longer than this (chars) get no marks or link completion.
const MARK_CHARS: usize = 2_000_000;
/// Above this many chars, a note's marks are made again at most every `LONG_NOTE_SECS`.
const LONG_NOTE: usize = 200_000;
const LONG_NOTE_SECS: f64 = 0.3;
/// Rows in the `[[` completion list.
const COMPLETIONS: usize = 40;

/// A palette list the Notes commands put up.
pub(super) enum NotesPick {
    Template(Vec<PathBuf>),
    /// Tags of the vault at this folder.
    Tag(PathBuf, Vec<String>),
    /// Where a tag is used: (note, 0-based line).
    TagUse(Vec<(PathBuf, usize)>),
}

/// What a rename changed, for Notes: Undo Rename. Files rewritten on disk
/// are kept by the disk job (`NotesState::disk`); the tabs are here.
pub(super) struct UndoRename {
    /// The vault: files are reached from it without following links.
    root: PathBuf,
    from: PathBuf,
    to: PathBuf,
    tabs: Vec<TabChange>,
    /// Files too large to keep for undo: this rename can't be undone.
    too_large: usize,
}

impl UndoRename {
    /// Can it be undone at all (every file's old text kept)?
    pub fn undoable(&self) -> bool {
        self.too_large == 0
    }
}

/// A note changed in its tab by a rename's link update.
struct TabChange {
    view: egui::Id,
    /// The tab's edit version right after.
    version: u64,
    path: PathBuf,
    /// The tab's text before and after the update.
    before: Arc<String>,
    after: Arc<String>,
    /// The file on disk then (size, fingerprint), to tell whether it was saved since.
    disk: Option<(u64, u64)>,
    /// The tab's encoding, for the bytes a save would have written.
    encoding: crate::encoding::Encoding,
}

/// `text` as saved in `enc` (None if it can't be).
fn encoded(text: &str, enc: crate::encoding::Encoding) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len());
    crate::encoding::encode_to(std::iter::once(text), enc, &mut out).ok()?;
    Some(out)
}

fn file_label(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn state_label(s: &crate::swap::State) -> String {
    match s {
        crate::swap::State::Restored => "restored".into(),
        crate::swap::State::Unchanged => "as it was".into(),
        crate::swap::State::KeptAt(p) => format!("kept at {}", p.display()),
    }
}

/// "Can't undo the rename; changed since: a.md (why), …".
fn refused_status(problems: &[(PathBuf, String)]) -> String {
    let list: Vec<String> = problems.iter().map(|(p, why)| format!("{} ({why})", file_label(p))).collect();
    format!("Can't undo the rename; nothing was changed. In the way: {}", list.join(", "))
}

/// `rel` (a path from Obsidian's settings) inside the vault at `root`; an
/// absolute or network path, `..`, or a folder linking elsewhere is refused.
pub(super) fn inside_vault(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let r = rel.trim();
    let outside = || format!("“{r}” is outside the vault");
    let drive = r.len() >= 2 && r.as_bytes()[1] == b':' && r.as_bytes()[0].is_ascii_alphabetic();
    if Path::new(r).is_absolute() || Path::new(r).has_root() || r.starts_with(['/', '\\']) || drive {
        return Err(outside());
    }
    let mut path = root.to_owned();
    for part in r.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return Err(outside()),
            p => path.push(p),
        }
    }
    confined(root, &path).map_err(|_| outside())?;
    Ok(path)
}

/// The closest existing folder of `path` is really inside `root` (no link
/// on the way leads out of the vault).
pub(super) fn confined(root: &Path, path: &Path) -> Result<(), String> {
    let real_root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut probe = path;
    while std::fs::symlink_metadata(probe).is_err() {
        probe = probe.parent().ok_or("no folder")?;
    }
    let real = std::fs::canonicalize(probe).map_err(|e| e.to_string())?;
    if real.starts_with(&real_root) { Ok(()) } else { Err(format!("{} is outside the vault", path.display())) }
}

/// One `[[` completion.
pub(super) struct Choice {
    pub label: String,
    pub detail: String,
    /// What goes between `[[` and `]]`.
    wiki: String,
    /// For Markdown links: (text, destination, already encoded).
    markdown: (String, String),
}

pub(super) struct Complete {
    pub view: egui::Id,
    /// Char position just after `[[`.
    start: usize,
    pub items: Vec<Choice>,
    pub selected: usize,
    /// (edit version, cursor) the list is for.
    key: (u64, usize),
    markdown_links: bool,
}

#[derive(Default)]
pub(super) struct NotesState {
    pub index: Index,
    /// Folders known to have `.obsidian` in them (or not).
    obsidian: HashMap<PathBuf, bool>,
    last_rescan: f64,
    focused: bool,
    /// Per editor view: the (edit version, index generation) its marks are for.
    marked: HashMap<egui::Id, (u64, u64)>,
    /// When each view's marks were last made (input time), to space them out in long notes.
    marked_at: HashMap<egui::Id, f64>,
    pub complete: Option<Complete>,
    pub dialog: Option<Dialog>,
    /// A rename's link updates on disk; it holds their undo too.
    pub disk: crate::replace::DiskReplace,
    /// Links and notes the running update changed in open tabs.
    disk_in_tabs: (usize, usize),
    pub last_rename: Option<UndoRename>,
    /// An Undo Rename whose files are being restored, and the tabs to change back after: (view, version).
    undoing: Option<(UndoRename, Vec<(egui::Id, u64)>)>,
    /// A link followed before the vault was indexed: (from, link, subpath).
    pending_follow: Option<(PathBuf, String, Option<String>)>,
    pub embeds: EmbedCache,
    /// `[[` candidates of a vault, for the index generation they were listed at.
    candidates: Option<(PathBuf, u64, Arc<Candidates>)>,
    /// Most bytes of originals a link update keeps for its undo (None: Replace in Files' limit).
    pub undo_limit: Option<usize>,
    /// Tests: runs between checking a new note's place and making it.
    #[cfg(test)]
    pub make_hook: Option<Box<dyn FnMut() + Send>>,
    /// For lists opened from a palette pick (which has no context at hand).
    ctx: Option<egui::Context>,
}

/// Is `rel` an image Strata shows (embeds, links)?
pub(super) fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"].iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// A name for a new note or a rename: one or more `/`-separated parts that
/// every system (and Obsidian) accepts.
pub(super) fn check_note_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Type a name".into());
    }
    for part in name.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err("Each folder and the name need some text (no empty parts, . or ..)".into());
        }
        if let Some(c) = part.chars().find(|c| matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()) {
            return Err(format!("A note name can't contain {c:?}"));
        }
        if part.ends_with([' ', '.']) || part.starts_with(' ') {
            return Err("A name can't start with a space or end with a space or a dot".into());
        }
    }
    Ok(())
}

/// Create `path` with `text`, making its folders; never replaces a file.
/// `path` as names of folders and the file below `root` (no `..`, no root).
fn parts_below(root: &Path, path: &Path) -> Option<Vec<String>> {
    path.strip_prefix(root).ok()?.components().map(|c| match c {
        std::path::Component::Normal(s) => s.to_str().map(str::to_owned),
        _ => None,
    }).collect::<Option<Vec<_>>>().filter(|p| !p.is_empty())
}

/// Create the note `path` inside the vault `root` with `text`, through
/// folders opened from the vault down without following links (`swap`):
/// a folder swapped for a link to elsewhere meanwhile is refused, and an
/// existing file is never replaced.
/// (`hook` runs between the checks and the first folder opened: tests.)
pub(super) fn create_note(root: &Path, path: &Path, text: &str, hook: &mut dyn FnMut()) -> std::io::Result<()> {
    let parts = parts_below(root, path).ok_or_else(|| std::io::Error::other("outside the vault"))?;
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    crate::swap::create_in_with(root, &parts, text.as_bytes(), &mut |_| hook())
}

/// The text of the file `path` inside `root`, read without following links on the way.
pub(super) fn read_vault_text(root: &Path, path: &Path) -> Result<String, String> {
    let parts = parts_below(root, path).ok_or("it's outside the vault")?;
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    let bytes = crate::swap::read_in(root, &parts).map_err(|e| match e.raw_os_error() {
        #[cfg(unix)]
        Some(libc::ELOOP) | Some(libc::ENOTDIR) => "a folder on the way is a link out of the vault".to_owned(),
        _ => e.to_string(),
    })?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err("it isn't text".into());
    }
    Ok(crate::encoding::decode_detect(&bytes).0.into_owned())
}

/// A link's last part as the index compares it: no `.md`, lowercase.
fn last_name(path: &str) -> String {
    without_md(path.rsplit('/').next().unwrap_or(path)).to_lowercase()
}

/// Text for a heading in a link: what would end the link is dropped, as Obsidian does.
fn heading_for_link(h: &str) -> String {
    h.chars().map(|c| if matches!(c, '[' | ']' | '|' | '#' | '^') { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

impl StrataApp {
    // ---- vaults ------------------------------------------------------------------

    fn has_obsidian(&mut self, dir: &Path) -> bool {
        if let Some(&known) = self.notes.obsidian.get(dir) {
            return known;
        }
        let yes = dir.join(".obsidian").is_dir();
        self.notes.obsidian.insert(dir.to_owned(), yes);
        yes
    }

    /// The vault folder of the file at `path`: the closest folder above it
    /// with `.obsidian` (also above the side bar folder holding it, when
    /// only part of a vault is open), else the side bar folder holding it.
    /// A file outside the side bar's folders is in a vault only when a
    /// folder above it has `.obsidian`.
    pub(super) fn vault_root(&mut self, path: &Path) -> Option<PathBuf> {
        let mut dir = path.parent();
        while let Some(d) = dir {
            if self.has_obsidian(d) {
                return Some(d.to_owned());
            }
            dir = d.parent();
        }
        self.sidebar.root_of(path).map(Path::to_owned)
    }

    /// Tab `i` is a note in a vault: its vault folder.
    pub(super) fn note_root(&mut self, i: usize) -> Option<PathBuf> {
        let d = self.docs.get(i)?;
        if d.binary.is_some() || d.compare.is_some() || !crate::markdown::is_markdown(d.path.as_deref(), d.highlighter.syntax_name()) {
            return None;
        }
        let path = d.path.clone()?;
        self.vault_root(&path)
    }

    /// The vault snapshot holding `path`, once it has been scanned.
    pub(super) fn vault_of(&mut self, path: &Path) -> Option<Arc<Vault>> {
        let root = self.vault_root(path)?;
        self.notes.index.ready(&root).then(|| self.notes.index.vault(&root).cloned()).flatten()
    }

    /// The vault Notes commands act on: the current note's, else the first
    /// side bar folder.
    fn command_root(&mut self) -> Option<PathBuf> {
        let from_doc = self.docs[self.active].path.clone().and_then(|p| self.vault_root(&p));
        // A side bar folder in a vault (or one): as for a file in it.
        let root = from_doc.or_else(|| self.sidebar.root().map(|r| r.join(".")).and_then(|p| self.vault_root(&p)));
        if root.is_none() {
            self.status = "Open your notes folder first (File ▸ Open Folder…)".into();
        }
        root
    }

    // ---- every frame ----------------------------------------------------------------

    /// Index upkeep, editor marks and link clicks (before the panes are drawn).
    pub(super) fn notes_frame(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let focused = ctx.input(|i| i.focused);
        self.notes.ctx = Some(ctx.clone());
        self.notes.index.poll(Some(ctx));
        self.notes_disk_poll(ctx);
        // The vaults of open notes are indexed; the others are let go.
        let mut roots: Vec<PathBuf> = Vec::new();
        for i in 0..self.docs.len() {
            if let Some(r) = self.note_root(i).filter(|r| !roots.contains(r)) {
                roots.push(r);
            }
        }
        if let Some(r) = self.notes.dialog.as_ref().and_then(Dialog::root).filter(|r| !roots.contains(r)) {
            roots.push(r);
        }
        if let Some(r) = self.notes.pending_follow.as_ref().map(|p| p.0.clone()).and_then(|p| self.vault_root(&p)).filter(|r| !roots.contains(r)) {
            roots.push(r);
        }
        self.notes.index.retain(&roots);
        let back = focused && !self.notes.focused;
        let periodic = focused && now - self.notes.last_rescan > RESCAN_SECS;
        for r in &roots {
            if self.notes.index.vault(r).is_none() || back || periodic {
                self.notes.index.request(r, Some(ctx));
            }
        }
        if back {
            // Back from another app: `.obsidian` may have come or gone.
            self.notes.obsidian.clear();
        }
        if periodic || back {
            self.notes.last_rescan = now;
        }
        self.notes.focused = focused;
        if roots.is_empty() {
            self.notes.complete = None;
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(RESCAN_SECS));
        // Marks for the notes on screen; link clicks they reported last frame.
        for g in 0..self.layout.groups() {
            let Some(i) = self.group_active(g) else { continue };
            self.update_marks(i);
            if let Some(pos) = self.docs[i].view.link_click.take() {
                self.activate(i);
                self.follow_link_at(i, pos);
            }
        }
        if let Some((from, link, sub)) = self.notes.pending_follow.clone()
            && self.vault_of(&from).is_some()
        {
            self.notes.pending_follow = None;
            self.follow_note_link(&from, &link, sub.as_deref());
        }
        self.plan_waiting_links();
    }

    /// Make tab `i`'s link and tag marks if its text or the index changed.
    fn update_marks(&mut self, i: usize) {
        let Some(root) = self.note_root(i) else {
            let d = &mut self.docs[i];
            if !d.view.note_marks.is_empty() {
                d.view.note_marks.clear();
            }
            self.notes.marked.remove(&d.view.id);
            return;
        };
        let generation = self.notes.index.generation();
        let vault = self.notes.index.vault(&root).cloned();
        let ready = self.notes.index.ready(&root);
        let d = &mut self.docs[i];
        let key = (d.editor.version(), generation);
        if self.notes.marked.get(&d.view.id) == Some(&key) {
            return;
        }
        // A long note is read again at most a few times a second while
        // typing; lines edited meanwhile just show no marks (see `NoteMarks`).
        let now = self.notes.ctx.as_ref().map_or(0.0, |c| c.input(|i| i.time));
        let last = self.notes.marked_at.get(&d.view.id).copied().unwrap_or(f64::NEG_INFINITY);
        if d.editor.buffer.len_chars() > LONG_NOTE && now - last < LONG_NOTE_SECS {
            if let Some(ctx) = &self.notes.ctx {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(LONG_NOTE_SECS));
            }
            return;
        }
        self.notes.marked_at.insert(d.view.id, now);
        self.notes.marked.insert(d.view.id, key);
        d.view.note_marks.clear();
        if d.editor.buffer.len_chars() > MARK_CHARS {
            return;
        }
        let text = d.editor.text_snapshot();
        let note = parse::parse(&text);
        let from = d.path.clone().unwrap_or_default();
        let starts: Vec<(usize, &str)> = parse::lines(&text).collect();
        let mut by_line: HashMap<usize, Vec<(Range<usize>, Mark)>> = HashMap::new();
        let mut push = |line: usize, range: &Range<usize>, mark: Mark| {
            let Some(&(start, _)) = starts.get(line) else { return };
            let a = text[start..range.start].chars().count();
            let b = a + text[range.clone()].chars().count();
            by_line.entry(line).or_default().push((a..b, mark));
        };
        for l in &note.links {
            // Before the first scan nothing is known to be missing.
            let found = l.path.is_empty() || !ready || vault.as_ref().is_some_and(|v| v.resolve(&l.path, &from).is_some());
            // A link spanning lines can't happen (links end at the line), but be safe.
            push(l.line, &l.range, if found { Mark::Link } else { Mark::Broken });
        }
        for t in &note.tags {
            push(t.line, &t.range, Mark::Tag);
        }
        for (line, marks) in by_line {
            if let Some(&(_, text)) = starts.get(line) {
                d.view.note_marks.insert(line, text, marks);
            }
        }
    }

    // ---- following links -------------------------------------------------------

    /// The link at char `pos` of tab `i`, followed.
    fn follow_link_at(&mut self, i: usize, pos: usize) {
        let d = &mut self.docs[i];
        let Some(from) = d.path.clone() else { return };
        let text = d.editor.text_snapshot();
        let byte = d.editor.buffer.char_to_byte(pos.min(d.editor.buffer.len_chars()));
        let links = parse::parse(&text).links;
        // The cursor right after a link (`[[a]]|`) counts too.
        let Some(link) = links.iter().find(|l| l.range.start <= byte && byte < l.range.end).or_else(|| links.iter().find(|l| l.range.end == byte)).cloned() else {
            self.status = "No link at the cursor".into();
            return;
        };
        self.follow_note_link(&from, &link.path, link.subpath.as_deref());
    }

    /// Notes: Follow Link: the link under the cursor.
    fn follow_link_under_cursor(&mut self) {
        if self.note_root(self.active).is_none() {
            self.status = "Follow Link works in notes inside an open folder".into();
            return;
        }
        let head = self.docs[self.active].editor.primary().head;
        self.follow_link_at(self.active, head);
    }

    /// Open what `link` (`#subpath` apart) in the note at `from` points to,
    /// or offer to create it.
    pub(super) fn follow_note_link(&mut self, from: &Path, link: &str, subpath: Option<&str>) {
        let Some(root) = self.vault_root(from) else {
            self.status = format!("{} isn't in an open folder, so its links can't be followed", from.display());
            return;
        };
        if link.trim().is_empty() {
            if let Some(sub) = subpath {
                self.push_jump();
                self.goto_subpath(sub);
            }
            return;
        }
        // A full path to a file (a Markdown link written that way) opens as it is.
        if Path::new(link).is_absolute() && Path::new(link).is_file() {
            self.push_jump();
            return self.open_linked(PathBuf::from(link), subpath);
        }
        let Some(vault) = self.vault_of(from) else {
            self.notes.index.request(&root, None);
            self.notes.pending_follow = Some((from.to_owned(), link.to_owned(), subpath.map(str::to_owned)));
            self.status = "Indexing notes…".into();
            return;
        };
        match vault.resolve(link, from) {
            Some(t) => {
                let path = vault.path_of(t);
                self.push_jump();
                self.open_linked(path, subpath);
            }
            None => {
                let ext = Path::new(link).extension().and_then(|e| e.to_str()).unwrap_or("");
                if !ext.is_empty() && !is_note(link) {
                    self.status = format!("“{link}” isn't in the vault");
                    return;
                }
                match self.new_note_path(&root, &vault.config, Some(from), link) {
                    Ok(path) => self.notes.dialog = Some(Dialog::Create { root, path, link: link.to_owned() }),
                    Err(e) => self.status = format!("Can't create “{link}”: {e}"),
                }
            }
        }
    }

    /// Open a linked file: text and images in a tab (at `subpath`), anything
    /// else shown in the file manager rather than launched.
    fn open_linked(&mut self, path: PathBuf, subpath: Option<&str>) {
        if !is_image(&path)
            && let crate::markdown::Action::Reveal(p) = crate::markdown::local_link(&path)
        {
            self.status = match crate::ctxmenu::reveal_in_file_manager(&p) {
                Ok(()) => format!("Not a text file, so Strata showed it instead of opening it: {}", p.display()),
                Err(e) => format!("Could not open the file manager: {e}"),
            };
            return;
        }
        self.open_path(path.clone(), false);
        if self.docs[self.active].path.as_ref() == Some(&path)
            && let Some(sub) = subpath
        {
            self.goto_subpath(sub);
        }
    }

    /// Go to a heading (`a#b` for nested ones: the last) or `^block` in the current tab.
    fn goto_subpath(&mut self, sub: &str) {
        let d = &mut self.docs[self.active];
        let text = d.editor.text_snapshot();
        let note = parse::parse(&text);
        let line = match sub.strip_prefix('^') {
            Some(id) => note.blocks.iter().find(|(b, _)| b.eq_ignore_ascii_case(id)).map(|(_, l)| *l),
            None => {
                let want = parse::heading_key(sub.rsplit('#').next().unwrap_or(sub));
                note.headings.iter().find(|h| parse::heading_key(&h.text) == want).map(|h| h.line)
            }
        };
        match line {
            Some(l) => self.goto_line(l),
            None => self.status = format!("No {} “{sub}” in {}", if sub.starts_with('^') { "block" } else { "heading" }, d.name()),
        }
    }

    /// Where a note for `link` goes: a path in the link is from the vault
    /// folder; a bare name goes where Obsidian's new-note setting says.
    pub(super) fn new_note_path(&mut self, root: &Path, config: &Config, from: Option<&Path>, link: &str) -> Result<PathBuf, String> {
        let name = link.trim().trim_start_matches('/');
        check_note_name(name)?;
        let dir = if name.contains('/') {
            root.to_owned()
        } else {
            match &config.new_notes {
                NewNotes::Root => root.to_owned(),
                NewNotes::Current => from.and_then(Path::parent).filter(|p| p.starts_with(root)).map_or_else(|| root.to_owned(), Path::to_owned),
                NewNotes::Folder(f) => f.split('/').fold(root.to_owned(), |p, part| p.join(part)),
            }
        };
        let file = if is_note(name) { name.to_owned() } else { format!("{name}.md") };
        let path = file.split('/').fold(dir, |p, part| p.join(part));
        if !path.starts_with(root) {
            return Err("it would be outside the vault".into());
        }
        // Not through a folder that links out of the vault either.
        confined(root, &path).map_err(|_| "it would be outside the vault".to_owned())?;
        Ok(path)
    }

    /// Make the note (empty, or `text`) and open it.
    pub(super) fn create_and_open(&mut self, root: &Path, path: &Path, text: &str) {
        if let Err(e) = confined(root, path) {
            self.status = format!("Not created: {e}");
            return;
        }
        #[cfg(test)]
        let mut hook = self.notes.make_hook.take();
        #[cfg(not(test))]
        let mut hook: Option<Box<dyn FnMut() + Send>> = None;
        let made = create_note(root, path, text, &mut || {
            if let Some(h) = hook.as_mut() {
                h();
            }
        });
        match made {
            Ok(()) => {
                self.notes.index.request(root, None);
                self.sidebar.refresh();
                self.open_path(path.to_owned(), false);
                self.status = format!("Created {}", path.display());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => self.open_path(path.to_owned(), false),
            Err(e) => {
                log::warn!("couldn't create {}: {e}", path.display());
                let why = match e.raw_os_error() {
                    #[cfg(unix)]
                    Some(libc::ELOOP) | Some(libc::ENOTDIR) => "a folder on the way is a link, so it would be outside the vault".to_owned(),
                    _ => e.to_string(),
                };
                self.status = format!("Could not create {}: {why}", path.display());
            }
        }
    }

    // ---- [[ completion -------------------------------------------------------------

    /// The completion list's keys, before the editor sees them. True if one was used.
    pub(super) fn notes_complete_keys(&mut self, ctx: &egui::Context) -> bool {
        // A dialog has the keyboard.
        if self.modal_open(ctx) {
            return false;
        }
        let id = self.docs[self.active].view.id;
        let Some(c) = self.notes.complete.as_mut().filter(|c| c.view == id && !c.items.is_empty()) else { return false };
        let (up, down, accept, esc) = ctx.input_mut(|i| {
            use egui::{Key, Modifiers};
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        let n = c.items.len();
        if up {
            c.selected = (c.selected + n - 1) % n;
        }
        if down {
            c.selected = (c.selected + 1) % n;
        }
        if accept {
            let k = c.selected;
            self.notes_accept(k);
        }
        if esc && let Some(c) = self.notes.complete.as_mut() {
            // Closed until the text or cursor moves.
            c.items.clear();
        }
        up || down || accept || esc
    }

    /// The `[[` completion list is up.
    pub(super) fn notes_completing(&self) -> bool {
        self.notes.complete.as_ref().is_some_and(|c| c.view == self.docs[self.active].view.id && !c.items.is_empty())
    }

    /// After an edit or a cursor move: open, refresh or close the `[[` list.
    pub(super) fn notes_complete_update(&mut self) {
        let Some(root) = self.note_root(self.active) else {
            self.notes.complete = None;
            return;
        };
        let d = &mut self.docs[self.active];
        let key = (d.editor.version(), d.editor.primary().head);
        if self.notes.complete.as_ref().is_some_and(|c| c.view == d.view.id && c.key == key) {
            return;
        }
        let view = d.view.id;
        // "No list here", remembered for this text and cursor.
        let close = |s: &mut Self| s.notes.complete = Some(Complete { view, start: 0, items: Vec::new(), selected: 0, key, markdown_links: false });
        if d.editor.selections().len() != 1 || !d.editor.primary().is_empty() || d.editor.buffer.len_chars() > MARK_CHARS {
            return close(self);
        }
        let head = key.1;
        let line = d.editor.buffer.line_of(head);
        let line_start = d.editor.buffer.line_start(line);
        let before = d.editor.buffer.slice(line_start..head);
        let Some(open) = before.rfind("[[") else { return close(self) };
        // The list opens as you type; moving the cursor into a link doesn't open it.
        let showing = self.notes.complete.as_ref().is_some_and(|c| c.view == view && !c.items.is_empty());
        let typed = self.notes.ctx.as_ref().is_some_and(|c| c.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_) | egui::Event::Paste(_)))));
        if !showing && !typed {
            return close(self);
        }
        let query = &before[open + 2..];
        // Already closed, an alias being typed, or inside inline code: no list.
        if query.contains("]]") || query.contains('|') || before[..open].matches('`').count() % 2 == 1 {
            return close(self);
        }
        let start = line_start + before[..open].chars().count() + 2;
        let text = d.editor.text_snapshot();
        let byte = d.editor.buffer.char_to_byte(start);
        if parse::in_code_block(&text, byte) {
            return close(self);
        }
        let from = d.path.clone().unwrap_or_default();
        let query = query.to_owned();
        let Some(vault) = self.notes.index.vault(&root).cloned() else { return close(self) };
        let generation = self.notes.index.generation();
        let cands = match &self.notes.candidates {
            Some((r, g, c)) if *r == root && *g == generation => c.clone(),
            _ => {
                let c = Arc::new(Candidates::of(&vault));
                self.notes.candidates = Some((root, generation, c.clone()));
                c
            }
        };
        let items = completions(&vault, &cands, &from, &query, &text);
        self.notes.complete = Some(Complete { view, start, items, selected: 0, key, markdown_links: vault.config.markdown_links });
    }

    /// Insert completion `k`: `[[link]]`, or a Markdown link in a vault set to use them.
    pub(super) fn notes_accept(&mut self, k: usize) {
        let Some(c) = self.notes.complete.take() else { return };
        let Some(item) = c.items.get(k) else { return };
        let d = &mut self.docs[self.active];
        let head = d.editor.primary().head;
        let len = d.editor.buffer.len_chars();
        // Only for the text and cursor the list was made for.
        if d.view.id != c.view || (d.editor.version(), head) != c.key || head < c.start || c.start < 2 {
            return;
        }
        // The `]]` auto-closing put after the cursor goes too.
        let closer = head + 2 <= len && d.editor.buffer.slice(head..head + 2) == "]]";
        let end = if closer { head + 2 } else { head };
        let (range, text) = if c.markdown_links {
            (c.start - 2..end, format!("[{}]({})", item.markdown.0, item.markdown.1))
        } else {
            (c.start..end, format!("{}]]", item.wiki))
        };
        d.editor.select_ranges(&[range], 0);
        d.editor.insert_text(&text);
        d.view.reveal();
        let head = d.editor.primary().head;
        // Don't reopen on the link just inserted.
        self.notes.complete = Some(Complete { view: c.view, start: c.start, items: Vec::new(), selected: 0, key: (d.editor.version(), head), markdown_links: c.markdown_links });
    }

    // ---- commands -------------------------------------------------------------------

    pub(super) fn notes_command(&mut self, ctx: &egui::Context, cmd: Cmd) {
        match cmd {
            Cmd::NotesDailyNote => self.open_daily_note(),
            Cmd::NotesNewNote => {
                if let Some(root) = self.command_root() {
                    self.notes.dialog = Some(Dialog::name(root, super::notes_panel::Naming::New, "Untitled"));
                }
            }
            Cmd::NotesNewFromTemplate => self.choose_template(ctx),
            Cmd::NotesFindTag => self.choose_tag(ctx, None),
            Cmd::NotesRename => self.ask_rename_note(),
            Cmd::NotesToggleBacklinks => {
                let shown = self.backlinks_shown();
                self.settings.notes_backlinks = if shown { "never" } else { "always" }.into();
                self.persist_settings();
            }
            Cmd::NotesFollowLink => self.follow_link_under_cursor(),
            Cmd::NotesUndoRename => self.undo_rename(ctx),
            Cmd::NotesCompareConflict => self.compare_conflict(None),
            _ => {}
        }
    }

    pub(super) fn notes_chose(&mut self, pick: NotesPick, i: usize) {
        match pick {
            NotesPick::Template(paths) => {
                let (Some(t), Some(root)) = (paths.get(i).cloned(), self.command_root()) else { return };
                self.notes.dialog = Some(Dialog::name(root, super::notes_panel::Naming::FromTemplate(t), "Untitled"));
            }
            NotesPick::Tag(root, tags) => {
                if let Some(tag) = tags.get(i) {
                    self.show_tag_uses(&root, tag);
                }
            }
            NotesPick::TagUse(spots) => {
                if let Some((p, line)) = spots.get(i).cloned() {
                    self.push_jump();
                    self.open_path(p, false);
                    self.goto_line(line);
                }
            }
        }
    }

    /// Notes: Open Daily Note: today's note, made from the daily template if new.
    fn open_daily_note(&mut self) {
        let Some(root) = self.command_root() else { return };
        let config = Config::read(&root);
        let now = dates::DateTime::now();
        let name = dates::format(&config.daily.format, &now);
        if let Err(e) = check_note_name(&name) {
            self.status = format!("The daily note format “{}” makes an unusable name: {e}", config.daily.format);
            return;
        }
        let rel = if config.daily.folder.is_empty() { name.clone() } else { format!("{}/{name}", config.daily.folder) };
        let path = match inside_vault(&root, &format!("{rel}.md")) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("Can't make the daily note: {e}");
                return;
            }
        };
        if path.exists() {
            self.push_jump();
            return self.open_path(path, false);
        }
        let title = name.rsplit('/').next().unwrap_or(&name).to_owned();
        let text = match self.read_template(&root, &config.daily.template) {
            Ok(t) => notes::fill_template(&t, &title, &now, &config.daily.format, &config.templates.time_format),
            Err(e) => {
                self.status = e;
                return;
            }
        };
        self.push_jump();
        self.create_and_open(&root, &path, &text);
    }

    /// A template's text (vault-relative, `.md` optional); empty for none.
    fn read_template(&mut self, root: &Path, template: &str) -> Result<String, String> {
        let written = template.trim();
        // Obsidian may write `/Templates/Day` for a path from the vault folder;
        // any other absolute path is outside the vault.
        let t = written.strip_prefix('/').filter(|_| !cfg!(windows)).unwrap_or(written);
        if t.is_empty() {
            return Ok(String::new());
        }
        let rel = if is_note(t) { t.to_owned() } else { format!("{t}.md") };
        let path = inside_vault(root, &rel).map_err(|e| format!("Couldn't use the template: {e}"))?;
        if t.len() != written.len() && !path.is_file() {
            return Err(format!("Couldn't use the template: “{written}” is outside the vault"));
        }
        read_vault_text(root, &path).map_err(|e| format!("Couldn't read the template {}: {e}", path.display()))
    }

    /// The templates folder: Obsidian's setting, else `Templates`.
    fn templates_dir(root: &Path, config: &Config) -> Result<PathBuf, String> {
        let folder = config.templates.folder.clone().unwrap_or_else(|| "Templates".into());
        inside_vault(root, &folder)
    }

    fn choose_template(&mut self, ctx: &egui::Context) {
        let Some(root) = self.command_root() else { return };
        let config = Config::read(&root);
        let dir = match Self::templates_dir(&root, &config) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("Can't use the templates folder: {e}");
                return;
            }
        };
        let mut found = Vec::new();
        let mut stack = vec![(dir.clone(), 0)];
        while let Some((d, depth)) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                if p.is_dir() && depth < 4 {
                    stack.push((p, depth + 1));
                } else if is_note(&name) && found.len() < 1000 {
                    found.push(p);
                }
            }
        }
        found.sort();
        if found.is_empty() {
            self.status = format!("No templates in {} (Obsidian's Templates setting, or a Templates folder)", dir.display());
            return;
        }
        let labels = found.iter().map(|p| p.strip_prefix(&dir).unwrap_or(p).with_extension("").display().to_string()).collect();
        self.choose(ctx, super::more::Choosing::Notes(NotesPick::Template(found)), "New note from template", labels);
    }

    /// Notes: Find Tag… (or a tag clicked in the preview: `tag`).
    pub(super) fn choose_tag(&mut self, ctx: &egui::Context, tag: Option<&str>) {
        let Some(root) = self.command_root() else { return };
        self.notes.index.request(&root, Some(ctx));
        if !self.notes.index.ready(&root) {
            self.status = "Indexing notes… try again in a moment".into();
            return;
        }
        let Some(vault) = self.notes.index.vault(&root).cloned() else { return };
        if let Some(t) = tag {
            return self.show_tag_uses(&root, t);
        }
        if vault.tags().is_empty() {
            self.status = "No tags in these notes yet".into();
            return;
        }
        let labels = vault
            .tags()
            .iter()
            .map(|t| {
                let notes = t.uses.iter().map(|u| u.0).collect::<std::collections::HashSet<_>>().len();
                format!("#{}  ({notes} note{})", t.name, if notes == 1 { "" } else { "s" })
            })
            .collect();
        let tags = vault.tags().iter().map(|t| t.name.clone()).collect();
        self.choose(ctx, super::more::Choosing::Notes(NotesPick::Tag(root, tags)), "Find tag", labels);
    }

    fn show_tag_uses(&mut self, root: &Path, tag: &str) {
        let Some(vault) = self.notes.index.vault(root).cloned() else { return };
        let Some(t) = vault.tags().iter().find(|t| t.name.eq_ignore_ascii_case(tag)) else {
            self.status = format!("No notes are tagged #{tag}");
            return;
        };
        let spots: Vec<(PathBuf, usize)> = t.uses.iter().map(|&(n, line)| (vault.path_of(n), line as usize)).collect();
        if let [(p, line)] = spots.as_slice() {
            self.push_jump();
            self.open_path(p.clone(), false);
            return self.goto_line(*line);
        }
        let labels = t.uses.iter().map(|&(n, line)| format!("{}  ·  line {}", without_md(vault.files.path(n)), line + 1)).collect();
        let ctx = self.notes.ctx.clone();
        match ctx {
            Some(ctx) => self.choose(&ctx, super::more::Choosing::Notes(NotesPick::TagUse(spots)), &format!("Notes tagged #{}", t.name), labels),
            None => self.status = format!("{} notes are tagged #{}", spots.len(), t.name),
        }
    }

    /// Notes: Compare with Conflict Copy (`which`: a copy, else the first).
    pub(super) fn compare_conflict(&mut self, which: Option<PathBuf>) {
        let Some(path) = self.docs[self.active].path.clone() else { return };
        let Some(vault) = self.vault_of(&path) else {
            self.status = "No sync conflict copies found for this file".into();
            return;
        };
        let Some(i) = vault.find(&path) else { return };
        let other = which.or_else(|| vault.copies(i).first().map(|&c| vault.path_of(c))).or_else(|| vault.original_of(i).map(|o| vault.path_of(o)));
        match other {
            Some(p) => self.compare_with_file(self.active, &p),
            None => self.status = "No sync conflict copies found for this file".into(),
        }
    }

    // ---- rename -----------------------------------------------------------------------

    fn ask_rename_note(&mut self) {
        let d = &self.docs[self.active];
        let Some(path) = d.path.clone() else {
            self.status = "Save the note before renaming it".into();
            return;
        };
        let Some(root) = self.vault_root(&path) else {
            self.status = "Rename Note works on files inside an open folder".into();
            return;
        };
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.notes.dialog = Some(Dialog::name(root, super::notes_panel::Naming::Rename(path), &stem));
    }

    /// Rename `from` to the name typed (same folder); then offer link updates.
    pub(super) fn rename_note(&mut self, from: &Path, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.contains('/') {
            return Err("A new name can't contain /".into());
        }
        check_note_name(name)?;
        let ext = from.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let file = if ext.is_empty() || name.to_lowercase().ends_with(&format!(".{}", ext.to_lowercase())) { name.to_owned() } else { format!("{name}.{ext}") };
        let to = from.with_file_name(&file);
        if to == from {
            return Ok(());
        }
        let same = std::fs::canonicalize(&to).ok().is_some_and(|t| std::fs::canonicalize(from).ok() == Some(t));
        if std::fs::symlink_metadata(&to).is_ok() && !same {
            return Err(format!("“{file}” already exists"));
        }
        crate::sidebar::rename(from, &to).map_err(|e| format!("Couldn't rename: {e}"))?;
        self.follow_rename(from, &to);
        self.link.requests.renamed.push((from.to_owned(), to.clone()));
        self.status = format!("Renamed to {}", to.display());
        self.notes_renamed(from, &to);
        Ok(())
    }

    /// A note was saved: its vault's index picks the change up now.
    pub(super) fn notes_saved(&mut self, path: &Path) {
        if let Some(root) = self.vault_root(path)
            && self.notes.index.vault(&root).is_some()
        {
            let ctx = self.notes.ctx.clone();
            self.notes.index.request(&root, ctx.as_ref());
        }
    }

    /// A file was renamed (here or in the side bar): offer to update links to it.
    pub(super) fn notes_renamed(&mut self, from: &Path, to: &Path) {
        // Undo Rename is for the last rename only: an older one's undo would
        // restore links to a name that has moved on.
        if !self.notes.disk.running() {
            self.notes.last_rename = None;
            self.notes.disk = crate::replace::DiskReplace::default();
        }
        if to.is_dir() {
            return;
        }
        let Some(root) = self.vault_root(to) else { return };
        if !from.starts_with(&root) {
            return;
        }
        // In a folder Strata hasn't indexed, only a note's rename is worth a look.
        if self.notes.index.vault(&root).is_none() && !is_note(&to.to_string_lossy()) && !self.has_obsidian(&root) {
            return;
        }
        let rel = |p: &Path| p.strip_prefix(&root).ok().map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"));
        let (Some(from_rel), Some(to_rel)) = (rel(from), rel(to)) else { return };
        self.notes.index.request(&root, None);
        self.notes.dialog = Some(Dialog::Links(LinkPlan::waiting(root, from_rel, to_rel)));
        self.plan_waiting_links();
    }

    /// Work out the link updates of a rename once its vault is indexed.
    fn plan_waiting_links(&mut self) {
        let limit = self.notes.undo_limit.unwrap_or(crate::replace::UNDO_BYTES);
        let Some(Dialog::Links(plan)) = &mut self.notes.dialog else { return };
        if let Some(rx) = &plan.reading {
            let Ok((files, skipped)) = rx.try_recv() else { return };
            let rename = plan.rename.clone().expect("set when reading starts");
            plan.ready(rename, files, skipped, limit);
            if plan.is_empty() {
                let to = rename_path(&plan.root, plan.rename_to());
                let skipped = plan_skipped_note(plan);
                self.notes.dialog = None;
                self.status = format!("Renamed to {} (no links to update{skipped})", to.display());
            }
            return;
        }
        if !plan.waiting || !self.notes.index.ready(&plan.root) || self.notes.index.scanning(&plan.root) {
            return;
        }
        let (root, from_rel, to_rel) = (plan.root.clone(), plan.rename_from().to_owned(), plan.rename_to().to_owned());
        let Some(vault) = self.notes.index.vault(&root).cloned() else { return };
        let rename = Arc::new(Rename::new(vault.files.paths(), &from_rel, &to_rel, vault.config.link_format));
        let names = rename.names();
        // Notes with a link ending in either name, and the renamed note (its links to itself).
        let mut candidates: Vec<String> = vault
            .files
            .paths()
            .iter()
            .zip(&vault.entries)
            .filter(|(_, e)| e.note.as_ref().is_some_and(|n| n.links.iter().any(|l| names.contains(&last_name(&l.path)))))
            .map(|(rel, _)| if *rel == from_rel { to_rel.clone() } else { rel.clone() })
            .collect();
        if is_note(&to_rel) {
            candidates.push(to_rel.clone());
        }
        // Open notes too: their unsaved links aren't in the index.
        candidates.extend(self.docs.iter().filter_map(|d| vault.rel(d.path.as_deref()?)).filter(|r| is_note(r)));
        candidates.sort();
        candidates.dedup();
        // Open tabs' text now (it's theirs that changes); the rest is read off the UI thread.
        let work: Vec<(String, PathBuf, Option<Arc<String>>)> = candidates
            .into_iter()
            .map(|rel| {
                let path = vault.abs(&rel);
                let tab = self.docs.iter_mut().find(|d| d.path.as_deref() == Some(path.as_path()) && d.compare.is_none()).map(|d| d.editor.text_snapshot());
                (rel, path, tab)
            })
            .collect();
        let (tx, rx) = std::sync::mpsc::channel();
        let (r, ctx) = (rename.clone(), self.notes.ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(plan_files(&r, work));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        let Some(Dialog::Links(plan)) = &mut self.notes.dialog else { return };
        plan.rename = Some(rename);
        plan.reading = Some(rx);
    }

    /// Make the planned link updates: open tabs in their buffer (one undo
    /// step each, left unsaved), the rest on disk through Replace in Files'
    /// job (atomic writes, originals kept for Notes: Undo Rename).
    pub(super) fn apply_link_plan(&mut self, plan: LinkPlan, ctx: Option<egui::Context>) {
        let Some(rename) = plan.rename.clone() else { return };
        // Two jobs rewriting the same files at once could undo each other.
        if self.notes.disk.running() {
            self.status = "Still updating links from the last rename; links weren't updated for this one".into();
            return;
        }
        let rewriter = Arc::new(Rewriter { root: plan.root.clone(), rename: (*rename).clone() });
        let mut tabs: Vec<TabChange> = Vec::new();
        let mut jobs = Vec::new();
        let (mut in_tabs, mut tab_files) = (0, 0);
        for f in &plan.files {
            let tab = self.docs.iter().position(|d| d.path.as_deref() == Some(f.path.as_path()) && d.compare.is_none());
            match tab {
                Some(i) => {
                    let disk = crate::swap::identity(&f.path);
                    let d = &mut self.docs[i];
                    let text = d.editor.text_snapshot();
                    let edits = crate::replace::Rewrite::edits(rewriter.as_ref(), &f.path, &text);
                    if edits.is_empty() {
                        continue;
                    }
                    let b = &d.editor.buffer;
                    let char_edits = edits.into_iter().map(|(r, s)| (b.byte_to_char(r.start)..b.byte_to_char(r.end), s)).collect::<Vec<_>>();
                    in_tabs += char_edits.len();
                    tab_files += 1;
                    d.editor.replace_ranges(char_edits);
                    d.preview = false;
                    let after = d.editor.text_snapshot();
                    tabs.push(TabChange { view: d.view.id, version: d.editor.version(), path: f.path.clone(), before: text, after, disk, encoding: d.encoding });
                }
                None => jobs.push(crate::replace::Job { path: f.path.clone(), stamp: f.stamp, expected: f.count }),
            }
        }
        let root = plan.root.clone();
        // The tabs' texts count toward the undo's memory too.
        let limit = self.notes.undo_limit.unwrap_or(crate::replace::UNDO_BYTES);
        let tab_bytes: usize = tabs.iter().map(|t| t.before.len() + t.after.len()).sum();
        let too_large = if tab_bytes > limit { plan.too_large.max(tabs.len()) } else { plan.too_large };
        self.notes.last_rename = Some(UndoRename { root: root.clone(), from: rename_path(&root, &rename.from), to: rename_path(&root, &rename.to), tabs, too_large });
        self.notes.disk.limit = Some(limit.saturating_sub(tab_bytes));
        self.notes.disk.start(jobs, crate::replace::Replacer::rewriting(rewriter), ctx);
        self.notes.disk_in_tabs = (in_tabs, tab_files);
    }

    /// Report a finished link update (or its undo).
    fn notes_disk_poll(&mut self, ctx: &egui::Context) {
        if !self.notes.disk.poll() {
            return;
        }
        for root in self.notes.index.roots().cloned().collect::<Vec<_>>() {
            self.notes.index.request(&root, Some(ctx));
        }
        if self.notes.disk.undoing {
            return self.finish_undo();
        }
        let d = &self.notes.disk;
        let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
        let (n, files) = (self.notes.disk_in_tabs.0 + d.replaced, self.notes.disk_in_tabs.1 + d.files);
        let mut text = format!("Updated {} in {}", plural(n, "link"), plural(files, "note"));
        if self.notes.disk_in_tabs.1 > 0 {
            text += &format!(" ({} in open tabs, unsaved)", plural(self.notes.disk_in_tabs.1, "note"));
        }
        if d.not_undoable > 0 {
            text += &format!("; {} too large to keep, so this rename can't be undone", plural(d.not_undoable, "file"));
        } else {
            text += ". Notes: Undo Rename puts them back";
        }
        if !d.skipped.is_empty() {
            let list: Vec<String> = d.skipped.iter().take(3).map(|(p, why)| format!("{}: {why}", file_label(p))).collect();
            text += &format!("; {} skipped ({})", d.skipped.len(), list.join("; "));
            log::warn!("link update skipped {} files", d.skipped.len());
        }
        self.status = text;
        if self.notes.disk.not_undoable > 0
            && let Some(u) = self.notes.last_rename.as_mut()
        {
            u.too_large = u.too_large.max(self.notes.disk.not_undoable);
        }
    }

    /// Notes: Undo Rename: links back as they were (byte for byte), and the
    /// old name, all or nothing. The files on disk (and the rename back) go
    /// through one transaction off the UI thread (see `swap`); open tabs are
    /// changed back last, once that has succeeded.
    fn undo_rename(&mut self, ctx: &egui::Context) {
        if self.notes.disk.running() {
            self.status = "Still updating links; try again in a moment".into();
            return;
        }
        let Some(u) = self.notes.last_rename.take() else {
            self.status = "No rename to undo".into();
            return;
        };
        if u.too_large > 0 || self.notes.disk.not_undoable > 0 {
            self.status = "This rename can't be undone: some files were too large to keep their old text".into();
            return;
        }
        let mut problems: Vec<(PathBuf, String)> = Vec::new();
        let mut in_tabs = Vec::new();
        let mut extra: Vec<crate::swap::Item> = Vec::new();
        for t in &u.tabs {
            if let Some(d) = self.docs.iter().find(|d| d.view.id == t.view) {
                if d.editor.version() == t.version {
                    in_tabs.push((t.view, t.version));
                } else {
                    problems.push((t.path.clone(), "edited in its tab since".into()));
                }
                continue;
            }
            // The tab is gone. Open again in another tab with edits: that's newer.
            if self.docs.iter().any(|d| d.path.as_deref() == Some(t.path.as_path()) && d.is_dirty()) {
                problems.push((t.path.clone(), "open with unsaved changes".into()));
                continue;
            }
            // Saved and closed: its file holds the update, which goes; closed
            // unsaved: its file is as it was, and stays.
            match (encoded(&t.before, t.encoding), encoded(&t.after, t.encoding)) {
                (Some(before), Some(after)) => extra.push(crate::swap::Item {
                    top: u.root.clone(),
                    path: t.path.clone(),
                    original: Some(Arc::new(before)),
                    expected: Some(crate::swap::identity_of(&after)),
                    unchanged: t.disk,
                    install_as: t.path.clone(),
                }),
                _ => problems.push((t.path.clone(), format!("its text can't be saved as {}", t.encoding.name()))),
            }
        }
        if !problems.is_empty() {
            self.status = refused_status(&problems);
            log::warn!("undo rename refused: {} files changed since", problems.len());
            self.notes.last_rename = Some(u);
            return;
        }
        // The renamed note goes back under its old name (with its old text, if it has some to get back).
        match extra.iter_mut().find(|i| i.path == u.to) {
            Some(i) => i.install_as = u.from.clone(),
            None => extra.push(crate::swap::Item { top: u.root.clone(), path: u.to.clone(), original: None, expected: None, unchanged: None, install_as: u.from.clone() }),
        }
        self.status = "Undo Rename: checking files…".into();
        let root = u.root.clone();
        self.notes.undoing = Some((u, in_tabs));
        self.notes.disk.start_restore(extra, Some(root), Some(ctx.clone()));
    }

    /// The disk half of Undo Rename finished: change the open tabs back, or say what's in the way.
    fn finish_undo(&mut self) {
        let Some((u, in_tabs)) = self.notes.undoing.take() else { return };
        use crate::swap::{Outcome, State};
        match self.notes.disk.undo.take() {
            Some(Outcome::Done(_)) => {
                let mut changed = 0;
                for (id, version) in in_tabs {
                    match self.docs.iter_mut().find(|d| d.view.id == id) {
                        Some(d) if d.editor.version() == version => d.editor.undo(),
                        _ => changed += 1,
                    }
                }
                self.follow_rename(&u.to, &u.from);
                self.link.requests.renamed.push((u.to.clone(), u.from.clone()));
                self.status = format!("Undo Rename: back to {}", u.from.display());
                if changed > 0 {
                    self.status += &format!("; {changed} open tab{} changed meanwhile: use Undo there", if changed == 1 { "" } else { "s" });
                }
            }
            Some(Outcome::Refused(why, kept)) => {
                let mut all = why;
                all.extend(kept.into_iter().map(|(p, s)| (p, state_label(&s))));
                self.status = refused_status(&all);
                // Nothing changed: it can be tried again.
                self.notes.last_rename = Some(u);
            }
            Some(Outcome::Partial(states, why)) => {
                // The note may or may not be back under its old name: follow wherever it is.
                if states.iter().any(|(p, s)| *p == u.to && *s == State::Restored) {
                    self.follow_rename(&u.to, &u.from);
                    self.link.requests.renamed.push((u.to.clone(), u.from.clone()));
                }
                let list: Vec<String> = states.iter().map(|(p, s)| format!("{}: {}", file_label(p), state_label(s))).collect();
                log::warn!("undo rename stopped partway: {why}");
                self.status = format!("Undo Rename stopped partway ({why}). {}", list.join("; "));
            }
            None => {}
        }
    }

    // ---- the menu -------------------------------------------------------------------

    pub(super) fn notes_menu(&self) -> crate::menus::Item {
        use crate::menus::Item;
        crate::menus::Item::Sub(
            "Notes".into(),
            vec![
                Item::cmd("Open Daily Note", Cmd::NotesDailyNote),
                Item::cmd("New Note…", Cmd::NotesNewNote),
                Item::cmd("New Note from Template…", Cmd::NotesNewFromTemplate),
                Item::Separator,
                Item::cmd("Follow Link", Cmd::NotesFollowLink),
                Item::cmd("Find Tag…", Cmd::NotesFindTag),
                Item::cmd("Backlinks", Cmd::NotesToggleBacklinks).checked(self.backlinks_shown()),
                Item::Separator,
                Item::cmd("Rename Note…", Cmd::NotesRename),
                Item::cmd("Undo Rename", Cmd::NotesUndoRename).enabled(self.notes.last_rename.as_ref().is_some_and(UndoRename::undoable)),
                Item::cmd("Compare with Conflict Copy", Cmd::NotesCompareConflict),
            ],
        )
    }
}

/// The files of a rename's link update (each with its changes) and those
/// that can't be updated safely, from `work`: (vault path, full path, text
/// of its open tab).
fn plan_files(rename: &Rename, work: Vec<(String, PathBuf, Option<Arc<String>>)>) -> super::notes_panel::Planned {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for (rel, path, tab) in work {
        let in_tab = tab.is_some();
        let (text, stamp) = match tab {
            Some(t) => (t.to_string(), None),
            None => match crate::search::read_text(&path) {
                Ok(f) if f.lossy => {
                    skipped.push((path.clone(), format!("not valid {} (saving would lose bytes)", f.encoding.name())));
                    continue;
                }
                Ok(f) => (f.text, Some(f.stamp)),
                Err(e) => {
                    skipped.push((path.clone(), e.to_string()));
                    continue;
                }
            },
        };
        let edits = rename.edits(&rel, &text);
        if !edits.is_empty() {
            let tab_bytes = if in_tab { text.len() } else { 0 };
            files.push(PlanFile { path, rel, stamp, in_tab, tab_bytes, count: edits.len(), lines: preview_lines(&text, &edits) });
        }
    }
    (files, skipped)
}

/// "; 2 files couldn't be read" for a status line, or nothing.
fn plan_skipped_note(plan: &LinkPlan) -> String {
    match plan.skipped_count() {
        0 => String::new(),
        n => format!("; {n} file{} with links couldn't be read", if n == 1 { "" } else { "s" }),
    }
}

fn rename_path(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_owned(), |p, part| p.join(part))
}

/// Each changed line before and after: (0-based line, old, new).
fn preview_lines(text: &str, edits: &[(Range<usize>, String)]) -> Vec<(usize, String, String)> {
    let mut out: Vec<(usize, String, String)> = Vec::new();
    let mut i = 0;
    while i < edits.len() {
        let start = text[..edits[i].0.start].rfind('\n').map_or(0, |p| p + 1);
        let end = text[edits[i].0.start..].find('\n').map_or(text.len(), |p| edits[i].0.start + p);
        let mut j = i;
        while j < edits.len() && edits[j].0.end <= end {
            j += 1;
        }
        let local: Vec<(Range<usize>, String)> = edits[i..j.max(i + 1)].iter().map(|(r, s)| (r.start - start..r.end - start, s.clone())).collect();
        let old = text[start..end].trim_end_matches('\r');
        let new = notes::rename::apply(old, &local);
        out.push((text[..start].matches('\n').count(), old.to_owned(), new));
        i = j.max(i + 1);
    }
    out
}

/// What `[[` offers: notes (by path without .md) and their aliases, then other
/// files, as (label, file, alias).
pub(super) struct Candidates {
    list: Vec<(String, u32, Option<String>)>,
    /// Where the files that aren't notes start.
    notes_end: usize,
}

impl Candidates {
    fn of(vault: &Vault) -> Candidates {
        let mut list: Vec<(String, u32, Option<String>)> = Vec::new();
        for (i, rel) in vault.files.paths().iter().enumerate() {
            let id = i as u32;
            if is_note(rel) {
                list.push((without_md(rel).to_owned(), id, None));
                for a in vault.note(id).map(|n| n.aliases.as_slice()).unwrap_or(&[]) {
                    list.push((a.clone(), id, Some(a.clone())));
                }
            }
        }
        let notes_end = list.len();
        list.extend(vault.files.paths().iter().enumerate().filter(|(_, r)| !is_note(r)).map(|(i, r)| (r.clone(), i as u32, None)));
        Candidates { list, notes_end }
    }
}

/// The `[[` list for `query` typed in the note at `from` (whose text is `text`).
fn completions(vault: &Vault, cands: &Candidates, from: &Path, query: &str, text: &str) -> Vec<Choice> {
    let from_rel = vault.rel(from).unwrap_or_default();
    let format = vault.config.link_format;
    let wiki_link = |id: u32| vault.files.link_path(id, &from_rel, format, false);
    let md_dest = |id: u32| parse::percent_encode(&vault.files.link_path(id, &from_rel, format, true));
    if let Some((note_part, sub)) = query.split_once('#') {
        // Headings (or `^block`s) of the note named before `#` (this note when empty).
        let target = if note_part.is_empty() { vault.find(from) } else { vault.resolve(note_part, from) };
        let (headings, blocks, here) = match target.and_then(|t| vault.note(t)).filter(|_| !note_part.is_empty()) {
            Some(n) => (n.headings.clone(), n.blocks.clone(), false),
            None if note_part.is_empty() => {
                let n = parse::parse(text);
                (n.headings.into_iter().map(|h| h.text).collect(), n.blocks.into_iter().map(|(b, _)| b).collect(), true)
            }
            None => return Vec::new(),
        };
        let prefix = if here { String::new() } else { target.map(wiki_link).unwrap_or_else(|| note_part.to_owned()) };
        let dest = if here { String::new() } else { target.map(md_dest).unwrap_or_default() };
        let (items, block): (Vec<String>, bool) = match sub.strip_prefix('^') {
            Some(_) => (blocks, true),
            None => (headings.into_iter().map(|h| heading_for_link(&h)).filter(|h| !h.is_empty()).collect(), false),
        };
        let q = if block { &sub[1..] } else { sub };
        return crate::fuzzy::rank(q, &items, |s| s, COMPLETIONS)
            .into_iter()
            .map(|(k, _, _)| {
                let h = &items[k];
                let sub = if block { format!("^{h}") } else { h.clone() };
                Choice {
                    label: if block { format!("^{h}") } else { h.clone() },
                    detail: if block { "block" } else { "heading" }.into(),
                    wiki: format!("{prefix}#{sub}"),
                    markdown: (h.clone(), format!("{dest}#{}", parse::percent_encode(&sub))),
                }
            })
            .collect();
    }
    let (list, notes_end) = (&cands.list, cands.notes_end);
    let ranked = crate::fuzzy::rank(query.trim(), list, |c| &c.0, COMPLETIONS * 4);
    let mut out: Vec<Choice> = ranked
        .into_iter()
        // With nothing typed, notes only (not every image in the vault).
        .filter(|(k, _, _)| !query.trim().is_empty() || *k < notes_end)
        .take(COMPLETIONS)
        .map(|(k, _, _)| {
            let (label, id, alias) = &list[k];
            let path = vault.files.path(*id);
            let name = without_md(path.rsplit('/').next().unwrap_or(path)).to_owned();
            let link = wiki_link(*id);
            match alias {
                Some(a) => Choice { label: a.clone(), detail: format!("alias of {}", without_md(path)), wiki: format!("{link}|{a}"), markdown: (a.clone(), md_dest(*id)) },
                None => Choice { label: label.rsplit('/').next().unwrap_or(label).to_owned(), detail: path.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_owned()), wiki: link, markdown: (name, md_dest(*id)) },
            }
        })
        .collect();
    out.dedup_by(|a, b| a.wiki == b.wiki);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names_are_checked() {
        assert!(check_note_name("My Note").is_ok());
        assert!(check_note_name("Folder/Sub/Note é").is_ok());
        for bad in ["", "  ", "a/../b", "a//b", "x:y", "what?", "a|b", "trailing.", " lead", "tab\there"] {
            assert!(check_note_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn settings_paths_stay_inside_the_vault() {
        let root = std::env::temp_dir().join(format!("strata-notes-inside-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Templates")).unwrap();
        assert_eq!(inside_vault(&root, "Templates/Day.md").unwrap(), root.join("Templates").join("Day.md"));
        assert_eq!(inside_vault(&root, "./Templates//Day.md").unwrap(), root.join("Templates").join("Day.md"));
        for bad in ["../secret", "Templates/../../secret", "/etc/hosts", "\\\\server\\share\\x", "//server/share/x", "C:\\x", "c:/x", "\\x"] {
            assert!(inside_vault(&root, bad).is_err(), "{bad}");
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn preview_lines_show_each_changed_line_once() {
        let text = "one [[Old]] and [[Old|x]]\ntwo\nthree [[Old]]\r\nfour";
        let edits = vec![(6..9, "New".to_owned()), (18..21, "New".to_owned()), (38..41, "New".to_owned())];
        assert_eq!(
            preview_lines(text, &edits),
            vec![(0, "one [[Old]] and [[Old|x]]".to_owned(), "one [[New]] and [[New|x]]".to_owned()), (2, "three [[Old]]".to_owned(), "three [[New]]".to_owned())]
        );
        assert_eq!(heading_for_link("A [b] | c #d ^e"), "A b c d e");
    }
}
