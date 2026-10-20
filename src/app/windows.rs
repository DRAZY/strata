//! Several windows in one Strata, like Sublime. Each window is a `StrataApp`
//! and `Windows` owns them: the first draws in eframe's root viewport, the
//! others in immediate viewports, each its own native window.
//!
//! Shared by every window: settings (a change in one applies to all at once),
//! recent files, folders and projects, clipboard history, the session (one
//! lock, one `session.json` listing every window) and, on macOS, the menu bar,
//! which follows the focused window. Each window has its own tabs, panes,
//! sidebar folder, find bar, palette, terminal panel and builds.
//!
//! Closing a window asks about its unsaved tabs (as Sublime does) and stops
//! its shells, builds and searches. Closing the last window quits, as Quit
//! does: with hot exit everything, every window, is kept for next time. On
//! macOS Strata quits with its last window rather than staying open without
//! one. eframe can't close its root window on its own, so when that window
//! is closed another window's tabs move into it, and it takes that window's
//! place on screen.

use super::StrataApp;
use crate::cli::Target;
use crate::document::Document;
use crate::session::{Session, SessionLock, Settings};
use egui::{Pos2, Vec2, ViewportBuilder, ViewportCommand, ViewportId};
use std::ops::Range;
use std::path::{Path, PathBuf};

/// What a window knows of the other windows, and what it asks of them.
#[derive(Default)]
pub(crate) struct Link {
    /// One of a `Windows`; false for a `StrataApp` on its own (tests).
    pub managed: bool,
    /// Keeps this window's panel sizes apart from the other windows'.
    salt: u64,
    /// Files open in the other windows.
    elsewhere: Vec<PathBuf>,
    /// Every window's title, for the Window menu, and which one is this.
    pub windows: Vec<String>,
    pub index: usize,
    /// Backups couldn't be set aside, so the session mustn't be saved.
    pub session_off: bool,
    /// The `Shared` version this window last synced with.
    shared_version: u64,
    /// The other windows of the session, restored with this (the first) one.
    restored: Vec<(StrataApp, Option<Pos2>, Option<Vec2>)>,
    pub requests: Requests,
}

/// A line and the columns to select on it.
type Spot = (usize, Range<usize>);

/// Asked for during a frame, carried out by `Windows` after it.
#[derive(Default)]
pub(crate) struct Requests {
    pub new_window: bool,
    pub quit: bool,
    /// Bring this window (an index into the Window menu) forward.
    pub focus_window: Option<usize>,
    /// Move this tab (a view id) to a new window.
    pub move_tab: Option<egui::Id>,
    /// Files to show in the windows that have them open, optionally selecting
    /// columns on a line.
    pub reveal: Vec<(PathBuf, Option<Spot>)>,
    /// Snippets were saved: the other windows reload theirs.
    pub snippets_changed: bool,
    /// Files or folders renamed (from, to) and moved to the Trash from this
    /// window's side bar: the other windows' tabs and trees follow.
    pub renamed: Vec<(PathBuf, PathBuf)>,
    pub deleted: Vec<PathBuf>,
}

impl Link {
    /// An egui id for a panel of this window.
    pub fn id(&self, name: &str) -> egui::Id {
        egui::Id::new((name, self.salt))
    }

    /// `path` is open in another window: ask for it to come forward there.
    pub fn open_elsewhere(&mut self, path: &Path) -> bool {
        if !self.elsewhere.iter().any(|p| p == path) {
            return false;
        }
        self.requests.reveal.push((path.to_owned(), None));
        true
    }

    /// `path` is open in another window (nothing is asked of it).
    pub fn is_open_elsewhere(&self, path: &Path) -> bool {
        self.elsewhere.iter().any(|p| p == path)
    }

    /// Select `cols` on `line` where `open_elsewhere` sent `path`.
    pub fn select_elsewhere(&mut self, path: &Path, line: usize, cols: Range<usize>) {
        if let Some(r) = self.requests.reveal.iter_mut().rev().find(|(p, _)| p == path) {
            r.1 = Some((line, cols));
        }
    }
}

