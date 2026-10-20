//! Left sidebar: the window's folders (each a top-level tree, like
//! Sublime's project folders) and find-in-files results.

use crate::find::Query;
use crate::replace::{DiskReplace, Job, Replacer};
use crate::search::{Scope, Search};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Key, RichText, Ui};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How often the folders shown in the tree are checked for changes made
/// outside Strata (seconds).
const WATCH_SECS: f64 = 2.0;

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Tab {
    Files,
    Search,
    /// Review Changes' list, drawn by the app (see `app::review_ui`).
    Changes,
}

pub enum Action {
    /// Open a file; `true` = preview (single click), replaced by the next preview.
    Open(PathBuf, bool),
    /// Open and select `cols` (chars) on 0-based `line`.
    OpenAt(PathBuf, usize, Range<usize>),
    /// Switch to / close tab `n` (from the Open Files list).
    Activate(usize),
    Close(usize),
    /// A file or folder was renamed on disk (open tabs follow it).
    Renamed(PathBuf, PathBuf),
    /// A file or folder went to the Trash (its tabs close, or keep their
    /// unsaved text marked deleted).
    Deleted(PathBuf),
    Status(String),
    /// The user confirmed Replace in Files: edit open tabs, then call
    /// [`Sidebar::start_disk_replace`] with the rest.
    ReplaceInFiles,
    /// Compare this file with an open tab (or another file).
    Compare(PathBuf),
}

/// A tab as the Open Files list shows it.
pub struct OpenFile {
    pub name: String,
    pub dirty: bool,
    pub active: bool,
    pub path: Option<PathBuf>,
    /// The tab's text if it has unsaved changes (find in files searches it
    /// instead of the file on disk).
    pub unsaved: Option<Arc<String>>,
}

#[derive(Clone, PartialEq)]
enum PromptKind {
    NewFile,
    NewFolder,
    Rename,
}

/// The name dialog for New File / New Folder / Rename.
struct Prompt {
    kind: PromptKind,
    /// Folder to create in, or the path being renamed.
    target: PathBuf,
    name: String,
    error: Option<String>,
}

struct Entry {
    path: PathBuf,
    name: String,
    is_dir: bool,
}

/// A folder's entries, and its modification time when they were read (it
/// changes when something is added, removed or renamed in it).
struct Listing {
    mtime: Option<std::time::SystemTime>,
    entries: Vec<Entry>,
}

/// Which entries the tree leaves out.
#[derive(Clone, PartialEq)]
pub struct TreeFilter {
    /// Hide what .gitignore (and .ignore, the global excludes) ignore.
    pub respect_gitignore: bool,
    /// Names (globs like `*.pyc`) never shown.
    pub exclude: Vec<String>,
}

impl Default for TreeFilter {
    fn default() -> Self {
        Self { respect_gitignore: true, exclude: vec![".git".into(), ".DS_Store".into()] }
    }
}

fn modified(dir: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(dir).and_then(|m| m.modified()).ok()
}
pub struct Sidebar {
    /// The window's folders, in the order added; empty when none is open.
    pub roots: Vec<PathBuf>,
    pub visible: bool,
    pub tab: Tab,
    /// Directory listings, read once per folder until it changes or is refreshed.
    listings: HashMap<PathBuf, Listing>,
    /// Folders whose entries were drawn this frame and last frame: only those
    /// are checked for outside changes.
    drawn: std::collections::HashSet<PathBuf>,
    drawn_before: std::collections::HashSet<PathBuf>,
    last_check: f64,
    filter: TreeFilter,
    exclude: globset::GlobSet,
    /// Asking "Move … to the Trash?".
    delete: Option<PathBuf>,
    pub query: Query,
    pub search: Search,
    focus_query: bool,
    prompt: Option<Prompt>,
    /// The Open Files list above the folder tree.
    pub show_open_files: bool,
    /// Find in Files "Where" and "Replace" fields.
    pub where_text: String,
    pub replacement: String,
    focus_replace: bool,
    /// Replace was pressed: confirm once the fresh search finishes.
    pending_replace: bool,
    /// Asking "Replace N occurrences across M files?": (occurrences, files, of which open in tabs).
    confirm: Option<(usize, usize, usize)>,
    /// Rewrites files that aren't open; holds the undo.
    pub disk: DiskReplace,
    /// Occurrences and files replaced in open tabs by the running replace.
    in_tabs: (usize, usize),
    /// Outcome of the last replace (or undo), shown above the results.
    report: Option<(String, Vec<(PathBuf, String)>)>,
    /// File to expand the tree to and scroll into view (Reveal in Side Bar),
    /// and frames left to try: each folder level opens over a frame or so.
    reveal: Option<(PathBuf, u8)>,
}

impl Sidebar {
    pub fn new() -> Self {
        Self {
            roots: Vec::new(),
            visible: true,
            tab: Tab::Files,
            listings: HashMap::new(),
            drawn: Default::default(),
            drawn_before: Default::default(),
            last_check: 0.0,
            filter: TreeFilter::default(),
            exclude: exclude_set(&TreeFilter::default().exclude),
            delete: None,
            query: Query::default(),
            search: Search::default(),
            focus_query: false,
            prompt: None,
            show_open_files: true,
            where_text: String::new(),
            replacement: String::new(),
            focus_replace: false,
            pending_replace: false,
            confirm: None,
            disk: DiskReplace::default(),
            in_tabs: (0, 0),
            report: None,
            reveal: None,
        }
    }

    /// The first folder: `$folder` for builds, the window title.
    pub fn root(&self) -> Option<&Path> {
        self.roots.first().map(PathBuf::as_path)
    }

    /// The folder `path` is in (the innermost, if folders nest).
    pub fn root_of(&self, path: &Path) -> Option<&Path> {
        self.roots.iter().filter(|r| path.starts_with(r)).max_by_key(|r| r.components().count()).map(PathBuf::as_path)
    }

