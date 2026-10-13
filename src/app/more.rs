//! More menu features: clipboard history and HTML copy, find-result stepping,
//! pane groups, the recent-file stack, header/implementation switching,
//! references and project symbols, build-system choice, macros, projects.

use super::StrataApp;
use crate::commands::Cmd;
use crate::panes::Layout;
use std::path::{Path, PathBuf};

const CLIP_HISTORY: usize = 15;
/// Presses of Ctrl+Tab closer together than this walk deeper into the stack.
const STACK_WALK_SECS: f64 = 1.5;

/// What a palette `Choice` list is for.
#[derive(Default)]
pub(super) enum Choosing {
    #[default]
    Nothing,
    Build,
    PasteHistory,
    Project(Vec<PathBuf>),
    Symbol(Vec<(PathBuf, usize)>),
    /// Font picker: (editor font?, the fonts offered after "Default").
    Font(bool, Vec<crate::fonts::InstalledFont>),
}

impl StrataApp {
    // ---- clipboard -----------------------------------------------------------------

    /// Remember text the app put on the clipboard this frame (Paste from History).
    pub(super) fn track_copies(&mut self, ctx: &egui::Context) {
        let copied: Vec<String> = ctx.output(|o| {
            o.commands
                .iter()
                .filter_map(|c| match c {
                    egui::OutputCommand::CopyText(t) if !t.is_empty() => Some(t.clone()),
                    _ => None,
                })
                .collect()
        });
        for t in copied {
            self.clip_history.retain(|h| h != &t);
            self.clip_history.insert(0, t);
            self.clip_history.truncate(CLIP_HISTORY);
        }
    }

    pub(super) fn paste_from_history(&mut self, ctx: &egui::Context) {
        if self.clip_history.is_empty() {
            self.status = "Nothing copied yet".into();
            return;
        }
        let labels = self.clip_history.iter().map(|t| one_line(t, 80)).collect();
        self.choose(ctx, Choosing::PasteHistory, "Paste from history", labels);
    }

    /// Copy the selection (or the whole file) as syntax-highlighted HTML.
    pub(super) fn copy_as_html(&mut self) {
        let d = &self.docs[self.active];
        let sel: String = d.editor.selections().iter().filter(|s| !s.is_empty()).map(|s| d.editor.buffer.slice(s.range())).collect::<Vec<_>>().join("\n");
        let text = if sel.is_empty() { d.editor.buffer.slice(0..d.editor.buffer.len_chars()) } else { sel };
        let html = crate::highlight::to_html(&text, d.highlighter.syntax_name(), d.highlighter.theme_name());
        self.status = match arboard::Clipboard::new().and_then(|mut c| c.set_html(&html, Some(&text))) {
            Ok(()) => "Copied as HTML".into(),
            Err(e) => format!("Couldn't copy: {e}"),
        };
    }

    // ---- palette choices ------------------------------------------------------------

    pub(super) fn choose(&mut self, ctx: &egui::Context, what: Choosing, hint: &str, labels: Vec<String>) {
        self.choosing = what;
        let pctx = self.palette_context();
        self.palette.show_choices(hint, labels, pctx, ctx);
    }

    pub(super) fn chose(&mut self, i: usize) {
        match std::mem::take(&mut self.choosing) {
            Choosing::Nothing => {}
            Choosing::Build => {
                if let Some(choice) = self.build_choices.get(i).cloned() {
                    if self.settings.save_all_on_build {
                        self.save_all();
                    }
                    let bctx = self.build_context();
                    if let Err(e) = self.build.build_with(&choice, &bctx) {
                        self.status = e;
                    }
                }
            }
            Choosing::PasteHistory => {
                if let Some(t) = self.clip_history.get(i).cloned() {
                    self.pending_events.push(egui::Event::Paste(t));
                }
            }
            Choosing::Project(paths) => {
                if let Some(p) = paths.get(i).cloned() {
                    self.open_project_path(p);
                }
            }
            Choosing::Font(editor, fonts) => {
                // Row 0 is the built-in font.
                let name = if i == 0 { String::new() } else { fonts.get(i - 1).map(|f| f.name.clone()).unwrap_or_default() };
                if editor {
                    self.settings.font_face = name;
                } else {
                    self.settings.ui_font_face = name;
                }
                self.persist_settings();
            }
            Choosing::Symbol(spots) => {
                if let Some((p, line)) = spots.get(i).cloned() {
                    self.push_jump();
                    self.open_path(p, false);
                    self.goto_line(line);
                }
            }
        }
    }