impl StrataApp {
    /// Quit: every window under `Windows`, else just this one.
    pub(super) fn quit(&mut self, ctx: &egui::Context) {
        if self.link.managed {
            self.link.requests.quit = true;
        } else {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    /// An empty window sharing this one's settings and history.
    fn new_window(&self) -> StrataApp {
        let mut app = StrataApp::with_settings(self.config.clone(), self.settings.clone(), String::new());
        app.settings_mtime = self.settings_mtime;
        app.recent_files.clone_from(&self.recent_files);
        app.recent_folders.clone_from(&self.recent_folders);
        app.recent_projects.clone_from(&self.recent_projects);
        app.clip_history.clone_from(&self.clip_history);
        // Fonts belong to the egui context, which every window shares.
        app.fonts_key.clone_from(&self.fonts_key);
        // So do the language servers.
        app.lsp = super::lsp_ui::LspState::sharing(self.lsp.hub.clone());
        // A shell starts on launch only in the first window.
        app.terminal_started = true;
        // Dev aids act on the first window.
        app.dev_palette = None;
        app.devshot = crate::devshot::DevShot::off();
        app.link.managed = self.link.managed;
        app.push_doc(Document::untitled());
        app
    }

    /// Restore the session's other windows (kept in `link.restored`).
    pub(super) fn restore_other_windows(&mut self, dir: &Path, session: &Session) -> Vec<String> {
        let mut unreadable = Vec::new();
        for w in session.window_states().iter().skip(1) {
            let mut app = self.new_window();
            app.docs.clear();
            unreadable.extend(app.restore_window(dir, w));
            if app.docs.is_empty() {
                app.push_doc(Document::untitled());
            }
            app.session_key = app.session_fingerprint();
            self.link.restored.push((app, w.position.map(Pos2::from), w.size.map(Vec2::from)));
        }
        unreadable
    }

    /// Settings changed in another window: take them, as `reload_settings` would.
    fn adopt_settings(&mut self, s: &Settings) {
        let indent_changed = (s.tab_size, s.translate_tabs_to_spaces) != (self.settings.tab_size, self.settings.translate_tabs_to_spaces);
        let theme_changed = s.theme != self.settings.theme;
        self.settings.clone_from(s);
        if theme_changed {
            self.apply_settings();
        }
        if indent_changed {
            self.apply_indentation();
        }
    }

    /// Take tab `i` out (to move it to another window); a window always keeps a tab.
    fn take_tab(&mut self, i: usize) -> Document {
        let mut blank = Document::untitled();
        blank.group = self.docs[i].group;
        let mut doc = std::mem::replace(&mut self.docs[i], blank);
        self.close_tab(i);
        // Views of one buffer stay in step only within a window.
        Self::detach_view(&mut doc);
        doc
    }

    /// Show `doc` as this (new, empty) window's only tab.
    fn adopt_tab(&mut self, mut doc: Document) {
        doc.group = 0;
        self.docs = vec![doc];
        self.focus_group = 0;
        self.activate(0);
    }

    /// The window is going away: stop its shells, build and search now.
    fn shutdown(&mut self) {
        self.sidebar.search.stop();
        self.build.cancel();
        self.terminal.tabs.clear();
    }

    /// One blank tab and no folder.
    fn pristine(&self) -> bool {
        self.docs.len() == 1 && self.docs[0].is_blank() && self.sidebar.roots.is_empty()
    }
}

/// Settings and history every window shares.
struct Shared {
    settings: Settings,
    settings_mtime: Option<std::time::SystemTime>,
    recent_files: Vec<PathBuf>,
    recent_folders: Vec<PathBuf>,
    recent_projects: Vec<PathBuf>,
    clip_history: Vec<String>,
    /// Bumped on every change; a window that has seen the latest one owns
    /// whatever differs (it changed it), else it's behind and catches up.
    version: u64,
}

impl Shared {
    fn of(app: &StrataApp) -> Self {
        Self {
            settings: app.settings.clone(),
            settings_mtime: app.settings_mtime,
            recent_files: app.recent_files.clone(),
            recent_folders: app.recent_folders.clone(),
            recent_projects: app.recent_projects.clone(),
            clip_history: app.clip_history.clone(),
            version: 0,
        }
    }

    /// Bring a window up to date before it runs.
    fn give(&mut self, app: &mut StrataApp) {
        if app.link.shared_version == self.version {
            // Current: anything different was changed by the window itself.
            return self.take(app);
        }
        if app.settings != self.settings {
            app.adopt_settings(&self.settings);
        }
        app.settings_mtime = self.settings_mtime;
        sync(&mut app.recent_files, &self.recent_files);
        sync(&mut app.recent_folders, &self.recent_folders);
        sync(&mut app.recent_projects, &self.recent_projects);
        sync(&mut app.clip_history, &self.clip_history);
        app.link.shared_version = self.version;
    }

    /// Keep what a window changed.
    fn take(&mut self, app: &mut StrataApp) {
        let changed = app.settings != self.settings
            || app.settings_mtime != self.settings_mtime
            || app.recent_files != self.recent_files
            || app.recent_folders != self.recent_folders
            || app.recent_projects != self.recent_projects
            || app.clip_history != self.clip_history;
        if changed {
            self.settings.clone_from(&app.settings);
            self.settings_mtime = app.settings_mtime;
            sync(&mut self.recent_files, &app.recent_files);
            sync(&mut self.recent_folders, &app.recent_folders);
            sync(&mut self.recent_projects, &app.recent_projects);
            sync(&mut self.clip_history, &app.clip_history);
            self.version += 1;
        }
        app.link.shared_version = self.version;
    }
}

fn sync<T: Clone + PartialEq>(to: &mut Vec<T>, from: &Vec<T>) {
    if to != from {
        to.clone_from(from);
    }
}

/// How every Strata window starts: see-through, with Strata's own title bar (macOS).
pub fn builder() -> ViewportBuilder {
    ViewportBuilder::default()
        .with_inner_size([1100.0, 720.0])
        .with_title("Strata")
        // Lets View ▸ Window Opacity see through to the desktop.
        .with_transparent(true)
        // macOS: the window is see-through, so Strata paints its own solid title bar
        // under the traffic lights (see `title_bar`) instead of the system one.
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        // Otherwise eframe shows its own logo in the Dock and taskbar.
        .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon-app.png")).expect("bundled icon"))
}

/// The frame on which a window's placement is checked: by then it is shown
/// and the system has reported which monitor it is on.
const PLACEMENT_FRAME: u8 = 3;

/// Where a window shows up when its saved place is on no monitor.
const FALLBACK_POSITION: Pos2 = Pos2::new(64.0, 64.0);

/// Commands that bring a window back on screen: moved to the main monitor if
/// its title bar is on none (`monitor`, the size of the monitor it's on, is
/// None then on macOS; `visible` checks a point elsewhere), and shrunk if
/// it's bigger than its monitor.
fn placement_fixes(outer: egui::Rect, inner: Vec2, monitor: Option<Vec2>, visible: impl Fn(Pos2) -> Option<bool>) -> Vec<ViewportCommand> {
    let mut cmds = Vec::new();
    // Somewhere on the title bar, where the window is grabbed to move it.
    let grab = outer.left_top() + egui::vec2(outer.width().min(120.0) / 2.0, 8.0);
    let offscreen = monitor.is_none() || visible(grab) == Some(false);
    if offscreen {
        cmds.push(ViewportCommand::OuterPosition(FALLBACK_POSITION));
    }
    if let Some(m) = monitor {
        // Only when it can't fit at all: a window zoomed to fill its screen stays.
        let fit = inner.min(m - egui::vec2(32.0, 64.0)).max(egui::vec2(320.0, 200.0));
        if inner.x > m.x || inner.y > m.y {
            cmds.push(ViewportCommand::InnerSize(fit));
        }
    }
    cmds
}

/// Is `p` (in points) on a monitor? None where the system can't tell us
/// (then the monitor size alone decides).
#[cfg(windows)]
fn on_a_monitor(p: Pos2, pixels_per_point: f32) -> Option<bool> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
    let pt = POINT { x: (p.x * pixels_per_point) as i32, y: (p.y * pixels_per_point) as i32 };
    // SAFETY: a plain query with a point by value.
    let monitor = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONULL) };
    Some(!monitor.is_null())
}

#[cfg(not(windows))]
fn on_a_monitor(_: Pos2, _: f32) -> Option<bool> {
    None
}

/// `builder` placed where a saved window was, ignoring nonsense sizes.
fn placed(position: Option<Pos2>, size: Option<Vec2>) -> ViewportBuilder {
    let mut b = builder();
    if let Some(p) = position.filter(|p| p.x.is_finite() && p.y.is_finite()) {
        b = b.with_position(p);
    }
    if let Some(s) = size.filter(|s| s.x.is_finite() && s.y.is_finite()) {
        b = b.with_inner_size(s.max(egui::vec2(320.0, 200.0)));
    }
    b
}

