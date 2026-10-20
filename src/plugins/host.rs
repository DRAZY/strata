//! One running plugin: its process, started the way language servers are
//! (`lsp::server::spawn_group`: a process group, or a job object on Windows,
//! so stopping it stops everything it started) and framed the same way
//! (`lsp::jsonrpc`); a writer thread fed by a bounded queue, so the window
//! never blocks on a plugin that stops reading; a reader thread that checks
//! every message against the plugin's permissions before anything else
//! happens (refusals are answered with an error and logged), carries out
//! file and log requests itself, and hands the rest to the window through
//! another bounded queue (a plugin that floods Strata is slowed down, the
//! window isn't); and a thread putting what it prints on stderr in its log.

use super::Perm;
use super::api::{self, INVALID_REQUEST, PARSE_ERROR, RpcError, Reply, STOPPED, TIMED_OUT, TOO_LARGE, Where};
use crate::lsp::jsonrpc;
use crate::lsp::server::{Proc, kill, spawn_group};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError, TrySendError, channel, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// A message bigger than this is refused (room for a 16 MB text, escaped).
pub const MAX_MESSAGE: usize = 64 << 20;
/// Messages waiting to be written to the plugin.
const OUT_QUEUE: usize = 512;
/// Requests waiting for a window: the reader waits (and so does the
/// plugin, on its full pipe) while this many are queued.
const IN_QUEUE: usize = 256;
/// Bytes of requests waiting for a window: past this the plugin waits.
const IN_BUDGET: usize = 64 << 20;
/// Bytes waiting to be written to the plugin: past this Strata drops what
/// it would send (the plugin isn't reading), so memory stays bounded.
const OUT_BUDGET: usize = 64 << 20;
pub const LOG_LINES: usize = 1000;
const LOG_LINE_CHARS: usize = 2000;
/// How long a plugin gets to answer `shutdown`, and then to exit.
const SHUTDOWN_REPLY: Duration = Duration::from_millis(800);
const EXIT_GRACE: Duration = Duration::from_millis(1500);

/// How to run a plugin.
#[derive(Clone, Debug)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// The whole environment (nothing else is inherited; see `manager::clean_env`).
    pub env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

/// A request (with an `id` to answer) or a notification for a window.
#[derive(Debug)]
pub struct Incoming {
    pub id: Option<Value>,
    pub method: String,
    pub params: Value,
    /// Roughly its bytes, given back with `Host::handled`.
    pub size: usize,
    /// Room kept for its answer in what goes to the plugin.
    pub reserve: usize,
}

/// Roughly how many bytes `v` takes as JSON (its strings, mostly).
pub fn json_size(v: &Value) -> usize {
    match v {
        Value::String(s) => s.len() + 2,
        Value::Array(a) => a.iter().map(json_size).sum::<usize>() + a.len() + 2,
        Value::Object(o) => o.iter().map(|(k, v)| k.len() + 4 + json_size(v)).sum::<usize>() + 2,
        _ => 8,
    }
}

/// Room to keep for the answer to a request a window handles: a document
/// can be large, every other answer is small.
fn reply_room(method: &str) -> usize {
    match method {
        "document/read" => api::MAX_TEXT + (1 << 20),
        _ => 4 << 10,
    }
}

/// The most a request for a window may carry: edits can be big, the rest is small.
fn max_size(method: &str) -> usize {
    match method {
        "document/applyEdits" => 40 << 20,
        "ui/pick" => 4 << 20,
        "ui/showPanel" => 2 << 20,
        _ => 256 << 10,
    }
}

/// The last lines a plugin logged or printed on stderr (shown in the
/// Plugins manager; kept in memory only, since they may quote documents).
#[derive(Default)]
pub struct Log {
    lines: VecDeque<String>,
    /// Lines dropped to stay within `LOG_LINES`.
    pub dropped: usize,
}

