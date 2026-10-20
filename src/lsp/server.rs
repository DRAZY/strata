//! One running language server: the process, a thread that writes to it
//! (it also turns document snapshots into change notifications, so that
//! work never happens on the UI thread), a thread that reads its messages,
//! and the state they share with the window.

use super::config::ServerConfig;
use super::jsonrpc;
use super::position::{self, Encoding, Pos};
use super::uri;
use ropey::Rope;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

/// How long a server gets to answer `shutdown` before it's told to exit anyway.
const SHUTDOWN_REPLY: Duration = Duration::from_millis(800);
/// Lines of the server's error output kept, for saying why it stopped.
const STDERR_LINES: usize = 20;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Started; `initialize` not answered yet.
    Starting,
    Ready,
    /// The process ended (or never got going), and why.
    Stopped(String),
}

/// An error answer to a request.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    fn gone() -> Self {
        Self { code: -32099, message: "the language server stopped".into() }
    }

    /// Cancelled, or outdated by an edit: nothing worth telling the user.
    pub fn is_benign(&self) -> bool {
        matches!(self.code, -32802..=-32800)
    }
}

pub type Reply = Result<Value, RpcError>;

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub range: (Pos, Pos),
    /// 1 error, 2 warning, 3 information, 4 hint.
    pub severity: u8,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
}

impl Diagnostic {
    fn from_json(v: &Value) -> Option<Self> {
        let code = match v.get("code") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        Some(Self {
            range: position::range_from_json(v.get("range")?)?,
            // Unspecified: treated as an error, as other editors do.
            severity: v.get("severity").and_then(Value::as_u64).filter(|s| (1..=4).contains(s)).unwrap_or(1) as u8,
            message: v.get("message")?.as_str()?.to_owned(),
            source: v.get("source").and_then(Value::as_str).map(str::to_owned),
            code,
        })
    }
}

/// A file's latest diagnostics.
#[derive(Clone, Debug)]
pub struct DiagSet {
    /// The document version they're for, when the server says.
    pub version: Option<i32>,
    pub items: Arc<Vec<Diagnostic>>,
    /// Distinct for every publication, across every server.
    pub stamp: u64,
}

fn next_stamp() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SyncKind {
    None,
    #[default]
    Full,
    Incremental,
}

/// What the server said it can do.
#[derive(Clone, Debug, Default)]
pub struct Caps {
    pub raw: Arc<Value>,
    pub encoding: Encoding,
    pub sync: SyncKind,
    /// Wants `didSave` notifications.
    pub save: bool,
    pub completion_triggers: Vec<String>,
    pub signature_triggers: Vec<String>,
}

impl Caps {
    fn parse(raw: Value) -> Self {
        let encoding = Encoding::from_name(raw.get("positionEncoding").and_then(Value::as_str));
        let sync_v = raw.get("textDocumentSync");
        let kind = match sync_v {
            Some(Value::Number(n)) => n.as_u64(),
            Some(v) => v.get("change").and_then(Value::as_u64),
            None => None,
        };
        let sync = match kind {
            Some(0) => SyncKind::None,
            Some(2) => SyncKind::Incremental,
            _ => SyncKind::Full,
        };
        let save = matches!(sync_v.and_then(|v| v.get("save")), Some(Value::Bool(true)) | Some(Value::Object(_)));
        let list = |provider: &str, key: &str| -> Vec<String> {
            raw.get(provider).and_then(|p| p.get(key)).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()
        };
        let completion_triggers = list("completionProvider", "triggerCharacters");
        let signature_triggers = list("signatureHelpProvider", "triggerCharacters");
        Self { raw: Arc::new(raw), encoding, sync, save, completion_triggers, signature_triggers }
    }

    /// Whether a `…Provider` capability is on (`true` or an options object).
    pub fn has(&self, provider: &str) -> bool {
        matches!(self.raw.get(provider), Some(Value::Bool(true)) | Some(Value::Object(_)))
    }

    /// `completionProvider.resolveProvider`.
    pub fn resolves_completions(&self) -> bool {
        self.raw.get("completionProvider").and_then(|p| p.get("resolveProvider")).and_then(Value::as_bool).unwrap_or(false)
    }
}

/// State the threads and the window share.
struct Shared {
    status: Status,
    caps: Caps,
    pending: HashMap<i64, Sender<Reply>>,
    diagnostics: HashMap<PathBuf, DiagSet>,
    /// Work the server reports (`$/progress`), by token: title and percentage.
    progress: BTreeMap<String, (String, Option<u64>)>,
    stderr: VecDeque<String>,
    /// Errors and warnings the server asked to show, not yet shown.
    messages: Vec<String>,
    /// Shutting down on purpose: the process ending isn't a crash.
    stopping: bool,
    /// It answered `initialize` (so stopping later is a crash, not a failure to start).
    was_ready: bool,
}

