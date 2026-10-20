//! Language servers in the window (`lsp` runs the servers): telling them
//! about the open tabs, their diagnostics as squiggles, gutter dots and a
//! problem list, completions in the completion popup, hover information,
//! go to definition and references, rename across files, formatting,
//! parameter hints, and the status bar's word on how the server is doing.
//! Every request is answered on a later frame; nothing here waits.

use super::StrataApp;
use super::completion::{ServerItem, ServerList};
use super::more::Choosing;
use crate::commands::Cmd;
use crate::editor::Marker;
use crate::lsp::edit::{self, FileEdit};
use crate::lsp::position::{self, Encoding, Pos};
use crate::lsp::server::{Diagnostic, Pending, Reply, RpcError};
use crate::lsp::hub::{MAX_INEXACT, Mapped};
use crate::lsp::{Hub, State, Trust, uri};
use egui::{Color32, Key, Pos2, Rect, vec2};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

/// Seconds the pointer rests on a word before its hover information shows.
const HOVER_DELAY: f64 = 0.5;
/// References listed at most (each needs its line read).
const MAX_LOCATIONS: usize = 500;
/// What the status bar says when hover finds nothing.
const NO_INFO: &str = "No information here";

/// What clicking the status bar's server item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StatusClick {
    Problems,
    Restart,
    /// Show the trust notice again.
    Ask,
}

/// Problems at a spot: (severity, message).
type Problems = Vec<(u8, String)>;
/// Char-range edits for one file.
type CharEdits = Vec<(Range<usize>, String)>;
use crate::document::FileId;
/// A file read off the UI thread for a change: decoded, its edits, and which file it was.
type ReadFile = (crate::document::Loaded, CharEdits, FileId);
/// An open tab's planned edits: (view, (editor id, version) planned on, edits).
type OpenEdit = (egui::Id, (u64, u64), CharEdits);

/// A change waiting for its unopened files to be read (see `lsp_apply_workspace_edit`).
struct Applying {
    rx: std::sync::mpsc::Receiver<Result<Vec<ReadFile>, String>>,
    open: Vec<OpenEdit>,
    back_to: egui::Id,
    count: usize,
}

/// A change waiting for its paths to be mapped on a worker.
struct Mapping {
    rx: std::sync::mpsc::Receiver<Vec<Mapped>>,
    files: Vec<FileEdit>,
    enc: Encoding,
    asked: Option<Vec<(PathBuf, (u64, u64))>>,
}

/// Lines read for a location list: (row, text).
type LineTexts = Vec<(usize, String)>;

/// A request in flight and what its answer is for.
struct Asked<T> {
    pending: Pending,
    what: T,
    /// `Hub::handoffs` when asked: an answer from before a document passed
    /// to another tab (maybe with other text) is void.
    epoch: u64,
}

/// Take a request's answer once it has come.
fn answered<T>(slot: &mut Option<Asked<T>>, epoch: u64) -> Option<(T, Reply)> {
    let reply = slot.as_mut()?.pending.poll()?;
    slot.take().map(|a| {
        let reply = if a.epoch == epoch { reply } else { Err(RpcError { code: -32801, message: "the file changed while the server worked; try again".into() }) };
        (a.what, reply)
    })
}

struct CompletionFor {
    view: egui::Id,
    start: usize,
}

struct GotoFor {
    definition: bool,
    enc: Encoding,
    word: String,
}

struct RenameFor {
    enc: Encoding,
    /// The open files' texts when asked (see `open_texts`).
    texts: Vec<(PathBuf, (u64, u64))>,
}

struct FormatFor {
    view: egui::Id,
    version: u64,
    enc: Encoding,
}

/// What a tab shows of its server's diagnostics.
struct Shown {
    stamp: u64,
    editor: u64,
    items: Arc<Vec<Diagnostic>>,
}

#[derive(Clone)]
struct HoverAt {
    view: egui::Id,
    at: usize,
    /// The word (or char) the pointer rests on: moving within it keeps the box.
    word: Range<usize>,
    anchor: Pos2,
    by_mouse: bool,
    /// (editor version, cursor) when shown: typing or moving hides a box from the keyboard.
    key: (u64, usize),
}

struct HoverBox {
    at: HoverAt,
    problems: Problems,
    info: Option<String>,
    rect: Option<Rect>,
    /// The contents grew since the box was last drawn.
    grow: bool,
}

#[derive(Default)]
struct HoverState {
    /// Where the pointer rests (view, word start) and since when.
    rest: Option<((egui::Id, usize), f64)>,
    /// The spot last shown or asked about, not asked again while the pointer stays.
    done: Option<(egui::Id, usize)>,
    pending: Option<Asked<HoverAt>>,
    shown: Option<HoverBox>,
}

struct RenameBox {
    view: egui::Id,
    at: usize,
    version: u64,
    old: String,
    text: String,
    anchor: Pos2,
    first: bool,
}

struct SignatureBox {
    view: egui::Id,
    line: usize,
    label: String,
    /// Byte range of the active parameter in `label`.
    active: Option<Range<usize>>,
    /// "2/3" when the server offers several signatures.
    count: Option<(usize, usize)>,
}

pub(super) struct LspState {
    pub hub: Rc<RefCell<Hub>>,
    /// This window, to the hub (a file is the server's in one window only).
    window: u64,
    shown: HashMap<egui::Id, Shown>,
    completion: Option<Asked<CompletionFor>>,
    resolve: Option<Asked<usize>>,
    goto: Option<Asked<GotoFor>>,
    rename_box: Option<RenameBox>,
    rename: Option<Asked<RenameFor>>,
    format: Option<Asked<FormatFor>>,
    hover: HoverState,
    signature: Option<Asked<(egui::Id, usize)>>,
    signature_box: Option<SignatureBox>,
    /// (view, editor version, cursor) last frame: typing only counts when the editor changed.
    last_edit: (egui::Id, u64, usize),
    /// A problem Next/Previous Problem just selected: its box shows at the
    /// caret on the next frame, once the view has scrolled to it.
    pending_problem: Option<(egui::Id, usize, Problems)>,
    /// A location list's lines being read off the UI thread, and its labels so far.
    labels: Option<(std::sync::mpsc::Receiver<LineTexts>, Vec<String>)>,
    /// A rename whose unopened files are being read.
    applying: Option<Applying>,
    /// A rename whose files are being told apart by identity, off the UI thread.
    mapping: Option<Mapping>,
}

fn next_window() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl LspState {
    /// The first window's: servers start only with a settings folder (not
    /// in tests), where trusted project folders are also remembered.
    pub fn new(config: Option<&Path>) -> Self {
        let mut hub = Hub::new(config.is_some());
        if let Some(dir) = config {
            hub.set_config_dir(dir);
        }
        Self::sharing(Rc::new(RefCell::new(hub)))
    }

    /// Another window's, sharing the servers.
    pub fn sharing(hub: Rc<RefCell<Hub>>) -> Self {
        Self {
            hub,
            window: next_window(),
            shown: HashMap::new(),
            completion: None,
            resolve: None,
            goto: None,
            rename_box: None,
            rename: None,
            format: None,
            hover: HoverState::default(),
            signature: None,
            signature_box: None,
            last_edit: (egui::Id::NULL, 0, 0),
            pending_problem: None,
            labels: None,
            applying: None,
            mapping: None,
        }
    }

    /// The rename box has the keyboard.
    pub fn renaming(&self) -> bool {
        self.rename_box.is_some()
    }

    /// A completion request is moot (the list was dismissed).
    pub fn drop_completion(&mut self) {
        self.completion = None;
    }
}

impl Drop for LspState {
    fn drop(&mut self) {
        // The window's files are closed in their servers.
        if let Ok(mut hub) = self.hub.try_borrow_mut() {
            hub.forget_window(self.window);
        }
    }
}

/// The word (identifier chars) around `at` in `ed`, or the char there.
fn word_around(ed: &crate::editor::Editor, at: usize) -> Range<usize> {
    let b = &ed.buffer;
    let line = b.line_of(at);
    let (ls, le) = (b.line_start(line), b.line_end(line));
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let (mut a, mut z) = (at, at);
    while a > ls && word(b.char(a - 1)) {
        a -= 1;
    }
    while z < le && word(b.char(z)) {
        z += 1;
    }
    if z == a { at..(at + 1).min(le.max(at)) } else { a..z }
}

/// "error", "warning"… for a severity.
fn severity_name(s: u8) -> &'static str {
    match s {
        1 => "error",
        2 => "warning",
        3 => "info",
        _ => "hint",
    }
}

