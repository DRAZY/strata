//! Plugins, the window half: the consent question, the Plugins manager,
//! the panel plugins show at the bottom, their status bar items, pick lists
//! (the palette's), commands in the palette and under Tools ▸ Plugins, their
//! keys, and the requests about documents (read, and edits that go through
//! the editor, so Undo takes them back). The plugins themselves are run by
//! `crate::plugins::manager`, shared by every window.

use super::StrataApp;
use super::more::Choosing;
use crate::commands::Cmd;
use crate::markdown::{DocRender, TopBlock};
use crate::menus::{Act, Item};
use crate::plugins::api::{self, BUSY, DOCUMENT_CHANGED, METHOD_NOT_FOUND, NO_DOCUMENT, READ_ONLY, RpcError, Reply, TOO_LARGE};
use crate::plugins::host::Incoming;
use crate::plugins::manager::{self, DocInfo, Manager, State};
use crate::plugins::package;
use egui::RichText;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;

/// Menu and palette actions that run a plugin command: `plugin:<name>:<command>`.
pub const ACT_PREFIX: &str = "plugin:";
const MAX_PANEL: usize = 1 << 20;
const MAX_PICK: usize = 5000;
/// Requests a window answers per frame; the rest wait (the plugin with them).
const REQUESTS_PER_FRAME: usize = 32;

fn next_window() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A panel a plugin showed.
struct Panel {
    plugin: String,
    title: String,
    blocks: Vec<TopBlock>,
    /// Plain text, shown as it is (instead of Markdown).
    text: Option<String>,
}

pub(super) struct PluginsState {
    pub mgr: Rc<RefCell<Manager>>,
    window: u64,
    panel: Option<Panel>,
    render: DocRender,
    /// A pick list on show: (plugin, run, request id).
    pick: Option<(String, u64, Option<Value>)>,
    pub manager_open: bool,
    selected: Option<String>,
    installing: Option<Receiver<Result<package::Staged, String>>>,
    /// Asking before removing this plugin.
    removing: Option<String>,
    /// A dialog closed: the editor takes the keyboard back next frame (not
    /// this one, while the dialog still holds it).
    refocus: bool,
}

impl PluginsState {
    pub fn new(config: Option<&Path>) -> Self {
        Self::sharing(Rc::new(RefCell::new(Manager::new(config, None))))
    }

    /// Another window's, sharing the plugins.
    pub fn sharing(mgr: Rc<RefCell<Manager>>) -> Self {
        Self { mgr, window: next_window(), panel: None, render: DocRender::compact(), pick: None, manager_open: false, selected: None, installing: None, removing: None, refocus: false }
    }

    /// A question of the plugins' is up in this window (dialog keys are its).
    pub fn asking(&self) -> bool {
        self.removing.is_some() || self.mgr.try_borrow().is_ok_and(|m| m.is_primary(self.window) && m.question().is_some())
    }
}

impl Drop for PluginsState {
    fn drop(&mut self) {
        if let Ok(mut m) = self.mgr.try_borrow_mut() {
            m.forget_window(self.window);
        }
    }
}

/// Keys Strata keeps outside its keymap (clipboard, quitting, hiding).
fn reserved(mods: egui::Modifiers, key: egui::Key) -> bool {
    use egui::Key::*;
    crate::keys::exact(mods, crate::keys::CMD) && matches!(key, C | X | V | Q | H | M)
}

/// A plugin's text for a place that takes only a plain string (menus,
/// tooltips, dialog lines): hidden and direction-changing characters
/// spelled out (`[RLO]`), so one can't reorder or hide what's around it.
fn shown(s: &str) -> String {
    crate::hidden::plain(s).into_owned()
}

/// One line of text from a plugin, without control characters, at most `max` chars.
fn line(s: &str, max: usize) -> String {
    s.lines().next().unwrap_or_default().chars().filter(|c| !c.is_control()).take(max).collect()
}

impl StrataApp {
    /// The plugin documents of this window (binary files aren't text).
    fn plugin_docs(&self) -> Vec<DocInfo> {
        self.docs.iter().filter(|d| d.binary.is_none()).map(|d| DocInfo { id: d.editor.id(), path: d.path.clone(), name: d.name(), version: d.editor.version(), syntax: d.highlighter.syntax_name().to_owned() }).collect()
    }

    fn plugin_doc_info(&self, i: usize) -> DocInfo {
        let d = &self.docs[i];
        DocInfo { id: d.editor.id(), path: d.path.clone(), name: d.name(), version: d.editor.version(), syntax: d.highlighter.syntax_name().to_owned() }
    }