    pub(super) fn pick_font(&mut self, ctx: &egui::Context, editor: bool) {
        let fonts = crate::fonts::installed_fonts();
        let mut labels = vec![if editor { "Default (Hack)".to_owned() } else { "Default (Ubuntu)".to_owned() }];
        labels.extend(fonts.iter().map(|f| f.name.clone()));
        let hint = if editor { "Editor font (monospaced fonts work best)" } else { "Interface font" };
        self.choose(ctx, Choosing::Font(editor, fonts), hint, labels);
    }

    /// Load the configured fonts and sizes when they change.
    pub(super) fn apply_fonts(&mut self, ctx: &egui::Context) {
        let key = (self.settings.font_face.clone(), self.settings.ui_font_face.clone(), self.settings.ui_font_size.to_bits());
        if self.fonts_key.as_ref() == Some(&key) {
            return;
        }
        self.fonts_key = Some(key);
        let editor = crate::fonts::find_font(&self.settings.font_face);
        let ui = crate::fonts::find_font(&self.settings.ui_font_face);
        let mut errors = crate::fonts::apply(ctx, editor.as_deref(), ui.as_deref());
        if editor.is_none() && !self.settings.font_face.is_empty() {
            errors.push(format!("font \"{}\" not found", self.settings.font_face));
        }
        if ui.is_none() && !self.settings.ui_font_face.is_empty() {
            errors.push(format!("font \"{}\" not found", self.settings.ui_font_face));
        }
        crate::fonts::apply_ui_size(ctx, self.settings.ui_font_size.clamp(9.0, 24.0));
        if !errors.is_empty() {
            self.status = format!("Fonts: {}", errors.join("; "));
        }
    }

    // ---- find results ----------------------------------------------------------------

    /// Step through find-in-folder hits like Sublime's Find Results.
    pub(super) fn step_find_result(&mut self, forward: bool) {
        let hits: Vec<(PathBuf, usize, std::ops::Range<usize>)> = self
            .sidebar
            .search
            .results
            .iter()
            .flat_map(|f| f.hits.iter().map(move |h| (f.path.clone(), h.line, h.cols.clone())))
            .collect();
        if hits.is_empty() {
            self.status = "No find results (Find ▸ Find in Files…)".into();
            return;
        }
        let n = hits.len();
        let i = match self.find_result {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None if forward => 0,
            None => n - 1,
        };
        self.find_result = Some(i);
        let (p, line, cols) = hits[i].clone();
        self.open_at(p, line, cols);
        self.status = format!("Result {} of {n}", i + 1);
    }

    // ---- pane groups ----------------------------------------------------------------

    /// Add a pane: one more column (or row, in a rows layout).
    pub(super) fn new_group(&mut self) {
        let (c, r) = (self.layout.cols.len(), self.layout.rows.len());
        let layout = if c == 1 && r > 1 { Layout::grid(1, (r + 1).min(4)) } else { Layout::grid((c + 1).min(4), r) };
        self.set_layout(layout);
    }

    /// Remove the focused pane; its tabs join the pane before it.
    pub(super) fn close_group(&mut self) {
        let n = self.layout.groups();
        if n < 2 {
            return;
        }
        let gone = self.focus_group;
        let into = gone.saturating_sub(1);
        for d in &mut self.docs {
            if d.group == gone {
                d.group = into;
            } else if d.group > gone {
                d.group -= 1;
            }
        }
        let (c, r) = (self.layout.cols.len(), self.layout.rows.len());
        let layout = if r > 1 && c == 1 { Layout::grid(1, r - 1) } else if c > 1 && r == 1 { Layout::grid(c - 1, 1) } else { Layout::grid(n - 1, 1) };
        self.focus_group = into;
        self.set_layout(layout);
    }

