//! The editor window: one buffer, gutter, status bar, open/save.

use crate::devshot::DevShot;
use crate::highlight::Highlighter;
use egui::{FontId, Key, KeyboardShortcut, Modifiers};
use std::path::PathBuf;

const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
const NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);

pub struct StrataApp {
    path: Option<PathBuf>,
    text: String,
    saved_text: String,
    highlighter: Highlighter,
    status: String,
    cursor: (usize, usize),
    font_size: f32,
    devshot: DevShot,
}

impl StrataApp {
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut app = Self {
            path: None,
            text: String::new(),
            saved_text: String::new(),
            highlighter: Highlighter::new(),
            status: String::new(),
            cursor: (1, 1),
            font_size: 14.0,
            devshot: DevShot::default(),
        };
        match path {
            Some(p) => app.open(p),
            None => app.highlighter.detect(None, ""),
        }
        app
    }

    fn dirty(&self) -> bool {
        self.text != self.saved_text
    }

    fn open(&mut self, path: PathBuf) {
        // A path that doesn't exist yet opens as a new, empty file.
        let text = if path.exists() {
            match std::fs::read(&path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => {
                    self.status = format!("Could not open {}: {e}", path.display());
                    return;
                }
            }
        } else {
            String::new()
        };
        self.highlighter.detect(Some(&path), &text);
        self.text = text.clone();
        self.saved_text = text;
        self.status = format!("Opened {}", path.display());
        self.path = Some(path);
    }

    fn save(&mut self, choose: bool) {
        let target = match (&self.path, choose) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new().save_file(),
        };
        let Some(path) = target else { return };
        match std::fs::write(&path, &self.text) {
            Ok(()) => {
                self.saved_text = self.text.clone();
                self.status = format!("Saved {}", path.display());
                if self.path.as_ref() != Some(&path) {
                    self.highlighter.detect(Some(&path), &self.text);
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
        let dot = if self.dirty() { " •" } else { "" };
        format!("{name}{dot} — Strata")
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_shortcut(&SAVE_AS)) {
            self.save(true);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&SAVE)) {
            self.save(false);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&OPEN)) {
            if let Some(p) = rfd::FileDialog::new().pick_file() {
                self.open(p);
            }
        }
        if ctx.input_mut(|i| i.consume_shortcut(&NEW)) {
            *self = Self::new(None);
        }
        ctx.input(|i| {
            if i.modifiers.command && i.key_pressed(Key::Plus) || i.modifiers.command && i.key_pressed(Key::Equals) {
                self.font_size = (self.font_size + 1.0).min(48.0);
            }
            if i.modifiers.command && i.key_pressed(Key::Minus) {
                self.font_size = (self.font_size - 1.0).max(8.0);
            }
        });
    }
}

impl eframe::App for StrataApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.handle_shortcuts(ctx);
        self.devshot.tick(ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.title()));

        let font = FontId::monospace(self.font_size);
        let bg = self.highlighter.background();

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Line {}, Column {}", self.cursor.0, self.cursor.1));
                ui.separator();
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(self.highlighter.syntax_name());
                });
            });
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(bg))
            .show(ui, |ui| {
                egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        // Gutter: line numbers share the editor's font so rows align.
                        let lines = self.text.lines().count().max(1)
                            + usize::from(self.text.ends_with('\n'));
                        let width = lines.to_string().len();
                        let numbers: String = (1..=lines)
                            .map(|n| format!("{n:>width$}\n"))
                            .collect();
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(numbers)
                                .font(font.clone())
                                .color(egui::Color32::from_gray(100)),
                        );
                        ui.add_space(8.0);

                        let highlighter = &mut self.highlighter;
                        let layout_font = font.clone();
                        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _w: f32| {
                            let job = highlighter.layout(buf.as_str(), layout_font.clone());
                            ui.fonts_mut(|f| f.layout_job(job))
                        };
                        let out = egui::TextEdit::multiline(&mut self.text)
                            .font(font.clone())
                            .code_editor()
                            .frame(egui::Frame::NONE)
                            .desired_width(f32::INFINITY)
                            .desired_rows(lines)
                            .lock_focus(true)
                            .layouter(&mut layouter)
                            .show(ui);
                        if let Some(range) = out.cursor_range {
                            let idx: usize = range.primary.index.into();
                            let before: String = self.text.chars().take(idx).collect();
                            let line = before.matches('\n').count() + 1;
                            let col = before.rsplit('\n').next().map_or(0, |s| s.chars().count()) + 1;
                            self.cursor = (line, col);
                        }
                    });
                });
            });
    }
}
