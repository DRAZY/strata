//! Every installed plugin and what it's doing: starting one when something
//! it waits for happens (one of its commands, a matching file opened or
//! saved, Strata starting), asking for consent first, telling a running one
//! about documents and settings, restarting one that crashes (at most three
//! times in five minutes, waiting longer each time), and routing what it
//! asks for to the window that can answer. Shared by every window, like the
//! language servers; nothing here waits on a plugin.

use super::api::{self, BUSY, RpcError, Reply, TIMED_OUT};
use super::host::{self, Host, Incoming, Launch, Log, Pending, lock};
use super::manifest::Manifest;
use super::store::Store;
use super::{API, Perm, package};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Crashes within `CRASH_WINDOW` that make Strata give up restarting.
pub const MAX_CRASHES: usize = 3;
const CRASH_WINDOW: Duration = Duration::from_secs(300);
/// Pause before the first restart; doubled for each crash since.
const FIRST_PAUSE: Duration = Duration::from_secs(1);
/// `document/changed` at most this often per document (typing is fast).
const CHANGE_EVERY: Duration = Duration::from_millis(150);
/// Messages kept waiting for a window, per plugin.
const INBOX: usize = 64;
/// Bytes of requests (and of their answers) a window takes on per frame,
/// beyond the first: a plugin reading big documents can't stall drawing.
const FRAME_BYTES: usize = 24 << 20;
pub const STATUS_ITEMS: usize = 4;
const STATUS_TEXT: usize = 60;

/// An open document, as plugins hear of it.
#[derive(Clone, Debug, PartialEq)]
pub struct DocInfo {
    /// The tab's editor, unique for the whole run.
    pub id: u64,
    pub path: Option<PathBuf>,
    pub name: String,
    /// Goes up with every change.
    pub version: u64,
    pub syntax: String,
}

impl DocInfo {
    pub fn to_json(&self) -> Value {
        json!({"id": self.id, "path": self.path, "name": self.name, "version": self.version, "syntax": self.syntax})
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusItem {
    pub plugin: String,
    pub id: String,
    pub text: String,
    pub tooltip: String,
    /// One of the plugin's commands, run on a click.
    pub command: Option<String>,
}

pub enum State {
    Stopped,
    /// Hashing the package (off the window's thread) to see if it's the one consented to.
    Checking(Receiver<Result<(String, Manifest), String>>),
    /// Waiting for the user's answer (about the snapshot named `sha256`).
    Asking { sha256: String, manifest: Box<Manifest> },
    Starting { init: Pending },
    Running,
    Restarting { at: Instant, why: String },
    Failed(String),
}

impl State {
    pub fn label(&self) -> String {
        match self {
            State::Stopped => "Not running".into(),
            State::Checking(_) | State::Starting { .. } => "Starting…".into(),
            State::Asking { .. } => "Waiting for your permission".into(),
            State::Running => "Running".into(),
            State::Restarting { why, at } => format!("Stopped ({why}); restarting in {} s", at.saturating_duration_since(Instant::now()).as_secs() + 1),
            State::Failed(why) => format!("Stopped: {why}"),
        }
    }

    fn busy(&self) -> bool {
        matches!(self, State::Checking(_) | State::Asking { .. } | State::Starting { .. } | State::Running)
    }
}

pub struct Plugin {
    pub name: String,
    pub dir: PathBuf,
    pub manifest: Result<Manifest, String>,
    pub state: State,
    pub log: Arc<Mutex<Log>>,
    host: Option<Host>,
    /// The shared lock on the snapshot it runs from (see `package::open_snapshot`).
    lock: Option<std::fs::File>,
    /// Counts starts, so an answer meant for an earlier process never reaches a new one.
    run: u64,
    crashes: Vec<Instant>,
    /// Commands waiting for the plugin to start: (command id, document).
    queued: Vec<(String, Option<DocInfo>)>,
    pub status: Vec<StatusItem>,
    /// The user said no this session: only an explicit start asks again.
    declined: bool,
    sent_settings: Option<Value>,
    /// Documents it was told about: version, and when.
    told: HashMap<u64, (u64, Instant)>,
    commands: Vec<(Pending, String)>,
    inbox: VecDeque<Incoming>,
    /// Unknown settings already warned about.
    warned: HashSet<String>,
}

impl Plugin {
    fn new(found: package::Found) -> Self {
        Self {
            name: found.name,
            dir: found.dir,
            manifest: found.manifest,
            state: State::Stopped,
            log: Default::default(),
            host: None,
            lock: None,
            run: 0,
            crashes: Vec::new(),
            queued: Vec::new(),
            status: Vec::new(),
            declined: false,
            sent_settings: None,
            told: HashMap::new(),
            commands: Vec::new(),
            inbox: VecDeque::new(),
            warned: HashSet::new(),
        }
    }

    pub fn title(&self) -> String {
        self.manifest.as_ref().map_or_else(|_| self.name.clone(), |m| m.title.clone())
    }

    /// The running process's id (tests).
    #[cfg(test)]
    pub fn pid(&self) -> Option<u32> {
        self.host.as_ref().and_then(Host::pid)
    }

    fn log(&self, line: impl Into<String>) {
        lock(&self.log).push(line);
    }

    /// Log each settings warning once.
    fn warn(&mut self, warnings: Vec<String>) {
        for w in warnings {
            if self.warned.insert(w.clone()) {
                self.log(w);
            }
        }
    }

    /// End the process (if any) and forget what it was told.
    fn drop_host(&mut self) {
        if let Some(h) = self.host.take() {
            h.stop();
        }
        self.lock = None;
        self.status.clear();
        self.told.clear();
        self.commands.clear();
        self.inbox.clear();
        self.sent_settings = None;
    }
}

/// A key a plugin binds, once it's clear the key is free.
#[derive(Clone, Debug, PartialEq)]
pub struct Bound {
    pub plugin: String,
    pub command: String,
    pub mods: egui::Modifiers,
    pub key: egui::Key,
}

/// A plugin key that isn't bound, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    pub plugin: String,
    pub key: String,
    pub why: String,
}

pub struct Manager {
    config: Option<PathBuf>,
    store: Store,
    pub plugins: Vec<Plugin>,
    /// Every window's side bar folders: where plugins may read and write files.
    folders: Arc<Mutex<Vec<PathBuf>>>,
    ctx: Option<egui::Context>,
    path_env: Option<std::ffi::OsString>,
    user_settings: BTreeMap<String, Map<String, Value>>,
    /// Each window's open documents and folders.
    windows: BTreeMap<u64, (Vec<DocInfo>, Vec<PathBuf>)>,
    /// Documents already checked for `open:` activation.
    seen: HashSet<u64>,
    /// The window that answers requests not about one of another window's documents.
    primary: Option<u64>,
    /// Plugins waiting for the consent question (the first is asked).
    asking: VecDeque<String>,
    notes: Vec<String>,
    pub init_timeout: Duration,
    pub command_timeout: Duration,
    /// Added to the environment Strata passes on (tests: what a user's shell might hold).
    #[cfg(test)]
    pub parent_env_for_tests: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

impl Manager {
    pub fn new(config: Option<&Path>, ctx: Option<egui::Context>) -> Self {
        if let Some(c) = config {
            package::tidy(c);
        }
        let mut m = Self {
            config: config.map(Path::to_path_buf),
            store: Store::load(config),
            plugins: Vec::new(),
            folders: Default::default(),
            ctx,
            path_env: crate::build::augmented_path(),
            user_settings: BTreeMap::new(),
            windows: BTreeMap::new(),
            seen: HashSet::new(),
            primary: None,
            asking: VecDeque::new(),
            notes: Vec::new(),
            init_timeout: Duration::from_secs(10),
            command_timeout: Duration::from_secs(30),
            #[cfg(test)]
            parent_env_for_tests: Vec::new(),
        };
        m.reload();
        if let Some(c) = config {
            let keep: HashSet<String> = m.store.state.plugins.values().filter_map(|e| e.grant.as_ref().map(|g| g.sha256.clone())).collect();
            package::prune_snapshots(c, &keep);
        }
        for i in 0..m.plugins.len() {
            if m.plugins[i].manifest.as_ref().is_ok_and(|m| m.activation.startup) {
                m.activate(i, false);
            }
        }
        m
    }