    pub(super) fn cycle_group(&mut self, forward: bool, move_file: bool) {
        let n = self.layout.groups();
        let g = if forward { (self.focus_group + 1) % n } else { (self.focus_group + n - 1) % n };
        if move_file { self.move_to_pane(g) } else { self.focus_pane(g) }
    }

    // ---- recent-file stack (Ctrl+Tab) --------------------------------------------------

    /// Tabs by most recent use; quick repeated presses walk further back.
    pub(super) fn stack_step(&mut self, now: f64, forward: bool) {
        let walking = self.stack_walk.as_ref().is_some_and(|(_, _, t)| now - t < STACK_WALK_SECS);
        if !walking {
            let mut order: Vec<usize> = (0..self.docs.len()).collect();
            order.sort_by_key(|&i| std::cmp::Reverse(self.docs[i].stamp));
            let ids = order.iter().map(|&i| self.docs[i].view.id).collect();
            self.stack_walk = Some((ids, 0, now));
        }
        let Some((ids, pos, t)) = self.stack_walk.as_mut() else { return };
        let n = ids.len();
        if n < 2 {
            return;
        }
        *pos = if forward { (*pos + 1) % n } else { (*pos + n - 1) % n };
        *t = now;
        let id = ids[*pos];
        // Activating restamps tabs; the walk keeps its own order until it times out.
        if let Some(i) = self.docs.iter().position(|d| d.view.id == id) {
            let walk = self.stack_walk.take();
            self.activate(i);
            self.stack_walk = walk;
        }
    }

    // ---- code navigation ----------------------------------------------------------------

    /// Jump between foo.h and foo.c / foo.cpp / foo.m (and back).
    pub(super) fn switch_header(&mut self) {
        let Some(path) = self.docs[self.active].path.clone() else { return };
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let headers = ["h", "hh", "hpp", "hxx", "h++", "inl", "ipp"];
        let sources = ["c", "cc", "cpp", "cxx", "c++", "m", "mm"];
        let wanted: &[&str] = if headers.contains(&ext.as_str()) {
            &sources
        } else if sources.contains(&ext.as_str()) {
            &headers
        } else {
            self.status = "Not a C, C++ or Objective-C file".into();
            return;
        };
        match wanted.iter().map(|e| path.with_extension(e)).find(|p| p.exists()) {
            Some(p) => self.open_path(p, false),
            None => self.status = format!("No matching {} file next to this one", if headers.contains(&ext.as_str()) { "source" } else { "header" }),
        }
    }

    /// Goto Reference: whole-word find in the folder for the word under the cursor.
    pub(super) fn goto_reference(&mut self) {
        let Some(word) = self.word_at_cursor() else { return };
        let Some(root) = self.sidebar.root.clone() else {
            self.find.query = crate::find::Query::literal(&word, true);
            self.open_find(false);
            return;
        };
        self.sidebar.query = crate::find::Query::literal(&word, true);
        self.sidebar.start_find_in_files(None);
        self.sidebar.search.start(&root, &self.sidebar.query, None);
        self.find_result = None;
        self.status = format!("References to {word}: see the Search tab, or step with Find ▸ Find Results");
    }

    /// Every symbol in the folder's files (bounded), offered in the palette.
    pub(super) fn goto_symbol_in_project(&mut self, ctx: &egui::Context) {
        let Some(root) = self.sidebar.root.clone() else {
            return self.run(ctx, Cmd::GotoSymbol);
        };
        let mut labels = Vec::new();
        let mut spots = Vec::new();
        let walker = ignore::WalkBuilder::new(&root).require_git(false).build();
        for entry in walker.flatten().filter(|e| e.file_type().is_some_and(|t| t.is_file())).take(5_000) {
            let p = entry.path();
            let Some(syntax) = crate::highlight::syntax_for_path(p) else { continue };
            if entry.metadata().map_or(true, |m| m.len() > 1_000_000) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(p) else { continue };
            let rel = p.strip_prefix(&root).unwrap_or(p).display().to_string();
            for s in crate::symbols::symbols(&syntax, &text) {
                labels.push(format!("{}  ·  {rel}:{}", s.name, s.line + 1));
                spots.push((p.to_owned(), s.line));
                if spots.len() >= 50_000 {
                    break;
                }
            }
        }
        if spots.is_empty() {
            self.status = "No symbols found in the folder".into();
            return;
        }
        self.choose(ctx, Choosing::Symbol(spots), "Symbol in project", labels);
    }

