//! Every language server of a Strata process: one per (server, project
//! folder), started the first time a file it handles is open, restarted
//! (with growing pauses) when it stops on its own, and told about the open
//! files and their edits. Used from the UI thread only; nothing here waits
//! for a server.

use super::config::{self, ServerConfig, ServerSetting};
use super::position::Encoding;
use super::server::{DiagSet, Diagnostic, Server, Status};
use super::uri;
use ropey::Rope;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Restarts allowed within `CRASH_WINDOW` before giving up on a server.
const MAX_CRASHES: usize = 5;
const CRASH_WINDOW: Duration = Duration::from_secs(180);
/// Pause before the first restart; it doubles with each crash, up to `MAX_PAUSE`.
const FIRST_PAUSE: Duration = Duration::from_secs(1);
const MAX_PAUSE: Duration = Duration::from_secs(30);
/// How long quitting waits for servers to exit before ending them.
const QUIT_WAIT: Duration = Duration::from_millis(1500);

/// What the status bar says about a file's server.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Starting,
    Ready,
    /// Ready, and reporting work (indexing…).
    Busy(String),
    /// Stopped on its own; starting again in a moment.
    Restarting { why: String, secs: u64 },
    /// Stopped too often, or couldn't start: waiting for LSP: Restart Language Servers.
    Failed(String),
}

struct Slot {
    config: ServerConfig,
    root: PathBuf,
    program: PathBuf,
    server: Option<Server>,
    /// Bumped each time a process starts: open files are sent to it again.
    run: u64,
    crashes: Vec<Instant>,
    retry_at: Option<Instant>,
    last_error: Option<String>,
    failed: Option<String>,
}

struct OpenDoc {
    slot: usize,
    run: u64,
    /// The document version last sent (the protocol's, counting from 1).
    version: i32,
    /// (editor id, editor version) of the text last sent.
    text_key: (u64, u64),
    /// The window whose tab the server is told about.
    window: u64,
    /// Modification time of the file when last seen: a change means it was saved.
    mtime: Option<SystemTime>,
}

pub struct Hub {
    /// False in tests unless a test turns it on (no server ever starts by accident).
    active: bool,
    enabled: bool,
    configs: Vec<ServerConfig>,
    settings_seen: Option<(bool, BTreeMap<String, ServerSetting>)>,
    slots: Vec<Slot>,
    docs: HashMap<PathBuf, OpenDoc>,
    /// Which server (slot) handles a file, or none; worked out once per file.
    assigned: HashMap<PathBuf, Option<usize>>,
    ctx: Option<egui::Context>,
    path_env: Option<OsString>,
    /// Problems for the status bar, said once each.
    notes: Vec<String>,
    said: Vec<String>,
}

impl Hub {
    pub fn new(active: bool) -> Self {
        Self {
            active,
            enabled: true,
            configs: Vec::new(),
            settings_seen: None,
            slots: Vec::new(),
            docs: HashMap::new(),
            assigned: HashMap::new(),
            ctx: None,
            path_env: crate::build::augmented_path(),
            notes: Vec::new(),
            said: Vec::new(),
        }
    }

    pub fn set_context(&mut self, ctx: &egui::Context) {
        if self.ctx.is_none() {
            self.ctx = Some(ctx.clone());
        }
    }

    /// Follow the `lsp_enabled` and `lsp_servers` settings. A change stops
    /// the running servers; files start the right ones again as they're seen.
    pub fn configure(&mut self, enabled: bool, user: &BTreeMap<String, ServerSetting>) {
        if self.settings_seen.as_ref().is_some_and(|(e, u)| *e == enabled && u == user) {
            return;
        }
        let (configs, problems) = config::resolve(user);
        for p in problems {
            self.note(p);
        }
        let first = self.settings_seen.is_none();
        self.settings_seen = Some((enabled, user.clone()));
        self.enabled = enabled;
        if !first || !self.slots.is_empty() {
            self.stop_all();
        }
        self.configs = configs;
    }

    /// Use exactly these servers (tests).
    #[cfg(test)]
    pub fn use_servers(&mut self, configs: Vec<ServerConfig>) {
        self.stop_all();
        self.active = true;
        self.enabled = true;
        self.settings_seen = Some((true, BTreeMap::new()));
        self.configs = configs;
    }

    fn on(&self) -> bool {
        self.active && self.enabled
    }

    /// Stop every server and forget the open files (they're sent again to
    /// whatever starts next).
    fn stop_all(&mut self) {
        for slot in &mut self.slots {
            if let Some(s) = slot.server.take() {
                s.stop();
                reap(s);
            }
        }
        self.slots.clear();
        self.docs.clear();
        self.assigned.clear();
    }

    /// LSP: Restart Language Servers: stop them all and start afresh,
    /// including servers that gave up and programs installed since.
    pub fn restart_all(&mut self) {
        self.stop_all();
        self.said.clear();
    }

    fn note(&mut self, text: String) {
        if !self.said.contains(&text) {
            log::warn!("{text}");
            self.said.push(text.clone());
            self.notes.push(text);
        }
    }