    pub fn set_context(&mut self, ctx: &egui::Context) {
        if self.ctx.is_none() {
            self.ctx = Some(ctx.clone());
        }
    }

    /// Read the installed plugins again (after an install or removal).
    pub fn reload(&mut self) {
        let found = self.config.as_deref().map(package::installed).unwrap_or_default();
        let names: Vec<String> = found.iter().map(|f| f.name.clone()).collect();
        // Plugins gone from disk stop.
        for p in self.plugins.iter_mut().filter(|p| !names.contains(&p.name)) {
            p.drop_host();
        }
        self.plugins.retain(|p| names.contains(&p.name));
        for f in found {
            match self.plugins.iter_mut().find(|p| p.name == f.name) {
                Some(p) => {
                    p.dir = f.dir;
                    p.manifest = f.manifest;
                }
                None => self.plugins.push(Plugin::new(f)),
            }
        }
        self.plugins.sort_by(|a, b| a.name.cmp(&b.name));
    }

    fn index(&self, name: &str) -> Option<usize> {
        self.plugins.iter().position(|p| p.name == name)
    }

    pub fn enabled(&self, name: &str) -> bool {
        self.store.enabled(name)
    }

    /// Status bar messages since the last call.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    fn note(&mut self, text: String) {
        self.notes.push(text);
    }

    // ---- starting ---------------------------------------------------------------------

