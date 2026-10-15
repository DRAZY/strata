//! Preferences ▸ Settings… (⌘, / Ctrl+,): every setting as a form, by
//! category, with search. In its own native window when the platform has
//! them, else docked over the editor like the help.
//!
//! What a setting is and how it's edited comes from `settings_meta`. A change
//! applies at once and goes to settings.json the way a menu toggle does
//! (`persist_settings`: only that key is rewritten, comments kept), and the
//! other windows pick it up through `windows`. While settings.json has an
//! error the form is read-only, so the broken file is never overwritten.

use super::StrataApp;
use crate::session::Settings;
use crate::settings_meta::{self as meta, Category, Control, Meta};
use egui::{Color32, Key, RichText, Sense, vec2};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::time::SystemTime;

pub struct SettingsUi {
    pub open: bool,
    /// The window has the keyboard (its own window is focused, or the
    /// docked one was clicked last).
    pub active: bool,
    category: Category,
    pub search: String,
    focus_search: bool,
    /// A field had the keyboard last frame (egui drops focus on Esc before we see it).
    search_focused: bool,
    field_focused: bool,
    /// Text being typed into a field, by key: committed when it loses focus.
    drafts: HashMap<&'static str, String>,
    /// settings.json's error, if it has one, and the modification time it was read at.
    file_error: Option<String>,
    file_checked: Option<Option<SystemTime>>,
    /// Font names for the pickers (editor, interface), found when first opened.
    fonts: [Option<Vec<String>>; 2],
    position: Option<egui::Pos2>,
    /// The docked window's rect last frame, to tell clicks inside from outside.
    rect: Option<egui::Rect>,
    /// Input made by menu items (copy, paste…) for the own window's next frame.
    pub pending: Vec<egui::Event>,
    scroll_top: bool,
    /// Bring the own window to the front (opened again while open).
    raise: bool,
}

impl Default for SettingsUi {
    fn default() -> Self {
        Self {
            open: false,
            active: false,
            category: Category::Appearance,
            search: String::new(),
            focus_search: false,
            search_focused: false,
            field_focused: false,
            drafts: HashMap::new(),
            file_error: None,
            file_checked: None,
            fonts: [None, None],
            position: None,
            rect: None,
            pending: Vec::new(),
            scroll_top: false,
            raise: false,
        }
    }
}

impl SettingsUi {
    #[cfg(test)]
    pub fn category_for_shots(&mut self, c: Category) {
        self.category = c;
    }
}