impl Log {
    pub fn push(&mut self, line: impl Into<String>) {
        let mut line: String = line.into();
        if line.chars().count() > LOG_LINE_CHARS {
            line = line.chars().take(LOG_LINE_CHARS).chain(['…']).collect();
        }
        if self.lines.len() >= LOG_LINES {
            self.lines.pop_front();
            self.dropped += 1;
        }
        self.lines.push_back(line);
    }

    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lines.len()
    }
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A thread that panicked mid-update leaves nothing half-written that matters here.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

enum Out {
    /// A message and its size (counted in `Flow::to_plugin` until written).
    Message(Value, usize),
    Shutdown,
}

/// Bytes on their way between the plugin and the window, both ways.
#[derive(Default)]
struct Flow {
    to_window: AtomicUsize,
    to_plugin: AtomicUsize,
    /// The writer thread ended (the plugin's input is closed).
    writer_gone: AtomicBool,
}

fn give_back(counter: &AtomicUsize, n: usize) {
    // Never below zero (the writer clears the count when it ends).
    let mut cur = counter.load(Ordering::Relaxed);
    while let Err(now) = counter.compare_exchange_weak(cur, cur.saturating_sub(n), Ordering::Relaxed, Ordering::Relaxed) {
        cur = now;
    }
}

#[derive(Default)]
struct Shared {
    pending: HashMap<i64, (Sender<Reply>, Instant)>,
    /// Room kept for the answers to requests a window has, by request id.
    reserved: HashMap<String, Vec<usize>>,
    /// Why the process ended, once it has.
    exited: Option<String>,
    stopping: bool,
}

/// What the reader thread needs.
struct Reader {
    name: String,
    perms: BTreeSet<Perm>,
    folders: Arc<Mutex<Vec<PathBuf>>>,
    log: Arc<Mutex<Log>>,
    shared: Arc<Mutex<Shared>>,
    out: SyncSender<Out>,
    incoming: SyncSender<Incoming>,
    flow: Arc<Flow>,
    ctx: Option<egui::Context>,
}

pub struct Host {
    proc: Arc<Proc>,
    out: SyncSender<Out>,
    incoming: Receiver<Incoming>,
    shared: Arc<Mutex<Shared>>,
    flow: Arc<Flow>,
    next_id: AtomicI64,
    log: Arc<Mutex<Log>>,
}

impl Host {
    /// Start the plugin. It's running when this returns; `initialize` is
    /// the caller's first request.
    pub fn start(name: &str, launch: &Launch, perms: BTreeSet<Perm>, folders: Arc<Mutex<Vec<PathBuf>>>, log: Arc<Mutex<Log>>, ctx: Option<egui::Context>) -> Result<Host, String> {
        let mut cmd = std::process::Command::new(&launch.program);
        cmd.args(&launch.args).current_dir(&launch.cwd).env_clear().envs(launch.env.iter().map(|(k, v)| (k, v)));
        let (proc, stdin, stdout, stderr) = spawn_group(&mut cmd).map_err(|e| format!("couldn't start {}: {e}", launch.program.display()))?;
        log::info!("started plugin {name} (pid {})", proc.pid().unwrap_or(0));
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (out_tx, out_rx) = sync_channel(OUT_QUEUE);
        let (in_tx, in_rx) = sync_channel(IN_QUEUE);
        let flow = Arc::new(Flow::default());
        let host = Host { proc: proc.clone(), out: out_tx.clone(), incoming: in_rx, shared: shared.clone(), flow: flow.clone(), next_id: AtomicI64::new(1), log: log.clone() };
        let started = (|| {
            let stdin = stdin.ok_or("no input pipe")?;
            let stdout = stdout.ok_or("no output pipe")?;
            if let Some(err) = stderr {
                let log = log.clone();
                spawn(&format!("plugin-stderr-{name}"), move || read_stderr(err, log))?;
            }
            let (shared2, flow2) = (shared.clone(), flow.clone());
            spawn(&format!("plugin-write-{name}"), move || write_loop(out_rx, stdin, shared2, flow2))?;
            let reader = Reader { name: name.to_owned(), perms, folders, log: log.clone(), shared: shared.clone(), out: out_tx, incoming: in_tx, flow, ctx };
            let proc2 = proc.clone();
            spawn(&format!("plugin-read-{name}"), move || read_loop(stdout, reader, proc2))
        })();
        if let Err(e) = started {
            lock(&host.shared).stopping = true;
            kill(&proc);
            return Err(format!("couldn't start {name}: {e}"));
        }
        Ok(host)
    }

