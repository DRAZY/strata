//! The editor window: one buffer, status bar, open/save.

use crate::buffer::Buffer;
use crate::devshot::DevShot;
use crate::editor::Editor;
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

pub struct StrataApp {
    path: Option<PathBuf>,
    editor: Editor,
    highlighter: Highlighter,
    view: EditorView,
    status: String,
    devshot: DevShot,
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
        };
        app.highlighter.detect(None, "");
        if let Some(p) = path {
            app.open(p);
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
        if pressed(&ZOOM_OUT) {
            self.view.font_size = (self.view.font_size - 1.0).max(8.0);
        }
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

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.view.show(ui, &mut self.editor, &mut self.highlighter));
    }
}
