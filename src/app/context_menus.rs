//! Right-click menus, Sublime-style: the editor text and gutter, editor tabs,
//! the terminal (text and tabs) and the Markdown preview. Menus draw with the
//! app menus' item renderer; a chosen item is queued and run once the frame
//! is drawn (`run_context_actions`), so the menus never fight over borrows.
//!
//! Clipboard items act on the menu's own target, not on whichever widget has
//! keyboard focus, but through the same paths as the keyboard shortcuts:
//! copies go out as egui copy commands (so Paste from History sees them) and
//! pastes read the system clipboard like Edit ▸ Paste.

use super::StrataApp;
use crate::commands::Cmd;
use crate::ctxmenu;
use crate::keybindings::Target as Bound;
use crate::document::Document;
use crate::keys::*;
use crate::menus::{Act, Item};
use egui::{Id, Key, Rect, Response};
use std::path::PathBuf;

/// What a menu was opened on. Editor tabs are named by view id, so queued
/// actions still find their tab if indices shift.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Target {
    /// Editor text.
    Editor(Id),
    /// Editor gutter, on a line.
    Gutter(Id, usize),
    /// A tab in an editor pane's tab bar.
    Tab(Id),
    Terminal,
    TerminalTab(usize),
    Preview,
}

// Menu-only actions (`Act::Named`).
const UNSAVED: &str = "ctx:unsaved";
const REVEAL: &str = "ctx:reveal";
const COPY_PATH: &str = "ctx:copy_path";
const REVEAL_SIDEBAR: &str = "ctx:reveal_sidebar";
const FOLD_LINE: &str = "ctx:fold_line";
const COPY_LINE: &str = "ctx:copy_line";
const CLOSE: &str = "ctx:close";
const CLOSE_OTHERS: &str = "ctx:close_others";
const CLOSE_RIGHT: &str = "ctx:close_right";
const CLOSE_UNMODIFIED: &str = "ctx:close_unmodified";
const NEW_WINDOW: &str = "ctx:new_window";
const SPLIT_VIEW: &str = "ctx:split_view";
const COMPARE: &str = "ctx:compare";
const TERM_CLEAR: &str = "ctx:term_clear";
const TERM_CLOSE: &str = "ctx:term_close";
const TERM_CLOSE_OTHERS: &str = "ctx:term_close_others";
const MD_COPY: &str = "ctx:md_copy";
const MD_COPY_HTML: &str = "ctx:md_copy_html";

fn named(id: &str) -> Act {
    Act::Named(id.to_owned())
}

/// Dev aid (pair with STRATA_SCREENSHOT): `STRATA_DEV_CONTEXT=editor` right-clicks
/// that area once it's on screen. Also `gutter`, `tab`, `terminal`,
/// `terminal-tab`, `preview`, `find` (with STRATA_DEV_FIND) and `palette`.
struct DevClick {
    what: String,
    frames: u32,
    /// 0: waiting, 1: pressed, 2: done.
    stage: u8,
    at: egui::Pos2,
}

#[derive(Default)]
pub(super) struct ContextMenus {
    pending: Vec<(Target, Act)>,
    /// Where each menu can be opened this frame (for the dev aid).
    rects: Vec<(&'static str, Rect)>,
    dev: Option<DevClick>,
}

impl ContextMenus {
    pub fn new() -> Self {
        let dev = crate::dev::var("STRATA_DEV_CONTEXT").map(|what| DevClick { what, frames: 0, stage: 0, at: egui::Pos2::ZERO });
        Self { dev, ..Default::default() }
    }

    fn note(&mut self, what: &'static str, rect: Rect) {
        if self.dev.is_some() {
            self.rects.retain(|(w, _)| *w != what);
            self.rects.push((what, rect));
        }
    }

    fn queue(&mut self, target: Target, act: Option<Act>) {
        if let Some(act) = act {
            self.pending.push((target, act));
        }
    }
}

impl StrataApp {
    fn doc_index(&self, id: Id) -> Option<usize> {
        self.docs.iter().position(|d| d.view.id == id)
    }

    /// Reveal in Finder, Copy File Path, Reveal in Side Bar.
    fn file_items(&self, path: Option<&PathBuf>) -> Vec<Item> {
        let in_tree = path.is_some_and(|p| self.sidebar.root_of(p).is_some());
        vec![
            Item::new(ctxmenu::reveal_label(), named(REVEAL)).enabled(path.is_some_and(|p| p.exists())),
            Item::new("Copy File Path", named(COPY_PATH)).enabled(path.is_some()),
            Item::new("Reveal in Side Bar", named(REVEAL_SIDEBAR)).enabled(in_tree),
        ]
    }

    // ---- editor --------------------------------------------------------------------

