//! The editor window: one buffer, status bar, open/save.

use crate::buffer::Buffer;
use crate::devshot::DevShot;
use crate::editor::Editor;
use crate::find::{self, Finder, Query};
use crate::highlight::Highlighter;
use crate::view::EditorView;
use egui::{Key, KeyboardShortcut, Modifiers};
use std::path::PathBuf;

const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
const NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
const ZOOM_IN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Equals);
const ZOOM_IN_PLUS: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Plus);
const ZOOM_OUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Minus);
const FIND: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
const REPLACE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::F);
const REPLACE_WIN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::H);
const FIND_NEXT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::G);
const FIND_PREV: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::G);
const FIND_NEXT_F3: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F3);
const FIND_PREV_F3: KeyboardShortcut = KeyboardShortcut::new(Modifiers::SHIFT, Key::F3);

#[derive(Default)]
struct FindBar {
    open: bool,
    replace: bool,
    query: Query,
    replacement: String,
    focus_query: bool,
    /// Where incremental search starts: the cursor when the bar opened.
    origin: usize,
    last_query: Query,
}

pub struct StrataApp {
    path: Option<PathBuf>,
    editor: Editor,
    highlighter: Highlighter,
    view: EditorView,
    status: String,
    devshot: DevShot,
    find: FindBar,
    finder: Finder,
}

impl StrataApp {
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut app = Self {
            path: None,
            editor: Editor::new(Buffer::from_str("")),
            highlighter: Highlighter::new(),
            view: EditorView::new(),
            status: String::new(),
            devshot: DevShot::default(),
            find: FindBar::default(),
            finder: Finder::default(),
        };
        app.highlighter.detect(None, "");
        if let Some(p) = path {
            app.open(p);
        }
        // Dev aid (pairs with STRATA_SCREENSHOT): open find/replace with a query.
        if let Ok(q) = std::env::var("STRATA_DEV_FIND") {
            app.open_find(true);
            app.find.query.text = q;
            app.find.query.regex = std::env::var_os("STRATA_DEV_FIND_REGEX").is_some();
        }
        app
    }

    fn open(&mut self, path: PathBuf) {
        // A path that doesn't exist yet opens as a new, empty file.
        let buffer = if path.exists() {
            match std::fs::read(&path) {
                Ok(bytes) => Buffer::from_bytes(&bytes),
                Err(e) => {
                    self.status = format!("Could not open {}: {e}", path.display());
                    return;
                }
            }
        } else {
            Buffer::from_str("")
        };
        self.highlighter.detect(Some(&path), &buffer.line_text(0));
        self.editor = Editor::new(buffer);
        self.view.reset();
        self.status = format!("Opened {}", path.display());
        self.path = Some(path);
    }

    fn save(&mut self, choose: bool) {
        let target = match (&self.path, choose) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new().save_file(),
        };
        let Some(path) = target else { return };
        match self.editor.buffer.write_to(&path) {
            Ok(()) => {
                self.editor.mark_saved();
                self.status = format!("Saved {}", path.display());
                if self.path.as_ref() != Some(&path) {
                    self.highlighter.detect(Some(&path), &self.editor.buffer.line_text(0));
                }
                self.path = Some(path);
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into());
        let dot = if self.editor.is_dirty() { " •" } else { "" };
        format!("{name}{dot} — Strata")
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let pressed = |s: &KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(s));
        if pressed(&SAVE_AS) {
            self.save(true);
        }
        if pressed(&SAVE) {
            self.save(false);
        }
        if pressed(&OPEN) {
            if let Some(p) = rfd::FileDialog::new().pick_file() {
                self.open(p);
            }
        }
        if pressed(&NEW) {
            let size = self.view.font_size;
            *self = Self::new(None);
            self.view.font_size = size;
        }
        if pressed(&ZOOM_IN) || pressed(&ZOOM_IN_PLUS) {
            self.view.font_size = (self.view.font_size + 1.0).min(48.0);
        }
        let replace = pressed(&REPLACE) || (!cfg!(target_os = "macos") && pressed(&REPLACE_WIN));
        if replace || pressed(&FIND) {
            self.open_find(replace);
        }
        if pressed(&FIND_PREV) || pressed(&FIND_PREV_F3) {
            self.find_step(false);
        }
        if pressed(&FIND_NEXT) || pressed(&FIND_NEXT_F3) {
            self.find_step(true);
        }
        if self.find.open && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.find.open = false;
            self.view.focus();
        }
        if pressed(&ZOOM_OUT) {
            self.view.font_size = (self.view.font_size - 1.0).max(8.0);
        }
    }
}