    /// Start plugin `i` if it isn't running: check its package, ask if the
    /// user hasn't approved this exact one, then run it. `explicit`: the
    /// user asked (a command, Start): it also retries after a refusal or a failure.
    fn activate(&mut self, i: usize, explicit: bool) {
        let p = &mut self.plugins[i];
        if !self.store.enabled(&p.name) {
            if explicit {
                let t = p.title();
                self.note(format!("{t} is turned off; turn it on in the Plugins manager"));
            }
            return;
        }
        if let Err(e) = &p.manifest {
            if explicit {
                let msg = format!("{} can't start: {e}", p.name);
                self.note(msg);
            }
            return;
        }
        // Failed and refused plugins wait for the user; restarts come from `tick`.
        if p.state.busy() || (!explicit && (p.declined || matches!(p.state, State::Failed(_) | State::Restarting { .. }))) {
            return;
        }
        if explicit {
            p.crashes.clear();
        }
        let dir = p.dir.clone();
        let Some(config) = self.config.clone() else {
            p.state = State::Failed("there's no settings folder to keep its approved copy in".into());
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        let job = move || {
            let r = package::snapshot(&config, &dir);
            let _ = tx.send(r);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        };
        if let Err(e) = std::thread::Builder::new().name("plugin-check".into()).spawn(job) {
            p.state = State::Failed(format!("couldn't check the package: {e}"));
            return;
        }
        p.state = State::Checking(rx);
    }

    /// The package was checked: start it, or ask first.
    fn checked(&mut self, i: usize, result: Result<(String, Manifest), String>) {
        let (sha, manifest) = match result {
            Ok(r) => r,
            Err(e) => return self.fail(i, format!("its package can't be used: {e}")),
        };
        let p = &mut self.plugins[i];
        if manifest.name != p.name {
            return self.fail(i, "its manifest was changed to another name".into());
        }
        if self.store.granted(&p.name, &sha, &manifest.permissions) {
            self.start_host(i, manifest, &sha);
        } else {
            p.state = State::Asking { sha256: sha, manifest: Box::new(manifest) };
            let name = p.name.clone();
            if !self.asking.contains(&name) {
                self.asking.push_back(name);
            }
        }
    }

    fn fail(&mut self, i: usize, why: String) {
        let p = &mut self.plugins[i];
        if let Some(h) = &p.host {
            // Whatever state it's in (not answering, say), it's ended now.
            h.kill();
        }
        p.drop_host();
        p.log(format!("Not started: {why}"));
        p.state = State::Failed(why.clone());
        let t = p.title();
        let dropped = std::mem::take(&mut p.queued).len();
        self.note(if dropped > 0 { format!("{t} couldn't run the command: {why}") } else { format!("{t} stopped: {why}") });
    }

    /// The consent question: the plugin, the package's identity and its manifest.
    pub fn question(&self) -> Option<(&Plugin, &str, &Manifest)> {
        self.asking.iter().find_map(|name| {
            let p = self.plugins.iter().find(|p| &p.name == name)?;
            match &p.state {
                State::Asking { sha256, manifest } => Some((p, sha256.as_str(), &**manifest)),
                _ => None,
            }
        })
    }

    /// The user answered the consent question.
    pub fn answer(&mut self, allow: bool) {
        // Plugins removed or stopped while they waited drop out of the line.
        let waiting = |m: &Self, n: &String| m.plugins.iter().any(|p| &p.name == n && matches!(p.state, State::Asking { .. }));
        let keep: VecDeque<String> = self.asking.iter().filter(|n| waiting(self, n)).cloned().collect();
        self.asking = keep;
        let Some(name) = self.asking.pop_front() else { return };
        let Some(i) = self.index(&name) else { return };
        let State::Asking { sha256, manifest } = std::mem::replace(&mut self.plugins[i].state, State::Stopped) else { return };
        if !allow {
            let p = &mut self.plugins[i];
            p.declined = true;
            p.queued.clear();
            p.log("Not started: permission wasn't given");
            let t = p.title();
            self.note(format!("{t} wasn't started"));
            return;
        }
        if let Err(e) = self.store.grant(&name, &sha256, &manifest.permissions) {
            self.note(e);
        }
        self.plugins[i].declined = false;
        self.start_host(i, *manifest, &sha256);
    }

    fn launch(&self, dir: &Path, m: &Manifest) -> Result<Launch, String> {
        #[allow(unused_mut)]
        let mut parent: Vec<(std::ffi::OsString, std::ffi::OsString)> = std::env::vars_os().collect();
        #[cfg(test)]
        parent.extend(self.parent_env_for_tests.iter().cloned());
        let mut env = clean_env(parent);
        if let Some(p) = &self.path_env {
            env.retain(|(k, _)| !k.eq_ignore_ascii_case("PATH"));
            env.push(("PATH".into(), p.clone()));
        }
        #[cfg(test)]
        if m.run.command == super::fake::COMMAND {
            let mut l = super::fake::launch(&m.run.args, dir);
            env.extend(l.env);
            l.env = env;
            return Ok(l);
        }
        let program = if m.run.in_package() {
            let parts = package::command_parts(&m.run.command).ok_or("its command isn't a path inside its folder")?;
            let p = parts.iter().fold(dir.to_path_buf(), |p, s| p.join(s));
            // A plain file really inside the approved copy (the copy has no links, but be sure).
            let inside = match (std::fs::canonicalize(&p), std::fs::canonicalize(dir)) {
                (Ok(real), Ok(top)) => real.starts_with(&top),
                _ => false,
            };
            if !inside || !std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_file()) {
                return Err(format!("{} isn't in its folder", m.run.command));
            }
            p
        } else {
            let path = self.path_env.clone().or_else(|| std::env::var_os("PATH")).unwrap_or_default();
            crate::lsp::config::find_executable(&m.run.command, &path).ok_or_else(|| format!("couldn't find {} (is it installed?)", m.run.command))?
        };
        Ok(Launch { program, args: m.run.args.clone(), cwd: dir.to_path_buf(), env })
    }