/// What the form asks for, carried out after drawing.
#[derive(Debug, PartialEq)]
pub(super) enum Change {
    /// Set a setting; `persist` writes settings.json (not while a slider is dragged).
    Set { key: &'static str, value: Value, persist: bool },
    Reset(&'static str),
    /// Open settings.json, at a key if given.
    OpenJson(Option<&'static str>),
    Copy(&'static str),
}

/// The JSON key the other per-language settings live under (written by hand).
const SYNTAX_SETTINGS: &str = "syntax_settings";

impl StrataApp {
    /// Preferences ▸ Settings…: open the window (or bring it forward).
    pub(super) fn open_settings_window(&mut self) {
        let s = &mut self.settings_ui;
        s.raise = s.open;
        s.open = true;
        s.active = true;
        s.focus_search = true;
        s.file_checked = None;
    }

    pub(super) fn close_settings_window(&mut self) {
        let s = &mut self.settings_ui;
        s.open = false;
        s.active = false;
        s.drafts.clear();
        self.doc().view.focus();
    }

    /// Commands that mean something else while the Settings window has the keyboard.
    pub(super) fn settings_takes(&mut self, cmd: crate::commands::Cmd) -> bool {
        use crate::commands::Cmd;
        if !(self.settings_ui.open && self.settings_ui.active) {
            return false;
        }
        match cmd {
            Cmd::Find | Cmd::IncrementalFind | Cmd::FindInFiles => self.settings_ui.focus_search = true,
            Cmd::CloseTab | Cmd::CloseWindow => self.close_settings_window(),
            _ => return false,
        }
        true
    }

    /// Where menu-made input (copy, paste, undo…) goes: the Settings window's
    /// own viewport when it has the keyboard, else this window.
    pub(super) fn menu_input(&mut self, ctx: &egui::Context) -> &mut Vec<egui::Event> {
        if self.settings_has_keys(ctx) { &mut self.settings_ui.pending } else { &mut self.pending_events }
    }

    /// The Settings window is a window of its own and has the keyboard.
    pub(super) fn settings_has_keys(&self, ctx: &egui::Context) -> bool {
        self.settings_ui.open && self.settings_ui.active && self.settings_in_own_window(ctx)
    }

    /// Keys for the docked window, before the keymap sees them.
    pub(super) fn settings_keys(&mut self, ctx: &egui::Context) {
        if !self.settings_ui.open || self.settings_in_own_window(ctx) {
            return;
        }
        let press = ctx.input(|i| i.events.iter().find_map(|e| match e {
            egui::Event::PointerButton { pos, pressed: true, .. } => Some(*pos),
            _ => None,
        }));
        if let (Some(p), Some(r)) = (press, self.settings_ui.rect) {
            self.settings_ui.active = r.contains(p);
        }
        self.settings_keys_in(ctx);
    }

    fn settings_keys_in(&mut self, ctx: &egui::Context) {
        if !self.settings_ui.active || self.palette.open {
            return;
        }
        let s = &mut self.settings_ui;
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            if s.search_focused && !s.search.is_empty() {
                s.search.clear();
                s.focus_search = true;
            } else if s.field_focused && !s.search_focused {
                // Esc leaves the field first.
            } else {
                self.close_settings_window();
            }
        }
    }

    fn settings_in_own_window(&self, ctx: &egui::Context) -> bool {
        !ctx.embed_viewports()
    }

    /// The Settings window, when open.
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_ui.open {
            return;
        }
        self.check_settings_file();
        let changes = if self.settings_in_own_window(ctx) { self.settings_own_window(ctx) } else { self.settings_docked(ctx) };
        for c in changes {
            self.settings_change(ctx, c);
        }
    }

    /// Notice errors in settings.json (checked when its modification time changes).
    fn check_settings_file(&mut self) {
        let Some(dir) = &self.config else { return };
        let path = Settings::path(dir);
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if self.settings_ui.file_checked == Some(mtime) {
            return;
        }
        self.settings_ui.file_checked = Some(mtime);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        self.settings_ui.file_error = if text.trim().is_empty() { None } else { Settings::parse(&text).err() };
    }

    fn settings_look(&self) -> Look {
        let hl = &self.docs[self.active].highlighter;
        Look { bg: hl.background(), accent: hl.accent(), ui_size: self.settings.ui_font_size.clamp(9.0, 24.0), active_theme: self.active_theme(), ids: [self.link.id("settings-top"), self.link.id("settings-categories")] }
    }

    fn settings_docked(&mut self, ctx: &egui::Context) -> Vec<Change> {
        let mut open = true;
        let screen = ctx.content_rect();
        let size = vec2((screen.width() - 80.0).clamp(360.0, 900.0), (screen.height() - 90.0).clamp(260.0, 680.0));
        let look = self.settings_look();
        let values = meta::values(&self.settings);
        let state = &mut self.settings_ui;
        let mut changes = Vec::new();
        let resp = egui::Window::new("Settings")
            .id(self.link.id("settings-window"))
            .open(&mut open)
            .collapsible(false)
            .default_size(size)
            .min_size(vec2(360.0, 240.0))
            .max_size(screen.size() - vec2(16.0, 16.0))
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .constrain(true)
            .resizable(true)
            .frame(egui::Frame::window(&ctx.global_style()).fill(look.bg).inner_margin(0))
            .show(ctx, |ui| changes = contents(state, ui, &values, &look));
        if let Some(r) = resp {
            self.settings_ui.rect = Some(r.response.rect);
        }
        if !open {
            self.close_settings_window();
        }
        changes
    }

    fn settings_own_window(&mut self, ctx: &egui::Context) -> Vec<Change> {
        let size = vec2(860.0, 640.0);
        if self.settings_ui.position.is_none() {
            let main = ctx.input(|i| i.viewport().outer_rect);
            self.settings_ui.position = Some(main.map_or(egui::pos2(120.0, 100.0), |r| (r.center() - size / 2.0).max(r.left_top())));
        }
        let mut builder = egui::ViewportBuilder::default()
            .with_title("Settings")
            .with_inner_size(size)
            .with_min_inner_size(vec2(420.0, 280.0))
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon-app.png")).expect("bundled icon"));
        if let Some(p) = self.settings_ui.position {
            builder = builder.with_position(p);
        }
        let id = egui::ViewportId::from_hash_of(self.link.id("settings-own-window"));
        let look = self.settings_look();
        let values = meta::values(&self.settings);
        let mut changes = Vec::new();
        let mut closed = false;
        ctx.show_viewport_immediate(id, builder, |ui, _| {
            let ctx = ui.ctx().clone();
            if ctx.input(|i| i.viewport().close_requested()) {
                closed = true;
                return;
            }
            if std::mem::take(&mut self.settings_ui.raise) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            let pending = std::mem::take(&mut self.settings_ui.pending);
            if !pending.is_empty() {
                ctx.input_mut(|i| i.events.extend(pending));
            }
            self.settings_ui.active = ctx.input(|i| i.viewport().focused.unwrap_or(false));
            self.settings_keys_in(&ctx);
            if !self.settings_ui.open {
                return;
            }
            // Keys the editor window's menu would take (on macOS the menu bar
            // sends them to the editor window: see `settings_takes`).
            if !cfg!(target_os = "macos") {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::F)) {
                    self.settings_ui.focus_search = true;
                }
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::W)) {
                    closed = true;
                    return;
                }
            }
            let state = &mut self.settings_ui;
            egui::CentralPanel::default().frame(egui::Frame::new().fill(look.bg)).show(ui, |ui| changes = contents(state, ui, &values, &look));
        });
        if closed {
            self.close_settings_window();
        }
        changes
    }

    /// Carry out what the form asked for.
    pub(super) fn settings_change(&mut self, ctx: &egui::Context, change: Change) {
        match change {
            Change::Set { key, value, persist } => self.change_setting(ctx, key, value, persist),
            Change::Reset(key) => {
                let value = meta::defaults().get(key).cloned().unwrap_or(Value::Null);
                self.change_setting(ctx, key, value, true);
            }
            Change::OpenJson(key) => self.open_settings_at(key),
            Change::Copy(key) => {
                ctx.copy_text(key.to_owned());
                self.status = format!("Copied \"{key}\"");
            }
        }
    }

    /// Change one setting as the menus do: it applies at once, everywhere,
    /// and (with `persist`) only its key in settings.json is rewritten.
    pub(super) fn change_setting(&mut self, ctx: &egui::Context, key: &str, value: Value, persist: bool) {
        if self.settings_ui.file_error.is_some() && persist {
            self.status = "settings.json has errors; fix them first so your settings aren't overwritten".into();
            return;
        }
        if key == "terminal_visible" {
            // The panel itself, which keeps the setting.
            if value.as_bool().is_some_and(|on| on != self.terminal.visible) {
                self.toggle_terminal(ctx);
            }
            return;
        }
        let indent = (self.settings.tab_size, self.settings.translate_tabs_to_spaces);
        if let Err(e) = meta::set(&mut self.settings, key, value) {
            self.status = format!("Couldn't change {e}");
            return;
        }
        self.apply_settings();
        if indent != (self.settings.tab_size, self.settings.translate_tabs_to_spaces) {
            self.apply_indentation();
        }
        if persist {
            self.persist_settings();
        }
    }

    /// settings.json in a tab, at `key`'s line if the file has it.
    pub(super) fn open_settings_at(&mut self, key: Option<&str>) {
        self.open_settings();
        let Some(key) = key else { return };
        let needle = format!("\"{key}\"");
        let d = &self.docs[self.active];
        if d.path.as_deref().and_then(|p| p.file_name()) != Some(std::ffi::OsStr::new("settings.json")) {
            return;
        }
        let lines = d.editor.buffer.len_lines();
        if let Some(line) = (0..lines).find(|&l| d.editor.buffer.line_text(l).contains(&needle)) {
            self.goto_line(line);
        }
    }
}