    /// The editor pane showing tab `i` drew `resp`: its right-click menu.
    pub(super) fn editor_context_menu(&mut self, resp: &Response, i: usize) {
        if resp.secondary_clicked() && i != self.active {
            self.activate(i);
        }
        let id = self.docs[i].view.id;
        let gutter = self.docs[i].view.context_line;
        if i == self.active {
            self.context.note("editor", resp.rect);
        }
        let act = ctxmenu::popup(resp, false, || match gutter {
            Some(line) => self.gutter_items(i, line),
            None => self.editor_items(i),
        });
        let target = gutter.map_or(Target::Editor(id), |line| Target::Gutter(id, line));
        self.context.queue(target, act);
    }

    fn editor_items(&self, i: usize) -> Vec<Item> {
        let d = &self.docs[i];
        let writable = !d.editor.read_only;
        // Spelling suggestions for a misspelled word under the cursor come first.
        let mut items = self.spelling_items(i);
        items.extend([Item::new("Show Unsaved Changes…", named(UNSAVED)).enabled(d.path.is_some() && d.is_dirty()), Item::Separator]);
        // With nothing selected, Cut and Copy take the caret's line (like Sublime).
        items.extend(ctxmenu::clipboard_items(writable, true, writable));
        items.push(Item::Separator);
        items.extend(self.file_items(d.path.as_ref()));
        items.extend(self.file_link_items(i));
        items.push(Item::Separator);
        let symbol = i == self.active && self.word_at_cursor().is_some();
        items.push(Item::cmd("Goto Definition", Cmd::GotoDefinition).enabled(symbol));
        let comments = crate::lang::comment_tokens(d.highlighter.syntax_name());
        let can_comment = writable && (comments.0.is_some() || comments.1.is_some());
        items.push(Item::new("Toggle Comment", Act::Editor("toggle_comment")).bound(Bound::Editor("toggle_comment")).enabled(can_comment));
        let line = d.editor.buffer.line_of(d.editor.primary().head);
        items.push(if d.view.is_fold_header(line) {
            Item::new("Unfold", Act::Named("unfold".into())).bound(Bound::View("unfold"))
        } else {
            let foldable = crate::fold::has_region(&d.editor.buffer, line, d.editor.tab_width) || d.editor.selections().iter().any(|s| {
                let b = &d.editor.buffer;
                b.line_of(s.start()) != b.line_of(s.end())
            });
            Item::new("Fold", Act::Named("fold".into())).bound(Bound::View("fold")).enabled(foldable)
        });
        items
    }

    fn gutter_items(&self, i: usize, line: usize) -> Vec<Item> {
        let d = &self.docs[i];
        let bookmarked = d.editor.bookmarks().iter().any(|&b| d.editor.buffer.line_of(b) == line);
        let fold = if d.view.is_fold_header(line) {
            Item::new("Unfold", named(FOLD_LINE))
        } else {
            Item::new("Fold", named(FOLD_LINE)).enabled(crate::fold::has_region(&d.editor.buffer, line, d.editor.tab_width))
        };
        vec![
            Item::new(if bookmarked { "Remove Bookmark" } else { "Add Bookmark" }, Act::Editor("bookmark_toggle")).bound(Bound::Editor("bookmark_toggle")),
            fold,
            Item::Separator,
            Item::new(format!("Copy Line {}", line + 1), named(COPY_LINE)),
        ]
    }

    fn run_editor_act(&mut self, ctx: &egui::Context, i: usize, line: Option<usize>, act: Act) {
        if i != self.active {
            self.activate(i);
        }
        let d = &mut self.docs[i];
        match &act {
            Act::Copy => ctx.copy_text(d.editor.copy()),
            Act::Cut => ctx.copy_text(d.editor.cut()),
            Act::Paste => {
                if let Some(text) = ctxmenu::clipboard_text() {
                    d.editor.paste(&text);
                }
            }
            Act::SelectAll => d.editor.select_all(),
            Act::Named(n) if n == FOLD_LINE => {
                if let Some(line) = line {
                    d.view.toggle_fold_at(&mut d.editor, line);
                }
            }
            Act::Named(n) if n == COPY_LINE => {
                if let Some(line) = line {
                    let ending = d.editor.buffer.line_ending().as_str();
                    ctx.copy_text(d.editor.buffer.line_text(line) + ending);
                }
            }
            Act::Named(n) if n == UNSAVED => return self.show_unsaved_changes(i),
            _ => return self.run_doc_act(ctx, i, act),
        }
        d.view.reveal();
        d.view.focus();
    }

