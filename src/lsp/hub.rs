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

/// Where trusted project folders are remembered, in Strata's config folder.
pub const TRUST_FILE: &str = "lsp_trusted_folders.json";

/// An answer to "Start language servers for this folder?".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// This time (until Strata quits).
    Once,
    /// And remember the folder.
    Always,
    /// Not this session.
    NotNow,
}

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
    /// Not started: the project folder isn't trusted (yet).
    NotTrusted,
}

struct Slot {
    config: ServerConfig,
    /// The project folder's real path, where the server runs.
    root: PathBuf,
    program: PathBuf,
    server: Option<Server>,
    /// Bumped each time a process starts: open files are sent to it again.
    run: u64,
    crashes: Vec<Instant>,
    retry_at: Option<Instant>,
    last_error: Option<String>,
    failed: Option<String>,
    /// Some process of it answered `initialize`: it can work here.
    worked: bool,
}

/// A file open in a server: one per real file, whatever paths and windows
/// its tabs reach it by.
struct OpenDoc {
    slot: usize,
    run: u64,
    /// The document version last sent (the protocol's, counting from 1).
    version: i32,
    /// (editor id, editor version) of the text last sent.
    text_key: (u64, u64),
    /// Modification time of the file when last seen: a change means it was saved.
    mtime: Option<SystemTime>,
    /// The tab speaking for it just changed: until that tab's text has gone
    /// out, nothing about the document is answered (see `handoffs`).
    handoff: bool,
    /// The tabs showing it: (window, the tab's path). The first one speaks
    /// for the file: its text is what the server has, and only it gets the
    /// server's answers. When it closes, the next one takes over.
    tabs: Vec<(u64, PathBuf)>,
}