struct Slot {
    /// `ViewportId::ROOT` for the first window.
    id: ViewportId,
    app: StrataApp,
    builder: ViewportBuilder,
    /// Top-left of the frame and size of the content, as last drawn (not
    /// while minimized or full screen).
    position: Option<Pos2>,
    size: Option<Vec2>,
    /// When it last had the keyboard, to pick who comes forward next.
    focused_at: u64,
    close_requested: bool,
    /// Frames drawn, up to the one where the placement is checked.
    frames: u8,
}

type Confirm = Box<dyn FnMut(&mut StrataApp, usize) -> bool>;

/// Every open window. The first is always the one in the root viewport.
pub struct Windows {
    slots: Vec<Slot>,
    config: Option<PathBuf>,
    /// The session lock, held while this process owns the session.
    lock: Option<SessionLock>,
    shared: Shared,
    /// The window with the keyboard, or the last one that had it.
    focused: ViewportId,
    clock: u64,
    next_salt: u64,
    /// More windows can be opened (false where viewports are embedded).
    multi: bool,
    /// Quitting was approved: let the root window close.
    exiting: bool,
    last_session_save: f64,
    session_key: u64,
    /// STRATA_DEV_WINDOWS=n opens n more windows on the first frame (dev aid).
    dev_windows: Option<usize>,
    /// Asks about one unsaved tab before its window closes (a dialog; tests answer themselves).
    confirm: Confirm,
}

impl Windows {
    /// The windows of the last session (hot exit), or one empty window.
    pub fn new(config: Option<PathBuf>) -> Self {
        let mut first = StrataApp::new(None, config.clone());
        let lock = first.session_lock.take();
        first.link.managed = true;
        let restored = std::mem::take(&mut first.link.restored);
        let shared = Shared::of(&first);
        let mut w = Self {
            slots: Vec::new(),
            config,
            lock,
            shared,
            focused: ViewportId::ROOT,
            clock: 0,
            next_salt: 0,
            multi: true,
            exiting: false,
            last_session_save: 0.0,
            session_key: 0,
            dev_windows: crate::dev::var("STRATA_DEV_WINDOWS").and_then(|n| n.parse().ok()),
            confirm: Box::new(|app: &mut StrataApp, i| app.confirm_close(i)),
        };
        w.add(first, ViewportId::ROOT, builder(), None, None);
        for (mut app, position, size) in restored {
            app.link.managed = true;
            w.add_window(app, position, size);
        }
        w.focused = ViewportId::ROOT;
        w.session_key = w.fingerprint();
        w
    }

    /// How the first window starts: where it was last time.
    pub fn first_builder(config: Option<&Path>) -> ViewportBuilder {
        let first = config.and_then(|d| Session::load(d).ok().flatten()).and_then(|s| s.window_states().into_iter().next());
        let (pos, size) = first.map(|w| (w.position.map(Pos2::from), w.size.map(Vec2::from))).unwrap_or_default();
        placed(pos, size)
    }

    /// The first window (the root viewport's).
    pub fn root(&mut self) -> &mut StrataApp {
        &mut self.slots[0].app
    }

    /// Use the macOS menu bar. Must run on the main thread.
    #[cfg(target_os = "macos")]
    pub fn install_native_menu(&mut self, ctx: &egui::Context) {
        let i = self.focused_index();
        self.slots[i].app.install_native_menu(ctx);
    }

    /// This process holds the session (so it's the one other `strata`s talk to).
    pub fn owns_session(&self) -> bool {
        self.lock.is_some()
    }

    /// Is `path` open in any window?
    pub fn has_open_file(&self, path: &Path) -> bool {
        self.slots.iter().any(|s| s.app.has_open_file(path))
    }

    fn add(&mut self, mut app: StrataApp, id: ViewportId, builder: ViewportBuilder, position: Option<Pos2>, size: Option<Vec2>) -> usize {
        self.next_salt += 1;
        app.link.salt = self.next_salt;
        self.clock += 1;
        self.slots.push(Slot { id, app, builder, position, size, focused_at: self.clock, close_requested: false, frames: 0 });
        self.slots.len() - 1
    }

    /// Show `app` in a native window of its own, focused.
    fn add_window(&mut self, app: StrataApp, position: Option<Pos2>, size: Option<Vec2>) -> usize {
        let id = ViewportId::from_hash_of(("strata-window", self.next_salt + 1));
        let i = self.add(app, id, placed(position, size), position, size);
        self.focused = id;
        i
    }

    /// A new empty window, a little below and right of window `from`.
    fn open_new_window(&mut self, from: usize) -> Option<usize> {
        if !self.multi {
            self.slots[from].app.status = "This system can't show more than one Strata window".into();
            return None;
        }
        let src = &self.slots[from];
        let app = src.app.new_window();
        let (position, size) = (src.position.map(|p| p + egui::vec2(28.0, 28.0)), src.size);
        Some(self.add_window(app, position, size))
    }

    fn index_of(&self, id: ViewportId) -> Option<usize> {
        self.slots.iter().position(|s| s.id == id)
    }

    fn focused_index(&self) -> usize {
        self.index_of(self.focused).unwrap_or(0)
    }

    fn focus(&mut self, i: usize, ctx: &egui::Context) {
        self.clock += 1;
        self.slots[i].focused_at = self.clock;
        self.focused = self.slots[i].id;
        ctx.send_viewport_cmd_to(self.slots[i].id, ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd_to(self.slots[i].id, ViewportCommand::Focus);
    }

    /// Bring window `i` up to date with the others before it runs.
    fn prepare(&mut self, i: usize) {
        let titles: Vec<String> = self.slots.iter().map(|s| s.app.title()).collect();
        let elsewhere: Vec<PathBuf> =
            self.slots.iter().enumerate().filter(|&(j, _)| j != i).flat_map(|(_, s)| s.app.docs.iter().filter_map(|d| d.path.clone())).collect();
        let app = &mut self.slots[i].app;
        self.shared.give(app);
        app.link.windows = titles;
        app.link.index = i;
        app.link.elsewhere = elsewhere;
    }

    /// Paths from the command line, another `strata` or Finder: files already
    /// open come forward in their window, the rest open in the focused window
    /// (a new one for `new_window`, unless that one is still empty). Returns
    /// the window to bring forward.
    pub fn open_targets(&mut self, targets: &[Target], new_window: bool) -> ViewportId {
        let mut into = self.focused_index();
        if new_window
            && !self.slots[into].app.pristine()
            && let Some(i) = self.open_new_window(into)
        {
            into = i;
        }
        let mut last = into;
        for t in targets {
            let owner = if t.path.is_dir() { None } else { self.slots.iter().position(|s| s.app.has_open_file(&t.path)) };
            let i = owner.unwrap_or(into);
            self.prepare(i);
            self.slots[i].app.open_targets(std::slice::from_ref(t));
            self.shared.take(&mut self.slots[i].app);
            last = i;
        }
        self.clock += 1;
        self.slots[last].focused_at = self.clock;
        self.focused = self.slots[last].id;
        self.focused
    }

    /// Draw every window, then act on what they asked for.
    pub fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.begin(&ctx);
        self.window_frame(0, ui, frame);
        for i in 1..self.slots.len() {
            let (id, builder) = (self.slots[i].id, self.slots[i].builder.clone());
            ctx.show_viewport_immediate(id, builder, |ui, _| self.window_frame(i, ui, frame));
        }
        self.settle(&ctx);
    }