/// Colors and sizes for drawing the form.
struct Look {
    bg: Color32,
    accent: Color32,
    ui_size: f32,
    active_theme: String,
    ids: [egui::Id; 2],
}

fn search_id() -> egui::Id {
    egui::Id::new("settings-search")
}

/// The settings shown: the search's matches, or the current category.
fn shown(state: &SettingsUi) -> Vec<&'static Meta> {
    let q = state.search.trim();
    meta::SETTINGS.iter().filter(|m| if q.is_empty() { m.category == state.category } else { meta::matches(m, q) }).collect()
}

/// The inside of the window. Returns what to change.
fn contents(state: &mut SettingsUi, ui: &mut egui::Ui, values: &Map<String, Value>, look: &Look) -> Vec<Change> {
    let mut changes = Vec::new();
    let fg = ui.visuals().text_color();
    let dim = fg.gamma_multiply(0.62);
    let panel = ui.visuals().panel_fill;
    let rule = ui.visuals().widgets.noninteractive.bg_stroke;
    let size = look.ui_size;
    ui.set_min_size(ui.available_size());

    // ---- search and the file
    egui::Panel::top(look.ids[0]).frame(egui::Frame::new().fill(panel).inner_margin(egui::Margin::symmetric(12, 8)).stroke(rule)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let find = crate::commands::Cmd::Find.shortcut_label().unwrap_or_default();
            let edit = egui::TextEdit::singleline(&mut state.search)
                .id(search_id())
                .hint_text(format!("Search settings   {find}"))
                .font(egui::FontId::proportional(size))
                .background_color(look.bg)
                .desired_width((ui.available_width() - 170.0).clamp(140.0, 440.0))
                .margin(egui::Margin::symmetric(8, 5));
            let r = ui.add(edit);
            if std::mem::take(&mut state.focus_search) {
                r.request_focus();
                if let Some(mut s) = egui::TextEdit::load_state(ui.ctx(), search_id()) {
                    s.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(state.search.chars().count()))));
                    s.store(ui.ctx(), search_id());
                }
            }
            if r.changed() {
                state.scroll_top = true;
            }
            state.search_focused = r.has_focus();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(RichText::new("Open settings.json").size(size)).on_hover_text("Edit the settings as JSON (Preferences: Settings (JSON))").clicked() {
                    changes.push(Change::OpenJson(None));
                }
            });
        });
        if let Some(e) = &state.file_error {
            ui.add_space(6.0);
            egui::Frame::new().fill(Color32::from_rgb(0xc0, 0x39, 0x2b).gamma_multiply(0.18)).corner_radius(4).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(e).size(size).strong());
                ui.label(RichText::new("Settings can't be changed here until the file is fixed, so nothing in it is overwritten. Fix it and save; this window follows.").size(size * 0.92).color(dim));
            });
        }
    });

    // ---- categories
    let query = state.search.trim().to_owned();
    egui::Panel::left(look.ids[1])
        .resizable(false)
        .exact_size((size * 13.0).clamp(150.0, 210.0))
        .frame(egui::Frame::new().fill(panel).inner_margin(egui::Margin { left: 8, right: 8, top: 10, bottom: 8 }).stroke(rule))
        .show(ui, |ui| {
            for c in Category::ALL {
                let count = meta::SETTINGS.iter().filter(|m| m.category == c && (query.is_empty() || meta::matches(m, &query))).count();
                if count == 0 && !query.is_empty() {
                    continue;
                }
                let current = query.is_empty() && c == state.category;
                let label = if query.is_empty() { c.label().to_owned() } else { format!("{}  ({count})", c.label()) };
                let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), size + 14.0), Sense::click());
                if current || r.hovered() || r.has_focus() {
                    let fill = if current { ui.visuals().selection.bg_fill } else { ui.visuals().widgets.hovered.weak_bg_fill };
                    ui.painter().rect_filled(rect, 4.0, fill);
                }
                if current {
                    ui.painter().rect_filled(egui::Rect::from_min_size(rect.left_top() + vec2(0.0, 4.0), vec2(3.0, rect.height() - 8.0)), 1.5, look.accent);
                }
                ui.painter().text(rect.left_center() + vec2(12.0, 0.0), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(size), if current { fg } else { fg.gamma_multiply(0.85) });
                if r.clicked() {
                    state.category = c;
                    state.search.clear();
                    state.scroll_top = true;
                }
            }
        });

    // ---- the settings
    egui::CentralPanel::default().frame(egui::Frame::new().fill(look.bg).inner_margin(egui::Margin { left: 18, right: 14, top: 10, bottom: 10 })).show(ui, |ui| {
        let mut scroll = egui::ScrollArea::vertical().id_salt("settings-rows").auto_shrink(false);
        if std::mem::take(&mut state.scroll_top) {
            scroll = scroll.vertical_scroll_offset(0.0);
        }
        scroll.show(ui, |ui| {
            let rows = shown(state);
            if rows.is_empty() {
                ui.add_space(20.0);
                ui.label(RichText::new(format!("No settings match \"{query}\".")).size(size).color(dim));
            }
            ui.add_enabled_ui(state.file_error.is_none(), |ui| {
                let mut category = None;
                // `syntax_settings` has its own row (`per_language_row`) below.
                for m in rows.into_iter().filter(|m| m.key != SYNTAX_SETTINGS) {
                    if category != Some(m.category) {
                        category = Some(m.category);
                        ui.add_space(if ui.min_rect().height() > 1.0 { 14.0 } else { 2.0 });
                        ui.label(RichText::new(m.category.label()).size(size * 1.25).strong());
                        ui.add_space(4.0);
                    }
                    row(state, ui, m, values, look, &mut changes);
                }
                let per_language = "per language syntax_settings indentation rulers wrap";
                let show_hint = if query.is_empty() { state.category == Category::Editor } else { query.split_whitespace().all(|w| per_language.contains(&w.to_lowercase())) };
                if show_hint {
                    per_language_row(ui, values, look, &mut changes);
                }
            });
        });
    });
    state.field_focused = ui.ctx().memory(|m| m.focused().is_some());
    changes
}