    /// Run plugin `i` from its approved copy, checked once more right
    /// before, and locked while it runs (so no Strata prunes it).
    fn start_host(&mut self, i: usize, manifest: Manifest, sha: &str) {
        let Some(config) = self.config.clone() else { return self.fail(i, "there's no settings folder".into()) };
        let (snapshot, snap_lock) = match package::open_snapshot(&config, sha) {
            Ok(s) => s,
            Err(e) => return self.fail(i, e),
        };
        let launch = match self.launch(&snapshot, &manifest) {
            Ok(l) => l,
            Err(e) => return self.fail(i, e),
        };
        let (settings, warnings) = effective_settings(&self.user_settings, &manifest);
        self.plugins[i].warn(warnings);
        let storage = self.config.as_ref().map(|c| c.join(package::DATA_DIR).join(&manifest.name));
        if let Some(s) = &storage {
            let _ = crate::private::create_dir_all(s);
        }
        let p = &mut self.plugins[i];
        p.lock = Some(snap_lock);
        let host = match Host::start(&p.name, &launch, manifest.permissions.clone(), self.folders.clone(), p.log.clone(), self.ctx.clone()) {
            Ok(h) => h,
            Err(e) => return self.fail(i, e),
        };
        let workspace = manifest.permissions.contains(&Perm::ReadWorkspace) || manifest.permissions.contains(&Perm::WriteWorkspace);
        let params = json!({
            "api": API,
            "strata": {"version": env!("CARGO_PKG_VERSION")},
            "plugin": {"name": manifest.name, "version": manifest.version},
            "permissions": manifest.permissions.iter().map(|p| p.id()).collect::<Vec<_>>(),
            "settings": settings,
            "storagePath": storage,
            "workspaceFolders": if workspace { json!(*lock(&self.folders)) } else { Value::Null },
        });
        let init = host.request("initialize", params, self.init_timeout);
        p.run += 1;
        p.log(format!("Starting {} {}", manifest.title, manifest.version));
        p.sent_settings = Some(settings);
        p.host = Some(host);
        p.manifest = Ok(manifest);
        p.state = State::Starting { init };
    }

    /// Start a plugin now (the manager's Start, or a command), asking first if needed.
    pub fn start(&mut self, name: &str) {
        if let Some(i) = self.index(name) {
            self.activate(i, true);
        }
    }

    pub fn stop(&mut self, name: &str) {
        self.halt(name, false);
    }

    /// Stop a plugin; `now`: end its process at once (before its files are
    /// replaced or removed, which Windows won't do while it runs).
    fn halt(&mut self, name: &str, now: bool) {
        let Some(i) = self.index(name) else { return };
        let p = &mut self.plugins[i];
        if now && let Some(h) = &p.host {
            h.kill();
        }
        p.drop_host();
        p.queued.clear();
        p.state = State::Stopped;
        self.asking.retain(|n| n != name);
    }

    pub fn set_enabled(&mut self, name: &str, on: bool) {
        if let Err(e) = self.store.set_enabled(name, on) {
            self.note(e);
        }
        if !on {
            self.stop(name);
        }
    }

    /// Stop and remove an installed plugin (and what was granted to it).
    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        self.halt(name, true);
        let config = self.config.clone().ok_or("there's no settings folder")?;
        package::remove(&config, name)?;
        self.store.forget(name)?;
        self.reload();
        Ok(())
    }

    /// Install a staged package (see `package::stage`), replacing and
    /// stopping an older version first.
    pub fn commit_install(&mut self, staged: package::Staged) -> Result<String, String> {
        let config = self.config.clone().ok_or("there's no settings folder")?;
        let (name, title, version) = (staged.manifest.name.clone(), staged.manifest.title.clone(), staged.manifest.version.clone());
        self.halt(&name, true);
        let updated = self.index(&name).is_some();
        package::commit(&config, staged)?;
        self.reload();
        if let Some(i) = self.index(&name) {
            let p = &mut self.plugins[i];
            p.declined = false;
            p.crashes.clear();
            p.state = State::Stopped;
            if p.manifest.as_ref().is_ok_and(|m| m.activation.startup) {
                self.activate(i, false);
            }
        }
        Ok(if updated { format!("Updated {title} to {version}") } else { format!("Installed {title} {version}") })
    }

    // ---- the state machine ------------------------------------------------------------

    /// Move every plugin along: package checks, `initialize` answers,
    /// crashes and restarts, command answers, document events, settings.
    pub fn tick(&mut self) {
        let now = Instant::now();
        let mut wake: Option<Duration> = None;
        for i in 0..self.plugins.len() {
            self.tick_one(i, now, &mut wake);
        }
        self.document_events(now, &mut wake);
        if let (Some(d), Some(ctx)) = (wake, &self.ctx) {
            ctx.request_repaint_after(d);
        }
    }