    pub(super) fn plugins_frame(&mut self, ctx: &egui::Context) {
        let window = self.plugins.window;
        let docs = self.plugin_docs();
        let front = self.docs.get(self.active).filter(|d| d.binary.is_none()).map(|d| d.editor.id());
        let focused = ctx.input(|i| i.focused);
        let (requests, notes) = {
            let mut m = self.plugins.mgr.borrow_mut();
            m.set_context(ctx);
            m.set_user_settings(&self.settings.plugins);
            m.sync_window(window, docs, self.sidebar.roots.clone(), focused, front);
            m.tick();
            let notes = if m.is_primary(window) { m.take_notes() } else { Vec::new() };
            (m.take_requests(window, REQUESTS_PER_FRAME), notes)
        };
        for (name, run, inc) in requests {
            // The window holds the request's data now: room for more from the plugin.
            self.plugins.mgr.borrow().handled(&name, run, inc.size);
            self.plugin_request(ctx, name, run, inc);
        }
        if let Some(n) = notes.into_iter().last() {
            self.status = n;
        }
        // The pick list closed some other way (Esc, a click outside, another list).
        if self.plugins.pick.is_some() && (!self.palette.open || !matches!(self.choosing, Choosing::Plugin)) {
            self.plugin_chose(None);
        }
        self.plugin_installing();
        // The keyboard goes back to the editor after a dialog (Undo, typing).
        if std::mem::take(&mut self.plugins.refocus) && !self.modal_open(ctx) {
            self.doc().view.focus();
        }
        self.plugin_keys(ctx);
        self.plugin_consent_ui(ctx);
        self.plugin_remove_ui(ctx);
        if self.plugins.manager_open {
            self.plugins_window(ctx);
        }
    }

    pub(super) fn plugins_command(&mut self, ctx: &egui::Context, cmd: Cmd) {
        match cmd {
            Cmd::PluginsManage => self.plugins.manager_open = true,
            Cmd::PluginsInstallFolder | Cmd::PluginsInstallZip => {
                if self.config.is_none() {
                    self.status = "Plugins need Strata's settings folder, which isn't available".into();
                    return;
                }
                let zip = cmd == Cmd::PluginsInstallZip;
                let picked = if zip { rfd::FileDialog::new().add_filter("Zip archive", &["zip"]).pick_file() } else { rfd::FileDialog::new().pick_folder() };
                if let Some(p) = picked {
                    self.plugin_install_from(ctx, p, zip);
                }
            }
            _ => {}
        }
    }

    /// Install from a folder or zip: read and checked off the window's
    /// thread, then swapped in (stopping an older version first).
    pub(super) fn plugin_install_from(&mut self, ctx: &egui::Context, path: PathBuf, zip: bool) {
        let Some(config) = self.config.clone() else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let job = move || {
            let files = if zip { package::read_zip(&path) } else { package::read_folder(&path) };
            let _ = tx.send(files.and_then(|f| package::stage(&config, f)));
            ctx.request_repaint();
        };
        match std::thread::Builder::new().name("plugin-install".into()).spawn(job) {
            Ok(_) => {
                self.plugins.installing = Some(rx);
                self.status = "Installing the plugin…".into();
            }
            Err(e) => self.status = format!("Couldn't install the plugin: {e}"),
        }
    }

    fn plugin_installing(&mut self) {
        let Some(rx) = &self.plugins.installing else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.plugins.installing = None;
        let name = result.as_ref().ok().map(|s| s.manifest.name.clone());
        self.status = match result.and_then(|staged| self.plugins.mgr.borrow_mut().commit_install(staged)) {
            Ok(msg) => {
                self.plugins.manager_open = true;
                self.plugins.selected = name;
                msg
            }
            Err(e) => format!("Couldn't install the plugin: {e}"),
        };
    }

    /// Whether an install is still being read (tests).
    #[cfg(test)]
    pub(super) fn plugin_installing_now(&self) -> bool {
        self.plugins.installing.is_some()
    }

    // ---- commands, menus, palette, keys --------------------------------------------------

    fn plugin_run(&mut self, name: &str, cmd: &str) {
        let doc = (self.active < self.docs.len() && self.docs[self.active].binary.is_none()).then(|| self.plugin_doc_info(self.active));
        self.plugins.mgr.borrow_mut().run_command(name, cmd, doc);
    }

    /// A menu or palette action `plugin:<name>:<command>`.
    pub(super) fn plugin_act(&mut self, act: &str) {
        if let Some((name, cmd)) = act.strip_prefix(ACT_PREFIX).and_then(|r| r.split_once(':')) {
            self.plugin_run(name, cmd);
        }
    }