    /// File and command items shared by the editor and tab menus.
    fn run_doc_act(&mut self, ctx: &egui::Context, i: usize, act: Act) {
        let path = self.docs[i].path.clone();
        match act {
            Act::Named(n) if n == REVEAL => {
                if let Some(p) = path
                    && let Err(e) = ctxmenu::reveal_in_file_manager(&p)
                {
                    self.status = format!("Could not open the file manager: {e}");
                }
            }
            Act::Named(n) if n == COPY_PATH => {
                if let Some(p) = path {
                    ctx.copy_text(p.display().to_string());
                    self.status = format!("Copied {}", p.display());
                }
            }
            Act::Named(n) if n == super::file_links::COPY_RELATIVE => self.copy_relative_path(ctx, i),
            Act::Named(n) if n == super::file_links::COPY_PERMALINK => self.copy_permalink(ctx, i),
            Act::Named(n) if n == REVEAL_SIDEBAR => {
                if let Some(p) = path
                    && !self.sidebar.reveal(&p)
                {
                    self.status = "The file isn't in the open folder".into();
                }
            }
            act => self.run_act(ctx, act),
        }
    }

    /// Show Unsaved Changes: a read-only tab with a unified diff of the
    /// tab's text against its file on disk.
    pub(super) fn show_unsaved_changes(&mut self, i: usize) {
        let d = &mut self.docs[i];
        let Some(path) = d.path.clone() else { return };
        let disk = match std::fs::read(&path) {
            Ok(bytes) => {
                let b = crate::buffer::Buffer::decode_as(&bytes, d.encoding);
                b.slice(0..b.len_chars())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                self.status = format!("Could not read {}: {e}", path.display());
                return;
            }
        };
        let name = d.name();
        let here = d.editor.text_snapshot();
        let Some(diff) = crate::git::unified_diff(&disk, &here, &format!("{} (on disk)", path.display()), &format!("{} (unsaved)", path.display())) else {
            self.status = format!("{name} has no unsaved changes");
            return;
        };
        let mut doc = Document::from_buffer(None, crate::buffer::Buffer::from_str(&format!("Unsaved changes: {name}\n{diff}")));
        doc.highlighter.set_syntax("Diff");
        doc.editor.read_only = true;
        self.push_doc(doc);
    }

    // ---- editor tabs ---------------------------------------------------------------

    /// Right-click menu for tab `i` in a pane's tab bar.
    pub(super) fn tab_context_menu(&mut self, resp: &Response, i: usize) {
        if i == self.active {
            self.context.note("tab", resp.rect);
        }
        let act = ctxmenu::popup(resp, false, || self.tab_items(i));
        self.context.queue(Target::Tab(self.docs[i].view.id), act);
    }

    fn tab_items(&self, i: usize) -> Vec<Item> {
        let tabs = self.group_tabs(self.docs[i].group);
        let k = tabs.iter().position(|&t| t == i).unwrap_or(0);
        let mut items = vec![
            Item::new("Close", named(CLOSE)).bound(Bound::Cmd(Cmd::CloseTab)),
            Item::new("Close Other Tabs", named(CLOSE_OTHERS)).enabled(tabs.len() > 1),
            Item::new("Close Tabs to the Right", named(CLOSE_RIGHT)).enabled(k + 1 < tabs.len()),
            Item::new("Close Unmodified Tabs", named(CLOSE_UNMODIFIED)).enabled(tabs.iter().any(|&t| !self.docs[t].is_dirty())),
            Item::Separator,
            Item::new("Move to New Window", named(NEW_WINDOW)).enabled(self.docs.len() > 1),
            Item::new("Split View", named(SPLIT_VIEW)).enabled(self.docs[i].compare.is_none()),
            Item::new("Compare with…", named(COMPARE)).bound(Bound::Cmd(Cmd::CompareWith)),
            Item::Separator,
            Item::cmd("New File", Cmd::NewTab),
            Item::cmd("Open File…", Cmd::Open),
            Item::Separator,
        ];
        let path = self.docs[i].path.as_ref();
        let mut files = self.file_items(path);
        // Copy File Path first, as in Sublime's tab menu.
        files.rotate_right(2);
        items.extend(files);
        items.extend(self.file_link_items(i));
        items
    }

    /// Close the tabs with these view ids, asking about unsaved ones; stops if one is kept.
    fn close_tabs(&mut self, ids: Vec<Id>) {
        for id in ids {
            let Some(i) = self.doc_index(id) else { continue };
            if !self.close_tab(i) {
                break;
            }
        }
    }