    /// The sidebar panel is drawn: there are folders, or the Search or
    /// Changes tab is up (both work without a folder).
    pub fn shown(&self) -> bool {
        self.visible && (!self.roots.is_empty() || self.tab != Tab::Files)
    }

    /// `path` for display: relative to its folder, after the folder's name
    /// when there are several, else in full.
    pub fn display_path(&self, path: &Path) -> String {
        let Some(root) = self.root_of(path) else { return path.display().to_string() };
        let rel = path.strip_prefix(root).unwrap_or(path).display().to_string().replace('\\', "/");
        match root.file_name() {
            Some(name) if self.roots.len() > 1 => format!("{}/{rel}", name.to_string_lossy()),
            _ => rel,
        }
    }

    /// Show `path` in the tree: open its folders, scroll to it. False if it
    /// isn't inside one of the folders.
    pub fn reveal(&mut self, path: &Path) -> bool {
        if self.root_of(path).is_none() {
            return false;
        }
        self.visible = true;
        self.tab = Tab::Files;
        self.reveal = Some((path.to_owned(), 60));
        self.refresh();
        true
    }

    /// Make `root` the only folder.
    pub fn open_folder(&mut self, root: PathBuf) {
        self.search.stop();
        self.search = Search::default();
        self.listings.clear();
        self.roots = vec![root];
        self.visible = true;
        self.tab = Tab::Files;
    }

    /// Add `root` after the other folders (Project ▸ Add Folder to
    /// Project…). False if it's already there.
    pub fn add_folder(&mut self, root: PathBuf) -> bool {
        self.visible = true;
        self.tab = Tab::Files;
        if self.roots.contains(&root) {
            return false;
        }
        self.roots.push(root);
        true
    }

    /// Take `root` out of the sidebar (nothing on disk changes).
    pub fn remove_folder(&mut self, root: &Path) {
        self.roots.retain(|r| r != root);
        self.listings.retain(|dir, _| !dir.starts_with(root) || self.roots.iter().any(|r| dir.starts_with(r)));
        if self.roots.is_empty() {
            self.close_folders();
        }
    }

    /// Remove all Folders from Project: an empty sidebar.
    pub fn close_folders(&mut self) {
        let (visible, show_open_files) = (self.visible, self.show_open_files);
        *self = Sidebar::new();
        (self.visible, self.show_open_files) = (visible, show_open_files);
    }

    /// Find in Folder…: the Search tab, looking in `dir`.
    pub fn find_in_folder(&mut self, dir: &Path) {
        self.where_text = dir.display().to_string();
        self.start_find_in_files(None);
    }

    /// Cmd+Shift+F: show the Search tab with `prefill` (if any) in the query.
    pub fn start_find_in_files(&mut self, prefill: Option<String>) {
        if let Some(text) = prefill {
            self.query.text = text;
        }
        self.visible = true;
        self.tab = Tab::Search;
        self.focus_query = true;
    }

    /// Find ▸ Replace in Files…: the Search tab with the Replace field focused.
    pub fn start_replace_in_files(&mut self, prefill: Option<String>) {
        self.start_find_in_files(prefill);
        self.focus_query = self.query.text.is_empty();
        self.focus_replace = !self.focus_query;
    }

    /// Run the search for the Find, Where and option fields.
    pub fn run_search(&mut self, open: &[OpenFile], ctx: Option<egui::Context>) {
        let mut open_paths: Vec<PathBuf> = open.iter().filter_map(|f| f.path.clone()).collect();
        // Several tabs can show one file (New View into File): search it once.
        let mut seen = std::collections::HashSet::new();
        open_paths.retain(|p| seen.insert(p.clone()));
        match Scope::parse(&self.where_text, &self.roots, &open_paths) {
            Ok(mut scope) => {
                scope.buffers = open.iter().filter_map(|f| Some((f.path.clone()?, f.unsaved.clone()?))).collect();
                self.search.start_in(scope, &self.where_text, &self.query, ctx);
            }
            Err(e) => self.search.fail(e),
        }
    }

    /// Replace pressed: search afresh, then ask before changing anything.
    pub fn request_replace(&mut self, open: &[OpenFile], ctx: Option<egui::Context>) {
        self.report = None;
        self.run_search(open, ctx);
        self.pending_replace = self.search.running;
    }

    fn ask_to_replace(&mut self, open: &[OpenFile]) {
        let s = &self.search;
        if s.error.is_some() {
            return;
        }
        if s.results.is_empty() {
            self.report = Some(("No matches to replace".into(), Vec::new()));
        } else if s.truncated {
            self.report = Some((format!("Over {} matches: narrow the search (Where) before replacing", crate::search::HIT_LIMIT), Vec::new()));
        } else {
            let open: Vec<PathBuf> = open.iter().filter_map(|f| f.path.as_deref().map(canonical)).collect();
            let in_tabs = s.results.iter().filter(|f| open.contains(&canonical(&f.path))).count();
            self.confirm = Some((s.total_hits(), s.results.len(), in_tabs));
        }
    }

    /// A dialog of the sidebar's is up (a name prompt, or Replace in Files' question).
    pub fn modal_open(&self) -> bool {
        self.prompt.is_some() || self.confirm.is_some() || self.delete.is_some()
    }

    /// Settings for what the tree hides; a change re-reads it.
    pub fn set_filter(&mut self, filter: &TreeFilter) {
        if *filter != self.filter {
            self.filter = filter.clone();
            self.exclude = exclude_set(&filter.exclude);
            self.refresh();
        }
    }

    /// Re-read the folders on screen whose contents changed outside Strata
    /// (checked every couple of seconds; `now` is the input time).
    pub fn watch(&mut self, now: f64) {
        if now - self.last_check < WATCH_SECS {
            return;
        }
        self.last_check = now;
        let stale: Vec<PathBuf> = self
            .drawn_before
            .iter()
            .filter(|dir| self.listings.get(*dir).is_some_and(|l| l.mtime != modified(dir)))
            .cloned()
            .collect();
        for dir in stale {
            self.listings.remove(&dir);
        }
    }