    /// Tools ▸ Plugins.
    pub(super) fn plugins_menu(&self) -> Item {
        let config = self.config.is_some();
        let mut items = vec![
            Item::cmd("Manage Plugins…", Cmd::PluginsManage),
            Item::cmd("Install from Folder…", Cmd::PluginsInstallFolder).enabled(config),
            Item::cmd("Install from Zip…", Cmd::PluginsInstallZip).enabled(config),
        ];
        let m = self.plugins.mgr.borrow();
        let keymap = crate::keybindings::current();
        let (bound, _) = m.key_bindings(|mods, key| keymap.is_bound(mods, key) || reserved(mods, key));
        let mut subs = Vec::new();
        for p in m.plugins.iter().filter(|p| m.enabled(&p.name)) {
            let Ok(mf) = &p.manifest else { continue };
            let cmds: Vec<Item> = mf
                .commands
                .iter()
                .filter(|c| c.menu)
                .map(|c| {
                    let hint = bound.iter().find(|b| b.plugin == p.name && b.command == c.id).map(|b| crate::keys::label(b.mods, b.key));
                    Item::Action { label: shown(&c.title), act: Act::Named(format!("{ACT_PREFIX}{}:{}", p.name, c.id)), keys: None, hint, check: None, enabled: true }
                })
                .collect();
            if !cmds.is_empty() {
                subs.push(Item::Sub(shown(&mf.title), cmds));
            }
        }
        if !subs.is_empty() {
            items.push(Item::Separator);
            items.extend(subs);
        }
        Item::Sub("Plugins".into(), items)
    }

    /// Plugin commands for the palette: (name, action, shortcut).
    pub(super) fn plugin_palette_items(&self) -> Vec<(String, String, String)> {
        let m = self.plugins.mgr.borrow();
        let keymap = crate::keybindings::current();
        let (bound, _) = m.key_bindings(|mods, key| keymap.is_bound(mods, key) || reserved(mods, key));
        let mut out = Vec::new();
        for p in m.plugins.iter().filter(|p| m.enabled(&p.name)) {
            let Ok(mf) = &p.manifest else { continue };
            for c in &mf.commands {
                let key = bound.iter().find(|b| b.plugin == p.name && b.command == c.id).map(|b| crate::keys::label(b.mods, b.key)).unwrap_or_default();
                out.push((format!("{}: {}", mf.title, c.title), format!("{ACT_PREFIX}{}:{}", p.name, c.id), key));
            }
        }
        out
    }

    /// Plugin keys: only free ones (see `Manager::key_bindings`), never in a dialog.
    fn plugin_keys(&mut self, ctx: &egui::Context) {
        if self.modal_open(ctx) || self.palette.open || self.settings_has_keys(ctx) {
            return;
        }
        let keymap = crate::keybindings::current();
        let (bound, _) = self.plugins.mgr.borrow().key_bindings(|mods, key| keymap.is_bound(mods, key) || reserved(mods, key));
        if bound.is_empty() {
            return;
        }
        let terminal = self.terminal_has_keys(ctx);
        let mut fired = Vec::new();
        ctx.input_mut(|i| {
            let mut k = 0;
            while k < i.events.len() {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = &i.events[k]
                    && let Some(b) = bound.iter().find(|b| b.key == *key && crate::keys::exact(*modifiers, b.mods))
                    && (!terminal || crate::terminal::app_keeps(None, *modifiers))
                {
                    fired.push((b.plugin.clone(), b.command.clone()));
                    i.events.remove(k);
                    // The text an Option or AltGr combination types isn't typed either.
                    if matches!(i.events.get(k), Some(egui::Event::Text(_))) {
                        i.events.remove(k);
                    }
                    continue;
                }
                k += 1;
            }
        });
        for (p, c) in fired {
            self.plugin_run(&p, &c);
        }
    }

    /// A document was saved (plugins waiting for it start; running ones hear of it).
    pub(super) fn plugins_saved(&mut self, i: usize) {
        if self.docs.get(i).is_some_and(|d| d.binary.is_none()) {
            let info = self.plugin_doc_info(i);
            self.plugins.mgr.borrow_mut().saved(&info);
        }
    }

    // ---- requests -----------------------------------------------------------------------

    fn plugin_title(&self, name: &str) -> String {
        let m = self.plugins.mgr.borrow();
        m.plugins.iter().find(|p| p.name == name).map_or_else(|| name.to_owned(), |p| p.title())
    }

