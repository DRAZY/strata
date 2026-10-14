//! The editor window: tabs, sidebar, find bar, status bar, and the session.

use crate::buffer::{Buffer, LineEnding};
use crate::commands::Cmd;
use crate::devshot::DevShot;
use crate::document::Document;
use crate::find::Query;
use crate::palette::{self, Palette};
use crate::panes::Layout;
use crate::session::{Session, SessionLock, Settings, TabState, WindowState};
use crate::sidebar::{self, Sidebar};
use egui::{Color32, FontId, Key, Rect, Sense, Stroke, pos2, vec2};
use std::path::{Path, PathBuf};

/// The find bar (its options live in `find_options`, which also has `Default`).
struct FindBar {
    open: bool,
    replace: bool,
    query: Query,
    replacement: String,
    focus_query: bool,
    /// Where incremental search starts: the cursor when the bar opened.
    origin: usize,
    last_query: Query,
    in_selection: bool,
    wrap: bool,
    preserve_case: bool,
    highlight: bool,
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
    /// settings.json's modification time when last read or written.
    settings_mtime: Option<std::time::SystemTime>,
    last_session_save: f64,
    /// What the session looked like when last written, to skip no-op saves.
    session_key: u64,
    /// Theme the window chrome was last styled for.
    chrome_theme: String,
    /// The window is full screen, as the system last reported it (it can be
    /// left with the green button or Esc, not only from the menu).
    fullscreen: bool,
    /// Whether the window was full screen before distraction-free mode.
    fullscreen_before_distraction: bool,
    /// Tab being dragged to a new position (or another pane).
    dragging_tab: Option<usize>,
    layout: Layout,
    /// Pane with keyboard focus; new tabs open here. It may be empty.
    focus_group: usize,
    /// Source of `Document::stamp`.
    next_stamp: u64,
    palette: Palette,
    /// Selections to put back if a palette preview (line, symbol) is cancelled.
    palette_restore: Option<(usize, Vec<std::ops::Range<usize>>, usize)>,
    /// Held while this window owns the session; None means hot exit is off
    /// for this window (another Strata has the session, or tests).
    session_lock: Option<SessionLock>,
    recent_files: Vec<PathBuf>,
    recent_folders: Vec<PathBuf>,
    /// Cursor history for Jump Back / Jump Forward: (view id, position).
    jumps_back: Vec<(egui::Id, usize)>,
    jumps_forward: Vec<(egui::Id, usize)>,
    /// Distraction-free mode is on: whether the sidebar was visible before.
    /// The minimap is hidden as a view override (see `minimap_shown`), so
    /// the setting is never changed, saved, or passed to other windows.
    distraction: Option<bool>,
    about_open: bool,
    markdown: crate::markdown::Preview,
    #[cfg(target_os = "macos")]
    native_menu: Option<crate::menus::native::NativeMenu>,
    build: crate::build::Build,
    /// Build With… choices offered in the palette.
    build_choices: Vec<crate::build::BuildChoice>,
    completion: completion::Completion,
    snippets: snippets_ui::SnippetState,
    choosing: more::Choosing,
    /// Text copied from Strata, newest first (Paste from History).
    clip_history: Vec<String>,
    /// Position in find-in-files results for Next/Previous Result, and the
    /// search (`Search::generation`) it is a position in.
    find_result: Option<(u64, usize)>,
    /// Ctrl+Tab walk: tab ids by recency, position, time of the last press.
    stack_walk: Option<(Vec<egui::Id>, usize, f64)>,
    /// Build system picked in Tools ▸ Build System (None = automatic).
    build_system: Option<String>,
    macro_recording: Option<Vec<egui::Event>>,
    macro_last: Vec<egui::Event>,
    recent_projects: Vec<PathBuf>,
    project_path: Option<PathBuf>,
    licenses_open: bool,
    /// Fonts last loaded: (editor face, interface face, interface size bits).
    fonts_key: Option<(String, String, u32)>,
    window_focused: bool,
    /// Git status for open files, computed off the UI thread.
    git: crate::git::GitWatcher,
    last_git_request: f64,
    /// The watcher was made before a UI context existed (rebuilt on the first frame).
    git_ctx_missing: bool,
    /// Text removed by Delete to Mark, for Yank.
    yank: String,
    /// Encoding chosen in the status bar: (reopen?, encoding), applied after drawing.
    pending_encoding: Option<(bool, crate::encoding::Encoding)>,
    /// Input synthesized by menu items (copy, paste, undo…) for the next frame.
    pending_events: Vec<egui::Event>,
    /// Dev aid: STRATA_DEV_PALETTE="p:query" (or "cmd:query") opens a panel on the first frame.
    dev_palette: Option<String>,
    terminal: crate::terminal::Panel,
    /// The terminal panel's launch-time setup ran.
    terminal_started: bool,
    /// Right-click menus (see `context_menus`).
    context: context_menus::ContextMenus,
    /// Ties to the other windows (see `windows`).
    link: windows::Link,
    /// keybindings.json and a pending chord (see `keymap_ui`).
    keymap: keymap_ui::KeymapState,
    /// Help ▸ Documentation (see `help_ui`).
    help: help_ui::HelpState,
    /// "Strata quit unexpectedly last time" (see `diagnostics`).
    crash_notice: Option<crate::crash::PreviousCrash>,
    /// Tabs sharing a buffer (see `clones`).
    views: clones::Views,
    /// File ▸ Print… options (see `printing`).
    print: printing::PrintDialog,
    /// A folder-wide search running in the background (see `lookup`).
    lookup: lookup::Lookup,
    /// The menu description as last built: (key mode, time built, menus).
    /// Rebuilt after input, a menu action, or a second (see `fresh_menus`).
    menus_cache: Option<(u8, f64, crate::menus::Menus)>,
    menus_stale: bool,
    /// The git gutter setting the tabs' line marks were made for.
    git_marks_on: bool,
    /// The window title last sent.
    last_title: String,
}

mod autosave;
#[cfg(test)]
mod bench;
mod clones;
mod compare_ui;
mod completion;
mod context_menus;
mod diagnostics;
mod features;
mod find_options;
#[cfg(test)]
mod headless;
mod help_ui;
mod keymap_ui;
mod large_files;
mod launch;
mod lookup;
pub(crate) mod menu_model;
mod more;
mod terminal_ui;
mod preview;
mod printing;
mod replace_files;
mod snippets_ui;
mod spelling;
mod textutil;
mod theme_ui;
#[cfg(test)]
mod ui_tests;
mod windows;

pub use windows::Windows;

#[cfg(test)]
use menu_model::EDITOR_COMMANDS;

impl StrataApp {
    pub fn new(path: Option<PathBuf>, config: Option<PathBuf>) -> Self {
        let asset_errors = crate::highlight::load_user_assets(config.as_deref());
        let (settings, settings_error) = config.as_deref().map(Settings::load).unwrap_or_default();
        let mut app = Self::with_settings(config, settings, startup_status(settings_error, asset_errors));
        if let Some(dir) = app.config.clone() {
            app.session_lock = SessionLock::acquire(&dir);
            if app.session_lock.is_none() {
                app.status = "Another Strata window owns the session: this one asks before closing unsaved tabs".into();
            }
        }
        if app.hot_exit() {
            app.restore_session();
        }
        match path {
            Some(p) if p.is_dir() => app.open_folder(p),
            Some(p) => app.open_path(p, false),
            None => {}
        }
        if app.docs.is_empty() {
            app.push_doc(Document::untitled());
        }
        app.session_key = app.session_fingerprint();

        // Dev aids (pair with STRATA_SCREENSHOT): run find-in-files on the open folders
        // (or open files), or open find/replace with a query.
        if let Ok(q) = std::env::var("STRATA_DEV_FIND_IN_FILES") {
            app.dev_find_in_files(q);
        }
        app.dev_select();
        if let Ok(q) = std::env::var("STRATA_DEV_FIND") {
            app.open_find(true);
            app.find.query.text = q;
            app.find.query.regex = std::env::var_os("STRATA_DEV_FIND_REGEX").is_some();
        }
        app
    }

    /// A window with no tabs yet, no session lock and nothing restored.
    fn with_settings(config: Option<PathBuf>, settings: Settings, status: String) -> Self {
        let settings_mtime = config.as_deref().and_then(|d| std::fs::metadata(Settings::path(d)).and_then(|m| m.modified()).ok());
        let mut app = Self {
            docs: Vec::new(),
            active: 0,
            closed: Vec::new(),
            status,
            devshot: DevShot::default(),
            find: FindBar::default(),
            sidebar: Sidebar::new(),
            settings,
            keymap: keymap_ui::KeymapState::new(config.as_deref()),
            config,
            last_disk_check: 0.0,
            settings_mtime,
            last_session_save: 0.0,
            session_key: 0,
            chrome_theme: String::new(),
            fullscreen: false,
            fullscreen_before_distraction: false,
            dragging_tab: None,
            layout: Layout::default(),
            focus_group: 0,
            next_stamp: 1,
            palette: Palette::new(menu_model::palette_editor_commands()),
            palette_restore: None,
            dev_palette: std::env::var("STRATA_DEV_PALETTE").ok(),
            session_lock: None,
            recent_files: Vec::new(),
            recent_folders: Vec::new(),
            jumps_back: Vec::new(),
            jumps_forward: Vec::new(),
            distraction: None,
            about_open: false,
            markdown: crate::markdown::Preview::default(),
            #[cfg(target_os = "macos")]
            native_menu: None,
            pending_events: Vec::new(),
            pending_encoding: None,
            build: crate::build::Build::new(None),
            build_choices: Vec::new(),
            completion: completion::Completion::default(),
            snippets: Default::default(),
            choosing: more::Choosing::Nothing,
            clip_history: Vec::new(),
            find_result: None,
            stack_walk: None,
            build_system: None,
            macro_recording: None,
            macro_last: Vec::new(),
            recent_projects: Vec::new(),
            project_path: None,
            licenses_open: false,
            yank: String::new(),
            fonts_key: None,
            window_focused: true,
            git: crate::git::GitWatcher::new(None),
            last_git_request: 0.0,
            git_ctx_missing: true,
            terminal: crate::terminal::Panel::new(),
            terminal_started: false,
            context: context_menus::ContextMenus::new(),
            link: windows::Link::default(),
            help: help_ui::HelpState::default(),
            crash_notice: None,
            views: Default::default(),
            print: Default::default(),
            lookup: Default::default(),
            menus_cache: None,
            menus_stale: true,
            git_marks_on: true,
            last_title: String::new(),
        };
        app.keymap_startup_status();
        app.build = crate::build::Build::new(app.config.clone());
        app.snippets = snippets_ui::SnippetState::new(app.config.as_deref());
        app
    }

    /// Ask for fresh git changes for the visible tabs when they've been
    /// edited (or every few seconds, to notice commits), and collect results.
    fn update_git(&mut self, ctx: &egui::Context, now: f64) {
        if self.git_ctx_missing {
            self.git = crate::git::GitWatcher::new(Some(ctx.clone()));
            self.git_ctx_missing = false;
        }
        while let Some((path, version, changes)) = self.git.poll() {
            for d in self.docs.iter_mut().filter(|d| d.path.as_ref() == Some(&path) && d.editor.version() == version) {
                d.git_changes = changes.clone();
                d.view.line_marks = git_marks(&d.git_changes, self.settings.git_gutter);
            }
        }
        let periodic = now - self.last_git_request > 3.0;
        for g in 0..self.layout.groups() {
            let Some(i) = self.group_active(g) else { continue };
            let d = &mut self.docs[i];
            let Some(path) = d.path.clone() else { continue };
            let v = d.editor.version();
            if d.git_version != Some(v) || periodic {
                d.git_version = Some(v);
                self.git.request(&path, d.editor.buffer.snapshot(), v);
            }
        }
        if periodic {
            self.last_git_request = now;
        }
    }

    /// Is word wrap on for tab `i`?
    fn wraps(&self, i: usize) -> bool {
        let d = &self.docs[i];
        let prose = matches!(d.highlighter.syntax_name(), "Markdown" | "MultiMarkdown" | "Plain Text" | "reStructuredText" | "Textile" | "LaTeX");
        d.can_wrap() && d.wrap.unwrap_or((self.settings.word_wrap || prose) && d.large.is_none())
    }

    /// Use the macOS menu bar. Must run on the main thread (not in tests).
    #[cfg(target_os = "macos")]
    pub fn install_native_menu(&mut self, ctx: &egui::Context) {
        self.native_menu = Some(crate::menus::native::NativeMenu::new(ctx));
    }

    fn hot_exit(&self) -> bool {
        self.settings.hot_exit && self.session_lock.is_some()
    }

    fn doc(&mut self) -> &mut Document {
        &mut self.docs[self.active]
    }

    // ---- tabs ----------------------------------------------------------

    /// Style a new tab with the current settings and add it to the focused
    /// pane, after that pane's current tab.
    fn push_doc(&mut self, mut doc: Document) {
        doc.highlighter.set_theme(&self.active_theme());
        doc.editor.tab_width = self.settings.tab_size;
        doc.editor.use_spaces = self.settings.translate_tabs_to_spaces;
        doc.group = self.focus_group;
        let at = match self.group_active(self.focus_group) {
            Some(i) => i + 1,
            None => self.docs.len(),
        };
        self.docs.insert(at, doc);
        self.activate(at);
    }

    /// Focus tab `i` (and its pane).
    fn activate(&mut self, i: usize) {
        self.select(i);
        for d in &mut self.docs {
            d.view.cancel_focus();
        }
        if let Some(d) = self.docs.get_mut(self.active) {
            d.view.focus();
        }
    }

    /// Make `i` the current tab without asking for keyboard focus.
    fn select(&mut self, i: usize) {
        self.active = i.min(self.docs.len().saturating_sub(1));
        self.next_stamp += 1;
        let stamp = self.next_stamp;
        if let Some(d) = self.docs.get_mut(self.active) {
            d.stamp = stamp;
            self.focus_group = d.group;
            if d.check_disk() {
                self.status = format!("Reloaded {} (changed on disk)", d.name());
            }
        }
    }

    /// The tab a pane shows: its most recently activated one.
    fn group_active(&self, g: usize) -> Option<usize> {
        self.docs.iter().enumerate().filter(|(_, d)| d.group == g).max_by_key(|(_, d)| d.stamp).map(|(i, _)| i)
    }