    /// Start of a pass: which window has the keyboard, and the menu bar follows it.
    fn begin(&mut self, ctx: &egui::Context) {
        self.multi = !ctx.embed_viewports();
        let focused = ctx.input(|i| i.raw.viewports.iter().find(|(_, v)| v.focused == Some(true)).map(|(id, _)| *id));
        if let Some(id) = focused
            && id != self.focused
            && let Some(i) = self.index_of(id)
        {
            self.clock += 1;
            self.slots[i].focused_at = self.clock;
            self.focused = id;
        }
        #[cfg(target_os = "macos")]
        {
            let to = self.focused_index();
            if self.slots[to].app.native_menu.is_none()
                && let Some(menu) = self.slots.iter_mut().find_map(|s| s.app.native_menu.take())
            {
                self.slots[to].app.native_menu = Some(menu);
            }
        }
    }

    /// One window's frame, in its own viewport.
    fn window_frame(&mut self, i: usize, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.prepare(i);
        let (focused, close, minimized, fullscreen, outer, inner) = ctx.input(|inp| {
            let v = inp.viewport();
            (v.focused == Some(true), v.close_requested(), v.minimized == Some(true), v.fullscreen == Some(true), v.outer_rect, v.inner_rect)
        });
        let slot = &mut self.slots[i];
        // A few frames in (once the window is up), make sure a window placed
        // where it was last time is on a monitor that's still connected.
        if slot.frames < PLACEMENT_FRAME {
            slot.frames += 1;
            if slot.frames == PLACEMENT_FRAME && !minimized && !fullscreen {
                let (monitor, ppp) = ctx.input(|inp| (inp.viewport().monitor_size, inp.viewport().native_pixels_per_point.unwrap_or(1.0)));
                if let (Some(outer), Some(inner)) = (outer, inner) {
                    for cmd in placement_fixes(outer, inner.size(), monitor, |p| on_a_monitor(p, ppp)) {
                        ctx.send_viewport_cmd(cmd);
                    }
                }
            }
        }
        if focused && self.focused != slot.id {
            self.clock += 1;
            slot.focused_at = self.clock;
            self.focused = slot.id;
        }
        if !minimized && !fullscreen {
            slot.position = outer.map(|r| r.min).or(slot.position);
            slot.size = inner.map(|r| r.size()).or(slot.size);
        }
        slot.close_requested |= close;
        eframe::App::ui(&mut slot.app, ui, frame);
        self.shared.take(&mut self.slots[i].app);
    }