    fn tick_one(&mut self, i: usize, now: Instant, wake: &mut Option<Duration>) {
        let mut soon = |d: Duration| *wake = Some(wake.map_or(d, |w| w.min(d)));
        if let State::Checking(rx) = &self.plugins[i].state
            && let Ok(r) = rx.try_recv()
        {
            self.checked(i, r);
        }
        if let Some(h) = &self.plugins[i].host {
            h.expire();
            h.watch();
        }
        let exited = self.plugins[i].host.as_ref().and_then(Host::exited);
        enum Next {
            Nothing,
            Fail(String),
            Started,
            Crashed(String),
            Restart,
        }
        let secs = self.init_timeout.as_secs_f64();
        let next = match &mut self.plugins[i].state {
            State::Starting { init } => match (exited, init.poll()) {
                (Some(why), _) => Next::Fail(format!("it stopped before it started ({why})")),
                (None, None) => {
                    soon(Duration::from_millis(250));
                    Next::Nothing
                }
                (None, Some(Err(e))) if e.code == TIMED_OUT => Next::Fail(format!("it didn't answer within {secs} s of starting")),
                (None, Some(Err(e))) => Next::Fail(format!("it refused to start: {}", e.message)),
                (None, Some(Ok(v))) => match v.get("api").and_then(Value::as_str) {
                    Some(a) if a != API => Next::Fail(format!("it speaks {}, but this Strata speaks {API}", a.chars().take(40).collect::<String>())),
                    _ => Next::Started,
                },
            },
            State::Running => exited.map_or(Next::Nothing, Next::Crashed),
            State::Restarting { at, .. } if *at <= now => Next::Restart,
            State::Restarting { at, .. } => {
                soon(at.saturating_duration_since(now));
                Next::Nothing
            }
            _ => Next::Nothing,
        };
        match next {
            Next::Nothing => {}
            Next::Fail(why) => self.fail(i, why),
            Next::Started => self.started(i),
            Next::Crashed(why) => self.crashed(i, why, now),
            Next::Restart => {
                self.plugins[i].state = State::Stopped;
                self.activate(i, false);
            }
        }
        // Answers to commands.
        let p = &mut self.plugins[i];
        let title = p.title();
        let mut failed = Vec::new();
        p.commands.retain_mut(|(pending, cmd)| match pending.poll() {
            None => true,
            Some(Ok(_)) => false,
            Some(Err(e)) => {
                failed.push(if e.code == TIMED_OUT { format!("{title}: {cmd} didn't finish in time") } else { format!("{title}: {cmd} failed: {}", e.message) });
                false
            }
        });
        if !p.commands.is_empty() {
            soon(Duration::from_millis(250));
        }
        for f in failed {
            self.note(f);
        }
    }

    fn started(&mut self, i: usize) {
        let p = &mut self.plugins[i];
        p.state = State::Running;
        if let Some(h) = &p.host {
            h.notify("initialized", json!({}));
        }
        p.log("Started");
        for (cmd, doc) in std::mem::take(&mut p.queued) {
            self.send_command(i, &cmd, doc.as_ref());
        }
    }

    fn crashed(&mut self, i: usize, why: String, now: Instant) {
        let p = &mut self.plugins[i];
        p.drop_host();
        p.crashes.retain(|t| now.duration_since(*t) < CRASH_WINDOW);
        p.crashes.push(now);
        let t = p.title();
        if p.crashes.len() >= MAX_CRASHES {
            let msg = format!("it stopped {MAX_CRASHES} times in {} minutes, so Strata won't restart it ({why}); start it again from the Plugins manager", CRASH_WINDOW.as_secs() / 60);
            p.log(format!("Not restarted: {msg}"));
            p.state = State::Failed(msg);
            self.note(format!("{t} keeps stopping; Strata won't restart it"));
        } else {
            let pause = FIRST_PAUSE * 2u32.pow(p.crashes.len() as u32 - 1);
            p.log(format!("Restarting in {} s", pause.as_secs()));
            p.state = State::Restarting { at: now + pause, why: why.clone() };
            self.note(format!("{t} stopped ({why}); restarting it"));
        }
    }

    // ---- commands -----------------------------------------------------------------------

    /// Run a plugin's command (starting the plugin first if it isn't running).
    pub fn run_command(&mut self, name: &str, cmd: &str, doc: Option<DocInfo>) {
        let Some(i) = self.index(name) else { return };
        let known = self.plugins[i].manifest.as_ref().is_ok_and(|m| m.commands.iter().any(|c| c.id == cmd));
        if !known {
            return;
        }
        if matches!(self.plugins[i].state, State::Running) {
            self.send_command(i, cmd, doc.as_ref());
        } else {
            self.plugins[i].queued.push((cmd.to_owned(), doc));
            self.activate(i, true);
        }
    }