    // ---- build systems ---------------------------------------------------------------

    pub(super) fn run_build(&mut self) {
        if self.settings.save_all_on_build {
            self.save_all();
        }
        self.reload_build_systems();
        let bctx = self.build_context();
        let chosen = self.build_system.clone().and_then(|name| self.build.choices(&bctx).into_iter().find(|c| c.system == name && c.variant.is_none()));
        let result = match chosen {
            Some(c) => self.build.build_with(&c, &bctx),
            None => self.build.build(&bctx),
        };
        if let Err(e) = result {
            self.status = e;
        }
    }

    pub(super) fn new_build_system(&mut self) {
        let Some(dir) = self.config.clone().map(|c| c.join("build_systems")) else { return };
        let path = (1..).map(|n| dir.join(if n == 1 { "untitled.json".to_owned() } else { format!("untitled-{n}.json") })).find(|p| !p.exists()).expect("a free name");
        let template = r#"{
    "name": "My Build",
    "cmd": ["make"],
    "working_dir": "$folder",
    "selector": { "extensions": ["c", "h"] },
    "file_regex": "^(?P<file>[^:]+):(?P<line>\\d+):(?P<col>\\d+): (?P<message>.*)$"
}
"#;
        match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, template)) {
            Ok(()) => self.open_path(path, false),
            Err(e) => self.status = format!("Couldn't create a build system: {e}"),
        }
    }

    // ---- macros ------------------------------------------------------------------------

    /// Record typed text and editing keys; playback replays them at the cursor(s).
    pub(super) fn toggle_macro_recording(&mut self) {
        match self.macro_recording.take() {
            Some(events) => {
                self.status = format!("Recorded a macro ({} steps). Play it with Tools ▸ Playback Macro", events.len());
                self.macro_last = events;
            }
            None => {
                self.macro_recording = Some(Vec::new());
                self.status = "Recording macro… (Tools ▸ Record Macro again to stop)".into();
            }
        }
    }

    pub(super) fn record_macro_input(&mut self, ctx: &egui::Context) {
        let Some(rec) = self.macro_recording.as_mut() else { return };
        if !self.docs[self.active].view.has_focus(ctx) {
            return;
        }
        ctx.input(|i| {
            for e in &i.events {
                match e {
                    egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Key { pressed: true, .. } => rec.push(e.clone()),
                    _ => {}
                }
            }
        });
    }

    pub(super) fn playback_macro(&mut self) {
        if self.macro_last.is_empty() {
            self.status = "No macro recorded".into();
            return;
        }
        self.docs[self.active].view.focus();
        self.pending_events.extend(self.macro_last.iter().cloned());
    }

    // ---- projects ------------------------------------------------------------------------

    pub(super) fn remember_project(&mut self, p: &Path) {
        self.recent_projects.retain(|q| q != p);
        self.recent_projects.insert(0, p.to_owned());
        self.recent_projects.truncate(15);
        self.project_path = Some(p.to_owned());
    }

    pub(super) fn switch_project(&mut self, ctx: &egui::Context) {
        let paths: Vec<PathBuf> = self.recent_projects.iter().filter(|p| p.exists()).cloned().collect();
        if paths.is_empty() {
            return self.open_project();
        }
        let labels = paths.iter().map(|p| p.file_stem().map_or_else(|| p.display().to_string(), |s| format!("{}  ·  {}", s.to_string_lossy(), p.display()))).collect();
        self.choose(ctx, Choosing::Project(paths), "Switch project", labels);
    }

    pub(super) fn close_project(&mut self) {
        self.sidebar = crate::sidebar::Sidebar::new();
        self.project_path = None;
    }

    pub(super) fn edit_project(&mut self) {
        match self.project_path.clone() {
            Some(p) => self.open_path(p, false),
            None => self.status = "No project is open (Project ▸ Save Project As… creates one)".into(),
        }
    }

    pub(super) fn licenses_window(&mut self, ctx: &egui::Context) {
        let mut open = self.licenses_open;
        egui::Window::new("Licenses and Attribution").open(&mut open).collapsible(false).default_width(460.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label("Strata is MIT licensed. It is built on these open-source projects:");
            ui.add_space(6.0);
            egui::Grid::new("licenses").striped(true).show(ui, |ui| {
                for (name, license) in LICENSES {
                    ui.label(*name);
                    ui.label(egui::RichText::new(*license).weak());
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Syntax definitions come from syntect, the bat project (via two-face) and Sublime Text community packages, each under its own license; color schemes come with syntect. Strata's own PowerShell and JSX grammars are MIT.").small().weak());
            egui::CollapsingHeader::new("Syntax definition licenses").show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    ui.label(egui::RichText::new(crate::highlight::SYNTAX_LICENSES).small().monospace());
                });
            });
        });
        self.licenses_open = open;
    }
}