    fn plugin_request(&mut self, ctx: &egui::Context, name: String, run: u64, inc: Incoming) {
        let reply: Reply = match inc.method.as_str() {
            "document/read" => self.plugin_read(&inc.params),
            "document/applyEdits" => self.plugin_apply(&name, &inc.params),
            "ui/showPanel" => self.plugin_show_panel(&name, &inc.params),
            "ui/notify" => self.plugin_notify(&name, &inc.params),
            "ui/statusItem" => self.plugins.mgr.borrow_mut().set_status_item(&name, &inc.params),
            "ui/pick" => match self.plugin_pick(ctx, &name, run, &inc) {
                // Answered when the list closes.
                Ok(()) => return,
                Err(e) => Err(e),
            },
            other => Err(RpcError::new(METHOD_NOT_FOUND, format!("Strata has no method \"{}\"", line(other, 80)))),
        };
        self.plugins.mgr.borrow().reply(&name, run, inc.id, reply);
    }

    /// The tab a request is about: its `document`, or the active tab.
    fn plugin_target(&self, params: &Value) -> Result<usize, RpcError> {
        let gone = || RpcError::new(NO_DOCUMENT, "the document isn't open");
        let i = match params.get("document") {
            None | Some(Value::Null) => self.active,
            Some(v) => {
                let id = v.as_u64().ok_or_else(|| RpcError::params("\"document\" must be a document id"))?;
                self.docs.iter().position(|d| d.editor.id() == id).ok_or_else(gone)?
            }
        };
        if self.docs.get(i).is_none_or(|d| d.binary.is_some()) {
            return Err(gone());
        }
        Ok(i)
    }

    fn plugin_read(&mut self, params: &Value) -> Reply {
        let i = self.plugin_target(params)?;
        let info = self.plugin_doc_info(i).to_json();
        let d = &self.docs[i];
        let rope = d.editor.buffer.snapshot();
        if rope.len_bytes() > api::MAX_TEXT {
            return Err(RpcError::new(TOO_LARGE, "the document is larger than 16 MB"));
        }
        let selections: Vec<Value> = d.editor.selections().iter().map(|s| json!({"start": api::position(&rope, s.start()), "end": api::position(&rope, s.end()), "active": api::position(&rope, s.head)})).collect();
        Ok(json!({"document": info, "text": String::from(&rope), "selections": selections, "lineEnding": d.editor.buffer.line_ending().as_str()}))
    }

    /// Edits through the editor: one undo step, only on the version they were made for.
    fn plugin_apply(&mut self, name: &str, params: &Value) -> Reply {
        if params.get("document").and_then(Value::as_u64).is_none() {
            return Err(RpcError::params("\"document\" must be a document id"));
        }
        let version = params.get("version").and_then(Value::as_u64).ok_or_else(|| RpcError::params("\"version\" must be the document version the edits were made for"))?;
        let i = self.plugin_target(params)?;
        let d = &mut self.docs[i];
        if d.editor.read_only {
            return Err(RpcError::new(READ_ONLY, format!("{} is read-only", d.name())));
        }
        if d.editor.version() != version {
            return Err(RpcError::new(DOCUMENT_CHANGED, format!("the document changed since version {version} (it's at {} now); read it again", d.editor.version())));
        }
        let eol = d.editor.buffer.line_ending().as_str();
        let edits = api::char_edits(&d.editor.buffer.snapshot(), params.get("edits").unwrap_or(&Value::Null), eol)?;
        let n = edits.len();
        if n > 0 {
            d.editor.apply_edits(edits);
        }
        let (doc, version) = (d.name(), d.editor.version());
        self.plugins.mgr.borrow().log(name, format!("Changed {doc} ({n} edits)"));
        Ok(json!({"applied": true, "version": version}))
    }

    fn plugin_show_panel(&mut self, name: &str, params: &Value) -> Reply {
        let title = line(params.get("title").and_then(Value::as_str).unwrap_or_default(), 100);
        let md = params.get("markdown").and_then(Value::as_str);
        let text = params.get("text").and_then(Value::as_str);
        let content = md.or(text).ok_or_else(|| RpcError::params("ui/showPanel needs \"markdown\" or \"text\""))?;
        if content.len() > MAX_PANEL {
            return Err(RpcError::new(TOO_LARGE, "a panel holds at most 1 MB"));
        }
        let plugin = self.plugin_title(name);
        self.plugins.panel = Some(match md {
            Some(md) => Panel { plugin, title, blocks: crate::markdown::parse(md), text: None },
            None => Panel { plugin, title, blocks: Vec::new(), text: Some(content.to_owned()) },
        });
        Ok(Value::Null)
    }

    fn plugin_notify(&mut self, name: &str, params: &Value) -> Reply {
        let msg = line(api::str_param(params, "message")?, 300);
        let level = params.get("level").and_then(Value::as_str).unwrap_or("info");
        let title = self.plugin_title(name);
        self.status = if level == "error" || level == "warning" { format!("{title} ({level}): {msg}") } else { format!("{title}: {msg}") };
        Ok(Value::Null)
    }