    fn send_command(&mut self, i: usize, cmd: &str, doc: Option<&DocInfo>) {
        let timeout = self.command_timeout;
        let p = &mut self.plugins[i];
        let title = p.manifest.as_ref().ok().and_then(|m| m.commands.iter().find(|c| c.id == cmd)).map_or_else(|| cmd.to_owned(), |c| c.title.clone());
        // Which file is in front (its path) is the document's business: only for plugins allowed to read or edit it.
        let may_see = p.manifest.as_ref().is_ok_and(|m| m.permissions.contains(&Perm::ReadDocument) || m.permissions.contains(&Perm::EditDocument));
        let Some(h) = &p.host else { return };
        let pending = h.request("command/execute", json!({"command": cmd, "document": doc.filter(|_| may_see).map(DocInfo::to_json)}), timeout);
        p.commands.push((pending, title));
    }

    /// Send any request to a running plugin (tests: the fake plugin's
    /// commands take arguments Strata itself never sends).
    #[cfg(test)]
    pub fn request(&self, name: &str, method: &str, params: Value) -> Option<Pending> {
        let p = &self.plugins[self.index(name)?];
        Some(p.host.as_ref()?.request(method, params, self.command_timeout))
    }

    // ---- documents and windows ----------------------------------------------------------

    /// A window's documents and side bar folders, each frame. The focused
    /// window answers requests that aren't about another window's document.
    pub fn sync_window(&mut self, window: u64, docs: Vec<DocInfo>, folders: Vec<PathBuf>, focused: bool) {
        if focused || self.primary.is_none_or(|p| !self.windows.contains_key(&p)) {
            self.primary = Some(window);
        }
        let changed = self.windows.get(&window).is_none_or(|(_, f)| *f != folders);
        self.windows.insert(window, (docs, folders));
        if changed {
            self.update_folders();
        }
    }

    /// This window shows the consent question and the plugins' messages.
    pub fn is_primary(&self, window: u64) -> bool {
        self.primary == Some(window)
    }

    pub fn forget_window(&mut self, window: u64) {
        self.windows.remove(&window);
        if self.primary == Some(window) {
            self.primary = self.windows.keys().next().copied();
        }
        self.update_folders();
    }

    fn update_folders(&mut self) {
        let mut all: Vec<PathBuf> = Vec::new();
        for (_, folders) in self.windows.values() {
            for f in folders {
                if !all.contains(f) {
                    all.push(f.clone());
                }
            }
        }
        *lock(&self.folders) = all;
    }

    /// Every open document in every window.
    fn all_docs(&self) -> Vec<&DocInfo> {
        let mut seen = HashSet::new();
        self.windows.values().flat_map(|(d, _)| d.iter()).filter(|d| seen.insert(d.id)).collect()
    }

    fn document_events(&mut self, now: Instant, wake: &mut Option<Duration>) {
        let docs: Vec<DocInfo> = self.all_docs().into_iter().cloned().collect();
        // Newly opened documents may start plugins.
        let fresh: Vec<DocInfo> = docs.iter().filter(|d| !self.seen.contains(&d.id)).cloned().collect();
        for d in &fresh {
            self.seen.insert(d.id);
            for i in 0..self.plugins.len() {
                let wants = self.plugins[i].manifest.as_ref().is_ok_and(|m| matches_any(&m.activation.open, d.path.as_deref()));
                if wants {
                    self.activate(i, false);
                }
            }
        }
        // Settings changed in settings.json (first, so what follows is judged by them).
        for p in &mut self.plugins {
            if !matches!(p.state, State::Running) {
                continue;
            }
            let Ok(m) = &p.manifest else { continue };
            let (eff, warnings) = effective_settings(&self.user_settings, m);
            p.warn(warnings);
            if p.sent_settings.as_ref() != Some(&eff)
                && let Some(h) = &p.host
                && h.notify("settings/changed", json!({"settings": eff}))
            {
                p.sent_settings = Some(eff);
            }
        }
        let ids: HashSet<u64> = docs.iter().map(|d| d.id).collect();
        self.seen.retain(|id| ids.contains(id));
        for p in &mut self.plugins {
            if !matches!(p.state, State::Running) || !p.manifest.as_ref().is_ok_and(|m| m.permissions.contains(&Perm::ReadDocument)) {
                continue;
            }
            let Some(h) = &p.host else { continue };
            for d in &docs {
                match p.told.get(&d.id) {
                    None => {
                        if h.notify("document/opened", json!({"document": d.to_json()})) {
                            p.told.insert(d.id, (d.version, now));
                        }
                    }
                    Some((v, at)) if *v != d.version => {
                        let due = *at + CHANGE_EVERY;
                        if due <= now {
                            if h.notify("document/changed", json!({"document": d.to_json()})) {
                                p.told.insert(d.id, (d.version, now));
                            }
                        } else {
                            let d = due - now;
                            *wake = Some(wake.map_or(d, |w| w.min(d)));
                        }
                    }
                    _ => {}
                }
            }
            let gone: Vec<u64> = p.told.keys().filter(|id| !ids.contains(id)).copied().collect();
            for id in gone {
                h.notify("document/closed", json!({"document": {"id": id}}));
                p.told.remove(&id);
            }
        }
    }