    fn group_tabs(&self, g: usize) -> Vec<usize> {
        (0..self.docs.len()).filter(|&i| self.docs[i].group == g).collect()
    }

    /// Switch layouts; tabs in panes that disappear move to the last remaining one.
    fn set_layout(&mut self, layout: Layout) {
        let last = layout.groups() - 1;
        for d in &mut self.docs {
            d.group = d.group.min(last);
        }
        self.layout = layout;
        self.focus_group = self.focus_group.min(last);
        self.activate(self.active);
    }

    fn focus_pane(&mut self, g: usize) {
        if g >= self.layout.groups() {
            return;
        }
        match self.group_active(g) {
            Some(i) => self.activate(i),
            None => self.focus_group = g,
        }
    }

    /// Move the current tab to pane `g`, adding columns if needed.
    fn move_to_pane(&mut self, g: usize) {
        if g >= self.layout.groups() {
            self.set_layout(Layout::grid(g + 1, 1));
        }
        if let Some(d) = self.docs.get_mut(self.active) {
            d.group = g;
        }
        self.activate(self.active);
    }

    /// Open a file in a tab, reusing its tab if it's already open.
    /// `preview` tabs (single click in the sidebar) replace each other until edited.
    fn open_path(&mut self, path: PathBuf, preview: bool) {
        if let Some(i) = self.tab_for_path(&path) {
            if !preview {
                self.docs[i].preview = false;
            }
            self.activate(i);
            return;
        }
        // Open in another window: that window and tab come forward instead.
        if self.link.open_elsewhere(&path) {
            return;
        }
        if !self.confirm_open(&path) {
            return;
        }
        let mut doc = match Document::open_with(path.clone(), self.large_file_bytes()) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("couldn't open {}: {e}", path.display());
                self.status = format!("Could not open {}: {e}", path.display());
                return;
            }
        };
        doc.preview = preview;
        self.status = large_files::opened_status(&doc, &path);
        if !preview {
            self.remember_file(&path);
        }
        let guessed = if self.settings.detect_indentation { doc.editor.guess_indentation() } else { None };
        // Replace an earlier preview, or a blank untitled tab, in the focused pane.
        let g = self.focus_group;
        let shown = self.group_active(g);
        let reuse = if preview { self.docs.iter().position(|d| d.preview && d.group == g) } else { None }
            .or_else(|| shown.filter(|&i| self.docs[i].is_blank()));
        if let Some(i) = reuse {
            doc.highlighter.set_theme(&self.active_theme());
            doc.editor.tab_width = self.settings.tab_size;
            doc.editor.use_spaces = self.settings.translate_tabs_to_spaces;
            doc.group = g;
            self.docs[i] = doc;
            self.activate(i);
        } else {
            self.push_doc(doc);
        }
        // Follow the file's own indentation (tabs vs spaces, width).
        if let Some((spaces, width)) = guessed {
            let e = &mut self.docs[self.active].editor;
            e.use_spaces = spaces;
            e.tab_width = width;
        }
    }

    /// Open (if needed) and select a find-in-files hit.
    fn open_at(&mut self, path: PathBuf, line: usize, cols: std::ops::Range<usize>) {
        self.push_jump();
        self.open_path(path.clone(), false);
        let d = self.doc();
        if d.path.as_ref() != Some(&path) {
            self.link.select_elsewhere(&path, line, cols);
            return;
        }
        select_on_line(d, line, cols);
        d.view.focus();
    }

    /// Goto Anything's `file@name`, once the file is open: the symbol best
    /// matching `query`, or the symbol list when it's empty.
    fn goto_symbol_named(&mut self, ctx: &egui::Context, query: &str) {
        if query.is_empty() {
            return self.run(ctx, Cmd::GotoSymbol);
        }
        let d = self.doc();
        let text = d.editor.text_snapshot();
        let symbols = crate::symbols::symbols(d.highlighter.syntax_name(), &text);
        match crate::fuzzy::rank(query, &symbols, |s| &s.name, 1).first() {
            Some(&(i, _, _)) => self.goto_line(symbols[i].line),
            None => self.status = format!("No symbol matching “{query}” in {}", d.name()),
        }
    }

    /// Goto Anything's `file#text`, once the file is open: select the first match.
    fn find_in_file(&mut self, query: &str) {
        let d = self.doc();
        let text = d.editor.text_snapshot();
        match palette::text_matches(&text, query, 1).into_iter().next() {
            Some(m) => select_on_line(d, m.line, m.cols),
            None => self.status = format!("“{query}” isn't in {}", d.name()),
        }
    }

    /// Ask before closing a tab with unsaved changes. True if it may close.
    fn confirm_close(&mut self, i: usize) -> bool {
        if !self.docs[i].is_dirty() || self.later_view(i) {
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
        if i >= self.docs.len() || (self.other_view(i).is_none() && !self.confirm_close(i)) {
            return false;
        }
        self.dragging_tab = None;
        let doc = self.docs.remove(i);
        let group = doc.group;
        if let Some(p) = doc.path {
            self.closed.retain(|c| c != &p);
            self.closed.push(p);
        }
        // Next: the pane's previous tab; an emptied pane hands focus to the most
        // recent tab elsewhere (a lone pane gets a fresh untitled tab).
        let next = self.group_active(group).or_else(|| {
            if self.layout.groups() > 1 { (0..self.docs.len()).max_by_key(|&k| self.docs[k].stamp) } else { None }
        });
        match next {
            Some(n) => self.activate(n),
            None => {
                self.focus_group = group;
                self.active = 0;
                self.push_doc(Document::untitled());
            }
        }
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
            _ => self.save_dialog(i).save_file(),
        };
        let Some(path) = target else { return false };
        if !self.confirm_overwrite(i, &path) {
            return false;
        }
        self.before_save(i);
        match self.docs[i].write_to(path.clone()) {
            Ok(()) => {
                self.git.invalidate(&path);
                self.docs[i].git_version = None;
                self.status = format!("Saved {}", path.display());
                self.sidebar.refresh();
                if self.config.as_deref().map(Settings::path).as_ref() == Some(&path) {
                    self.reload_settings();
                }
                self.snippet_saved(&path);
                true
            }
            Err(e) => {
                log::warn!("couldn't save {}: {e}", path.display());
                self.status = format!("Save failed: {e}");
                false
            }
        }
    }

    /// Saving would clobber outside changes, bytes that didn't decode, or
    /// text the file's encoding can't hold: ask first.
    fn confirm_overwrite(&mut self, i: usize, path: &std::path::Path) -> bool {
        let d = &self.docs[i];
        if let Err(msg) = d.editor.buffer.check_encodable(d.encoding) {
            let ok = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Can't save in this encoding")
                .set_description(format!("{msg}.\n\nSave {} as UTF-8 instead?", d.name()))
                .set_buttons(rfd::MessageButtons::OkCancelCustom("Save as UTF-8".into(), "Cancel".into()))
                .show();
            if !matches!(ok, rfd::MessageDialogResult::Ok | rfd::MessageDialogResult::Yes) && ok != rfd::MessageDialogResult::Custom("Save as UTF-8".into()) {
                return false;
            }
            self.docs[i].encoding = crate::encoding::Encoding::UTF8;
        }
        let d = &self.docs[i];
        let same_file = d.path.as_deref() == Some(path);
        let mut reasons = Vec::new();
        if same_file && d.changed_on_disk() {
            reasons.push("It was changed by another program since you opened it; saving replaces those changes.");
        }
        if d.editor.buffer.lossy {
            reasons.push("Some bytes couldn't be decoded and show as \u{FFFD}; saving writes that character in their place.");
        }
        if reasons.is_empty() {
            return true;
        }
        let ok = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Overwrite file?")
            .set_description(format!("{}\n\n{}", d.name(), reasons.join("\n\n")))
            .set_buttons(rfd::MessageButtons::OkCancelCustom("Save Anyway".into(), "Cancel".into()))
            .show();
        matches!(ok, rfd::MessageDialogResult::Ok | rfd::MessageDialogResult::Yes) || ok == rfd::MessageDialogResult::Custom("Save Anyway".into())
    }

    fn save_all(&mut self) {
        let mut failed = Vec::new();
        let mut saved = 0;
        for i in 0..self.docs.len() {
            if self.docs[i].is_dirty() && self.docs[i].path.is_some() {
                if self.save(i, false) {
                    saved += 1;
                } else {
                    failed.push(self.docs[i].name());
                }
            }
        }
        self.status = if failed.is_empty() {
            format!("Saved {saved} file{}", if saved == 1 { "" } else { "s" })
        } else {
            format!("Not saved: {}", failed.join(", "))
        };
    }

    // ---- settings --------------------------------------------------------

    fn open_settings(&mut self) {
        let Some(dir) = self.config.clone() else { return };
        let path = Settings::path(&dir);
        if !path.exists()
            && let Err(e) = self.settings.save(&dir) {
                log::warn!("couldn't write settings: {e}");
                self.status = format!("Could not write settings: {e}");
                return;
            }
        self.open_path(path, false);
    }

    /// Re-read settings.json. A file with errors keeps the current settings.
    fn reload_settings(&mut self) {
        let Some(dir) = self.config.clone() else { return };
        self.settings_mtime = std::fs::metadata(Settings::path(&dir)).and_then(|m| m.modified()).ok();
        let text = std::fs::read_to_string(Settings::path(&dir)).unwrap_or_default();
        match Settings::parse(if text.trim().is_empty() { "{}" } else { &text }) {
            Ok(settings) => {
                let indent_changed = (settings.tab_size, settings.translate_tabs_to_spaces) != (self.settings.tab_size, self.settings.translate_tabs_to_spaces);
                self.settings = settings;
                self.status = "Settings reloaded".into();
                self.apply_settings();
                if indent_changed {
                    self.apply_indentation();
                }
            }
            Err(e) => {
                log::warn!("{e} (keeping current settings)");
                self.status = format!("{e} (keeping current settings)");
            }
        }
    }

    /// Theme changes; each file keeps its own indentation.
    fn apply_settings(&mut self) {
        let theme = self.active_theme();
        for d in &mut self.docs {
            d.highlighter.set_theme(&theme);
        }
    }

    /// The default indentation, pushed to every open file (status bar, settings file).
    fn apply_indentation(&mut self) {
        for d in &mut self.docs {
            d.editor.tab_width = self.settings.tab_size;
            d.editor.use_spaces = self.settings.translate_tabs_to_spaces;
        }
    }

    /// settings.json edited outside Strata: pick it up.
    fn watch_settings(&mut self) {
        let Some(dir) = &self.config else { return };
        let mtime = std::fs::metadata(Settings::path(dir)).and_then(|m| m.modified()).ok();
        if mtime.is_some() && mtime != self.settings_mtime {
            self.reload_settings();
        }
    }

    fn persist_settings(&mut self) {
        if let Some(dir) = &self.config {
            match self.settings.save(dir) {
                Ok(()) => self.settings_mtime = std::fs::metadata(Settings::path(dir)).and_then(|m| m.modified()).ok(),
                Err(e) => {
                    log::warn!("couldn't write settings: {e}");
                    self.status = format!("Could not write settings: {e}");
                }
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
        let session = match Session::load(&dir) {
            Ok(Some(s)) => s,
            Ok(None) => return,
            Err(e) => return self.set_aside_backups(&dir, &e),
        };
        self.recent_files = session.recent_files.clone();
        self.recent_projects = session.recent_projects.clone();
        self.recent_folders = session.recent_folders.clone();
        // This is the first window; `Windows` takes the others from here.
        let mut unreadable = session.window_states().first().map(|w| self.restore_window(&dir, w)).unwrap_or_default();
        unreadable.extend(self.restore_other_windows(&dir, &session));
        if !unreadable.is_empty() {
            self.set_aside_backups(&dir, &unreadable.join("; "));
        }
    }

    /// Reopen one window's tabs (unsaved text from `backups/`), panes and
    /// folder. Returns the backups that couldn't be read.
    fn restore_window(&mut self, dir: &std::path::Path, session: &WindowState) -> Vec<String> {
        if let Some(layout) = session.layout.clone() {
            self.layout = layout.sanitized();
        }
        let backups = Session::backups(dir);
        let mut unreadable = Vec::new();
        // The tab each saved tab became (views of one buffer name an earlier one).
        let mut restored: Vec<Option<usize>> = Vec::new();
        for tab in &session.tabs {
            let source = tab.clone_of.and_then(|k| restored.get(k).copied().flatten());
            let doc = match (&tab.backup, &tab.path) {
                _ if source.is_some() => source.map(|k| self.new_view_of(k)),
                (Some(name), path) => match std::fs::read(backups.join(name)) {
                    Ok(bytes) => {
                        let mut d = Document::from_buffer(path.clone(), Buffer::from_bytes(&bytes));
                        d.restored_dirty = true;
                        d.encoding = tab.encoding.as_deref().and_then(crate::encoding::Encoding::from_id).unwrap_or_default();
                        d.disk_mtime = tab.disk_mtime;
                        // Edited while Strata was closed (a git pull, another editor)?
                        d.conflict = tab.conflict || path.as_deref().and_then(crate::document::mtime).is_some_and(|now| Some(now) != tab.disk_mtime);
                        d.deleted = path.as_ref().is_some_and(|p| !p.exists());
                        Some(d)
                    }
                    Err(e) => {
                        unreadable.push(format!("{name}: {e}"));
                        None
                    }
                },
                (None, Some(path)) if path.exists() => Document::open_with(path.clone(), self.large_file_bytes()).ok(),
                _ => None,
            };
            let Some(mut doc) = doc else {
                restored.push(None);
                continue;
            };
            doc.wrap = tab.wrap;
            if let Some(s) = &tab.syntax {
                doc.highlighter.set_syntax(s);
            }
            if tab.selections.is_empty() {
                // A session from before every selection was kept.
                let at = tab.cursor.min(doc.editor.buffer.len_chars());
                doc.editor.select_ranges(&[at..at], 0);
            } else {
                doc.editor.set_selection_pairs(&tab.selections, tab.primary);
            }
            doc.view.reveal_centered();
            self.focus_group = tab.group.min(self.layout.groups() - 1);
            let n = self.docs.len();
            self.push_doc(doc);
            // Keep the saved order rather than "after the pane's current tab".
            let d = self.docs.remove(self.active);
            self.docs.push(d);
            self.docs[n].stamp = tab.stamp;
            restored.push(Some(n));
        }
        self.set_find_options(&session.find);
        self.next_stamp = self.docs.iter().map(|d| d.stamp).max().unwrap_or(0) + 1;
        self.activate(session.active.min(self.docs.len().saturating_sub(1)));
        // Every folder (`folders`); a session from before several folders has just `folder`.
        let folders = if session.folders.is_empty() { session.folder.iter().cloned().collect() } else { session.folders.clone() };
        for (k, folder) in folders.into_iter().filter(|f| f.is_dir()).enumerate() {
            if k == 0 {
                self.sidebar.open_folder(folder);
            } else {
                self.sidebar.add_folder(folder);
            }
            self.sidebar.visible = session.sidebar_visible;
        }
        unreadable
    }

    /// The session couldn't be fully restored: keep every backup out of
    /// reach of pruning and say where it went.
    fn set_aside_backups(&mut self, dir: &std::path::Path, why: &str) {
        self.status = match Session::quarantine_backups(dir) {
            Ok(to) => format!("Couldn't restore the last session ({why}). Unsaved text was kept in {}", to.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => format!("Couldn't restore the last session ({why})"),
            Err(e) => {
                // Can't move them, so don't risk pruning them either.
                self.session_lock = None;
                self.link.session_off = true;
                format!("Couldn't restore the last session ({why}) or set its backups aside ({e}); session saving is off")
            }
        };
        log::error!("{}", self.status);
    }

    /// Changes whenever saving the session would write something different.
    fn session_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.active, &self.sidebar.roots, self.sidebar.visible, self.layout.cols.len(), self.layout.rows.len()).hash(&mut h);
        (&self.recent_files, &self.recent_folders, self.find_options()).hash(&mut h);
        for d in &self.docs {
            (&d.path, d.is_dirty(), d.editor.version(), d.group, d.stamp).hash(&mut h);
            for s in d.editor.selections() {
                (s.anchor, s.head).hash(&mut h);
            }
        }
        h.finish()
    }

    /// Write the session now (quitting): waits for an autosave in progress.
    fn save_session(&mut self) -> bool {
        let Some((dir, session, backups)) = self.session_to_save() else { return false };
        autosave::wait();
        match session.save(&dir, &backups) {
            Ok(()) => {
                self.session_key = self.session_fingerprint();
                true
            }
            Err(e) => {
                log::error!("couldn't save the session: {e}");
                self.status = format!("Could not save session: {e}");
                false
            }
        }
    }

    /// This window's part of the session; unsaved text is added to `backups`
    /// (names unique across every window saved with it).
    fn window_state(&self, backups: &mut autosave::Backups) -> WindowState {
        let mut tabs = Vec::new();
        // Saved tab of each doc, for views of one buffer (see `clones`).
        let mut saved: Vec<Option<usize>> = Vec::new();
        for (i, d) in self.docs.iter().enumerate() {
            let clone_of = match self.session_view_of(i) {
                Some(None) => {
                    saved.push(None);
                    continue;
                }
                Some(Some(k)) => saved[k],
                None => None,
            };
            saved.push((!d.is_blank()).then_some(tabs.len()));
            if d.is_blank() {
                continue;
            }
            let backup = (d.is_dirty() && clone_of.is_none()).then(|| {
                let name = Session::backup_name_for(d.editor.text_id());
                backups.push((name.clone(), d.editor.buffer.snapshot()));
                name
            });
            tabs.push(TabState {
                path: d.path.clone(),
                backup,
                cursor: d.editor.primary().head,
                selections: d.editor.selections().iter().map(|s| (s.anchor, s.head)).collect(),
                primary: d.editor.selections().iter().position(|s| *s == d.editor.primary()).unwrap_or(0),
                syntax: Some(d.highlighter.syntax_name().to_owned()),
                disk_mtime: d.disk_mtime,
                conflict: d.conflict,
                group: d.group,
                wrap: d.wrap,
                encoding: Some(d.encoding.id().to_owned()),
                stamp: d.stamp,
                clone_of,
            });
        }
        WindowState {
            folder: self.sidebar.roots.first().cloned(),
            folders: self.sidebar.roots.clone(),
            sidebar_visible: self.sidebar.visible,
            active: self.active.min(tabs.len().saturating_sub(1)),
            tabs,
            layout: Some(self.layout.clone()),
            position: None,
            size: None,
            find: self.find_options(),
        }
    }

    // ---- commands --------------------------------------------------------

    fn run(&mut self, ctx: &egui::Context, cmd: Cmd) {
        if self.help_takes(cmd) {
            return;
        }
        match cmd {
            Cmd::NewTab => self.push_doc(Document::untitled()),
            Cmd::Open => {
                let dir = self.docs[self.active].path.as_ref().and_then(|p| p.parent()).map(|p| p.to_owned());
                let mut dialog = rfd::FileDialog::new();
                if let Some(dir) = dir.or_else(|| self.sidebar.root().map(Path::to_owned)) {
                    dialog = dialog.set_directory(dir);
                }
                for p in dialog.pick_files().unwrap_or_default() {
                    self.open_path(p, false);
                }
            }
            Cmd::OpenFolder => {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.open_folder(dir);
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
            Cmd::NextTab | Cmd::PrevTab => {
                let tabs = self.group_tabs(self.docs[self.active].group);
                let k = tabs.iter().position(|&i| i == self.active).unwrap_or(0);
                let n = tabs.len();
                self.activate(tabs[if cmd == Cmd::NextTab { (k + 1) % n } else { (k + n - 1) % n }]);
            }
            Cmd::LayoutSingle => self.set_layout(Layout::grid(1, 1)),
            Cmd::LayoutColumns2 => self.set_layout(Layout::grid(2, 1)),
            Cmd::LayoutColumns3 => self.set_layout(Layout::grid(3, 1)),
            Cmd::LayoutColumns4 => self.set_layout(Layout::grid(4, 1)),
            Cmd::LayoutRows2 => self.set_layout(Layout::grid(1, 2)),
            Cmd::LayoutGrid => self.set_layout(Layout::grid(2, 2)),
            Cmd::SplitRight => {
                let g = self.docs[self.active].group;
                if g + 1 >= self.layout.groups() {
                    self.set_layout(Layout::grid(self.layout.cols.len() + 1, self.layout.rows.len()));
                }
                self.move_to_pane((g + 1).min(self.layout.groups() - 1));
            }
            Cmd::FocusGroup(g) => self.focus_pane(g as usize),
            Cmd::Build => self.run_build(),
            Cmd::BuildWith => {
                self.reload_build_systems();
                let bctx = self.build_context();
                self.build_choices = self.build.choices(&bctx);
                let labels = self.build_choices.iter().map(|c| c.label.clone()).collect();
                self.choose(ctx, more::Choosing::Build, "Build with…", labels);
            }
            Cmd::ReplaceNext => self.replace_one(),
            Cmd::ToggleFindRegex | Cmd::ToggleFindCase | Cmd::ToggleFindWholeWord | Cmd::ToggleFindInSelection | Cmd::ToggleFindWrap | Cmd::ToggleFindPreserveCase | Cmd::ToggleFindHighlight => self.toggle_find_option(cmd),
            Cmd::ToggleSpellCheck | Cmd::NextMisspelling | Cmd::PrevMisspelling => self.run_spelling(cmd),
            Cmd::NextFindResult => self.step_find_result(true),
            Cmd::PrevFindResult => self.step_find_result(false),
            Cmd::CancelFindInFiles => self.sidebar.search.stop(),
            Cmd::ReplaceInFiles => {
                let prefill = self.selection_for_find_in_files();
                self.sidebar.start_replace_in_files(prefill);
            }
            Cmd::ToggleOpenFiles => self.sidebar.show_open_files = !self.sidebar.show_open_files,
            Cmd::LayoutRows3 => self.set_layout(Layout::grid(1, 3)),
            Cmd::NewGroup => self.new_group(),
            Cmd::CloseGroup => self.close_group(),
            Cmd::MoveToNewGroup => {
                self.new_group();
                self.move_to_pane(self.layout.groups() - 1);
            }
            Cmd::FocusNextGroup => self.cycle_group(true, false),
            Cmd::FocusPrevGroup => self.cycle_group(false, false),
            Cmd::MoveToNextGroup => self.cycle_group(true, true),
            Cmd::MoveToPrevGroup => self.cycle_group(false, true),
            Cmd::NextInStack => self.stack_step(ctx.input(|i| i.time), true),
            Cmd::PrevInStack => self.stack_step(ctx.input(|i| i.time), false),
            Cmd::SwitchHeader => self.switch_header(),
            Cmd::GotoReference => self.goto_reference(),
            Cmd::GotoSymbolInProject => self.goto_symbol_in_project(ctx),
            Cmd::CopyAsHtml => self.copy_as_html(),
            Cmd::ToggleMarkdownPreview => {
                self.settings.markdown_preview = !self.settings.markdown_preview;
                self.persist_settings();
            }
            Cmd::ExportMarkdownHtml => self.export_markdown_html(),
            Cmd::CloneFile => self.clone_file(),
            Cmd::CompareWith => self.compare_with(ctx, self.active),
            Cmd::CompareWithHead => self.compare_with_head(self.active),
            Cmd::NextDifference => self.step_difference(true),
            Cmd::PrevDifference => self.step_difference(false),
            Cmd::CompareSwap => self.compare_toggle(true),
            Cmd::CompareIgnoreWhitespace => self.compare_toggle(false),
            Cmd::Print => self.open_print(printing::Output::Viewer),
            Cmd::ExportPdf => self.open_print(printing::Output::Pdf),
            Cmd::ExportHtml => self.open_print(printing::Output::Html),
            Cmd::PasteFromHistory => self.paste_from_history(ctx),
            Cmd::NewBuildSystem => self.new_build_system(),
            Cmd::ToggleSaveAllOnBuild => {
                self.settings.save_all_on_build = !self.settings.save_all_on_build;
                self.persist_settings();
            }
            Cmd::RecordMacro => self.toggle_macro_recording(),
            Cmd::PlaybackMacro => self.playback_macro(),
            Cmd::SwitchProject => self.switch_project(ctx),
            Cmd::CloseProject => self.close_project(),
            Cmd::EditProject => self.edit_project(),
            Cmd::Licenses => self.licenses_open = true,
            Cmd::SelectEditorFont => self.pick_font(ctx, true),
            Cmd::SelectUiFont => self.pick_font(ctx, false),
            Cmd::NextModification | Cmd::PrevModification => {
                let d = &self.docs[self.active];
                let line = d.editor.buffer.line_of(d.editor.primary().head);
                match crate::git::next_change(&d.git_changes, line, cmd == Cmd::NextModification) {
                    Some(l) => {
                        self.push_jump();
                        self.goto_line(l);
                    }
                    None => self.status = "No changes since the last commit".into(),
                }
            }
            Cmd::UiLarger | Cmd::UiSmaller => {
                let step = if cmd == Cmd::UiLarger { 1.0 } else { -1.0 };
                self.settings.ui_font_size = (self.settings.ui_font_size + step).clamp(9.0, 24.0);
                self.persist_settings();
            }
            Cmd::CancelBuild => self.build.cancel(),
            Cmd::NextResult | Cmd::PrevResult => {
                let r = if cmd == Cmd::NextResult { self.build.next_result() } else { self.build.prev_result() };
                if let Some(crate::build::BuildAction::Goto(p, line, col)) = r {
                    self.open_at(p, line, col..col);
                }
            }
            Cmd::ToggleBuildResults => self.build.visible = !self.build.visible,
            Cmd::ToggleTerminal => self.toggle_terminal(ctx),
            Cmd::NewTerminal => self.new_terminal(ctx),
            Cmd::Snippets => self.show_snippets(ctx),
            Cmd::NewSnippet => self.new_snippet(),
            Cmd::ShowCompletions => {
                self.completion.open(&mut self.docs[self.active], true, &self.snippets.lib);
                self.snippet_from_completion();
            }
            Cmd::CloseWindow => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Cmd::NewWindow => self.link.requests.new_window = true,
            Cmd::RevertFile => self.revert(),
            Cmd::ClearRecent => self.clear_recent(),
            Cmd::ToggleStatusBar => {
                self.settings.show_status_bar = !self.settings.show_status_bar;
                self.persist_settings();
            }
            Cmd::ToggleTabs => {
                self.settings.show_tabs = !self.settings.show_tabs;
                self.persist_settings();
            }
            Cmd::DistractionFree => self.toggle_distraction_free(ctx),
            Cmd::IncrementalFind => self.open_find(false),
            Cmd::UseSelectionForFind | Cmd::UseSelectionForReplace => {
                let d = &self.docs[self.active];
                let text = d.editor.buffer.slice(d.editor.primary().range());
                if cmd == Cmd::UseSelectionForFind {
                    self.find.query.text = if self.find.query.regex { regex::escape(&text) } else { text };
                } else {
                    self.find.replacement = text;
                }
            }
            Cmd::QuickFind => {
                // The next occurrence of the selection becomes the (single) selection.
                let d = &self.docs[self.active];
                let text = d.editor.buffer.slice(d.editor.primary().range());
                if !text.is_empty() {
                    self.find.query = Query::literal(&text, false);
                    self.find_step(true);
                }
            }
            Cmd::JumpBack => self.jump(true),
            Cmd::JumpForward => self.jump(false),
            Cmd::GotoDefinition => self.goto_definition_in(Some(ctx.clone())),
            Cmd::OpenProject => self.open_project(),
            Cmd::SaveProjectAs => self.save_project_as(),
            Cmd::AddFolder => {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.add_folder(dir);
                }
            }
            Cmd::CloseFolder => self.sidebar.close_folders(),
            Cmd::RefreshFolders => self.sidebar.refresh(),
            Cmd::About => self.about_open = true,
            Cmd::OpenLogFolder | Cmd::ShowLatestLog | Cmd::ReportProblem => self.diagnostics_command(cmd),
            Cmd::InstallShellCommand => self.install_shell_command(),
            Cmd::Documentation => self.open_help(None, false),
            Cmd::KeyboardShortcuts => self.open_help(Some(crate::help::SHORTCUTS), false),
            Cmd::SearchDocumentation => self.open_help(None, true),
            Cmd::GettingStarted => self.open_help(Some(crate::help::WELCOME), false),
            Cmd::ToggleWordWrap => {
                let on = self.wraps(self.active);
                self.docs[self.active].wrap = Some(!on);
                if !self.docs[self.active].can_wrap() {
                    self.status = "Word wrap is off for files with a line longer than 200,000 characters".into();
                }
            }
            Cmd::MoveToGroup(g) => self.move_to_pane(g as usize),
            Cmd::ToggleSidebar => {
                // Same test as the menu's check mark: is it on screen?
                self.sidebar.visible = !self.sidebar.shown();
                if self.sidebar.visible && !self.sidebar.shown() {
                    self.status = "The side bar shows once a folder is open (File ▸ Open Folder…)".into();
                }
            }
            Cmd::Find => self.open_find(false),
            Cmd::Replace => self.open_find(true),
            Cmd::FindNext => self.find_step(true),
            Cmd::FindPrev => self.find_step(false),
            Cmd::FindInFiles => {
                let prefill = self.selection_for_find_in_files();
                self.sidebar.start_find_in_files(prefill);
                self.cancel_editor_focus();
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
                // Leaving full screen also leaves distraction-free mode, which needs it.
                if !self.fullscreen && let Some(sidebar) = self.distraction.take() {
                    self.sidebar.visible = sidebar;
                }
            }
            Cmd::Settings => self.open_settings(),
            Cmd::KeyBindings => self.open_key_bindings(),
            Cmd::CommandPalette => self.open_palette(ctx, palette::Mode::Commands, ""),
            Cmd::GotoAnything => self.open_palette(ctx, palette::Mode::Files, ""),
            Cmd::GotoLine => self.open_palette(ctx, palette::Mode::Files, ":"),
            Cmd::GotoSymbol => self.open_palette(ctx, palette::Mode::Files, "@"),
            Cmd::SelectTheme => self.open_palette(ctx, palette::Mode::Theme, ""),
            Cmd::SelectSyntax => self.open_palette(ctx, palette::Mode::Syntax, ""),
        }
    }

    /// A dialog that takes the keyboard is up (Rename, New File, Replace in
    /// Files' question, Print), or a menu: window and editor commands wait,
    /// and the dialog alone sees Esc and Enter.
    fn modal_open(&self, ctx: &egui::Context) -> bool {
        self.sidebar.modal_open() || self.print.is_open() || egui::Popup::is_any_open(ctx)
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.modal_open(ctx) {
            self.keymap.modal_frame(ctx);
            return;
        }
        // The help window's own keys (Esc, back/forward) come first.
        self.help_keys(ctx);
        // Window and editor commands, from the keymap (see `keymap_ui`).
        self.dispatch_keys(ctx);
        if self.terminal_has_keys(ctx) {
            return;
        }
        if self.find.open && !self.palette.open && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.find.open = false;
            self.doc().view.focus();
        }
    }

    /// Pick up edits to user build systems; report broken ones.
    fn reload_build_systems(&mut self) {
        let errors = self.build.reload_systems();
        if !errors.is_empty() {
            self.status = format!("Build systems: {}", errors.join("; "));
        }
    }

    fn build_context(&self) -> crate::build::BuildContext {
        let d = &self.docs[self.active];
        // The sidebar folder holding the file, else the first one.
        let folder = d.path.as_deref().and_then(|p| self.sidebar.root_of(p)).or(self.sidebar.root()).map(Path::to_owned);
        crate::build::BuildContext { file: d.path.clone(), folder, syntax: Some(d.highlighter.syntax_name().to_owned()) }
    }

    /// Another widget (find bar, palette, a dialog) asks for the keyboard: an
    /// editor focus still queued (say, from the palette closing) mustn't take it back.
    fn cancel_editor_focus(&mut self) {
        for d in &mut self.docs {
            d.view.cancel_focus();
        }
    }

    fn open_palette(&mut self, ctx: &egui::Context, mode: palette::Mode, prefix: &str) {
        self.cancel_editor_focus();
        let d = &mut self.docs[self.active];
        let sels = d.editor.selections().iter().map(|s| s.range()).collect();
        let primary = d.editor.selections().iter().position(|s| *s == d.editor.primary()).unwrap_or(0);
        self.palette_restore = Some((self.active, sels, primary));
        let pctx = self.palette_context();
        self.palette.show_mode(mode, prefix, pctx, ctx);
    }

    fn palette_context(&mut self) -> palette::Context {
        let d = &mut self.docs[self.active];
        palette::Context {
            // Only `@` and `#` read it; a rope clone costs nothing up front.
            text: palette::DocText::new(d.editor.buffer.snapshot()),
            syntax: d.highlighter.syntax_name().to_owned(),
            lines: d.editor.buffer.len_lines(),
            roots: self.sidebar.roots.clone(),
            theme: self.active_theme(),
            open: self.docs.iter().filter_map(|d| d.path.clone()).collect(),
        }
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
                Event::Open(p) => {
                    self.push_jump();
                    self.open_path(p, false);
                }
                Event::GotoLine { line, preview } => {
                    if !preview {
                        self.palette_jump();
                    }
                    self.goto_line(line);
                    if preview {
                        continue;
                    }
                }
                Event::Select { line, cols, preview } => {
                    if !preview {
                        self.palette_jump();
                    }
                    select_on_line(self.doc(), line, cols);
                    if preview {
                        continue;
                    }
                }
                Event::GotoSymbolNamed(q) => self.goto_symbol_named(ctx, &q),
                Event::FindInFile(q) => self.find_in_file(&q),
                Event::Theme { name, preview } => {
                    for d in &mut self.docs {
                        d.highlighter.set_theme(&name);
                    }
                    if preview {
                        continue;
                    }
                    self.pick_theme(name);
                }
                Event::Syntax(s) => self.doc().highlighter.set_syntax(&s),
                Event::Chose(i) => self.chose(i),
                Event::Cancel => {
                    self.apply_settings();
                    if let Some((i, sels, primary)) = self.palette_restore.take()
                        && let Some(d) = self.docs.get_mut(i) {
                            d.editor.select_ranges(&sels, primary);
                            d.view.reveal_centered();
                        }
                }
            }
            if !self.palette.open {
                self.palette_restore = None;
                self.doc().view.focus();
            }
        }
    }

    /// Jump history records where you were before the panel opened.
    fn palette_jump(&mut self) {
        if let Some((i, sels, _)) = &self.palette_restore
            && let Some(d) = self.docs.get(*i)
        {
            self.jumps_back.push((d.view.id, sels.first().map_or(0, |r| r.start)));
            self.jumps_forward.clear();
        }
    }

    fn run_editor(&mut self, id: &str) {
        use textutil::*;
        let tool: Option<TextTool> = match id {
            "tx_json_format" => Some(json_format),
            "tx_json_minify" => Some(json_minify),
            "tx_json_sort" => Some(json_sort_keys),
            "tx_b64_encode" => Some(base64_encode),
            "tx_b64_decode" => Some(base64_decode),
            "tx_url_encode" => Some(url_encode),
            "tx_url_decode" => Some(url_decode),
            "tx_html_escape" => Some(html_escape),
            "tx_html_unescape" => Some(html_unescape),
            _ => None,
        };
        if let Some(f) = tool {
            return self.transform(f);
        }
        match id {
            "tx_uuid" => return self.insert_at_cursors(&uuid_v4()),
            "tx_date" => return self.insert_at_cursors(&now_string()),
            _ => {}
        }
        let d = &mut self.docs[self.active];
        let e = &mut d.editor;
        match id {
            "toggle_comment" => match crate::lang::comment_tokens(d.highlighter.syntax_name()) {
                (Some(token), _) => e.toggle_comment(token),
                (None, Some((open, close))) => e.toggle_block_comment(open, close),
                _ => self.status = format!("No comment syntax for {}", d.highlighter.syntax_name()),
            },
            "indent" => e.indent(),
            "outdent" => e.outdent(),
            "duplicate" => e.duplicate_lines(),
            "delete_line" => e.delete_lines(),
            "move_up" => e.move_lines(true),
            "move_down" => e.move_lines(false),
            "join" => e.join_lines(),
            "line_after" => e.insert_line_after(),
            "line_before" => e.insert_line_before(),
            "upper" => e.swap_case_upper(),
            "lower" => e.swap_case_lower(),
            "sort" => e.sort_lines(false),
            "sort_case" => e.sort_lines(true),
            "reverse" => e.reverse_lines(),
            "unique" => e.unique_lines(),
            "trim" => e.trim_trailing_whitespace(),
            "expand_line" => e.expand_to_line(),
            "split_lines" => e.split_into_lines(),
            "cursor_above" => e.add_cursor_vertical(-1),
            "cursor_below" => e.add_cursor_vertical(1),
            "next_occurrence" => e.select_next_occurrence(),
            "all_occurrences" => e.select_all_occurrences(),
            "expand_brackets" => e.expand_to_brackets(),
            "jump_bracket" => e.jump_to_bracket(),
            "select_all" => e.select_all(),
            "single_selection" => e.collapse_to_primary(),
            "toggle_block_comment" => match crate::lang::comment_tokens(d.highlighter.syntax_name()) {
                (_, Some((open, close))) => e.toggle_block_comment(open, close),
                _ => self.status = format!("No block comment syntax for {}", d.highlighter.syntax_name()),
            },
            "delete_word_forward" => e.delete_word_forward(),
            "delete_word_backward" => e.delete_word_backward(),
            "lower_camel" => e.lower_camel_case(),
            "upper_camel" => e.upper_camel_case(),
            "snake" => e.snake_case(),
            "kebab" => e.kebab_case(),
            "sel_sort_ci" => e.sort_selections_case_insensitive(),
            "expand_selection" => e.expand_selection(),
            "expand_line_up" => e.expand_to_line_upward(),
            "expand_block" => e.expand_to_block(),
            "set_mark" => e.set_mark(),
            "select_to_mark" => e.select_to_mark(),
            "delete_to_mark" => {
                let cut = e.delete_to_mark();
                if !cut.is_empty() {
                    self.yank = cut;
                }
            }
            "swap_mark" => e.swap_with_mark(),
            "clear_mark" => e.clear_mark(),
            "yank" => {
                let text = self.yank.clone();
                e.insert_text(&text);
            }
            "soft_undo" => e.soft_undo(),
            "soft_redo" => e.soft_redo(),
            "paste_and_indent" => {
                let text = self.keymap.pasted.take().map_or_else(|| arboard::Clipboard::new().and_then(|mut c| c.get_text()).ok(), Some);
                if let Some(text) = text {
                    e.paste_and_indent(&text);
                }
            }
            "delete_to_end" => e.delete_to_line_end(),
            "delete_to_start" => e.delete_to_line_start(),
            "transpose" => e.transpose(),
            "title_case" => e.title_case(),
            "swap_case" => e.swap_case(),
            "shuffle" => e.shuffle_lines(),
            "sel_sort" => e.sort_selections(),
            "sel_reverse" => e.reverse_selections(),
            "sel_unique" => e.unique_selections(),
            "sel_shuffle" => e.shuffle_selections(),
            "invert_selection" => e.invert_selection(),
            "expand_word" => e.expand_to_word(),
            "expand_paragraph" => e.expand_to_paragraph(),
            "expand_indentation" => e.expand_to_indentation(),
            "wrap_paragraph" => e.wrap_paragraph(wrap_paragraph_width(&self.settings)),
            "reindent" => e.reindent(),
            "to_spaces" => e.convert_indentation(true),
            "to_tabs" => e.convert_indentation(false),
            "guess_indent" => match e.guess_indentation() {
                Some((spaces, width)) => {
                    e.use_spaces = spaces;
                    e.tab_width = width;
                    self.status = format!("Indentation: {} {width}", if spaces { "spaces," } else { "tabs, width" });
                }
                None => self.status = "No indented lines to guess from".into(),
            },
            "bookmark_toggle" => e.toggle_bookmark(),
            "bookmark_next" => e.next_bookmark(),
            "bookmark_prev" => e.prev_bookmark(),
            "bookmark_clear" => e.clear_bookmarks(),
            "bookmark_select" => e.select_all_bookmarks(),
            "undo" => e.undo(),
            "redo" => e.redo(),
            _ => return,
        }
        d.view.reveal();
    }

    /// Bring `menus_cache` up to date if anything may have changed what the
    /// menus show: this frame's input (keys, clicks, typing), a menu action,
    /// the key mode (a chord, the terminal), or a second passing (builds,
    /// git and the like change state in the background). Building them is
    /// too much work for every frame. True if rebuilt.
    fn fresh_menus(&mut self, ctx: &egui::Context) -> bool {
        // macOS menus take their shortcut keys before Strata sees them: while
        // a chord waits for its second key, or the terminal has the keyboard,
        // they go without (mode 1, 2). In-window menus never take keys.
        let mode = if !cfg!(target_os = "macos") {
            0
        } else if self.keymap.chord_pending() {
            1
        } else if self.terminal_has_keys(ctx) {
            2
        } else {
            0
        };
        let now = ctx.input(|i| i.time);
        let input = ctx.input(|i| i.events.iter().any(|e| !matches!(e, egui::Event::PointerMoved(_) | egui::Event::MouseMoved(_) | egui::Event::PointerGone)));
        let fresh = !self.menus_stale && !input && self.menus_cache.as_ref().is_some_and(|(m, t, _)| *m == mode && now - t < 1.0 && now >= *t);
        if fresh {
            return false;
        }
        let mut menus = self.menus();
        match mode {
            1 => menus.iter_mut().for_each(|(_, items)| crate::menus::strip_keys(items)),
            #[cfg(target_os = "macos")]
            2 => menus.iter_mut().for_each(|(_, items)| crate::terminal::strip_menu_keys(items)),
            _ => {}
        }
        self.menus_cache = Some((mode, now, menus));
        self.menus_stale = false;
        true
    }

    /// Tabs of `from` (a file, or files inside a folder) now at `to` follow it.
    pub(super) fn follow_rename(&mut self, from: &Path, to: &Path) {
        for d in &mut self.docs {
            if let Some(rest) = d.path.as_ref().and_then(|p| p.strip_prefix(from).ok()) {
                let new = if rest.as_os_str().is_empty() { to.to_owned() } else { to.join(rest) };
                d.highlighter.detect(Some(&new), &d.editor.buffer.line_text(0));
                d.path = Some(new);
            }
        }
        self.sidebar.refresh();
    }

    /// `gone` (a file, or a folder) was moved to the Trash: its tabs close,
    /// except those with unsaved changes, which stay marked deleted.
    pub(super) fn follow_delete(&mut self, gone: &Path) {
        let mut i = 0;
        while i < self.docs.len() {
            let inside = self.docs[i].path.as_ref().is_some_and(|p| p.starts_with(gone));
            if inside && !self.docs[i].is_dirty() && self.close_tab(i) {
                continue;
            }
            if inside {
                self.docs[i].check_disk();
            }
            i += 1;
        }
        self.sidebar.refresh();
    }

    /// macOS title bar: content runs under the system title bar, which would be
    /// see-through with the transparent window. Paint a solid strip with the title,
    /// drag to move, double-click to zoom. Hidden in full screen, where macOS hides it too.
    fn title_bar(&mut self, ui: &mut egui::Ui) {
        if !cfg!(target_os = "macos") || ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false)) {
            return;
        }
        let title = self.title();
        crate::fonts::ensure_cjk(ui.ctx(), &title);
        egui::Panel::top("titlebar").exact_size(28.0).frame(egui::Frame::new().fill(ui.visuals().panel_fill)).show(ui, |ui| {
            let rect = ui.max_rect();
            let resp = ui.interact(rect, egui::Id::new("titlebar"), Sense::click_and_drag());
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, title, FontId::proportional(13.0), ui.visuals().text_color().gamma_multiply(0.85));
            if resp.double_clicked() {
                let zoomed = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!zoomed));
            } else if resp.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        });
    }

    fn title(&self) -> String {
        let d = &self.docs[self.active];
        let dot = if d.is_dirty() { " •" } else { "" };
        let more = match self.sidebar.roots.len() {
            0 | 1 => String::new(),
            n => format!(" +{}", n - 1),
        };
        let folder = self.sidebar.root().and_then(|r| r.file_name()).map(|n| format!(" — {}{more}", n.to_string_lossy())).unwrap_or_default();
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
        self.find.replace = replace;
        self.find.focus_query = true;
        self.cancel_editor_focus();
        self.find.origin = p.start();
        self.find_opening();
    }

    fn refresh_matches(&mut self) {
        let scope = self.find_scope();
        let d = &mut self.docs[self.active];
        d.finder.update(&mut d.editor, &self.find.query, scope.as_deref());
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
            return self.open_find(self.find.open && self.find.replace);
        }
        self.refresh_matches();
        let d = &self.docs[self.active];
        let p = d.editor.primary();
        let on_match = d.finder.index_of(&p.range()).is_some();
        let wrap = self.find.wrap;
        let i = if forward {
            d.finder.next(if on_match { p.end() } else { p.start() }, wrap)
        } else {
            d.finder.prev(p.start(), wrap)
        };
        match i {
            Some(i) => self.select_match(i),
            None if !d.finder.matches.is_empty() => {
                self.status = format!("No more matches {} (Wrap is off)", if forward { "below" } else { "above" });
            }
            None => {}
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
            let Ok(r) = crate::replace::Replacer::new(&q, &rep) else { return };
            let text = d.editor.text_snapshot();
            let (byte, end) = (d.editor.buffer.char_to_byte(p.start()), d.editor.buffer.char_to_byte(p.end()));
            let eol = d.editor.buffer.line_ending().as_str();
            // Only the selected match: the text past it is cut off (as In Selection's scope may be).
            if let Some(caps) = r.regex().captures_at(&text[..end], byte).filter(|c| c.get(0).is_some_and(|m| m.range() == (byte..end))) {
                let rep = self.replacement_for(&r, &caps, eol);
                self.doc().editor.replace_ranges(vec![(p.range(), rep)]);
                self.refresh_matches();
            }
        }
        self.find_step(true);
    }

    fn replace_all(&mut self) {
        // The same replacement syntax as Replace in Files (`\1`, `$1`, `${name}`, `\n`, `\t`).
        let Ok(r) = crate::replace::Replacer::new(&self.find.query, &self.find.replacement) else { return };
        let edits = self.replace_all_edits(&r);
        let n = edits.len();
        let d = self.doc();
        d.editor.replace_ranges(edits);
        d.view.reveal();
        let within = if self.find.in_selection { " in the selection" } else { "" };
        self.status = format!("Replaced {n} occurrence{}{within}", if n == 1 { "" } else { "s" });
    }

    fn find_bar(&mut self, ui: &mut egui::Ui) {
        crate::fonts::ensure_cjk(ui.ctx(), &self.find.query.text);
        crate::fonts::ensure_cjk(ui.ctx(), &self.find.replacement);
        self.refresh_matches();
        // Typing in the query jumps to the first match after where search began.
        if self.find.query != self.find.last_query {
            self.find.last_query = self.find.query.clone();
            if let Some(i) = self.docs[self.active].finder.next(self.find.origin, self.find.wrap) {
                self.select_match(i);
            }
        }

        let enter = ui.input(|i| i.key_pressed(Key::Enter));
        let mods = ui.input(|i| i.modifiers);
        let label = self.match_label();
        let mut action = None;
        let mut toggled = None;
        // The fields give up width so the buttons always fit: the toggles' and
        // buttons' widths are last frame's.
        let widths = egui::Id::new("find-bar-widths");
        let (toggles_w, buttons_w) = ui.ctx().data(|d| d.get_temp::<(f32, f32)>(widths)).unwrap_or((190.0, 300.0));
        let gap = ui.spacing().item_spacing.x;
        let field_w = (ui.available_width() - toggles_w - buttons_w - 4.0 * gap).clamp(80.0, 320.0);
        let label_w = if field_w < 320.0 { 64.0 } else { 90.0 };
        let mut measured = (toggles_w, buttons_w);
        // A grid keeps the Find and Replace fields in one column.
        egui::Grid::new("find-grid").num_columns(3).show(ui, |ui| {
            let r = ui.horizontal(|ui| find_options::search_toggles(ui, &self.find));
            toggled = r.inner;
            measured.0 = r.response.rect.width();
            let q = &mut self.find.query;
            let id = egui::Id::new("find-query");
            let field = crate::ctxmenu::text_field(ui, id, &mut q.text, |ui, text| {
                ui.add_sized([field_w, ui.spacing().interact_size.y], egui::TextEdit::singleline(text).id(id).hint_text("Find").font(egui::TextStyle::Monospace))
            });
            if std::mem::take(&mut self.find.focus_query) {
                field.request_focus();
            }
            if field.lost_focus() && enter {
                action = Some(if mods.alt { "all" } else if mods.shift { "prev" } else { "next" });
                field.request_focus();
            }
            let r = ui.horizontal(|ui| {
                ui.add_sized([label_w, 18.0], egui::Label::new(label).truncate());
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
            measured.1 = r.response.rect.width();
            ui.end_row();

            if self.find.replace {
                toggled = toggled.or(find_options::replace_toggles(ui, &self.find));
                let regex = self.find.query.regex;
                let id = egui::Id::new("replace-text");
                let hint = if regex { "Replace (\\1 or $1 for groups, \\n, \\t)" } else { "Replace" };
                let field = crate::ctxmenu::text_field(ui, id, &mut self.find.replacement, |ui, text| {
                    ui.add_sized([field_w, ui.spacing().interact_size.y], egui::TextEdit::singleline(text).id(id).hint_text(hint).font(egui::TextStyle::Monospace))
                });
                if field.lost_focus() && enter {
                    action = Some(if mods.command { "replace-all" } else { "replace" });
                    field.request_focus();
                }
                ui.horizontal(|ui| {
                    ui.add_space(label_w + ui.spacing().item_spacing.x);
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
        if measured != (toggles_w, buttons_w) {
            ui.ctx().data_mut(|d| d.insert_temp(widths, measured));
            ui.ctx().request_repaint();
        }
        if let Some(cmd) = toggled {
            self.toggle_find_option(cmd);
        }
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
            (0, _) if self.find.in_selection && d.editor.find_scope().is_some_and(<[_]>::is_empty) => "No selection".to_owned(),
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

/// Status bar items on the left, most important first.
#[derive(Clone, Copy)]
enum StatusLeft {
    Conflict,
    Deleted,
    Lossy,
    Branch,
    Words,
}

/// Which status bar items fit (see `status_fit`).
struct StatusFit {
    /// "12:5" instead of "Line 12, Column 5".
    short_pos: bool,
    left: [bool; 5],
    /// Syntax, encoding, line endings, indentation, theme.
    right: [bool; 5],
    /// Each right item's width (with its separator) when last drawn.
    right_widths: [f32; 5],
}

/// Choose the status bar items that fit its width, by priority: the cursor
/// position and the syntax always; then the encoding, line endings,
/// indentation and theme menus; then the warnings, the branch and the word
/// count; the message gets what's left (at least a little).
fn status_fit(ui: &egui::Ui, pos: &str, conflict: bool, deleted: bool, lossy: bool, branch: Option<&str>, words: Option<&str>) -> StatusFit {
    let right_widths: [f32; 5] = ui.ctx().data(|d| d.get_temp(egui::Id::new("status-bar-widths"))).unwrap_or([100.0, 70.0, 40.0, 90.0, 70.0]);
    let font = egui::TextStyle::Body.resolve(ui.style());
    let gap = ui.spacing().item_spacing.x;
    let text_w = |s: &str| ui.painter().layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x + gap;
    let sep = 2.0 * gap + 6.0;
    let message_min = 48.0;
    let mut room = ui.available_width() - message_min - sep;
    let (full, short) = (text_w(pos), text_w("0000:000"));
    room -= short;
    let mut right = [false; 5];
    for (k, w) in right_widths.iter().enumerate() {
        // The syntax menu stays even when it doesn't fit.
        if k == 0 || *w <= room {
            right[k] = true;
            room -= w;
        } else {
            break;
        }
    }
    let short_pos = full - short > room;
    if !short_pos {
        room -= full - short;
    }
    let button = 2.0 * ui.spacing().button_padding.x;
    let wanted: [(bool, f32); 5] = [
        (conflict, sep + text_w("Changed on disk") + text_w("Reload") + button),
        (deleted, sep + text_w("Deleted on disk (save to recreate)")),
        (lossy, sep + text_w("Decoding errors")),
        (branch.is_some(), sep + branch.map_or(0.0, |b| text_w(&format!("⎇ {b}")))),
        (words.is_some(), sep + words.map_or(0.0, text_w)),
    ];
    let mut left = [false; 5];
    for (k, (on, w)) in wanted.into_iter().enumerate() {
        if on && w <= room {
            left[k] = true;
            room -= w;
        }
    }
    StatusFit { short_pos, left, right, right_widths }
}

enum TabAction {
    Activate(usize),
    Close(usize),
    /// Double-click on a pane's empty tab bar.
    New(usize),
}

impl StrataApp {
    /// Panels, widgets and menus take their colors from the editor theme.
    fn style_chrome(&mut self, ctx: &egui::Context) {
        let hl = &self.docs[self.active].highlighter;
        let opacity = self.settings.window_opacity.clamp(0.3, 1.0);
        let key = format!("{}@{opacity}", hl.theme_key());
        if self.chrome_theme == key {
            return;
        }
        self.chrome_theme = key;
        let (bg, fg, sel, light) = (hl.background(), hl.foreground(), hl.selection(), hl.is_light());
        let high_contrast = hl.kind().is_high_contrast();
        let mut v = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
        // High contrast keeps the scheme's pure background and full-strength text and borders.
        let panel = if high_contrast { bg } else if light { shade(bg, 0.94) } else { shade(bg, 0.82) };
        // Chrome (tabs, sidebar, status bar, menus) stays solid; only the text areas see through.
        v.panel_fill = panel;
        v.window_fill = mix(panel, bg, 0.5);
        v.extreme_bg_color = if light { shade(bg, 1.02) } else { shade(bg, 0.7) };
        v.faint_bg_color = mix(panel, fg, 0.04);
        v.selection.bg_fill = sel;
        v.override_text_color = Some(if high_contrast { fg } else { mix(fg, panel, 0.12) });
        // Selected rows (sidebar, menus) keep the text color on the selection.
        v.selection.stroke = Stroke::new(1.0, fg);
        v.hyperlink_color = hl.accent();
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, mix(panel, fg, if high_contrast { 0.5 } else { 0.10 }));
        v.window_stroke = Stroke::new(1.0, mix(panel, fg, if high_contrast { 0.6 } else { 0.16 }));
        v.widgets.inactive.weak_bg_fill = mix(panel, fg, 0.06);
        v.widgets.hovered.weak_bg_fill = mix(panel, fg, 0.12);
        v.widgets.active.weak_bg_fill = mix(panel, fg, 0.18);
        // Both of egui's styles, so a system light/dark switch keeps these colors.
        ctx.set_visuals_of(egui::Theme::Dark, v.clone());
        ctx.set_visuals_of(egui::Theme::Light, v);
    }

    /// Hints over an empty window, centred in the editor area (not over the
    /// side bar, terminal or other panels); left out when that area is small.
    fn welcome(&self, ctx: &egui::Context) {
        let Some(rect) = ctx.data(|d| d.get_temp::<Rect>(egui::Id::new(("panes-rect", self.link.id("panes"))))) else { return };
        if rect.width() < 360.0 || rect.height() < 260.0 {
            return;
        }
        // The background layer: above the editor, below windows like About.
        let painter = ctx.layer_painter(egui::LayerId::background()).with_clip_rect(rect);
        let hl = &self.docs[0].highlighter;
        let (fg, accent) = (hl.foreground(), hl.accent());
        let center = rect.center() + vec2(0.0, -60.0);
        painter.text(center, egui::Align2::CENTER_CENTER, "Strata", FontId::proportional(40.0), accent.gamma_multiply(0.85));
        let rows = [
            ("Goto Anything", Cmd::GotoAnything),
            ("Command Palette", Cmd::CommandPalette),
            ("Open Folder", Cmd::OpenFolder),
            ("Open File", Cmd::Open),
            ("New File", Cmd::NewTab),
            ("Documentation", Cmd::Documentation),
        ];
        for (i, (name, cmd)) in rows.into_iter().enumerate() {
            let y = center.y + 54.0 + i as f32 * 26.0;
            painter.text(pos2(center.x - 8.0, y), egui::Align2::RIGHT_CENTER, name, FontId::proportional(14.0), fg.gamma_multiply(0.55));
            // Unbound: say where to find it instead.
            let keys = cmd.shortcut_label().unwrap_or_else(|| if cmd == Cmd::Documentation { "Help menu".into() } else { String::new() });
            painter.text(pos2(center.x + 8.0, y), egui::Align2::LEFT_CENTER, keys, FontId::proportional(14.0), fg.gamma_multiply(0.85));
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

    /// One pane's tab bar. Records each tab's rect for drag and drop.
    fn tab_bar(&mut self, ui: &mut egui::Ui, group: usize, names: &[String], tab_rects: &mut Vec<(usize, Rect)>) -> Option<TabAction> {
        let bg = self.docs[self.active].highlighter.background();
        let fg = ui.visuals().text_color();
        let bar = ui.max_rect();
        ui.painter().rect_filled(bar, 0.0, ui.visuals().panel_fill);
        let shown = self.group_active(group);
        let focused = group == self.focus_group;
        let font = FontId::proportional(self.settings.ui_font_size.clamp(9.0, 24.0));
        let height = 32.0;
        let mut action = None;
        // Tabs that don't fit scroll (the mouse wheel scrolls them sideways);
        // an arrow at the end then lists them all. The current tab is kept in view.
        let memory = egui::Id::new(("tab-strip", group, self.link.id("tabs")));
        let (last_shown, overflowed, last_w) = ui.ctx().data(|d| d.get_temp::<(Option<egui::Id>, bool, f32)>(memory)).unwrap_or_default();
        let shown_id = shown.map(|i| self.docs[i].view.id);
        // A new current tab, or the strip changed width (window resized, arrow shown).
        let reveal = shown_id != last_shown || (bar.width() - last_w).abs() > 0.5;
        let list_w = if overflowed { 26.0 } else { 0.0 };
        let strip = Rect::from_min_max(bar.min, pos2(bar.max.x - list_w, bar.max.y));
        let tabs = self.group_tabs(group);
        let out = ui.scope_builder(egui::UiBuilder::new().max_rect(strip), |ui| {
            ui.style_mut().always_scroll_the_only_direction = true;
            // Not animated: a scroll retargeted mid-animation overshoots.
            egui::ScrollArea::horizontal().id_salt(("tabs", group)).animated(false).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            ui.horizontal(|ui| {
                for &i in &tabs {
                    let name = &names[i];
                    let d = &self.docs[i];
                    let galley = ui.painter().layout_no_wrap(name.clone(), font.clone(), fg);
                    let w = (galley.size().x + 52.0).clamp(90.0, 260.0);
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click_and_drag());
                    tab_rects.push((i, rect));
                    let active = Some(i) == shown;
                    // Only the focused pane's tab gets the accent line.
                    let accented = active && focused;
                    let close = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(16.0, 16.0));
                    let over_close = resp.hover_pos().is_some_and(|p| close.expand(2.0).contains(p));

                    let fill = if active { bg } else if resp.hovered() { mix(ui.visuals().panel_fill, bg, 0.5) } else { ui.visuals().panel_fill };
                    ui.painter().rect_filled(rect, egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, fill);
                    if accented {
                        // Accent line on the active tab, in the theme's accent color.
                        let accent = self.docs[i].highlighter.accent();
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

                    if active && reveal {
                        resp.scroll_to_me(None);
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
                    self.tab_context_menu(&resp, i);
                }
                // Double-click the empty part of the bar for a new tab.
                let rest = ui.allocate_exact_size(vec2(ui.available_width().max(40.0), height), Sense::click()).1;
                if rest.double_clicked() {
                    action = Some(TabAction::New(group));
                } else if rest.clicked() {
                    self.focus_pane(group);
                }
            });
        })
        })
        .inner;
        // The spacer after the tabs takes at least 40 points; past that, they overflow.
        let overflowing = out.content_size.x > out.inner_rect.width() + 41.0 || (overflowed && out.content_size.x > out.inner_rect.width() + 1.0);
        // Remember the strip's width as drawn: with the arrow, it is narrower.
        let width = if overflowing == overflowed { bar.width() } else { -1.0 };
        ui.ctx().data_mut(|d| d.insert_temp(memory, (shown_id, overflowing, width)));
        if overflowing != overflowed {
            ui.ctx().request_repaint();
        }
        if overflowed {
            let at = Rect::from_min_max(pos2(strip.max.x, bar.min.y), bar.max);
            let resp = ui.interact(at, memory.with("list"), Sense::click()).on_hover_text("All tabs in this pane");
            if resp.hovered() {
                ui.painter().rect_filled(at.shrink(3.0), 4.0, mix(ui.visuals().panel_fill, fg, 0.10));
            }
            // A down chevron.
            let c = at.center();
            let stroke = Stroke::new(1.4, fg.gamma_multiply(0.75));
            ui.painter().line_segment([c + vec2(-4.0, -2.0), c + vec2(0.0, 2.0)], stroke);
            ui.painter().line_segment([c + vec2(0.0, 2.0), c + vec2(4.0, -2.0)], stroke);
            let mut chosen = None;
            egui::Popup::menu(&resp).show(|ui| {
                egui::ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
                    for &i in &tabs {
                        let d = &self.docs[i];
                        crate::fonts::ensure_cjk(ui.ctx(), &names[i]);
                        let label = if d.is_dirty() { format!("● {}", names[i]) } else { names[i].clone() };
                        if ui.selectable_label(Some(i) == shown, label).clicked() {
                            chosen = Some(i);
                            ui.close();
                        }
                    }
                });
            });
            if let Some(i) = chosen {
                action = Some(TabAction::Activate(i));
            }
        }
        action
    }

    /// A dragged tab: over another tab it takes that place (in that pane);
    /// released over a pane's editor it moves to the end of that pane.
    fn drag_tab(&mut self, ctx: &egui::Context, tab_rects: &[(usize, Rect)], cells: &[Rect]) {
        let Some(from) = self.dragging_tab.filter(|&f| f < self.docs.len()) else {
            self.dragging_tab = None;
            return;
        };
        let (down, pos) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.hover_pos()));
        if let Some(p) = pos {
            if let Some(&(to, _)) = tab_rects.iter().find(|(_, r)| r.contains(p)) {
                if to != from {
                    let group = self.docs[to].group;
                    let mut d = self.docs.remove(from);
                    d.group = group;
                    self.docs.insert(to, d);
                    self.dragging_tab = Some(to);
                    self.activate(to);
                }
            } else if !down
                && let Some(g) = cells.iter().position(|c| c.contains(p))
                && g != self.docs[from].group
            {
                let mut d = self.docs.remove(from);
                d.group = g;
                self.docs.push(d);
                self.activate(self.docs.len() - 1);
            }
            if down && cells.iter().any(|c| c.contains(p)) {
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            }
        }
        if !down {
            self.dragging_tab = None;
        }
    }

    /// Every pane: its tab bar over its editor (or an empty-pane hint).
    fn panes(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let area = ui.max_rect();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(("panes-rect", self.link.id("panes"))), area));
        let cells = self.layout.cells(area);
        let names = self.display_names();
        for n in &names {
            crate::fonts::ensure_cjk(&ctx, n);
        }
        let mut tab_rects = Vec::new();
        let mut actions = Vec::new();
        let find_open = self.find.open && self.find.highlight;
        let bar_h = if self.settings.show_tabs && self.distraction.is_none() { 32.0 } else { 0.0 };
        for (g, cell) in cells.iter().enumerate() {
            let bar = Rect::from_min_size(cell.min, vec2(cell.width(), bar_h));
            let body = Rect::from_min_max(pos2(cell.left(), cell.top() + bar_h), cell.max);
            if bar_h > 0.0 {
                let action = ui.scope_builder(egui::UiBuilder::new().max_rect(bar).id_salt(("bar", g)), |ui| self.tab_bar(ui, g, &names, &mut tab_rects)).inner;
                actions.extend(action);
            }
            ui.scope_builder(egui::UiBuilder::new().max_rect(body).id_salt(("pane", g)), |ui| match self.group_active(g) {
                Some(i) => {
                    let is_active = i == self.active;
                    let d = &mut self.docs[i];
                    let matches = if find_open && is_active { std::mem::take(&mut d.finder.matches) } else { Vec::new() };
                    let resp = match d.compare.as_mut() {
                        Some(c) => compare_ui::show(c, ui, &mut d.editor, &mut d.highlighter, &mut d.view),
                        None => d.view.show(ui, &mut d.editor, &mut d.highlighter, &matches),
                    };
                    if find_open && is_active {
                        d.finder.matches = matches;
                    }
                    self.editor_context_menu(&resp, i);
                }
                None => {
                    let resp = ui.allocate_rect(body, Sense::click());
                    let hl = &self.docs[self.active].highlighter;
                    ui.painter().rect_filled(body, 0.0, hl.background().gamma_multiply(self.settings.window_opacity.clamp(0.3, 1.0)));
                    let hint = format!("Empty pane · drag a tab here, or {}", Cmd::MoveToGroup(g as u8).shortcut_label().unwrap_or_default());
                    ui.painter().text(body.center(), egui::Align2::CENTER_CENTER, hint, FontId::proportional(13.0), hl.foreground().gamma_multiply(0.4));
                    if resp.clicked() {
                        self.focus_pane(g);
                    }
                }
            });
        }
        // Clicking into a pane's editor makes its tab the current one.
        for g in 0..cells.len() {
            if let Some(i) = self.group_active(g)
                && i != self.active
                && self.docs[i].view.has_focus(&ctx)
            {
                self.select(i);
            }
        }
        if self.layout.groups() > 1 {
            let color = mix(ui.visuals().panel_fill, ui.visuals().text_color(), 0.12);
            self.layout.dividers(ui, area, color);
            // A thin accent marks the focused pane.
            if let Some(cell) = cells.get(self.focus_group) {
                let accent = self.docs[self.active].highlighter.accent().gamma_multiply(0.5);
                ui.painter().hline(cell.x_range(), cell.top() + 0.5, Stroke::new(1.0, accent));
            }
        }
        #[cfg(test)]
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("test-tab-rects"), tab_rects.clone()));
        self.drag_tab(&ctx, &tab_rects, &cells);
        for action in actions {
            match action {
                TabAction::Activate(i) => self.activate(i),
                TabAction::Close(i) => {
                    self.close_tab(i);
                }
                TabAction::New(g) => {
                    self.focus_group = g;
                    self.push_doc(Document::untitled());
                }
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        crate::fonts::ensure_cjk(ui.ctx(), &self.status);
        let words = self.word_count();
        let branch = self.docs[self.active].path.as_deref().and_then(|p| self.git.repo(p)).map(|r| r.branch);
        let d = &self.docs[self.active];
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
        // Narrow windows: items drop out by priority rather than overlap.
        let fit = status_fit(ui, &pos, d.conflict, d.deleted, d.editor.buffer.lossy, branch.as_deref(), words.as_deref());
        ui.horizontal(|ui| {
            let d = &mut self.docs[self.active];
            ui.label(if fit.short_pos { format!("{line}:{col}") } else { pos });
            if d.deleted && fit.left[StatusLeft::Deleted as usize] {
                ui.separator();
                ui.colored_label(Color32::from_rgb(0xe5, 0x9e, 0x40), "Deleted on disk (save to recreate)");
            }
            if d.editor.buffer.lossy && fit.left[StatusLeft::Lossy as usize] {
                ui.separator();
                ui.colored_label(Color32::from_rgb(0xe5, 0x9e, 0x40), "Decoding errors").on_hover_text(format!("Some bytes couldn't be read as {} and show as �. Try File ▸ Reopen with Encoding.", d.encoding.name()));
            }
            if d.conflict && fit.left[StatusLeft::Conflict as usize] {
                ui.separator();
                ui.colored_label(Color32::from_rgb(0xe5, 0x9e, 0x40), "Changed on disk");
                if ui.small_button("Reload").on_hover_text("Discard your edits and load the file from disk").clicked()
                    && let Err(e) = d.reload_from_disk() {
                        self.status = format!("Could not reload: {e}");
                    }
            }
            if let Some(branch) = branch.filter(|_| fit.left[StatusLeft::Branch as usize]) {
                ui.separator();
                ui.label(format!("⎇ {branch}"));
            }
            if let Some(words) = words.filter(|_| fit.left[StatusLeft::Words as usize]) {
                ui.separator();
                ui.label(words);
            }
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut widths = fit.right_widths;
                let edge = |ui: &egui::Ui| ui.min_rect().left();
                let mut x = edge(ui);
                let mut measure = |ui: &egui::Ui, k: usize| {
                    widths[k] = x - edge(ui);
                    x = edge(ui);
                };
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
                measure(ui, 0);
                let d = &mut self.docs[self.active];
                if fit.right[1] {
                    let enc = d.encoding;
                    let mut picked = None;
                    ui.menu_button(enc.name(), |ui| {
                        ui.menu_button("Reopen with Encoding", |ui| {
                            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                                for e in crate::encoding::Encoding::all() {
                                    if ui.selectable_label(*e == enc, e.name()).clicked() {
                                        picked = Some((true, *e));
                                        ui.close();
                                    }
                                }
                            });
                        });
                        ui.menu_button("Save with Encoding", |ui| {
                            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                                for e in crate::encoding::Encoding::all() {
                                    if ui.selectable_label(*e == enc, e.name()).clicked() {
                                        picked = Some((false, *e));
                                        ui.close();
                                    }
                                }
                            });
                        });
                    });
                    if let Some((reopen, e)) = picked {
                        self.pending_encoding = Some((reopen, e));
                    }
                    ui.separator();
                    measure(ui, 1);
                }
                if fit.right[2] {
                    let d = &mut self.docs[self.active];
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
                    measure(ui, 2);
                }
                if fit.right[3] {
                    // Like Sublime, this shows and changes the current file's indentation.
                    let (old_tab, old_spaces) = (self.docs[self.active].editor.tab_width, self.docs[self.active].editor.use_spaces);
                    let (mut tab, mut spaces) = (old_tab, old_spaces);
                    ui.menu_button(format!("{}: {tab}", if spaces { "Spaces" } else { "Tab Size" }), |ui| {
                        ui.checkbox(&mut spaces, "Indent using spaces");
                        ui.separator();
                        for n in [2, 3, 4, 8] {
                            ui.radio_value(&mut tab, n, format!("Width: {n}"));
                        }
                    });
                    if (tab, spaces) != (old_tab, old_spaces) {
                        let e = &mut self.docs[self.active].editor;
                        e.tab_width = tab;
                        e.use_spaces = spaces;
                    }
                    ui.separator();
                    measure(ui, 3);
                }
                if fit.right[4] {
                    self.theme_menu_ui(ui);
                    measure(ui, 4);
                }
                ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("status-bar-widths"), widths));
                // The message takes whatever room is left, cut short with … rather than overlapping.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(&self.status).truncate()).on_hover_text(&self.status);
                });
            });
        });
    }
}

