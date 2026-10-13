//! Left sidebar: the open folder's file tree and find-in-files results.

use crate::find::Query;
use crate::replace::{DiskReplace, Job, Replacer};
use crate::search::{Scope, Search};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Key, RichText, Ui};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Never shown in the tree.
const HIDDEN: &[&str] = &[".git", ".DS_Store"];

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Tab {
    Files,
    Search,
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
    Status(String),
    /// The user confirmed Replace in Files: edit open tabs, then call
    /// [`Sidebar::start_disk_replace`] with the rest.
    ReplaceInFiles,
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

pub struct Sidebar {
    pub root: Option<PathBuf>,
    pub visible: bool,
    pub tab: Tab,
    /// Directory listings, read once per folder until refreshed.
    listings: HashMap<PathBuf, Vec<Entry>>,
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
            root: None,
            visible: true,
            tab: Tab::Files,
            listings: HashMap::new(),
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

    /// Show `path` in the tree: open its folders, scroll to it. False if it
    /// isn't inside the open folder.
    pub fn reveal(&mut self, path: &Path) -> bool {
        if !self.root.as_deref().is_some_and(|r| path.starts_with(r)) {
            return false;
        }
        self.visible = true;
        self.tab = Tab::Files;
        self.reveal = Some((path.to_owned(), 60));
        self.refresh();
        true
    }

    pub fn open_folder(&mut self, root: PathBuf) {
        self.search.stop();
        self.search = Search::default();
        self.listings.clear();
        self.root = Some(root);
        self.visible = true;
        self.tab = Tab::Files;
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
        let open_paths: Vec<PathBuf> = open.iter().filter_map(|f| f.path.clone()).collect();
        match Scope::parse(&self.where_text, self.root.as_deref(), &open_paths) {
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
            format!("Undo Replace in Files: restored {}", plural(d.files, "file"))
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
        self.search.poll();
        let finished = self.poll_replace(open, Some(ui.ctx().clone())).map(Action::Status);
        if self.pending_replace && !self.search.running {
            self.pending_replace = false;
            self.ask_to_replace(open);
        }
        let confirmed = self.confirm_replace(ui.ctx());
        let prompted = self.prompt(ui.ctx()).or(confirmed).or(finished);
        let Some(root) = self.root.clone() else {
            ui.add_space(8.0);
            ui.label(RichText::new("No folder open").weak());
            ui.label(RichText::new("Cmd+Shift+O to open one").weak().small());
            return None;
        };
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Files, "Files");
            ui.selectable_value(&mut self.tab, Tab::Search, "Search");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.tab == Tab::Files && ui.small_button("⟳").on_hover_text("Refresh").clicked() {
                    self.refresh();
                }
            });
        });
        ui.separator();
        let action = match self.tab {
            Tab::Files => {
                let mut action = None;
                egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    if self.show_open_files && open.len() > 1 {
                        egui::CollapsingHeader::new(RichText::new("OPEN FILES").small().weak()).id_salt("open-files").default_open(true).show(ui, |ui| {
                            for (i, f) in open.iter().enumerate() {
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
                    let name = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
                    let mut header = egui::CollapsingHeader::new(RichText::new(name).strong()).id_salt(&root).default_open(true);
                    if self.reveal.is_some() {
                        header = header.open(Some(true));
                    }
                    let header = header.show(ui, |ui| self.tree(ui, &root, current, &mut action));
                    header.header_response.context_menu(|ui| self.menu(ui, &root, true, &mut action));
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
            Tab::Search => self.search_tab(ui, &root, open),
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
        if Some(path) != self.root.as_deref() && ui.button("Rename…").clicked() {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            ask(PromptKind::Rename, path.to_owned(), name);
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
        if ui.button("Find in Folder…").clicked() {
            self.start_find_in_files(None);
            ui.close();
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
        if !done || p.name.trim().is_empty() {
            return None;
        }
        let name = p.name.trim().to_owned();
        let result = match p.kind {
            PromptKind::NewFile => {
                let path = p.target.join(&name);
                if path.exists() {
                    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "already exists"))
                } else {
                    path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, "")).map(|_| Action::Open(path, false))
                }
            }
            PromptKind::NewFolder => std::fs::create_dir_all(p.target.join(&name)).map(|_| Action::Status(format!("Created {name}"))),
            PromptKind::Rename => {
                let to = p.target.with_file_name(&name);
                if to.exists() {
                    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "already exists"))
                } else {
                    std::fs::rename(&p.target, &to).map(|_| Action::Renamed(p.target.clone(), to))
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
        self.listings.entry(dir.to_owned()).or_insert_with(|| {
            let mut entries: Vec<Entry> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if HIDDEN.contains(&name.as_str()) {
                        return None;
                    }
                    let is_dir = e.file_type().ok()?.is_dir();
                    Some(Entry { path: e.path(), name, is_dir })
                })
                .collect();
            // Folders first, then case-insensitive by name.
            entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            entries
        })
    }

    fn tree(&mut self, ui: &mut Ui, dir: &Path, current: Option<&Path>, action: &mut Option<Action>) {
        // Collect first: rendering children needs `&mut self` again.
        let entries: Vec<(PathBuf, String, bool)> =
            self.listing(dir).iter().map(|e| (e.path.clone(), e.name.clone(), e.is_dir)).collect();
        for (path, name, is_dir) in entries {
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

    fn search_tab(&mut self, ui: &mut Ui, root: &Path, open: &[OpenFile]) -> Option<Action> {
        let (mut find, mut replace) = (false, false);
        let busy = self.disk.running();
        ui.horizontal(|ui| {
            ui.toggle_value(&mut self.query.regex, ".*").on_hover_text("Regular expression ($1, ${name} or \\1 in Replace)");
            ui.toggle_value(&mut self.query.case_sensitive, "Aa").on_hover_text("Case sensitive");
            ui.toggle_value(&mut self.query.whole_word, "\"ab\"").on_hover_text("Whole word");
        });
        let mut field = |ui: &mut Ui, label: &str, text: &mut String, id: &str, hint: &str, focus: bool| {
            ui.horizontal(|ui| {
                ui.add_sized([48.0, 18.0], egui::Label::new(RichText::new(label).small().weak()));
                let id = egui::Id::new(id);
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
        field(ui, "Find", &mut self.query.text, "find-in-files", "Find in folder", fq);
        field(ui, "Where", &mut self.where_text, "find-in-files-where", "folder, *.rs, -*.min.js, <open files>", false);
        field(ui, "Replace", &mut self.replacement, "find-in-files-replace", "Replace with", fr);
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
            let what = if self.disk.undoing { "Restoring" } else { "Replacing" };
            ui.label(RichText::new(format!("{what}… {done} of {total} files")).small().weak());
        }
        if let Some((text, skipped)) = &self.report {
            ui.label(RichText::new(text).small());
            if !skipped.is_empty() {
                egui::CollapsingHeader::new(RichText::new(format!("{} skipped", skipped.len())).small()).id_salt("replace-skipped").default_open(skipped.len() <= 5).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        for (path, why) in skipped {
                            let line = if path.as_os_str().is_empty() { why.clone() } else { format!("{}: {why}", path.strip_prefix(root).unwrap_or(path).display()) };
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
        self.results(ui, root)
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

    fn results(&self, ui: &mut Ui, root: &Path) -> Option<Action> {
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
                        let rel = file.path.strip_prefix(root).unwrap_or(&file.path);
                        ui.label(RichText::new(rel.display().to_string()).strong());
                    }
                    Some(hi) => {
                        let hit = &file.hits[hi];
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

/// For comparing paths that may differ only by symlinks or `..`.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

/// "  42  preview text" with the match highlighted.
fn hit_text(ui: &Ui, line: usize, preview: &str, cols: &Range<usize>) -> LayoutJob {
    let font = FontId::monospace(12.0);
    let fg = ui.visuals().text_color();
    let dim = TextFormat::simple(font.clone(), fg.gamma_multiply(0.5));
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