    /// Ask for a new name for `path` (as its Rename… menu item does).
    #[cfg(test)]
    pub fn ask_rename(&mut self, path: &Path) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.prompt = Some(Prompt { kind: PromptKind::Rename, target: path.to_owned(), name, error: None });
    }

    /// The name typed in the open prompt, and its error.
    #[cfg(test)]
    pub fn prompt_state(&mut self) -> Option<(&mut String, Option<&str>)> {
        self.prompt.as_mut().map(|p| (&mut p.name, p.error.as_deref()))
    }

    /// The "Replace N occurrences…?" question is up.
    #[cfg(test)]
    pub fn confirming(&self) -> bool {
        self.confirm.is_some()
    }

    /// Proceed without asking (dev hook: the caller replaces right away).
    pub fn skip_confirm(&mut self) {
        self.pending_replace = false;
        self.confirm = None;
    }

    /// After open tabs were edited (`in_tabs` = occurrences, files), rewrite
    /// `jobs` on disk in the background.
    pub fn start_disk_replace(&mut self, jobs: Vec<Job>, r: Replacer, in_tabs: (usize, usize), ctx: Option<egui::Context>) {
        self.in_tabs = in_tabs;
        // Its undo touches only files inside these folders (a link's target elsewhere is refused).
        self.disk.roots = self.roots.clone();
        self.disk.start(jobs, r, ctx);
    }

    /// Restore the files the last replace rewrote on disk.
    pub fn undo_replace(&mut self, ctx: Option<egui::Context>) {
        if !self.disk.running() && self.disk.undoable() > 0 {
            self.in_tabs = (0, 0);
            self.disk.start_undo(ctx);
        }
    }

    /// Collect the disk job; when it ends, summarize it, refresh the
    /// results and return the summary.
    pub fn poll_replace(&mut self, open: &[OpenFile], ctx: Option<egui::Context>) -> Option<String> {
        if !self.disk.poll() {
            return None;
        }
        let d = &self.disk;
        let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
        let mut text = if d.undoing {
            let summary = match &d.undo {
                // All or nothing: the files in the way are listed below.
                Some(crate::swap::Outcome::Refused(..)) => "Undo Replace in Files: nothing was restored, because some files changed since the replace".to_owned(),
                Some(crate::swap::Outcome::Partial(..)) => format!("Undo Replace in Files stopped partway: restored {}", plural(d.files, "file")),
                _ => format!("Undo Replace in Files: restored {}", plural(d.files, "file")),
            };
            let left: String = d.leftovers.iter().map(|p| format!("; an emptied folder of Strata's couldn't be removed: {}", p.display())).collect();
            summary + &left
        } else {
            let (n, files) = (self.in_tabs.0 + d.replaced, self.in_tabs.1 + d.files);
            let mut t = format!("Replaced {} in {}", plural(n, "occurrence"), plural(files, "file"));
            if self.in_tabs.1 > 0 {
                t += &format!(" ({} in open tabs, unsaved)", plural(self.in_tabs.1, "file"));
            }
            if d.not_undoable > 0 {
                t += &format!("; {} too large to keep for undo", plural(d.not_undoable, "file"));
            }
            t
        };
        if !d.skipped.is_empty() {
            text += &format!("; {} skipped", d.skipped.len());
        }
        self.report = Some((text.clone(), d.skipped.clone()));
        self.run_search(open, ctx);
        Some(text)
    }

    pub fn refresh(&mut self) {
        self.listings.clear();
    }

    pub fn show(&mut self, ui: &mut Ui, current: Option<&Path>, open: &[OpenFile]) -> Option<Action> {
        self.drawn_before = std::mem::take(&mut self.drawn);
        self.watch(ui.input(|i| i.time));
        self.search.poll();
        let finished = self.poll_replace(open, Some(ui.ctx().clone())).map(Action::Status);
        if self.pending_replace && !self.search.running {
            self.pending_replace = false;
            self.ask_to_replace(open);
        }
        let confirmed = self.confirm_replace(ui.ctx());
        let deleted = self.confirm_delete(ui.ctx());
        let prompted = self.prompt(ui.ctx()).or(confirmed).or(deleted).or(finished);
        let roots = self.roots.clone();
        ui.horizontal(|ui| {
            // No folder: find in files (in open files) and the changes list are on offer.
            if !roots.is_empty() {
                ui.selectable_value(&mut self.tab, Tab::Files, "Files");
            }
            ui.selectable_value(&mut self.tab, Tab::Search, "Search");
            ui.selectable_value(&mut self.tab, Tab::Changes, "Changes");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.tab == Tab::Files && ui.small_button("⟳").on_hover_text("Refresh").clicked() {
                    self.refresh();
                }
                if roots.is_empty() && ui.small_button("×").on_hover_text("Close").clicked() {
                    self.tab = Tab::Files;
                }
            });
        });
        if roots.is_empty() && self.tab == Tab::Files {
            return prompted;
        }
        ui.separator();
        let action = match self.tab {
            Tab::Files => {
                let mut action = None;
                egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    if self.show_open_files && open.len() > 1 {
                        egui::CollapsingHeader::new(RichText::new("OPEN FILES").small().weak()).id_salt("open-files").default_open(true).show(ui, |ui| {
                            for (i, f) in open.iter().enumerate() {
                                crate::fonts::ensure_cjk(ui.ctx(), &f.name);
                                ui.horizontal(|ui| {
                                    let label = if f.dirty { format!("● {}", f.name) } else { f.name.clone() };
                                    if ui.selectable_label(f.active, label).clicked() {
                                        action = Some(Action::Activate(i));
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.small_button("×").on_hover_text("Close").clicked() {
                                            action = Some(Action::Close(i));
                                        }
                                    });
                                });
                            }
                        });
                    }
                    // Each folder is a top-level tree, like Sublime's project folders.
                    for root in &roots {
                        let name = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
                        crate::fonts::ensure_cjk(ui.ctx(), &name);
                        let mut header = egui::CollapsingHeader::new(RichText::new(name).strong()).id_salt(root).default_open(true);
                        if self.reveal.as_ref().is_some_and(|(r, _)| r.starts_with(root)) {
                            header = header.open(Some(true));
                        }
                        let header = header.show(ui, |ui| self.tree(ui, root, current, &mut action));
                        header.header_response.on_hover_text(root.display().to_string()).context_menu(|ui| self.menu(ui, root, true, &mut action));
                    }
                });
                if let Some((_, tries)) = &mut self.reveal {
                    *tries = tries.saturating_sub(1);
                    if *tries == 0 {
                        self.reveal = None;
                    }
                    ui.ctx().request_repaint();
                }
                action
            }
            Tab::Search => self.search_tab(ui, open),
            Tab::Changes => None,
        };
        prompted.or(action)
    }

    /// Right-click menu for a tree row.
    fn menu(&mut self, ui: &mut Ui, path: &Path, is_dir: bool, action: &mut Option<Action>) {
        let dir = if is_dir { path.to_owned() } else { path.parent().unwrap_or(path).to_owned() };
        let mut ask = |kind: PromptKind, target: PathBuf, name: String| {
            self.prompt = Some(Prompt { kind, target, name, error: None });
        };
        if ui.button("New File…").clicked() {
            ask(PromptKind::NewFile, dir.clone(), String::new());
            ui.close();
        }
        if ui.button("New Folder…").clicked() {
            ask(PromptKind::NewFolder, dir.clone(), String::new());
            ui.close();
        }
        let is_root = self.roots.iter().any(|r| r == path);
        if !is_root && ui.button("Rename…").clicked() {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            ask(PromptKind::Rename, path.to_owned(), name);
            ui.close();
        }
        if !is_root && ui.button(format!("Move to {}…", crate::trash::trash_name())).clicked() {
            self.delete = Some(path.to_owned());
            ui.close();
        }
        ui.separator();
        if ui.button("Copy Path").clicked() {
            ui.ctx().copy_text(path.display().to_string());
            ui.close();
        }
        if ui.button(crate::ctxmenu::reveal_label()).clicked() {
            if let Err(e) = crate::ctxmenu::reveal_in_file_manager(path) {
                *action = Some(Action::Status(format!("Could not open the file manager: {e}")));
            }
            ui.close();
        }
        if ui.button("Find in Folder…").on_hover_text("Find in Files, in this folder").clicked() {
            self.find_in_folder(&dir);
            ui.close();
        }
        if !is_dir && ui.button("Compare with Open File…").on_hover_text("Side by side with an open tab, the git HEAD version, or another file").clicked() {
            *action = Some(Action::Compare(path.to_owned()));
            ui.close();
        }
        if is_root {
            ui.separator();
            if ui.button("Remove Folder from Project").on_hover_text("Take it out of the side bar; nothing on disk changes").clicked() {
                self.remove_folder(path);
                ui.close();
            }
        }
    }

    /// "Replace N occurrences across M files?"
    fn confirm_replace(&mut self, ctx: &egui::Context) -> Option<Action> {
        let (n, files, in_tabs) = self.confirm?;
        let (mut yes, mut no) = (false, false);
        egui::Modal::new(egui::Id::new("replace-in-files-confirm")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new("Replace in Files").strong());
            ui.add_space(4.0);
            let s = |n: usize| if n == 1 { "" } else { "s" };
            ui.label(format!("Replace {n} occurrence{} across {files} file{}?", s(n), s(files)));
            let with = if self.replacement.is_empty() { "nothing (matches are deleted)".to_owned() } else { format!("“{}”", self.replacement) };
            ui.label(RichText::new(format!("“{}” → {with}", self.query.text)).monospace().small());
            let on_disk = files - in_tabs;
            let mut note = String::new();
            if in_tabs > 0 {
                let their = if in_tabs == 1 { "its tab" } else { "their tabs" };
                note += &format!("{in_tabs} open file{} will change in {their}, unsaved (Undo works there). ", s(in_tabs));
            }
            if on_disk > 0 {
                note += &format!("{on_disk} file{} will be rewritten on disk (Undo Replace in Files restores them).", s(on_disk));
            }
            ui.label(RichText::new(note.trim_end()).small().weak());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                yes = ui.button("Replace").clicked() || ui.input(|i| i.key_pressed(Key::Enter));
                no = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });
        if yes || no {
            self.confirm = None;
        }
        yes.then_some(Action::ReplaceInFiles)
    }

    /// "Move … to the Trash?"
    fn confirm_delete(&mut self, ctx: &egui::Context) -> Option<Action> {
        let path = self.delete.clone()?;
        let (mut yes, mut no) = (false, false);
        egui::Modal::new(egui::Id::new("sidebar-delete")).show(ctx, |ui| {
            ui.set_width(340.0);
            let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
            crate::fonts::ensure_cjk(ui.ctx(), &name);
            let what = if path.is_dir() { "the folder" } else { "" };
            ui.label(RichText::new(format!("Move {what} “{name}” to the {}?", crate::trash::trash_name()).replace("  ", " ")).strong());
            ui.add_space(4.0);
            let more = if path.is_dir() { "Everything in it goes too. " } else { "" };
            ui.label(RichText::new(format!("{more}You can put it back from the {}. Open tabs of it close; any with unsaved changes stay open, marked deleted.", crate::trash::trash_name())).small().weak());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                yes = ui.button(format!("Move to {}", crate::trash::trash_name())).clicked() || ui.input(|i| i.key_pressed(Key::Enter));
                no = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });
        if !(yes || no) {
            return None;
        }
        self.delete = None;
        if no {
            return None;
        }
        self.refresh();
        Some(match crate::trash::move_to_trash(&path) {
            Ok(()) => Action::Deleted(path),
            Err(e) => Action::Status(format!("Could not move {} to the {}: {e}", path.display(), crate::trash::trash_name())),
        })
    }

    /// The name dialog, if one is open. Creates/renames on Enter.
    fn prompt(&mut self, ctx: &egui::Context) -> Option<Action> {
        let p = self.prompt.as_mut()?;
        let title = match p.kind {
            PromptKind::NewFile => "New file name",
            PromptKind::NewFolder => "New folder name",
            PromptKind::Rename => "Rename to",
        };
        let mut done = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("sidebar-prompt")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(RichText::new(title).strong());
            crate::fonts::ensure_cjk(ui.ctx(), &p.name);
            let field = ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(f32::INFINITY));
            field.request_focus();
            if let Some(e) = &p.error {
                ui.colored_label(Color32::LIGHT_RED, e);
            }
            ui.horizontal(|ui| {
                done = ui.button("OK").clicked() || ui.input(|i| i.key_pressed(Key::Enter));
                cancel = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });
        if cancel {
            self.prompt = None;
            return None;
        }
        if !done {
            return None;
        }
        let name = p.name.trim().to_owned();
        if let Err(e) = check_name(&name, p.kind != PromptKind::Rename) {
            p.error = Some(e);
            return None;
        }
        let exists = || std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("“{name}” already exists"));
        let result = match p.kind {
            PromptKind::NewFile => {
                let path = p.target.join(&name);
                if std::fs::symlink_metadata(&path).is_ok() {
                    Err(exists())
                } else {
                    // `create_new`: never truncate a file that appeared meanwhile.
                    path.parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|()| std::fs::OpenOptions::new().write(true).create_new(true).open(&path))
                        .map(|_| Action::Open(path, false))
                }
            }
            PromptKind::NewFolder => {
                let path = p.target.join(&name);
                if std::fs::symlink_metadata(&path).is_ok() { Err(exists()) } else { std::fs::create_dir_all(&path).map(|_| Action::Status(format!("Created {name}"))) }
            }
            PromptKind::Rename => {
                let to = p.target.with_file_name(&name);
                if to == p.target {
                    // Nothing to do.
                    Ok(Action::Status(String::new()))
                } else if std::fs::symlink_metadata(&to).is_ok() && !same_file(&p.target, &to) {
                    Err(exists())
                } else {
                    rename(&p.target, &to).map(|_| Action::Renamed(p.target.clone(), to))
                }
            }
        };
        match result {
            Ok(action) => {
                self.prompt = None;
                self.refresh();
                Some(action)
            }
            Err(e) => {
                p.error = Some(e.to_string());
                None
            }
        }
    }

    fn listing(&mut self, dir: &Path) -> &[Entry] {
        self.drawn.insert(dir.to_owned());
        let (filter, exclude) = (&self.filter, &self.exclude);
        &self
            .listings
            .entry(dir.to_owned())
            .or_insert_with(|| {
                // The time first: a change while reading shows up next check.
                let mtime = modified(dir);
                let mut entries = read_folder(dir, filter.respect_gitignore);
                entries.retain(|e| !exclude.is_match(&e.name));
                // Folders first, then case-insensitive by name.
                entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
                Listing { mtime, entries }
            })
            .entries
    }

    fn tree(&mut self, ui: &mut Ui, dir: &Path, current: Option<&Path>, action: &mut Option<Action>) {
        // Collect first: rendering children needs `&mut self` again.
        let entries: Vec<(PathBuf, String, bool)> =
            self.listing(dir).iter().map(|e| (e.path.clone(), e.name.clone(), e.is_dir)).collect();
        for (path, name, is_dir) in entries {
            crate::fonts::ensure_cjk(ui.ctx(), &name);
            if is_dir {
                // Children are listed only once the folder is expanded.
                let mut header = egui::CollapsingHeader::new(&name).id_salt(&path).default_open(false);
                if self.reveal.as_ref().is_some_and(|(r, _)| r.starts_with(&path)) {
                    header = header.open(Some(true));
                }
                let header = header.show(ui, |ui| self.tree(ui, &path, current, action));
                header.header_response.context_menu(|ui| self.menu(ui, &path, true, action));
            } else {
                let r = ui.selectable_label(current == Some(path.as_path()), &name);
                if self.reveal.as_ref().is_some_and(|(r, _)| *r == path) {
                    self.reveal = None;
                    r.scroll_to_me(Some(egui::Align::Center));
                }
                r.context_menu(|ui| self.menu(ui, &path, false, action));
                if r.double_clicked() {
                    *action = Some(Action::Open(path, false));
                } else if r.clicked() {
                    *action = Some(Action::Open(path, true));
                }
            }
        }
    }

    fn search_tab(&mut self, ui: &mut Ui, open: &[OpenFile]) -> Option<Action> {
        let (mut find, mut replace) = (false, false);
        let busy = self.disk.running();
        ui.horizontal(|ui| {
            ui.toggle_value(&mut self.query.regex, ".*").on_hover_text("Regular expression ($1, ${name} or \\1 in Replace)");
            ui.toggle_value(&mut self.query.case_sensitive, "Aa").on_hover_text("Case sensitive");
            ui.toggle_value(&mut self.query.whole_word, "\"ab\"").on_hover_text("Whole word");
        });
        use crate::find_history::Field;
        let mut field = |ui: &mut Ui, label: &str, text: &mut String, id: &str, hint: &str, focus: bool, history: Field| {
            crate::fonts::ensure_cjk(ui.ctx(), text);
            ui.horizontal(|ui| {
                let small = egui::TextStyle::Small.resolve(ui.style()).size;
                ui.add_sized([(small * 4.4).max(48.0), 18.0], egui::Label::new(RichText::new(label).small().weak()));
                let id = egui::Id::new(id);
                crate::find_history::keys(ui, id, history, text);
                let r = crate::ctxmenu::text_field(ui, id, text, |ui, text| {
                    ui.add(egui::TextEdit::singleline(text).id(id).hint_text(hint).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY))
                });
                if focus {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    find = true;
                }
            });
        };
        let (fq, fr) = (std::mem::take(&mut self.focus_query), std::mem::take(&mut self.focus_replace));
        let (find_hint, where_hint) = if self.roots.is_empty() {
            ("Find in open files", "<open files> (no folder is open), folder, *.rs")
        } else {
            ("Find in folders", "folder, *.rs, -*.min.js, <open files>")
        };
        field(ui, "Find", &mut self.query.text, "find-in-files", find_hint, fq, Field::FilesFind);
        field(ui, "Where", &mut self.where_text, "find-in-files-where", where_hint, false, Field::FilesWhere);
        field(ui, "Replace", &mut self.replacement, "find-in-files-replace", "Replace with", fr, Field::FilesReplace);
        ui.horizontal(|ui| {
            find |= ui.add_enabled(!busy, egui::Button::new("Find")).clicked();
            replace = ui
                .add_enabled(!busy && !self.query.text.is_empty(), egui::Button::new("Replace…"))
                .on_hover_text("Replace every match (asks first). Open files change in their tabs, unsaved; the rest are rewritten on disk.")
                .clicked();
            if self.search.running || busy {
                ui.spinner();
                if ui.small_button("Stop").clicked() {
                    self.search.stop();
                    self.pending_replace = false;
                    self.disk.cancel();
                }
            }
        });
        if (find || replace) && !busy {
            crate::find_history::record(Field::FilesFind, &self.query.text);
            crate::find_history::record(Field::FilesWhere, &self.where_text);
            if replace {
                crate::find_history::record(Field::FilesReplace, &self.replacement);
            }
        }
        if find && !busy {
            self.report = None;
            self.pending_replace = false;
            self.run_search(open, Some(ui.ctx().clone()));
        }
        if replace && !busy {
            self.request_replace(open, Some(ui.ctx().clone()));
        }
        if busy {
            let (done, total) = self.disk.progress();
            let label = if self.disk.undoing { "Checking files, then restoring them…".to_owned() } else { format!("Replacing… {done} of {total} files") };
            ui.label(RichText::new(label).small().weak());
        }
        if let Some((text, skipped)) = &self.report {
            ui.label(RichText::new(text).small());
            if !skipped.is_empty() {
                egui::CollapsingHeader::new(RichText::new(format!("{} skipped", skipped.len())).small()).id_salt("replace-skipped").default_open(skipped.len() <= 5).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        for (path, why) in skipped {
                            let line = if path.as_os_str().is_empty() { why.clone() } else { format!("{}: {why}", self.display_path(path)) };
                            ui.label(RichText::new(line).small().weak());
                        }
                    });
                });
            }
        }
        let undoable = self.disk.undoable();
        if undoable > 0 && !busy {
            let s = if undoable == 1 { "" } else { "s" };
            let tip = format!("Restore the {undoable} file{s} rewritten on disk (any changed since are left alone). For open tabs, use Undo in each tab.");
            if ui.small_button("Undo Replace in Files").on_hover_text(tip).clicked() {
                self.undo_replace(Some(ui.ctx().clone()));
            }
        }
        ui.label(self.summary());
        ui.separator();
        self.results(ui)
    }

    fn summary(&self) -> RichText {
        if let Some(e) = &self.search.error {
            return RichText::new(e).color(Color32::LIGHT_RED);
        }
        let hits = self.search.total_hits();
        let files = self.search.results.len();
        let scanned = self.search.files_scanned();
        if scanned == 0 && !self.search.running {
            return RichText::new("");
        }
        let more = if self.search.truncated { "+ (stopped at limit)" } else { "" };
        RichText::new(format!("{hits}{more} matches in {files} files · {scanned} scanned")).small().weak()
    }

    fn results(&self, ui: &mut Ui) -> Option<Action> {
        // Flatten into rows so only visible ones are laid out.
        let rows: Vec<(usize, Option<usize>)> = self
            .search
            .results
            .iter()
            .enumerate()
            .flat_map(|(fi, f)| std::iter::once((fi, None)).chain((0..f.hits.len()).map(move |hi| (fi, Some(hi)))))
            .collect();
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
        let mut action = None;
        egui::ScrollArea::both().auto_shrink(false).show_rows(ui, row_h, rows.len(), |ui, range| {
            // One line per row (scroll sideways for long previews) keeps rows fixed-height.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for &(fi, hi) in &rows[range] {
                let file = &self.search.results[fi];
                match hi {
                    None => {
                        crate::fonts::ensure_cjk(ui.ctx(), &file.path.to_string_lossy());
                        ui.label(RichText::new(self.display_path(&file.path)).strong()).on_hover_text(file.path.display().to_string());
                    }
                    Some(hi) => {
                        let hit = &file.hits[hi];
                        crate::fonts::ensure_cjk(ui.ctx(), &hit.preview);
                        if ui.add(egui::Button::new(hit_text(ui, hit.line, &hit.preview, &hit.preview_cols)).frame(false)).clicked() {
                            action = Some(Action::OpenAt(file.path.clone(), hit.line, hit.cols.clone()));
                        }
                    }
                }
            }
        });
        action
    }
}