    /// After the windows drew: carry out what they asked for.
    fn settle(&mut self, ctx: &egui::Context) {
        // After the first frame, when the first window knows where it is.
        if let Some(n) = self.dev_windows.take() {
            for _ in 0..n {
                self.open_new_window(self.slots.len() - 1);
            }
        }
        let mut quit = false;
        let mut i = 0;
        while i < self.slots.len() {
            let r = std::mem::take(&mut self.slots[i].app.link.requests);
            quit |= r.quit;
            if r.snippets_changed {
                for (j, s) in self.slots.iter_mut().enumerate() {
                    if j != i {
                        s.app.snippets.lib.reload();
                    }
                }
            }
            for s in self.slots.iter_mut().enumerate().filter(|(j, _)| *j != i).map(|(_, s)| s) {
                for (from, to) in &r.renamed {
                    s.app.follow_rename(from, to);
                }
                for gone in &r.deleted {
                    s.app.follow_delete(gone);
                }
            }
            if let Some(k) = r.focus_window.filter(|&k| k < self.slots.len()) {
                self.focus(k, ctx);
            }
            for (path, at) in r.reveal {
                self.reveal(i, path, at, ctx);
            }
            if let Some(view) = r.move_tab {
                self.move_tab(i, view);
            }
            if r.new_window {
                self.open_new_window(i);
            }
            i += 1;
        }
        if quit {
            self.quit(ctx);
        }
        // Windows asked to close (window button, Close Window, or Close for the quit).
        let root_closing = self.slots[0].close_requested;
        if root_closing && self.exiting {
            return;
        }
        let closing: Vec<ViewportId> = self.slots.iter().filter(|s| s.close_requested).map(|s| s.id).collect();
        for s in &mut self.slots {
            s.close_requested = false;
        }
        if root_closing {
            // eframe would quit; it's up to us.
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::CancelClose);
        }
        for id in closing {
            if let Some(i) = self.index_of(id) {
                if self.slots.len() == 1 {
                    self.quit(ctx);
                } else {
                    self.close_window(i, ctx);
                }
            }
        }
        self.autosave(ctx);
    }

    /// Show `path` (selecting `at`) in the window that has it open.
    fn reveal(&mut self, from: usize, path: PathBuf, at: Option<Spot>, ctx: &egui::Context) {
        let to = self.slots.iter().position(|s| s.app.has_open_file(&path));
        let i = to.unwrap_or(from);
        self.prepare(i);
        if to.is_none() {
            // Closed there in the meantime: open it here after all.
            self.slots[i].app.link.elsewhere.clear();
        }
        let app = &mut self.slots[i].app;
        match at {
            Some((line, cols)) => app.open_at(path, line, cols),
            None => app.open_path(path, false),
        }
        self.shared.take(&mut self.slots[i].app);
        if i != from {
            self.focus(i, ctx);
        }
    }

    fn move_tab(&mut self, from: usize, view: egui::Id) {
        let app = &mut self.slots[from].app;
        let Some(t) = app.docs.iter().position(|d| d.view.id == view) else { return };
        if app.docs.len() < 2 || !self.multi {
            return;
        }
        let doc = app.take_tab(t);
        if let Some(i) = self.open_new_window(from) {
            self.slots[i].app.adopt_tab(doc);
        }
    }

    /// Close window `i` (not the last one), asking about its unsaved tabs first.
    fn close_window(&mut self, i: usize, ctx: &egui::Context) {
        self.prepare(i);
        for t in 0..self.slots[i].app.docs.len() {
            if !(self.confirm)(&mut self.slots[i].app, t) {
                return;
            }
        }
        self.shared.take(&mut self.slots[i].app);
        let next = (0..self.slots.len()).filter(|&j| j != i).max_by_key(|&j| self.slots[j].focused_at).expect("another window");
        // The root viewport always shows a window: the next one moves into it.
        let next_id = if i == 0 { ViewportId::ROOT } else { self.slots[next].id };
        let mut gone = if i == 0 {
            // The root window stays: the next window's tabs move into it, and it
            // moves to where that window was.
            let s = self.slots.remove(next);
            let root = &mut self.slots[0];
            if let Some(p) = s.position {
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::OuterPosition(p));
            }
            if let Some(size) = s.size {
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::InnerSize(size));
            }
            (root.position, root.size) = (s.position, s.size);
            let old = std::mem::replace(&mut root.app, s.app);
            root.app.activate(root.app.active);
            old
        } else {
            self.slots.remove(i).app
        };
        #[cfg(target_os = "macos")]
        let menu = gone.native_menu.take();
        gone.shutdown();
        drop(gone);
        let next = self.index_of(next_id).unwrap_or(0);
        #[cfg(target_os = "macos")]
        if menu.is_some() && self.slots[next].app.native_menu.is_none() {
            self.slots[next].app.native_menu = menu;
        }
        self.focus(next, ctx);
    }

    /// Quit: with hot exit every window is kept for next time; without, each
    /// unsaved tab is asked about (and quitting stops at the first Cancel).
    fn quit(&mut self, ctx: &egui::Context) {
        if self.exiting {
            return;
        }
        if !(self.hot_exit() && self.save_session()) {
            for i in 0..self.slots.len() {
                if self.slots[i].app.docs.iter().any(|d| d.is_dirty()) {
                    self.focus(i, ctx);
                }
                self.prepare(i);
                for t in 0..self.slots[i].app.docs.len() {
                    if !(self.confirm)(&mut self.slots[i].app, t) {
                        return;
                    }
                }
            }
        }
        self.exiting = true;
        for s in &mut self.slots {
            s.app.shutdown();
        }
        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
    }

    fn hot_exit(&self) -> bool {
        self.shared.settings.hot_exit && self.lock.is_some() && !self.slots.iter().any(|s| s.app.link.session_off)
    }

    /// Changes whenever saving the session would write something different.
    fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for s in &self.slots {
            s.app.session_fingerprint().hash(&mut h);
            format!("{:?}", (s.position, s.size)).hash(&mut h);
        }
        (&self.shared.recent_projects, self.slots.len()).hash(&mut h);
        h.finish()
    }

    /// Save every window as you go, so a crash loses at most a few seconds.
    fn autosave(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if now - self.last_session_save > 5.0 {
            self.last_session_save = now;
            if let Some(e) = super::autosave::take_error() {
                let i = self.focused_index();
                self.slots[i].app.status = format!("Could not save session: {e}");
                self.session_key = 0;
            }
            let key = self.fingerprint();
            if self.hot_exit() && key != self.session_key
                && let Some((dir, session, backups)) = self.session_to_save()
                && super::autosave::start(session, dir, backups)
            {
                self.session_key = key;
            }
        }
    }

    /// After a crash: keep every unsaved tab. With hot exit the session is
    /// saved as on quit (it all reopens next launch); otherwise, or if that
    /// fails, the unsaved texts go in a `backups.recovered-<time>` folder.
    pub fn emergency_save(&mut self) -> crate::crash::Rescue {
        use crate::crash::Rescue;
        let tabs = self.unsaved().count();
        if self.hot_exit() && crate::crash::rescue(|| if self.write_session(false) { Rescue::Session { tabs } } else { Rescue::Nothing }) != Rescue::Nothing {
            return Rescue::Session { tabs };
        }
        if tabs == 0 {
            return Rescue::Nothing;
        }
        let Some(dir) = self.config.clone() else { return Rescue::Failed("no settings folder".into()) };
        let texts: Vec<(String, String)> = self.unsaved().map(|d| (d.name(), d.editor.buffer.slice(0..d.editor.buffer.len_chars()))).collect();
        match crate::crash::write_recovered(&dir, &texts) {
            Ok(path) => Rescue::Folder { path, tabs },
            Err(e) => Rescue::Failed(e.to_string()),
        }
    }

    fn unsaved(&self) -> impl Iterator<Item = &Document> {
        self.slots.iter().flat_map(|s| &s.app.docs).filter(|d| d.is_dirty())
    }

    /// Show the "Strata quit unexpectedly" notice in the first window.
    pub fn show_crash_notice(&mut self, crash: crate::crash::PreviousCrash) {
        self.slots[0].app.crash_notice = Some(crash);
    }

    /// Write the session now (quitting): waits for an autosave in progress.
    fn save_session(&mut self) -> bool {
        self.write_session(true)
    }

    /// `prune`: drop backups the new session doesn't use (not after a crash).
    fn write_session(&mut self, prune: bool) -> bool {
        let Some((dir, session, backups)) = self.session_to_save() else { return false };
        super::autosave::wait();
        let written = if prune { session.save(&dir, &backups) } else { session.save_keeping_backups(&dir, &backups) };
        match written {
            Ok(()) => {
                self.session_key = self.fingerprint();
                true
            }
            Err(e) => {
                let i = self.focused_index();
                self.slots[i].app.status = format!("Could not save session: {e}");
                false
            }
        }
    }

    /// Every window's session, ready to write; None when it isn't ours to save.
    fn session_to_save(&self) -> Option<(std::path::PathBuf, Session, super::autosave::Backups)> {
        let dir = self.config.clone().filter(|_| self.lock.is_some())?;
        let mut backups = Vec::new();
        let windows = self
            .slots
            .iter()
            .map(|s| {
                let mut w = s.app.window_state(&mut backups);
                w.position = s.position.map(|p| [p.x, p.y]);
                w.size = s.size.map(|v| [v.x, v.y]);
                w
            })
            .collect();
        let mut session = Session::of_windows(windows);
        session.recent_files.clone_from(&self.shared.recent_files);
        session.recent_folders.clone_from(&self.shared.recent_folders);
        session.recent_projects.clone_from(&self.shared.recent_projects);
        Some((dir, session, backups))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Cmd;
    use egui::{Context, Event, Key, Modifiers, RawInput, Rect, ViewportEvent, ViewportInfo, pos2, vec2};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-windows-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn context() -> Context {
        let ctx = Context::default();
        // What eframe does on desktops (no viewports are really shown here).
        ctx.set_embed_viewports(false);
        ctx
    }

    /// One pass of window `i` (as eframe would run it in that window's
    /// viewport), then what the window set does after its windows drew.
    /// Returns the commands sent to the root viewport.
    fn pass(w: &mut Windows, ctx: &Context, i: usize, events: Vec<Event>, close: bool) -> Vec<ViewportCommand> {
        let id = w.slots[i].id;
        let mut info = ViewportInfo { focused: Some(true), ..Default::default() };
        info.inner_rect = Some(Rect::from_min_size(pos2(100.0 * i as f32, 50.0), vec2(1000.0, 600.0)));
        info.outer_rect = info.inner_rect;
        if close {
            info.events.push(ViewportEvent::Close);
        }
        let raw = RawInput {
            viewport_id: id,
            viewports: std::iter::once((id, info)).collect(),
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            let mut frame = eframe::Frame::_new_kittest();
            w.begin(ui.ctx());
            w.window_frame(i, ui, &mut frame);
            w.settle(ui.ctx());
        });
        out.textures_delta.clear();
        out.viewport_output.get(&ViewportId::ROOT).map(|o| o.commands.clone()).unwrap_or_default()
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
    }

    fn cmd() -> Modifiers {
        if crate::keys::MAC { Modifiers::MAC_CMD | Modifiers::COMMAND } else { Modifiers::CTRL | Modifiers::COMMAND }
    }

    fn text(app: &StrataApp) -> String {
        let e = &app.docs[app.active].editor;
        e.buffer.slice(0..e.buffer.len_chars())
    }

    #[test]
    fn new_window_shortcut_opens_an_empty_window_that_gets_the_commands() {
        let ctx = context();
        let mut w = Windows::new(None);
        w.root().doc().editor.insert_text("first");
        pass(&mut w, &ctx, 0, vec![key(Key::N, cmd() | Modifiers::SHIFT)], false);
        assert_eq!(w.slots.len(), 2);
        assert!(w.slots[1].app.pristine(), "a new window is empty");
        assert_eq!(w.focused, w.slots[1].id, "and has the keyboard");
        assert_eq!(w.slots[1].position, Some(pos2(28.0, 78.0)), "a little below and right of the first");
        // Shortcuts act on the window they're pressed in.
        pass(&mut w, &ctx, 1, vec![key(Key::N, cmd())], false);
        assert_eq!((w.slots[0].app.docs.len(), w.slots[1].app.docs.len()), (1, 2));
        assert_eq!(text(&w.slots[0].app), "first");
    }

    #[test]
    fn the_menu_bar_follows_the_focused_window() {
        let ctx = context();
        let mut w = Windows::new(None);
        w.open_new_window(0).unwrap();
        pass(&mut w, &ctx, 1, vec![], false);
        // Window ▸ <first window> brings it forward.
        assert_eq!(w.slots[1].app.link.windows.len(), 2);
        w.slots[1].app.run_act(&ctx, crate::menus::Act::Named("window:0".into()));
        pass(&mut w, &ctx, 1, vec![], false);
        assert_eq!(w.focused, ViewportId::ROOT);
        // A command from the menu bar runs in the window it was chosen in.
        w.slots[1].app.run_act(&ctx, crate::menus::Act::Cmd(Cmd::NewTab));
        assert_eq!((w.slots[0].app.docs.len(), w.slots[1].app.docs.len()), (1, 2));
    }

    #[test]
    fn settings_and_recent_files_are_shared() {
        let dir = temp_dir("shared");
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        let ctx = context();
        let mut w = Windows::new(None);
        w.open_new_window(0).unwrap();
        w.slots[1].app.run(&ctx, Cmd::ZoomIn);
        w.slots[1].app.settings.theme = "Strata Light".into();
        w.slots[1].app.settings.tab_size = 8;
        w.slots[1].app.open_path(dir.join("a.txt"), false);
        pass(&mut w, &ctx, 1, vec![], false);
        pass(&mut w, &ctx, 0, vec![], false);
        let first = &w.slots[0].app;
        assert_eq!(first.settings.font_size, 15.0);
        assert_eq!(first.docs[0].highlighter.theme_name(), "Strata Light", "a theme change shows at once");
        assert_eq!(first.docs[0].editor.tab_width, 8, "a new default indentation applies, as when settings.json changes");
        assert_eq!(first.recent_files, vec![dir.join("a.txt")]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_change_in_the_settings_window_reaches_every_window() {
        use super::super::settings_ui::Change;
        let ctx = context();
        let mut w = Windows::new(None);
        w.open_new_window(0).unwrap();
        w.slots[1].app.run(&ctx, Cmd::SettingsWindow);
        let set = |key, value| Change::Set { key, value, persist: true };
        w.slots[1].app.settings_change(&ctx, set("theme", serde_json::Value::from("Strata Light")));
        w.slots[1].app.settings_change(&ctx, set("minimap", serde_json::Value::Bool(false)));
        pass(&mut w, &ctx, 1, vec![], false);
        pass(&mut w, &ctx, 0, vec![], false);
        let first = &w.slots[0].app;
        assert!(!first.settings.minimap);
        assert_eq!(first.docs[0].highlighter.theme_name(), "Strata Light");
    }

    #[test]
    fn windows_off_every_monitor_come_back() {
        let r = |x: f32, y: f32| Rect::from_min_size(pos2(x, y), vec2(1000.0, 700.0));
        let size = vec2(1000.0, 680.0);
        let monitor = Some(vec2(1440.0, 900.0));
        assert!(placement_fixes(r(100.0, 80.0), size, monitor, |_| None).is_empty(), "on screen: left alone");
        assert!(placement_fixes(r(-1500.0, 80.0), size, monitor, |_| Some(true)).is_empty(), "on a second monitor to the left");
        // macOS: on no monitor at all.
        assert_eq!(placement_fixes(r(5000.0, 80.0), size, None, |_| None), [ViewportCommand::OuterPosition(FALLBACK_POSITION)]);
        // Windows: the nearest monitor is reported, but the title bar is on none.
        assert_eq!(placement_fixes(r(5000.0, 80.0), size, monitor, |_| Some(false)), [ViewportCommand::OuterPosition(FALLBACK_POSITION)]);
        // Bigger than the (smaller, now) monitor: shrunk to fit.
        let fixes = placement_fixes(r(0.0, 0.0), vec2(2400.0, 1400.0), monitor, |_| Some(true));
        assert_eq!(fixes, [ViewportCommand::InnerSize(vec2(1408.0, 836.0))]);
        assert!(placement_fixes(r(0.0, 0.0), vec2(1440.0, 875.0), monitor, |_| Some(true)).is_empty(), "a zoomed window stays");
    }

    #[test]
    fn side_bar_rename_and_trash_reach_every_window() {
        let dir = temp_dir("rename");
        let (a, b, c) = (dir.join("a.txt"), dir.join("b.txt"), dir.join("c.txt"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&c, "c").unwrap();
        let ctx = context();
        let mut w = Windows::new(None);
        w.root().open_path(a.clone(), false);
        w.root().open_path(c.clone(), false);
        w.open_new_window(0).unwrap();
        w.slots[1].app.open_folder(dir.clone());
        pass(&mut w, &ctx, 1, vec![], false);
        // Rename a.txt to b.txt in the second window's side bar.
        w.slots[1].app.sidebar.ask_rename(&a);
        *w.slots[1].app.sidebar.prompt_state().unwrap().0 = "b.txt".into();
        pass(&mut w, &ctx, 1, vec![], false);
        pass(&mut w, &ctx, 1, vec![key(Key::Enter, Modifiers::NONE)], false);
        assert!(b.exists() && !a.exists());
        assert!(w.slots[0].app.has_open_file(&b), "the first window's tab followed the rename");
        // Move c.txt to the Trash: the first window's (clean) tab closes.
        w.slots[1].app.link.requests.deleted.push(c.clone());
        pass(&mut w, &ctx, 1, vec![], false);
        assert!(!w.slots[0].app.has_open_file(&c));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_file_open_in_another_window_comes_forward_there() {
        let dir = temp_dir("elsewhere");
        let file = dir.join("a.txt");
        std::fs::write(&file, "one\ntwo\n").unwrap();
        let ctx = context();
        let mut w = Windows::new(None);
        w.root().open_path(file.clone(), false);
        w.open_new_window(0).unwrap();
        pass(&mut w, &ctx, 1, vec![], false);
        w.slots[1].app.open_at(file.clone(), 1, 0..3);
        pass(&mut w, &ctx, 1, vec![], false);
        assert!(!w.slots[1].app.has_open_file(&file), "not opened twice");
        assert_eq!(w.focused, ViewportId::ROOT);
        let d = &w.slots[0].app.docs[w.slots[0].app.active];
        assert_eq!(d.editor.buffer.slice(d.editor.primary().range()), "two");
        // From another `strata` too.
        assert_eq!(w.open_targets(&[Target::plain(file.clone())], false), ViewportId::ROOT);
        assert_eq!(w.slots.iter().filter(|s| s.app.has_open_file(&file)).count(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn new_window_flag_opens_files_in_a_new_window_unless_the_current_one_is_empty() {
        let dir = temp_dir("dash-n");
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        let mut w = Windows::new(None);
        w.open_targets(&[Target::plain(dir.join("a.txt"))], true);
        assert_eq!(w.slots.len(), 1, "the empty window is used");
        let id = w.open_targets(&[Target::plain(dir.join("b.txt"))], true);
        assert_eq!(w.slots.len(), 2);
        assert_eq!(id, w.slots[1].id);
        assert!(w.slots[1].app.has_open_file(&dir.join("b.txt")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn closing_a_window_asks_about_unsaved_tabs() {
        let ctx = context();
        let mut w = Windows::new(None);
        w.open_new_window(0).unwrap();
        w.slots[1].app.doc().editor.insert_text("unsaved");
        let asked = std::rc::Rc::new(std::cell::Cell::new(0));
        let a = asked.clone();
        w.confirm = Box::new(move |app, i| {
            if app.docs[i].is_dirty() {
                a.set(a.get() + 1);
                return false; // Cancel
            }
            true
        });
        pass(&mut w, &ctx, 1, vec![], true);
        assert_eq!(asked.get(), 1);
        assert_eq!(w.slots.len(), 2, "Cancel keeps the window and its text");
        assert_eq!(text(&w.slots[1].app), "unsaved");

        // Close Window without unsaved text closes just that window.
        w.slots[1].app.doc().editor.undo();
        w.slots[1].app.run(&ctx, Cmd::CloseWindow);
        pass(&mut w, &ctx, 1, vec![], true);
        assert_eq!(w.slots.len(), 1);
        assert_eq!(w.focused, ViewportId::ROOT);
    }

    #[cfg(unix)]
    #[test]
    fn closing_a_window_ends_its_shells() {
        let ctx = context();
        let mut w = Windows::new(None);
        w.open_new_window(0).unwrap();
        w.slots[1].app.terminal.open(&ctx, &std::env::temp_dir()).unwrap();
        let pid = w.slots[1].app.terminal.tabs[0].pid() as i32;
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        assert_eq!(unsafe { kill(pid, 0) }, 0, "the shell runs");
        pass(&mut w, &ctx, 1, vec![], true);
        assert_eq!(w.slots.len(), 1);
        // Gone, or a zombie waiting for the reaper thread (also fine). Generous
        // deadline: CI machines can be slow to reap.
        let running = || {
            std::process::Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().is_ok_and(|o| {
                let stat = String::from_utf8_lossy(&o.stdout);
                // Z: zombie; E: exiting (macOS), waiting only to be reaped.
                !stat.trim().is_empty() && !stat.contains(['Z', 'E'])
            })
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while running() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!running(), "no orphan shell");
    }

    #[test]
    fn closing_the_first_window_moves_the_next_into_it() {
        let ctx = context();
        let mut w = Windows::new(None);
        w.open_new_window(0).unwrap();
        w.slots[1].app.doc().editor.insert_text("second");
        pass(&mut w, &ctx, 1, vec![], false);
        let commands = pass(&mut w, &ctx, 0, vec![], true);
        assert!(commands.contains(&ViewportCommand::CancelClose), "eframe must not quit");
        assert!(commands.contains(&ViewportCommand::OuterPosition(pos2(100.0, 50.0))), "it takes the other window's place");
        assert_eq!(w.slots.len(), 1);
        assert_eq!(w.slots[0].id, ViewportId::ROOT);
        assert_eq!(text(&w.slots[0].app), "second");
    }

    #[test]
    fn closing_the_last_window_quits_keeping_everything() {
        let config = temp_dir("quit");
        let ctx = context();
        let mut w = Windows::new(Some(config.clone()));
        w.confirm = Box::new(|_, _| panic!("hot exit doesn't ask"));
        w.root().doc().editor.insert_text("kept");
        let commands = pass(&mut w, &ctx, 0, vec![], true);
        assert!(commands.contains(&ViewportCommand::Close));
        let commands = pass(&mut w, &ctx, 0, vec![], true);
        assert!(!commands.contains(&ViewportCommand::CancelClose), "then the app closes");
        drop(w);
        let mut back = Windows::new(Some(config.clone()));
        assert_eq!(text(back.root()), "kept");
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn every_window_is_restored() {
        let config = temp_dir("restore");
        let file = config.join("a.txt");
        std::fs::write(&file, "on disk").unwrap();
        let ctx = context();
        let mut w = Windows::new(Some(config.clone()));
        w.root().open_path(file.clone(), false);
        w.open_new_window(0).unwrap();
        w.slots[1].app.doc().editor.insert_text("scratch one");
        w.slots[1].app.push_doc(Document::untitled());
        w.slots[1].app.doc().editor.insert_text("scratch two");
        w.open_new_window(1).unwrap();
        w.slots[2].app.doc().editor.insert_text("third");
        pass(&mut w, &ctx, 1, vec![], false);
        assert!(w.save_session());
        drop(w);

        let back = Windows::new(Some(config.clone()));
        assert_eq!(back.slots.len(), 3);
        assert!(back.slots[0].app.has_open_file(&file));
        let texts: Vec<String> = back.slots[1].app.docs.iter().map(|d| d.editor.buffer.slice(0..d.editor.buffer.len_chars())).collect();
        assert_eq!(texts, ["scratch one", "scratch two"]);
        assert_eq!(text(&back.slots[2].app), "third");
        assert_eq!(back.slots[1].position, Some(pos2(100.0, 50.0)));
        // Each unsaved tab has its own backup.
        assert_eq!(std::fs::read_dir(Session::backups(&config)).unwrap().count(), 3);
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn a_tab_moves_to_a_new_window() {
        let ctx = context();
        let mut w = Windows::new(None);
        w.root().doc().editor.insert_text("stays");
        w.root().push_doc(Document::untitled());
        w.root().doc().editor.insert_text("moves");
        let view = w.root().doc().view.id;
        w.root().link.requests.move_tab = Some(view);
        pass(&mut w, &ctx, 0, vec![], false);
        assert_eq!(w.slots.len(), 2);
        assert_eq!(text(&w.slots[0].app), "stays");
        assert_eq!(w.slots[1].app.docs.len(), 1);
        assert_eq!(text(&w.slots[1].app), "moves");
        assert!(w.slots[1].app.docs[0].is_dirty(), "unsaved text comes along");
    }

    #[test]
    fn quit_without_hot_exit_asks_in_every_window() {
        let ctx = context();
        let mut w = Windows::new(None);
        w.open_new_window(0).unwrap();
        w.slots[1].app.doc().editor.insert_text("unsaved");
        w.confirm = Box::new(|app, i| !app.docs[i].is_dirty());
        w.slots[0].app.link.requests.quit = true;
        let commands = pass(&mut w, &ctx, 0, vec![], false);
        assert!(!commands.contains(&ViewportCommand::Close), "Cancel stops quitting");
        assert!(!w.exiting);
        w.confirm = Box::new(|_, _| true);
        w.slots[0].app.link.requests.quit = true;
        let commands = pass(&mut w, &ctx, 0, vec![], false);
        assert!(commands.contains(&ViewportCommand::Close));
    }

    #[test]
    fn the_emergency_save_keeps_the_last_good_backups() {
        let config = temp_dir("crash-keep");
        let mut w = Windows::new(Some(config.clone()));
        w.root().doc().editor.insert_text("good");
        assert!(w.save_session());
        let read_all = || -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(Session::backups(&config)).unwrap().map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap()).collect();
            v.sort();
            v
        };
        assert_eq!(read_all(), vec!["good"]);
        // The state after a panic may be damaged: the backups from before it stay.
        w.root().doc().editor.insert_text(" then the crash");
        assert_eq!(w.emergency_save(), crate::crash::Rescue::Session { tabs: 1 });
        assert_eq!(read_all(), vec!["good", "good then the crash"]);
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn a_crash_saves_unsaved_tabs_for_the_next_launch() {
        use crate::crash::Rescue;
        let config = temp_dir("crash");
        let mut w = Windows::new(Some(config.clone()));
        w.root().doc().editor.insert_text("typed before the crash");
        w.open_new_window(0).unwrap();
        w.slots[1].app.doc().editor.insert_text("second window");
        assert_eq!(w.emergency_save(), Rescue::Session { tabs: 2 });
        let backups: Vec<String> = std::fs::read_dir(Session::backups(&config)).unwrap().map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap()).collect();
        assert!(backups.contains(&"typed before the crash".to_owned()) && backups.contains(&"second window".to_owned()), "{backups:?}");
        // No quit: dropping only releases the session lock, as the process ending does.
        drop(w);
        let mut back = Windows::new(Some(config.clone()));
        assert_eq!(back.slots.len(), 2);
        assert_eq!(text(back.root()), "typed before the crash");
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn without_hot_exit_a_crash_writes_unsaved_tabs_to_a_folder() {
        use crate::crash::Rescue;
        let config = temp_dir("crash-no-hot-exit");
        std::fs::write(Settings::path(&config), r#"{ "hot_exit": false }"#).unwrap();
        let mut w = Windows::new(Some(config.clone()));
        w.root().doc().editor.insert_text("only copy");
        let Rescue::Folder { path, tabs: 1 } = w.emergency_save() else { panic!("not saved to a folder") };
        assert!(path.starts_with(&config));
        let files: Vec<_> = std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(files.len(), 1);
        assert_eq!(std::fs::read_to_string(&files[0]).unwrap(), "only copy");
        assert_eq!(Windows::new(None).emergency_save(), Rescue::Nothing, "nothing unsaved");
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn syntax_settings_reach_every_window() {
        let dir = temp_dir("syntax-settings");
        std::fs::write(dir.join("a.yml"), "a: 1\n").unwrap();
        let ctx = context();
        let mut w = Windows::new(None);
        w.open_new_window(0).unwrap();
        w.slots[0].app.open_path(dir.join("a.yml"), false);
        w.slots[1].app.settings.syntax_settings.insert("yaml".into(), crate::doc_settings::Layer { tab_size: Some(6), ..Default::default() });
        pass(&mut w, &ctx, 1, vec![], false);
        pass(&mut w, &ctx, 0, vec![], false);
        assert_eq!(w.slots[0].app.docs[0].editor.tab_width, 6);
        std::fs::remove_dir_all(dir).ok();
    }
}