impl StrataApp {
    fn open_find(&mut self, replace: bool) {
        let p = self.editor.primary();
        let selected = self.editor.buffer.slice(p.range());
        if !selected.is_empty() && !selected.contains('\n') {
            self.find.query.text = if self.find.query.regex { regex::escape(&selected) } else { selected };
        }
        self.find.open = true;
        self.find.replace |= replace;
        self.find.focus_query = true;
        self.find.origin = p.start();
    }

    fn refresh_matches(&mut self) {
        self.finder.update(&mut self.editor, &self.find.query);
    }

    fn select_match(&mut self, i: usize) {
        let r = self.finder.matches[i].clone();
        self.editor.select_ranges(&[r], 0);
        self.view.reveal();
    }

    /// Cmd+G / Shift+Cmd+G (F3 / Shift+F3): next or previous match.
    fn find_step(&mut self, forward: bool) {
        if self.find.query.text.is_empty() {
            return self.open_find(false);
        }
        self.refresh_matches();
        let p = self.editor.primary();
        let on_match = self.finder.index_of(&p.range()).is_some();
        let i = if forward {
            self.finder.next_from(if on_match { p.end() } else { p.start() })
        } else {
            self.finder.prev_before(p.start())
        };
        if let Some(i) = i {
            self.select_match(i);
        }
    }

    /// Every match becomes a cursor; the bar closes so you can type.
    fn find_all(&mut self) {
        self.refresh_matches();
        if self.finder.matches.is_empty() {
            return;
        }
        let head = self.editor.primary().head;
        let primary = self.finder.next_from(head).unwrap_or(0);
        self.editor.select_ranges(&self.finder.matches.clone(), primary);
        self.find.open = false;
        self.view.focus();
        self.view.reveal();
    }

    fn replace_one(&mut self) {
        self.refresh_matches();
        let p = self.editor.primary();
        if self.finder.index_of(&p.range()).is_some() {
            let Ok(re) = self.find.query.build() else { return };
            let text = self.editor.text_snapshot();
            let byte = self.editor.buffer.char_to_byte(p.start());
            if let Some(rep) = find::expand(&self.find.query, &re, &text, byte, &self.find.replacement) {
                self.editor.replace_ranges(vec![(p.range(), rep)]);
                self.refresh_matches();
            }
        }
        self.find_step(true);
    }

    fn replace_all(&mut self) {
        let Ok(re) = self.find.query.build() else { return };
        let text = self.editor.text_snapshot();
        let mut edits = Vec::new();
        let (mut byte, mut chr) = (0, 0);
        for caps in re.captures_iter(&text) {
            let m = caps.get(0).expect("group 0");
            chr += text[byte..m.start()].chars().count();
            let start = chr;
            chr += m.as_str().chars().count();
            byte = m.end();
            let rep = if self.find.query.regex {
                let mut out = String::new();
                caps.expand(&self.find.replacement, &mut out);
                out
            } else {
                self.find.replacement.clone()
            };
            edits.push((start..chr, rep));
        }
        let n = edits.len();
        self.editor.replace_ranges(edits);
        self.status = format!("Replaced {n} occurrence{}", if n == 1 { "" } else { "s" });
        self.view.reveal();
    }