impl eframe::App for StrataApp {
    /// See-through windows need a transparent clear; the panels paint the color.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        // The real full screen state: the window's button or the system may change it.
        if let Some(full) = ctx.input(|i| i.viewport().fullscreen) {
            self.fullscreen = full;
        }
        // Views of one buffer catch up with last frame's late edits (menus, palette).
        if self.sync_views() {
            ctx.request_repaint();
        }
        // Menu choices: native ones arrive between frames, synthesized input joins this frame.
        #[cfg(target_os = "macos")]
        if self.native_menu.is_some() {
            let rebuilt = self.fresh_menus(ctx);
            let modal = self.modal_open(ctx);
            let native = self.native_menu.as_mut().expect("checked");
            if rebuilt && let Some((_, _, menus)) = &self.menus_cache {
                native.update(menus);
            }
            let acts = native.poll();
            self.menus_stale |= !acts.is_empty();
            for act in acts {
                // A dialog is up: only the clipboard and undo items, which go to its text field.
                if modal && !matches!(act, crate::menus::Act::Copy | crate::menus::Act::Cut | crate::menus::Act::Paste | crate::menus::Act::SelectAll | crate::menus::Act::Undo | crate::menus::Act::Redo) {
                    continue;
                }
                self.run_act(ctx, act);
            }
        }
        if !self.pending_events.is_empty() {
            let events = std::mem::take(&mut self.pending_events);
            ctx.input_mut(|i| i.events.extend(events));
        }
        self.handle_shortcuts(ctx);
        // A pasted text not taken by Paste and Indent this frame is stale.
        self.keymap.pasted = None;
        self.record_macro_input(ctx);
        self.build.poll(ctx);
        self.poll_lookup(ctx);
        let typed_word = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t.chars().all(|c| c.is_alphanumeric() || c == '_'))));
        if self.completion.is_open() {
            let d = &mut self.docs[self.active];
            self.completion.keys(ctx, d);
        }
        self.snippet_keys(ctx);
        let typed = self.devshot.tick(ctx);
        self.pending_events.extend(typed);
        self.apply_fonts(ctx);
        self.sync_appearance(ctx);
        self.style_chrome(ctx);

        if self.link.managed {
            // One of several windows: `Windows` closes it and saves the session.
        } else if ctx.input(|i| i.viewport().close_requested()) {
            // Hot exit: unsaved work goes into the session instead of a prompt.
            let saved = self.hot_exit() && self.save_session();
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
            self.autosave();
        }
        // Files and folders dropped on the window open in tabs / the sidebar.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_owned()).filter(|p| !p.as_os_str().is_empty()).collect());
        for p in dropped {
            if p.is_dir() {
                self.open_folder(p);
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
        self.update_git(ctx, now);
        // Save on focus lost (a setting, off by default like Sublime's).
        let focused = ctx.input(|i| i.focused);
        if self.window_focused && !focused && self.settings.save_on_focus_lost {
            self.save_all();
        }
        if !self.window_focused && focused {
            // Back from another app, which may have changed the folders.
            self.sidebar.refresh();
        }
        self.window_focused = focused;
        if now - self.last_disk_check > 1.0 {
            self.last_disk_check = now;
            self.watch_settings();
            self.watch_keymap();
            // Every visible tab: each pane shows one.
            for g in 0..self.layout.groups() {
                if let Some(i) = self.group_active(g) {
                    let d = &mut self.docs[i];
                    if d.check_disk() {
                        self.status = format!("Reloaded {} (changed on disk)", d.name());
                    }
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
        let minimap = self.minimap_shown();
        for i in 0..self.docs.len() {
            let wrap = self.wraps(i);
            let d = &mut self.docs[i];
            d.view.wrap = wrap;
            d.view.wrap_column = self.settings.wrap_column;
            d.view.opacity = self.settings.window_opacity.clamp(0.3, 1.0);
            let s = &self.settings;
            d.view.show_whitespace = match s.draw_white_space.as_str() {
                "all" => crate::view::Whitespace::All,
                "none" => crate::view::Whitespace::None,
                _ => crate::view::Whitespace::Selection,
            };
            d.view.highlight_trailing_whitespace = s.highlight_trailing_whitespace;
            d.view.bracket_colors = s.bracket_colors && d.large.is_none();
            d.view.color_swatches = s.color_swatches;
            d.view.sticky_scroll = s.sticky_scroll && d.large.is_none();
            d.view.smooth_scroll = s.smooth_scroll;
            // Made when git's changes arrive (`update_git`); again only when the
            // setting flips or a tab got changes some other way (a cloned view).
            if s.git_gutter != self.git_marks_on || (s.git_gutter && d.view.line_marks.is_empty() != d.git_changes.is_empty()) {
                d.view.line_marks = git_marks(&d.git_changes, s.git_gutter);
            }
            d.view.caret_width = self.settings.caret_width.clamp(1.0, 6.0);
            d.view.caret_shape = match self.settings.caret_style.as_str() {
                "block" => crate::view::CaretShape::Block,
                "underline" => crate::view::CaretShape::Underline,
                _ => crate::view::CaretShape::Line,
            };
            d.view.caret_blink = match self.settings.caret_blink.as_str() {
                "smooth" => crate::view::CaretBlink::Smooth,
                "solid" => crate::view::CaretBlink::Solid,
                _ => crate::view::CaretBlink::Blink,
            };
            d.view.rulers.clone_from(&self.settings.rulers);
            if d.preview && d.editor.version() > 0 {
                d.preview = false;
            }
            d.view.font_size = self.settings.font_size;
            d.view.line_spacing = (self.settings.line_height, self.settings.line_padding_top, self.settings.line_padding_bottom);
            d.view.minimap = minimap && d.large.is_none();
        }
        self.git_marks_on = self.settings.git_gutter;
        let title = self.title();
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }

        self.title_bar(ui);
        if !cfg!(target_os = "macos") && self.distraction.is_none() {
            self.fresh_menus(ctx);
            let menus = self.menus_cache.take().expect("just built");
            let chosen = egui::Panel::top("menubar").show(ui, |ui| crate::menus::menu_bar(ui, &menus.2)).inner;
            self.menus_cache = Some(menus);
            if let Some(act) = chosen {
                self.menus_stale = true;
                self.run_act(ctx, act);
            }
        }
        if self.settings.show_status_bar && self.distraction.is_none() {
            egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        }
        self.terminal_panel(ui);
        if self.about_open {
            self.about_window(ctx);
        }
        if self.licenses_open {
            self.licenses_window(ctx);
        }
        self.crash_notice_ui(ctx);
        if self.build.visible && self.distraction.is_none() {
            let action = egui::Panel::bottom(self.link.id("build")).resizable(true).default_size(180.0).show(ui, |ui| self.build.show(ui)).inner;
            if let Some(crate::build::BuildAction::Goto(p, line, col)) = action {
                self.open_at(p, line, col..col);
            }
        }
        match self.pending_encoding.take() {
            Some((true, e)) => self.reopen_with_encoding(e),
            Some((false, e)) => self.save_with_encoding(e),
            None => {}
        }

        if self.find.open {
            egui::Panel::bottom("find").show(ui, |ui| self.find_bar(ui));
        }

        self.markdown_panel(ui);

        if self.sidebar.shown() {
            self.sidebar.set_filter(&sidebar::TreeFilter { respect_gitignore: self.settings.sidebar_respect_gitignore, exclude: self.settings.file_exclude_patterns.clone() });
            let current = self.docs[self.active].path.clone();
            let open = self.open_files_for_sidebar();
            let action = egui::Panel::left(self.link.id("sidebar"))
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
                    self.follow_rename(&from, &to);
                    self.status = format!("Renamed to {}", to.display());
                    // The other windows' tabs and trees follow too.
                    self.link.requests.renamed.push((from, to));
                }
                Some(sidebar::Action::Deleted(gone)) => {
                    self.follow_delete(&gone);
                    self.status = format!("Moved {} to the {}", gone.display(), crate::trash::trash_name());
                    self.link.requests.deleted.push(gone);
                }
                Some(sidebar::Action::Status(s)) => self.status = s,
                Some(sidebar::Action::ReplaceInFiles) => self.replace_in_files(Some(ui.ctx().clone())),
                Some(sidebar::Action::Compare(p)) => self.compare_file_with_tab(ctx, p),
                None => {}
            }
        }

        for d in &mut self.docs {
            (d.editor.auto_indent, d.editor.smart_indent) = (self.settings.auto_indent, self.settings.smart_indent);
            d.editor.indent_rules = crate::indent::rules_for(d.highlighter.syntax_name());
        }
        if self.sidebar.modal_open() || self.print.is_open() {
            // The dialog's field keeps the keyboard.
            self.cancel_editor_focus();
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.panes(ui));
        if self.sync_views() {
            ctx.request_repaint();
        }
        self.compare_actions(ctx);
        {
            let d = &mut self.docs[self.active];
            if self.settings.auto_complete || self.completion.is_open() {
                self.completion.after_edit(d, typed_word && self.settings.auto_complete, &self.snippets.lib);
            }
            self.completion.show(ctx, d);
        }
        self.snippet_from_completion();
        self.help_window(ctx);
        self.print_dialog(ctx);
        let dialog_open = self.palette.open || self.about_open || self.licenses_open || self.help.open;
        if self.docs.len() == 1 && self.docs[0].is_blank() && self.layout.groups() == 1 && !dialog_open {
            self.welcome(ctx);
        }
        if let Some(spec) = self.dev_palette.take() {
            let (mode, q) = spec.split_once(':').unwrap_or((&spec, ""));
            match mode {
                // "split:2" puts the current tab in a second pane.
                "split" => self.move_to_pane(q.parse::<usize>().unwrap_or(2).saturating_sub(1)),
                // "splitview:120": Split View, the new view at line 120; "compare:path": Compare with that file.
                "splitview" => self.dev_split_view(q),
                "compare" => self.compare_with_file(self.active, Path::new(q)),
                "p" => self.open_palette(ctx, palette::Mode::Files, q),
                "about" => self.about_open = true,
                "theme" => self.open_palette(ctx, palette::Mode::Theme, q),
                "licenses" => self.licenses_open = true,
                // "fif:query": Find in Files once the launch files are open.
                "fif" => self.dev_find_in_files(q.to_owned()),
                "help" => self.dev_help(q, false),
                "helpsearch" => self.dev_help(q, true),
                "fold" => self.run_act(ctx, crate::menus::Act::Named(format!("fold_level:{}", q.parse::<usize>().unwrap_or(2)))),
                // "run:key_bindings" runs a command by its keybindings.json id.
                "run" => self.run_command_id(ctx, q),
                _ => self.open_palette(ctx, palette::Mode::Commands, q),
            }
        }
        let events = self.palette.show(ctx);
        self.palette_events(ctx, events);
        self.run_context_actions(ctx);
        self.track_copies(ctx);
        self.update_spelling(ctx);
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.dev_context_input(ctx, raw_input);
    }
}