/// Locations in a definition or references answer: Location, Location[]
/// or LocationLink[].
fn locations(v: &Value) -> Vec<(PathBuf, (Pos, Pos))> {
    let one = |l: &Value| -> Option<(PathBuf, (Pos, Pos))> {
        let uri = l.get("uri").or_else(|| l.get("targetUri"))?.as_str()?;
        let range = l.get("targetSelectionRange").or_else(|| l.get("range")).or_else(|| l.get("targetRange"))?;
        Some((uri::to_path(uri)?, position::range_from_json(range)?))
    };
    match v {
        Value::Array(a) => a.iter().filter_map(one).collect(),
        Value::Object(_) => one(v).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Hover contents as Markdown: MarkupContent, a MarkedString or a list of them.
fn hover_markdown(v: &Value) -> Option<String> {
    let piece = |c: &Value| -> Option<String> {
        match c {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => {
                let value = o.get("value")?.as_str()?;
                match (o.get("kind").and_then(Value::as_str), o.get("language").and_then(Value::as_str)) {
                    // Plain text: shown as is, not read as Markdown.
                    (Some("plaintext"), _) => Some(value.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")),
                    (_, Some(lang)) => Some(format!("```{lang}\n{value}\n```")),
                    _ => Some(value.to_owned()),
                }
            }
            _ => None,
        }
    };
    let contents = v.get("contents")?;
    let text = match contents {
        Value::Array(a) => a.iter().filter_map(piece).collect::<Vec<_>>().join("\n\n---\n\n"),
        c => piece(c)?,
    };
    (!text.trim().is_empty()).then_some(text)
}

impl StrataApp {
    /// Each frame, after the panes: the servers hear about edits, and their
    /// answers and diagnostics land.
    pub(super) fn lsp_frame(&mut self, ctx: &egui::Context) {
        let hub_rc = self.lsp.hub.clone();
        let mut hub = hub_rc.borrow_mut();
        hub.set_context(ctx);
        hub.configure(self.settings.lsp_enabled, &self.settings.lsp_servers, self.settings.lsp_trust_all);
        hub.tick();
        let window = self.lsp.window;
        let roots = self.sidebar.roots.clone();
        let mut open: Vec<PathBuf> = Vec::new();
        for d in &self.docs {
            let Some(path) = d.path.as_ref() else { continue };
            // Views of one file share its text: the first speaks for all.
            if d.large.is_some() || d.binary.is_some() || d.compare.is_some() || open.contains(path) {
                continue;
            }
            open.push(path.clone());
            let ed = &d.editor;
            hub.sync(window, path, (ed.id(), ed.version()), || ed.buffer.snapshot(), d.disk_mtime, &roots);
        }
        hub.end_frame(window, &open);
        let notes = hub.take_notes();
        drop(hub);
        if let Some(n) = notes.into_iter().last() {
            self.status = n;
        }
        self.lsp_show_diagnostics();
        self.lsp_answers(ctx);
    }

    /// Put each tab's latest diagnostics on its text, once per publication.
    fn lsp_show_diagnostics(&mut self) {
        let hub_rc = self.lsp.hub.clone();
        let hub = hub_rc.borrow();
        let window = self.lsp.window;
        for d in &mut self.docs {
            let id = d.view.id;
            let set = d.path.as_ref().and_then(|p| hub.diagnostics(window, p).map(|s| (p, s)));
            let Some((path, set)) = set else {
                if self.lsp.shown.remove(&id).is_some() || !d.editor.markers().is_empty() {
                    d.editor.set_markers(Vec::new());
                }
                continue;
            };
            if self.lsp.shown.get(&id).is_some_and(|s| s.stamp == set.stamp && s.editor == d.editor.id()) {
                continue;
            }
            let Some((server, version)) = hub.server_for(window, path) else { continue };
            // For an older text: the markers already there follow the edits
            // until the server catches up.
            if set.version.is_some_and(|v| v != version) {
                continue;
            }
            let enc = server.caps().encoding;
            let rope = d.editor.buffer.snapshot();
            let markers = set
                .items
                .iter()
                .enumerate()
                .map(|(k, it)| {
                    let a = position::from_lsp(&rope, it.range.0, enc);
                    let b = position::from_lsp(&rope, it.range.1, enc).max(a);
                    Marker { range: a..b, style: it.severity, id: k as u32 }
                })
                .collect();
            d.editor.set_markers(markers);
            self.lsp.shown.insert(id, Shown { stamp: set.stamp, editor: d.editor.id(), items: set.items.clone() });
        }
        let ids: Vec<egui::Id> = self.docs.iter().map(|d| d.view.id).collect();
        self.lsp.shown.retain(|id, _| ids.contains(id));
    }

    /// The problems (severity, message) at `at` in tab `i`.
    fn problems_at(&self, i: usize, at: usize) -> Problems {
        let d = &self.docs[i];
        let Some(shown) = self.lsp.shown.get(&d.view.id) else { return Vec::new() };
        d.editor
            .markers()
            .iter()
            .filter(|m| m.range.start <= at && (at < m.range.end || (m.range.is_empty() && at == m.range.start)))
            .filter_map(|m| shown.items.get(m.id as usize))
            .map(|it| (it.severity, describe(it)))
            .collect()
    }

    /// The problems starting on `line` of tab `i`.
    fn problems_on_line(&self, i: usize, line: usize) -> Problems {
        let d = &self.docs[i];
        let Some(shown) = self.lsp.shown.get(&d.view.id) else { return Vec::new() };
        let b = &d.editor.buffer;
        let (a, z) = (b.line_start(line), b.line_end(line));
        d.editor.markers().iter().filter(|m| m.range.start >= a && m.range.start <= z).filter_map(|m| shown.items.get(m.id as usize)).map(|it| (it.severity, describe(it))).collect()
    }

    /// The server for tab `i` if it's running and can do `provider`, with
    /// the file's path and the server's position encoding.
    fn lsp_for(&self, i: usize, provider: &str) -> Option<(PathBuf, Encoding)> {
        let d = self.docs.get(i)?;
        let path = d.path.clone()?;
        let hub = self.lsp.hub.borrow();
        let (server, _) = hub.server_for(self.lsp.window, &path)?;
        let caps = server.caps();
        caps.has(provider).then_some((path, caps.encoding))
    }

    /// Send a request to tab `i`'s server.
    /// Send a request to tab `i`'s server, after the tab's latest text (an
    /// edit made earlier this frame hasn't been synced yet): positions in the
    /// request are in that text.
    fn lsp_request(&self, i: usize, method: &'static str, params: Value) -> Option<Pending> {
        let d = self.docs.get(i)?;
        let path = d.path.clone()?;
        let mut hub = self.lsp.hub.borrow_mut();
        let ed = &d.editor;
        hub.sync(self.lsp.window, &path, (ed.id(), ed.version()), || ed.buffer.snapshot(), d.disk_mtime, &self.sidebar.roots);
        let (server, _) = hub.server_for(self.lsp.window, &path)?;
        Some(server.request(method, params))
    }

    /// `{"textDocument": …, "position": …}` for char `at` of tab `i`.
    fn lsp_position(&self, i: usize, path: &Path, at: usize, enc: Encoding) -> Value {
        let rope = self.docs[i].editor.buffer.snapshot();
        json!({"textDocument": {"uri": uri::from_path(&self.lsp.hub.borrow().server_path(path))}, "position": position::to_lsp(&rope, at, enc).to_json()})
    }

    fn lsp_error(&mut self, what: &str, e: &crate::lsp::server::RpcError) {
        if !e.is_benign() {
            self.status = format!("{what}: {}", e.message.lines().next().unwrap_or_default());
        }
    }

    /// Answers that came in since last frame.
    fn lsp_answers(&mut self, ctx: &egui::Context) {
        let epoch = self.lsp.hub.borrow().handoffs();
        // Before a new rename answer, so its files are read on a later frame.
        self.lsp_mapped();
        self.lsp_applied();
        if let Some((what, reply)) = answered(&mut self.lsp.completion, epoch) {
            self.lsp_completion_answer(what, reply);
        }
        if let Some((i, reply)) = answered(&mut self.lsp.resolve, epoch) {
            match reply {
                Ok(v) => self.completion.resolved(i, &v),
                Err(_) => self.completion.mark_resolved(i),
            }
        }
        if let Some((what, reply)) = answered(&mut self.lsp.goto, epoch) {
            self.lsp_goto_answer(ctx, what, reply);
        }
        if let Some((what, reply)) = answered(&mut self.lsp.rename, epoch) {
            match reply.map_err(|e| e.message).and_then(|v| edit::workspace_edit(&v)) {
                Ok(files) if files.iter().all(|f| f.edits.is_empty()) => self.status = "Nothing to rename here".into(),
                Ok(files) => match self.lsp_apply_workspace_edit(files, what.enc, Some(&what.texts)) {
                    Ok(msg) => self.status = msg,
                    Err(e) => self.status = format!("Couldn't rename: {e}"),
                },
                Err(e) => self.status = format!("Couldn't rename: {e}"),
            }
        }
        if let Some((what, reply)) = answered(&mut self.lsp.format, epoch) {
            self.lsp_format_answer(what, reply);
        }
        if let Some((at, reply)) = answered(&mut self.lsp.hover.pending, epoch) {
            if let Ok(v) = reply
                && let Some(text) = hover_markdown(&v)
            {
                if self.status == NO_INFO {
                    self.status.clear();
                }
                match &mut self.lsp.hover.shown {
                    Some(b) if b.at.view == at.view && b.at.at == at.at => {
                        b.info = Some(text);
                        b.grow = true;
                    }
                    _ => self.lsp.hover.shown = Some(HoverBox { at, problems: Vec::new(), info: Some(text), rect: None, grow: false }),
                }
            } else if !at.by_mouse && self.lsp.hover.shown.is_none() {
                self.status = NO_INFO.into();
            }
        }
        if let Some((rx, labels)) = &mut self.lsp.labels
            && let Ok(lines) = rx.try_recv()
        {
            for (k, text) in lines {
                if let Some(l) = labels.get_mut(k).filter(|_| !text.is_empty()) {
                    l.push_str("  ");
                    l.push_str(&text);
                }
            }
            let labels = std::mem::take(labels);
            self.lsp.labels = None;
            self.palette.update_choices(labels);
        }
        if let Some(((view, line), reply)) = answered(&mut self.lsp.signature, epoch) {
            self.lsp_signature_answer(view, line, reply);
        }
        // The highlighted completion's details, asked for once.
        if self.completion.is_open() && self.lsp.resolve.is_none()
            && let Some((k, raw)) = self.completion.selected_unresolved()
        {
            let resolves = self.docs[self.active].path.as_ref().and_then(|p| self.lsp.hub.borrow().server_for(self.lsp.window, p).map(|(s, _)| s.caps().resolves_completions()));
            match resolves {
                Some(true) => self.lsp.resolve = self.lsp_request(self.active, "completionItem/resolve", raw).map(|pending| Asked { epoch: self.lsp.hub.borrow().handoffs(), pending, what: k }),
                _ => self.completion.mark_resolved(k),
            }
        }
    }

    // ---- completion ------------------------------------------------------------------

    /// After the completion popup caught up with typing: ask the server
    /// when a word starts, after a trigger character (`.`), when its last
    /// list was incomplete, or on Ctrl+Space (`explicit`).
    pub(super) fn lsp_completion(&mut self, typed: &str, explicit: bool) {
        let i = self.active;
        let d = &self.docs[i];
        let key = (d.view.id, d.editor.version(), d.editor.primary().head);
        let edited = key != self.lsp.last_edit;
        self.lsp.last_edit = key;
        if !explicit && (!edited || typed.is_empty() || d.editor.selections().len() >= 50) {
            return;
        }
        let Some(path) = d.path.clone() else { return };
        let caps = {
            let hub = self.lsp.hub.borrow();
            let Some((server, _)) = hub.server_for(self.lsp.window, &path) else { return };
            server.caps()
        };
        self.lsp_signature_typed(typed, &caps.signature_triggers, explicit);
        if !caps.has("completionProvider") {
            return;
        }
        let d = &self.docs[i];
        let head = d.editor.primary().head;
        let start = d.editor.word_prefix_at_cursor().map_or(head, |(s, _)| s);
        let trigger = caps.completion_triggers.iter().find(|t| typed.ends_with(t.as_str())).cloned();
        let auto = d.local.effective.auto_complete;
        let typed_word = typed.chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_');
        let have = self.completion.server_state(d.view.id);
        let asked = self.lsp.completion.as_ref().map(|a| (a.what.view, a.what.start));
        let incomplete = have.is_some_and(|(s, inc)| s == start && inc);
        let want = explicit
            || (auto && trigger.is_some() && start == head)
            || (auto && typed_word && (incomplete || (have.is_none_or(|(s, _)| s != start) && asked != Some((d.view.id, start)))));
        if !want {
            return;
        }
        let context = match (&trigger, explicit) {
            (Some(t), false) if start == head => json!({"triggerKind": 2, "triggerCharacter": t}),
            _ if incomplete => json!({"triggerKind": 3}),
            _ => json!({"triggerKind": 1}),
        };
        let mut params = self.lsp_position(i, &path, head, caps.encoding);
        params["context"] = context;
        let view = d.view.id;
        if let Some(pending) = self.lsp_request(i, "textDocument/completion", params) {
            self.lsp.completion = Some(Asked { epoch: self.lsp.hub.borrow().handoffs(), pending, what: CompletionFor { view, start } });
        }
    }

    fn lsp_completion_answer(&mut self, what: CompletionFor, reply: Reply) {
        let v = match reply {
            Ok(v) => v,
            Err(e) => return self.lsp_error("Completions", &e),
        };
        let (items, incomplete) = match &v {
            Value::Array(a) => (a.clone(), false),
            Value::Object(o) => (o.get("items").and_then(Value::as_array).cloned().unwrap_or_default(), o.get("isIncomplete").and_then(Value::as_bool).unwrap_or(false)),
            _ => return,
        };
        let items: Vec<ServerItem> = items.iter().filter_map(ServerItem::from_json).collect();
        let i = self.active;
        if self.docs[i].view.id != what.view {
            return;
        }
        let list = ServerList { view: what.view, start: what.start, items, incomplete };
        self.completion.set_server_items(&mut self.docs[i], list, &self.snippets.lib);
    }

    /// Insert the server completion chosen in the popup: its text edit (or
    /// text) in place of the word, a snippet through the snippet engine, and
    /// the edits that come with it (an import), as the server gave them.
    pub(super) fn lsp_insert_completion(&mut self) {
        if let Some(item) = self.completion.take_server_item() {
            self.lsp_insert_server_item(item);
        }
    }

    pub(super) fn lsp_insert_server_item(&mut self, item: ServerItem) {
        let i = self.active;
        let enc = self.docs[i].path.as_ref().and_then(|p| self.lsp.hub.borrow().server_for(self.lsp.window, p).map(|(s, _)| s.caps().encoding)).unwrap_or_default();
        let d = &mut self.docs[i];
        if d.editor.selections().len() > 1 {
            // Several cursors: the same text at each, in place of the word
            // typed there (the server's ranges are for the primary cursor
            // only, and its extra edits would apply once, so they're left out).
            let insert = edit::normalize_breaks(&item.insert, d.editor.buffer.line_ending().as_str());
            // Each cursor's own partial word goes (they may differ: `pr`, `pri`).
            let b = &d.editor.buffer;
            let words: Vec<(Range<usize>, String)> = d
                .editor
                .selections()
                .iter()
                .map(|s| {
                    let mut a = s.start();
                    let ls = b.line_start(b.line_of(a));
                    while s.is_empty() && a > ls && (b.char(a - 1).is_alphanumeric() || b.char(a - 1) == '_') {
                        a -= 1;
                    }
                    (a..s.end(), String::new())
                })
                .filter(|(r, _)| !r.is_empty())
                .collect();
            d.editor.apply_edits(words);
            if item.snippet {
                let snippet = crate::snippets::Snippet { trigger: String::new(), description: String::new(), scope: String::new(), content: insert, path: None };
                self.insert_snippet(&snippet, "");
            } else {
                d.editor.insert_text(&insert);
            }
            return;
        }
        let rope = d.editor.buffer.snapshot();
        let eol = d.editor.buffer.line_ending().as_str();
        let head = d.editor.primary().head;
        let word_start = d.editor.word_prefix_at_cursor().map_or(head, |(s, _)| s);
        let line = d.editor.buffer.line_of(head);
        // The edit's range was for the text when the server was asked; what
        // was typed since (up to the cursor) is replaced too.
        let (start, end) = match item.range {
            Some((a, b)) => {
                let (a, b) = (position::from_lsp(&rope, a, enc), position::from_lsp(&rope, b, enc));
                if a <= head && d.editor.buffer.line_of(a) == line { (a, b.max(head)) } else { (word_start, head) }
            }
            None => (word_start, head),
        };
        let insert = edit::normalize_breaks(&item.insert, eol);
        let extra: Vec<(Range<usize>, String)> = edit::to_char_edits(&rope, &item.additional, enc, eol).unwrap_or_default().into_iter().filter(|(r, _)| r.end <= start || r.start >= end).collect();
        let shift = |p: usize| -> usize {
            let delta: isize = extra.iter().filter(|(r, _)| r.end <= p).map(|(r, t)| t.chars().count() as isize - r.len() as isize).sum();
            (p as isize + delta) as usize
        };
        if item.snippet {
            let (s, e, h) = (shift(start), shift(end), shift(head));
            d.editor.apply_edits(extra.clone());
            if e > h {
                d.editor.select_ranges(&[h..e], 0);
                d.editor.replace_ranges(vec![(h..e, String::new())]);
            }
            d.editor.select_ranges(&[h..h], 0);
            let prefix = d.editor.buffer.slice(s..h);
            let snippet = crate::snippets::Snippet { trigger: prefix.clone(), description: String::new(), scope: String::new(), content: insert, path: None };
            self.insert_snippet(&snippet, &prefix);
        } else {
            let cursor = shift(start) + insert.chars().count();
            let mut edits = extra;
            edits.push((start..end, insert));
            d.editor.apply_edits_then(edits, cursor);
            d.view.reveal();
        }
    }

    // ---- parameter hints -----------------------------------------------------------------

    /// After typing: ask for parameter hints after `(` or `,` (the server's
    /// trigger characters), and again after `)` to learn when they end.
    fn lsp_signature_typed(&mut self, typed: &str, triggers: &[String], explicit: bool) {
        let shown = self.lsp.signature_box.is_some();
        let hit = triggers.iter().any(|t| typed.ends_with(t.as_str())) || (shown && typed.ends_with(')'));
        if hit && !explicit {
            self.lsp_ask_signature();
        }
    }

    fn lsp_ask_signature(&mut self) {
        let i = self.active;
        let Some((path, enc)) = self.lsp_for(i, "signatureHelpProvider") else { return };
        let d = &self.docs[i];
        let head = d.editor.primary().head;
        let line = d.editor.buffer.line_of(head);
        let params = self.lsp_position(i, &path, head, enc);
        let view = d.view.id;
        if let Some(pending) = self.lsp_request(i, "textDocument/signatureHelp", params) {
            self.lsp.signature = Some(Asked { epoch: self.lsp.hub.borrow().handoffs(), pending, what: (view, line) });
        }
    }

    fn lsp_signature_answer(&mut self, view: egui::Id, line: usize, reply: Reply) {
        let Ok(v) = reply else { return };
        let sigs = v.get("signatures").and_then(Value::as_array).cloned().unwrap_or_default();
        if sigs.is_empty() {
            self.lsp.signature_box = None;
            return;
        }
        let k = v.get("activeSignature").and_then(Value::as_u64).unwrap_or(0) as usize;
        let sig = &sigs[k.min(sigs.len() - 1)];
        let label = sig.get("label").and_then(Value::as_str).unwrap_or_default().to_owned();
        let p = sig.get("activeParameter").or_else(|| v.get("activeParameter")).and_then(Value::as_u64).unwrap_or(0) as usize;
        let active = sig.get("parameters").and_then(Value::as_array).and_then(|ps| ps.get(p)).and_then(|param| match param.get("label")? {
            Value::String(s) => label.find(s.as_str()).map(|at| at..at + s.len()),
            // [start, end] in UTF-16 units of the label.
            Value::Array(a) => {
                let (s, e) = (a.first()?.as_u64()? as usize, a.get(1)?.as_u64()? as usize);
                let byte = |units: usize| {
                    let mut n = 0;
                    label.char_indices().find(|(_, c)| {
                        let past = n >= units;
                        n += c.len_utf16();
                        past
                    }).map_or(label.len(), |(i, _)| i)
                };
                Some(byte(s)..byte(e))
            }
            _ => None,
        });
        let count = (sigs.len() > 1).then_some((k.min(sigs.len() - 1) + 1, sigs.len()));
        self.lsp.signature_box = Some(SignatureBox { view, line, label, active, count });
    }

    // ---- commands -------------------------------------------------------------------------

    pub(super) fn lsp_command(&mut self, ctx: &egui::Context, cmd: Cmd) {
        match cmd {
            Cmd::LspHover => self.lsp_hover_at_cursor(),
            Cmd::LspRename => self.lsp_start_rename(),
            Cmd::LspFormat => self.lsp_format(),
            Cmd::LspProblems => self.lsp_problems(ctx),
            Cmd::LspNextProblem => self.lsp_step_problem(true),
            Cmd::LspPrevProblem => self.lsp_step_problem(false),
            Cmd::LspSignatureHelp => {
                if self.lsp_for(self.active, "signatureHelpProvider").is_some() {
                    self.lsp_ask_signature();
                } else {
                    self.status = self.lsp_unavailable("Parameter hints");
                }
            }
            Cmd::LspRestart => {
                self.lsp.hub.borrow_mut().restart_all();
                for d in &mut self.docs {
                    d.editor.set_markers(Vec::new());
                }
                self.lsp.shown.clear();
                self.status = "Restarting language servers".into();
            }
            _ => {}
        }
    }

    /// Why a server feature isn't there for the current file.
    fn lsp_unavailable(&self, what: &str) -> String {
        let d = &self.docs[self.active];
        if !self.settings.lsp_enabled {
            return format!("{what}: language servers are off (lsp_enabled)");
        }
        if d.path.as_ref().is_some_and(|p| self.lsp.hub.borrow().follows_other_tab(self.lsp.window, p)) {
            return format!("{what}: this file is also open in another tab (or window), which its language server follows");
        }
        match d.path.as_ref().and_then(|p| self.lsp.hub.borrow().state_for(p)) {
            None => format!("{what}: no language server for this file"),
            Some((name, State::Starting)) => format!("{what}: {name} is still starting"),
            Some((name, State::Ready | State::Busy(_))) => format!("{what}: {name} doesn't offer this"),
            Some((name, State::NotTrusted)) => format!("{what}: {name} isn't started for this folder (see the notice, or click the status bar)"),
            Some((name, _)) => format!("{what}: {name} isn't running"),
        }
    }

    /// Go to Definition (`definition`) or Find References through the
    /// server. False when there's no server for it: the caller falls back
    /// to Strata's own symbol search.
    pub(super) fn lsp_goto(&mut self, definition: bool) -> bool {
        let i = self.active;
        let provider = if definition { "definitionProvider" } else { "referencesProvider" };
        let Some((path, enc)) = self.lsp_for(i, provider) else { return false };
        let head = self.docs[i].editor.primary().head;
        let mut params = self.lsp_position(i, &path, head, enc);
        let method = if definition { "textDocument/definition" } else {
            params["context"] = json!({"includeDeclaration": true});
            "textDocument/references"
        };
        let word = self.word_at_cursor().unwrap_or_default();
        let Some(pending) = self.lsp_request(i, method, params) else { return false };
        self.lsp.goto = Some(Asked { epoch: self.lsp.hub.borrow().handoffs(), pending, what: GotoFor { definition, enc, word } });
        true
    }

    fn lsp_goto_answer(&mut self, ctx: &egui::Context, what: GotoFor, reply: Reply) {
        let found = match reply {
            Ok(v) => {
                let found = locations(&v);
                let paths: Vec<PathBuf> = found.iter().map(|(p, _)| p.clone()).collect();
                let hub = self.lsp.hub.borrow();
                // Many files: listed by the server's names; the one chosen is mapped then.
                if hub.count_inexact(&paths) > MAX_INEXACT {
                    found
                } else {
                    let mapped = hub.client_paths(self.lsp.window, &paths);
                    found.into_iter().zip(mapped).map(|((_, r), m)| (match m { Mapped::Tab { path, .. } | Mapped::Unopened(path) => path }, r)).collect()
                }
            }
            Err(e) if e.is_benign() => return,
            Err(_) => Vec::new(),
        };
        if found.is_empty() {
            // The server knows nothing here: Strata's own search may.
            if what.definition {
                self.goto_definition_in(Some(ctx.clone()));
            } else {
                self.goto_reference();
            }
            return;
        }
        if found.len() == 1 {
            let (p, r) = found.into_iter().next().expect("one");
            self.push_jump();
            self.lsp_open_location(&p, r, what.enc);
            return;
        }
        let kind = if what.definition { "Definitions" } else { "References" };
        let hint = if what.word.is_empty() { kind.to_owned() } else { format!("{kind} of {}", what.word) };
        let spots: Vec<(PathBuf, (Pos, Pos), Encoding)> = found.into_iter().take(MAX_LOCATIONS).map(|(p, r)| (p, r, what.enc)).collect();
        self.show_locations(ctx, spots, &hint);
    }

    /// "file.rs:12:5" for a location.
    fn place(&self, p: &Path, r: (Pos, Pos)) -> String {
        format!("{}:{}:{}", self.sidebar.display_path(p), r.0.line + 1, r.0.col + 1)
    }

    /// List locations in the palette as "file.rs:12:5  the line's text". The
    /// lines of open files are there at once; files that aren't open are
    /// read on a worker thread and their lines filled in when it's done.
    fn show_locations(&mut self, ctx: &egui::Context, spots: Vec<(PathBuf, (Pos, Pos), Encoding)>, hint: &str) {
        let mut labels = Vec::with_capacity(spots.len());
        let mut unread: Vec<(usize, PathBuf, usize)> = Vec::new();
        for (k, (p, r, _)) in spots.iter().enumerate() {
            let line = r.0.line as usize;
            let place = self.place(p, *r);
            match self.docs.iter().find(|d| d.path.as_deref().is_some_and(|dp| uri::normalize(dp) == *p)) {
                Some(d) if line < d.editor.buffer.len_lines() => labels.push(format!("{place}  {}", label_text(&d.editor.buffer.line_text(line)))),
                Some(_) => labels.push(place),
                None => {
                    labels.push(place);
                    unread.push((k, p.clone(), line));
                }
            }
        }
        self.lsp.labels = None;
        if !unread.is_empty() {
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx2 = ctx.clone();
            let spawned = std::thread::Builder::new().name("lsp-labels".into()).spawn(move || {
                let mut files: HashMap<PathBuf, Vec<String>> = HashMap::new();
                let lines: Vec<(usize, String)> = unread
                    .into_iter()
                    .map(|(k, p, line)| {
                        let text = files.entry(p.clone()).or_insert_with(|| std::fs::read(&p).map(|b| crate::encoding::detect(&b).text.lines().map(str::to_owned).collect()).unwrap_or_default());
                        (k, text.get(line).map(|t| label_text(t)).unwrap_or_default())
                    })
                    .collect();
                let _ = tx.send(lines);
                ctx2.request_repaint();
            });
            if spawned.is_ok() {
                self.lsp.labels = Some((rx, labels.clone()));
            }
        }
        self.choose(ctx, Choosing::Locations(spots), hint, labels);
    }

    /// Open `path` and select `range` (server positions in `enc`).
    pub(super) fn lsp_open_location(&mut self, path: &Path, range: (Pos, Pos), enc: Encoding) {
        // The tab showing that file, if one does (lists of many files aren't mapped ahead).
        let path = &self.lsp.hub.borrow().client_path(self.lsp.window, path);
        let open = self.docs.iter().position(|d| d.path.as_deref().is_some_and(|dp| uri::normalize(dp) == uri::normalize(path)));
        match open {
            Some(i) => self.activate(i),
            None => self.open_path(path.to_owned(), false),
        }
        let d = self.doc();
        if d.path.as_deref().is_none_or(|dp| uri::normalize(dp) != uri::normalize(path)) {
            return;
        }
        let rope = d.editor.buffer.snapshot();
        let (a, b) = (position::from_lsp(&rope, range.0, enc), position::from_lsp(&rope, range.1, enc));
        d.editor.select_ranges(&[a..b.max(a)], 0);
        d.view.reveal_centered();
        d.view.focus();
    }

    fn lsp_hover_at_cursor(&mut self) {
        let i = self.active;
        let d = &self.docs[i];
        let head = d.editor.primary().head;
        let Some(caret) = d.view.caret_rect() else { return };
        let at = HoverAt { view: d.view.id, at: head, word: word_around(&d.editor, head), anchor: caret.left_bottom(), by_mouse: false, key: (d.editor.version(), head) };
        let problems = self.problems_at(i, head);
        let asked = self.lsp_ask_hover(i, &at);
        if !problems.is_empty() {
            self.lsp.hover.shown = Some(HoverBox { at, problems, info: None, rect: None, grow: false });
        } else if !asked {
            self.status = self.lsp_unavailable("Hover information");
        }
    }

    /// Ask tab `i`'s server about the spot; false if it can't say.
    fn lsp_ask_hover(&mut self, i: usize, at: &HoverAt) -> bool {
        let Some((path, enc)) = self.lsp_for(i, "hoverProvider") else { return false };
        let params = self.lsp_position(i, &path, at.at, enc);
        match self.lsp_request(i, "textDocument/hover", params) {
            Some(pending) => {
                self.lsp.hover.pending = Some(Asked { epoch: self.lsp.hub.borrow().handoffs(), pending, what: at.clone() });
                true
            }
            None => false,
        }
    }

    fn lsp_start_rename(&mut self) {
        let i = self.active;
        if self.lsp_for(i, "renameProvider").is_none() {
            self.status = self.lsp_unavailable("Rename Symbol");
            return;
        }
        let d = &self.docs[i];
        let head = d.editor.primary().head;
        let w = word_around(&d.editor, head);
        let old = d.editor.buffer.slice(w.clone());
        if !old.chars().any(|c| c.is_alphanumeric() || c == '_') {
            self.status = "Rename Symbol: put the cursor on a name".into();
            return;
        }
        let Some(caret) = d.view.caret_rect() else { return };
        self.lsp.rename_box = Some(RenameBox { view: d.view.id, at: w.start, version: d.editor.version(), text: old.clone(), old, anchor: caret.left_bottom(), first: true });
    }

    fn lsp_rename_submit(&mut self, b: RenameBox) {
        let Some(i) = self.docs.iter().position(|d| d.view.id == b.view) else { return };
        let name = b.text.trim().to_owned();
        if name.is_empty() || name == b.old {
            return;
        }
        let Some((path, enc)) = self.lsp_for(i, "renameProvider") else { return };
        let mut params = self.lsp_position(i, &path, b.at, enc);
        params["newName"] = json!(name);
        // The text of every open file now: an answer is only good for that text.
        let texts = self.open_texts();
        if let Some(pending) = self.lsp_request(i, "textDocument/rename", params) {
            self.lsp.rename = Some(Asked { epoch: self.lsp.hub.borrow().handoffs(), pending, what: RenameFor { enc, texts } });
            self.status = format!("Renaming {} to {name}…", b.old);
        }
    }

    /// (path, (editor id, editor version)) of every file open in this window.
    fn open_texts(&self) -> Vec<(PathBuf, (u64, u64))> {
        self.docs.iter().filter_map(|d| Some((uri::normalize(d.path.as_deref()?), (d.editor.id(), d.editor.version())))).collect()
    }

    /// Apply a server's edits to every file they touch, each as one undo
    /// step in its tab; files that aren't open are opened (left unsaved for
    /// review). All or nothing: every file is checked and its edits worked
    /// out on the text its tab holds before any changes. Files that aren't
    /// open are read and decoded on a worker thread ("Renaming… (n files)")
    /// and the whole edit is applied when that's done, after checking again
    /// that nothing changed meanwhile. With `asked`, the open files' texts
    /// when the server was asked: an edit for a file changed since is refused.
    /// Returns the status message.
    pub(super) fn lsp_apply_workspace_edit(&mut self, files: Vec<FileEdit>, enc: Encoding, asked: Option<&[(PathBuf, (u64, u64))]>) -> Result<String, String> {
        if self.lsp.applying.is_some() || self.lsp.mapping.is_some() {
            return Err("another change is still being applied".into());
        }
        let paths: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
        let hub = self.lsp.hub.borrow();
        if hub.count_inexact(&paths) <= MAX_INEXACT {
            let mapped = hub.client_paths(self.lsp.window, &paths);
            drop(hub);
            return self.lsp_apply_mapped(files, mapped, enc, asked);
        }
        // Many files to tell apart by identity: stat them on a worker first.
        let (docs, window, ctx) = (hub.doc_tabs(), self.lsp.window, hub.context());
        drop(hub);
        let (tx, rx) = std::sync::mpsc::channel();
        let count = files.len();
        std::thread::Builder::new()
            .name("lsp-map".into())
            .spawn(move || {
                let _ = tx.send(crate::lsp::hub::map_paths(&docs, window, &paths));
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            })
            .map_err(|e| format!("couldn't start looking at the files: {e}"))?;
        self.lsp.mapping = Some(Mapping { rx, files, enc, asked: asked.map(<[_]>::to_vec) });
        Ok(format!("Renaming… ({count} files)"))
    }

    /// A change's paths were mapped on a worker: go on with it.
    fn lsp_mapped(&mut self) {
        let Some(m) = &self.lsp.mapping else { return };
        let mapped = match m.rx.try_recv() {
            Ok(mapped) => mapped,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Vec::new(),
        };
        let m = self.lsp.mapping.take().expect("checked");
        if mapped.len() != m.files.len() {
            self.status = "Couldn't rename: looking at the files failed".into();
            return;
        }
        self.status = match self.lsp_apply_mapped(m.files, mapped, m.enc, m.asked.as_deref()) {
            Ok(msg) => msg,
            Err(e) => format!("Couldn't rename: {e}"),
        };
    }

    /// `lsp_apply_workspace_edit` once each file's path is mapped to a tab
    /// (in this window, or another: refused) or to its real path.
    fn lsp_apply_mapped(&mut self, files: Vec<FileEdit>, mapped: Vec<Mapped>, enc: Encoding, asked: Option<&[(PathBuf, (u64, u64))]>) -> Result<String, String> {
        let window = self.lsp.window;
        let mut open: Vec<OpenEdit> = Vec::new();
        let mut unread: Vec<(PathBuf, Vec<edit::TextEdit>, FileId)> = Vec::new();
        for (mut f, m) in files.into_iter().zip(mapped).filter(|(f, _)| !f.edits.is_empty()) {
            let server_name = f.path.clone();
            f.path = match m {
                Mapped::Tab { window: w, path } if w == window => path,
                Mapped::Tab { .. } => return Err(format!("{} is open in another window: rename from there, or close it there", file_name(&f.path))),
                Mapped::Unopened(real) => real,
            };
            // (A tab the servers don't know of, a large file say, still counts by its name.)
            let at = self.docs.iter().position(|d| d.path.as_deref().is_some_and(|p| uri::normalize(p) == f.path || uri::normalize(p) == server_name));
            let then = asked.map(|a| a.iter().find(|(p, _)| *p == f.path).map(|(_, k)| *k));
            match at {
                Some(i) => {
                    let d = &self.docs[i];
                    if d.editor.read_only || d.binary.is_some() {
                        return Err(format!("{} is read-only", d.name()));
                    }
                    let key = (d.editor.id(), d.editor.version());
                    if then.is_some_and(|k| k != Some(key)) {
                        return Err(changed(&f.path));
                    }
                    if let (Some(v), Some((_, now))) = (f.version, self.lsp.hub.borrow().server_for(window, &f.path))
                        && v != now
                    {
                        return Err(changed(&f.path));
                    }
                    let edits = edit::to_char_edits(&d.editor.buffer.snapshot(), &f.edits, enc, d.editor.buffer.line_ending().as_str())?;
                    open.push((d.view.id, key, edits));
                }
                None => {
                    if then.is_some_and(|k| k.is_some()) {
                        // Open (maybe with unsaved edits) when asked, closed now.
                        return Err(changed(&f.path));
                    }
                    if self.link.is_open_elsewhere(&f.path) || self.link.is_open_elsewhere(&server_name) {
                        return Err(format!("{} is open in another window: rename from there, or close it there", file_name(&f.path)));
                    }
                    // The file as the server's answer found it (a stat, no read).
                    let Some(now) = crate::document::file_id(&f.path).filter(|_| f.path.is_file()) else {
                        return Err(format!("{} doesn't exist", f.path.display()));
                    };
                    unread.push((f.path, f.edits, now));
                }
            }
        }
        let back_to = self.docs[self.active].view.id;
        let count = open.len() + unread.len();
        if unread.is_empty() {
            return self.lsp_finish_edit(open, Vec::new(), back_to, count);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let large = self.large_file_bytes();
        let ctx = self.lsp.hub.borrow().context();
        let job = move || {
            let read = (|| {
                let mut out = Vec::new();
                for (path, edits, expected) in unread {
                    if crate::document::file_id(&path) != Some(expected) {
                        return Err(changed(&path));
                    }
                    // Decoded as its tab will hold it (encoding, BOM), so offsets match.
                    let loaded = crate::document::Document::load_text(path.clone(), large).map_err(|e| format!("couldn't read {}: {e}", file_name(&path)))?;
                    let loaded = loaded.ok_or_else(|| format!("{} isn't a text file", file_name(&path)))?;
                    let char_edits = edit::to_char_edits(&loaded.buffer.snapshot(), &edits, enc, loaded.buffer.line_ending().as_str())?;
                    // Last, right before handing back: still the bytes planned on, and the same file.
                    let again = std::fs::read(&path).map_err(|e| format!("couldn't read {}: {e}", file_name(&path)))?;
                    if crate::document::content_hash(&again) != loaded.hash || crate::document::file_id(&path) != Some(expected) {
                        return Err(changed(&path));
                    }
                    out.push((loaded, char_edits, expected));
                }
                Ok(out)
            })();
            let _ = tx.send(read);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        };
        std::thread::Builder::new().name("lsp-apply".into()).spawn(job).map_err(|e| format!("couldn't start reading the files: {e}"))?;
        self.lsp.applying = Some(Applying { rx, open, back_to, count });
        Ok(format!("Renaming… ({count} files)"))
    }

    /// The files read for a change came in: check again, then apply it all.
    fn lsp_applied(&mut self) {
        let Some(job) = &self.lsp.applying else { return };
        let read = match job.rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("reading the files failed".into()),
        };
        let job = self.lsp.applying.take().expect("checked");
        let result = read.and_then(|new| self.lsp_finish_edit(job.open, new, job.back_to, job.count));
        self.status = match result {
            Ok(msg) => msg,
            Err(e) => format!("Couldn't rename: {e}"),
        };
    }

    /// Apply checked edits: to open tabs (unchanged since they were
    /// planned), and to files read off the UI thread (unchanged on disk and
    /// still not open), which become tabs. Nothing changes if any check fails.
    fn lsp_finish_edit(&mut self, open: Vec<OpenEdit>, new: Vec<ReadFile>, back_to: egui::Id, count: usize) -> Result<String, String> {
        for (id, key, _) in &open {
            match self.docs.iter().find(|d| d.view.id == *id) {
                Some(d) if (d.editor.id(), d.editor.version()) == *key => {}
                Some(d) => return Err(changed(d.path.as_deref().unwrap_or(Path::new("the file")))),
                None => return Err("a file was closed meanwhile; try again".into()),
            }
        }
        // Opened meanwhile (here, in another window, under any name)?
        let paths: Vec<PathBuf> = new.iter().map(|(l, _, _)| l.path.clone()).collect();
        let now_open = self.lsp.hub.borrow().client_paths(self.lsp.window, &paths);
        for ((loaded, _, id), m) in new.iter().zip(&now_open) {
            let p = &loaded.path;
            if matches!(m, Mapped::Tab { .. }) || self.docs.iter().any(|d| d.path.as_deref().is_some_and(|dp| uri::normalize(dp) == uri::normalize(p))) || self.link.is_open_elsewhere(p) || crate::document::file_id(p) != Some(*id) {
                return Err(changed(p));
            }
        }
        let mut places = 0;
        for (id, _, edits) in open {
            places += edits.len();
            let i = self.docs.iter().position(|d| d.view.id == id).expect("checked above");
            self.docs[i].editor.apply_edits(edits);
        }
        let opened = new.len();
        for (loaded, edits, _) in new {
            places += edits.len();
            self.remember_file(&loaded.path.clone());
            self.push_doc(crate::document::Document::from_loaded(loaded));
            self.doc_settings(self.active);
            self.docs[self.active].editor.apply_edits(edits);
        }
        if let Some(i) = self.docs.iter().position(|d| d.view.id == back_to) {
            self.activate(i);
        }
        let in_files = if count == 1 { "1 file".to_owned() } else { format!("{count} files") };
        let mut msg = format!("Changed {places} {} in {in_files}", if places == 1 { "place" } else { "places" });
        if opened > 0 {
            msg.push_str(&format!(" ({opened} opened to review; save to keep the changes)"));
        }
        Ok(msg)
    }

    fn lsp_format(&mut self) {
        let i = self.active;
        let Some((path, enc)) = self.lsp_for(i, "documentFormattingProvider") else {
            self.status = self.lsp_unavailable("Format Document");
            return;
        };
        let d = &self.docs[i];
        let options = json!({"tabSize": d.editor.tab_width, "insertSpaces": d.editor.use_spaces, "trimTrailingWhitespace": true, "insertFinalNewline": true});
        let what = FormatFor { view: d.view.id, version: d.editor.version(), enc };
        let params = json!({"textDocument": {"uri": uri::from_path(&self.lsp.hub.borrow().server_path(&path))}, "options": options});
        if let Some(pending) = self.lsp_request(i, "textDocument/formatting", params) {
            self.lsp.format = Some(Asked { epoch: self.lsp.hub.borrow().handoffs(), pending, what });
        }
    }

    fn lsp_format_answer(&mut self, what: FormatFor, reply: Reply) {
        let v = match reply {
            Ok(v) => v,
            Err(e) => {
                self.status = format!("Couldn't format: {}", e.message.lines().next().unwrap_or_default());
                return;
            }
        };
        let Some(i) = self.docs.iter().position(|d| d.view.id == what.view) else { return };
        let d = &mut self.docs[i];
        if d.editor.version() != what.version {
            self.status = "The file changed while it was being formatted; format again".into();
            return;
        }
        let edits = edit::text_edits(&v).and_then(|e| edit::to_char_edits(&d.editor.buffer.snapshot(), &e, what.enc, d.editor.buffer.line_ending().as_str()));
        self.status = match edits {
            Ok(e) if e.is_empty() => "Already formatted".into(),
            Ok(e) => {
                d.editor.apply_edits(e);
                "Formatted".into()
            }
            Err(e) => format!("Couldn't format: {e}"),
        };
    }

    /// Every problem the servers report, in the palette.
    fn lsp_problems(&mut self, ctx: &egui::Context) {
        let all = self.lsp.hub.borrow().all_diagnostics(self.lsp.window);
        let mut rows: Vec<(PathBuf, Diagnostic, Encoding)> = all.into_iter().flat_map(|(p, items, enc)| items.iter().map(move |d| (p.clone(), d.clone(), enc)).collect::<Vec<_>>()).collect();
        rows.retain(|(_, d, _)| d.severity <= 3);
        if rows.is_empty() {
            self.status = if self.settings.lsp_enabled { "No problems reported".into() } else { "Language servers are off (lsp_enabled)".into() };
            return;
        }
        rows.sort_by(|a, b| (a.1.severity, &a.0, a.1.range.0).cmp(&(b.1.severity, &b.0, b.1.range.0)));
        rows.truncate(MAX_LOCATIONS);
        let notes: Vec<String> = rows.iter().map(|(_, d, _)| format!("{}: {}", severity_name(d.severity), d.message.lines().next().unwrap_or_default())).collect();
        let spots: Vec<(PathBuf, (Pos, Pos), Encoding)> = rows.into_iter().map(|(p, d, enc)| (p, d.range, enc)).collect();
        let labels: Vec<String> = spots.iter().zip(&notes).map(|((p, r, _), n)| format!("{n}  ·  {}", self.place(p, *r))).collect();
        let errors = notes.iter().filter(|n| n.starts_with("error")).count();
        let hint = format!("Problems: {errors} {}, {} other", if errors == 1 { "error" } else { "errors" }, notes.len() - errors);
        self.choose(ctx, Choosing::Locations(spots), &hint, labels);
    }

    /// Next / Previous Problem: select the next (previous) problem in this file and
    /// show what it is.
    fn lsp_step_problem(&mut self, forward: bool) {
        let i = self.active;
        let d = &self.docs[i];
        let sel = d.editor.primary().range();
        let marks: Vec<Range<usize>> = d.editor.markers().iter().filter(|m| (1..=3).contains(&m.style)).map(|m| m.range.clone()).collect();
        if marks.is_empty() {
            self.status = "No problems in this file".into();
            return;
        }
        let pick = if forward {
            marks.iter().find(|r| r.start > sel.start).or_else(|| marks.first())
        } else {
            marks.iter().rev().find(|r| r.start < sel.start).or_else(|| marks.last())
        };
        let Some(r) = pick.cloned() else { return };
        let d = &mut self.docs[i];
        d.editor.select_ranges(std::slice::from_ref(&r), 0);
        d.view.reveal_centered();
        let problems = self.problems_at(i, r.start);
        // Shown at the caret next frame, once it has moved there.
        self.lsp.hover.shown = None;
        self.lsp.hover.pending = None;
        self.lsp.pending_problem = Some((self.docs[i].view.id, r.start, problems));
    }

    // ---- popups ---------------------------------------------------------------------------

    /// Hover boxes, parameter hints and the rename box, after the panes.
    pub(super) fn lsp_popups(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        // A problem stepped to last frame: its box at the caret, now that it's there.
        if let Some((view, at, problems)) = self.lsp.pending_problem.take()
            && let Some(i) = self.docs.iter().position(|d| d.view.id == view)
            && let Some(caret) = self.docs[i].view.caret_rect()
        {
            let d = &self.docs[i];
            let at = HoverAt { view, at, word: word_around(&d.editor, at), anchor: caret.left_bottom(), by_mouse: false, key: (d.editor.version(), d.editor.primary().head) };
            self.lsp.hover.shown = Some(HoverBox { at, problems, info: None, rect: None, grow: false });
        }
        self.lsp_trust_notice(ctx);
        self.lsp_mouse_hover(ctx, now);
        self.lsp_draw_hover(ctx);
        self.lsp_draw_signature(ctx);
        self.lsp_rename_ui(ctx);
    }

    fn lsp_mouse_hover(&mut self, ctx: &egui::Context, now: f64) {
        // A box or list that takes the keyboard (rename, the palette, a
        // dialog) closes hover information and keeps new boxes away.
        if self.lsp.rename_box.is_some() || self.palette.open || self.sidebar.modal_open() || self.print.is_open() {
            self.lsp.hover = HoverState::default();
            return;
        }
        let typing = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_) | egui::Event::Key { pressed: true, .. } | egui::Event::Paste(_))));
        let clicked = ctx.input(|i| i.pointer.any_pressed());
        let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
        // A box from the keyboard goes when the text or cursor changes, or on a click.
        if let Some(b) = &self.lsp.hover.shown
            && !b.at.by_mouse
        {
            let still = self.docs.iter().find(|d| d.view.id == b.at.view).is_some_and(|d| (d.editor.version(), d.editor.primary().head) == b.at.key);
            if !still || clicked || ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.lsp.hover.shown = None;
            }
        }
        if typing || clicked {
            if self.lsp.hover.shown.as_ref().is_some_and(|b| b.at.by_mouse) {
                self.lsp.hover.shown = None;
            }
            self.lsp.hover.rest = None;
            return;
        }
        // The spot under the pointer: a char in a visible editor, or a gutter line with problems.
        let mut spot: Option<(usize, egui::Id, usize, Range<usize>, Pos2, bool)> = None;
        for g in 0..self.layout.groups() {
            let Some(i) = self.group_active(g) else { continue };
            let d = &self.docs[i];
            if let Some((c, p)) = d.view.pointer_char {
                spot = Some((i, d.view.id, c, word_around(&d.editor, c), p, false));
            } else if let Some((line, p)) = d.view.pointer_gutter
                && d.editor.markers().iter().any(|m| m.range.start >= d.editor.buffer.line_start(line) && m.range.start <= d.editor.buffer.line_end(line))
            {
                let start = d.editor.buffer.line_start(line);
                spot = Some((i, d.view.id, start, start..start, p, true));
            }
        }
        let over_box = self.lsp.hover.shown.as_ref().and_then(|b| b.rect).zip(pointer_pos).is_some_and(|(r, p)| r.expand(4.0).contains(p));
        if let Some(b) = &self.lsp.hover.shown
            && b.at.by_mouse
            && !over_box
        {
            let same = spot.as_ref().is_some_and(|s| s.1 == b.at.view && (s.3 == b.at.word || b.at.word.contains(&s.2)));
            if !same {
                self.lsp.hover.shown = None;
            }
        }
        let Some((i, view, at, word, p, gutter)) = spot else {
            if !over_box {
                self.lsp.hover.rest = None;
                self.lsp.hover.done = None;
            }
            return;
        };
        let key = (view, if gutter { usize::MAX - at } else { word.start });
        match self.lsp.hover.rest {
            Some((k, _)) if k == key => {}
            _ => {
                self.lsp.hover.rest = Some((key, now));
                self.lsp.hover.done = None;
            }
        }
        let since = self.lsp.hover.rest.map_or(now, |(_, t)| t);
        if self.lsp.hover.done == Some(key) {
            return;
        }
        if now - since < HOVER_DELAY {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(HOVER_DELAY - (now - since) + 0.01));
            return;
        }
        self.lsp.hover.done = Some(key);
        let d = &self.docs[i];
        let at_info = HoverAt { view, at, word, anchor: p + vec2(0.0, d.view.font_size * 0.9), by_mouse: true, key: (d.editor.version(), d.editor.primary().head) };
        let problems = if gutter { self.problems_on_line(i, d.editor.buffer.line_of(at)) } else { self.problems_at(i, at) };
        let on_word = !gutter && {
            let c = self.docs[i].editor.buffer.char(at);
            c.is_alphanumeric() || c == '_'
        };
        if on_word {
            self.lsp_ask_hover(i, &at_info);
        }
        if !problems.is_empty() {
            self.lsp.hover.shown = Some(HoverBox { at: at_info, problems, info: None, rect: None, grow: false });
        }
    }

    fn lsp_draw_hover(&mut self, ctx: &egui::Context) {
        let Some(b) = &mut self.lsp.hover.shown else { return };
        let Some(d) = self.docs.iter().find(|d| d.view.id == b.at.view) else {
            self.lsp.hover.shown = None;
            return;
        };
        let light = d.highlighter.is_light();
        let size = (d.view.font_size - 1.0).max(11.0);
        let screen = ctx.content_rect();
        // Below the spot, or above it near the bottom of the window.
        let below = screen.bottom() - b.at.anchor.y > 220.0;
        let (pivot, at) = if below { (egui::Align2::LEFT_TOP, b.at.anchor + vec2(0.0, 4.0)) } else { (egui::Align2::LEFT_BOTTOM, b.at.anchor - vec2(0.0, size * 1.6 + 6.0)) };
        // The information arriving under problems already shown: measure again, or the box keeps its old height.
        let grow = std::mem::take(&mut b.grow);
        let resp = egui::Area::new(egui::Id::new("lsp-hover")).order(egui::Order::Foreground).pivot(pivot).fixed_pos(at).constrain(true).sizing_pass(grow).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(8.0).show(ui, |ui| {
                ui.set_max_width(560.0f32.min(screen.width() - 32.0));
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for (sev, msg) in &b.problems {
                        ui.horizontal_wrapped(|ui| {
                            let color = crate::view::marker_color(*sev, light).unwrap_or(ui.visuals().weak_text_color());
                            ui.label(egui::RichText::new(severity_name(*sev)).color(color).strong().size(size));
                            ui.label(crate::hidden::widget(msg, egui::FontId::proportional(size), ui.visuals().text_color()));
                        });
                    }
                    if let Some(info) = &b.info {
                        if !b.problems.is_empty() {
                            ui.separator();
                        }
                        show_markdown(ui, info, size);
                    }
                });
            });
        });
        b.rect = Some(resp.response.rect);
    }

    fn lsp_draw_signature(&mut self, ctx: &egui::Context) {
        let Some(s) = &self.lsp.signature_box else { return };
        let i = self.active;
        let d = &self.docs[i];
        let gone = d.view.id != s.view || d.editor.buffer.line_of(d.editor.primary().head) != s.line || ctx.input(|i| i.key_pressed(Key::Escape));
        let Some(caret) = d.view.caret_rect().filter(|_| !gone) else {
            self.lsp.signature_box = None;
            return;
        };
        let size = (d.view.font_size - 1.0).max(11.0);
        let font = egui::FontId::monospace(size);
        let (fg, accent) = (d.highlighter.foreground(), d.highlighter.caret());
        let mut job = egui::text::LayoutJob::default();
        if let Some((k, n)) = s.count {
            job.append(&format!("{k}/{n}  "), 0.0, egui::TextFormat::simple(font.clone(), fg.gamma_multiply(0.6)));
        }
        let mut at = 0;
        if let Some(r) = s.active.clone().filter(|r| r.end <= s.label.len() && s.label.is_char_boundary(r.start) && s.label.is_char_boundary(r.end)) {
            crate::hidden::append(&mut job, &s.label[..r.start], 0.0, egui::TextFormat::simple(font.clone(), fg));
            let mut strong = egui::TextFormat::simple(font.clone(), accent);
            strong.underline = egui::Stroke::new(1.0, accent);
            crate::hidden::append(&mut job, &s.label[r.clone()], 0.0, strong);
            at = r.end;
        }
        crate::hidden::append(&mut job, &s.label[at..], 0.0, egui::TextFormat::simple(font, fg));
        egui::Area::new(egui::Id::new("lsp-signature")).order(egui::Order::Foreground).pivot(egui::Align2::LEFT_BOTTOM).fixed_pos(caret.left_top() - vec2(4.0, 2.0)).constrain(true).interactable(false).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(6.0).show(ui, |ui| {
                ui.set_max_width(640.0);
                ui.label(job);
            });
        });
    }

    fn lsp_rename_ui(&mut self, ctx: &egui::Context) {
        let Some(b) = &mut self.lsp.rename_box else { return };
        let still = self.docs.get(self.active).is_some_and(|d| d.view.id == b.view && d.editor.version() == b.version);
        if !still {
            self.lsp.rename_box = None;
            return;
        }
        let id = egui::Id::new("lsp-rename");
        let mut submit = false;
        let mut cancel = false;
        egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(b.anchor + vec2(-4.0, 4.0)).constrain(true).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(8.0).show(ui, |ui| {
                ui.label(egui::RichText::new(format!("Rename {} to", crate::hidden::plain(&b.old))).weak());
                let resp = ui.add(egui::TextEdit::singleline(&mut b.text).id(id).desired_width(260.0).font(egui::TextStyle::Monospace));
                if b.first {
                    b.first = false;
                    resp.request_focus();
                    // The whole old name selected, ready to be typed over.
                    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
                        let n = b.text.chars().count();
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))));
                        state.store(ctx, id);
                    }
                }
                let (enter, esc) = ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                submit = enter && (resp.lost_focus() || resp.has_focus());
                cancel = esc || (resp.lost_focus() && !enter);
                ui.label(egui::RichText::new("Enter to rename, Esc to cancel").weak().small());
            });
        });
        if submit || cancel {
            let b = self.lsp.rename_box.take().expect("open");
            if let Some(i) = self.docs.iter().position(|d| d.view.id == b.view) {
                self.docs[i].view.focus();
            }
            if submit {
                self.lsp_rename_submit(b);
            }
        }
    }

    // ---- status bar ---------------------------------------------------------------------

    /// The current file's server for the status bar: text, color, tooltip,
    /// and what a click does.
    pub(super) fn lsp_status(&self) -> Option<(String, Option<Color32>, String, StatusClick)> {
        let d = &self.docs[self.active];
        let (name, state) = self.lsp.hub.borrow().state_for(d.path.as_ref()?)?;
        let amber = Color32::from_rgb(0xe5, 0x9e, 0x40);
        Some(match state {
            State::Starting => (format!("{name}: starting…"), None, format!("{name} is starting"), StatusClick::Problems),
            State::Busy(work) => (format!("{name}: {work}"), None, format!("{name} is working: {work}"), StatusClick::Problems),
            State::Ready => {
                let counts = |sev: u8| d.editor.markers().iter().filter(|m| m.style == sev).count();
                let (e, w) = (counts(1), counts(2));
                let mut parts = Vec::new();
                if e > 0 {
                    parts.push(format!("{e} {}", if e == 1 { "error" } else { "errors" }));
                }
                if w > 0 {
                    parts.push(format!("{w} {}", if w == 1 { "warning" } else { "warnings" }));
                }
                let text = if parts.is_empty() { name.clone() } else { format!("{name}: {}", parts.join(", ")) };
                (text, None, format!("{name} is running. Click to list the problems."), StatusClick::Problems)
            }
            State::Restarting { why, secs } => {
                let when = if secs > 0 { format!("restarting in {secs} s") } else { "restarting".into() };
                (format!("{name}: {when}"), Some(amber), format!("{name} stopped ({why})"), StatusClick::Problems)
            }
            State::Failed(why) => (format!("{name}: stopped"), Some(amber), format!("{name} stopped ({why}). Click to try again."), StatusClick::Restart),
            State::NotTrusted => (format!("{name}: not started"), None, "Language servers can run this project's code, so Strata asks first. Click to choose.".into(), StatusClick::Ask),
        })
    }

    /// The status bar item was clicked.
    pub(super) fn lsp_status_clicked(&mut self, ctx: &egui::Context, click: StatusClick) {
        match click {
            StatusClick::Restart => self.lsp_command(ctx, Cmd::LspRestart),
            StatusClick::Problems => self.lsp_problems(ctx),
            StatusClick::Ask => {
                if let Some(path) = self.docs[self.active].path.clone() {
                    self.lsp.hub.borrow_mut().ask_again(&path);
                }
            }
        }
    }

    /// The answer to the trust notice for `root`.
    pub(super) fn lsp_answer_trust(&mut self, root: &Path, answer: Trust) {
        self.lsp.hub.borrow_mut().answer_trust(root, answer);
        let folder = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
        self.status = match answer {
            Trust::Once => format!("Starting language servers for {folder}"),
            Trust::Always => format!("Starting language servers for {folder}; Strata won't ask again for it"),
            Trust::NotNow => format!("Language servers not started for {folder} (click the status bar to change your mind)"),
        };
    }

    /// "Start language servers for this folder?", in the corner: servers
    /// can run a project's own code, so nothing starts until the user says.
    fn lsp_trust_notice(&mut self, ctx: &egui::Context) {
        let Some((root, names)) = self.lsp.hub.borrow().trust_question(self.lsp.window) else { return };
        let folder = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
        let who = match names.as_slice() {
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
            [] => "Language servers".into(),
        };
        let mut answer = None;
        egui::Window::new("Start language servers?")
            .id(self.link.id("lsp-trust"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -40.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .show(ctx, |ui| {
                let size = egui::TextStyle::Body.resolve(ui.style()).size;
                ui.set_max_width((size * 30.0).max(400.0));
                ui.strong(format!("Start language servers for “{folder}”?"));
                ui.label(format!("{who} can run code from this project, such as build scripts and the tools it configures. Start {} only for projects you trust.", if names.len() == 1 { "it" } else { "them" }));
                ui.label(egui::RichText::new(root.display().to_string()).weak());
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    for (label, a) in [("Start", Trust::Once), ("Always for This Folder", Trust::Always), ("Not Now", Trust::NotNow)] {
                        if ui.button(label).clicked() {
                            answer = Some(a);
                        }
                    }
                });
            });
        if let Some(a) = answer {
            self.lsp_answer_trust(&root, a);
        }
    }
}

