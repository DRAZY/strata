//! The editor window: tabs, sidebar, find bar, status bar, and the session.

use crate::buffer::{Buffer, LineEnding};
use crate::commands::{self, Cmd};
use crate::devshot::DevShot;
use crate::document::Document;
use crate::find::{self, Query};
use crate::palette::{self, Palette};
use crate::session::{Session, Settings, TabState};
use crate::sidebar::{self, Sidebar};
use egui::{Color32, FontId, Key, Rect, Sense, Stroke, pos2, vec2};
use std::path::PathBuf;

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
    docs: Vec<Document>,
    active: usize,
    /// Recently closed files, for Reopen Closed Tab.
    closed: Vec<PathBuf>,
    status: String,
    devshot: DevShot,
    find: FindBar,
    sidebar: Sidebar,
    settings: Settings,
    /// Settings and session folder; None in tests (nothing is persisted).
    config: Option<PathBuf>,
    last_disk_check: f64,
    last_session_save: f64,
    /// What the session looked like when last written, to skip no-op saves.
    session_key: u64,
    /// Theme the window chrome was last styled for.
    chrome_theme: String,
    fullscreen: bool,
    /// Tab being dragged to a new position.
    dragging_tab: Option<usize>,
    palette: Palette,
    /// Selections to put back if a palette preview (line, symbol) is cancelled.
    palette_restore: Option<(usize, Vec<std::ops::Range<usize>>, usize)>,
    /// Dev aid: STRATA_DEV_PALETTE="p:query" (or "cmd:query") opens a panel on the first frame.
    dev_palette: Option<String>,
}

/// Editor commands offered in the command palette: (id, name, shortcut label).
const EDITOR_COMMANDS: &[(&str, &str, &str)] = &[];

impl StrataApp {
    pub fn new(path: Option<PathBuf>, config: Option<PathBuf>) -> Self {
        let (settings, settings_error) = config.as_deref().map(Settings::load).unwrap_or_default();
        let mut app = Self {
            docs: Vec::new(),
            active: 0,
            closed: Vec::new(),
            status: settings_error.unwrap_or_default(),
            devshot: DevShot::default(),
            find: FindBar::default(),
            sidebar: Sidebar::new(),
            settings,
            config,
            last_disk_check: 0.0,
            last_session_save: 0.0,
            session_key: 0,
            chrome_theme: String::new(),
            fullscreen: false,
            dragging_tab: None,
            palette: Palette::new(EDITOR_COMMANDS),
            palette_restore: None,
            dev_palette: std::env::var("STRATA_DEV_PALETTE").ok(),
        };
        if app.settings.hot_exit {
            app.restore_session();
        }
        match path {
            Some(p) if p.is_dir() => app.sidebar.open_folder(p),
            Some(p) => app.open_path(p, false),
            None => {}
        }
        if app.docs.is_empty() {
            app.push_doc(Document::untitled());
        }
        app.session_key = app.session_fingerprint();

        // Dev aids (pair with STRATA_SCREENSHOT): run find-in-files on the open folder,
        // or open find/replace with a query.
        if let (Ok(q), Some(root)) = (std::env::var("STRATA_DEV_FIND_IN_FILES"), app.sidebar.root.clone()) {
            app.sidebar.start_find_in_files(Some(q));
            app.sidebar.search.start(&root, &app.sidebar.query, None);
            while app.sidebar.search.running {
                app.sidebar.search.poll();
            }
            if let Some(f) = app.sidebar.search.results.first() {
                let (p, h) = (f.path.clone(), f.hits[0].clone());
                app.open_at(p, h.line, h.cols);
            }
        }
        if let Ok(q) = std::env::var("STRATA_DEV_FIND") {
            app.open_find(true);
            app.find.query.text = q;
            app.find.query.regex = std::env::var_os("STRATA_DEV_FIND_REGEX").is_some();
        }
        app
    }

    fn doc(&mut self) -> &mut Document {
        &mut self.docs[self.active]
    }

    // ---- tabs ----------------------------------------------------------

    /// Style a new tab with the current settings and add it after the active one.
    fn push_doc(&mut self, mut doc: Document) {
        doc.highlighter.set_theme(&self.settings.theme);
        doc.editor.tab_width = self.settings.tab_size;
        let at = if self.docs.is_empty() { 0 } else { self.active + 1 };
        self.docs.insert(at, doc);
        self.activate(at);
    }

    fn activate(&mut self, i: usize) {
        self.active = i.min(self.docs.len().saturating_sub(1));
        if let Some(d) = self.docs.get_mut(self.active) {
            d.view.focus();
            if d.check_disk() {
                self.status = format!("Reloaded {} (changed on disk)", d.name());
            }
        }
    }