/// One setting: label, description, key, control, and Reset when changed.
fn row(state: &mut SettingsUi, ui: &mut egui::Ui, m: &'static Meta, values: &Map<String, Value>, look: &Look, changes: &mut Vec<Change>) {
    let size = look.ui_size;
    let fg = ui.visuals().text_color();
    let dim = fg.gamma_multiply(0.62);
    let value = values.get(m.key).cloned().unwrap_or(Value::Null);
    let changed = !meta::is_default(values, m.key);
    let width = ui.available_width();
    let control_w = (width * 0.42).clamp(170.0, 320.0);
    let top = ui.cursor().top();
    ui.add_space(6.0);
    let label_w = (width - control_w - 20.0).max(120.0);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(vec2(label_w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_width(label_w);
            ui.set_max_width(label_w);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(m.label).size(size).strong());
                if changed && ui.add(egui::Button::new(RichText::new("Reset").size(size * 0.85).color(look.accent)).frame(false)).on_hover_text(format!("Back to the default, {}", short(&meta::defaults()[m.key]))).clicked() {
                    changes.push(Change::Reset(m.key));
                }
            });
            ui.add(egui::Label::new(RichText::new(meta::description(m.key)).size(size * 0.92).color(dim)).wrap());
            if let Some(note) = inactive_note(m.key, values) {
                ui.label(RichText::new(note).size(size * 0.85).italics().color(dim));
            }
            if let Some(p) = meta::range_problem(m, &value) {
                ui.label(RichText::new(p).size(size * 0.85).color(Color32::from_rgb(0xe0, 0x8a, 0x2e)));
            }
            let key = ui.add(egui::Label::new(RichText::new(m.key).monospace().size(size * 0.8).color(fg.gamma_multiply(0.42))).sense(Sense::click()));
            if key.on_hover_text("The key in settings.json: click to copy").clicked() {
                changes.push(Change::Copy(m.key));
            }
        });
        ui.add_space(8.0);
        ui.allocate_ui_with_layout(vec2(control_w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_max_width(control_w);
            control(state, ui, m, &value, look, changes);
        });
    });
    ui.add_space(8.0);
    let bottom = ui.cursor().top();
    let left = ui.max_rect().left();
    if changed {
        // A bar in the margin marks settings that differ from the default.
        ui.painter().line_segment([egui::pos2(left - 10.0, top + 8.0), egui::pos2(left - 10.0, bottom - 8.0)], egui::Stroke::new(2.0, look.accent));
    }
    ui.painter().hline(left..=ui.max_rect().right(), bottom, ui.visuals().widgets.noninteractive.bg_stroke);
}