    fn run_tab_act(&mut self, ctx: &egui::Context, i: usize, act: Act) {
        let group = self.docs[i].group;
        let tabs = self.group_tabs(group);
        let ids = |sel: &dyn Fn(usize, usize) -> bool| -> Vec<Id> { tabs.iter().enumerate().filter(|&(k, &t)| sel(k, t)).map(|(_, &t)| self.docs[t].view.id).collect() };
        let k = tabs.iter().position(|&t| t == i).unwrap_or(0);
        let me = self.docs[i].view.id;
        match &act {
            Act::Named(n) if n == CLOSE => {
                self.close_tab(i);
            }
            Act::Named(n) if n == CLOSE_OTHERS => {
                let others = ids(&|_, t| t != i);
                self.close_tabs(others);
                if let Some(i) = self.doc_index(me) {
                    self.activate(i);
                }
            }
            Act::Named(n) if n == CLOSE_RIGHT => {
                let right = ids(&|j, _| j > k);
                self.close_tabs(right);
                if let Some(i) = self.doc_index(me) {
                    self.activate(i);
                }
            }
            Act::Named(n) if n == CLOSE_UNMODIFIED => {
                let clean = ids(&|_, t| !self.docs[t].is_dirty());
                self.close_tabs(clean);
            }
            Act::Named(n) if n == NEW_WINDOW => self.link.requests.move_tab = Some(me),
            Act::Named(n) if n == SPLIT_VIEW => self.split_view(i),
            Act::Named(n) if n == COMPARE => self.compare_with(ctx, i),
            Act::Cmd(Cmd::NewTab | Cmd::Open) => {
                // New tabs open in the pane whose tab bar was clicked.
                self.focus_group = group;
                self.run_act(ctx, act);
            }
            _ => self.run_doc_act(ctx, i, act),
        }
    }

    // ---- terminal ------------------------------------------------------------------

    /// Menus for the terminal's text area and its tabs (after the panel is drawn).
    pub(super) fn terminal_context_menus(&mut self) {
        if let Some(body) = self.terminal.body.clone() {
            self.context.note("terminal", body.rect);
            let act = ctxmenu::popup(&body, false, || self.terminal_items());
            self.context.queue(Target::Terminal, act);
        }
        for (i, label) in self.terminal.tab_labels.clone() {
            if i == 0 {
                self.context.note("terminal-tab", label.rect);
            }
            let n = self.terminal.tabs.len();
            let act = ctxmenu::popup(&label, false, || {
                vec![
                    Item::new("Close Terminal", named(TERM_CLOSE)),
                    Item::new("Close Other Terminals", named(TERM_CLOSE_OTHERS)).enabled(n > 1),
                    Item::Separator,
                    Item::cmd("New Terminal", Cmd::NewTerminal),
                ]
            });
            self.context.queue(Target::TerminalTab(i), act);
        }
    }

    fn terminal_items(&self) -> Vec<Item> {
        let clear = Item::new("Clear", named(TERM_CLEAR));
        vec![
            Item::new("Copy", Act::Copy).key(CMD, Key::C).enabled(self.terminal.has_selection()),
            Item::new("Paste", Act::Paste).key(CMD, Key::V),
            Item::new("Select All", Act::SelectAll),
            Item::Separator,
            // ⌘K clears on macOS; elsewhere Ctrl+K belongs to the shell.
            if MAC { clear.key(CMD, Key::K) } else { clear },
            Item::Separator,
            Item::cmd("New Terminal", Cmd::NewTerminal),
            Item::new("Close Terminal", named(TERM_CLOSE)),
            Item::new("Hide Panel", Act::Cmd(Cmd::ToggleTerminal)).bound(Bound::Cmd(Cmd::ToggleTerminal)),
        ]
    }

    fn run_terminal_act(&mut self, ctx: &egui::Context, tab: Option<usize>, act: Act) {
        let t = &mut self.terminal;
        match &act {
            Act::Copy => {
                t.copy(ctx);
            }
            Act::Paste => {
                if let Some(text) = ctxmenu::clipboard_text() {
                    t.paste(&text);
                }
            }
            Act::SelectAll => t.select_all(),
            Act::Named(n) if n == TERM_CLEAR => t.clear(),
            Act::Named(n) if n == TERM_CLOSE => {
                if t.close(tab.unwrap_or(t.active)) {
                    // The last one: hide the panel, as when its shell exits.
                    t.visible = true;
                    self.run(ctx, Cmd::ToggleTerminal);
                }
                return;
            }
            Act::Named(n) if n == TERM_CLOSE_OTHERS => {
                let keep = tab.unwrap_or(t.active);
                for j in (0..t.tabs.len()).rev().filter(|&j| j != keep) {
                    t.close(j);
                }
                t.active = 0;
            }
            Act::Cmd(c) => return self.run(ctx, *c),
            _ => {}
        }
        self.terminal.focus();
    }

    // ---- Markdown preview ------------------------------------------------------------