/// A file's name, for messages.
fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// "x changed while the server worked; try again".
fn changed(p: &Path) -> String {
    format!("{} changed while the server worked; try again", file_name(p))
}

/// A line of code as a list label: trimmed, at most 100 chars.
fn label_text(line: &str) -> String {
    line.trim().chars().take(100).collect()
}

/// A diagnostic's message with its source and code.
fn describe(d: &Diagnostic) -> String {
    match (&d.source, &d.code) {
        (Some(s), Some(c)) => format!("{} ({s} {c})", d.message),
        (Some(s), None) => format!("{} ({s})", d.message),
        (None, Some(c)) => format!("{} ({c})", d.message),
        (None, None) => d.message.clone(),
    }
}

/// Draw hover Markdown: paragraphs with inline code and emphasis, code
/// blocks in the editor font, lists, rules.
fn show_markdown(ui: &mut egui::Ui, src: &str, size: f32) {
    for b in crate::markdown::parse(src) {
        block(ui, &b.block, size);
    }
}

fn block(ui: &mut egui::Ui, b: &crate::markdown::Block, size: f32) {
    use crate::markdown::Block;
    match b {
        Block::Paragraph(inl) => {
            ui.label(inline_job(ui, inl, size, false));
        }
        Block::Heading { text, .. } => {
            ui.label(inline_job(ui, text, size, true));
        }
        Block::Code { code, .. } => {
            let fill = ui.visuals().faint_bg_color;
            egui::Frame::new().fill(fill).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
                ui.label(crate::hidden::widget(code.trim_end(), egui::FontId::monospace(size), ui.visuals().text_color()));
            });
        }
        Block::Quote { blocks, .. } | Block::Footnote { blocks, .. } => {
            ui.indent("quote", |ui| blocks.iter().for_each(|b| block(ui, b, size)));
        }
        Block::List { start, items } => {
            for (k, item) in items.iter().enumerate() {
                let mark = start.map_or_else(|| "•".to_owned(), |s| format!("{}.", s + k as u64));
                ui.horizontal_top(|ui| {
                    ui.label(egui::RichText::new(mark).size(size));
                    ui.vertical(|ui| item.blocks.iter().for_each(|b| block(ui, b, size)));
                });
            }
        }
        Block::Table { head, rows, .. } => {
            for row in std::iter::once(head).chain(rows) {
                let text = row.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("  |  ");
                ui.label(crate::hidden::widget(&text, egui::FontId::proportional(size), ui.visuals().text_color()));
            }
        }
        Block::Rule => {
            ui.separator();
        }
        // (Embeds come only from notes, never from a hover.)
        Block::Html(s) | Block::Embed { target: s, .. } => {
            ui.label(crate::hidden::widget(s, egui::FontId::proportional(size), ui.visuals().weak_text_color()));
        }
    }
}