/// The gutter's marks for git's line changes (none when the gutter is off).
fn git_marks(changes: &crate::git::Changes, on: bool) -> Vec<(std::ops::Range<usize>, crate::view::LineMark)> {
    if !on {
        return Vec::new();
    }
    changes
        .iter()
        .map(|(r, c)| {
            let mark = match c {
                crate::git::LineChange::Added => crate::view::LineMark::Added,
                crate::git::LineChange::Modified => crate::view::LineMark::Modified,
                crate::git::LineChange::Deleted => crate::view::LineMark::Deleted,
            };
            (r.clone(), mark)
        })
        .collect()
}

/// Select `cols` (chars, clamped) on 0-based `line` and scroll to it.
fn select_on_line(d: &mut Document, line: usize, cols: std::ops::Range<usize>) {
    let line = line.min(d.editor.buffer.len_lines().saturating_sub(1));
    let start = d.editor.buffer.line_start(line);
    let end = d.editor.buffer.line_end(line);
    let range = (start + cols.start).min(end)..(start + cols.end).min(end);
    d.editor.select_ranges(&[range], 0);
    d.view.reveal_centered();
}

/// Wrap Paragraph at Ruler's column: the first ruler, like Sublime, else
/// `wrap_width` (80 when that is 0).
fn wrap_paragraph_width(s: &Settings) -> usize {
    s.rulers.first().copied().filter(|&r| r > 0).unwrap_or(if s.wrap_width > 0 { s.wrap_width } else { 80 })
}