    fn send(&self, msg: Value) -> bool {
        let size = json_size(&msg);
        let queued = self.flow.to_plugin.load(Ordering::Relaxed);
        if queued > 0 && queued + size > OUT_BUDGET {
            lock(&self.log).push("Strata dropped a message: the plugin isn't reading what Strata sends");
            return false;
        }
        self.flow.to_plugin.fetch_add(size, Ordering::Relaxed);
        match self.out.try_send(Out::Message(msg, size)) {
            Ok(()) => true,
            Err(e) => {
                give_back(&self.flow.to_plugin, size);
                if matches!(e, TrySendError::Full(_)) {
                    lock(&self.log).push("Strata dropped a message: the plugin isn't reading what Strata sends");
                }
                false
            }
        }
    }

    /// A window is done with a request (`Incoming::size`): room for more.
    pub fn handled(&self, size: usize) {
        give_back(&self.flow.to_window, size);
    }

    /// Send a request; the answer (or an error after `timeout`) comes through `Pending`.
    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Pending {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        lock(&self.shared).pending.insert(id, (tx, Instant::now() + timeout));
        if !self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})) {
            // Not sent: answered at once rather than after the timeout.
            if let Some((tx, _)) = lock(&self.shared).pending.remove(&id) {
                let _ = tx.send(Err(RpcError::new(STOPPED, "the plugin isn't taking messages")));
            }
        }
        Pending { rx, done: false }
    }

    /// Send a notification; false if it couldn't be queued.
    pub fn notify(&self, method: &str, params: Value) -> bool {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Answer a request a window handled. Room was kept for it when the
    /// request came in, so it's never dropped for being over the budget.
    pub fn reply(&self, id: Value, reply: Reply) {
        let reserved = lock(&self.shared).reserved.get_mut(&id.to_string()).and_then(Vec::pop).unwrap_or(0);
        let msg = response(id, reply);
        let size = json_size(&msg);
        self.flow.to_plugin.fetch_add(size, Ordering::Relaxed);
        give_back(&self.flow.to_plugin, reserved);
        if self.out.try_send(Out::Message(msg, size)).is_err() {
            give_back(&self.flow.to_plugin, size);
            lock(&self.log).push("Strata dropped an answer: the plugin isn't reading what Strata sends");
        }
    }

    /// Up to `max` messages waiting for a window.
    pub fn take(&self, max: usize) -> Vec<Incoming> {
        let mut out = Vec::new();
        while out.len() < max {
            match self.incoming.try_recv() {
                Ok(m) => out.push(m),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    /// Requests of Strata's that the plugin didn't answer in time get an error.
    pub fn expire(&self) {
        let now = Instant::now();
        let mut s = lock(&self.shared);
        let late: Vec<i64> = s.pending.iter().filter(|(_, (_, at))| *at <= now).map(|(id, _)| *id).collect();
        for id in late {
            if let Some((tx, _)) = s.pending.remove(&id) {
                let _ = tx.send(Err(RpcError::new(TIMED_OUT, "the plugin didn't answer in time")));
            }
        }
    }

    /// Why the process ended, once it has.
    pub fn exited(&self) -> Option<String> {
        lock(&self.shared).exited.clone()
    }

    /// The plugin exited, but something it started still holds its output
    /// open (so the reader never sees the end): end that too.
    pub fn watch(&self) {
        if lock(&self.shared).exited.is_none() && has_exited(&self.proc) {
            kill(&self.proc);
        }
    }

    /// Ask the plugin to shut down; it's ended for good if it hasn't
    /// exited shortly after (whatever it started too).
    pub fn stop(&self) {
        {
            let mut s = lock(&self.shared);
            if s.stopping {
                return;
            }
            s.stopping = true;
        }
        if self.out.try_send(Out::Shutdown).is_err() {
            kill(&self.proc);
            return;
        }
        let proc = self.proc.clone();
        let _ = spawn("plugin-reap", move || {
            let deadline = Instant::now() + SHUTDOWN_REPLY + EXIT_GRACE;
            while !has_exited(&proc) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            // Exited or not: whatever it started goes with it.
            kill(&proc);
        });
    }

    /// End the process (and what it started) now.
    pub fn kill(&self) {
        lock(&self.shared).stopping = true;
        kill(&self.proc);
    }

    #[cfg(test)]
    pub fn pid(&self) -> Option<u32> {
        self.proc.pid()
    }
}

impl Drop for Host {
    /// No plugin process outlives its host.
    fn drop(&mut self) {
        self.stop();
    }
}

/// A request of Strata's in flight.
pub struct Pending {
    rx: Receiver<Reply>,
    done: bool,
}

impl Pending {
    /// The answer, once it's here (an error if the plugin stopped or took too long).
    pub fn poll(&mut self) -> Option<Reply> {
        if self.done {
            return None;
        }
        match self.rx.try_recv() {
            Ok(r) => {
                self.done = true;
                Some(r)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.done = true;
                Some(Err(RpcError::new(STOPPED, "the plugin stopped before answering")))
            }
        }
    }

    /// Wait for the answer (tests only: the window never blocks on a plugin).
    #[cfg(test)]
    pub fn wait(&mut self, timeout: Duration) -> Option<Reply> {
        let r = match self.rx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(RpcError::new(STOPPED, "the plugin stopped before answering")),
        };
        self.done = true;
        Some(r)
    }
}

fn response(id: Value, reply: Reply) -> Value {
    match reply {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e.to_json()}),
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<(), String> {
    std::thread::Builder::new().name(name.to_owned()).spawn(f).map(drop).map_err(|e| format!("couldn't start thread {name}: {e}"))
}

fn write_loop(rx: Receiver<Out>, stdin: std::process::ChildStdin, shared: Arc<Mutex<Shared>>, flow: Arc<Flow>) {
    write_messages(rx, stdin, shared, &flow);
    flow.writer_gone.store(true, Ordering::Relaxed);
    flow.to_plugin.store(0, Ordering::Relaxed);
}

fn write_messages(rx: Receiver<Out>, mut stdin: std::process::ChildStdin, shared: Arc<Mutex<Shared>>, flow: &Flow) {
    let mut write = |msg: &Value| stdin.write_all(&jsonrpc::encode(msg)).and_then(|_| stdin.flush()).is_ok();
    while let Ok(out) = rx.recv() {
        match out {
            Out::Message(m, size) => {
                let ok = write(&m);
                give_back(&flow.to_plugin, size);
                if !ok {
                    return;
                }
            }
            Out::Shutdown => {
                // Id 0 is never used for anything else.
                let (tx, reply) = channel();
                lock(&shared).pending.insert(0, (tx, Instant::now() + SHUTDOWN_REPLY));
                if write(&json!({"jsonrpc": "2.0", "id": 0, "method": "shutdown", "params": null})) {
                    let _ = reply.recv_timeout(SHUTDOWN_REPLY);
                    write(&json!({"jsonrpc": "2.0", "method": "exit", "params": null}));
                }
                // Dropping stdin closes it: a plugin that missed `exit` sees the end of its input.
                return;
            }
        }
    }
}

fn read_stderr(mut err: impl Read, log: Arc<Mutex<Log>>) {
    let mut buf = [0u8; 8192];
    let mut line = Vec::new();
    while let Ok(n) = err.read(&mut buf) {
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if b == b'\n' {
                let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                line.clear();
                if !text.is_empty() {
                    lock(&log).push(format!("stderr: {text}"));
                }
            } else if line.len() < LOG_LINE_CHARS * 4 {
                line.push(b);
            }
        }
    }
}

fn read_loop(mut stdout: impl Read, r: Reader, proc: Arc<Proc>) {
    let mut decoder = jsonrpc::Decoder::with_limit(MAX_MESSAGE);
    let mut buf = vec![0u8; 64 << 10];
    loop {
        let n = match stdout.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        decoder.push(&buf[..n]);
        while let Some(msg) = decoder.next() {
            match msg {
                Ok(m) => r.handle(m),
                Err(jsonrpc::FrameError::BadJson(e)) => {
                    lock(&r.log).push(format!("Strata couldn't read a message (not JSON: {e})"));
                    r.answer(Some(Value::Null), Err(RpcError::new(PARSE_ERROR, "the message isn't JSON")));
                }
                Err(e) => lock(&r.log).push(format!("Strata skipped a message: {e}")),
            }
        }
    }
    let why = finish(&proc);
    let stopping = {
        let mut s = lock(&r.shared);
        // Dropping the senders answers every waiting request with "stopped".
        s.pending.clear();
        s.exited = Some(why.clone());
        s.stopping
    };
    if !stopping {
        log::warn!("plugin {} stopped: {why}", r.name);
    }
    lock(&r.log).push(format!("The plugin stopped: {why}"));
    if let Some(ctx) = &r.ctx {
        ctx.request_repaint();
    }
}

/// The plugin closed its output: wait for it to exit (briefly), end
/// whatever it left running in its group, and say how it ended.
fn finish(proc: &Proc) -> String {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut exited = has_exited(proc);
    while !exited && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        exited = has_exited(proc);
    }
    // Ends the group too (helpers it left running), then reaps it.
    kill(proc);
    let status = proc.lock().as_mut().and_then(|c| c.try_wait().ok().flatten());
    match status {
        Some(s) if exited && s.success() => "it exited".to_owned(),
        Some(s) if exited => format!("it exited ({s})"),
        _ => "it closed its output and was stopped".to_owned(),
    }
}