    /// The preview panel drew `resp` (its whole area). Its links and tables take
    /// clicks themselves, so a right-click anywhere in it opens the menu.
    pub(super) fn preview_context_menu(&mut self, resp: &Response) {
        self.context.note("preview", resp.rect);
        let open = ctxmenu::secondary_click_in(&resp.ctx, resp.rect, resp.layer_id);
        let d = &self.docs[self.active];
        let md = crate::markdown::is_markdown(d.path.as_deref(), d.highlighter.syntax_name());
        let act = ctxmenu::popup(resp, open, || {
            vec![
                Item::new("Copy Markdown", named(MD_COPY)).enabled(md),
                Item::new("Copy as HTML", named(MD_COPY_HTML)).enabled(md),
                Item::Separator,
                Item::cmd("Export as HTML…", Cmd::ExportMarkdownHtml).enabled(md),
                Item::Separator,
                Item::cmd("Hide Preview", Cmd::ToggleMarkdownPreview),
            ]
        });
        self.context.queue(Target::Preview, act);
    }

    fn run_preview_act(&mut self, ctx: &egui::Context, act: Act) {
        let src = self.docs[self.active].editor.text_snapshot();
        match act {
            Act::Named(n) if n == MD_COPY => ctx.copy_text(src.to_string()),
            Act::Named(n) if n == MD_COPY_HTML => {
                let html = crate::markdown::to_html_fragment(&src);
                self.status = match arboard::Clipboard::new().and_then(|mut c| c.set_html(&html, Some(src.as_ref()))) {
                    Ok(()) => "Copied as HTML".into(),
                    Err(e) => format!("Couldn't copy: {e}"),
                };
            }
            act => self.run_act(ctx, act),
        }
    }

    // ---- dispatch ------------------------------------------------------------------

    /// Run what was chosen in a context menu this frame.
    pub(super) fn run_context_actions(&mut self, ctx: &egui::Context) {
        for (target, act) in std::mem::take(&mut self.context.pending) {
            match target {
                Target::Editor(id) | Target::Gutter(id, _) => {
                    let line = if let Target::Gutter(_, l) = target { Some(l) } else { None };
                    if let Some(i) = self.doc_index(id) {
                        self.run_editor_act(ctx, i, line, act);
                    }
                }
                Target::Tab(id) => {
                    if let Some(i) = self.doc_index(id) {
                        self.run_tab_act(ctx, i, act);
                    }
                }
                Target::Terminal => self.run_terminal_act(ctx, None, act),
                Target::TerminalTab(i) if i < self.terminal.tabs.len() => self.run_terminal_act(ctx, Some(i), act),
                Target::TerminalTab(_) => {}
                Target::Preview => self.run_preview_act(ctx, act),
            }
        }
    }