/// One folder's entries (not its subfolders'), leaving out what .gitignore
/// ignores when `gitignore` is on.
fn read_folder(dir: &Path, gitignore: bool) -> Vec<Entry> {
    let entry = |path: PathBuf, is_dir: bool| {
        let name = path.file_name()?.to_string_lossy().into_owned();
        Some(Entry { path, name, is_dir })
    };
    if !gitignore {
        return std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| entry(e.path(), e.file_type().ok()?.is_dir())).collect();
    }
    ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .require_git(false)
        .build()
        .flatten()
        .filter(|e| e.depth() == 1)
        .filter_map(|e| {
            // A symlink to a folder lists as a folder.
            let is_dir = e.file_type()?.is_dir() || (e.path_is_symlink() && e.path().is_dir());
            entry(e.into_path(), is_dir)
        })
        .collect()
}

fn exclude_set(patterns: &[String]) -> globset::GlobSet {
    let mut b = globset::GlobSetBuilder::new();
    for p in patterns {
        if let Ok(g) = globset::Glob::new(p) {
            b.add(g);
        }
    }
    b.build().unwrap_or_else(|_| globset::GlobSet::empty())
}

/// Names Windows reserves for devices, with or without an extension.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A name typed for a new file or folder (`nested`: `a/b/c` makes the
/// folders in between) or for a rename (one name only).
fn check_name(name: &str, nested: bool) -> Result<(), String> {
    if name.is_empty() {
        return Err("Type a name".into());
    }
    if name.contains('\\') {
        return Err(if nested { "Use / between folder names, not \\" } else { "A name can't contain \\" }.into());
    }
    if !nested && name.contains('/') {
        return Err("A name can't contain /".into());
    }
    name.split('/').try_for_each(check_component)
}