    /// Open a file in a tab, reusing its tab if it's already open.
    /// `preview` tabs (single click in the sidebar) replace each other until edited.
    fn open_path(&mut self, path: PathBuf, preview: bool) {
        if let Some(i) = self.docs.iter().position(|d| d.path.as_ref() == Some(&path)) {
            if !preview {
                self.docs[i].preview = false;
            }
            self.activate(i);
            return;
        }
        let mut doc = match Document::open(path.clone()) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("Could not open {}: {e}", path.display());
                return;
            }
        };
        doc.preview = preview;
        self.status = format!("Opened {}", path.display());
        // Replace an earlier preview, or a blank untitled tab.
        let reuse = if preview { self.docs.iter().position(|d| d.preview) } else { None }
            .or_else(|| self.docs.get(self.active).is_some_and(Document::is_blank).then_some(self.active));
        if let Some(i) = reuse {
            doc.highlighter.set_theme(&self.settings.theme);
            doc.editor.tab_width = self.settings.tab_size;
            self.docs[i] = doc;
            self.activate(i);
        } else {
            self.push_doc(doc);
        }
    }

    /// Open (if needed) and select a find-in-files hit.
    fn open_at(&mut self, path: PathBuf, line: usize, cols: std::ops::Range<usize>) {
        self.open_path(path.clone(), false);
        let d = self.doc();
        if d.path.as_ref() != Some(&path) {
            return;
        }
        let start = d.editor.buffer.line_start(line);
        let end = d.editor.buffer.line_end(line);
        let range = (start + cols.start).min(end)..(start + cols.end).min(end);
        d.editor.select_ranges(&[range], 0);
        d.view.reveal_centered();
        d.view.focus();
    }

    /// Ask before closing a tab with unsaved changes. True if it may close.
    fn confirm_close(&mut self, i: usize) -> bool {
        if !self.docs[i].is_dirty() {
            return true;
        }
        let answer = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Unsaved changes")
            .set_description(format!("Save changes to {} before closing?", self.docs[i].name()))
            .set_buttons(rfd::MessageButtons::YesNoCancel)
            .show();
        match answer {
            rfd::MessageDialogResult::Yes => self.save(i, false),
            rfd::MessageDialogResult::No => true,
            _ => false,
        }
    }

    fn close_tab(&mut self, i: usize) -> bool {
        if i >= self.docs.len() || !self.confirm_close(i) {
            return false;
        }
        let doc = self.docs.remove(i);
        if let Some(p) = doc.path {
            self.closed.retain(|c| c != &p);
            self.closed.push(p);
        }
        if self.docs.is_empty() {
            self.docs.push(Document::untitled());
        }
        if i < self.active || self.active >= self.docs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.activate(self.active);
        true
    }

    fn reopen_closed(&mut self) {
        while let Some(p) = self.closed.pop() {
            if p.exists() {
                return self.open_path(p, false);
            }
        }
    }

    // ---- saving --------------------------------------------------------

    /// True if the tab ends up saved.
    fn save(&mut self, i: usize, choose: bool) -> bool {
        let target = match (&self.docs[i].path, choose) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new().set_file_name(self.docs[i].name()).save_file(),
        };
        let Some(path) = target else { return false };
        match self.docs[i].write_to(path.clone()) {
            Ok(()) => {
                self.status = format!("Saved {}", path.display());
                self.sidebar.refresh();
                if self.config.as_deref().map(Settings::path).as_ref() == Some(&path) {
                    self.reload_settings();
                }
                true
            }
            Err(e) => {
                self.status = format!("Save failed: {e}");
                false
            }
        }
    }

    fn save_all(&mut self) {
        for i in 0..self.docs.len() {
            if self.docs[i].is_dirty() && self.docs[i].path.is_some() {
                self.save(i, false);
            }
        }
    }

    // ---- settings --------------------------------------------------------

    fn open_settings(&mut self) {
        let Some(dir) = self.config.clone() else { return };
        let path = Settings::path(&dir);
        if !path.exists() {
            if let Err(e) = self.settings.save(&dir) {
                self.status = format!("Could not write settings: {e}");
                return;
            }
        }
        self.open_path(path, false);
    }

    fn reload_settings(&mut self) {
        let Some(dir) = self.config.clone() else { return };
        let (settings, error) = Settings::load(&dir);
        self.settings = settings;
        self.status = error.unwrap_or_else(|| "Settings reloaded".into());
        self.apply_settings();
    }

    fn apply_settings(&mut self) {
        for d in &mut self.docs {
            d.highlighter.set_theme(&self.settings.theme);
            d.editor.tab_width = self.settings.tab_size;
        }
    }

    fn persist_settings(&mut self) {
        if let Some(dir) = &self.config {
            if let Err(e) = self.settings.save(dir) {
                self.status = format!("Could not write settings: {e}");
            }
        }
    }

    fn zoom(&mut self, delta: f32) {
        self.settings.font_size = if delta == 0.0 { 14.0 } else { (self.settings.font_size + delta).clamp(8.0, 48.0) };
        self.persist_settings();
    }

    // ---- session (hot exit) ----------------------------------------------

    fn restore_session(&mut self) {
        let Some(dir) = self.config.clone() else { return };
        let Some(session) = Session::load(&dir) else { return };
        let backups = Session::backups(&dir);
        for tab in &session.tabs {
            let doc = match (&tab.backup, &tab.path) {
                (Some(name), path) => match std::fs::read(backups.join(name)) {
                    Ok(bytes) => {
                        let mut d = Document::from_buffer(path.clone(), Buffer::from_bytes(&bytes));
                        d.restored_dirty = true;
                        // Whatever is on disk now is what a save would overwrite.
                        d.disk_mtime = path.as_deref().and_then(crate::document::mtime);
                        Some(d)
                    }
                    Err(_) => None,
                },
                (None, Some(path)) if path.exists() => Document::open(path.clone()).ok(),
                _ => None,
            };
            let Some(mut doc) = doc else { continue };
            if let Some(s) = &tab.syntax {
                doc.highlighter.set_syntax(s);
            }
            let at = tab.cursor.min(doc.editor.buffer.len_chars());
            doc.editor.select_ranges(&[at..at], 0);
            doc.view.reveal_centered();
            self.push_doc(doc);
        }
        self.active = session.active.min(self.docs.len().saturating_sub(1));
        if let Some(folder) = session.folder.filter(|f| f.is_dir()) {
            self.sidebar.open_folder(folder);
            self.sidebar.visible = session.sidebar_visible;
        }
    }

    /// Changes whenever saving the session would write something different.
    fn session_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.active, &self.sidebar.root, self.sidebar.visible).hash(&mut h);
        for d in &self.docs {
            (&d.path, d.is_dirty(), d.editor.version(), d.editor.primary().head).hash(&mut h);
        }
        h.finish()
    }

    fn save_session(&mut self) -> bool {
        let Some(dir) = self.config.clone() else { return false };
        let mut backups = Vec::new();
        let mut tabs = Vec::new();
        for (n, d) in self.docs.iter().enumerate() {
            if d.is_blank() {
                continue;
            }
            let backup = d.is_dirty().then(|| {
                let name = format!("{n}.txt");
                backups.push((name.clone(), d.editor.buffer.slice(0..d.editor.buffer.len_chars())));
                name
            });
            tabs.push(TabState {
                path: d.path.clone(),
                backup,
                cursor: d.editor.primary().head,
                syntax: Some(d.highlighter.syntax_name().to_owned()),
            });
        }
        let session = Session {
            folder: self.sidebar.root.clone(),
            sidebar_visible: self.sidebar.visible,
            active: self.active.min(tabs.len().saturating_sub(1)),
            tabs,
        };
        match session.save(&dir, &backups) {
            Ok(()) => {
                self.session_key = self.session_fingerprint();
                true
            }
            Err(e) => {
                self.status = format!("Could not save session: {e}");
                false
            }
        }
    }

    // ---- commands --------------------------------------------------------

    fn run(&mut self, ctx: &egui::Context, cmd: Cmd) {
        match cmd {
            Cmd::NewTab => self.push_doc(Document::untitled()),
            Cmd::Open => {
                let dir = self.docs[self.active].path.as_ref().and_then(|p| p.parent()).map(|p| p.to_owned());
                let mut dialog = rfd::FileDialog::new();
                if let Some(dir) = dir.or_else(|| self.sidebar.root.clone()) {
                    dialog = dialog.set_directory(dir);
                }
                for p in dialog.pick_files().unwrap_or_default() {
                    self.open_path(p, false);
                }
            }
            Cmd::OpenFolder => {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.sidebar.open_folder(dir);
                }
            }
            Cmd::Save => {
                self.save(self.active, false);
            }
            Cmd::SaveAs => {
                self.save(self.active, true);
            }
            Cmd::SaveAll => self.save_all(),
            Cmd::CloseTab => {
                self.close_tab(self.active);
            }
            Cmd::CloseAll => {
                while self.docs.len() > 1 || !self.docs[0].is_blank() {
                    if !self.close_tab(self.docs.len() - 1) {
                        break;
                    }
                }
            }
            Cmd::ReopenClosed => self.reopen_closed(),
            Cmd::NextTab => self.activate((self.active + 1) % self.docs.len()),
            Cmd::PrevTab => self.activate((self.active + self.docs.len() - 1) % self.docs.len()),
            Cmd::ToggleSidebar => self.sidebar.visible = !self.sidebar.visible,
            Cmd::Find => self.open_find(false),
            Cmd::Replace => self.open_find(true),
            Cmd::FindNext => self.find_step(true),
            Cmd::FindPrev => self.find_step(false),
            Cmd::FindInFiles => {
                let d = &self.docs[self.active];
                let selected = d.editor.buffer.slice(d.editor.primary().range());
                let prefill = (!selected.is_empty() && !selected.contains('\n')).then_some(selected);
                self.sidebar.start_find_in_files(prefill);
            }
            Cmd::ZoomIn => self.zoom(1.0),
            Cmd::ZoomOut => self.zoom(-1.0),
            Cmd::ZoomReset => self.zoom(0.0),
            Cmd::ToggleMinimap => {
                self.settings.minimap = !self.settings.minimap;
                self.persist_settings();
            }
            Cmd::Fullscreen => {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            }
            Cmd::Settings => self.open_settings(),
            Cmd::CommandPalette => self.open_palette(ctx, palette::Mode::Commands, ""),
            Cmd::GotoAnything => self.open_palette(ctx, palette::Mode::Files, ""),
            Cmd::GotoLine => self.open_palette(ctx, palette::Mode::Files, ":"),
            Cmd::GotoSymbol => self.open_palette(ctx, palette::Mode::Files, "@"),
            Cmd::SelectTheme => self.open_palette(ctx, palette::Mode::Theme, ""),
            Cmd::SelectSyntax => self.open_palette(ctx, palette::Mode::Syntax, ""),
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        for cmd in commands::pressed(ctx) {
            self.run(ctx, cmd);
        }
        // Cmd+1…9 picks a tab (Alt on Windows/Linux, like Sublime); 9 is always the last.
        let mods = if crate::keys::MAC { crate::keys::CMD } else { crate::keys::ALT };
        let digits = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        for (n, key) in digits.into_iter().enumerate() {
            if crate::keys::take(ctx, mods, key) {
                self.activate(if n == 8 { self.docs.len() - 1 } else { n });
            }
        }
        if self.find.open && !self.palette.open && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.find.open = false;
            self.doc().view.focus();
        }
    }

    fn open_palette(&mut self, ctx: &egui::Context, mode: palette::Mode, prefix: &str) {
        let d = &mut self.docs[self.active];
        let sels = d.editor.selections().iter().map(|s| s.range()).collect();
        let primary = d.editor.selections().iter().position(|s| *s == d.editor.primary()).unwrap_or(0);
        self.palette_restore = Some((self.active, sels, primary));
        let pctx = palette::Context {
            text: d.editor.text_snapshot(),
            syntax: d.highlighter.syntax_name().to_owned(),
            lines: d.editor.buffer.len_lines(),
            root: self.sidebar.root.clone(),
            theme: self.settings.theme.clone(),
            open: self.docs.iter().filter_map(|d| d.path.clone()).collect(),
        };
        self.palette.show_mode(mode, prefix, pctx, ctx);
    }

    fn goto_line(&mut self, line: usize) {
        let d = self.doc();
        let line = line.min(d.editor.buffer.len_lines() - 1);
        let at = d.editor.buffer.line_start(line);
        d.editor.select_ranges(&[at..at], 0);
        d.view.reveal_centered();
    }

    fn palette_events(&mut self, ctx: &egui::Context, events: Vec<palette::Event>) {
        use palette::Event;
        for event in events {
            match event {
                Event::Run(c) => self.run(ctx, c),
                Event::Editor(id) => self.run_editor(id),
                Event::Open(p) => self.open_path(p, false),
                Event::GotoLine { line, preview } => {
                    self.goto_line(line);
                    if preview {
                        continue;
                    }
                }
                Event::Theme { name, preview } => {
                    for d in &mut self.docs {
                        d.highlighter.set_theme(&name);
                    }
                    if preview {
                        continue;
                    }
                    self.settings.theme = name;
                    self.persist_settings();
                }
                Event::Syntax(s) => self.doc().highlighter.set_syntax(&s),
                Event::Cancel => {
                    self.apply_settings();
                    if let Some((i, sels, primary)) = self.palette_restore.take() {
                        if let Some(d) = self.docs.get_mut(i) {
                            d.editor.select_ranges(&sels, primary);
                            d.view.reveal_centered();
                        }
                    }
                }
            }
            if !self.palette.open {
                self.palette_restore = None;
                self.doc().view.focus();
            }
        }
    }

    fn run_editor(&mut self, _id: &str) {}

    fn title(&self) -> String {
        let d = &self.docs[self.active];
        let dot = if d.is_dirty() { " •" } else { "" };
        let folder = self.sidebar.root.as_ref().and_then(|r| r.file_name()).map(|n| format!(" — {}", n.to_string_lossy())).unwrap_or_default();
        format!("{}{dot}{folder} — Strata", d.name())
    }
}