    /// Dev aid: synthesize the right-click asked for in STRATA_DEV_CONTEXT.
    pub(super) fn dev_context_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let Some(dev) = &mut self.context.dev else { return };
        dev.frames += 1;
        ctx.request_repaint();
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE };
        match dev.stage {
            0 => {
                // A shell needs a moment to print its prompt.
                let wait = if dev.what.starts_with("terminal") { 1.5 } else { 0.0 };
                if dev.frames < 4 || raw.time.unwrap_or(0.0) < wait {
                    return;
                }
                let rect = match dev.what.as_str() {
                    "gutter" => self.context.rects.iter().find(|(w, _)| *w == "editor").map(|(_, r)| *r),
                    "find" => ctx.read_response(Id::new("find-query")).map(|r| r.rect),
                    "palette" => ctx.read_response(Id::new("palette-query")).map(|r| r.rect),
                    what => self.context.rects.iter().find(|(w, _)| *w == what).map(|(_, r)| *r),
                };
                let Some(rect) = rect else { return };
                dev.at = match dev.what.as_str() {
                    "editor" => rect.left_top() + egui::vec2(150.0, 30.0),
                    "gutter" => rect.left_top() + egui::vec2(14.0, 30.0),
                    _ => rect.center(),
                };
                raw.events.push(egui::Event::PointerMoved(dev.at));
                raw.events.push(button(dev.at, true));
                dev.stage = 1;
            }
            1 => {
                raw.events.push(button(dev.at, false));
                dev.stage = 2;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::Editor;
    use egui::accesskit;
    use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, pos2, vec2};

    /// Run one frame; returns the accessibility tree's labelled nodes: (label, rect, enabled).
    fn frame(app: &mut StrataApp, ctx: &Context, events: Vec<Event>) -> Vec<(String, Rect, bool)> {
        ctx.enable_accesskit();
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            let mut frame = eframe::Frame::_new_kittest();
            eframe::App::ui(app, ui, &mut frame);
        });
        out.textures_delta.clear();
        for c in &out.platform_output.commands {
            if let egui::OutputCommand::CopyText(t) = c {
                COPIED.with(|c| c.borrow_mut().push(t.clone()));
            }
        }
        let Some(update) = out.platform_output.accesskit_update else { return Vec::new() };
        update
            .nodes
            .iter()
            .filter_map(|(_, n)| {
                let label = n.label()?.to_owned();
                let b: accesskit::Rect = n.bounds()?;
                Some((label, Rect::from_min_max(pos2(b.x0 as f32, b.y0 as f32), pos2(b.x1 as f32, b.y1 as f32)), !n.is_disabled()))
            })
            .collect()
    }

    fn click(button: PointerButton, at: Pos2) -> [Vec<Event>; 2] {
        let b = |pressed| Event::PointerButton { pos: at, button, pressed, modifiers: Modifiers::NONE };
        [vec![Event::PointerMoved(at), b(true)], vec![b(false)]]
    }

    /// Right-click at `at` and return the open menu's items.
    fn open_menu(app: &mut StrataApp, ctx: &Context, at: Pos2) -> Vec<(String, Rect, bool)> {
        for events in click(PointerButton::Secondary, at) {
            frame(app, ctx, events);
        }
        frame(app, ctx, vec![]);
        frame(app, ctx, vec![])
    }

    fn item<'a>(nodes: &'a [(String, Rect, bool)], label: &str) -> &'a (String, Rect, bool) {
        // Labels read "Copy ⌘ C": the row's text, then its shortcut. "Copy File
        // Path" also starts with "Copy ", so the shortest match wins.
        nodes
            .iter()
            .filter(|(l, ..)| l.trim_end() == label || l.starts_with(&format!("{label} ")))
            .min_by_key(|(l, ..)| l.len())
            .unwrap_or_else(|| panic!("no {label:?} in {:?}", nodes.iter().map(|n| &n.0).collect::<Vec<_>>()))
    }

    /// Click the menu item `label` (it must be enabled) and finish the frame.
    fn choose(app: &mut StrataApp, ctx: &Context, nodes: &[(String, Rect, bool)], label: &str) {
        let (_, rect, enabled) = item(nodes, label);
        assert!(enabled, "{label} is disabled");
        for events in click(PointerButton::Primary, rect.center()) {
            frame(app, ctx, events);
        }
        frame(app, ctx, vec![]);
    }

    thread_local! {
        /// Text the frames run on this thread put on the clipboard.
        static COPIED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// The last text copied in a frame on this thread (the context is unused,
    /// it only marks which app's frames are meant).
    fn copied(_ctx: &Context) -> Option<String> {
        COPIED.with(|c| c.borrow().last().cloned())
    }

    fn app_with(text: &str) -> StrataApp {
        let mut app = StrataApp::new(None, None);
        app.docs[0].editor = Editor::new(Buffer::from_str(text));
        app
    }

    fn text(app: &StrataApp) -> String {
        let e = &app.docs[app.active].editor;
        e.buffer.slice(0..e.buffer.len_chars())
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-ctx-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A point on line `line` (0-based), well inside the text.
    fn text_point(app: &StrataApp, ctx: &Context, line: usize) -> Pos2 {
        let r = ctx.read_response(app.docs[app.active].view.id).unwrap().rect;
        let row = app.settings.font_size * 1.25;
        pos2(r.left() + 120.0, r.top() + row * (line as f32 + 0.5))
    }

    #[test]
    fn editor_menu_keeps_selection_copies_and_cuts() {
        let ctx = Context::default();
        let mut app = app_with("alpha beta gamma\nsecond line\n");
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        app.doc().editor.select_ranges(&[0..16], 0);

        // Inside the selection: it stays, and Copy copies it.
        let at = text_point(&app, &ctx, 0);
        let menu = open_menu(&mut app, &ctx, at);
        assert_eq!(app.doc().editor.primary().range(), 0..16, "right-click inside a selection keeps it");
        for label in ["Cut", "Copy", "Paste", "Select All", "Goto Definition", "Toggle Comment", "Copy File Path"] {
            item(&menu, label);
        }
        assert!(!item(&menu, "Show Unsaved Changes…").2, "untitled tabs have no file to compare");
        assert!(!item(&menu, "Copy File Path").2);
        choose(&mut app, &ctx, &menu, "Copy");
        assert_eq!(copied(&ctx).as_deref(), Some("alpha beta gamma"));
        assert_eq!(app.clip_history.first().map(String::as_str), Some("alpha beta gamma"), "menu copies reach Paste from History");

        // Outside it: the caret moves to the click first.
        let at = text_point(&app, &ctx, 1);
        let menu = open_menu(&mut app, &ctx, at);
        let p = app.doc().editor.primary();
        assert!(p.is_empty() && app.doc().editor.buffer.line_of(p.head) == 1, "caret moved to line 2");
        choose(&mut app, &ctx, &menu, "Select All");
        assert_eq!(app.doc().editor.primary().range(), 0..29);

        // Cut acts on the menu's own editor even when focus is elsewhere.
        app.doc().editor.select_ranges(&[0..16], 0);
        app.doc().view.cancel_focus();
        app.open_find(false);
        frame(&mut app, &ctx, vec![]);
        let at = text_point(&app, &ctx, 0);
        let menu = open_menu(&mut app, &ctx, at);
        choose(&mut app, &ctx, &menu, "Cut");
        assert_eq!(text(&app), "\nsecond line\n");
        assert_eq!(copied(&ctx).as_deref(), Some("alpha beta gamma"));
    }

    #[test]
    fn gutter_menu_bookmarks_and_copies_its_line() {
        let ctx = Context::default();
        let mut app = app_with("fn a() {\n    1\n}\nlast\n");
        app.doc().highlighter.set_syntax("Rust");
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let r = ctx.read_response(app.doc().view.id).unwrap().rect;
        let row = app.settings.font_size * 1.25;
        let menu = open_menu(&mut app, &ctx, pos2(r.left() + 8.0, r.top() + row * 3.5));
        assert_eq!(app.doc().view.context_line, Some(3));
        choose(&mut app, &ctx, &menu, "Copy Line 4");
        assert_eq!(copied(&ctx).as_deref(), Some("last\n"));
        let menu = open_menu(&mut app, &ctx, pos2(r.left() + 8.0, r.top() + row * 0.5));
        assert!(item(&menu, "Fold").2, "line 1 starts a block");
        choose(&mut app, &ctx, &menu, "Add Bookmark");
        assert_eq!(app.doc().editor.bookmarks().len(), 1);
        let menu = open_menu(&mut app, &ctx, pos2(r.left() + 8.0, r.top() + row * 0.5));
        choose(&mut app, &ctx, &menu, "Fold");
        assert!(app.doc().view.is_fold_header(0));
        item(&open_menu(&mut app, &ctx, pos2(r.left() + 8.0, r.top() + row * 0.5)), "Remove Bookmark");
    }

    #[test]
    fn unsaved_changes_open_a_read_only_diff() {
        let dir = temp_dir("unsaved");
        let file = dir.join("notes.txt");
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(file.clone()), None);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let at = text_point(&app, &ctx, 0);
        let menu = open_menu(&mut app, &ctx, at);
        assert!(!item(&menu, "Show Unsaved Changes…").2, "a clean file has nothing to show");
        assert!(item(&menu, "Copy File Path").2);
        frame(&mut app, &ctx, vec![Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        assert!(!egui::Popup::is_any_open(&ctx), "Escape closes the menu");
        frame(&mut app, &ctx, vec![]);

        app.doc().editor.select_ranges(&[4..7], 0);
        app.doc().editor.insert_text("2");
        let at = text_point(&app, &ctx, 0);
        let menu = open_menu(&mut app, &ctx, at);
        choose(&mut app, &ctx, &menu, "Show Unsaved Changes…");
        assert_eq!(app.docs.len(), 2);
        let shown = text(&app);
        assert!(shown.starts_with("Unsaved changes: notes.txt\n"), "{shown}");
        assert!(shown.contains("@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n"), "{shown}");
        assert!(!app.doc().is_dirty(), "closes without asking");
        app.doc().editor.insert_text("typing");
        assert_eq!(text(&app), shown, "read-only");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn tab_menu_closes_others_and_to_the_right() {
        let dir = temp_dir("tabs");
        let ctx = Context::default();
        let mut app = StrataApp::new(None, None);
        for f in ["a.rs", "b.rs", "c.rs", "d.rs"] {
            std::fs::write(dir.join(f), f).unwrap();
            app.open_path(dir.join(f), false);
        }
        app.activate(1);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        // Tabs are 90+ px wide from x = 0; b.rs is the second.
        let names: Vec<String> = app.docs.iter().map(|d| d.name()).collect();
        assert_eq!(names, ["a.rs", "b.rs", "c.rs", "d.rs"]);
        let menu = open_menu(&mut app, &ctx, pos2(140.0, 44.0));
        assert!(item(&menu, "Close Tabs to the Right").2);
        assert!(item(&menu, "Copy File Path").2);
        choose(&mut app, &ctx, &menu, "Close Tabs to the Right");
        let names: Vec<String> = app.docs.iter().map(|d| d.name()).collect();
        assert_eq!(names, ["a.rs", "b.rs"]);
        let menu = open_menu(&mut app, &ctx, pos2(140.0, 44.0));
        choose(&mut app, &ctx, &menu, "Close Other Tabs");
        assert_eq!(app.docs.len(), 1);
        assert_eq!(app.doc().name(), "b.rs");
        let menu = open_menu(&mut app, &ctx, pos2(40.0, 44.0));
        assert!(!item(&menu, "Close Other Tabs").2);
        choose(&mut app, &ctx, &menu, "Close Unmodified Tabs");
        assert!(app.docs.len() == 1 && app.docs[0].is_blank(), "a fresh untitled tab replaces the last one");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn preview_menu_copies_source_and_hides() {
        let dir = temp_dir("preview");
        let file = dir.join("readme.md");
        std::fs::write(&file, "# Title\n\nSee [a link](https://example.com) here.\n").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(file), None);
        app.settings.markdown_preview = true;
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![]);
        }
        let menu = open_menu(&mut app, &ctx, pos2(900.0, 300.0));
        assert!(item(&menu, "Export as HTML…").2);
        choose(&mut app, &ctx, &menu, "Copy Markdown");
        assert_eq!(copied(&ctx).as_deref(), Some("# Title\n\nSee [a link](https://example.com) here.\n"));
        let menu = open_menu(&mut app, &ctx, pos2(900.0, 300.0));
        choose(&mut app, &ctx, &menu, "Hide Preview");
        assert!(!app.settings.markdown_preview);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn find_field_menu_selects_all_and_copies() {
        let ctx = Context::default();
        let mut app = app_with("text");
        app.open_find(false);
        app.find.query.text = "needle".into();
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let field = ctx.read_response(Id::new("find-query")).unwrap().rect;
        let menu = open_menu(&mut app, &ctx, field.center());
        assert!(!item(&menu, "Copy").2, "nothing selected yet");
        choose(&mut app, &ctx, &menu, "Select All");
        let menu = open_menu(&mut app, &ctx, field.center());
        assert!(item(&menu, "Copy").2, "right-click kept the selection");
        choose(&mut app, &ctx, &menu, "Copy");
        assert_eq!(copied(&ctx).as_deref(), Some("needle"));
        let menu = open_menu(&mut app, &ctx, field.center());
        choose(&mut app, &ctx, &menu, "Cut");
        assert_eq!(app.find.query.text, "");
    }

    #[test]
    fn palette_field_menu_works_without_closing_the_palette() {
        let ctx = Context::default();
        let mut app = app_with("text");
        frame(&mut app, &ctx, vec![]);
        app.open_palette(&ctx, crate::palette::Mode::Commands, "toggle");
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let field = ctx.read_response(Id::new("palette-query")).unwrap().rect;
        let menu = open_menu(&mut app, &ctx, field.center());
        choose(&mut app, &ctx, &menu, "Select All");
        assert!(app.palette.open, "using the field's menu keeps the palette open");
        let menu = open_menu(&mut app, &ctx, field.center());
        choose(&mut app, &ctx, &menu, "Copy");
        assert_eq!(copied(&ctx).as_deref(), Some("toggle"));
        assert!(app.palette.open);
    }

    #[cfg(unix)]
    #[test]
    fn terminal_menu_selects_copies_and_closes() {
        let ctx = Context::default();
        let mut app = StrataApp::new(None, None);
        frame(&mut app, &ctx, vec![]);
        app.run(&ctx, Cmd::ToggleTerminal);
        app.run(&ctx, Cmd::NewTerminal);
        assert_eq!(app.terminal.tabs.len(), 2);
        app.terminal.active_terminal().unwrap().write("echo strata-ctx-$((6*7))\r");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline && !app.terminal.active_terminal().unwrap().screen_text().contains("strata-ctx-42") {
            frame(&mut app, &ctx, vec![]);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        frame(&mut app, &ctx, vec![]);
        let body = app.terminal.body.as_ref().unwrap().rect;
        let menu = open_menu(&mut app, &ctx, body.center());
        assert!(!item(&menu, "Copy").2, "nothing selected");
        assert!(!app.terminal.has_selection(), "the right-click didn't start a selection");
        choose(&mut app, &ctx, &menu, "Select All");
        assert!(app.terminal.has_selection());
        let menu = open_menu(&mut app, &ctx, body.center());
        assert!(app.terminal.has_selection(), "right-click keeps the selection");
        choose(&mut app, &ctx, &menu, "Copy");
        assert!(copied(&ctx).is_some_and(|t| t.contains("strata-ctx-42")));

        // Tab menu: close the other terminal; then Close Terminal hides the panel.
        let label = app.terminal.tab_labels[1].1.rect;
        let menu = open_menu(&mut app, &ctx, label.center());
        choose(&mut app, &ctx, &menu, "Close Other Terminals");
        assert_eq!(app.terminal.tabs.len(), 1);
        let body = app.terminal.body.as_ref().unwrap().rect;
        let menu = open_menu(&mut app, &ctx, body.center());
        choose(&mut app, &ctx, &menu, "Close Terminal");
        assert!(app.terminal.tabs.is_empty() && !app.terminal.visible);
    }
}