const LICENSES: &[(&str, &str)] = &[
    ("egui / eframe", "MIT or Apache-2.0"),
    ("wgpu", "MIT or Apache-2.0"),
    ("ropey", "MIT"),
    ("syntect", "MIT"),
    ("two-face (bat's syntax collection)", "MIT or Apache-2.0"),
    ("regex", "MIT or Apache-2.0"),
    ("ignore", "MIT or Unlicense"),
    ("encoding_rs", "Apache-2.0 or MIT (data: BSD-3-Clause)"),
    ("rfd", "MIT"),
    ("muda", "Apache-2.0 or MIT"),
    ("arboard", "MIT or Apache-2.0"),
    ("serde / serde_json", "MIT or Apache-2.0"),
];

/// First line of `text`, shortened for a one-line list.
fn one_line(text: &str, max: usize) -> String {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let mut s: String = first.chars().take(max).collect();
    if first.chars().count() > max || text.trim().lines().count() > 1 {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::document::Document;

    #[test]
    fn groups_open_close_and_cycle() {
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        app.new_group();
        app.new_group();
        assert_eq!(app.layout.groups(), 3);
        app.cycle_group(false, true);
        assert_eq!(app.docs[app.active].group, 2, "moving backwards wraps to the last pane");
        app.close_group();
        assert_eq!(app.layout.groups(), 2);
        assert!(app.docs.iter().all(|d| d.group < 2));
        app.run(&ctx, Cmd::LayoutRows3);
        assert_eq!((app.layout.cols.len(), app.layout.rows.len()), (1, 3));
    }

    #[test]
    fn ctrl_tab_walks_most_recent_first() {
        let mut app = StrataApp::new(None, None);
        for t in ["a", "b", "c"] {
            app.push_doc(Document::from_buffer(None, Buffer::from_str(t)));
        }
        let name = |app: &StrataApp| app.docs[app.active].name();
        app.activate(1); // order of use now: a(1), c(3), b(2)...
        let first = name(&app);
        app.stack_step(10.0, true);
        let second = name(&app);
        assert_ne!(first, second);
        app.stack_step(10.5, true);
        assert_ne!(name(&app), second, "a quick second press goes further back");
        app.stack_step(20.0, true);
        app.stack_step(30.0, true);
        assert_eq!(name(&app), name(&app));
    }

    #[test]
    fn header_switch_finds_the_counterpart() {
        let dir = std::env::temp_dir().join(format!("strata-hdr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.h"), "int f();").unwrap();
        std::fs::write(dir.join("x.cpp"), "int f() {}").unwrap();
        let mut app = StrataApp::new(None, None);
        app.open_path(dir.join("x.h"), false);
        app.switch_header();
        assert_eq!(app.docs[app.active].path, Some(dir.join("x.cpp")));
        app.switch_header();
        assert_eq!(app.docs[app.active].path, Some(dir.join("x.h")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn one_line_labels() {
        assert_eq!(one_line("\n  hello\nworld", 80), "hello…");
        assert_eq!(one_line("short", 80), "short");
    }
}