/// To the writer thread.
enum Out {
    Message(Value),
    Open { uri: String, language: String, version: i32, text: Rope },
    Change { uri: String, version: i32, text: Rope },
    Close { uri: String },
    Save { uri: String },
    Shutdown,
}

/// What both threads need to talk to the process.
#[derive(Clone)]
struct Io {
    shared: Arc<Mutex<Shared>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicI64>,
    ctx: Option<egui::Context>,
}

impl Io {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        lock(&self.shared)
    }

    /// Write a message; false once the pipe is closed.
    fn write(&self, msg: &Value) -> bool {
        let mut stdin = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        let Some(pipe) = stdin.as_mut() else { return false };
        let ok = pipe.write_all(&jsonrpc::encode(msg)).and_then(|_| pipe.flush()).is_ok();
        if !ok {
            *stdin = None;
        }
        ok
    }

    /// Send a request whose answer comes back on the returned channel.
    fn request(&self, method: &str, params: Value) -> (i64, Receiver<Reply>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.lock().pending.insert(id, tx);
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        (id, rx)
    }

    fn repaint(&self) {
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    fn stop_with(&self, why: String) {
        let mut s = self.lock();
        if !matches!(s.status, Status::Stopped(_)) {
            s.status = Status::Stopped(why);
        }
    }
}

fn lock(m: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    // A thread that panicked mid-update leaves nothing half-written that matters here.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

pub struct Server {
    io: Io,
    tx: Sender<Out>,
    child: Arc<Proc>,
}

/// The server's process, and on Windows the job object that holds it and
/// everything it starts (killing `cmd.exe` alone would leave a `.cmd`
/// server's node running). Plugins (`crate::plugins`) run the same way.
pub(crate) struct Proc {
    child: Mutex<Option<Child>>,
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl Proc {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Option<Child>> {
        self.child.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Still running (not exited, not reaped).
    pub(crate) fn running(&self) -> bool {
        self.lock().as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }

    pub(crate) fn pid(&self) -> Option<u32> {
        self.lock().as_ref().map(Child::id)
    }
}

/// A process started by `spawn_group`, and its standard streams.
pub(crate) type Spawned = (Arc<Proc>, Option<ChildStdin>, Option<ChildStdout>, Option<ChildStderr>);

/// Start `cmd` with piped standard streams in a process group of its own
/// (a job object on Windows, and no console window), so that `kill` also
/// ends whatever it started (cargo check, a plugin's helpers…). On Windows
/// it starts suspended and runs only once it's in the job, so nothing it
/// starts can get out first; if Windows won't put it in a job it never runs.
pub(crate) fn spawn_group(cmd: &mut Command) -> std::io::Result<Spawned> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }
    let mut child = cmd.spawn()?;
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    #[cfg(windows)]
    let job = match job::Job::holding(&child) {
        Some(j) if job::Job::resume(&child) => Some(j),
        // Not contained, so not run: it's still suspended and hasn't done anything.
        other => {
            drop(other);
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other("Windows wouldn't let Strata keep track of the processes it would start"));
        }
    };
    let proc = Arc::new(Proc {
        child: Mutex::new(Some(child)),
        #[cfg(windows)]
        job,
    });
    Ok((proc, stdin, stdout, stderr))
}

impl Server {
    /// Start `program` (the config's command, found on `path_env`) for the
    /// project at `root`. Returns once the process runs; `initialize` is
    /// answered later (see `status`).
    pub fn start(config: &ServerConfig, program: &Path, root: &Path, path_env: Option<&std::ffi::OsStr>, ctx: Option<egui::Context>) -> Result<Server, String> {
        let mut cmd = Command::new(program);
        cmd.args(&config.args).current_dir(root);
        if let Some(p) = path_env {
            cmd.env("PATH", p);
        }
        cmd.envs(&config.env);
        let (child, stdin, stdout, stderr) = spawn_group(&mut cmd).map_err(|e| format!("couldn't start {}: {e}", config.command))?;
        log::info!("started language server {} (pid {}) for {}", config.command, child.pid().unwrap_or(0), root.display());
        let shared = Arc::new(Mutex::new(Shared {
            status: Status::Starting,
            caps: Caps::default(),
            pending: HashMap::new(),
            diagnostics: HashMap::new(),
            progress: BTreeMap::new(),
            stderr: VecDeque::new(),
            messages: Vec::new(),
            stopping: false,
            was_ready: false,
        }));
        let io = Io { shared, stdin: Arc::new(Mutex::new(stdin)), next_id: Arc::new(AtomicI64::new(1)), ctx };
        let (tx, rx) = channel();
        let name = config.display_name();

        // Without any one of its threads the server would sit in Starting
        // forever: that's a failure to start, and the process is ended.
        let started = (|| {
            if let Some(err) = stderr {
                let io = io.clone();
                spawn(&format!("lsp-stderr-{name}"), move || read_stderr(err, io))?;
            }
            let out = stdout.ok_or("no output pipe")?;
            let (io2, child2, settings, root2) = (io.clone(), child.clone(), config.settings.clone(), root.to_owned());
            spawn(&format!("lsp-read-{name}"), move || read_loop(out, io2, child2, settings, root2))?;
            let params = initialize_params(config, root);
            let io2 = io.clone();
            spawn(&format!("lsp-write-{name}"), move || write_loop(rx, io2, params))
        })();
        if let Err(e) = started {
            #[cfg(test)]
            tests::FAILED_PID.with(|p| p.set(child.lock().as_ref().map(Child::id)));
            io.lock().stopping = true;
            kill(&child);
            return Err(format!("couldn't start {}: {e}", config.command));
        }
        Ok(Server { io, tx, child })
    }