/// The process has exited, found out without reaping it on Unix (so that
/// `kill` can still end its group safely).
fn has_exited(proc: &Proc) -> bool {
    #[cfg(unix)]
    {
        proc.pid().is_none_or(crate::lsp::server::exited)
    }
    #[cfg(not(unix))]
    {
        !proc.running()
    }
}

impl Reader {
    fn repaint(&self) {
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    /// Wait while `counter` plus `size` is over `budget` (unless nothing is
    /// queued); false if the plugin is being stopped meanwhile.
    fn wait_for_room(&self, counter: &AtomicUsize, size: usize, budget: usize) -> bool {
        loop {
            let queued = counter.load(Ordering::Relaxed);
            if queued == 0 || queued + size <= budget {
                return true;
            }
            if lock(&self.shared).stopping || self.flow.writer_gone.load(Ordering::Relaxed) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn answer(&self, id: Option<Value>, reply: Reply) {
        let Some(id) = id else { return };
        let msg = response(id, reply);
        let size = json_size(&msg);
        // A plugin that asks for files and doesn't read the answers waits here.
        if !self.wait_for_room(&self.flow.to_plugin, size, OUT_BUDGET) {
            return;
        }
        self.flow.to_plugin.fetch_add(size, Ordering::Relaxed);
        if self.out.send(Out::Message(msg, size)).is_err() {
            give_back(&self.flow.to_plugin, size);
        }
    }

    /// One message from the plugin.
    fn handle(&self, msg: Value) {
        let Some(obj) = msg.as_object() else {
            lock(&self.log).push("Strata skipped a message that isn't a JSON object");
            return;
        };
        let id = obj.get("id").cloned();
        match obj.get("method") {
            Some(Value::String(method)) => {
                if id.as_ref().is_some_and(|v| !v.is_number() && !v.is_string()) {
                    return self.answer(Some(Value::Null), Err(RpcError::new(INVALID_REQUEST, "\"id\" must be a number or a string")));
                }
                let params = obj.get("params").cloned().unwrap_or(Value::Null);
                match api::check(method, &self.perms) {
                    Err(e) => {
                        // The method name only: parameters may hold document text.
                        lock(&self.log).push(format!("Refused {}: {}", clip(method), e.message));
                        log::warn!("plugin {}: refused {} ({})", self.name, clip(method), e.code);
                        self.answer(id, Err(e));
                    }
                    Ok(m) if m.at == Where::Host => {
                        let reply = self.host_method(m.name, &params);
                        self.answer(id, reply);
                    }
                    Ok(m) => {
                        let size = json_size(&params);
                        if size > max_size(m.name) {
                            lock(&self.log).push(format!("Refused {}: it's too large", m.name));
                            return self.answer(id, Err(RpcError::new(TOO_LARGE, format!("{} can carry at most {} KB", m.name, max_size(m.name) >> 10))));
                        }
                        // Waits while the window has plenty to do, and until there's room
                        // for the answer: the plugin is held up, not the window.
                        let reserve = if id.is_some() { reply_room(m.name) } else { 0 };
                        if !self.wait_for_room(&self.flow.to_window, size, IN_BUDGET) || !self.wait_for_room(&self.flow.to_plugin, reserve, OUT_BUDGET) {
                            return self.answer(id, Err(RpcError::new(STOPPED, "Strata is shutting the plugin down")));
                        }
                        self.flow.to_window.fetch_add(size, Ordering::Relaxed);
                        self.flow.to_plugin.fetch_add(reserve, Ordering::Relaxed);
                        if let Some(id) = &id {
                            lock(&self.shared).reserved.entry(id.to_string()).or_default().push(reserve);
                        }
                        if self.incoming.send(Incoming { id: id.clone(), method: m.name.to_owned(), params, size, reserve }).is_err() {
                            give_back(&self.flow.to_window, size);
                            give_back(&self.flow.to_plugin, reserve);
                            if let Some(i) = &id {
                                lock(&self.shared).reserved.get_mut(&i.to_string()).and_then(Vec::pop);
                            }
                            self.answer(id, Err(RpcError::new(STOPPED, "Strata is shutting the plugin down")));
                        }
                        self.repaint();
                    }
                }
            }
            Some(_) => self.answer(id.or(Some(Value::Null)), Err(RpcError::new(INVALID_REQUEST, "\"method\" must be a string"))),
            None => {
                let Some(n) = id.as_ref().and_then(Value::as_i64) else {
                    lock(&self.log).push("Strata skipped an answer to a request it never sent");
                    return;
                };
                let reply = match obj.get("error") {
                    Some(e) => Err(RpcError::new(e.get("code").and_then(Value::as_i64).unwrap_or(0), e.get("message").and_then(Value::as_str).unwrap_or("unknown error").chars().take(500).collect::<String>())),
                    None => Ok(obj.get("result").cloned().unwrap_or(Value::Null)),
                };
                let tx = lock(&self.shared).pending.remove(&n);
                if let Some((tx, _)) = tx {
                    let _ = tx.send(reply);
                    self.repaint();
                }
            }
        }
    }

    /// Methods the reader carries out itself (after the permission check).
    fn host_method(&self, method: &str, params: &Value) -> Reply {
        match method {
            "log" => {
                let text = params.get("message").and_then(Value::as_str).unwrap_or_default();
                let level = params.get("level").and_then(Value::as_str).filter(|l| matches!(*l, "error" | "warning" | "info" | "debug")).unwrap_or("info");
                lock(&self.log).push(if level == "info" { text.to_owned() } else { format!("{level}: {text}") });
                Ok(Value::Null)
            }
            "workspace/folders" => Ok(json!({"folders": *lock(&self.folders)})),
            "workspace/readFile" => {
                let path = api::str_param(params, "path")?;
                let folders = lock(&self.folders).clone();
                let text = super::workspace::read(&folders, Path::new(path))?;
                Ok(json!({"text": text}))
            }
            "workspace/writeFile" => {
                let path = api::str_param(params, "path")?;
                let text = api::str_param(params, "text")?;
                let folders = lock(&self.folders).clone();
                super::workspace::write(&folders, Path::new(path), text)?;
                lock(&self.log).push(format!("Wrote {path}"));
                Ok(json!({"written": true}))
            }
            _ => Err(RpcError::new(api::INTERNAL_ERROR, format!("{method} isn't handled here"))),
        }
    }
}

fn clip(s: &str) -> String {
    s.chars().take(80).collect()
}
