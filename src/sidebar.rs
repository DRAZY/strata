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

    pub fn show(&mut self, ui: &mut Ui, current: Option<&Path>) -> Option<Action> {
        self.search.poll();
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
        match self.tab {
            Tab::Files => {
                let mut action = None;
                egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    let name = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
                    egui::CollapsingHeader::new(RichText::new(name).strong())
                        .id_salt(&root)
                        .default_open(true)
                        .show(ui, |ui| self.tree(ui, &root, current, &mut action));
                });
                action
            }
            Tab::Search => self.search_tab(ui, &root),
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
                egui::CollapsingHeader::new(&name)
                    .id_salt(&path)
                    .default_open(false)
                    .show(ui, |ui| self.tree(ui, &path, current, action));
            } else {
                let r = ui.selectable_label(current == Some(path.as_path()), &name);
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