    pub fn status(&self) -> Status {
        self.io.lock().status.clone()
    }

    /// It got as far as answering `initialize`, at some point.
    pub fn was_ready(&self) -> bool {
        self.io.lock().was_ready
    }

    pub fn is_ready(&self) -> bool {
        self.status() == Status::Ready
    }

    pub fn caps(&self) -> Caps {
        self.io.lock().caps.clone()
    }

    /// Work in progress the server reports ("Indexing 40%"), if any.
    pub fn progress(&self) -> Option<String> {
        let s = self.io.lock();
        s.progress.values().next().map(|(title, pct)| match pct {
            Some(p) => format!("{title} {p}%"),
            None => title.clone(),
        })
    }

    /// Messages the server asked to show since the last call.
    pub fn take_messages(&self) -> Vec<String> {
        std::mem::take(&mut self.io.lock().messages)
    }

    pub fn request(&self, method: &'static str, params: Value) -> Pending {
        let id = self.io.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.io.lock().pending.insert(id, tx);
        let _ = self.tx.send(Out::Message(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})));
        Pending { id, rx, tx: self.tx.clone(), shared: Arc::downgrade(&self.io.shared), done: false }
    }

    pub fn open(&self, path: &Path, language: String, version: i32, text: Rope) {
        let _ = self.tx.send(Out::Open { uri: uri::from_path(path), language, version, text });
    }

    pub fn change(&self, path: &Path, version: i32, text: Rope) {
        let _ = self.tx.send(Out::Change { uri: uri::from_path(path), version, text });
    }

    pub fn close(&self, path: &Path) {
        let _ = self.tx.send(Out::Close { uri: uri::from_path(path) });
    }

    pub fn saved(&self, path: &Path) {
        let _ = self.tx.send(Out::Save { uri: uri::from_path(path) });
    }

    /// The latest diagnostics for a file (kept by its normalized path).
    pub fn diagnostics(&self, path: &Path) -> Option<DiagSet> {
        self.io.lock().diagnostics.get(path).cloned()
    }

    /// Every file with diagnostics.
    pub fn all_diagnostics(&self) -> Vec<(PathBuf, Arc<Vec<Diagnostic>>)> {
        self.io.lock().diagnostics.iter().filter(|(_, d)| !d.items.is_empty()).map(|(p, d)| (p.clone(), d.items.clone())).collect()
    }

    /// Ask the server to shut down and exit (it's killed if it hasn't by
    /// the time `wait_exit` gives up).
    pub fn stop(&self) {
        self.io.lock().stopping = true;
        let _ = self.tx.send(Out::Shutdown);
    }

    /// Wait until the process has ended, at most until `deadline`.
    pub fn wait_exit(&self, deadline: Instant) -> bool {
        loop {
            {
                let mut c = self.child.lock();
                match c.as_mut().map(Child::try_wait) {
                    None | Some(Ok(Some(_))) | Some(Err(_)) => return true,
                    Some(Ok(None)) => {}
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// End the process (and the processes it started) now.
    pub fn kill(&self) {
        self.io.lock().stopping = true;
        kill(&self.child);
    }

    #[cfg(test)]
    pub fn pid(&self) -> Option<u32> {
        self.child.lock().as_ref().map(Child::id)
    }
}

impl Drop for Server {
    /// A server let go of while its process still runs (it failed to
    /// initialize, or was replaced) is asked to exit and, if it doesn't,
    /// killed: no process is ever left behind.
    fn drop(&mut self) {
        if !self.child.running() {
            return;
        }
        self.stop();
        let proc = self.child.clone();
        let _ = spawn("lsp-reap", move || {
            let deadline = Instant::now() + Duration::from_millis(1500);
            while proc.running() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if proc.running() {
                kill(&proc);
            }
        });
    }
}

pub(crate) fn kill(proc: &Proc) {
    #[cfg(windows)]
    if let Some(job) = &proc.job {
        job.terminate();
    }
    let mut c = proc.lock();
    if let Some(ch) = c.as_mut() {
        #[cfg(unix)]
        if !reaped(ch.id()) {
            // SAFETY: the whole group (see `spawn_group`); the server's pid is the
            // group's id, and still its own while the process isn't reaped.
            unsafe {
                libc::killpg(ch.id() as libc::pid_t, libc::SIGKILL);
            }
        }
        let _ = ch.kill();
        let _ = ch.wait();
    }
}

/// The process was waited for already: its pid may belong to someone else
/// by now, so its group mustn't be signalled. (Running or exited but not
/// yet reaped: still ours.)
#[cfg(unix)]
pub(crate) fn reaped(pid: u32) -> bool {
    peek(pid).is_none()
}

/// Whether the process has exited, without reaping it; None once it's been
/// reaped (no such child any more).
#[cfg(unix)]
fn peek(pid: u32) -> Option<bool> {
    // SAFETY: zeroed plain data, filled in by the call.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer; WNOWAIT leaves the process to be reaped by `Child`.
    let r = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT | libc::WNOHANG) };
    if r != 0 {
        // Only "no such child" means it's gone; any other error says nothing.
        return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) { None } else { Some(false) };
    }
    // With WNOHANG and nothing to report, si_pid stays 0.
    // SAFETY: filled in by a successful call.
    Some(unsafe { info.si_pid() } != 0)
}