// ---- find bar --------------------------------------------------------------

impl StrataApp {
    fn open_find(&mut self, replace: bool) {
        let d = &self.docs[self.active];
        let p = d.editor.primary();
        let selected = d.editor.buffer.slice(p.range());
        if !selected.is_empty() && !selected.contains('\n') {
            self.find.query.text = if self.find.query.regex { regex::escape(&selected) } else { selected };
        }
        self.find.open = true;
        self.find.replace |= replace;
        self.find.focus_query = true;
        self.find.origin = p.start();
    }

    fn refresh_matches(&mut self) {
        let d = &mut self.docs[self.active];
        d.finder.update(&mut d.editor, &self.find.query);
    }

    fn select_match(&mut self, i: usize) {
        let d = self.doc();
        let r = d.finder.matches[i].clone();
        d.editor.select_ranges(&[r], 0);
        d.view.reveal_centered();
    }

    /// Cmd+G / Shift+Cmd+G (F3 / Shift+F3): next or previous match.
    fn find_step(&mut self, forward: bool) {
        if self.find.query.text.is_empty() {
            return self.open_find(false);
        }
        self.refresh_matches();
        let d = &self.docs[self.active];
        let p = d.editor.primary();
        let on_match = d.finder.index_of(&p.range()).is_some();
        let i = if forward {
            d.finder.next_from(if on_match { p.end() } else { p.start() })
        } else {
            d.finder.prev_before(p.start())
        };
        if let Some(i) = i {
            self.select_match(i);
        }
    }