    /// A document was saved: plugins waiting for it start, running ones hear of it.
    pub fn saved(&mut self, doc: &DocInfo) {
        let now = Instant::now();
        for i in 0..self.plugins.len() {
            let Ok(m) = &self.plugins[i].manifest else { continue };
            if matches_any(&m.activation.save, doc.path.as_deref()) {
                self.activate(i, false);
            }
            let p = &mut self.plugins[i];
            if !matches!(p.state, State::Running) || !p.manifest.as_ref().is_ok_and(|m| m.permissions.contains(&Perm::ReadDocument)) {
                continue;
            }
            let Some(h) = &p.host else { continue };
            if !p.told.contains_key(&doc.id) && h.notify("document/opened", json!({"document": doc.to_json()})) {
                p.told.insert(doc.id, (doc.version, now));
            }
            h.notify("document/saved", json!({"document": doc.to_json()}));
        }
    }

    // ---- requests for windows -----------------------------------------------------------

    /// Requests this window should answer: about its own documents, or (in
    /// the primary window) anything else. Each comes with its plugin's name
    /// and run (for `reply`). At most `max`.
    pub fn take_requests(&mut self, window: u64, max: usize) -> Vec<(String, u64, Incoming)> {
        let mine: HashSet<u64> = self.windows.get(&window).map(|(d, _)| d.iter().map(|d| d.id).collect()).unwrap_or_default();
        let anywhere: HashSet<u64> = self.all_docs().iter().map(|d| d.id).collect();
        let primary = self.primary == Some(window);
        let mut out = Vec::new();
        let mut bytes = 0usize;
        for p in &mut self.plugins {
            if let Some(h) = &p.host {
                let room = INBOX.saturating_sub(p.inbox.len());
                p.inbox.extend(h.take(room));
            }
            let mut keep = VecDeque::new();
            while let Some(m) = p.inbox.pop_front() {
                let target = m.params.get("document").and_then(Value::as_u64);
                let ours = match target {
                    Some(id) if mine.contains(&id) => true,
                    Some(id) => primary && !anywhere.contains(&id),
                    None => primary,
                };
                if ours && out.len() < max && (out.is_empty() || bytes < FRAME_BYTES) {
                    bytes += m.size + m.reserve;
                    out.push((p.name.clone(), p.run, m));
                } else {
                    keep.push_back(m);
                }
            }
            p.inbox = keep;
        }
        if self.plugins.iter().any(|p| !p.inbox.is_empty())
            && let Some(ctx) = &self.ctx
        {
            ctx.request_repaint();
        }
        out
    }

    /// A window took a request's data (see `Host::handled`).
    pub fn handled(&self, name: &str, run: u64, size: usize) {
        if let Some(i) = self.index(name) {
            let p = &self.plugins[i];
            if p.run == run
                && let Some(h) = &p.host
            {
                h.handled(size);
            }
        }
    }

    /// Answer a request (dropped if that run of the plugin is over).
    pub fn reply(&self, name: &str, run: u64, id: Option<Value>, reply: Reply) {
        let (Some(id), Some(i)) = (id, self.index(name)) else { return };
        let p = &self.plugins[i];
        if p.run == run
            && let Some(h) = &p.host
        {
            h.reply(id, reply);
        }
    }

    /// Note something in a plugin's log.
    pub fn log(&self, name: &str, line: String) {
        if let Some(i) = self.index(name) {
            self.plugins[i].log(line);
        }
    }

    /// `ui/statusItem`: add, change or (empty text) remove one of the plugin's items.
    pub fn set_status_item(&mut self, name: &str, params: &Value) -> Reply {
        let id = api::str_param(params, "id")?.chars().take(64).collect::<String>();
        let clean = |s: &str, max: usize| -> String { s.lines().next().unwrap_or_default().chars().filter(|c| !c.is_control()).take(max).collect() };
        let text = clean(params.get("text").and_then(Value::as_str).unwrap_or_default(), STATUS_TEXT);
        let tooltip = clean(params.get("tooltip").and_then(Value::as_str).unwrap_or_default(), 300);
        let command = params.get("command").and_then(Value::as_str).map(str::to_owned);
        let i = self.index(name).ok_or_else(|| RpcError::new(api::INTERNAL_ERROR, "no such plugin"))?;
        let p = &mut self.plugins[i];
        if let Some(c) = &command
            && !p.manifest.as_ref().is_ok_and(|m| m.commands.iter().any(|k| &k.id == c))
        {
            return Err(RpcError::params(format!("\"command\" {c} isn't one of the plugin's commands")));
        }
        let at = p.status.iter().position(|s| s.id == id);
        match (at, text.is_empty()) {
            (Some(k), true) => {
                p.status.remove(k);
            }
            (Some(k), false) => p.status[k] = StatusItem { plugin: name.to_owned(), id, text, tooltip, command },
            (None, true) => {}
            (None, false) if p.status.len() >= STATUS_ITEMS => return Err(RpcError::new(BUSY, format!("a plugin can show at most {STATUS_ITEMS} status bar items"))),
            (None, false) => p.status.push(StatusItem { plugin: name.to_owned(), id, text, tooltip, command }),
        }
        Ok(Value::Null)
    }

    pub fn status_items(&self) -> Vec<&StatusItem> {
        self.plugins.iter().filter(|p| matches!(p.state, State::Running)).flat_map(|p| p.status.iter()).collect()
    }

    // ---- settings and keys --------------------------------------------------------------