/// The control for one setting.
fn control(state: &mut SettingsUi, ui: &mut egui::Ui, m: &'static Meta, value: &Value, look: &Look, changes: &mut Vec<Change>) {
    let size = look.ui_size;
    let key = m.key;
    let id = egui::Id::new(("settings-control", key));
    let set = |value: Value| Change::Set { key, value, persist: true };
    let w = ui.available_width();
    match m.control {
        Control::Toggle => {
            let mut on = value.as_bool().unwrap_or(false);
            if toggle(ui, &mut on, look.accent).changed() {
                changes.push(set(Value::Bool(on)));
            }
        }
        Control::Number { min, max, step, whole } => {
            let mut v = value.as_f64().unwrap_or(min);
            let decimals = if whole { 0 } else if step < 0.1 { 2 } else if step < 1.0 { 1 } else { 0 };
            let r = ui.add(egui::DragValue::new(&mut v).range(min..=max).speed(step).fixed_decimals(decimals).update_while_editing(false));
            // While dragging the value shows at once; settings.json is written when the drag ends.
            if r.changed() || r.drag_stopped() {
                let v = (v / step).round() * step;
                let value = if whole { Value::from(v.round() as u64) } else { serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number) };
                changes.push(Change::Set { key, value, persist: !r.dragged() });
            }
        }
        Control::MaybeNumber { min, max, none } => {
            ui.horizontal(|ui| {
                let v = value.as_f64();
                let mut edge = v.is_none();
                if ui.checkbox(&mut edge, RichText::new(none).size(size)).changed() {
                    changes.push(set(if edge { Value::Null } else { Value::from(80) }));
                }
                if let Some(mut n) = v {
                    let r = ui.add(egui::DragValue::new(&mut n).range(min..=max).speed(1.0).fixed_decimals(0).update_while_editing(false));
                    if r.changed() && !r.dragged() || r.drag_stopped() {
                        changes.push(set(Value::from(n.round() as u64)));
                    }
                }
            });
        }
        Control::Choice(options) => {
            let current = value.as_str().unwrap_or_default();
            let label = options.iter().find(|(v, _)| v.eq_ignore_ascii_case(current)).map_or(current, |(_, l)| *l);
            let short = options.len() <= 3 && options.iter().map(|(_, l)| l.len()).sum::<usize>() <= 24;
            if short {
                ui.horizontal(|ui| {
                    for (v, l) in options {
                        if ui.add(egui::Button::selectable(v.eq_ignore_ascii_case(current), RichText::new(*l).size(size))).clicked() {
                            changes.push(set(Value::from(*v)));
                        }
                    }
                });
            } else {
                egui::ComboBox::from_id_salt(id).selected_text(RichText::new(label).size(size)).width(w.min(260.0)).show_ui(ui, |ui| {
                    for (v, l) in options {
                        if ui.selectable_label(v.eq_ignore_ascii_case(current), RichText::new(*l).size(size)).clicked() {
                            changes.push(set(Value::from(*v)));
                        }
                    }
                });
            }
        }
        Control::Text => {
            let current = value.as_str().unwrap_or_default();
            if let Some(text) = text_field(ui, &mut state.drafts, key, current, w, true) {
                changes.push(set(Value::from(text)));
            }
        }
        Control::Font { editor } => {
            let current = value.as_str().unwrap_or_default().to_owned();
            let shown = if current.is_empty() { "Default (built-in)".to_owned() } else { current.clone() };
            egui::ComboBox::from_id_salt(id).selected_text(RichText::new(shown).size(size)).width(w.min(260.0)).height(360.0).show_ui(ui, |ui| {
                let fonts = state.fonts[usize::from(!editor)].get_or_insert_with(|| crate::fonts::font_choices(editor).into_iter().map(|f| f.name).collect());
                if ui.selectable_label(current.is_empty(), RichText::new("Default (built-in)").size(size)).clicked() {
                    changes.push(set(Value::from("")));
                }
                for f in fonts.iter() {
                    if ui.selectable_label(*f == current, RichText::new(f).size(size)).clicked() {
                        changes.push(set(Value::from(f.as_str())));
                    }
                }
            });
        }
        Control::Scheme => {
            let current = value.as_str().unwrap_or_default();
            let canonical = crate::highlight::theme_canonical_name(current).unwrap_or(current).to_owned();
            egui::ComboBox::from_id_salt(id).selected_text(RichText::new(&canonical).size(size)).width(w.min(260.0)).height(420.0).show_ui(ui, |ui| {
                for (group, names) in crate::highlight::theme_groups() {
                    ui.label(RichText::new(group.label()).size(size * 0.85).weak());
                    for name in names {
                        ui.horizontal(|ui| {
                            swatch(ui, name, vec2(size * 1.6, size));
                            let shown = if name == look.active_theme { format!("{name}  (showing)") } else { name.to_owned() };
                            if ui.selectable_label(name == canonical, RichText::new(shown).size(size)).clicked() {
                                changes.push(set(Value::from(name)));
                            }
                        });
                    }
                }
            });
            swatch(ui, &canonical, vec2(w.min(260.0) + ui.spacing().icon_width + ui.spacing().button_padding.x * 2.0, 6.0));
        }
        Control::Words | Control::Columns => {
            let numbers = m.control == Control::Columns;
            let items: Vec<Value> = value.as_array().cloned().unwrap_or_default();
            let mut remove = None;
            if !items.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for (i, item) in items.iter().enumerate() {
                        let text = item.as_str().map_or_else(|| item.to_string(), str::to_owned);
                        if ui.add(egui::Button::new(RichText::new(format!("{text}  ×")).size(size * 0.92))).on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                    }
                });
            }
            if let Some(i) = remove {
                let mut items = items.clone();
                items.remove(i);
                changes.push(set(Value::Array(items)));
            }
            let hint = if numbers { "Add a column" } else { "Add…" };
            let draft_key: &'static str = key;
            if let Some(text) = add_field(ui, &mut state.drafts, draft_key, hint, w.min(240.0)) {
                let mut items = items;
                if numbers {
                    match text.trim().parse::<u64>() {
                        Ok(n) if n > 0 && !items.iter().any(|v| v.as_u64() == Some(n)) => {
                            items.push(Value::from(n));
                            items.sort_by_key(|v| v.as_u64());
                        }
                        _ => return,
                    }
                } else if !text.trim().is_empty() && !items.iter().any(|v| v.as_str() == Some(text.trim())) {
                    items.push(Value::from(text.trim()));
                } else {
                    return;
                }
                changes.push(set(Value::Array(items)));
            }
        }
        Control::Flags(options) => {
            let items: Vec<String> = value.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
            for (flag, label) in options {
                let mut on = items.iter().any(|i| i.eq_ignore_ascii_case(flag));
                if ui.checkbox(&mut on, RichText::new(*label).size(size)).changed() {
                    let mut items: Vec<String> = items.iter().filter(|i| !i.eq_ignore_ascii_case(flag)).cloned().collect();
                    if on {
                        items.push((*flag).to_owned());
                    }
                    changes.push(set(Value::from(items)));
                }
            }
        }
        Control::Json => {
            let n = value.as_object().map_or(0, Map::len);
            if ui.button(RichText::new("Edit in settings.json").size(size)).clicked() {
                changes.push(Change::OpenJson(Some(key)));
            }
            ui.label(RichText::new(if n == 1 { "1 entry".to_owned() } else { format!("{n} entries") }).size(size * 0.85).weak());
        }
    }
}