    /// Every match becomes a cursor; the bar closes so you can type.
    fn find_all(&mut self) {
        self.refresh_matches();
        let d = &mut self.docs[self.active];
        if d.finder.matches.is_empty() {
            return;
        }
        let head = d.editor.primary().head;
        let primary = d.finder.next_from(head).unwrap_or(0);
        d.editor.select_ranges(&d.finder.matches, primary);
        d.view.focus();
        d.view.reveal();
        self.find.open = false;
    }

    fn replace_one(&mut self) {
        self.refresh_matches();
        let (q, rep) = (self.find.query.clone(), self.find.replacement.clone());
        let d = self.doc();
        let p = d.editor.primary();
        if d.finder.index_of(&p.range()).is_some() {
            let Ok(re) = q.build() else { return };
            let text = d.editor.text_snapshot();
            let byte = d.editor.buffer.char_to_byte(p.start());
            if let Some(rep) = find::expand(&q, &re, &text, byte, &rep) {
                d.editor.replace_ranges(vec![(p.range(), rep)]);
                self.refresh_matches();
            }
        }
        self.find_step(true);
    }

    fn replace_all(&mut self) {
        let Ok(re) = self.find.query.build() else { return };
        let (regex, replacement) = (self.find.query.regex, self.find.replacement.clone());
        let d = self.doc();
        let text = d.editor.text_snapshot();
        let mut edits = Vec::new();
        let (mut byte, mut chr) = (0, 0);
        for caps in re.captures_iter(&text) {
            let m = caps.get(0).expect("group 0");
            chr += text[byte..m.start()].chars().count();
            let start = chr;
            chr += m.as_str().chars().count();
            byte = m.end();
            let rep = if regex {
                let mut out = String::new();
                caps.expand(&replacement, &mut out);
                out
            } else {
                replacement.clone()
            };
            edits.push((start..chr, rep));
        }
        let n = edits.len();
        d.editor.replace_ranges(edits);
        d.view.reveal();
        self.status = format!("Replaced {n} occurrence{}", if n == 1 { "" } else { "s" });
    }