    fn plugin_pick(&mut self, ctx: &egui::Context, name: &str, run: u64, inc: &Incoming) -> Result<(), RpcError> {
        if inc.id.is_none() {
            return Err(RpcError::new(api::INVALID_REQUEST, "ui/pick must be a request"));
        }
        if self.plugins.pick.is_some() || self.palette.open || self.modal_open(ctx) {
            return Err(RpcError::new(BUSY, "another list or dialog is open"));
        }
        let items = inc.params.get("items").and_then(Value::as_array).ok_or_else(|| RpcError::params("\"items\" must be a list"))?;
        if items.is_empty() || items.len() > MAX_PICK {
            return Err(RpcError::params(format!("\"items\" must have 1 to {MAX_PICK} entries")));
        }
        let labels = items
            .iter()
            .map(|it| match it {
                Value::String(s) => Ok(line(s, 200)),
                Value::Object(_) => {
                    let label = line(api::str_param(it, "label")?, 200);
                    Ok(match it.get("detail").and_then(Value::as_str) {
                        Some(d) => format!("{label} — {}", line(d, 200)),
                        None => label,
                    })
                }
                _ => Err(RpcError::params("each item is text or {\"label\", \"detail\"}")),
            })
            .collect::<Result<Vec<String>, RpcError>>()?;
        let hint = inc.params.get("placeholder").and_then(Value::as_str).map(|p| line(p, 100)).filter(|p| !p.is_empty()).unwrap_or_else(|| format!("{}: choose one", self.plugin_title(name)));
        self.plugins.pick = Some((name.to_owned(), run, inc.id.clone()));
        // The rows are drawn with markers by the palette; its hint is plain text.
        self.choose(ctx, Choosing::Plugin, &shown(&hint), labels);
        Ok(())
    }

    /// The pick list was answered (`Some(index)`) or closed.
    pub(super) fn plugin_chose(&mut self, index: Option<usize>) {
        if let Some((name, run, id)) = self.plugins.pick.take() {
            let reply = Ok(index.map_or(Value::Null, |i| json!({"index": i})));
            self.plugins.mgr.borrow().reply(&name, run, id, reply);
        }
    }

    // ---- the panel and status items -------------------------------------------------------

    /// The last panel a plugin showed, at the bottom like build results.
    pub(super) fn plugins_panel(&mut self, ui: &mut egui::Ui) {
        if self.plugins.panel.is_none() || self.distraction.is_some() {
            return;
        }
        let id = self.link.id("plugin-panel");
        let size = self.settings.font_size.clamp(10.0, 20.0);
        let hl = &self.docs[self.active].highlighter;
        let st = &mut self.plugins;
        let (Some(p), render) = (&mut st.panel, &mut st.render) else { return };
        let (mut close, mut link) = (false, None);
        egui::Panel::bottom(id).resizable(true).default_size(200.0).min_size(80.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                let head = if p.title.is_empty() { p.plugin.clone() } else { format!("{}: {}", p.plugin, p.title) };
                let font = egui::TextStyle::Body.resolve(ui.style());
                ui.label(crate::hidden::widget(&head, font, ui.visuals().strong_text_color()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.small_button("Close").clicked();
                });
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt("plugin-panel-text").auto_shrink(false).show(ui, |ui| match &p.text {
                Some(t) => {
                    let font = egui::TextStyle::Monospace.resolve(ui.style());
                    let mut job = crate::hidden::job(t, egui::text::TextFormat::simple(font, ui.visuals().text_color()));
                    job.wrap.max_width = ui.available_width();
                    ui.add(egui::Label::new(job).wrap());
                }
                None => link = render.show(ui, hl, size, &p.blocks).clicked,
            });
        });
        if close {
            self.plugins.panel = None;
        }
        if let Some(url) = link.filter(|u| u.starts_with("https://") || u.starts_with("http://")) {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    }

    /// The status bar's plugin items as one text (to measure whether they fit).
    pub(super) fn plugin_status_text(&self) -> Option<String> {
        let m = self.plugins.mgr.borrow();
        let items = m.status_items(self.plugins.window);
        (!items.is_empty()).then(|| items.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("  ·  "))
    }