    fn find_bar(&mut self, ui: &mut egui::Ui) {
        self.refresh_matches();
        // Typing in the query jumps to the first match after where search began.
        if self.find.query != self.find.last_query {
            self.find.last_query = self.find.query.clone();
            if let Some(i) = self.finder.next_from(self.find.origin) {
                self.select_match(i);
            }
        }

        let enter = ui.input(|i| i.key_pressed(Key::Enter));
        let mods = ui.input(|i| i.modifiers);
        let label = self.match_label();
        let mut action = None;
        // A grid keeps the Find and Replace fields in one column.
        egui::Grid::new("find-grid").num_columns(3).show(ui, |ui| {
            let q = &mut self.find.query;
            ui.horizontal(|ui| {
                ui.toggle_value(&mut q.regex, ".*").on_hover_text("Regular expression");
                ui.toggle_value(&mut q.case_sensitive, "Aa").on_hover_text("Case sensitive");
                ui.toggle_value(&mut q.whole_word, "\"ab\"").on_hover_text("Whole word");
            });
            let field = ui.add_sized(
                [320.0, ui.spacing().interact_size.y],
                egui::TextEdit::singleline(&mut q.text)
                    .id(egui::Id::new("find-query"))
                    .hint_text("Find")
                    .font(egui::TextStyle::Monospace),
            );
            if std::mem::take(&mut self.find.focus_query) {
                field.request_focus();
            }
            if field.lost_focus() && enter {
                action = Some(if mods.alt { "all" } else if mods.shift { "prev" } else { "next" });
                field.request_focus();
            }
            ui.horizontal(|ui| {
                ui.add_sized([90.0, 18.0], egui::Label::new(label));
                if ui.button("Previous").clicked() {
                    action = Some("prev");
                }
                if ui.button("Next").clicked() {
                    action = Some("next");
                }
                if ui.button("Find All").on_hover_text("Alt+Enter: a cursor at every match").clicked() {
                    action = Some("all");
                }
            });
            ui.end_row();

            if self.find.replace {
                ui.label("");
                let regex = self.find.query.regex;
                let field = ui.add_sized(
                    [320.0, ui.spacing().interact_size.y],
                    egui::TextEdit::singleline(&mut self.find.replacement)
                        .id(egui::Id::new("replace-text"))
                        .hint_text(if regex { "Replace ($1 for groups)" } else { "Replace" })
                        .font(egui::TextStyle::Monospace),
                );
                if field.lost_focus() && enter {
                    action = Some(if mods.command { "replace-all" } else { "replace" });
                    field.request_focus();
                }
                ui.horizontal(|ui| {
                    ui.add_space(90.0 + ui.spacing().item_spacing.x);
                    if ui.button("Replace").clicked() {
                        action = Some("replace");
                    }
                    if ui.button("Replace All").clicked() {
                        action = Some("replace-all");
                    }
                });
                ui.end_row();
            }
        });
        match action {
            Some("prev") => self.find_step(false),
            Some("next") => self.find_step(true),
            Some("all") => self.find_all(),
            Some("replace") => self.replace_one(),
            Some("replace-all") => self.replace_all(),
            _ => {}
        }
    }

    fn match_label(&self) -> egui::RichText {
        if let Some(e) = &self.finder.error {
            return egui::RichText::new(e).color(egui::Color32::LIGHT_RED);
        }
        if self.find.query.text.is_empty() {
            return egui::RichText::new("");
        }
        let n = self.finder.matches.len();
        let plus = if self.finder.truncated { "+" } else { "" };
        let text = match (n, self.finder.index_of(&self.editor.primary().range())) {
            (0, _) => "No results".to_owned(),
            (_, Some(i)) => format!("{} of {n}{plus}", i + 1),
            _ => format!("{n}{plus} matches"),
        };
        egui::RichText::new(text)
    }
}