    fn find_bar(&mut self, ui: &mut egui::Ui) {
        self.refresh_matches();
        // Typing in the query jumps to the first match after where search began.
        if self.find.query != self.find.last_query {
            self.find.last_query = self.find.query.clone();
            if let Some(i) = self.docs[self.active].finder.next_from(self.find.origin) {
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
        let d = &self.docs[self.active];
        if let Some(e) = &d.finder.error {
            return egui::RichText::new(e).color(egui::Color32::LIGHT_RED);
        }
        if self.find.query.text.is_empty() {
            return egui::RichText::new("");
        }
        let n = d.finder.matches.len();
        let plus = if d.finder.truncated { "+" } else { "" };
        let text = match (n, d.finder.index_of(&d.editor.primary().range())) {
            (0, _) => "No results".to_owned(),
            (_, Some(i)) => format!("{} of {n}{plus}", i + 1),
            _ => format!("{n}{plus} matches"),
        };
        egui::RichText::new(text)
    }
}

// ---- chrome: tab bar, status bar, theme ----------------------------------------

/// Scale a color's brightness (1.0 = unchanged).
fn shade(c: Color32, f: f32) -> Color32 {
    let s = |v: u8| (v as f32 * f).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(s(c.r()), s(c.g()), s(c.b()))
}

/// Blend toward `to` by `t` (0 = `from`).
fn mix(from: Color32, to: Color32, t: f32) -> Color32 {
    let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(m(from.r(), to.r()), m(from.g(), to.g()), m(from.b(), to.b()))
}

enum TabAction {
    Activate(usize),
    Close(usize),
    Move(usize, usize),
    New,
}

impl StrataApp {
    /// Panels, widgets and menus take their colors from the editor theme.
    fn style_chrome(&mut self, ctx: &egui::Context) {
        let hl = &self.docs[self.active].highlighter;
        if self.chrome_theme == hl.theme_name() {
            return;
        }
        self.chrome_theme = hl.theme_name().to_owned();
        let (bg, fg, sel, light) = (hl.background(), hl.foreground(), hl.selection(), hl.is_light());
        let mut v = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
        let panel = if light { shade(bg, 0.94) } else { shade(bg, 0.82) };
        v.panel_fill = panel;
        v.window_fill = mix(panel, bg, 0.5);
        v.extreme_bg_color = if light { shade(bg, 1.02) } else { shade(bg, 0.7) };
        v.faint_bg_color = mix(panel, fg, 0.04);
        v.selection.bg_fill = sel;
        v.override_text_color = Some(mix(fg, panel, 0.12));
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, mix(panel, fg, 0.10));
        v.widgets.inactive.weak_bg_fill = mix(panel, fg, 0.06);
        v.widgets.hovered.weak_bg_fill = mix(panel, fg, 0.12);
        v.widgets.active.weak_bg_fill = mix(panel, fg, 0.18);
        ctx.set_visuals(v);
    }

    /// Hints over an empty window.
    fn welcome(&self, ctx: &egui::Context) {
        let rect = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("welcome")));
        let hl = &self.docs[0].highlighter;
        let (fg, accent) = (hl.foreground(), hl.caret());
        let center = rect.center() + vec2(if self.sidebar.visible && self.sidebar.root.is_some() { 130.0 } else { 0.0 }, -60.0);
        painter.text(center, egui::Align2::CENTER_CENTER, "Strata", FontId::proportional(40.0), accent.gamma_multiply(0.85));
        let rows = [
            ("Goto Anything", Cmd::GotoAnything),
            ("Command Palette", Cmd::CommandPalette),
            ("Open Folder", Cmd::OpenFolder),
            ("Open File", Cmd::Open),
            ("New File", Cmd::NewTab),
        ];
        for (i, (name, cmd)) in rows.into_iter().enumerate() {
            let y = center.y + 54.0 + i as f32 * 26.0;
            painter.text(pos2(center.x - 8.0, y), egui::Align2::RIGHT_CENTER, name, FontId::proportional(14.0), fg.gamma_multiply(0.55));
            painter.text(pos2(center.x + 8.0, y), egui::Align2::LEFT_CENTER, cmd.shortcut_label().unwrap_or_default(), FontId::proportional(14.0), fg.gamma_multiply(0.85));
        }
    }

    fn display_names(&self) -> Vec<String> {
        let names: Vec<String> = self.docs.iter().map(|d| d.name()).collect();
        // Same file name in two folders: add the folder to tell them apart.
        names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let dup = names.iter().filter(|m| *m == n).count() > 1;
                match self.docs[i].path.as_ref().and_then(|p| p.parent()).and_then(|p| p.file_name()) {
                    Some(dir) if dup => format!("{n} — {}", dir.to_string_lossy()),
                    _ => n.clone(),
                }
            })
            .collect()
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) -> Option<TabAction> {
        let bg = self.docs[self.active].highlighter.background();
        let fg = ui.visuals().text_color();
        let bar = ui.max_rect();
        ui.painter().rect_filled(bar, 0.0, ui.visuals().panel_fill);
        let names = self.display_names();
        let font = FontId::proportional(13.0);
        let height = 32.0;
        let mut action = None;
        let mut rects = Vec::with_capacity(self.docs.len());
        egui::ScrollArea::horizontal().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            ui.horizontal(|ui| {
                for (i, name) in names.iter().enumerate() {
                    let d = &self.docs[i];
                    let galley = ui.painter().layout_no_wrap(name.clone(), font.clone(), fg);
                    let w = (galley.size().x + 52.0).clamp(90.0, 260.0);
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click_and_drag());
                    rects.push(rect);
                    let active = i == self.active;
                    let close = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(16.0, 16.0));
                    let over_close = resp.hover_pos().is_some_and(|p| close.expand(2.0).contains(p));

                    let fill = if active { bg } else if resp.hovered() { mix(ui.visuals().panel_fill, bg, 0.5) } else { ui.visuals().panel_fill };
                    ui.painter().rect_filled(rect, egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, fill);
                    if active {
                        // Accent line on the active tab, in the theme's caret color.
                        let accent = self.docs[i].highlighter.caret();
                        ui.painter().line_segment([rect.left_top() + vec2(6.0, 0.5), rect.right_top() + vec2(-6.0, 0.5)], Stroke::new(2.0, accent));
                    }
                    let text_color = if active { fg } else { fg.gamma_multiply(0.6) };
                    let mut job = egui::text::LayoutJob::single_section(name.clone(), egui::TextFormat { font_id: font.clone(), color: text_color, italics: d.preview, ..Default::default() });
                    job.wrap.max_width = w - 44.0;
                    job.wrap.max_rows = 1;
                    job.wrap.break_anywhere = true;
                    let g = ui.painter().layout_job(job);
                    ui.painter().galley(pos2(rect.left() + 14.0, rect.center().y - g.size().y / 2.0), g, text_color);

                    if over_close || (resp.hovered() && !d.is_dirty()) || (active && !d.is_dirty()) {
                        if over_close {
                            ui.painter().rect_filled(close, 4.0, mix(fill, fg, 0.15));
                        }
                        let c = close.center();
                        let s = 3.5;
                        let stroke = Stroke::new(1.3, text_color);
                        ui.painter().line_segment([c + vec2(-s, -s), c + vec2(s, s)], stroke);
                        ui.painter().line_segment([c + vec2(-s, s), c + vec2(s, -s)], stroke);
                    } else if d.is_dirty() {
                        ui.painter().circle_filled(close.center(), 4.0, text_color);
                    }
                    if d.conflict {
                        ui.painter().circle_filled(pos2(rect.left() + 7.0, rect.center().y), 2.5, Color32::from_rgb(0xe5, 0x9e, 0x40));
                    }

                    let resp = resp.on_hover_text(d.path.as_ref().map_or_else(|| "Unsaved new file".to_owned(), |p| p.display().to_string()));
                    if resp.middle_clicked() || (resp.clicked() && over_close) {
                        action = Some(TabAction::Close(i));
                    } else if resp.drag_started() {
                        self.dragging_tab = Some(i);
                        action = Some(TabAction::Activate(i));
                    } else if resp.double_clicked() && d.preview {
                        self.docs[i].preview = false;
                    } else if resp.clicked() {
                        action = Some(TabAction::Activate(i));
                    }
                }
                // Double-click the empty part of the bar for a new tab.
                let rest = ui.allocate_exact_size(vec2(ui.available_width().max(40.0), height), Sense::click()).1;
                if rest.double_clicked() {
                    action = Some(TabAction::New);
                }
            });
        });
        if let Some(from) = self.dragging_tab {
            let (down, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.hover_pos()));
            if !down {
                self.dragging_tab = None;
            } else if let Some(p) = pos {
                if let Some(to) = rects.iter().position(|r| p.x >= r.left() && p.x < r.right()) {
                    if to != from {
                        action = Some(TabAction::Move(from, to));
                        self.dragging_tab = Some(to);
                    }
                }
            }
        }
        action
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let d = &mut self.docs[self.active];
            let (line, col) = d.editor.cursor_line_col();
            let cursors = d.editor.selections().len();
            let selected: usize = d.editor.selections().iter().map(|s| s.end() - s.start()).sum();
            let mut pos = format!("Line {line}, Column {col}");
            if cursors > 1 {
                pos.push_str(&format!("  ·  {cursors} cursors"));
            }
            if selected > 0 {
                pos.push_str(&format!("  ·  {selected} selected"));
            }
            ui.label(pos);
            if d.conflict {
                ui.separator();
                ui.colored_label(Color32::from_rgb(0xe5, 0x9e, 0x40), "Changed on disk");
                if ui.small_button("Reload").on_hover_text("Discard your edits and load the file from disk").clicked() {
                    if let Some(bytes) = d.path.as_ref().and_then(|p| std::fs::read(p).ok()) {
                        d.reload(Buffer::from_bytes(&bytes));
                        d.restored_dirty = false;
                    }
                }
            }
            ui.separator();
            ui.label(&self.status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let d = &mut self.docs[self.active];
                ui.menu_button(d.highlighter.syntax_name().to_owned(), |ui| {
                    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        for name in crate::highlight::syntax_names() {
                            if ui.selectable_label(d.highlighter.syntax_name() == name, name).clicked() {
                                d.highlighter.set_syntax(name);
                                ui.close();
                            }
                        }
                    });
                });
                ui.separator();
                let ending = d.editor.buffer.line_ending();
                ui.menu_button(if ending == LineEnding::Lf { "LF" } else { "CRLF" }, |ui| {
                    ui.label(egui::RichText::new("Convert line endings").weak());
                    for (e, label) in [(LineEnding::Lf, "LF (macOS, Linux)"), (LineEnding::CrLf, "CRLF (Windows)")] {
                        if ui.selectable_label(ending == e, label).clicked() {
                            d.set_line_ending(e);
                            ui.close();
                        }
                    }
                });
                ui.separator();
                let mut tab = self.settings.tab_size;
                let mut spaces = self.settings.translate_tabs_to_spaces;
                ui.menu_button(format!("{}: {tab}", if spaces { "Spaces" } else { "Tab Size" }), |ui| {
                    ui.checkbox(&mut spaces, "Indent using spaces");
                    ui.separator();
                    for n in [2, 3, 4, 8] {
                        ui.radio_value(&mut tab, n, format!("Width: {n}"));
                    }
                });
                if (tab, spaces) != (self.settings.tab_size, self.settings.translate_tabs_to_spaces) {
                    self.settings.tab_size = tab;
                    self.settings.translate_tabs_to_spaces = spaces;
                    self.apply_settings();
                    self.persist_settings();
                }
                ui.separator();
                let theme = self.docs[self.active].highlighter.theme_name().to_owned();
                ui.menu_button("Theme", |ui| {
                    ui.label(egui::RichText::new("Color scheme").weak());
                    for name in crate::highlight::theme_names() {
                        if ui.selectable_label(theme == name, name).clicked() {
                            self.settings.theme = name.to_owned();
                            self.apply_settings();
                            self.persist_settings();
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("Color scheme");
            });
        });
    }
}