/// The status line after startup: problems with settings.json and with user
/// themes and syntaxes.
fn startup_status(settings_error: Option<String>, asset_errors: Vec<String>) -> String {
    let problems: Vec<String> = settings_error.into_iter().chain(asset_errors).collect();
    problems.iter().for_each(|p| log::warn!("{p}"));
    problems.join("; ")
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
        assert_eq!(app.sidebar.roots, std::slice::from_ref(&dir));
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
    fn wrap_paragraph_uses_the_first_ruler() {
        let mut app = app_with("aaa bbb ccc ddd eee\n");
        app.settings.rulers = vec![8, 40];
        app.run_editor("wrap_paragraph");
        assert_eq!(text(&app), "aaa bbb\nccc ddd\neee\n");
        // No rulers: wrap_width.
        let mut app = app_with("aaa bbb ccc ddd eee\n");
        app.settings.wrap_width = 12;
        app.run_editor("wrap_paragraph");
        assert_eq!(text(&app), "aaa bbb ccc\nddd eee\n");
    }

    #[test]
    fn startup_reports_settings_and_asset_errors_together() {
        let s = startup_status(Some("settings.json: bad".into()), vec!["Theme x.tmTheme: broken".into(), "Syntax y: broken".into()]);
        assert!(s.contains("settings.json: bad") && s.contains("x.tmTheme") && s.contains("Syntax y"), "{s}");
        assert_eq!(startup_status(None, vec![]), "");
        assert_eq!(startup_status(None, vec!["a".into()]), "a");
    }

    #[test]
    fn find_and_replace_toggle_the_replace_row() {
        let mut app = app_with("x");
        app.open_find(true);
        assert!(app.find.replace);
        app.open_find(false);
        assert!(!app.find.replace, "Find shows the find row only");
        app.open_find(true);
        assert!(app.find.replace);
    }

    #[test]
    fn find_bar_replace_understands_sublime_escapes() {
        let mut app = app_with("a=1\r\nb=22\r\n");
        query(&mut app, r"(?P<k>\w)=(\d+)", true);
        app.find.replacement = r"\2\t${k}\n$1".into();
        app.replace_all();
        assert_eq!(text(&app), "1\ta\r\na\r\n22\tb\r\nb\r\n", "\\n becomes the file's line ending");
        let mut app = app_with("a=1 b=2");
        query(&mut app, r"(\w)=(\d)", true);
        app.find.replacement = r"\2\1".into();
        app.replace_one();
        app.replace_one();
        assert_eq!(text(&app), "1a b=2");
    }

    #[test]
    fn goto_anything_symbol_and_text_after_opening_a_file() {
        let ctx = Context::default();
        let mut app = app_with("fn alpha() {}\nfn gamma() {}\n// TODO: tidy\n");
        app.doc().highlighter.set_syntax("Rust");
        app.palette_events(&ctx, vec![palette::Event::GotoSymbolNamed("gam".into())]);
        assert_eq!(ed(&app).buffer.line_of(ed(&app).primary().head), 1, "the best symbol match");
        app.palette_events(&ctx, vec![palette::Event::FindInFile("todo".into())]);
        let r = ed(&app).primary().range();
        assert_eq!(ed(&app).buffer.slice(r), "TODO", "the first match is selected");
        app.palette_events(&ctx, vec![palette::Event::GotoSymbolNamed("zzz".into())]);
        assert!(app.status.contains("zzz"), "{}", app.status);
        app.palette_events(&ctx, vec![palette::Event::Select { line: 0, cols: 3..8, preview: false }]);
        assert_eq!(ed(&app).buffer.slice(ed(&app).primary().range()), "alpha");
    }

    #[test]
    fn find_in_files_without_a_folder_opens_the_search_tab() {
        let dir = temp_dir("fif-nofolder");
        std::fs::write(dir.join("a.txt"), "needle\n").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(None, None);
        app.open_path(dir.join("a.txt"), false);
        for cmd in [Cmd::FindInFiles, Cmd::ReplaceInFiles] {
            app.sidebar.visible = false;
            app.run(&ctx, cmd);
            assert!(app.sidebar.shown(), "{cmd:?} shows the Search tab");
        }
        app.sidebar.visible = false;
        app.run_act(&ctx, crate::menus::Act::Named("show_find_results".into()));
        assert!(app.sidebar.shown());
        render(&mut app, &ctx, vec![]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn every_folder_survives_a_restart() {
        let config = temp_dir("roots-session");
        let (a, b) = (config.join("a"), config.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let mut app = StrataApp::new(Some(a.clone()), Some(config.clone()));
        app.add_folder(b.clone());
        assert_eq!(app.sidebar.roots, [a.clone(), b.clone()]);
        assert_eq!(app.recent_folders.first(), Some(&b));
        assert!(app.save_session());
        drop(app);
        let back = StrataApp::new(None, Some(config.clone()));
        assert_eq!(back.sidebar.roots, [a.clone(), b.clone()]);
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn projects_keep_every_folder() {
        let dir = temp_dir("roots-project");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let project = dir.join("p.strata-project");
        std::fs::write(&project, r#"{"folders": ["a", "b"]}"#).unwrap();
        let mut app = StrataApp::new(None, None);
        app.open_project_path(project.clone());
        assert_eq!(app.sidebar.roots, [a.clone(), b.clone()]);
        let saved = dir.join("q.strata-project");
        app.save_project_to(&saved);
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&saved).unwrap()).unwrap();
        assert_eq!(json["folders"], serde_json::json!(["a", "b"]));
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
        drop(app); // releases the session lock

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
    fn hot_exit_keeps_every_cursor_and_selection() {
        let config = temp_dir("session-sels");
        let file = config.join("multi.txt");
        std::fs::write(&file, "alpha beta gamma delta\n").unwrap();
        let mut app = StrataApp::new(Some(file.clone()), Some(config.clone()));
        app.doc().editor.select_ranges(&[0..5, 6..10, 17..17], 1);
        // A backwards selection (head before anchor) keeps its direction.
        app.doc().editor.set_selection_pairs(&[(0, 5), (10, 6), (17, 17)], 1);
        let before: Vec<(usize, usize)> = ed(&app).selections().iter().map(|s| (s.anchor, s.head)).collect();
        let primary = ed(&app).primary();
        assert!(app.save_session());
        drop(app);
        let back = StrataApp::new(None, Some(config.clone()));
        let after: Vec<(usize, usize)> = ed(&back).selections().iter().map(|s| (s.anchor, s.head)).collect();
        assert_eq!(after, before);
        assert_eq!(ed(&back).primary(), primary);
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn old_sessions_without_selections_restore_the_cursor() {
        let tab: TabState = serde_json::from_str(r#"{"path": null, "cursor": 3}"#).unwrap();
        assert_eq!((tab.cursor, tab.selections.len()), (3, 0));
    }

    #[test]
    fn every_palette_editor_command_runs() {
        let mut app = app_with("b\na\nb\n");
        app.doc().highlighter.set_syntax("Python");
        // Paste and Indent reads the real clipboard, so it's left out here.
        for c in EDITOR_COMMANDS.iter().filter(|c| c.id != "paste_and_indent") {
            app.run_editor(c.id);
        }
        app.docs[0].editor = Editor::new(Buffer::from_str("b\na\n"));
        app.doc().editor.select_ranges(&[0..0], 0);
        app.run_editor("toggle_comment");
        assert!(text(&app).starts_with('#'), "{}", text(&app));
    }

    #[test]
    fn second_window_neither_restores_nor_saves_the_session() {
        let config = temp_dir("lock");
        let mut first = StrataApp::new(None, Some(config.clone()));
        first.doc().editor.insert_text("mine");
        assert!(first.save_session());
        let mut second = StrataApp::new(None, Some(config.clone()));
        assert!(second.docs[0].is_blank(), "didn't restore the first window's tabs");
        second.doc().editor.insert_text("theirs");
        assert!(!second.save_session());
        drop((first, second));
        let back = StrataApp::new(None, Some(config.clone()));
        assert_eq!(text(&back), "mine");
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn broken_session_keeps_backups_and_restored_conflicts_are_flagged() {
        let config = temp_dir("broken");
        let file = config.join("a.txt");
        std::fs::write(&file, "v1").unwrap();
        let mut app = StrataApp::new(Some(file.clone()), Some(config.clone()));
        app.doc().editor.insert_text("edit ");
        assert!(app.save_session());
        drop(app);
        // Someone changes the file while Strata is closed.
        std::fs::write(&file, "v2 from git pull").unwrap();
        let session = std::fs::read_to_string(config.join("session.json")).unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&session).unwrap();
        json["tabs"][0]["disk_mtime"] = serde_json::json!({"secs_since_epoch": 1, "nanos_since_epoch": 0});
        std::fs::write(config.join("session.json"), json.to_string()).unwrap();
        let app = StrataApp::new(None, Some(config.clone()));
        assert!(app.docs[0].conflict, "outside change while closed is flagged");
        drop(app);

        std::fs::write(config.join("session.json"), "").unwrap();
        let mut app = StrataApp::new(None, Some(config.clone()));
        assert!(app.status.contains("Unsaved text was kept"), "{}", app.status);
        app.doc().editor.insert_text("x");
        app.save_session();
        let kept = std::fs::read_dir(&config).unwrap().flatten().find(|e| e.file_name().to_string_lossy().starts_with("backups.recovered")).unwrap();
        let backup = std::fs::read_dir(kept.path()).unwrap().next().unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(backup.path()).unwrap(), "edit v1");
        std::fs::remove_dir_all(config).ok();
    }

    fn render(app: &mut StrataApp, ctx: &Context, events: Vec<Event>) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 600.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            let mut frame = eframe::Frame::_new_kittest();
            eframe::App::ui(app, ui, &mut frame);
        });
        out.textures_delta.clear();
    }

    /// A frame as eframe runs it: the raw input hook first.
    fn render_hooked(app: &mut StrataApp, ctx: &Context, events: Vec<Event>, modifiers: Modifiers) {
        let events = std::iter::once(Event::ModifiersChanged(modifiers)).chain(events).collect();
        let mut raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 600.0))), events, ..Default::default() };
        eframe::App::raw_input_hook(app, ctx, &mut raw);
        let mut out = ctx.run_ui(raw, |ui| {
            let mut frame = eframe::Frame::_new_kittest();
            eframe::App::ui(app, ui, &mut frame);
        });
        out.textures_delta.clear();
    }

    /// The primary modifier as egui-winit reports it.
    fn primary() -> Modifiers {
        if cfg!(target_os = "macos") { Modifiers::MAC_CMD | Modifiers::COMMAND } else { Modifiers::CTRL | Modifiers::COMMAND }
    }

    #[test]
    fn keys_egui_turns_into_copy_cut_paste_still_reach_the_keymap() {
        use crate::keybindings::{Keymap, install};
        let ctx = Context::default();
        let mut app = app_with("one\n");
        let (k, _) = Keymap::with_user(r#"[ { "keys": ["primary+c"], "command": "duplicate_line" } ]"#).unwrap();
        install(k);
        app.doc().view.focus();
        render_hooked(&mut app, &ctx, vec![], Modifiers::NONE);
        // egui-winit sends ⌘C / Ctrl+C as Event::Copy, never as a key press.
        render_hooked(&mut app, &ctx, vec![Event::Copy], primary());
        install(Keymap::defaults());
        assert_eq!(text(&app), "one\none\n", "the user's binding ran");
    }

    #[test]
    fn paste_and_indent_shortcut_is_not_a_plain_paste() {
        let ctx = Context::default();
        let mut app = app_with("    a\n    ");
        let end = ed(&app).buffer.len_chars();
        app.doc().editor.select_ranges(&[end..end], 0);
        app.doc().view.focus();
        render_hooked(&mut app, &ctx, vec![], Modifiers::NONE);
        let pasted = "if x:\n    y\n";
        let mut want = Editor::new(Buffer::from_str("    a\n    "));
        want.select_ranges(&[end..end], 0);
        want.paste_and_indent(pasted);
        let want = want.buffer.slice(0..want.buffer.len_chars());
        // Ctrl+Shift+V (⇧⌘V) arrives as Event::Paste with the clipboard text.
        render_hooked(&mut app, &ctx, vec![Event::Paste(pasted.into())], primary() | Modifiers::SHIFT);
        assert_eq!(text(&app), want);
        // Plain ⌘V / Ctrl+V still pastes as is.
        let mut app = app_with("");
        app.doc().view.focus();
        render_hooked(&mut app, &ctx, vec![], Modifiers::NONE);
        render_hooked(&mut app, &ctx, vec![Event::Paste("  z".into())], primary());
        assert_eq!(text(&app), "  z");
    }

    #[test]
    fn theme_changes_keep_each_files_indentation() {
        let mut app = StrataApp::new(None, None);
        app.doc().editor.tab_width = 2;
        app.doc().editor.use_spaces = false;
        app.settings.theme = "Strata Light".into();
        app.apply_settings();
        assert_eq!((app.doc().editor.tab_width, app.doc().editor.use_spaces), (2, false));
        app.apply_indentation();
        assert_eq!((app.doc().editor.tab_width, app.doc().editor.use_spaces), (app.settings.tab_size, app.settings.translate_tabs_to_spaces));
    }

    #[test]
    fn panes_hold_their_own_tabs() {
        let dir = temp_dir("panes");
        for f in ["a.rs", "b.rs", "c.rs"] {
            std::fs::write(dir.join(f), f).unwrap();
        }
        let ctx = Context::default();
        let mut app = StrataApp::new(None, None);
        app.open_path(dir.join("a.rs"), false);
        app.open_path(dir.join("b.rs"), false);
        app.run(&ctx, Cmd::MoveToGroup(1));
        assert_eq!(app.layout.groups(), 2, "moving to pane 2 splits the window");
        assert_eq!((app.focus_group, app.doc().name()), (1, "b.rs".to_owned()));
        assert_eq!(app.group_active(0).map(|i| app.docs[i].name()), Some("a.rs".into()));

        // New files open in the focused pane; tab cycling stays inside it.
        app.open_path(dir.join("c.rs"), false);
        assert_eq!(app.docs[app.active].group, 1);
        app.run(&ctx, Cmd::NextTab);
        assert_eq!(app.doc().name(), "b.rs");
        app.run(&ctx, Cmd::FocusGroup(0));
        assert_eq!(app.doc().name(), "a.rs");

        // Closing the last tab of a pane hands focus to another pane.
        app.close_tab(app.active);
        assert_eq!(app.group_active(0), None);
        assert_eq!(app.focus_group, 1);
        for _ in 0..2 {
            render(&mut app, &ctx, vec![]);
        }

        // Back to one pane: everything gathers there.
        app.run(&ctx, Cmd::LayoutSingle);
        assert!(app.docs.iter().all(|d| d.group == 0));
        assert_eq!(app.docs.len(), 2);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn clicking_a_pane_focuses_its_tab() {
        let mut app = app_with("left\n");
        app.push_doc(Document::from_buffer(None, Buffer::from_str("right\n")));
        let ctx = Context::default();
        app.run(&ctx, Cmd::MoveToGroup(1));
        app.run(&ctx, Cmd::FocusGroup(0));
        render(&mut app, &ctx, vec![]);
        render(&mut app, &ctx, vec![]);
        assert_eq!(app.focus_group, 0);
        // The right pane starts at x=500 plus its gutter; click its first line.
        let p = egui::pos2(800.0, 45.0);
        let button = |pressed| Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        render(&mut app, &ctx, vec![Event::PointerMoved(p), button(true)]);
        render(&mut app, &ctx, vec![button(false)]);
        assert_eq!(app.focus_group, 1);
        assert_eq!(text(&app), "right\n");
    }

    #[test]
    fn layout_and_panes_survive_a_restart() {
        let config = temp_dir("panes-session");
        let (a, b) = (config.join("a.txt"), config.join("b.txt"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(a.clone()), Some(config.clone()));
        app.open_path(b.clone(), false);
        app.run(&ctx, Cmd::MoveToGroup(1));
        app.layout.cols = vec![0.3, 0.7];
        assert!(app.save_session());
        drop(app);
        let back = StrataApp::new(None, Some(config.clone()));
        assert_eq!(back.layout.cols, vec![0.3, 0.7]);
        let pane = |p: &PathBuf| back.docs.iter().find(|d| d.path.as_ref() == Some(p)).unwrap().group;
        assert_eq!((pane(&a), pane(&b)), (0, 1));
        assert_eq!(back.docs[back.active].path.as_ref(), Some(&b));
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