    /// Status bar messages since the last call (problems, and what servers asked to show).
    pub fn take_notes(&mut self) -> Vec<String> {
        for slot in &self.slots {
            if let Some(s) = &slot.server {
                for m in s.take_messages() {
                    self.notes.push(format!("{}: {m}", slot.config.display_name()));
                }
            }
        }
        std::mem::take(&mut self.notes)
    }

    /// The server slot for a file, starting its server if this is the
    /// first file it handles in that project. None: no server for it.
    fn slot_for(&mut self, path: &Path, roots: &[PathBuf]) -> Option<usize> {
        if let Some(a) = self.assigned.get(path) {
            return *a;
        }
        let found = self.find_slot(path, roots);
        self.assigned.insert(path.to_owned(), found);
        found
    }

    fn find_slot(&mut self, path: &Path, roots: &[PathBuf]) -> Option<usize> {
        let path_env = self.path_env.clone().or_else(|| std::env::var_os("PATH")).unwrap_or_default();
        // The first server for the file type whose program is there.
        let (config, program) = self.configs.iter().filter(|c| c.handles(path)).find_map(|c| config::find_executable(&c.command, &path_env).map(|p| (c.clone(), p))).or_else(|| {
            // A server the user set up whose program is missing: worth saying (built-ins aren't).
            let wanted = self.configs.iter().find(|c| c.handles(path) && !c.builtin).cloned();
            if let Some(c) = wanted {
                self.note(format!("Language server “{}” not found (lsp_servers ▸ {})", c.command, c.name));
            }
            None
        })?;
        let root = config::find_root(path, &config.root_markers, roots);
        if let Some(i) = self.slots.iter().position(|s| s.config.name == config.name && s.root == root) {
            return Some(i);
        }
        self.slots.push(Slot { config, root, program, server: None, run: 0, crashes: Vec::new(), retry_at: None, last_error: None, failed: None });
        let i = self.slots.len() - 1;
        self.start(i);
        Some(i)
    }

    fn start(&mut self, i: usize) {
        let ctx = self.ctx.clone();
        let path_env = self.path_env.clone();
        let slot = &mut self.slots[i];
        slot.run += 1;
        slot.retry_at = None;
        match Server::start(&slot.config, &slot.program, &slot.root, path_env.as_deref(), ctx) {
            Ok(s) => slot.server = Some(s),
            Err(e) => {
                // Like a crash: tried again after a pause, up to the limit.
                slot.last_error = Some(e.clone());
                self.crashed(i, e);
            }
        }
    }