/// A tab's file as servers know it.
#[derive(Clone)]
struct Resolved {
    /// The key of the document it belongs to: the file's real path (links
    /// resolved, the last one too), or another name of the same file (a
    /// hard link) that a tab opened first.
    doc: PathBuf,
    /// The file's modification time when last looked at: a change means
    /// it was saved, reloaded or changed on disk, so it's looked at again.
    seen: Option<SystemTime>,
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
    /// Start servers in any folder without asking (`lsp_trust_all`).
    trust_all: bool,
    /// Project folders whose servers may run: remembered ones and this session's.
    trusted: Vec<PathBuf>,
    /// Folders the user said Not Now to, this session.
    declined: Vec<PathBuf>,
    /// Folders waiting for an answer: (folder, server names, the window asking).
    asking: Vec<(PathBuf, Vec<String>, u64)>,
    /// Files whose server waits on their folder's trust: (folder, server name).
    waiting: HashMap<PathBuf, (PathBuf, String)>,
    /// The file remembered folders are kept in (None in tests: nothing is written).
    trust_file: Option<PathBuf>,
    /// The real file behind each tab's path (worked out once per tab).
    resolved: HashMap<PathBuf, Resolved>,
    /// See `handoffs`.
    handoffs: u64,
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
            trust_all: false,
            trusted: Vec::new(),
            declined: Vec::new(),
            asking: Vec::new(),
            waiting: HashMap::new(),
            trust_file: None,
            resolved: HashMap::new(),
            handoffs: 0,
        }
    }

    /// Remember trusted folders in `dir` (Strata's config folder), and read
    /// the ones remembered before.
    pub fn set_config_dir(&mut self, dir: &Path) {
        let file = dir.join(TRUST_FILE);
        if let Ok(text) = std::fs::read_to_string(&file) {
            match serde_json::from_str::<Vec<PathBuf>>(&text) {
                // A folder that no longer resolves isn't trusted.
                Ok(list) => self.trusted = list.iter().filter_map(|p| real_path(p)).collect(),
                Err(e) => self.note(format!("{TRUST_FILE}: {e}")),
            }
        }
        self.trust_file = Some(file);
    }

    /// `real` is a project folder's real path (links resolved); it's
    /// compared with the trusted folders' real paths component by component,
    /// so a link inside a trusted folder to a project elsewhere isn't trusted.
    fn may_run(&self, real: &Path) -> bool {
        self.trust_all || self.trusted.iter().any(|t| real.starts_with(t))
    }

    /// The folder this window should ask about, and the servers waiting on it.
    pub fn trust_question(&self, window: u64) -> Option<(PathBuf, Vec<String>)> {
        self.asking.iter().find(|a| a.2 == window).map(|a| (a.0.clone(), a.1.clone()))
    }

    /// The answer to `trust_question` for `root`.
    pub fn answer_trust(&mut self, root: &Path, answer: Trust) {
        self.asking.retain(|a| a.0 != root);
        match answer {
            Trust::Once | Trust::Always => match real_path(root) {
                Some(real) => {
                    if answer == Trust::Always {
                        self.save_trusted(&real);
                    }
                    self.trusted.push(real);
                }
                // Trust needs a real folder: without one nothing starts (and isn't asked again).
                None => {
                    self.note(format!("Language servers not started: the folder {} can't be found", root.display()));
                    self.declined.push(root.to_owned());
                }
            },
            Trust::NotNow => self.declined.push(root.to_owned()),
        }
        // Its files are looked at again on the next sync.
        self.waiting.retain(|_, w| w.0 != root);
    }

    /// Add `root` to the remembered folders (re-read first, so other Strata
    /// processes' additions aren't lost).
    fn save_trusted(&mut self, root: &Path) {
        let Some(file) = self.trust_file.clone() else { return };
        let mut list: Vec<PathBuf> = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if !list.iter().any(|p| p == root) {
            list.push(root.to_owned());
        }
        let text = serde_json::to_string_pretty(&list).unwrap_or_default() + "\n";
        let written = file.parent().map_or(Ok(()), crate::private::create_dir_all).and_then(|_| crate::buffer::write_atomic(&file, |out| std::io::Write::write_all(out, text.as_bytes())));
        if let Err(e) = written {
            self.note(format!("Couldn't remember the folder in {}: {e}", file.display()));
        }
    }

    /// Ask again about the folder of `path` (after Not Now: the status bar was clicked).
    pub fn ask_again(&mut self, path: &Path) {
        let path = uri::normalize(path);
        if let Some((root, _)) = self.waiting.get(&path).cloned() {
            self.declined.retain(|d| *d != root);
            self.waiting.retain(|_, w| w.0 != root);
        }
    }

    /// Ask before starting servers (tests; `use_servers` trusts every folder).
    #[cfg(test)]
    pub fn require_trust(&mut self) {
        self.trust_all = false;
    }

    /// The UI context (for worker threads to ask for a repaint).
    pub fn context(&self) -> Option<egui::Context> {
        self.ctx.clone()
    }

    pub fn set_context(&mut self, ctx: &egui::Context) {
        if self.ctx.is_none() {
            self.ctx = Some(ctx.clone());
        }
    }

    /// Follow the `lsp_enabled` and `lsp_servers` settings. A change stops
    /// the running servers; files start the right ones again as they're seen.
    pub fn configure(&mut self, enabled: bool, user: &BTreeMap<String, ServerSetting>, trust_all: bool) {
        if trust_all != self.trust_all {
            self.trust_all = trust_all;
            self.waiting.clear();
            self.asking.clear();
        }
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
        self.trust_all = true;
    }

    /// Use the configured (real) servers, as Strata does with a settings folder (tests).
    #[cfg(test)]
    pub fn activate(&mut self) {
        self.active = true;
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
        self.waiting.clear();
        self.asking.clear();
    }

    /// LSP: Restart Language Servers: stop them all and start afresh,
    /// including servers that gave up and programs installed since.
    pub fn restart_all(&mut self) {
        self.stop_all();
        self.said.clear();
        self.declined.clear();
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
    /// None also while the file's folder waits to be trusted.
    fn slot_for(&mut self, path: &Path, roots: &[PathBuf], window: u64) -> Option<usize> {
        if let Some(a) = self.assigned.get(path) {
            return *a;
        }
        if self.waiting.contains_key(path) {
            return None;
        }
        match self.find_slot(path, roots, window) {
            Ok(found) => {
                self.assigned.insert(path.to_owned(), found);
                found
            }
            Err(Untrusted) => None,
        }
    }

    fn find_slot(&mut self, path: &Path, roots: &[PathBuf], window: u64) -> Result<Option<usize>, Untrusted> {
        let path_env = self.path_env.clone().or_else(|| std::env::var_os("PATH")).unwrap_or_default();
        // The first server for the file type whose program is there.
        let found = self.configs.iter().filter(|c| c.handles(path)).find_map(|c| config::find_executable(&c.command, &path_env).map(|p| (c.clone(), p))).or_else(|| {
            // A server the user set up whose program is missing: worth saying (built-ins aren't).
            let wanted = self.configs.iter().find(|c| c.handles(path) && !c.builtin).cloned();
            if let Some(c) = wanted {
                self.note(format!("Language server “{}” not found (lsp_servers ▸ {})", c.command, c.name));
            }
            None
        });
        let Some((config, program)) = found else { return Ok(None) };
        let root = config::find_root(path, &config.root_markers, roots);
        // The folder's real path: what's checked is what the server runs in
        // (its working folder and rootUri), so swapping a link afterwards
        // can't point it elsewhere. No real path, no trust.
        let real = real_path(&root);
        if let Some(r) = &real
            && let Some(i) = self.slots.iter().position(|s| s.config.name == config.name && s.root == *r)
        {
            return Ok(Some(i));
        }
        // Servers can run a project's code (build scripts, proc macros,
        // configured tools): only in folders the user trusts.
        let Some(real) = real.filter(|r| self.may_run(r)) else {
            let name = config.display_name();
            self.waiting.insert(path.to_owned(), (root.clone(), name.clone()));
            if !self.declined.contains(&root) {
                match self.asking.iter_mut().find(|a| a.0 == root) {
                    Some(a) if !a.1.contains(&name) => a.1.push(name),
                    Some(_) => {}
                    None => self.asking.push((root, vec![name], window)),
                }
                if let Some(ctx) = &self.ctx {
                    ctx.request_repaint();
                }
            }
            return Err(Untrusted);
        };
        self.slots.push(Slot { config, root: real, program, server: None, run: 0, crashes: Vec::new(), retry_at: None, last_error: None, failed: None, worked: false });
        let i = self.slots.len() - 1;
        self.start(i);
        Ok(Some(i))
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
                slot.last_error = Some(e.clone());
                self.stopped(i, e);
            }
        }
    }

    /// A server's process ended (or never started) without being asked to.
    /// One that worked before is restarted; one that never got as far as
    /// answering `initialize` (a program that isn't really a server, a
    /// rustup stub without its component) would only fail again.
    fn stopped(&mut self, i: usize, why: String) {
        let slot = &mut self.slots[i];
        slot.worked |= slot.server.as_ref().is_some_and(Server::was_ready);
        if slot.worked {
            return self.crashed(i, why);
        }
        slot.server = None;
        slot.failed = Some(why.clone());
        let name = slot.config.display_name();
        self.note(format!("{name} couldn't start: {why}"));
        self.docs.retain(|_, d| d.slot != i);
    }

    /// A server that worked stopped on its own: schedule a restart, or give up.
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
                self.stopped(i, why);
            }
            let slot = &self.slots[i];
            if slot.server.is_none() && slot.failed.is_none() && slot.retry_at.is_some_and(|t| t <= now) {
                self.start(i);
            }
        }
    }

    /// Keep the server in step with a window's tab showing `path`: open the
    /// file the first time any tab shows it, send this tab's text when
    /// `text_key` changes (`text` makes the snapshot only then) if it's the
    /// tab that speaks for the file, and `didSave` when the file's
    /// modification time changes. Tabs the server doesn't handle are skipped.
    pub fn sync(&mut self, window: u64, path: &Path, text_key: (u64, u64), text: impl FnOnce() -> Rope, mtime: Option<SystemTime>, roots: &[PathBuf]) {
        if !self.on() {
            return;
        }
        let tab = uri::normalize(path);
        let Some(i) = self.slot_for(&tab, roots, window) else { return };
        // Saved, reloaded or changed on disk: is it still its document's file?
        if let Some(r) = self.resolved.get(&tab)
            && r.seen != mtime
        {
            let doc = r.doc.clone();
            self.recheck_doc(&doc);
            if let Some(r) = self.resolved.get_mut(&tab) {
                r.seen = mtime;
            }
        }
        let r = self.resolve(&tab, mtime);
        let me = (window, tab.clone());
        if let Some(d) = self.docs.get_mut(&r.doc)
            && !d.tabs.contains(&me)
        {
            d.tabs.push(me.clone());
        }
        let slot = &self.slots[i];
        let Some(server) = slot.server.as_ref().filter(|s| !matches!(s.status(), Status::Stopped(_))) else { return };
        match self.docs.get_mut(&r.doc) {
            // Another tab (another path to the same file, or another window) speaks for it.
            Some(d) if d.tabs.first() != Some(&me) => {}
            Some(d) if d.slot == i && d.run == slot.run => {
                if d.text_key != text_key {
                    d.text_key = text_key;
                    d.version += 1;
                    server.change(&r.doc, d.version, text());
                }
                d.handoff = false;
                if d.mtime != mtime {
                    d.mtime = mtime;
                    if mtime.is_some() {
                        server.saved(&r.doc);
                    }
                }
            }
            _ => {
                let language = slot.config.language_id.clone().unwrap_or_else(|| config::language_id(&r.doc, &slot.config.name));
                server.open(&r.doc, language, 1, text());
                let tabs = self.docs.remove(&r.doc).map_or_else(|| vec![me.clone()], |d| d.tabs);
                self.docs.insert(r.doc.clone(), OpenDoc { slot: i, run: slot.run, version: 1, text_key, mtime, handoff: false, tabs });
            }
        }
    }

    /// The real file behind a tab's path, worked out once while the tab is open.
    fn resolve(&mut self, tab: &Path, mtime: Option<SystemTime>) -> Resolved {
        if let Some(r) = self.resolved.get(tab) {
            return r.clone();
        }
        let real = real_path(tab)
            .or_else(|| path_parent_real(tab))
            .map_or_else(|| tab.to_owned(), |p| uri::normalize(&p));
        // Already open under another name for the same file (a hard link)?
        let doc = if self.docs.contains_key(&real) { real.clone() } else { self.doc_key_by_identity(tab).unwrap_or_else(|| real.clone()) };
        let r = Resolved { doc, seen: mtime };
        self.resolved.insert(tab.to_owned(), r.clone());
        r
    }

    /// After a window synced its tabs: its tabs that aren't open anymore
    /// let go of their files; a file no tab shows is closed in its server.
    pub fn end_frame(&mut self, window: u64, open: &[PathBuf]) {
        let open: Vec<PathBuf> = open.iter().map(|p| uri::normalize(p)).collect();
        let mut gone = Vec::new();
        let mut handed_off = false;
        for (real, d) in self.docs.iter_mut() {
            let before = d.tabs.first().cloned();
            d.tabs.retain(|(w, t)| *w != window || open.contains(t));
            if d.tabs.is_empty() {
                gone.push(real.clone());
            } else if d.tabs.first() != before.as_ref() {
                // A new tab speaks for the file, maybe with other text: its
                // text goes out on its next sync, and until then nothing is
                // answered; answers asked for by the old tab are void.
                d.text_key = (0, 0);
                d.handoff = true;
                handed_off = true;
            }
        }
        if handed_off {
            self.handoffs += 1;
        }
        for real in gone {
            self.close(&real);
        }
        let shown: Vec<PathBuf> = self.docs.values().flat_map(|d| d.tabs.iter().map(|t| t.1.clone())).collect();
        self.resolved.retain(|tab, _| shown.contains(tab) || open.contains(tab));
    }

    /// A window closed: its files are closed in their servers.
    pub fn forget_window(&mut self, window: u64) {
        self.end_frame(window, &[]);
        // Its question goes; another window's files ask again.
        let roots: Vec<PathBuf> = self.asking.iter().filter(|a| a.2 == window).map(|a| a.0.clone()).collect();
        self.asking.retain(|a| a.2 != window);
        self.waiting.retain(|_, w| !roots.contains(&w.0));
    }

    fn close(&mut self, real: &Path) {
        if let Some(d) = self.docs.remove(real)
            && let Some(slot) = self.slots.get(d.slot)
            && slot.run == d.run
            && let Some(s) = &slot.server
        {
            s.close(real);
        }
    }

    /// The name the server knows a tab's file by: its real path.
    pub fn server_path(&self, path: &Path) -> PathBuf {
        let tab = uri::normalize(path);
        self.resolved.get(&tab).map_or(tab, |r| r.doc.clone())
    }

    /// Times a document passed to another tab; an answer asked for before
    /// the count changed may be for another text, so it's dropped.
    pub fn handoffs(&self) -> u64 {
        self.handoffs
    }

    /// The open document that is the same file as `path` (identities
    /// stat'ed now: a save replaces a file, and an old identity can be reused).
    fn doc_key_by_identity(&self, path: &Path) -> Option<PathBuf> {
        let id = file_key(path)?;
        self.docs.keys().find(|k| file_key(k) == Some(id)).cloned()
    }

    /// A tab showing the file a server named, in `window` if it has one;
    /// for a file no tab shows, its real path (never rebuilt from a folder
    /// name a link could point elsewhere by now).
    pub fn client_path(&self, window: u64, path: &Path) -> PathBuf {
        match self.client_paths(window, &[path.to_owned()]).pop() {
            Some(Mapped::Tab { path, .. }) | Some(Mapped::Unopened(path)) => path,
            None => uri::normalize(path),
        }
    }

    /// Where each of a server's paths leads (see `Mapped`). Exact names
    /// cost nothing; for the others, each open document's file and each
    /// distinct path are stat'ed once.
    pub fn client_paths(&self, window: u64, paths: &[PathBuf]) -> Vec<Mapped> {
        map_paths(&self.doc_tabs(), window, paths)
    }

    /// Every open document's key and its tabs, for mapping paths (here or on a worker).
    pub fn doc_tabs(&self) -> DocTabs {
        self.docs.iter().map(|(k, d)| (k.clone(), d.tabs.clone())).collect()
    }

    /// Distinct paths among `paths` that aren't an open document's exact
    /// name (each would need a stat to map).
    pub fn count_inexact(&self, paths: &[PathBuf]) -> usize {
        let mut seen = std::collections::HashSet::new();
        paths.iter().map(|p| uri::normalize(p)).filter(|p| !self.docs.contains_key(p) && seen.insert(p.clone())).count()
    }

    /// Look again at which file every tab is (the window came back to the front).
    pub fn recheck_files(&mut self) {
        let keys: Vec<PathBuf> = self.docs.keys().cloned().collect();
        for k in keys {
            self.recheck_doc(&k);
        }
    }

    /// Tabs of a document whose file isn't the document's file any more
    /// (another program replaced one of two hard-linked names, so they're
    /// two files now) leave it: their next sync gives them their own.
    fn recheck_doc(&mut self, key: &Path) {
        let Some(d) = self.docs.get(key) else { return };
        let key_id = file_key(key);
        let leaving: Vec<(u64, PathBuf)> = d
            .tabs
            .iter()
            .filter(|(_, t)| {
                let real = real_path(t).map(|r| uri::normalize(&r));
                real.as_deref() != Some(key) && (key_id.is_none() || file_key(t) != key_id)
            })
            .cloned()
            .collect();
        if leaving.is_empty() {
            return;
        }
        for (_, t) in &leaving {
            self.resolved.remove(t);
        }
        let d = self.docs.get_mut(key).expect("checked");
        let before = d.tabs.first().cloned();
        d.tabs.retain(|t| !leaving.contains(t));
        if d.tabs.is_empty() {
            self.close(key);
        } else if d.tabs.first() != before.as_ref() {
            d.text_key = (0, 0);
            d.handoff = true;
            self.handoffs += 1;
        }
    }

    /// The tab's file is open in a server through another tab (another
    /// window, or another path to it), which the server follows.
    pub fn follows_other_tab(&self, window: u64, path: &Path) -> bool {
        let tab = uri::normalize(path);
        self.resolved.get(&tab).and_then(|r| self.docs.get(&r.doc)).is_some_and(|d| d.tabs.first().is_some_and(|t| *t != (window, tab.clone())))
    }

    /// The tab's document when this tab speaks for it.
    fn doc_of_tab(&self, window: u64, path: &Path) -> Option<(&OpenDoc, &Path)> {
        let tab = uri::normalize(path);
        let r = self.resolved.get(&tab)?;
        let d = self.docs.get(&r.doc).filter(|d| !d.handoff && d.tabs.first().is_some_and(|t| t.0 == window && t.1 == tab))?;
        Some((d, &r.doc))
    }

    /// The running server that has `path` open for this window (its tab
    /// speaking for the file), and the document version it was last sent.
    pub fn server_for(&self, window: u64, path: &Path) -> Option<(&Server, i32)> {
        let (d, _) = self.doc_of_tab(window, path)?;
        let slot = self.slots.get(d.slot).filter(|s| s.run == d.run)?;
        Some((slot.server.as_ref().filter(|s| s.is_ready())?, d.version))
    }

    /// The latest diagnostics for an open file, from its server.
    pub fn diagnostics(&self, window: u64, path: &Path) -> Option<DiagSet> {
        let (d, real) = self.doc_of_tab(window, path)?;
        let slot = self.slots.get(d.slot).filter(|s| s.run == d.run)?;
        slot.server.as_ref()?.diagnostics(real)
    }

    /// Every file with diagnostics, from every server (named as `window`'s
    /// tabs name them), with the position encoding of the server that sent them.
    pub fn all_diagnostics(&self, window: u64) -> Vec<(PathBuf, Arc<Vec<Diagnostic>>, Encoding)> {
        let mut out = Vec::new();
        for s in self.slots.iter().filter_map(|s| s.server.as_ref()) {
            let enc = s.caps().encoding;
            let found = s.all_diagnostics();
            let paths: Vec<PathBuf> = found.iter().map(|(p, _)| p.clone()).collect();
            // Many files nobody has open: named as the server names them
            // (a click maps the one chosen), rather than stat'ing them all here.
            let names: Vec<PathBuf> = if self.count_inexact(&paths) > MAX_INEXACT {
                paths
            } else {
                self.client_paths(window, &paths).into_iter().map(|m| match m {
                    Mapped::Tab { path, .. } | Mapped::Unopened(path) => path,
                }).collect()
            };
            out.extend(names.into_iter().zip(found).map(|(p, (_, d))| (p, d, enc)));
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
        if let Some((_, name)) = self.waiting.get(&uri::normalize(path)) {
            return Some((name.clone(), State::NotTrusted));
        }
        let i = (*self.assigned.get(&uri::normalize(path))?)?;
        let slot = self.slots.get(i)?;
        // A built-in server that never started is as good as not installed:
        // said once in the status message, not shown on every file.
        if slot.config.builtin && !slot.worked && slot.failed.is_some() {
            return None;
        }
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

    /// The folder the server handling `path` runs in (tests).
    #[cfg(all(test, unix))]
    pub fn root_for(&self, path: &Path) -> Option<PathBuf> {
        let i = (*self.assigned.get(&uri::normalize(path))?)?;
        Some(self.slots[i].root.clone())
    }

    /// The process id of the server handling `path` (tests).
    #[cfg(test)]
    pub fn pid_for(&self, path: &Path) -> Option<u32> {
        let i = (*self.assigned.get(&uri::normalize(path))?)?;
        self.slots[i].server.as_ref()?.pid()
    }
}

/// `path` with links resolved; None if it can't be (it's gone). On
/// Windows without the `\\?\` prefix canonicalizing adds (URIs and
/// servers want plain drive paths).
fn real_path(path: &Path) -> Option<PathBuf> {
    crate::paths::canonical(path).ok()
}

/// A missing file's real path: its folder's, plus its name.
fn path_parent_real(path: &Path) -> Option<PathBuf> {
    Some(real_path(path.parent()?)?.join(path.file_name()?))
}

/// Which file `path` is, following links: device and inode, or on Windows
/// volume serial number and file index.
fn file_key(path: &Path) -> Option<(u64, u64)> {
    #[cfg(test)]
    IDENTITY_STATS.with(|c| c.set(c.get() + 1));
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(path).ok()?;
        Some((m.dev(), m.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
        let file = std::fs::File::open(path).ok()?;
        // SAFETY: plain data, filled in by the call on a valid handle.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
            return None;
        }
        Some((info.dwVolumeSerialNumber as u64, (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

/// More paths needing a stat than this, and the mapping isn't done on the UI thread.
pub const MAX_INEXACT: usize = 200;

/// Open documents' keys and their tabs (see `Hub::doc_tabs`).
pub type DocTabs = Vec<(PathBuf, Vec<(u64, PathBuf)>)>;

/// Where each path from a server leads, given the open documents: by exact
/// name, else by identity (a link, another spelling or case, a hard link).
/// Each document's file and each distinct path are stat'ed at most once,
/// and only if some path isn't an exact name. Runs on any thread.
pub fn map_paths(docs: &DocTabs, window: u64, paths: &[PathBuf]) -> Vec<Mapped> {
    let mut ids: Option<HashMap<(u64, u64), usize>> = None;
    let mut seen: HashMap<PathBuf, Mapped> = HashMap::new();
    let tab_of = |k: usize| {
        let tabs = &docs[k].1;
        let t = tabs.iter().find(|t| t.0 == window).or(tabs.first()).expect("a document has tabs");
        Mapped::Tab { window: t.0, path: t.1.clone() }
    };
    paths
        .iter()
        .map(|p| {
            let n = uri::normalize(p);
            if let Some(m) = seen.get(&n) {
                return m.clone();
            }
            let doc = docs.iter().position(|(k, _)| *k == n).or_else(|| {
                let ids = ids.get_or_insert_with(|| docs.iter().enumerate().filter_map(|(k, (key, _))| file_key(key).map(|id| (id, k))).collect());
                file_key(&n).and_then(|id| ids.get(&id).copied())
            });
            let m = match doc {
                Some(k) => tab_of(k),
                None => Mapped::Unopened(real_path(&n).map_or_else(|| n.clone(), |r| uri::normalize(&r))),
            };
            seen.insert(n, m.clone());
            m
        })
        .collect()
}

/// Where a path from a server leads.
#[derive(Clone, Debug, PartialEq)]
pub enum Mapped {
    /// A tab shows that file (in `window`).
    Tab { window: u64, path: PathBuf },
    /// No tab shows it: its real path.
    Unopened(PathBuf),
}

#[cfg(test)]
thread_local! {
    static IDENTITY_STATS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Identity lookups since `reset_identity_stats` on this thread (tests).
#[cfg(test)]
pub fn identity_stats() -> usize {
    IDENTITY_STATS.with(|c| c.get())
}

#[cfg(test)]
pub fn reset_identity_stats() {
    IDENTITY_STATS.with(|c| c.set(0));
}

/// A server that waits for its folder to be trusted.
struct Untrusted;

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
