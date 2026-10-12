//! Left sidebar: the open folder's file tree and find-in-files results.

use crate::find::Query;
use crate::search::Search;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Key, RichText, Ui};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

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
}

/// A tab as the Open Files list shows it.
pub struct OpenFile {
    pub name: String,
    pub dirty: bool,
    pub active: bool,
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
        }
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

    pub fn refresh(&mut self) {
        self.listings.clear();
    }

    pub fn show(&mut self, ui: &mut Ui, current: Option<&Path>, open: &[OpenFile]) -> Option<Action> {
        self.search.poll();
        let prompted = self.prompt(ui.ctx());
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
                    let header = egui::CollapsingHeader::new(RichText::new(name).strong())
                        .id_salt(&root)
                        .default_open(true)
                        .show(ui, |ui| self.tree(ui, &root, current, &mut action));
                    header.header_response.context_menu(|ui| self.menu(ui, &root, true, &mut action));
                });
                action
            }
            Tab::Search => self.search_tab(ui, &root),
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
        let reveal = if cfg!(target_os = "macos") { "Reveal in Finder" } else { "Show in Explorer" };
        if ui.button(reveal).clicked() {
            if let Err(e) = reveal_in_file_manager(path) {
                *action = Some(Action::Status(format!("Could not open the file manager: {e}")));
            }
            ui.close();
        }
        if ui.button("Find in Folder…").clicked() {
            self.start_find_in_files(None);
            ui.close();
        }
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
                let header = egui::CollapsingHeader::new(&name)
                    .id_salt(&path)
                    .default_open(false)
                    .show(ui, |ui| self.tree(ui, &path, current, action));
                header.header_response.context_menu(|ui| self.menu(ui, &path, true, action));
            } else {
                let r = ui.selectable_label(current == Some(path.as_path()), &name);
                r.context_menu(|ui| self.menu(ui, &path, false, action));
                if r.double_clicked() {
                    *action = Some(Action::Open(path, false));
                } else if r.clicked() {
                    *action = Some(Action::Open(path, true));
                }
            }
        }
    }

    fn search_tab(&mut self, ui: &mut Ui, root: &Path) -> Option<Action> {
        let mut run = false;
        ui.horizontal(|ui| {
            ui.toggle_value(&mut self.query.regex, ".*").on_hover_text("Regular expression");
            ui.toggle_value(&mut self.query.case_sensitive, "Aa").on_hover_text("Case sensitive");
            ui.toggle_value(&mut self.query.whole_word, "\"ab\"").on_hover_text("Whole word");
        });
        let field = ui.add(
            egui::TextEdit::singleline(&mut self.query.text)
                .id(egui::Id::new("find-in-files"))
                .hint_text("Find in folder")
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut self.focus_query) {
            field.request_focus();
        }
        if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            run = true;
        }
        ui.horizontal(|ui| {
            if ui.button("Find").clicked() {
                run = true;
            }
            if self.search.running {
                ui.spinner();
                if ui.small_button("Stop").clicked() {
                    self.search.stop();
                }
            }
        });
        if run {
            self.search.start(root, &self.query, Some(ui.ctx().clone()));
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

fn reveal_in_file_manager(path: &Path) -> std::io::Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    cmd.spawn().map(|_| ())
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