    /// The `plugins` section of settings.json.
    pub fn set_user_settings(&mut self, s: &BTreeMap<String, Map<String, Value>>) {
        if &self.user_settings != s {
            self.user_settings = s.clone();
        }
    }

    /// The keys plugins bind, and those they can't have: a key Strata (or
    /// the user's keybindings.json) uses, or one an earlier plugin took.
    pub fn key_bindings(&self, taken: impl Fn(egui::Modifiers, egui::Key) -> bool) -> (Vec<Bound>, Vec<Conflict>) {
        let mut bound: Vec<Bound> = Vec::new();
        let mut conflicts = Vec::new();
        for p in self.plugins.iter().filter(|p| self.store.enabled(&p.name)) {
            let Ok(m) = &p.manifest else { continue };
            for k in &m.keys {
                if taken(k.mods, k.key) {
                    conflicts.push(Conflict { plugin: p.name.clone(), key: k.text.clone(), why: "Strata already uses this key".into() });
                } else if let Some(other) = bound.iter().find(|b| b.key == k.key && crate::keys::same_mods(b.mods, k.mods)) {
                    conflicts.push(Conflict { plugin: p.name.clone(), key: k.text.clone(), why: format!("the {} plugin already uses this key", other.plugin) });
                } else {
                    bound.push(Bound { plugin: p.name.clone(), command: k.command.clone(), mods: k.mods, key: k.key });
                }
            }
        }
        (bound, conflicts)
    }

    /// The pick list was answered or cancelled while another was open (tests).
    #[cfg(test)]
    pub fn inbox_len(&self, name: &str) -> usize {
        self.index(name).map_or(0, |i| self.plugins[i].inbox.len())
    }
}

/// Interpreters' search paths and startup hooks, and the dynamic loader's,
/// would let code outside the approved package run inside the plugin.
const SCRUBBED: &[&str] = &[
    "PYTHONPATH", "PYTHONHOME", "PYTHONSTARTUP", "PYTHONINSPECT", "PYTHONUSERBASE", "PYTHONEXECUTABLE", "PYTHONBREAKPOINT",
    "NODE_OPTIONS", "NODE_PATH", "NODE_REPL_EXTERNAL_MODULE",
    "RUBYOPT", "RUBYLIB", "PERL5LIB", "PERL5OPT", "PERLLIB",
    "LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT",
];

/// The environment a plugin runs with: Strata's own, without the variables
/// that load code from elsewhere (`SCRUBBED`, `DYLD_*`), and with Python
/// told to ignore the user's site folder and the current folder.
pub fn clean_env(parent: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let set: [(&str, &str); 5] = [("STRATA_PLUGIN_API", API), ("PYTHONNOUSERSITE", "1"), ("PYTHONSAFEPATH", "1"), ("PYTHONDONTWRITEBYTECODE", "1"), ("PYTHONUNBUFFERED", "1")];
    let mut env: Vec<(std::ffi::OsString, std::ffi::OsString)> = parent
        .into_iter()
        .filter(|(k, _)| {
            let up = k.to_string_lossy().to_ascii_uppercase();
            !SCRUBBED.contains(&up.as_str()) && !up.starts_with("DYLD_") && !set.iter().any(|(s, _)| *s == up)
        })
        .collect();
    env.extend(set.iter().map(|(k, v)| ((*k).into(), (*v).into())));
    env
}

impl Drop for Manager {
    fn drop(&mut self) {
        for p in &mut self.plugins {
            p.drop_host();
        }
    }
}

/// A plugin's settings: its defaults, with the user's values of the same
/// type over them; and warnings about unknown names and wrong types.
pub fn effective_settings(user: &BTreeMap<String, Map<String, Value>>, m: &Manifest) -> (Value, Vec<String>) {
    let mut out = Map::new();
    for (k, s) in &m.settings {
        out.insert(k.clone(), s.default.clone());
    }
    let mut warnings = Vec::new();
    if let Some(user) = user.get(&m.name) {
        for (k, v) in user {
            match m.settings.get(k) {
                Some(s) if s.default.is_null() || std::mem::discriminant(&s.default) == std::mem::discriminant(v) => {
                    out.insert(k.clone(), v.clone());
                }
                Some(_) => warnings.push(format!("Setting {k} in settings.json has the wrong type; using the default")),
                None => warnings.push(format!("Setting {k} in settings.json isn't one of this plugin's settings")),
            }
        }
    }
    (Value::Object(out), warnings)
}

/// `path` matches one of `globs` (by file name, or by the whole path for a glob with a `/`).
fn matches_any(globs: &[String], path: Option<&Path>) -> bool {
    let Some(path) = path else { return false };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    globs.iter().any(|g| {
        globset::Glob::new(g).is_ok_and(|glob| {
            let m = glob.compile_matcher();
            if g.contains('/') { m.is_match(path) } else { m.is_match(&name) }
        })
    })
}

/// The log's lines (tests and the manager window).
pub fn log_lines(log: &Mutex<Log>) -> Vec<String> {
    host::lock(log).lines().map(str::to_owned).collect()
}

#[cfg(test)]
pub(crate) mod tests;