    /// A server stopped on its own: schedule a restart, or give up.
    fn crashed(&mut self, i: usize, why: String) {
        let now = Instant::now();
        let slot = &mut self.slots[i];
        slot.server = None;
        slot.crashes.retain(|t| now.duration_since(*t) < CRASH_WINDOW);
        slot.crashes.push(now);
        let name = slot.config.display_name();
        if slot.crashes.len() >= MAX_CRASHES {
            slot.failed = Some(why.clone());
            let text = format!("{name} keeps stopping ({why}). LSP: Restart Language Servers tries again");
            self.note(text);
        } else {
            let pause = (FIRST_PAUSE * 2u32.pow(slot.crashes.len() as u32 - 1)).min(MAX_PAUSE);
            slot.retry_at = Some(now + pause);
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(pause);
            }
        }
        // Its files go to the next process when it starts.
        self.docs.retain(|_, d| d.slot != i);
    }

    /// Notice servers that stopped and restart the ones whose pause is over.
    pub fn tick(&mut self) {
        if !self.on() {
            return;
        }
        let now = Instant::now();
        for i in 0..self.slots.len() {
            let stopped = match self.slots[i].server.as_ref().map(Server::status) {
                Some(Status::Stopped(why)) => Some(why),
                _ => None,
            };
            if let Some(why) = stopped {
                self.slots[i].last_error = Some(why.clone());
                self.crashed(i, why);
            }
            let slot = &self.slots[i];
            if slot.server.is_none() && slot.failed.is_none() && slot.retry_at.is_some_and(|t| t <= now) {
                self.start(i);
            }
        }
    }

    /// Keep the server in step with a window's tab showing `path`: open it
    /// the first time, send its text when `text_key` changes (`text` makes
    /// the snapshot only then), and `didSave` when the file's
    /// modification time changes. Tabs the server doesn't handle, and a
    /// file another window already told the server about, are skipped.
    pub fn sync(&mut self, window: u64, path: &Path, text_key: (u64, u64), text: impl FnOnce() -> Rope, mtime: Option<SystemTime>, roots: &[PathBuf]) {
        if !self.on() {
            return;
        }
        let path = uri::normalize(path);
        if self.docs.get(&path).is_some_and(|d| d.window != window) {
            return;
        }
        let Some(i) = self.slot_for(&path, roots) else { return };
        let slot = &self.slots[i];
        let Some(server) = slot.server.as_ref().filter(|s| !matches!(s.status(), Status::Stopped(_))) else { return };
        match self.docs.get_mut(&path) {
            Some(d) if d.slot == i && d.run == slot.run => {
                if d.text_key != text_key {
                    d.text_key = text_key;
                    d.version += 1;
                    server.change(&path, d.version, text());
                }
                if d.mtime != mtime {
                    d.mtime = mtime;
                    if mtime.is_some() {
                        server.saved(&path);
                    }
                }
            }
            _ => {
                let language = slot.config.language_id.clone().unwrap_or_else(|| config::language_id(&path, &slot.config.name));
                server.open(&path, language, 1, text());
                self.docs.insert(path, OpenDoc { slot: i, run: slot.run, version: 1, text_key, window, mtime });
            }
        }
    }

    /// After a window synced its tabs: files of that window that aren't
    /// open anymore are closed in their servers.
    pub fn end_frame(&mut self, window: u64, open: &[PathBuf]) {
        let open: Vec<PathBuf> = open.iter().map(|p| uri::normalize(p)).collect();
        let gone: Vec<PathBuf> = self.docs.iter().filter(|(p, d)| d.window == window && !open.contains(p)).map(|(p, _)| p.clone()).collect();
        for p in gone {
            self.close(&p);
        }
    }

    /// A window closed: its files are closed in their servers.
    pub fn forget_window(&mut self, window: u64) {
        self.end_frame(window, &[]);
    }

    fn close(&mut self, path: &Path) {
        if let Some(d) = self.docs.remove(path)
            && let Some(slot) = self.slots.get(d.slot)
            && slot.run == d.run
            && let Some(s) = &slot.server
        {
            s.close(path);
        }
    }

    /// The running server that has `path` open for this window, and the
    /// document version it was last sent.
    pub fn server_for(&self, window: u64, path: &Path) -> Option<(&Server, i32)> {
        let d = self.docs.get(&uri::normalize(path)).filter(|d| d.window == window)?;
        let slot = self.slots.get(d.slot).filter(|s| s.run == d.run)?;
        Some((slot.server.as_ref().filter(|s| s.is_ready())?, d.version))
    }

    /// The latest diagnostics for an open file, from its server.
    pub fn diagnostics(&self, window: u64, path: &Path) -> Option<DiagSet> {
        let path = uri::normalize(path);
        let d = self.docs.get(&path).filter(|d| d.window == window)?;
        let slot = self.slots.get(d.slot).filter(|s| s.run == d.run)?;
        slot.server.as_ref()?.diagnostics(&path)
    }

    /// Every file with diagnostics, from every server, with the position
    /// encoding of the server that sent them.
    pub fn all_diagnostics(&self) -> Vec<(PathBuf, Arc<Vec<Diagnostic>>, Encoding)> {
        let mut out = Vec::new();
        for s in self.slots.iter().filter_map(|s| s.server.as_ref()) {
            let enc = s.caps().encoding;
            out.extend(s.all_diagnostics().into_iter().map(|(p, d)| (p, d, enc)));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The server for a file and how it's doing, for the status bar (None:
    /// no server handles it, or LSP is off).
    pub fn state_for(&self, path: &Path) -> Option<(String, State)> {
        if !self.on() {
            return None;
        }
        let i = (*self.assigned.get(&uri::normalize(path))?)?;
        let slot = self.slots.get(i)?;
        let name = slot.config.display_name();
        let state = match (&slot.server, &slot.failed) {
            (_, Some(why)) => State::Failed(why.clone()),
            (Some(s), None) => match s.status() {
                Status::Starting => State::Starting,
                Status::Ready => s.progress().map_or(State::Ready, State::Busy),
                Status::Stopped(why) => State::Restarting { why, secs: 0 },
            },
            (None, None) => {
                let secs = slot.retry_at.map_or(0, |t| t.saturating_duration_since(Instant::now()).as_secs_f32().ceil() as u64);
                State::Restarting { why: slot.last_error.clone().unwrap_or_default(), secs }
            }
        };
        Some((name, state))
    }

    /// The process id of the server handling `path` (tests).
    #[cfg(test)]
    pub fn pid_for(&self, path: &Path) -> Option<u32> {
        let i = (*self.assigned.get(&uri::normalize(path))?)?;
        self.slots[i].server.as_ref()?.pid()
    }
}

/// Wait for a server we asked to stop, off the UI thread; end it if it dawdles.
fn reap(s: Server) {
    let _ = std::thread::Builder::new().name("lsp-reap".into()).spawn(move || {
        if !s.wait_exit(Instant::now() + QUIT_WAIT) {
            s.kill();
        }
    });
}

impl Drop for Hub {
    /// Quitting: ask every server to shut down, give them a moment
    /// together, then end the ones still running.
    fn drop(&mut self) {
        let servers: Vec<Server> = self.slots.iter_mut().filter_map(|s| s.server.take()).collect();
        for s in &servers {
            s.stop();
        }
        let deadline = Instant::now() + QUIT_WAIT;
        for s in &servers {
            if !s.wait_exit(deadline) {
                s.kill();
            }
        }
    }
}