    /// Draw the plugins' status bar items; one with a command runs it on a click.
    pub(super) fn plugin_status_ui(&mut self, ui: &mut egui::Ui) {
        let items: Vec<manager::StatusItem> = self.plugins.mgr.borrow().status_items(self.plugins.window).into_iter().cloned().collect();
        let mut run = None;
        for (k, s) in items.iter().enumerate() {
            if k > 0 {
                ui.label(RichText::new("·").weak());
            }
            let tip = shown(&if s.tooltip.is_empty() { self.plugin_title(&s.plugin) } else { format!("{}: {}", self.plugin_title(&s.plugin), s.tooltip) });
            let text = crate::hidden::label(ui, &s.text);
            let r = match &s.command {
                Some(_) => ui.add(egui::Button::new(text).frame(false)),
                None => ui.label(text),
            };
            if r.on_hover_text(tip).clicked()
                && let Some(c) = &s.command
            {
                run = Some((s.plugin.clone(), c.clone()));
            }
        }
        if let Some((p, c)) = run {
            self.plugin_run(&p, &c);
        }
    }

    // ---- dialogs ------------------------------------------------------------------------

    fn plugin_consent_ui(&mut self, ctx: &egui::Context) {
        let window = self.plugins.window;
        let (dir, sha, mf) = {
            let m = self.plugins.mgr.borrow();
            if !m.is_primary(window) {
                return;
            }
            let Some((p, sha, mf)) = m.question() else { return };
            (p.dir.clone(), sha.to_owned(), mf.clone())
        };
        self.cancel_editor_focus();
        let size = self.settings.ui_font_size.clamp(9.0, 24.0);
        let small = crate::fonts::secondary_size(size);
        let screen = ctx.content_rect();
        let (mut allow, mut deny) = (false, false);
        egui::Modal::new(egui::Id::new("plugin-consent")).show(ctx, |ui| {
            ui.set_width((screen.width() - 80.0).clamp(320.0, 640.0));
            ui.label(RichText::new(format!("Allow the {} plugin to run?", shown(&mf.title))).strong().size(size * 1.1));
            ui.add_space(4.0);
            let mut who = format!("{} {}", mf.title, mf.version);
            if !mf.author.is_empty() {
                who.push_str(&format!(" by {}", mf.author));
            }
            ui.label(RichText::new(shown(&who)).size(small));
            if !mf.description.is_empty() {
                ui.add(egui::Label::new(RichText::new(shown(&mf.description)).size(small)).wrap());
            }
            let mut command = mf.run.command.clone();
            for a in &mf.run.args {
                command.push(' ');
                command.push_str(a);
            }
            ui.add(egui::Label::new(RichText::new(format!("Strata will run: {}", shown(&command))).size(small).monospace()).wrap());
            ui.add(egui::Label::new(RichText::new(format!("In: {}", dir.display())).size(small)).wrap());
            if !mf.run.in_package() {
                let note = format!("{} itself, and libraries installed on your computer, aren't part of the package Strata checks.", shown(&mf.run.command));
                ui.add(egui::Label::new(RichText::new(note).size(small).color(ui.visuals().weak_text_color())).wrap());
            }
            ui.add_space(6.0);
            if mf.permissions.is_empty() {
                ui.label(RichText::new("It asks for no permissions: it can add its commands and write to its own log, nothing more through Strata.").size(small));
            } else {
                ui.label(RichText::new("It asks to:").size(small));
                for p in &mf.permissions {
                    let text = format!("•  {}", p.describe());
                    if p.unlimited() {
                        ui.label(RichText::new(format!("{text} (Strata can't limit this)")).size(small).color(ui.visuals().warn_fg_color));
                    } else {
                        ui.label(RichText::new(text).size(small));
                    }
                }
            }
            ui.add_space(6.0);
            let honest = "A plugin is a program that runs on your computer with your account's access. Strata only does what you allow here when the plugin asks Strata, but it can't sandbox the program itself: like any program you run, it could read or change your files, start other programs or use the network on its own. Allow plugins only from people you trust.";
            ui.add(egui::Label::new(RichText::new(honest).size(small).color(ui.visuals().weak_text_color())).wrap());
            ui.add(egui::Label::new(RichText::new(format!("Package {}… If any of its files change, Strata asks again.", &sha[..12.min(sha.len())])).size(small).color(ui.visuals().weak_text_color())).wrap());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                allow = ui.button("Allow and Start").clicked();
                deny = ui.button("Don't Start").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if allow || deny {
            self.plugins.mgr.borrow_mut().answer(allow);
            self.plugins.refocus = true;
        }
    }

    fn plugin_remove_ui(&mut self, ctx: &egui::Context) {
        let Some(name) = self.plugins.removing.clone() else { return };
        let title = self.plugin_title(&name);
        let (mut remove, mut cancel) = (false, false);
        egui::Modal::new(egui::Id::new("plugin-remove")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(format!("Remove {}?", shown(&title))).strong());
            ui.add(egui::Label::new("Its copy in Strata's settings folder is deleted and what you allowed it is forgotten. Its own data (in the PluginData folder) stays.").wrap());
            ui.horizontal(|ui| {
                remove = ui.button("Remove").clicked();
                cancel = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if remove {
            self.plugin_remove(&name);
        } else if cancel {
            self.plugins.removing = None;
        }
        if remove || cancel {
            self.plugins.refocus = true;
        }
    }

    pub(super) fn plugin_remove(&mut self, name: &str) {
        self.plugins.removing = None;
        let title = self.plugin_title(name);
        self.status = match self.plugins.mgr.borrow_mut().remove(name) {
            Ok(()) => format!("Removed {title}"),
            Err(e) => format!("Couldn't remove {title}: {e}"),
        };
    }

    /// The Plugins manager: every installed plugin, what it may do, its log.
    fn plugins_window(&mut self, ctx: &egui::Context) {
        enum Do {
            Install(bool),
            Enable(String, bool),
            Start(String),
            Stop(String),
            Restart(String),
            Remove(String),
            CopyLog(String),
        }
        let mut action = None;
        let mut open = true;
        let keymap = crate::keybindings::current();
        let config = self.config.clone();
        let st = &mut self.plugins;
        let m = st.mgr.borrow();
        let (_, conflicts) = m.key_bindings(|mods, key| keymap.is_bound(mods, key) || reserved(mods, key));
        if st.selected.as_ref().is_none_or(|s| !m.plugins.iter().any(|p| &p.name == s)) {
            st.selected = m.plugins.first().map(|p| p.name.clone());
        }
        let screen = ctx.content_rect();
        let size = egui::vec2((screen.width() - 80.0).clamp(360.0, 760.0), (screen.height() - 120.0).clamp(240.0, 520.0));
        egui::Window::new("Plugins")
            .id(self.link.id("plugins-window"))
            .open(&mut open)
            .collapsible(false)
            .default_size(size)
            .default_pos(screen.center() - size / 2.0)
            .constrain(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.add_enabled(config.is_some(), egui::Button::new("Install from Folder…")).clicked() {
                        action = Some(Do::Install(false));
                    }
                    if ui.add_enabled(config.is_some(), egui::Button::new("Install from Zip…")).clicked() {
                        action = Some(Do::Install(true));
                    }
                });
                if let Some(c) = &config {
                    ui.label(RichText::new(format!("Installed plugins are kept in {}", package::plugins_dir(c).display())).small().weak());
                }
                ui.separator();
                if m.plugins.is_empty() {
                    ui.label("No plugins are installed. A plugin is a folder (or a zip of one) with a strata-plugin.json; see Plugins in the manual.");
                    return;
                }
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(200.0);
                        egui::ScrollArea::vertical().id_salt("plugins-list").show(ui, |ui| {
                            for p in &m.plugins {
                                let on = m.enabled(&p.name);
                                let version = p.manifest.as_ref().map(|m| m.version.clone()).unwrap_or_default();
                                let mut text = RichText::new(shown(&format!("{}  {version}", p.title())));
                                if !on || p.manifest.is_err() {
                                    text = text.weak();
                                }
                                if ui.selectable_label(st.selected.as_deref() == Some(p.name.as_str()), text).clicked() {
                                    st.selected = Some(p.name.clone());
                                }
                            }
                        });
                    });
                    ui.separator();
                    let Some(p) = m.plugins.iter().find(|p| st.selected.as_deref() == Some(p.name.as_str())) else { return };
                    ui.vertical(|ui| {
                        egui::ScrollArea::vertical().id_salt("plugin-details").show(ui, |ui| {
                            ui.label(RichText::new(shown(&p.title())).strong().size(16.0));
                            let on = m.enabled(&p.name);
                            match &p.manifest {
                                Err(e) => {
                                    ui.add(egui::Label::new(RichText::new(shown(&format!("This plugin can't be used: {e}"))).color(ui.visuals().error_fg_color)).wrap());
                                }
                                Ok(mf) => {
                                    let mut who = format!("Version {}", mf.version);
                                    if !mf.author.is_empty() {
                                        who.push_str(&format!(" · {}", mf.author));
                                    }
                                    if !mf.license.is_empty() {
                                        who.push_str(&format!(" · {}", mf.license));
                                    }
                                    ui.label(RichText::new(shown(&who)).small());
                                    if !mf.description.is_empty() {
                                        ui.add(egui::Label::new(shown(&mf.description)).wrap());
                                    }
                                }
                            }
                            ui.label(RichText::new(format!("Folder: {}", p.dir.display())).small().weak());
                            ui.add_space(4.0);
                            ui.label(shown(&format!("Status: {}", if on { p.state.label() } else { "Turned off".into() })));
                            ui.horizontal(|ui| {
                                let mut enabled = on;
                                if ui.checkbox(&mut enabled, "Turned on").changed() {
                                    action = Some(Do::Enable(p.name.clone(), enabled));
                                }
                                let running = matches!(p.state, State::Running | State::Starting { .. } | State::Checking(_));
                                if running {
                                    if ui.button("Stop").clicked() {
                                        action = Some(Do::Stop(p.name.clone()));
                                    }
                                    if ui.button("Restart").clicked() {
                                        action = Some(Do::Restart(p.name.clone()));
                                    }
                                } else if ui.add_enabled(on && p.manifest.is_ok(), egui::Button::new("Start")).clicked() {
                                    action = Some(Do::Start(p.name.clone()));
                                }
                                if ui.button("Remove…").clicked() {
                                    action = Some(Do::Remove(p.name.clone()));
                                }
                            });
                            if let Ok(mf) = &p.manifest {
                                ui.add_space(6.0);
                                ui.label(RichText::new("Permissions").strong());
                                if mf.permissions.is_empty() {
                                    ui.label(RichText::new("None: it can add its commands and write to its own log.").small());
                                }
                                for perm in &mf.permissions {
                                    let t = format!("{} ({})", perm.describe(), perm.id());
                                    if perm.unlimited() {
                                        ui.label(RichText::new(format!("{t}: Strata can't limit this")).small().color(ui.visuals().warn_fg_color));
                                    } else {
                                        ui.label(RichText::new(t).small());
                                    }
                                }
                                ui.add(egui::Label::new(RichText::new("Strata can't sandbox a plugin's program: what it does outside Strata isn't limited by these.").small().weak()).wrap());
                                if !mf.commands.is_empty() {
                                    ui.add_space(6.0);
                                    ui.label(RichText::new("Commands").strong());
                                    for c in &mf.commands {
                                        let key = mf.keys.iter().find(|k| k.command == c.id).map(|k| format!("  ({})", crate::keys::label(k.mods, k.key))).unwrap_or_default();
                                        ui.label(RichText::new(shown(&format!("{}: {}{key}", mf.title, c.title))).small());
                                    }
                                }
                                for c in conflicts.iter().filter(|c| c.plugin == p.name) {
                                    ui.label(RichText::new(shown(&format!("Key {} isn't bound: {}", c.key, c.why))).small().color(ui.visuals().warn_fg_color));
                                }
                                if !mf.settings.is_empty() {
                                    ui.add_space(6.0);
                                    ui.label(RichText::new("Settings").strong());
                                    ui.label(RichText::new(format!("Change these in settings.json under \"plugins\": {{\"{}\": {{…}}}}.", mf.name)).small().weak());
                                    for (k, s) in &mf.settings {
                                        let desc = if s.description.is_empty() { String::new() } else { format!(": {}", s.description) };
                                        ui.label(RichText::new(shown(&format!("{k} (default {}){desc}", s.default))).small());
                                    }
                                }
                            }
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Log").strong());
                                if ui.small_button("Copy Log").clicked() {
                                    action = Some(Do::CopyLog(p.name.clone()));
                                }
                            });
                            let lines = manager::log_lines(&p.log);
                            egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(4).inner_margin(6).show(ui, |ui| {
                                egui::ScrollArea::vertical().id_salt("plugin-log").max_height(180.0).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
                                    if lines.is_empty() {
                                        ui.label(RichText::new("Nothing logged yet.").small().weak());
                                    }
                                    for l in lines.iter().rev().take(300).rev() {
                                        ui.label(RichText::new(shown(l)).small().monospace());
                                    }
                                });
                            });
                        });
                    });
                });
            });
        drop(m);
        if !open {
            self.plugins.manager_open = false;
        }
        match action {
            None => {}
            Some(Do::Install(zip)) => self.plugins_command(ctx, if zip { Cmd::PluginsInstallZip } else { Cmd::PluginsInstallFolder }),
            Some(Do::Enable(name, on)) => self.plugins.mgr.borrow_mut().set_enabled(&name, on),
            Some(Do::Start(name)) => self.plugins.mgr.borrow_mut().start(&name),
            Some(Do::Stop(name)) => self.plugins.mgr.borrow_mut().stop(&name),
            Some(Do::Restart(name)) => {
                let mut m = self.plugins.mgr.borrow_mut();
                m.stop(&name);
                m.start(&name);
            }
            Some(Do::Remove(name)) => self.plugins.removing = Some(name),
            Some(Do::CopyLog(name)) => {
                let m = self.plugins.mgr.borrow();
                if let Some(p) = m.plugins.iter().find(|p| p.name == name) {
                    ctx.copy_text(manager::log_lines(&p.log).join("\n"));
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