/// Settings set per language live in settings.json only.
fn per_language_row(ui: &mut egui::Ui, values: &Map<String, Value>, look: &Look, changes: &mut Vec<Change>) {
    let size = look.ui_size;
    let dim = ui.visuals().text_color().gamma_multiply(0.62);
    ui.add_space(6.0);
    let width = ui.available_width();
    let control_w = (width * 0.42).clamp(170.0, 320.0);
    let label_w = (width - control_w - 20.0).max(120.0);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(vec2(label_w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_width(label_w);
            ui.set_max_width(label_w);
            ui.label(RichText::new("Per-language settings").size(size).strong());
            ui.add(egui::Label::new(RichText::new("Settings for one language (indentation, wrapping, rulers…) are edited in settings.json.").size(size * 0.92).color(dim)).wrap());
        });
        ui.add_space(8.0);
        if ui.button(RichText::new("Edit in settings.json").size(size)).clicked() {
            changes.push(Change::OpenJson(values.contains_key(SYNTAX_SETTINGS).then_some(SYNTAX_SETTINGS)));
        }
    });
    ui.add_space(8.0);
}

/// A note when a scheme setting isn't the one in use.
fn inactive_note(key: &str, values: &Map<String, Value>) -> Option<&'static str> {
    let mode = values.get("theme_mode").and_then(Value::as_str).unwrap_or("fixed").trim().to_ascii_lowercase();
    let auto = matches!(mode.as_str(), "auto" | "system");
    match key {
        "theme" if mode != "fixed" => Some("Not used: the mode isn't Fixed."),
        "dark_theme" if !auto && mode != "dark" => Some("Used when the mode is Follow system or Always dark."),
        "light_theme" if !auto && mode != "light" => Some("Used when the mode is Follow system or Always light."),
        _ => None,
    }
}

/// A default value as the user would write it.
fn short(v: &Value) -> String {
    match v {
        Value::String(s) if s.is_empty() => "empty".into(),
        Value::String(s) if s.len() > 40 => "the built-in selector".into(),
        Value::Number(n) => n.as_f64().map_or_else(|| n.to_string(), meta::fmt_number),
        other => other.to_string(),
    }
}

/// A single-line field that commits when it loses focus (Enter or Tab):
/// returns the new text then. `draft` text is kept while typing.
fn text_field(ui: &mut egui::Ui, drafts: &mut HashMap<&'static str, String>, key: &'static str, current: &str, width: f32, mono: bool) -> Option<String> {
    let id = egui::Id::new(("settings-text", key));
    let mut text = drafts.get(key).cloned().unwrap_or_else(|| current.to_owned());
    let mut edit = egui::TextEdit::singleline(&mut text).id(id).desired_width(width);
    if mono {
        edit = edit.font(egui::TextStyle::Monospace);
    }
    let r = ui.add(edit);
    if r.has_focus() {
        drafts.insert(key, text.clone());
    }
    if r.lost_focus() {
        drafts.remove(key);
        if text != current {
            return Some(text);
        }
    }
    None
}