/// One file or folder name.
fn check_component(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Empty folder name (two slashes, or one at the start or end)".into());
    }
    if name == "." || name == ".." {
        return Err(format!("“{name}” isn't a name"));
    }
    if name.contains('\0') {
        return Err("A name can't contain a null character".into());
    }
    if cfg!(windows) {
        if let Some(c) = name.chars().find(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (*c as u32) < 32) {
            return Err(format!("A name can't contain {c:?} on Windows"));
        }
        let stem = name.split('.').next().unwrap_or(name).trim_end().to_uppercase();
        if RESERVED.contains(&stem.as_str()) {
            return Err(format!("“{name}” is reserved by Windows"));
        }
        if name.ends_with(['.', ' ']) {
            return Err("A name can't end with a dot or a space on Windows".into());
        }
    }
    Ok(())
}

/// Are `a` and `b` the same file (e.g. names differing only in case on a
/// case-insensitive disk)?
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        // Windows disks ignore case: the same name in other letters is the same file.
        a.parent() == b.parent() && a.file_name().map(|n| n.to_string_lossy().to_lowercase()) == b.file_name().map(|n| n.to_string_lossy().to_lowercase())
    }
}

/// Rename `from` to `to`; a change of case only goes through a temporary
/// name, which file systems that ignore case need.
pub(crate) fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    if !same_file(from, to) {
        return std::fs::rename(from, to);
    }
    let tmp = (0..).map(|n| from.with_file_name(format!(".strata-rename-{}-{n}", std::process::id()))).find(|p| std::fs::symlink_metadata(p).is_err()).expect("a free name");
    std::fs::rename(from, &tmp)?;
    std::fs::rename(&tmp, to).inspect_err(|_| {
        let _ = std::fs::rename(&tmp, from);
    })
}