impl eframe::App for StrataApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.handle_shortcuts(ctx);
        self.devshot.tick(ctx);
        self.style_chrome(ctx);

        if ctx.input(|i| i.viewport().close_requested()) {
            // Hot exit: unsaved work goes into the session instead of a prompt.
            let saved = self.settings.hot_exit && self.save_session();
            if !saved {
                for i in 0..self.docs.len() {
                    if !self.confirm_close(i) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                        break;
                    }
                }
            }
        } else if now - self.last_session_save > 5.0 {
            // Also save as you go, so a crash loses at most a few seconds.
            self.last_session_save = now;
            if self.settings.hot_exit && self.session_fingerprint() != self.session_key {
                self.save_session();
            }
        }
        // Files and folders dropped on the window open in tabs / the sidebar.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_owned()).filter(|p| !p.as_os_str().is_empty()).collect());
        for p in dropped {
            if p.is_dir() {
                self.sidebar.open_folder(p);
            } else {
                self.open_path(p, false);
            }
        }
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            let r = ctx.content_rect().shrink(12.0);
            let accent = self.docs[self.active].highlighter.caret();
            painter.rect_filled(r, 10.0, Color32::from_black_alpha(120));
            painter.rect_stroke(r, 10.0, Stroke::new(2.0, accent), egui::StrokeKind::Inside);
            painter.text(r.center(), egui::Align2::CENTER_CENTER, "Drop to open", FontId::proportional(22.0), accent);
        }
        if now - self.last_disk_check > 1.0 {
            self.last_disk_check = now;
            let d = self.doc();
            if d.check_disk() {
                self.status = format!("Reloaded {} (changed on disk)", d.name());
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
        for d in &mut self.docs {
            if d.preview && d.editor.version() > 0 {
                d.preview = false;
            }
            d.view.font_size = self.settings.font_size;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.title()));

        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        if self.find.open {
            egui::Panel::bottom("find").show(ui, |ui| self.find_bar(ui));
        }

        if self.sidebar.visible && self.sidebar.root.is_some() {
            let current = self.docs[self.active].path.clone();
            let names = self.display_names();
            let open: Vec<sidebar::OpenFile> = self
                .docs
                .iter()
                .zip(names)
                .enumerate()
                .map(|(i, (d, name))| sidebar::OpenFile { name, dirty: d.is_dirty(), active: i == self.active })
                .collect();
            let action = egui::Panel::left("sidebar")
                .resizable(true)
                .default_size(260.0)
                .show(ui, |ui| self.sidebar.show(ui, current.as_deref(), &open))
                .inner;
            match action {
                Some(sidebar::Action::Open(p, preview)) => {
                    self.open_path(p, preview);
                    // Keep focus in the tree for previews so arrowing through files works.
                    if !preview {
                        self.doc().view.focus();
                    }
                }
                Some(sidebar::Action::OpenAt(p, line, cols)) => self.open_at(p, line, cols),
                Some(sidebar::Action::Activate(i)) => self.activate(i),
                Some(sidebar::Action::Close(i)) => {
                    self.close_tab(i);
                }
                Some(sidebar::Action::Renamed(from, to)) => {
                    // Open tabs inside a renamed folder (or the file itself) follow it.
                    for d in &mut self.docs {
                        if let Some(rest) = d.path.as_ref().and_then(|p| p.strip_prefix(&from).ok()) {
                            let new = if rest.as_os_str().is_empty() { to.clone() } else { to.join(rest) };
                            d.highlighter.detect(Some(&new), &d.editor.buffer.line_text(0));
                            d.disk_mtime = crate::document::mtime(&new);
                            d.path = Some(new);
                        }
                    }
                    self.status = format!("Renamed to {}", to.display());
                }
                Some(sidebar::Action::Status(s)) => self.status = s,
                None => {}
            }
        }

        let tab_action = egui::Panel::top("tabs")
            .exact_size(32.0)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.tab_bar(ui))
            .inner;
        match tab_action {
            Some(TabAction::Activate(i)) => self.activate(i),
            Some(TabAction::Close(i)) => {
                self.close_tab(i);
            }
            Some(TabAction::Move(from, to)) => {
                let d = self.docs.remove(from);
                self.docs.insert(to, d);
                self.active = to;
            }
            Some(TabAction::New) => self.push_doc(Document::untitled()),
            None => {}
        }

        let find_open = self.find.open;
        let d = &mut self.docs[self.active];
        let matches = if find_open { std::mem::take(&mut d.finder.matches) } else { Vec::new() };
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| d.view.show(ui, &mut d.editor, &mut d.highlighter, &matches));
        if find_open {
            d.finder.matches = matches;
        }
        if self.docs.len() == 1 && self.docs[0].is_blank() && !self.palette.open {
            self.welcome(ctx);
        }
        if let Some(spec) = self.dev_palette.take() {
            let (mode, q) = spec.split_once(':').unwrap_or((&spec, ""));
            let mode = if mode == "p" { palette::Mode::Files } else { palette::Mode::Commands };
            self.open_palette(ctx, mode, q);
        }
        let events = self.palette.show(ctx);
        self.palette_events(ctx, events);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use egui::{Context, Event, Modifiers, Pos2, RawInput};

    fn app_with(text: &str) -> StrataApp {
        let mut app = StrataApp::new(None, None);
        app.docs[0].editor = Editor::new(Buffer::from_str(text));
        app
    }

    fn text(app: &StrataApp) -> String {
        let e = &app.docs[app.active].editor;
        e.buffer.slice(0..e.buffer.len_chars())
    }

    fn ed(app: &StrataApp) -> &Editor {
        &app.docs[app.active].editor
    }

    fn query(app: &mut StrataApp, text: &str, regex: bool) {
        app.find.query = Query { text: text.into(), regex, case_sensitive: true, whole_word: false };
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-app-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn find_next_prev_wrap() {
        let mut app = app_with("cat dog cat dog");
        query(&mut app, "dog", false);
        app.find_step(true);
        assert_eq!(ed(&app).primary().range(), 4..7);
        app.find_step(true);
        assert_eq!(ed(&app).primary().range(), 12..15);
        app.find_step(true);
        assert_eq!(ed(&app).primary().range(), 4..7, "wraps to top");
        app.find_step(false);
        assert_eq!(ed(&app).primary().range(), 12..15, "wraps to bottom");
    }

    #[test]
    fn find_all_makes_cursors_and_closes_bar() {
        let mut app = app_with("x1 x2 x3");
        app.open_find(false);
        query(&mut app, r"x\d", true);
        app.find_all();
        assert_eq!(ed(&app).selections().len(), 3);
        assert!(!app.find.open);
        app.doc().editor.insert_text("y");
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
        app.doc().editor.undo();
        assert_eq!(text(&app), "a=1\nb=22\n");
    }

    #[test]
    fn replace_one_replaces_current_then_moves_on() {
        let mut app = app_with("aa bb aa");
        query(&mut app, "aa", false);
        app.find.replacement = "cc".into();
        app.replace_one(); // first press only selects the match
        assert_eq!(ed(&app).primary().range(), 0..2);
        app.replace_one();
        assert_eq!(text(&app), "cc bb aa");
        assert_eq!(ed(&app).primary().range(), 6..8);
    }

    #[test]
    fn typing_a_query_jumps_and_counts() {
        let mut app = app_with("one two one two");
        app.doc().editor.select_ranges(&[5..5], 0);
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
        assert_eq!(ed(&app).primary().range(), 8..11, "first match after the cursor");
        assert_eq!(app.match_label().text(), "2 of 2");
    }

    #[test]
    fn find_in_files_hit_opens_and_selects() {
        let dir = temp_dir("fif");
        std::fs::write(dir.join("x.py"), "a = 1\n    target = 2\n").unwrap();
        let mut app = StrataApp::new(Some(dir.clone()), None);
        assert_eq!(app.sidebar.root.as_deref(), Some(dir.as_path()));
        app.sidebar.search.start(&dir, &Query::literal("target", false), None);
        while app.sidebar.search.running {
            app.sidebar.search.poll();
        }
        let f = &app.sidebar.search.results[0];
        let (p, h) = (f.path.clone(), f.hits[0].clone());
        app.open_at(p.clone(), h.line, h.cols);
        assert_eq!(app.docs.len(), 1, "the blank untitled tab was reused");
        assert_eq!(app.doc().path.as_deref(), Some(p.as_path()));
        let r = ed(&app).primary().range();
        assert_eq!(ed(&app).buffer.slice(r), "target");
        assert_eq!(app.doc().highlighter.syntax_name(), "Python");

        // The sidebar renders the tree headlessly without panicking.
        let ctx = Context::default();
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(300.0, 400.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            app.sidebar.show(ui, Some(&p), &[]);
        });
        out.textures_delta.clear();
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn bad_regex_shows_error_instead_of_matches() {
        let mut app = app_with("abc");
        query(&mut app, "(", true);
        app.refresh_matches();
        assert!(app.doc().finder.error.is_some());
        assert!(app.doc().finder.matches.is_empty());
    }

    #[test]
    fn tabs_open_reuse_preview_and_close() {
        let dir = temp_dir("tabs");
        for f in ["a.rs", "b.rs", "c.rs", "d.rs"] {
            std::fs::write(dir.join(f), f).unwrap();
        }
        let mut app = StrataApp::new(None, None);
        app.open_path(dir.join("a.rs"), false);
        app.open_path(dir.join("b.rs"), false);
        assert_eq!((app.docs.len(), app.active), (2, 1), "the blank tab was reused for a.rs");
        app.open_path(dir.join("a.rs"), false);
        assert_eq!((app.docs.len(), app.active), (2, 0), "an open file is reused");

        // Previews replace each other until one is edited.
        app.open_path(dir.join("c.rs"), true);
        app.open_path(dir.join("d.rs"), true);
        assert_eq!(app.docs.len(), 3);
        assert!(app.docs.iter().all(|d| d.path != Some(dir.join("c.rs"))));
        app.doc().editor.insert_text("edit");
        app.doc().preview = false; // what a frame does after an edit
        app.open_path(dir.join("c.rs"), true);
        assert_eq!(app.docs.len(), 4);

        app.activate(2);
        assert!(app.close_tab(0));
        assert_eq!(app.docs.len(), 3);
        assert_eq!(app.active, 1, "active tab index follows the removal");
        app.reopen_closed();
        assert_eq!(app.docs.len(), 4);
        assert_eq!(app.doc().path, Some(dir.join("a.rs")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn hot_exit_round_trips_unsaved_text() {
        let config = temp_dir("session");
        let file = config.join("notes.md");
        std::fs::write(&file, "saved\n").unwrap();
        let mut app = StrataApp::new(Some(file.clone()), Some(config.clone()));
        app.doc().editor.select_ranges(&[5..5], 0);
        app.doc().editor.insert_text(" and edited");
        app.push_doc(Document::untitled());
        app.doc().editor.insert_text("scratch");
        assert!(app.save_session());

        let mut back = StrataApp::new(None, Some(config.clone()));
        assert_eq!(back.docs.len(), 2);
        assert_eq!(back.active, 1);
        assert_eq!(text(&back), "scratch");
        assert!(back.docs[1].is_dirty());
        back.activate(0);
        assert_eq!(text(&back), "saved and edited\n");
        assert_eq!(back.doc().path.as_deref(), Some(file.as_path()));
        assert_eq!(ed(&back).primary().head, 16);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "saved\n", "disk untouched until you save");
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn full_frame_with_tabs_renders() {
        let mut app = app_with("fn main() {}\n");
        app.push_doc(Document::untitled());
        let ctx = Context::default();
        for _ in 0..2 {
            let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0))), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                let mut frame = eframe::Frame::_new_kittest();
                eframe::App::ui(&mut app, ui, &mut frame);
            });
            out.textures_delta.clear();
        }
        assert_eq!(app.docs.len(), 2);
    }
}