/// The "Add…" field under a list: returns the text when Enter is pressed.
fn add_field(ui: &mut egui::Ui, drafts: &mut HashMap<&'static str, String>, key: &'static str, hint: &str, width: f32) -> Option<String> {
    let id = egui::Id::new(("settings-add", key));
    let mut text = drafts.get(key).cloned().unwrap_or_default();
    let r = ui.add(egui::TextEdit::singleline(&mut text).id(id).hint_text(hint).desired_width(width));
    let entered = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if r.has_focus() {
        drafts.insert(key, text.clone());
    } else {
        drafts.remove(key);
    }
    if entered {
        drafts.remove(key);
        r.request_focus();
        return (!text.trim().is_empty()).then_some(text);
    }
    None
}

/// An on/off switch. Space or Enter flips it when it has the keyboard.
fn toggle(ui: &mut egui::Ui, on: &mut bool, accent: Color32) -> egui::Response {
    let h = ui.spacing().interact_size.y * 0.9;
    let size = vec2(h * 1.8, h);
    let (rect, mut r) = ui.allocate_exact_size(size, Sense::click());
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, ""));
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(r.id, *on);
        let visuals = ui.style().interact_selectable(&r, *on);
        let rect = rect.expand(visuals.expansion);
        let radius = 0.5 * rect.height();
        let off = ui.visuals().widgets.inactive.bg_fill;
        let fill = if ui.is_enabled() { off.lerp_to_gamma(accent, t) } else { off };
        ui.painter().rect(rect, radius, fill, visuals.bg_stroke, egui::StrokeKind::Inside);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        let knob = if *on { Color32::WHITE } else { ui.visuals().text_color().gamma_multiply(0.8) };
        ui.painter().circle_filled(egui::pos2(x, rect.center().y), 0.72 * radius, knob);
        if r.has_focus() {
            ui.painter().rect_stroke(rect.expand(2.0), radius + 2.0, ui.visuals().selection.stroke, egui::StrokeKind::Outside);
        }
    }
    r
}

/// A scheme's colors for previews: background, text, and three syntax colors.
#[derive(Clone, Copy)]
pub(super) struct Preview {
    pub bg: Color32,
    pub fg: Color32,
    pub syntax: [Color32; 3],
}

/// The colors of the scheme called `name` (None if there is none).
pub(super) fn preview(name: &str) -> Option<Preview> {
    thread_local! {
        static CACHE: std::cell::RefCell<HashMap<String, (usize, Preview)>> = Default::default();
    }
    let scheme = crate::highlight::scheme(name)?;
    let ptr = std::sync::Arc::as_ptr(&scheme) as usize;
    if let Some(p) = CACHE.with(|c| c.borrow().get(name).filter(|(k, _)| *k == ptr).map(|(_, p)| *p)) {
        return Some(p);
    }
    let color = |c: Option<syntect::highlighting::Color>, or: Color32| c.map_or(or, |c| Color32::from_rgb(c.r, c.g, c.b));
    let s = &scheme.theme.settings;
    let bg = color(s.background, Color32::from_gray(30));
    let fg = color(s.foreground, Color32::from_gray(210));
    let scope = |sc: &str| crate::highlight::scope_color(name, sc).unwrap_or(fg);
    let p = Preview { bg, fg, syntax: [scope("keyword"), scope("string"), scope("comment")] };
    CACHE.with(|c| c.borrow_mut().insert(name.to_owned(), (ptr, p)));
    Some(p)
}