/// For comparing paths that may differ only by symlinks or `..`.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

/// "  42  preview text" with the match highlighted.
fn hit_text(ui: &Ui, line: usize, preview: &str, cols: &Range<usize>) -> LayoutJob {
    let font = FontId::monospace(egui::TextStyle::Small.resolve(ui.style()).size);
    let fg = ui.visuals().text_color();
    let dim = TextFormat::simple(font.clone(), ui.visuals().weak_text_color());
    let normal = TextFormat::simple(font.clone(), fg);
    let mut hit = TextFormat::simple(font, ui.visuals().strong_text_color());
    hit.background = ui.visuals().selection.bg_fill;
    let byte = |c: usize| preview.char_indices().nth(c).map_or(preview.len(), |(b, _)| b);
    let (a, b) = (byte(cols.start), byte(cols.end));
    let mut job = LayoutJob::default();
    job.append(&format!("{:>5}  ", line + 1), 0.0, dim);
    job.append(&preview[..a], 0.0, normal.clone());
    job.append(&preview[a..b], 0.0, hit);
    job.append(&preview[b..], 0.0, normal);
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-sidebar-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn finish(s: &mut Sidebar) {
        while s.search.running {
            s.search.poll();
            std::thread::yield_now();
        }
    }

    fn open_file(path: &Path) -> OpenFile {
        OpenFile { name: path.file_name().unwrap().to_string_lossy().into_owned(), dirty: false, active: false, path: Some(path.to_owned()), unsaved: None }
    }

    #[test]
    fn several_folders_are_roots_and_all_are_searched() {
        let dir = temp_dir("roots");
        let (a, b) = (dir.join("alpha"), dir.join("beta"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x.txt"), "needle a\n").unwrap();
        std::fs::write(b.join("y.txt"), "needle b\n").unwrap();
        let mut s = Sidebar::new();
        s.open_folder(a.clone());
        assert!(s.add_folder(b.clone()));
        assert!(!s.add_folder(b.clone()), "a folder is added once");
        assert_eq!(s.roots, [a.clone(), b.clone()]);
        s.query = Query::literal("needle", false);
        s.run_search(&[], None);
        finish(&mut s);
        assert_eq!(s.search.results.len(), 2, "an empty Where searches every folder");
        s.where_text = "<open folders>".into();
        s.run_search(&[], None);
        finish(&mut s);
        assert_eq!(s.search.results.len(), 2);
        assert_eq!(s.display_path(&b.join("y.txt")), "beta/y.txt", "results say which folder");
        assert_eq!(s.root_of(&b.join("y.txt")), Some(b.as_path()));
        assert!(s.reveal(&b.join("y.txt")), "files in any folder can be revealed");
        s.remove_folder(&a);
        assert_eq!(s.roots, std::slice::from_ref(&b));
        assert_eq!(s.display_path(&b.join("y.txt")), "y.txt");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn without_a_folder_find_in_files_searches_open_files() {
        let dir = temp_dir("nofolder");
        let (open, closed) = (dir.join("open.txt"), dir.join("closed.txt"));
        std::fs::write(&open, "a needle\n").unwrap();
        std::fs::write(&closed, "another needle\n").unwrap();
        let mut s = Sidebar::new();
        s.start_find_in_files(None);
        assert!(s.shown(), "the Search tab shows even with no folder open");
        s.query = Query::literal("needle", false);
        s.run_search(&[open_file(&open)], None);
        finish(&mut s);
        let found: Vec<&Path> = s.search.results.iter().map(|f| f.path.as_path()).collect();
        assert_eq!(found, [open.as_path()], "like Sublime: <open files>");
        s.run_search(&[], None);
        let e = s.search.error.clone().unwrap_or_default();
        assert!(e.contains("open a folder"), "{e}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn find_in_folder_searches_that_folder() {
        let dir = temp_dir("findin");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let mut s = Sidebar::new();
        s.open_folder(dir.clone());
        s.find_in_folder(&dir.join("sub"));
        assert_eq!(s.where_text, dir.join("sub").display().to_string());
        assert!(s.tab == Tab::Search && s.shown());
        std::fs::remove_dir_all(dir).ok();
    }

    fn names(s: &mut Sidebar, dir: &Path) -> Vec<String> {
        s.listing(dir).iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn the_tree_hides_ignored_and_excluded_files() {
        let dir = temp_dir("ignore");
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n*.log\n").unwrap();
        for f in ["main.rs", "debug.log", ".env", "x.pyc", ".DS_Store"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let mut s = Sidebar::new();
        s.open_folder(dir.clone());
        assert_eq!(names(&mut s, &dir), [".env", ".gitignore", "main.rs", "x.pyc"]);
        s.set_filter(&TreeFilter { respect_gitignore: true, exclude: vec!["*.pyc".into(), ".git".into(), ".DS_Store".into(), ".env".into()] });
        assert_eq!(names(&mut s, &dir), [".gitignore", "main.rs"]);
        s.set_filter(&TreeFilter { respect_gitignore: false, exclude: vec![] });
        assert_eq!(names(&mut s, &dir), [".git", "target", ".DS_Store", ".env", ".gitignore", "debug.log", "main.rs", "x.pyc"]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn folders_on_screen_notice_outside_changes() {
        let dir = temp_dir("watch");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let mut s = Sidebar::new();
        s.open_folder(dir.clone());
        assert_eq!(names(&mut s, &dir), ["sub"]);
        names(&mut s, &dir.join("sub"));
        // A frame later (what was drawn is what's watched)…
        s.drawn_before = std::mem::take(&mut s.drawn);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("new.txt"), "").unwrap();
        std::fs::write(dir.join("sub/inner.txt"), "").unwrap();
        s.watch(1.0);
        assert_eq!(names(&mut s, &dir), ["sub"], "not checked more than every couple of seconds");
        s.watch(5.0);
        assert_eq!(names(&mut s, &dir), ["sub", "new.txt"]);
        assert_eq!(names(&mut s, &dir.join("sub")), ["inner.txt"]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn names_are_checked() {
        assert!(check_name("", true).is_err());
        assert!(check_name("a/b.rs", true).is_ok(), "New File makes the folders in between");
        assert!(check_name("a/b.rs", false).is_err(), "a rename is one name");
        for bad in ["..", ".", "a/../b", "a//b", "/abs", "a\\b", "x/"] {
            assert!(check_name(bad, true).is_err(), "{bad}");
        }
        assert!(check_name(".env", false).is_ok());
        assert_eq!(check_name("CON.txt", false).is_err(), cfg!(windows));
        assert_eq!(check_name("a:b", false).is_err(), cfg!(windows));
    }

    /// Run the name prompt as if OK were pressed.
    fn submit(s: &mut Sidebar, kind: PromptKind, target: &Path, name: &str) -> (Option<Action>, Option<String>) {
        s.prompt = Some(Prompt { kind, target: target.to_owned(), name: name.into(), error: None });
        let ctx = egui::Context::default();
        let mut action = None;
        let raw = egui::RawInput { events: vec![egui::Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }], ..Default::default() };
        ctx.run_ui(raw, |ui| action = s.prompt(ui.ctx())).textures_delta.clear();
        (action, s.prompt.as_ref().and_then(|p| p.error.clone()))
    }

    #[test]
    fn new_and_rename_refuse_bad_and_taken_names_and_allow_case_changes() {
        let dir = temp_dir("names");
        std::fs::create_dir_all(dir.join("taken")).unwrap();
        std::fs::write(dir.join("readme.md"), "x").unwrap();
        let mut s = Sidebar::new();
        let (a, e) = submit(&mut s, PromptKind::NewFolder, &dir, "taken");
        assert!(a.is_none() && e.is_some_and(|e| e.contains("already exists")), "New Folder no longer says Created");
        let (a, e) = submit(&mut s, PromptKind::NewFile, &dir, "../escape.txt");
        assert!(a.is_none() && e.is_some() && !dir.parent().unwrap().join("escape.txt").exists());
        let (a, _) = submit(&mut s, PromptKind::NewFile, &dir, "deep/er/file.rs");
        assert!(matches!(a, Some(Action::Open(..))) && dir.join("deep/er/file.rs").is_file());
        // Only the case changes: allowed, also where the disk ignores case.
        let (a, e) = submit(&mut s, PromptKind::Rename, &dir.join("readme.md"), "README.md");
        assert!(matches!(a, Some(Action::Renamed(..))), "{e:?}");
        let listed: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(listed.contains(&"README.md".to_owned()) && !listed.contains(&"readme.md".to_owned()), "{listed:?}");
        assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "x");
        // Onto another existing file: refused.
        let (a, e) = submit(&mut s, PromptKind::Rename, &dir.join("README.md"), "taken");
        assert!(a.is_none() && e.is_some());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn two_roots_render_headlessly() {
        let dir = temp_dir("render");
        let (a, b) = (dir.join("one"), dir.join("two"));
        std::fs::create_dir_all(a.join("src")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("f.rs"), "").unwrap();
        let mut s = Sidebar::new();
        s.open_folder(a);
        s.add_folder(b);
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 400.0))), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                s.show(ui, None, &[]);
            });
            out.textures_delta.clear();
        }
        std::fs::remove_dir_all(dir).ok();
    }
}