fn inline_job(ui: &egui::Ui, inl: &crate::markdown::Inline, size: f32, heading: bool) -> egui::text::LayoutJob {
    let v = ui.visuals();
    let (text, strong, code_bg) = (v.text_color(), v.strong_text_color(), v.faint_bg_color);
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    for run in &inl.runs {
        let Some(s) = inl.text.get(run.range.clone()) else { continue };
        let font = if run.style.code { egui::FontId::monospace(size) } else { egui::FontId::proportional(size) };
        let mut f = egui::TextFormat::simple(font, if run.style.bold || heading { strong } else { text });
        f.italics = run.style.italic;
        if run.style.code {
            f.background = code_bg;
        }
        if run.link.is_some() {
            f.underline = egui::Stroke::new(1.0, text.gamma_multiply(0.5));
        }
        crate::hidden::append(&mut job, s, 0.0, f);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::super::ui_tests::{frame, press, temp_dir};
    use super::*;
    use crate::lsp::fake;
    use egui::{Context, Event, Modifiers};
    use std::time::{Duration, Instant};

    #[test]
    fn hover_markdown_marks_hidden_characters_everywhere() {
        let r = "\u{202E}";
        let src = format!("Para{r} `co{r}de`\n\n```rust\nlet x{r} = 1;\n```\n\n| A{r} | B |\n|--|--|\n| c | d |\n\n<b>ht{r}ml</b>\n\n- it{r}em\n");
        let ctx = Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show_markdown(ui, &src, 13.0));
        out.textures_delta.clear();
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut texts = Vec::new();
        out.shapes.iter().for_each(|c| walk(&c.shape, &mut texts));
        assert!(texts.iter().all(|t| !crate::hidden::any(t)), "raw control: {texts:?}");
        for want in ["Para", "co", "let x", "A", "ht", "it"] {
            assert!(texts.iter().any(|t| t.contains(want) && t.contains("\u{2009}RLO\u{2009}")), "{want}: {texts:?}");
        }
    }

    /// A window with the fake server for `*.fake` files, the folder open and
    /// the first file in a tab, once the server has answered for it.
    fn app_with(tag: &str, files: &[(&str, &str)], options: Value) -> (StrataApp, Context, PathBuf) {
        let dir = temp_dir(&format!("lsp-{tag}"));
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        let ctx = Context::default();
        let mut app = StrataApp::new(None, None);
        app.lsp.hub.borrow_mut().use_servers(vec![fake::config("fake", &["*.fake"], options)]);
        // These tests are about the features; the trust notice has its own.
        app.settings.lsp_trust_all = true;
        app.open_folder(dir.clone());
        app.open_path(dir.join(files[0].0), false);
        wait(&mut app, &ctx, "the server to start", |a| matches!(a.lsp_status(), Some((t, ..)) if !t.contains("starting")));
        (app, ctx, dir)
    }

    /// Run frames until `done`, for up to 20 seconds.
    fn wait(app: &mut StrataApp, ctx: &Context, what: &str, done: impl Fn(&StrataApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            frame(app, ctx, vec![]);
            if done(app) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what} (status: {:?}, {})", app.lsp_status(), app.status);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn text(app: &StrataApp) -> String {
        let b = &app.docs[app.active].editor.buffer;
        b.slice(0..b.len_chars())
    }

    fn markers(app: &StrataApp) -> Vec<(Range<usize>, u8)> {
        app.docs[app.active].editor.markers().iter().map(|m| (m.range.clone(), m.style)).collect()
    }

    /// Tab `d` shows the file at `path`, whichever path it was opened by
    /// (files a rename opens go by their real path).
    fn is(d: &crate::document::Document, path: &Path) -> bool {
        let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
        d.path.as_deref().is_some_and(|p| real(p) == real(path))
    }

    fn put_cursor(app: &mut StrataApp, at: usize) {
        app.doc().editor.select_ranges(&[at..at], 0);
    }

    #[test]
    fn diagnostics_arrive_render_and_follow_edits() {
        let (mut app, ctx, _) = app_with("diag", &[("a.fake", "let bad = 1;\nlet meh = 2;\n")], json!({}));
        wait(&mut app, &ctx, "diagnostics", |a| markers(a).len() == 2);
        assert_eq!(markers(&app), [(4..7, 1), (17..20, 2)]);
        // Typing before them moves them at once, before the server says anything.
        put_cursor(&mut app, 0);
        frame(&mut app, &ctx, vec![Event::Text("xx ".into())]);
        assert_eq!(text(&app), "xx let bad = 1;\nlet meh = 2;\n");
        assert_eq!(markers(&app)[0].0, 7..10);
        // The server's next report agrees.
        wait(&mut app, &ctx, "new diagnostics", |a| a.lsp.shown.values().next().is_some_and(|s| s.items.len() == 2) && a.docs[0].editor.markers()[0].range == (7..10));
        let status = app.lsp_status().unwrap().0;
        assert!(status.contains("1 error, 1 warning"), "{status}");
        // Drawn: a wavy line in the error color, and a dot in the gutter.
        let out = frame(&mut app, &ctx, vec![]);
        let light = app.docs[0].highlighter.is_light();
        let red = crate::view::marker_color(1, light).unwrap();
        let squiggles = out.shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::Path(p) if p.stroke.color == egui::epaint::ColorMode::Solid(red))).count();
        let dots = out.shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::Circle(c) if c.fill == red)).count();
        assert!(squiggles >= 1 && dots >= 1, "squiggles {squiggles}, dots {dots}");
        // Fixing the problem clears it.
        let r = app.docs[0].editor.markers()[0].range.clone();
        app.doc().editor.select_ranges(std::slice::from_ref(&r), 0);
        frame(&mut app, &ctx, vec![Event::Text("good".into())]);
        wait(&mut app, &ctx, "the error to go", |a| markers(a).iter().all(|(_, s)| *s != 1));
        // Next Problem goes to the warning and says what it is.
        put_cursor(&mut app, 0);
        app.run(&ctx, Cmd::LspNextProblem);
        frame(&mut app, &ctx, vec![]);
        let warning = markers(&app)[0].0.clone();
        assert_eq!(app.docs[0].editor.primary().range(), warning);
        assert!(app.lsp.hover.shown.as_ref().is_some_and(|b| b.problems.iter().any(|(s, m)| *s == 2 && m.contains("could be better"))));
    }

    #[test]
    fn crlf_and_emoji_text_stays_in_step_with_the_server() {
        let (mut app, ctx, _) = app_with("crlf", &[("a.fake", "a👋 ok\r\nbad\r\n")], json!({}));
        wait(&mut app, &ctx, "diagnostics", |a| !markers(a).is_empty());
        let start = app.docs[0].editor.buffer.line_start(1);
        assert_eq!(markers(&app), [(start..start + 3, 1)]);
        // Edits on both lines (with line breaks typed as the file's own).
        put_cursor(&mut app, 2);
        frame(&mut app, &ctx, vec![Event::Text("日本".into())]);
        let end = app.docs[0].editor.buffer.line_end(1);
        put_cursor(&mut app, end);
        frame(&mut app, &ctx, vec![press(egui::Key::Enter, Modifiers::NONE)]);
        frame(&mut app, &ctx, vec![Event::Text("meh bad".into())]);
        assert_eq!(text(&app), "a👋日本 ok\r\nbad\r\nmeh bad\r\n");
        // The server's copy matches: its diagnostics land on the right words.
        wait(&mut app, &ctx, "diagnostics for the new text", |a| markers(a).len() == 3);
        let found: Vec<String> = app.docs[0].editor.markers().iter().map(|m| app.docs[0].editor.buffer.slice(m.range.clone())).collect();
        assert_eq!(found, ["bad", "meh", "bad"]);
    }

    #[test]
    fn completions_from_the_server_come_first_and_bring_their_imports() {
        let (mut app, ctx, _) = app_with("complete", &[("a.fake", "start\n")], json!({}));
        app.settings.auto_complete = true;
        put_cursor(&mut app, 6);
        frame(&mut app, &ctx, vec![Event::Text("fa".into())]);
        wait(&mut app, &ctx, "server completions", |a| a.completion.labels().first().map(String::as_str) == Some("fake_alpha"));
        assert_eq!(&app.completion.labels()[..3], ["fake_alpha", "fake_beta", "fake_snippet"]);
        // Its details are fetched while it's highlighted; Enter inserts it with its import.
        wait(&mut app, &ctx, "the item's details", |a| a.completion.selected_unresolved().is_none() && a.lsp.resolve.is_none());
        frame(&mut app, &ctx, vec![press(egui::Key::Enter, Modifiers::NONE)]);
        assert_eq!(text(&app), "use fake;\nstart\nfake_alpha");
        assert_eq!(app.docs[0].editor.primary().head, text(&app).chars().count());
        app.doc().editor.undo();
        assert_eq!(text(&app), "start\nfa", "one undo step");

        // A snippet: placeholders become fields.
        put_cursor(&mut app, 8);
        frame(&mut app, &ctx, vec![Event::Text("ke_s".into())]);
        wait(&mut app, &ctx, "the snippet item", |a| a.completion.labels().first().map(String::as_str) == Some("fake_snippet"));
        frame(&mut app, &ctx, vec![press(egui::Key::Tab, Modifiers::NONE)]);
        assert_eq!(text(&app), "start\nfake_snippet(arg)");
        assert_eq!(app.docs[0].editor.primary().range(), 19..22, "the placeholder is selected");

        // After a trigger character the list opens with nothing typed.
        app.doc().editor.select_ranges(&[23..23], 0);
        frame(&mut app, &ctx, vec![Event::Text(".".into())]);
        wait(&mut app, &ctx, "completions after a dot", |a| a.completion.is_open() && a.completion.labels().contains(&"fake_beta".to_owned()));
        frame(&mut app, &ctx, vec![press(egui::Key::Escape, Modifiers::NONE)]);
        assert!(!app.completion.is_open());
    }

    #[test]
    fn a_snippet_completion_expands_at_every_cursor() {
        let (mut app, ctx, _) = app_with("snippet-multi", &[("a.fake", "pri\npri\npri\n")], json!({}));
        app.settings.auto_complete = true;
        app.doc().editor.select_ranges(&[3..3, 7..7, 11..11], 0);
        frame(&mut app, &ctx, vec![Event::Text("n".into())]);
        wait(&mut app, &ctx, "the println! item", |a| a.completion.labels().contains(&"println!".to_owned()));
        let k = app.completion.labels().iter().position(|l| l == "println!").unwrap();
        app.completion.choose(&mut app.docs[0], k);
        app.lsp_insert_completion();
        frame(&mut app, &ctx, vec![]);
        let line = "println!(\"text\", value);";
        assert_eq!(text(&app), format!("{line}\n{line}\n{line}\n"));
        // The first field (`text`) is selected at each cursor.
        let sels: Vec<String> = app.docs[0].editor.selections().iter().map(|s| app.docs[0].editor.buffer.slice(s.range())).collect();
        assert_eq!(sels, ["text", "text", "text"]);
    }

    #[test]
    fn each_cursor_replaces_its_own_partial_word() {
        // Cursors after `pr`, `pri` and `pri`: each word is replaced whole.
        let (mut app, ctx, _) = app_with("snippet-prefixes", &[("a.fake", "pr\npri\npri\n")], json!({}));
        app.settings.auto_complete = true;
        app.doc().editor.select_ranges(&[2..2, 6..6, 10..10], 0);
        frame(&mut app, &ctx, vec![Event::Text("i".into())]);
        // The primary cursor now has `pri`, the others `prii`.
        wait(&mut app, &ctx, "the println! item", |a| a.completion.labels().contains(&"println!".to_owned()));
        let k = app.completion.labels().iter().position(|l| l == "println!").unwrap();
        app.completion.choose(&mut app.docs[0], k);
        app.lsp_insert_completion();
        frame(&mut app, &ctx, vec![]);
        let line = "println!(\"text\", value);";
        assert_eq!(text(&app), format!("{line}\n{line}\n{line}\n"));
        // A plain item too.
        app.doc().editor.undo();
        app.doc().editor.undo();
        assert_eq!(text(&app), "pri\nprii\nprii\n");
        app.doc().editor.select_ranges(&[3..3, 8..8, 13..13], 0);
        app.lsp_insert_server_item(crate::app::completion::ServerItem::from_json(&json!({"label": "printer"})).unwrap());
        assert_eq!(text(&app), "printer\nprinter\nprinter\n");
    }

    #[test]
    fn hover_from_the_keyboard_and_the_mouse() {
        let (mut app, ctx, _) = app_with("hover", &[("a.fake", "let bad = thing;\n")], json!({}));
        wait(&mut app, &ctx, "diagnostics", |a| !markers(a).is_empty());
        put_cursor(&mut app, 12);
        frame(&mut app, &ctx, vec![]);
        app.run(&ctx, Cmd::LspHover);
        wait(&mut app, &ctx, "hover info", |a| a.lsp.hover.shown.as_ref().and_then(|b| b.info.as_ref()).is_some_and(|i| i.contains("Hover for `thing`")));
        frame(&mut app, &ctx, vec![]);
        assert!(ctx.memory(|m| m.area_rect(egui::Id::new("lsp-hover"))).is_some(), "drawn");
        // Typing closes it.
        frame(&mut app, &ctx, vec![press(egui::Key::ArrowRight, Modifiers::NONE)]);
        frame(&mut app, &ctx, vec![]);
        assert!(app.lsp.hover.shown.is_none());

        // The pointer resting on the misspelled word: its problem and the server's info.
        put_cursor(&mut app, 5);
        frame(&mut app, &ctx, vec![]);
        let caret = app.docs[0].view.caret_rect().unwrap();
        let over = caret.center() + vec2(2.0, 0.0);
        for _ in 0..4 {
            frame(&mut app, &ctx, vec![Event::PointerMoved(over)]);
        }
        wait(&mut app, &ctx, "hover box", |a| a.lsp.hover.shown.as_ref().is_some_and(|b| !b.problems.is_empty() && b.info.is_some()));
        let b = app.lsp.hover.shown.as_ref().unwrap();
        assert!(b.problems[0].1.contains("is not allowed") && b.info.as_ref().unwrap().contains("`bad`"));
        // Moving away closes it.
        frame(&mut app, &ctx, vec![Event::PointerMoved(Pos2::new(5.0, 590.0))]);
        frame(&mut app, &ctx, vec![]);
        assert!(app.lsp.hover.shown.is_none());
    }

    #[test]
    fn definition_and_references_go_through_the_server() {
        let (mut app, ctx, dir) = app_with("goto", &[("a.fake", "fn target() {}\ncall(target);\ntarget();\n")], json!({}));
        put_cursor(&mut app, 21);
        app.run(&ctx, Cmd::GotoDefinition);
        wait(&mut app, &ctx, "the jump", |a| a.docs[a.active].editor.primary().range() == (3..9));
        // References: several, listed in the palette.
        app.run(&ctx, Cmd::GotoReference);
        wait(&mut app, &ctx, "the list", |a| a.palette.open);
        assert!(matches!(&app.choosing, Choosing::Locations(spots) if spots.len() == 3));
        // A word the server doesn't know: Strata's own search takes over.
        std::fs::write(dir.join("b.fake"), "nothing\n").unwrap();
        app.palette.open = false;
        put_cursor(&mut app, 0);
        app.doc().editor.insert_text(" ");
        put_cursor(&mut app, 0);
        app.run(&ctx, Cmd::GotoDefinition);
        wait(&mut app, &ctx, "the fallback", |a| a.lsp.goto.is_none());
    }

    #[test]
    fn a_request_right_after_an_edit_sees_the_edit() {
        let (mut app, ctx, _) = app_with("flush", &[("a.fake", "fn target\n")], json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        // An edit and a request in the same frame, before the frame's sync.
        let end = app.docs[0].editor.buffer.len_chars();
        put_cursor(&mut app, end);
        app.doc().editor.insert_text("call(target)\n");
        put_cursor(&mut app, end + 6);
        assert!(app.lsp_goto(true));
        wait(&mut app, &ctx, "the jump", |a| a.lsp.goto.is_none());
        assert_eq!(app.docs[0].editor.primary().range(), 3..9, "the server knew the new line");
    }

    #[test]
    fn reference_lines_of_unopened_files_are_read_off_the_ui_thread() {
        let (mut app, ctx, _) = app_with("refs-bg", &[("a.fake", "fn target() {}\n"), ("b.fake", "// other\n    target();\n")], json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        put_cursor(&mut app, 4);
        app.run(&ctx, Cmd::GotoReference);
        wait(&mut app, &ctx, "the list", |a| a.palette.open);
        // The open file's line is there at once; the other file's follows.
        let first = app.palette.choice_labels().to_vec();
        assert!(first.iter().any(|l| l.contains("a.fake:1:4  fn target() {}")), "{first:?}");
        wait(&mut app, &ctx, "the other file's line", |a| a.palette.choice_labels().iter().any(|l| l.contains("b.fake:2:5  target();")));
        assert!(app.lsp.labels.is_none());
    }

    #[test]
    fn rename_changes_open_and_closed_files_with_undo() {
        let (mut app, ctx, dir) = app_with("rename", &[("a.fake", "old = 1;\nprint(old);\n"), ("b.fake", "use old;\n"), ("c.txt", "old\n")], json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp.hub.borrow().server_for(a.lsp.window, &dir.join("a.fake")).is_some());
        put_cursor(&mut app, 1);
        frame(&mut app, &ctx, vec![]);
        app.run(&ctx, Cmd::LspRename);
        assert!(app.lsp.renaming());
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.lsp.rename_box.as_ref().unwrap().text, "old");
        // Typed over (the old name starts selected), then Enter.
        frame(&mut app, &ctx, vec![Event::Text("new".into())]);
        assert_eq!(app.lsp.rename_box.as_ref().unwrap().text, "new");
        frame(&mut app, &ctx, vec![press(egui::Key::Enter, Modifiers::NONE)]);
        assert!(!app.lsp.renaming());
        wait(&mut app, &ctx, "the rename", |a| a.docs.iter().any(|d| is(d, &dir.join("b.fake"))));
        let a = app.docs.iter().position(|d| is(d, &dir.join("a.fake"))).unwrap();
        let b = app.docs.iter().position(|d| is(d, &dir.join("b.fake"))).unwrap();
        let all = |app: &StrataApp, i: usize| app.docs[i].editor.buffer.slice(0..app.docs[i].editor.buffer.len_chars());
        assert_eq!(all(&app, a), "new = 1;\nprint(new);\n");
        assert_eq!(all(&app, b), "use new;\n");
        assert_eq!(app.active, a, "the tab you renamed from stays in front");
        assert!(app.docs[b].is_dirty() && std::fs::read_to_string(dir.join("b.fake")).unwrap() == "use old;\n", "opened, unsaved");
        assert!(app.status.contains("3 places in 2 files"), "{}", app.status);
        assert_eq!(std::fs::read_to_string(dir.join("c.txt")).unwrap(), "old\n");
        app.docs[a].editor.undo();
        assert_eq!(all(&app, a), "old = 1;\nprint(old);\n", "one undo step");
    }

    /// Start a rename of the word at `at` in the active tab to `name`, as Enter in the box would.
    fn rename(app: &mut StrataApp, ctx: &Context, at: usize, name: &str) {
        put_cursor(app, at);
        frame(app, ctx, vec![]);
        app.run(ctx, Cmd::LspRename);
        let mut b = app.lsp.rename_box.take().expect("the rename box");
        b.text = name.into();
        app.lsp_rename_submit(b);
        assert!(app.lsp.rename.is_some());
    }

    #[test]
    fn a_rename_answer_for_text_that_changed_since_is_dropped() {
        let (mut app, ctx, _) = app_with("rename-stale", &[("a.fake", "old = 1;\nprint(old);\n")], json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        rename(&mut app, &ctx, 1, "new");
        // Typing before the answer is applied: its positions are stale now.
        put_cursor(&mut app, 0);
        app.doc().editor.insert_text("// typed\n");
        wait(&mut app, &ctx, "the answer", |a| a.lsp.rename.is_none());
        assert_eq!(text(&app), "// typed\nold = 1;\nprint(old);\n", "nothing applied");
        assert!(app.status.contains("changed"), "{}", app.status);
    }

    #[test]
    fn a_rename_that_cant_reach_every_file_changes_none() {
        // b.fake isn't text: it would open in the hex view, which takes no edits.
        let (mut app, ctx, dir) = app_with("rename-partial", &[("a.fake", "old = 1;\n"), ("b.fake", "\0\0 old \0\0")], json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        rename(&mut app, &ctx, 1, "new");
        wait(&mut app, &ctx, "the answer", |a| a.lsp.rename.is_none() && a.lsp.applying.is_none());
        assert_eq!(text(&app), "old = 1;\n", "all or nothing");
        assert!(app.status.contains("b.fake"), "says which file: {}", app.status);
        assert!(!app.docs.iter().any(|d| is(d, &dir.join("b.fake"))), "nothing opened");
    }

    #[test]
    fn renaming_into_unopened_files_uses_the_text_their_tabs_will_hold() {
        // A BOM (not part of the text), and a Latin-1 file (not UTF-8 at all).
        let (mut app, ctx, dir) = app_with("rename-bom", &[("a.fake", "use foo;\n"), ("b.fake", "\u{feff}fn foo() {}\n")], json!({}));
        std::fs::write(dir.join("c.fake"), b"// caf\xE9\nfn foo() {}\n").unwrap();
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        rename(&mut app, &ctx, 5, "bar");
        wait(&mut app, &ctx, "the rename", |a| a.lsp.rename.is_none() && a.lsp.applying.is_none());
        assert_eq!(text(&app), "use bar;\n", "{}", app.status);
        let line = |app: &StrataApp, name: &str, n: usize| app.docs.iter().find(|d| is(d, &dir.join(name))).map(|d| d.editor.buffer.line_text(n));
        assert_eq!(line(&app, "b.fake", 0).as_deref(), Some("fn bar() {}"));
        assert_eq!(line(&app, "c.fake", 1).as_deref(), Some("fn bar() {}"));
        assert_eq!(line(&app, "c.fake", 0).as_deref(), Some("// café"));
    }

    #[test]
    fn unopened_rename_targets_are_read_on_a_worker() {
        let files: Vec<(String, String)> = (0..6).map(|k| (format!("f{k}.fake"), format!("// file {k}\nuse old;\n"))).collect();
        let mut all: Vec<(&str, &str)> = vec![("a.fake", "old = 1;\n")];
        all.extend(files.iter().map(|(n, t)| (n.as_str(), t.as_str())));
        let (mut app, ctx, dir) = app_with("rename-worker", &all, json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        rename(&mut app, &ctx, 1, "new");
        // The frame the answer lands: the six files are being read, none opened yet.
        wait(&mut app, &ctx, "the answer", |a| a.lsp.rename.is_none());
        assert!(app.status.starts_with("Renaming… (7 files)"), "{}", app.status);
        assert_eq!(app.docs.len(), 1, "nothing opened on the UI thread");
        assert_eq!(text(&app), "old = 1;\n", "and nothing changed yet");
        wait(&mut app, &ctx, "the rename", |a| a.lsp.applying.is_none());
        assert_eq!(app.docs.len(), 7, "{}", app.status);
        assert_eq!(text(&app), "new = 1;\n");
        let f3 = app.docs.iter().find(|d| is(d, &dir.join("f3.fake"))).unwrap();
        assert_eq!(f3.editor.buffer.line_text(1), "use new;");
        assert!(app.status.contains("7 files"), "{}", app.status);
    }

    #[test]
    fn a_rename_touching_many_files_maps_them_off_the_ui_thread() {
        let n = crate::lsp::hub::MAX_INEXACT + 10;
        let files: Vec<(String, String)> = (0..n).map(|k| (format!("f{k:03}.fake"), "use old;\n".to_owned())).collect();
        let mut all: Vec<(&str, &str)> = vec![("a.fake", "old = 1;\n")];
        all.extend(files.iter().map(|(a, b)| (a.as_str(), b.as_str())));
        let (mut app, ctx, dir) = app_with("rename-many", &all, json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        rename(&mut app, &ctx, 1, "new");
        wait(&mut app, &ctx, "the answer", |a| a.lsp.rename.is_none());
        assert!(app.lsp.mapping.is_some(), "the paths go to a worker first: {}", app.status);
        assert_eq!(app.docs.len(), 1);
        wait(&mut app, &ctx, "the rename", |a| a.lsp.mapping.is_none() && a.lsp.applying.is_none());
        assert_eq!(text(&app), "new = 1;\n", "{}", app.status);
        assert_eq!(app.docs.len(), n + 1);
        let last = app.docs.iter().find(|d| is(d, &dir.join(format!("f{:03}.fake", n - 1)))).unwrap();
        assert_eq!(last.editor.buffer.line_text(0), "use new;");
    }

    #[test]
    fn a_file_changed_on_disk_while_it_was_read_cancels_the_rename() {
        let (mut app, ctx, dir) = app_with("rename-disk", &[("a.fake", "old = 1;\n"), ("b.fake", "use old;\n")], json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        rename(&mut app, &ctx, 1, "new");
        wait(&mut app, &ctx, "the answer", |a| a.lsp.rename.is_none());
        // Another program writes b.fake before the edits are applied.
        std::fs::write(dir.join("b.fake"), "// rewritten elsewhere\nuse old;\n").unwrap();
        wait(&mut app, &ctx, "the end of the rename", |a| a.lsp.applying.is_none());
        assert_eq!(text(&app), "old = 1;\n", "all or nothing: {}", app.status);
        assert!(app.status.contains("b.fake"), "{}", app.status);
    }

    #[test]
    fn a_same_size_rewrite_with_the_old_time_still_cancels_the_rename() {
        let (mut app, ctx, dir) = app_with("rename-sneaky", &[("a.fake", "old = 1;\n"), ("b.fake", "use old;\n")], json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        let b = dir.join("b.fake");
        let mtime = std::fs::metadata(&b).unwrap().modified().unwrap();
        rename(&mut app, &ctx, 1, "new");
        wait(&mut app, &ctx, "the answer", |a| a.lsp.rename.is_none());
        // Same length, same modification time: only the content (and the
        // system's change time) tell.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&b, "// old ;\n").unwrap();
        std::fs::File::options().write(true).open(&b).unwrap().set_modified(mtime).unwrap();
        wait(&mut app, &ctx, "the end of the rename", |a| a.lsp.applying.is_none());
        assert_eq!(text(&app), "old = 1;\n", "nothing applied: {}", app.status);
        assert!(app.status.contains("b.fake"), "{}", app.status);
        assert!(!app.docs.iter().any(|d| is(d, &b)));
    }

    #[test]
    fn a_tab_opened_by_a_rename_notices_later_changes_on_disk() {
        let (mut app, ctx, dir) = app_with("rename-later", &[("a.fake", "old = 1;\n"), ("b.fake", "use old;\n")], json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        rename(&mut app, &ctx, 1, "new");
        wait(&mut app, &ctx, "the rename", |a| a.lsp.rename.is_none() && a.lsp.applying.is_none());
        let b = dir.join("b.fake");
        let i = app.docs.iter().position(|d| is(d, &b)).expect("opened");
        assert!(app.docs[i].is_dirty() && !app.docs[i].check_disk(), "unsaved, and nothing changed yet");
        std::fs::write(&b, "// someone else's edit\nuse old;\n").unwrap();
        assert!(!app.docs[i].check_disk(), "the unsaved rename isn't thrown away");
        assert!(app.docs[i].conflict, "but the tab says the file changed on disk");
    }

    #[cfg(unix)]
    #[test]
    fn swapping_the_folder_link_before_the_answer_doesnt_redirect_the_rename() {
        let base = temp_dir("lsp-alias-swap");
        let (r1, r2) = (base.join("r1"), base.join("r2"));
        std::fs::create_dir_all(&r1).unwrap();
        std::fs::create_dir_all(&r2).unwrap();
        std::fs::write(r1.join("a.fake"), "old = 1;\n").unwrap();
        std::fs::write(r1.join("b.fake"), "use old;\n").unwrap();
        std::fs::write(r2.join("b.fake"), "use old; // elsewhere\n").unwrap();
        let alias = base.join("alias");
        std::os::unix::fs::symlink(&r1, &alias).unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(None, None);
        app.lsp.hub.borrow_mut().use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        app.settings.lsp_trust_all = true;
        app.open_folder(alias.clone());
        app.open_path(alias.join("a.fake"), false);
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        rename(&mut app, &ctx, 1, "new");
        // The link now points elsewhere before the answer is applied.
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&r2, &alias).unwrap();
        wait(&mut app, &ctx, "the rename", |a| a.lsp.rename.is_none() && a.lsp.applying.is_none());
        assert_eq!(std::fs::read_to_string(r2.join("b.fake")).unwrap(), "use old; // elsewhere\n");
        let b = app.docs.iter().find(|d| d.path.as_ref().is_some_and(|p| p.ends_with("b.fake"))).expect("b opened");
        assert_eq!(b.path.as_deref(), Some(std::fs::canonicalize(r1.join("b.fake")).unwrap().as_path()), "opened by its real path");
        assert_eq!(b.editor.buffer.line_text(0), "use new;");
    }

    #[cfg(unix)]
    #[test]
    fn an_answer_for_a_tab_that_closed_never_lands_on_the_tab_taking_over() {
        let base = temp_dir("lsp-handoff");
        let real = base.join("d");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("x.fake"), "old = 1;\n").unwrap();
        std::os::unix::fs::symlink(&real, base.join("a1")).unwrap();
        std::os::unix::fs::symlink(&real, base.join("a2")).unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(None, None);
        app.lsp.hub.borrow_mut().use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        app.settings.lsp_trust_all = true;
        app.open_path(base.join("a2/x.fake"), false);
        app.open_path(base.join("a1/x.fake"), false);
        // The two tabs differ: the second one has unsaved lines above.
        let i2 = app.docs.iter().position(|d| d.path.as_deref() == Some(base.join("a2/x.fake").as_path())).unwrap();
        // a2 opened first, so it speaks for the file; make it the one renaming.
        app.activate(i2);
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        let i1 = app.docs.iter().position(|d| d.path.as_deref() == Some(base.join("a1/x.fake").as_path())).unwrap();
        app.docs[i1].editor.insert_text("// a1 only\n// more\n");
        rename(&mut app, &ctx, 1, "new");
        // The tab that asked closes before the answer: a1 takes over.
        let i2 = app.active;
        app.close_tab(i2);
        wait(&mut app, &ctx, "the answer", |a| a.lsp.rename.is_none() && a.lsp.applying.is_none());
        let a1 = app.docs.iter().find(|d| d.path.as_deref() == Some(base.join("a1/x.fake").as_path())).unwrap();
        assert_eq!(a1.editor.buffer.slice(0..a1.editor.buffer.len_chars()), "// a1 only\n// more\nold = 1;\n", "nothing applied: {}", app.status);
        assert!(app.status.contains("changed"), "{}", app.status);
    }

    #[test]
    fn saving_doesnt_lose_track_of_which_file_a_tab_is() {
        let (mut app, ctx, dir) = app_with("rename-after-save", &[("a.fake", "old = 1;\n")], json!({"shout_paths": true}));
        if !dir.join("A.FAKE").exists() {
            return; // A case-sensitive file system: covered by the hub's hard-link test.
        }
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        // A save renames a new file over the old one: a new identity.
        app.doc().editor.insert_text("// saved\n");
        app.save(0, false);
        frame(&mut app, &ctx, vec![]);
        put_cursor(&mut app, 0);
        app.doc().editor.insert_text("// unsaved\n");
        frame(&mut app, &ctx, vec![]);
        rename(&mut app, &ctx, 21, "new");
        wait(&mut app, &ctx, "the rename", |a| a.lsp.rename.is_none() && a.lsp.applying.is_none());
        assert_eq!(app.docs.len(), 1, "no second tab: {}", app.status);
        assert_eq!(text(&app), "// unsaved\n// saved\nnew = 1;\n");
    }

    #[test]
    fn a_differently_cased_path_from_the_server_finds_the_open_tab() {
        let (mut app, ctx, dir) = app_with("rename-case", &[("a.fake", "old = 1;\n")], json!({"shout_paths": true}));
        if !dir.join("A.FAKE").exists() {
            return; // A case-sensitive file system: no such thing as a differently cased same file.
        }
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        // Unsaved edits the file on disk doesn't have.
        app.doc().editor.insert_text("// unsaved\n");
        frame(&mut app, &ctx, vec![]);
        rename(&mut app, &ctx, 12, "new");
        wait(&mut app, &ctx, "the rename", |a| a.lsp.rename.is_none() && a.lsp.applying.is_none());
        assert_eq!(app.docs.len(), 1, "no second tab from the stale file on disk: {}", app.status);
        assert_eq!(text(&app), "// unsaved\nnew = 1;\n");
    }

    #[test]
    fn workspace_edits_check_everything_before_changing_anything() {
        let (mut app, _ctx, dir) = app_with("wsedit", &[("a.fake", "one\n"), ("ro.fake", "two\n")], json!({}));
        app.open_path(dir.join("ro.fake"), false);
        app.doc().editor.read_only = true;
        let edit = |line| crate::lsp::edit::TextEdit { range: (Pos::new(line, 0), Pos::new(line, 3)), text: "X".into() };
        let files = vec![FileEdit { path: uri::normalize(&dir.join("a.fake")), version: None, edits: vec![edit(0)] }, FileEdit { path: uri::normalize(&dir.join("ro.fake")), version: None, edits: vec![edit(0)] }];
        let err = app.lsp_apply_workspace_edit(files, Encoding::Utf16, None).unwrap_err();
        assert!(err.contains("read-only"), "{err}");
        let a = app.docs.iter().find(|d| is(d, &dir.join("a.fake"))).unwrap();
        assert_eq!(a.editor.buffer.line_text(0), "one", "nothing changed");
        // A missing file is also refused up front.
        let files = vec![FileEdit { path: dir.join("gone.fake"), version: None, edits: vec![edit(0)] }];
        assert!(app.lsp_apply_workspace_edit(files, Encoding::Utf16, None).is_err());
    }

    #[test]
    fn format_document_keeps_the_cursor() {
        let (mut app, ctx, _) = app_with("format", &[("a.fake", "one   \ntwo\t\nthree\n")], json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        put_cursor(&mut app, 18);
        app.run(&ctx, Cmd::LspFormat);
        wait(&mut app, &ctx, "formatting", |a| text(a) == "one\ntwo\nthree\n");
        assert_eq!(app.docs[0].editor.primary().head, 14, "still before `ee`");
        assert_eq!(app.status, "Formatted");
        app.doc().editor.undo();
        assert_eq!(text(&app), "one   \ntwo\t\nthree\n");
    }

    #[test]
    fn parameter_hints_after_an_open_paren() {
        let (mut app, ctx, _) = app_with("signature", &[("a.fake", "call\n")], json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        put_cursor(&mut app, 4);
        frame(&mut app, &ctx, vec![Event::Text("(".into())]);
        wait(&mut app, &ctx, "parameter hints", |a| a.lsp.signature_box.is_some());
        let s = app.lsp.signature_box.as_ref().unwrap();
        assert_eq!(&s.label[s.active.clone().unwrap()], "first: i32");
        frame(&mut app, &ctx, vec![press(egui::Key::Escape, Modifiers::NONE)]);
        frame(&mut app, &ctx, vec![]);
        assert!(app.lsp.signature_box.is_none());
    }

    #[test]
    fn a_crashing_server_is_restarted_and_says_so_in_the_status_bar() {
        let dir = temp_dir("lsp-crash-ui");
        let marker = dir.join("crashed");
        let (mut app, ctx, _) = app_with("crash", &[("a.fake", "fine bad\n")], json!({"marker": marker}));
        wait(&mut app, &ctx, "diagnostics", |a| !markers(a).is_empty());
        put_cursor(&mut app, 0);
        frame(&mut app, &ctx, vec![Event::Text("CRASH_NOW ".into())]);
        wait(&mut app, &ctx, "the restart notice", |a| a.lsp_status().is_some_and(|s| s.0.contains("restarting")));
        assert!(app.docs[0].editor.markers().is_empty(), "a stopped server's problems go");
        wait(&mut app, &ctx, "the server back with its diagnostics", |a| !markers(a).is_empty() && a.lsp_status().is_some_and(|s| !s.0.contains("restarting")));
    }

    #[test]
    fn syncing_a_big_file_costs_the_ui_thread_next_to_nothing() {
        // 100,000 lines (about 3 MB): each keystroke's sync is a snapshot of
        // the rope (shared, not copied); the diff happens on the writer thread.
        let big: String = (0..100_000).map(|i| format!("line {i} of a big file\n")).collect();
        let (mut app, ctx, _) = app_with("perf", &[("a.fake", &big)], json!({}));
        put_cursor(&mut app, 50);
        let mut worst = Duration::ZERO;
        for _ in 0..30 {
            app.doc().editor.insert_text("x");
            let t = Instant::now();
            app.lsp_frame(&ctx);
            worst = worst.max(t.elapsed());
        }
        assert!(worst < Duration::from_millis(5), "slowest sync took {worst:?}");
        // And the server got every keystroke (it reports on the text it has).
        app.doc().editor.insert_text(" bad ");
        wait(&mut app, &ctx, "diagnostics on the big file", |a| markers(a).len() == 1);
    }

    /// Real servers, when asked for: `STRATA_LSP_REAL=<folder> cargo test
    /// --release lsp_real_servers -- --nocapture` drives rust-analyzer (and
    /// clangd, when installed) through the window on a small project and
    /// saves screenshots of each step in the folder. `STRATA_LSP_REPO=<a
    /// file in a big Rust project>` also tries hover and definition there.
    #[test]
    fn lsp_real_servers() {
        let Some(shots) = std::env::var_os("STRATA_LSP_REAL").map(PathBuf::from) else { return };
        std::fs::create_dir_all(&shots).unwrap();
        let mut shooter = super::super::headless::Shooter::new(1200, 700, 2.0);
        let mut shoot = |app: &mut StrataApp, ctx: &Context, name: &str| {
            if let Some(s) = shooter.as_mut() {
                s.shoot(ctx, app, 3, vec![], &shots.join(format!("{name}.ppm")));
                eprintln!("saved {}", shots.join(format!("{name}.ppm")).display());
            }
        };
        let wait_long = |app: &mut StrataApp, ctx: &Context, secs: u64, what: &str, done: &dyn Fn(&StrataApp) -> bool| {
            let t = Instant::now();
            while !done(app) {
                assert!(t.elapsed() < Duration::from_secs(secs), "timed out waiting for {what} (status: {:?}, {})", app.lsp_status(), app.status);
                frame(app, ctx, vec![]);
                std::thread::sleep(Duration::from_millis(20));
            }
            eprintln!("{what}: {:.1} s", t.elapsed().as_secs_f32());
        };
        let find = |app: &StrataApp, needle: &str| app.docs[app.active].editor.buffer.slice(0..app.docs[app.active].editor.buffer.len_chars()).find(needle).map(|b| app.docs[app.active].editor.buffer.byte_to_char(b)).unwrap();

        // A small cargo project with a type error.
        let dir = temp_dir("lsp-real");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "mod util;\n\nfn main() {\n    let total = util::add_numbers(2, 3);\n    let wrong: i32 = \"text\";\n    println!(\"{total} {wrong}\");\n}\n").unwrap();
        std::fs::write(dir.join("src/util.rs"), "/// Adds two numbers.\npub fn add_numbers(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub fn   messy( x:i32 )->i32{x}\n").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(None, None);
        app.settings.minimap = false;
        app.lsp.hub.borrow_mut().activate();
        app.open_folder(dir.clone());
        app.open_path(dir.join("src/main.rs"), false);
        // Nothing starts until the folder is trusted.
        wait_long(&mut app, &ctx, 10, "the trust notice", &|a| a.lsp.hub.borrow().trust_question(a.lsp.window).is_some());
        assert!(app.lsp.hub.borrow().pid_for(&dir.join("src/main.rs")).is_none());
        shoot(&mut app, &ctx, "0-trust-notice");
        let (root, names) = app.lsp.hub.borrow().trust_question(app.lsp.window).unwrap();
        eprintln!("asked about {} for {names:?}", root.display());
        app.lsp_answer_trust(&root, Trust::Once);
        wait_long(&mut app, &ctx, 180, "rust-analyzer's diagnostics", &|a| a.docs[a.active].editor.markers().iter().any(|m| m.style == 1));
        let name = app.lsp_status().unwrap().0;
        assert!(name.starts_with("rust-analyzer"), "{name}");
        let err = app.docs[app.active].editor.markers().iter().find(|m| m.style == 1).unwrap().range.clone();
        eprintln!("error marker on {:?}", app.docs[app.active].editor.buffer.slice(err.clone()));
        shoot(&mut app, &ctx, "1-diagnostics");

        // Hover on the function.
        let call = find(&app, "add_numbers");
        put_cursor(&mut app, call + 2);
        frame(&mut app, &ctx, vec![]);
        app.run(&ctx, Cmd::LspHover);
        wait_long(&mut app, &ctx, 60, "hover", &|a| a.lsp.hover.shown.as_ref().and_then(|b| b.info.as_ref()).is_some_and(|i| i.contains("add_numbers")));
        eprintln!("hover: {:?}", app.lsp.hover.shown.as_ref().unwrap().info);
        shoot(&mut app, &ctx, "2-hover");

        // Completion after `util::`.
        let end = app.docs[app.active].editor.buffer.line_end(4);
        put_cursor(&mut app, end);
        app.settings.auto_complete = true;
        frame(&mut app, &ctx, vec![press(egui::Key::Enter, Modifiers::NONE)]);
        frame(&mut app, &ctx, vec![Event::Text("util::".into())]);
        frame(&mut app, &ctx, vec![Event::Text("ad".into())]);
        wait_long(&mut app, &ctx, 60, "completions", &|a| a.completion.labels().iter().any(|l| l.starts_with("add_numbers")));
        eprintln!("completions: {:?}", app.completion.labels());
        shoot(&mut app, &ctx, "3-completion");
        frame(&mut app, &ctx, vec![press(egui::Key::Enter, Modifiers::NONE)]);
        eprintln!("after accepting: {:?}", app.docs[app.active].editor.buffer.line_text(5));
        assert!(app.docs[app.active].editor.buffer.line_text(5).contains("util::add_numbers"));
        app.doc().editor.undo();
        app.doc().editor.undo();
        app.doc().editor.undo();
        frame(&mut app, &ctx, vec![]);

        // Go to definition: into util.rs.
        put_cursor(&mut app, call + 2);
        app.run(&ctx, Cmd::GotoDefinition);
        wait_long(&mut app, &ctx, 60, "the definition", &|a| a.docs[a.active].path.as_ref().is_some_and(|p| p.ends_with("util.rs")));
        let sel = app.docs[app.active].editor.primary().range();
        assert_eq!(app.docs[app.active].editor.buffer.slice(sel), "add_numbers");
        shoot(&mut app, &ctx, "4-definition");

        // References from the definition: the declaration and the call.
        app.run(&ctx, Cmd::GotoReference);
        wait_long(&mut app, &ctx, 60, "references", &|a| a.palette.open || a.lsp.goto.is_none() && a.docs[a.active].path.as_ref().is_some_and(|p| p.ends_with("main.rs")));
        if let Choosing::Locations(spots) = &app.choosing {
            eprintln!("{} references", spots.len());
        }
        shoot(&mut app, &ctx, "5-references");
        app.palette.open = false;
        app.choosing = Choosing::Nothing;

        // Rename across both files (main.rs open, util.rs too).
        let i = app.docs.iter().position(|d| d.path.as_ref().is_some_and(|p| p.ends_with("util.rs"))).unwrap();
        app.activate(i);
        let at = find(&app, "add_numbers");
        put_cursor(&mut app, at + 1);
        frame(&mut app, &ctx, vec![]);
        app.run(&ctx, Cmd::LspRename);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![Event::Text("sum_numbers".into())]);
        shoot(&mut app, &ctx, "6-rename-box");
        frame(&mut app, &ctx, vec![press(egui::Key::Enter, Modifiers::NONE)]);
        let main = app.docs.iter().position(|d| d.path.as_ref().is_some_and(|p| p.ends_with("main.rs"))).unwrap();
        wait_long(&mut app, &ctx, 60, "the rename", &|a| a.docs[main].editor.buffer.line_text(3).contains("util::sum_numbers"));
        eprintln!("rename: {}", app.status);
        shoot(&mut app, &ctx, "7-renamed");

        // Format the messy function.
        app.activate(i);
        app.run(&ctx, Cmd::LspFormat);
        wait_long(&mut app, &ctx, 60, "formatting", &|a| a.docs[i].editor.buffer.slice(0..a.docs[i].editor.buffer.len_chars()).contains("pub fn messy(x: i32) -> i32 {"));
        shoot(&mut app, &ctx, "8-formatted");

        // Save (a new file renamed over the old), then rename again: the
        // saved tab and the unsaved one are both still found.
        #[cfg(unix)]
        let inode = |p: &Path| std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(p).unwrap());
        #[cfg(unix)]
        let before = inode(&dir.join("src/util.rs"));
        app.activate(i);
        assert!(app.save(i, false), "{}", app.status);
        #[cfg(unix)]
        assert_ne!(inode(&dir.join("src/util.rs")), before, "the save replaced the file");
        frame(&mut app, &ctx, vec![]);
        let at = find(&app, "sum_numbers");
        put_cursor(&mut app, at + 1);
        frame(&mut app, &ctx, vec![]);
        app.run(&ctx, Cmd::LspRename);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![Event::Text("total_of".into())]);
        frame(&mut app, &ctx, vec![press(egui::Key::Enter, Modifiers::NONE)]);
        let main = app.docs.iter().position(|d| d.path.as_ref().is_some_and(|p| p.ends_with("main.rs"))).unwrap();
        wait_long(&mut app, &ctx, 60, "the rename after saving", &|a| a.docs[main].editor.buffer.line_text(3).contains("util::total_of") && a.docs[i].editor.buffer.line_text(1).contains("fn total_of"));
        assert_eq!(app.docs.len(), 2, "no duplicate tabs: {}", app.status);
        eprintln!("rename after save: {}", app.status);
        shoot(&mut app, &ctx, "8b-renamed-after-save");

        // clangd, when installed: a C file with an error.
        if crate::lsp::config::find_executable("clangd", &crate::build::augmented_path().or_else(|| std::env::var_os("PATH")).unwrap_or_default()).is_some() {
            let cdir = temp_dir("lsp-real-c");
            std::fs::write(cdir.join("compile_flags.txt"), "-std=c11\n").unwrap();
            std::fs::write(cdir.join("main.c"), "#include <stdio.h>\n\nstatic int twice(int n) { return n * 2; }\n\nint main(void) {\n    printf(\"%d\\n\", twice(21));\n    return undefined_name;\n}\n").unwrap();
            let ctx = Context::default();
            let mut app = StrataApp::new(None, None);
            app.settings.minimap = false;
            app.settings.lsp_trust_all = true;
            app.lsp.hub.borrow_mut().activate();
            app.open_folder(cdir.clone());
            app.open_path(cdir.join("main.c"), false);
            wait_long(&mut app, &ctx, 120, "clangd's diagnostics", &|a| a.docs[a.active].editor.markers().iter().any(|m| m.style == 1));
            let at = find(&app, "twice(21)");
            put_cursor(&mut app, at + 1);
            frame(&mut app, &ctx, vec![]);
            app.run(&ctx, Cmd::LspHover);
            wait_long(&mut app, &ctx, 60, "clangd hover", &|a| a.lsp.hover.shown.as_ref().and_then(|b| b.info.as_ref()).is_some_and(|i| i.contains("twice")));
            shoot(&mut app, &ctx, "c-clangd");
            app.run(&ctx, Cmd::GotoDefinition);
            wait_long(&mut app, &ctx, 60, "clangd definition", &|a| a.docs[a.active].editor.cursor_line_col().0 == 3);
        }

        // Optionally, a big real project.
        if let Some(file) = std::env::var_os("STRATA_LSP_REPO").map(PathBuf::from) {
            let ctx = Context::default();
            let mut app = StrataApp::new(None, None);
            app.settings.minimap = false;
            app.settings.lsp_trust_all = true;
            app.lsp.hub.borrow_mut().activate();
            app.open_path(file.clone(), false);
            wait_long(&mut app, &ctx, 120, "the big project's server", &|a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
            let at = find(&app, "pub struct StrataApp") + "pub struct ".len();
            put_cursor(&mut app, at + 3);
            frame(&mut app, &ctx, vec![]);
            // While it indexes, hover may come back empty: ask again now and then.
            let t = Instant::now();
            while !app.lsp.hover.shown.as_ref().and_then(|b| b.info.as_ref()).is_some_and(|i| i.contains("StrataApp")) {
                assert!(t.elapsed() < Duration::from_secs(600), "no hover in the big project ({:?})", app.lsp_status());
                app.run(&ctx, Cmd::LspHover);
                for _ in 0..100 {
                    frame(&mut app, &ctx, vec![]);
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            eprintln!("hover in the big project after {:.1} s (status {:?})", t.elapsed().as_secs_f32(), app.lsp_status());
            shoot(&mut app, &ctx, "9-repo-hover");
            // A name defined in another file.
            let at = find(&app, "lsp_ui::LspState::new") + "lsp_ui::".len();
            put_cursor(&mut app, at + 2);
            app.run(&ctx, Cmd::GotoDefinition);
            wait_long(&mut app, &ctx, 120, "definition in the big project", &|a| a.docs[a.active].path.as_ref() != Some(&file));
            eprintln!("definition of LspState: {:?} line {}", app.docs[app.active].path, app.docs[app.active].editor.cursor_line_col().0);
            shoot(&mut app, &ctx, "10-repo-definition");
        }
    }

    /// `STRATA_LSP_SHOTS=<folder>`: the popup tests below also save screenshots there.
    fn maybe_shoot(app: &mut StrataApp, ctx: &Context, name: &str) {
        let Some(dir) = std::env::var_os("STRATA_LSP_SHOTS").map(PathBuf::from) else { return };
        let _ = std::fs::create_dir_all(&dir);
        if let Some(mut s) = super::super::headless::Shooter::new(1000, 600, 2.0) {
            s.shoot(ctx, app, 3, vec![], &dir.join(format!("{name}.ppm")));
        }
    }

    #[test]
    fn the_completion_list_stays_inside_the_window() {
        let line = format!("{}fa", "x ".repeat(44));
        let (mut app, ctx, _) = app_with("complete-edge", &[("a.fake", "start\n")], json!({}));
        app.settings.auto_complete = true;
        app.settings.minimap = false;
        app.doc().wrap = Some(false);
        app.doc().editor.insert_text(&line[..line.len() - 1]);
        frame(&mut app, &ctx, vec![Event::Text("a".into())]);
        wait(&mut app, &ctx, "server completions", |a| a.completion.labels().first().map(String::as_str) == Some("fake_alpha"));
        frame(&mut app, &ctx, vec![]);
        let caret = app.docs[0].view.caret_rect().unwrap();
        assert!(caret.left() > 700.0, "the cursor is near the right edge ({caret:?})");
        let list = ctx.memory(|m| m.area_rect(egui::Id::new("completions"))).unwrap();
        assert!(list.right() <= 1000.0 - 4.0 && list.left() >= 0.0, "{list:?}");
        // The long signature is cut short rather than clipped by the window.
        let out = frame(&mut app, &ctx, vec![]);
        let notes: Vec<String> = out.shapes.iter().filter_map(|c| match &c.shape {
            egui::Shape::Text(t) if t.galley.text().starts_with("fn fake_alpha") => Some(t.galley.text().to_owned()),
            _ => None,
        }).collect();
        assert!(notes.iter().any(|n| n.ends_with('…')), "{notes:?}");
        maybe_shoot(&mut app, &ctx, "completion-at-edge");
    }

    #[test]
    fn hover_closes_when_the_rename_box_or_palette_opens() {
        let (mut app, ctx, _) = app_with("hover-modal", &[("a.fake", "let thing = 1;\n")], json!({}));
        wait(&mut app, &ctx, "ready", |a| a.lsp_status().is_some_and(|s| !s.0.contains("starting")));
        put_cursor(&mut app, 6);
        frame(&mut app, &ctx, vec![]);
        let over = app.docs[0].view.caret_rect().unwrap().center() + vec2(2.0, 0.0);
        let hover_up = |app: &mut StrataApp, ctx: &Context| {
            for _ in 0..4 {
                frame(app, ctx, vec![Event::PointerMoved(over)]);
            }
            wait(app, ctx, "hover box", |a| a.lsp.hover.shown.as_ref().is_some_and(|b| b.info.is_some()));
        };
        hover_up(&mut app, &ctx);
        app.run(&ctx, Cmd::LspRename);
        frame(&mut app, &ctx, vec![]);
        assert!(app.lsp.renaming());
        assert!(app.lsp.hover.shown.is_none(), "the rename box replaces the hover box");
        // Resting the pointer there while renaming doesn't bring it back.
        for _ in 0..4 {
            frame(&mut app, &ctx, vec![Event::PointerMoved(over)]);
        }
        std::thread::sleep(Duration::from_millis(200));
        frame(&mut app, &ctx, vec![]);
        assert!(app.lsp.hover.shown.is_none() && app.lsp.hover.pending.is_none());
        maybe_shoot(&mut app, &ctx, "rename-without-hover");
        frame(&mut app, &ctx, vec![press(egui::Key::Escape, Modifiers::NONE)]);
        assert!(!app.lsp.renaming());
        // The palette too.
        frame(&mut app, &ctx, vec![Event::PointerMoved(Pos2::new(5.0, 590.0))]);
        hover_up(&mut app, &ctx);
        app.run(&ctx, Cmd::CommandPalette);
        frame(&mut app, &ctx, vec![]);
        assert!(app.lsp.hover.shown.is_none());
    }

    #[test]
    fn the_trust_notice_asks_before_a_server_starts() {
        let dir = temp_dir("lsp-trust-ui");
        std::fs::write(dir.join("a.fake"), "let bad = 1;\n").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(None, None);
        app.lsp.hub.borrow_mut().use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        app.lsp.hub.borrow_mut().require_trust();
        app.open_folder(dir.clone());
        app.open_path(dir.join("a.fake"), false);
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![]);
        }
        assert!(app.lsp.hub.borrow().pid_for(&dir.join("a.fake")).is_none());
        let notice = ctx.memory(|m| m.area_rect(app.link.id("lsp-trust")));
        assert!(notice.is_some(), "the notice is up");
        assert_eq!(app.lsp_status().map(|s| (s.0.ends_with("not started"), s.3)), Some((true, StatusClick::Ask)));
        maybe_shoot(&mut app, &ctx, "trust-notice");
        // Typing goes on meanwhile: the notice doesn't take the keyboard.
        frame(&mut app, &ctx, vec![Event::Text("x".into())]);
        assert!(text(&app).starts_with('x'));
        app.lsp_answer_trust(&dir, Trust::Once);
        wait(&mut app, &ctx, "diagnostics", |a| !markers(a).is_empty());
        assert!(ctx.memory(|m| m.area_rect(app.link.id("lsp-trust"))).is_none() || app.lsp.hub.borrow().trust_question(app.lsp.window).is_none());
    }

    #[test]
    fn language_server_keys_leave_f8_to_review_changes() {
        use crate::keybindings::{Keymap, Target};
        let keymap = Keymap::defaults();
        for b in keymap.bindings() {
            if let Some(Target::Cmd(c)) = b.action
                && c.id().starts_with("lsp_")
            {
                assert!(b.keys.iter().all(|s| s.key != egui::Key::F8), "{} uses F8", c.id());
            }
        }
        // Next and previous problem are ⌘K / Ctrl+K chords.
        for c in [Cmd::LspNextProblem, Cmd::LspPrevProblem] {
            let strokes: Vec<_> = keymap.bindings().iter().filter(|b| b.action == Some(Target::Cmd(c))).map(|b| b.keys.clone()).collect();
            assert!(strokes.len() == 1 && strokes[0].len() == 2 && strokes[0][0].key == egui::Key::K, "{strokes:?}");
        }
    }

    #[test]
    fn answers_in_their_various_shapes() {
        let (a, b) = if cfg!(windows) { ("file:///C:/p/a.rs", r"C:\p\a.rs") } else { ("file:///p/a.rs", "/p/a.rs") };
        let r = json!({"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}});
        let one = locations(&json!({"uri": a, "range": r}));
        assert_eq!(one, vec![(PathBuf::from(b), (Pos::new(1, 2), Pos::new(1, 5)))]);
        let links = locations(&json!([{"targetUri": a, "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}}, "targetSelectionRange": r}]));
        assert_eq!(links[0].1, (Pos::new(1, 2), Pos::new(1, 5)), "the name, not the whole item");
        assert!(locations(&Value::Null).is_empty());

        assert_eq!(hover_markdown(&json!({"contents": {"kind": "markdown", "value": "**x**"}})).as_deref(), Some("**x**"));
        assert_eq!(hover_markdown(&json!({"contents": {"language": "rust", "value": "fn x()"}})).as_deref(), Some("```rust\nfn x()\n```"));
        assert_eq!(hover_markdown(&json!({"contents": ["a", {"language": "c", "value": "int x"}]})).as_deref(), Some("a\n\n---\n\n```c\nint x\n```"));
        assert_eq!(hover_markdown(&json!({"contents": {"kind": "plaintext", "value": "a *b*"}})).as_deref(), Some("    a *b*"), "plain text isn't Markdown");
        assert_eq!(hover_markdown(&json!({"contents": ""})), None);
    }
}