/// A strip in a scheme's colors.
fn swatch(ui: &mut egui::Ui, name: &str, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let Some(p) = preview(name) else { return };
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, p.bg);
    let w = rect.width() / 5.0;
    for (i, c) in [p.syntax[0], p.fg, p.syntax[1], p.syntax[2]].into_iter().enumerate() {
        let x = rect.left() + w * (0.5 + i as f32);
        let bar = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - rect.height() * 0.15), vec2(w * 0.8, (rect.height() * 0.3).max(1.0)));
        painter.rect_filled(bar, 1.0, c);
    }
    painter.rect_stroke(rect, 3.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Cmd;
    use egui::{Context, Event, Modifiers, Pos2, RawInput, Rect};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-settings-ui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn frame(app: &mut StrataApp, ctx: &Context, events: Vec<Event>) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| eframe::App::ui(app, ui, &mut eframe::Frame::_new_kittest()));
        out.textures_delta.clear();
    }

    fn set(key: &'static str, value: Value) -> Change {
        Change::Set { key, value, persist: true }
    }

    #[test]
    fn a_change_writes_only_its_key_and_keeps_comments() {
        let dir = temp_dir("write");
        let original = "// My settings\n{\n    // big text\n    \"font_size\": 16, // was 12\n    \"theme\": \"Nord\",\n}\n";
        std::fs::write(dir.join("settings.json"), original).unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(None, Some(dir.clone()));
        app.run(&ctx, Cmd::SettingsWindow);
        frame(&mut app, &ctx, vec![]);
        assert!(app.settings_ui.open && app.settings_ui.file_error.is_none());
        app.settings_change(&ctx, set("minimap", Value::Bool(false)));
        assert!(!app.settings.minimap);
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert_eq!(text, "// My settings\n{\n    // big text\n    \"font_size\": 16, // was 12\n    \"theme\": \"Nord\",\n    \"minimap\": false,\n}\n");
        // A number in place, a fraction written the way it was typed.
        app.settings_change(&ctx, set("line_height", Value::from(1.2)));
        app.settings_change(&ctx, set("font_size", Value::from(18.0)));
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(text.contains("\"font_size\": 18.0, // was 12") && text.contains("\"line_height\": 1.2,\n") && text.contains("// big text"), "{text}");
        assert_eq!(app.settings.line_height, 1.2);
        // Reset puts the default back (written, since the file had a value).
        app.settings_change(&ctx, Change::Reset("font_size"));
        assert_eq!(app.settings.font_size, 14.0);
        assert!(std::fs::read_to_string(dir.join("settings.json")).unwrap().contains("\"font_size\": 14.0, // was 12"));
        // A slider being dragged changes the setting but not the file.
        app.settings_change(&ctx, Change::Set { key: "font_size", value: Value::from(20.0), persist: false });
        assert_eq!(app.settings.font_size, 20.0);
        assert!(std::fs::read_to_string(dir.join("settings.json")).unwrap().contains("\"font_size\": 14.0"));
        // The default indentation reaches open files, as when settings.json changes.
        app.settings_change(&ctx, set("tab_size", Value::from(2)));
        assert_eq!(app.docs[0].editor.tab_width, 2);
        // A bad value changes nothing.
        app.settings_change(&ctx, set("tab_size", Value::from("wide")));
        assert_eq!(app.settings.tab_size, 2);
        assert!(app.status.contains("tab_size"), "{}", app.status);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_broken_settings_file_makes_the_form_read_only() {
        let dir = temp_dir("broken");
        std::fs::write(dir.join("settings.json"), "{ \"font_size\": 16,, }").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(None, Some(dir.clone()));
        app.run(&ctx, Cmd::SettingsWindow);
        frame(&mut app, &ctx, vec![]);
        assert!(app.settings_ui.file_error.as_deref().is_some_and(|e| e.contains("settings.json")));
        app.settings_change(&ctx, set("minimap", Value::Bool(false)));
        assert!(app.settings.minimap, "nothing changes");
        assert_eq!(std::fs::read_to_string(dir.join("settings.json")).unwrap(), "{ \"font_size\": 16,, }");
        // Fixed on disk: the form follows.
        std::fs::write(dir.join("settings.json"), "{ \"font_size\": 16 }").unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(2);
        std::fs::File::options().write(true).open(dir.join("settings.json")).unwrap().set_modified(later).unwrap();
        frame(&mut app, &ctx, vec![]);
        assert!(app.settings_ui.file_error.is_none());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn search_filters_every_category() {
        let mut s = SettingsUi::default();
        assert!(shown(&s).iter().all(|m| m.category == Category::Appearance));
        s.search = "save".into();
        let keys: Vec<_> = shown(&s).iter().map(|m| m.key).collect();
        assert!(keys.contains(&"trim_trailing_white_space_on_save") && keys.contains(&"save_all_on_build"), "{keys:?}");
        assert!(!keys.contains(&"font_size"));
        s.search = "terminal clipboard".into();
        let keys: Vec<_> = shown(&s).iter().map(|m| m.key).collect();
        assert_eq!(keys, ["terminal_allow_clipboard_write", "terminal_allow_clipboard_read"]);
        s.search = "ruler".into();
        let keys: Vec<_> = shown(&s).iter().map(|m| m.key).collect();
        assert!(keys.contains(&"rulers") && keys.contains(&"wrap_width"), "{keys:?}");
    }

    #[test]
    fn opens_from_its_shortcut_and_closes_with_esc() {
        let ctx = Context::default();
        let mut app = StrataApp::new(None, None);
        let cmd = if crate::keys::MAC { Modifiers::MAC_CMD | Modifiers::COMMAND } else { Modifiers::CTRL | Modifiers::COMMAND };
        let press = |key: Key, modifiers: Modifiers| Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers };
        frame(&mut app, &ctx, vec![press(Key::Comma, cmd)]);
        assert!(app.settings_ui.open, "⌘, / Ctrl+, opens the Settings window");
        assert_eq!(app.docs.len(), 1, "not settings.json");
        // Every category draws, docked (no native windows here).
        for c in Category::ALL {
            app.settings_ui.category = c;
            frame(&mut app, &ctx, vec![]);
        }
        app.settings_ui.search = "font".into();
        frame(&mut app, &ctx, vec![]);
        // Find goes to its search box; Esc clears the search, then closes.
        app.run(&ctx, Cmd::Find);
        assert!(!app.find.open, "Find is the window's search while it has the keyboard");
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
        assert!(app.settings_ui.open && app.settings_ui.search.is_empty());
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
        }
        assert!(!app.settings_ui.open);
        // The JSON is still a command away.
        assert!(Cmd::Settings.name().contains("JSON") && Cmd::Settings.default_keys().is_empty());
        assert_eq!(Cmd::SettingsWindow.id(), "settings");
    }

    #[test]
    fn open_json_at_a_key() {
        let dir = temp_dir("json-at");
        std::fs::write(dir.join("settings.json"), "{\n  \"font_size\": 16,\n  \"theme_overrides\": {}\n}\n").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(None, Some(dir.clone()));
        app.settings_change(&ctx, Change::OpenJson(Some("theme_overrides")));
        let d = &app.docs[app.active];
        assert_eq!(d.path.as_deref(), Some(dir.join("settings.json").as_path()));
        assert_eq!(d.editor.cursor_line_col().0, 3, "the line with the key");
        std::fs::remove_dir_all(dir).ok();
    }
}