/// The process has exited (reaped or not), found without reaping it, so
/// its group can still be ended safely afterwards.
#[cfg(unix)]
pub(crate) fn exited(pid: u32) -> bool {
    peek(pid).is_none_or(|exited| exited)
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<(), String> {
    #[cfg(test)]
    if tests::FAIL_THREAD.with(|t| t.get().is_some_and(|prefix| name.starts_with(prefix))) {
        return Err(format!("couldn't start thread {name}: refused by the test"));
    }
    std::thread::Builder::new().name(name.to_owned()).spawn(f).map(drop).map_err(|e| {
        log::warn!("couldn't start thread {name}: {e}");
        format!("couldn't start thread {name}: {e}")
    })
}

/// A request in flight. Dropping it before the answer cancels it.
pub struct Pending {
    id: i64,
    rx: Receiver<Reply>,
    tx: Sender<Out>,
    shared: Weak<Mutex<Shared>>,
    done: bool,
}

impl Pending {
    /// The answer, once it has come (an error if the server stopped).
    pub fn poll(&mut self) -> Option<Reply> {
        if self.done {
            return None;
        }
        match self.rx.try_recv() {
            Ok(r) => {
                self.done = true;
                Some(r)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.done = true;
                Some(Err(RpcError::gone()))
            }
        }
    }

    /// Wait for the answer (tests only: the window never blocks on a server).
    #[cfg(test)]
    pub fn wait(&mut self, timeout: Duration) -> Option<Reply> {
        let r = match self.rx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => Err(RpcError::gone()),
        };
        self.done = true;
        Some(r)
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        if let Some(shared) = self.shared.upgrade() {
            lock(&shared).pending.remove(&self.id);
            let _ = self.tx.send(Out::Message(notification("$/cancelRequest", json!({"id": self.id}))));
        }
    }
}

fn initialize_params(config: &ServerConfig, root: &Path) -> Value {
    let root_uri = uri::from_path(root);
    let name = root.file_name().map_or_else(|| root.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
    json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "Strata", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": root_uri,
        "rootPath": root.to_string_lossy(),
        "workspaceFolders": [{"uri": root_uri, "name": name}],
        "initializationOptions": config.initialization_options.clone().unwrap_or(Value::Null),
        "capabilities": {
            "general": {"positionEncodings": ["utf-8", "utf-16"]},
            "workspace": {
                "configuration": true,
                "workspaceFolders": true,
                "applyEdit": false,
                // Only text changes: no creating, renaming or deleting files.
                "workspaceEdit": {"documentChanges": true, "resourceOperations": []},
            },
            "window": {"workDoneProgress": true, "showMessage": {}},
            "textDocument": {
                "synchronization": {"didSave": true, "willSave": false, "willSaveWaitUntil": false, "dynamicRegistration": false},
                "publishDiagnostics": {"versionSupport": true},
                "completion": {
                    "contextSupport": true,
                    "completionItem": {
                        "snippetSupport": true,
                        "insertReplaceSupport": true,
                        "documentationFormat": ["plaintext", "markdown"],
                        "resolveSupport": {"properties": ["additionalTextEdits", "detail", "documentation"]},
                    },
                },
                "hover": {"contentFormat": ["markdown", "plaintext"]},
                "signatureHelp": {"signatureInformation": {"documentationFormat": ["plaintext", "markdown"], "parameterInformation": {"labelOffsetSupport": true}}},
                "definition": {"linkSupport": true},
                "references": {},
                "rename": {"prepareSupport": false},
                "formatting": {},
            },
        },
    })
}

// ---- the writer thread ------------------------------------------------------------