impl eframe::App for StrataApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.handle_shortcuts(ctx);
        self.devshot.tick(ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.title()));

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let (line, col) = self.editor.cursor_line_col();
                let cursors = self.editor.selections().len();
                let mut pos = format!("Line {line}, Column {col}");
                if cursors > 1 {
                    pos.push_str(&format!("  ({cursors} cursors)"));
                }
                ui.label(pos);
                ui.separator();
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(self.highlighter.syntax_name());
                    ui.separator();
                    ui.label(match self.editor.buffer.line_ending() {
                        crate::buffer::LineEnding::Lf => "LF",
                        crate::buffer::LineEnding::CrLf => "CRLF",
                    });
                });
            });
        });

        if self.find.open {
            egui::Panel::bottom("find").show(ui, |ui| self.find_bar(ui));
        }

        let matches = if self.find.open { std::mem::take(&mut self.finder.matches) } else { Vec::new() };
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.view.show(ui, &mut self.editor, &mut self.highlighter, &matches));
        if self.find.open {
            self.finder.matches = matches;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, Pos2, RawInput, Rect, vec2};

    fn app_with(text: &str) -> StrataApp {
        let mut app = StrataApp::new(None);
        app.editor = Editor::new(Buffer::from_str(text));
        app
    }

    fn text(app: &StrataApp) -> String {
        app.editor.buffer.slice(0..app.editor.buffer.len_chars())
    }

    fn query(app: &mut StrataApp, text: &str, regex: bool) {
        app.find.query = Query { text: text.into(), regex, case_sensitive: true, whole_word: false };
    }

    #[test]
    fn find_next_prev_wrap() {
        let mut app = app_with("cat dog cat dog");
        query(&mut app, "dog", false);
        app.find_step(true);
        assert_eq!(app.editor.primary().range(), 4..7);
        app.find_step(true);
        assert_eq!(app.editor.primary().range(), 12..15);
        app.find_step(true);
        assert_eq!(app.editor.primary().range(), 4..7, "wraps to top");
        app.find_step(false);
        assert_eq!(app.editor.primary().range(), 12..15, "wraps to bottom");
    }

    #[test]
    fn find_all_makes_cursors_and_closes_bar() {
        let mut app = app_with("x1 x2 x3");
        app.open_find(false);
        query(&mut app, r"x\d", true);
        app.find_all();
        assert_eq!(app.editor.selections().len(), 3);
        assert!(!app.find.open);
        app.editor.insert_text("y");
        assert_eq!(text(&app), "y y y");
    }

    #[test]
    fn replace_all_regex_groups_is_one_undo_step() {
        let mut app = app_with("a=1\nb=22\n");
        query(&mut app, r"(\w)=(\d+)", true);
        app.find.replacement = "$2:$1".into();
        app.replace_all();
        assert_eq!(text(&app), "1:a\n22:b\n");
        assert_eq!(app.status, "Replaced 2 occurrences");
        app.editor.undo();
        assert_eq!(text(&app), "a=1\nb=22\n");
    }

    #[test]
    fn replace_one_replaces_current_then_moves_on() {
        let mut app = app_with("aa bb aa");
        query(&mut app, "aa", false);
        app.find.replacement = "cc".into();
        app.replace_one(); // first press only selects the match
        assert_eq!(app.editor.primary().range(), 0..2);
        app.replace_one();
        assert_eq!(text(&app), "cc bb aa");
        assert_eq!(app.editor.primary().range(), 6..8);
    }

    #[test]
    fn typing_a_query_jumps_and_counts() {
        let mut app = app_with("one two one two");
        app.editor.select_ranges(&[5..5], 0);
        app.open_find(false);
        query(&mut app, "one", false);
        let ctx = Context::default();
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 200.0))),
            events: vec![Event::ModifiersChanged(Modifiers::NONE)],
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| app.find_bar(ui));
        out.textures_delta.clear();
        assert_eq!(app.editor.primary().range(), 8..11, "first match after the cursor");
        assert_eq!(app.match_label().text(), "2 of 2");
    }

    #[test]
    fn bad_regex_shows_error_instead_of_matches() {
        let mut app = app_with("abc");
        query(&mut app, "(", true);
        app.refresh_matches();
        assert!(app.finder.error.is_some());
        assert!(app.finder.matches.is_empty());
    }
}