fn write_loop(rx: Receiver<Out>, io: Io, params: Value) {
    let (_, init) = io.request("initialize", params);
    // Everything else waits in `rx` until the server has answered.
    let reply = loop {
        match init.recv_timeout(Duration::from_millis(100)) {
            Ok(r) => break r,
            Err(RecvTimeoutError::Timeout) if io.lock().stopping => return,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    };
    let result = match reply {
        Ok(r) => r,
        Err(e) => {
            io.stop_with(format!("it refused to start: {}", e.message));
            io.repaint();
            return;
        }
    };
    let caps = Caps::parse(result.get("capabilities").cloned().unwrap_or(Value::Null));
    {
        let mut s = io.lock();
        s.caps = caps.clone();
        if s.status == Status::Starting {
            s.status = Status::Ready;
            s.was_ready = true;
        }
    }
    io.write(&notification("initialized", json!({})));
    io.repaint();
    // The text the server has of each open document, to send only what changed.
    let mut texts: HashMap<String, Rope> = HashMap::new();
    while let Ok(first) = rx.recv() {
        let mut batch: Vec<Out> = std::iter::once(first).chain(rx.try_iter()).collect();
        coalesce(&mut batch);
        for out in batch {
            if !send(out, &io, &caps, &mut texts) {
                return;
            }
        }
    }
}

/// Typing faster than the server reads: of several snapshots of a document
/// in a row, only the last needs sending.
fn coalesce(batch: &mut Vec<Out>) {
    let mut i = 0;
    while i + 1 < batch.len() {
        let same = matches!((&batch[i], &batch[i + 1]), (Out::Change { uri: a, .. }, Out::Change { uri: b, .. }) if a == b);
        if same {
            batch.remove(i);
        } else {
            i += 1;
        }
    }
}

/// Write one message; false when the writer should stop.
fn send(out: Out, io: &Io, caps: &Caps, texts: &mut HashMap<String, Rope>) -> bool {
    match out {
        Out::Message(m) => {
            io.write(&m);
        }
        Out::Open { uri, language, version, text } => {
            let doc = json!({"uri": uri, "languageId": language, "version": version, "text": String::from(&text)});
            texts.insert(uri, text);
            io.write(&notification("textDocument/didOpen", json!({"textDocument": doc})));
        }
        Out::Change { uri, version, text } => {
            let Some(old) = texts.get(&uri) else { return true };
            let changes = match caps.sync {
                SyncKind::None => None,
                SyncKind::Full => (old != &text).then(|| json!([{"text": String::from(&text)}])),
                SyncKind::Incremental => change_event(old, &text, caps.encoding).map(|c| json!([c])),
            };
            texts.insert(uri.clone(), text);
            if let Some(changes) = changes {
                io.write(&notification("textDocument/didChange", json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": changes})));
            }
        }
        Out::Close { uri } => {
            texts.remove(&uri);
            io.write(&notification("textDocument/didClose", json!({"textDocument": {"uri": uri}})));
        }
        Out::Save { uri } => {
            if caps.save && texts.contains_key(&uri) {
                io.write(&notification("textDocument/didSave", json!({"textDocument": {"uri": uri}})));
            }
        }
        Out::Shutdown => {
            let (_, reply) = io.request("shutdown", Value::Null);
            let _ = reply.recv_timeout(SHUTDOWN_REPLY);
            io.write(&notification("exit", Value::Null));
            // Closing stdin tells servers that missed `exit` to go too.
            *io.stdin.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return false;
        }
    }
    true
}

/// The one change that turns `old` into `new`: the span between their
/// common start and common end (never splitting a `\r\n`), or None if
/// they're the same.
pub fn change_event(old: &Rope, new: &Rope, enc: Encoding) -> Option<Value> {
    let (start, old_end, new_end) = changed_span(old, new)?;
    Some(json!({
        "range": position::range_to_json((position::to_lsp(old, start, enc), position::to_lsp(old, old_end, enc))),
        "text": String::from(new.slice(start..new_end)),
    }))
}

/// (start, end in `old`, end in `new`) of what differs, in chars.
fn changed_span(old: &Rope, new: &Rope) -> Option<(usize, usize, usize)> {
    if old == new {
        return None;
    }
    let (ol, nl) = (old.len_chars(), new.len_chars());
    let mut start = old.chars().zip(new.chars()).take_while(|(a, b)| a == b).count();
    let room = ol.min(nl) - start;
    let mut tail = old.chars_at(ol).reversed().zip(new.chars_at(nl).reversed()).take(room).take_while(|(a, b)| a == b).count();
    // The range is read against the old text, where a spot between \r and
    // \n can't be named: widen it to take the whole pair.
    let splits = |at: usize| at > 0 && at < ol && old.char(at - 1) == '\r' && old.char(at) == '\n';
    if splits(start) {
        start -= 1;
    }
    if tail > 0 && splits(ol - tail) {
        tail -= 1;
    }
    Some((start, ol - tail, nl - tail))
}

// ---- the reader threads -------------------------------------------------------------

fn read_stderr(mut err: impl Read, io: Io) {
    let mut buf = [0u8; 8192];
    let mut line = Vec::new();
    while let Ok(n) = err.read(&mut buf) {
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if b == b'\n' {
                let text = String::from_utf8_lossy(&line).trim().to_owned();
                line.clear();
                if !text.is_empty() {
                    let mut s = io.lock();
                    s.stderr.push_back(text);
                    if s.stderr.len() > STDERR_LINES {
                        s.stderr.pop_front();
                    }
                }
            } else if line.len() < 4096 {
                line.push(b);
            }
        }
    }
}

fn read_loop(mut out: impl Read, io: Io, child: Arc<Proc>, settings: Option<Value>, root: PathBuf) {
    let mut decoder = jsonrpc::Decoder::default();
    let mut buf = vec![0u8; 64 << 10];
    loop {
        let n = match out.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        decoder.push(&buf[..n]);
        while let Some(msg) = decoder.next() {
            match msg {
                Ok(m) => handle(m, &io, settings.as_ref(), &root),
                Err(e) => log::warn!("language server: {e}"),
            }
        }
    }
    // The server closed its output: it has exited, or is about to.
    let status = {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut c = child.lock();
        let mut status = None;
        if let Some(ch) = c.as_mut() {
            while status.is_none() && Instant::now() < deadline {
                match ch.try_wait() {
                    Ok(Some(s)) => status = Some(s),
                    Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
        }
        drop(c);
        if status.is_none() {
            kill(&child);
        }
        status
    };
    let why = {
        let mut s = io.lock();
        s.pending.clear();
        s.diagnostics.clear();
        s.progress.clear();
        let last = s.stderr.back().cloned();
        let code = match status {
            Some(st) if st.success() => "it exited".to_owned(),
            Some(st) => format!("it exited ({st})"),
            None => "it stopped answering".to_owned(),
        };
        match last {
            Some(l) => format!("{code}: {}", l.chars().take(200).collect::<String>()),
            None => code,
        }
    };
    if !io.lock().stopping {
        log::warn!("language server for {} stopped: {why}", root.display());
    }
    io.stop_with(why);
    io.repaint();
}

/// One message from the server.
fn handle(m: Value, io: &Io, settings: Option<&Value>, root: &Path) {
    let method = m.get("method").and_then(Value::as_str);
    let id = m.get("id").cloned();
    match (method, id) {
        // An answer to one of ours.
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else { return };
            let reply = match m.get("error") {
                Some(e) => Err(RpcError { code: e.get("code").and_then(Value::as_i64).unwrap_or(0), message: e.get("message").and_then(Value::as_str).unwrap_or("unknown error").to_owned() }),
                None => Ok(m.get("result").cloned().unwrap_or(Value::Null)),
            };
            let tx = io.lock().pending.remove(&id);
            if let Some(tx) = tx {
                let _ = tx.send(reply);
                io.repaint();
            }
        }
        // A request from the server: always answered, or it may wait forever.
        (Some(method), Some(id)) => {
            let params = m.get("params").cloned().unwrap_or(Value::Null);
            let result = server_request(method, &params, io, settings, root);
            let reply = match result {
                Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
                Err((code, message)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
            };
            io.write(&reply);
        }
        (Some(method), None) => notification_from_server(method, m.get("params").unwrap_or(&Value::Null), io),
        (None, None) => {}
    }
}

fn server_request(method: &str, params: &Value, io: &Io, settings: Option<&Value>, root: &Path) -> Result<Value, (i64, String)> {
    match method {
        "workspace/configuration" => {
            let items = params.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
            Ok(Value::Array(items.iter().map(|it| section(settings, it.get("section").and_then(Value::as_str))).collect()))
        }
        "workspace/workspaceFolders" => {
            let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Ok(json!([{"uri": uri::from_path(root), "name": name}]))
        }
        "window/showMessageRequest" => {
            show_message(params, io);
            Ok(Value::Null)
        }
        "window/showDocument" => Ok(json!({"success": false})),
        "workspace/applyEdit" => Ok(json!({"applied": false, "failureReason": "Strata doesn't apply edits a server starts on its own"})),
        "client/registerCapability" | "client/unregisterCapability" | "window/workDoneProgress/create" => Ok(Value::Null),
        m if m.starts_with("workspace/") && m.ends_with("/refresh") => Ok(Value::Null),
        _ => Err((-32601, format!("{method} isn't supported"))),
    }
}

/// The part of the `settings` object a `workspace/configuration` item asks for.
fn section(settings: Option<&Value>, section: Option<&str>) -> Value {
    let Some(mut v) = settings else { return Value::Null };
    if let Some(path) = section.filter(|s| !s.is_empty()) {
        for key in path.split('.') {
            match v.get(key) {
                Some(next) => v = next,
                None => return Value::Null,
            }
        }
    }
    v.clone()
}

fn show_message(params: &Value, io: &Io) {
    // 1 error, 2 warning: worth the status bar. Information and logs aren't.
    let kind = params.get("type").and_then(Value::as_u64).unwrap_or(4);
    if kind <= 2
        && let Some(text) = params.get("message").and_then(Value::as_str)
    {
        let first = text.lines().next().unwrap_or_default().chars().take(200).collect();
        io.lock().messages.push(first);
        io.repaint();
    }
}

fn notification_from_server(method: &str, params: &Value, io: &Io) {
    match method {
        "textDocument/publishDiagnostics" => {
            let Some(path) = params.get("uri").and_then(Value::as_str).and_then(uri::to_path) else { return };
            let items: Vec<Diagnostic> = params.get("diagnostics").and_then(Value::as_array).map(|a| a.iter().filter_map(Diagnostic::from_json).collect()).unwrap_or_default();
            let version = params.get("version").and_then(Value::as_i64).map(|v| v as i32);
            io.lock().diagnostics.insert(uri::normalize(&path), DiagSet { version, items: Arc::new(items), stamp: next_stamp() });
            io.repaint();
        }
        "$/progress" => {
            let Some(token) = params.get("token").map(|t| t.to_string()) else { return };
            let Some(value) = params.get("value") else { return };
            let pct = value.get("percentage").and_then(Value::as_u64);
            let mut s = io.lock();
            match value.get("kind").and_then(Value::as_str) {
                Some("begin") => {
                    let title = value.get("title").and_then(Value::as_str).unwrap_or("Working").to_owned();
                    s.progress.insert(token, (title, pct));
                }
                Some("report") => {
                    if let Some(p) = s.progress.get_mut(&token) {
                        p.1 = pct.or(p.1);
                    }
                }
                Some("end") => {
                    s.progress.remove(&token);
                }
                _ => {}
            }
            drop(s);
            io.repaint();
        }
        "window/showMessage" => show_message(params, io),
        _ => {}
    }
}

/// A Windows job object that kills every process in it when closed or
/// terminated, so a server and the processes it started end together.
#[cfg(windows)]
pub(crate) mod job {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
        TerminateJobObject,
    };

    pub struct Job(HANDLE);

    // SAFETY: a job handle may be used and closed from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        /// A new job holding `child`; None if Windows refuses (the process
        /// then still runs, it just isn't grouped).
        pub fn holding(child: &std::process::Child) -> Option<Job> {
            // SAFETY: plain Win32 calls on a handle we own; `info` outlives the call.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const core::ffi::c_void, std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
                if set == 0 || AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0 {
                    CloseHandle(job);
                    return None;
                }
                Some(Job(job))
            }
        }

        /// Resume the threads of `child`, started suspended (once it's in a
        /// job). False if there were none to resume or one couldn't be.
        pub fn resume(child: &std::process::Child) -> bool {
            use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
            use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
            use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
            let pid = child.id();
            let mut resumed = 0;
            // SAFETY: plain Win32 calls on handles we open and close here; `entry` outlives them.
            unsafe {
                let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
                if snap == INVALID_HANDLE_VALUE {
                    return false;
                }
                let mut entry: THREADENTRY32 = std::mem::zeroed();
                entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
                let mut more = Thread32First(snap, &mut entry) != 0;
                while more {
                    if entry.th32OwnerProcessID == pid {
                        let t = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                        if t.is_null() || ResumeThread(t) == u32::MAX {
                            if !t.is_null() {
                                CloseHandle(t);
                            }
                            CloseHandle(snap);
                            return false;
                        }
                        CloseHandle(t);
                        resumed += 1;
                    }
                    more = Thread32Next(snap, &mut entry) != 0;
                }
                CloseHandle(snap);
            }
            resumed > 0
        }

        pub fn terminate(&self) {
            // SAFETY: the handle is open until `drop`.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: closing our own handle once; KILL_ON_JOB_CLOSE ends what's left.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// Threads whose name starts with this fail to start (this test thread only).
        pub(super) static FAIL_THREAD: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
        /// The process of the last server whose threads failed to start.
        pub(super) static FAILED_PID: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
    }

    #[test]
    fn a_server_whose_threads_cant_start_is_ended() {
        let config = super::super::fake::config("fake", &["*.fake"], json!({}));
        let dir = std::env::temp_dir();
        FAIL_THREAD.with(|t| t.set(Some("lsp-write")));
        let result = Server::start(&config, Path::new(&config.command), &dir, None, None);
        FAIL_THREAD.with(|t| t.set(None));
        let err = result.err().expect("a failure to start");
        assert!(err.contains("couldn't start thread"), "{err}");
        let pid = FAILED_PID.with(|p| p.get()).expect("it had started a process");
        #[cfg(unix)]
        // SAFETY: signal 0 only checks whether the process exists.
        assert_ne!(unsafe { libc::kill(pid as libc::pid_t, 0) }, 0, "the process is gone");
        let _ = pid;
    }

    fn span(a: &str, b: &str) -> Option<(usize, usize, usize)> {
        changed_span(&Rope::from_str(a), &Rope::from_str(b))
    }

    #[test]
    fn changes_are_the_differing_span() {
        assert_eq!(span("same", "same"), None);
        assert_eq!(span("hello world", "hello brave world"), Some((6, 6, 12)));
        assert_eq!(span("abcdef", "abef"), Some((2, 4, 2)));
        assert_eq!(span("", "new"), Some((0, 0, 3)));
        assert_eq!(span("aaa", "aaaa"), Some((3, 3, 4)), "repeated letters: one insert at the end");
        // A \r\n pair of the old text is never split.
        assert_eq!(span("a\r\nb", "a\r\r\nb"), Some((1, 3, 4)));
        assert_eq!(span("x\r\ny", "x\ny"), Some((1, 3, 2)));
        assert_eq!(span("x\ny", "x\r\ny"), Some((1, 1, 2)));
    }

    #[test]
    fn change_events_use_the_servers_position_encoding() {
        let old = Rope::from_str("a👋b\nline two\n");
        let new = Rope::from_str("a👋xb\nline two\n");
        let utf16 = change_event(&old, &new, Encoding::Utf16).unwrap();
        assert_eq!(utf16, json!({"range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 3}}, "text": "x"}));
        let utf8 = change_event(&old, &new, Encoding::Utf8).unwrap();
        assert_eq!(utf8["range"]["start"], json!({"line": 0, "character": 5}));
        // Replaying the events rebuilds the text, edit after edit.
        let steps = ["fn main() {}\n", "fn main() {\n    \n}\n", "fn main() {\n    let 日本 = 1;\n}\n", "fn main() {\r\n    let 日本 = 1;\r\n}\r\n", "x"];
        let mut server_text = Rope::from_str(steps[0]);
        for w in steps.windows(2) {
            let ev = change_event(&Rope::from_str(w[0]), &Rope::from_str(w[1]), Encoding::Utf16).unwrap();
            let (a, b) = position::range_from_json(&ev["range"]).unwrap();
            let (a, b) = (position::from_lsp(&server_text, a, Encoding::Utf16), position::from_lsp(&server_text, b, Encoding::Utf16));
            server_text.remove(a..b);
            server_text.insert(a, ev["text"].as_str().unwrap());
            assert_eq!(server_text.to_string(), w[1]);
        }
    }

    #[test]
    fn snapshots_in_a_row_coalesce() {
        let rope = Rope::from_str("x");
        let ch = |u: &str, v| Out::Change { uri: u.into(), version: v, text: rope.clone() };
        let mut batch = vec![ch("a", 1), ch("a", 2), ch("b", 3), ch("a", 4), Out::Message(json!(1)), ch("a", 5), ch("a", 6)];
        coalesce(&mut batch);
        let versions: Vec<i32> = batch.iter().filter_map(|o| if let Out::Change { version, .. } = o { Some(*version) } else { None }).collect();
        assert_eq!(versions, [2, 3, 4, 6]);
    }

    #[test]
    fn capabilities() {
        let caps = Caps::parse(json!({
            "positionEncoding": "utf-8",
            "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": false}},
            "completionProvider": {"triggerCharacters": [".", ":"], "resolveProvider": true},
            "hoverProvider": true,
            "renameProvider": {"prepareProvider": true},
            "definitionProvider": false,
        }));
        assert_eq!((caps.encoding, caps.sync, caps.save), (Encoding::Utf8, SyncKind::Incremental, true));
        assert_eq!(caps.completion_triggers, [".", ":"]);
        assert!(caps.has("hoverProvider") && caps.has("renameProvider") && !caps.has("definitionProvider") && !caps.has("referencesProvider"));
        assert!(caps.resolves_completions());
        let caps = Caps::parse(json!({"textDocumentSync": 1}));
        assert_eq!((caps.encoding, caps.sync, caps.save), (Encoding::Utf16, SyncKind::Full, false));
    }

    #[test]
    fn configuration_sections() {
        let settings = json!({"rust-analyzer": {"check": {"command": "clippy"}}});
        assert_eq!(section(Some(&settings), Some("rust-analyzer.check")), json!({"command": "clippy"}));
        assert_eq!(section(Some(&settings), Some("missing")), Value::Null);
        assert_eq!(section(Some(&settings), None), settings);
        assert_eq!(section(None, Some("x")), Value::Null);
    }

    #[test]
    fn diagnostics_parse() {
        let d = Diagnostic::from_json(&json!({"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}}, "message": "bad", "code": 42, "source": "x"})).unwrap();
        assert_eq!((d.severity, d.code.as_deref(), d.source.as_deref()), (1, Some("42"), Some("x")));
        assert!(Diagnostic::from_json(&json!({"message": "no range"})).is_none());
    }
}
