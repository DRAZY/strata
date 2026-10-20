//! The window side of running Markdown code blocks (see `crate::runblock`):
//! the preview's Run button and the Notes: Run Code Block commands, the
//! confirmation that shows exactly what runs where, sending to the terminal
//! (through Send to Terminal's queue) and putting captured output under the
//! block.
//!
//! A block only runs on a click or a command, never when a file opens or the
//! preview draws, and never from an AI answer: those panels draw no Run
//! buttons and aren't documents the commands can reach.

use super::StrataApp;
use crate::commands::Cmd;
use crate::runblock::{self, Allowed, CodeBlock, FileKey, Lang, Runs};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

/// Where a block's text goes. Tabs are named by `Terminal::id`, so what the
/// confirmation showed is where it lands, or nowhere.
#[derive(Clone, Debug, PartialEq)]
enum Dest {
    /// A shell tab that's already open.
    Terminal { id: u64, title: String },
    /// No shell tab: a new one.
    NewShell,
    /// The REPL tab Run started before for this language.
    Repl { id: u64, program: String },
    NewRepl { program: String },
    /// A hidden process of its own, whose output goes under the block.
    Capture { program: PathBuf, dir: PathBuf },
}

impl Dest {
    fn describe(&self, timeout: u32) -> String {
        match self {
            Dest::Terminal { title, .. } => format!("The terminal tab “{title}”: typed in, then Enter."),
            Dest::NewShell => "A new terminal tab with your shell: typed in, then Enter.".into(),
            Dest::Repl { program, .. } => format!("The {program} tab in the terminal: typed in, then Enter."),
            Dest::NewRepl { program } => format!("A new terminal tab running {program}: typed in, then Enter."),
            Dest::Capture { program, dir } => format!(
                "A separate process, not your terminal: {} in {}, with no input, stopped after {timeout} s. What it prints replaces the output block under the code (Undo takes it back).",
                program.display(),
                dir.display()
            ),
        }
    }
}

/// A run waiting for its confirmation.
struct Request {
    /// The tab's editor (`Editor::id`).
    doc: u64,
    path: Option<PathBuf>,
    block: CodeBlock,
    lang: Lang,
    dest: Dest,
    /// The document was read as a note (its blocks are found the same way again).
    notes: bool,
}

/// The confirmation dialog's state.
struct Ask {
    req: Request,
    /// The file, when its answer can be remembered (it's on disk, in a folder
    /// open in the side bar).
    key: Option<FileKey>,
    dont_ask: bool,
    hidden: Vec<char>,
}

/// A capture running on its own thread.
struct Running {
    doc: u64,
    block: CodeBlock,
    notes: bool,
    /// The block's text in the editor (`Editor::pin`), from its opening
    /// fence to the end of its closing one, and that text as it was.
    pin: Option<(u64, String)>,
    rx: Receiver<Result<runblock::Captured, String>>,
}

#[derive(Default)]
pub(super) struct CodeRunState {
    allowed: Option<Allowed>,
    asking: Option<Ask>,
    running: Option<Running>,
    /// Remembered keys of files Strata is saving (see `code_run_before_save`).
    saving: Option<FileKey>,
}

impl CodeRunState {
    /// The confirmation is up (it keeps the keyboard).
    pub(super) fn asking(&self) -> bool {
        self.asking.is_some()
    }
}

impl StrataApp {
    fn allowed(&mut self) -> &mut Allowed {
        let config = self.config.clone();
        self.code_run.allowed.get_or_insert_with(|| Allowed::new(config.as_deref()))
    }

    pub(super) fn code_run_command(&mut self, ctx: &egui::Context, cmd: Cmd) {
        match cmd {
            Cmd::NotesForgetCodeBlockFiles => {
                self.status = match self.allowed().forget_all() {
                    Ok(0) => "No files were allowed to run code blocks without asking".into(),
                    Ok(n) => format!("Forgot {n} file{}: running their code blocks asks again", if n == 1 { "" } else { "s" }),
                    Err(e) => e,
                };
            }
            _ => self.run_block_at_cursor(ctx, cmd == Cmd::NotesRunCodeBlockCapture),
        }
    }

    /// Notes: Run Code Block (and Capture Output): the block the cursor is in.
    fn run_block_at_cursor(&mut self, ctx: &egui::Context, capture: bool) {
        let i = self.active;
        let notes = self.note_root(i).is_some();
        let d = &mut self.docs[i];
        if d.binary.is_some() || !crate::markdown::is_markdown(d.path.as_deref(), d.highlighter.syntax_name()) {
            self.status = "Run Code Block works in Markdown files".into();
            return;
        }
        let text = d.editor.text_snapshot();
        let pos = d.editor.buffer.char_to_byte(d.editor.primary().head);
        let blocks = runblock::code_blocks(&text, notes);
        let Some(b) = runblock::block_at(&text, &blocks, pos) else {
            self.status = "Put the cursor in a fenced code block first".into();
            return;
        };
        self.request_run(ctx, i, blocks[b].clone(), notes, capture);
    }

    /// The preview's Run button, for the block that starts at byte `at` and
    /// showed `code`.
    pub(super) fn run_block_from_preview(&mut self, ctx: &egui::Context, at: usize, code: &str) {
        let i = self.active;
        let note = self.note_root(i).is_some();
        let text = self.docs[i].editor.text_snapshot();
        let code = code.replace("\r\n", "\n");
        // The preview of a big file is redrawn a moment after typing stops:
        // run only what the button showed. (The preview reads a note as one
        // once its vault is indexed, so both readings are tried.)
        for notes in [note, !note] {
            if let Some(b) = runblock::code_blocks(&text, notes).into_iter().find(|b| b.at == at && b.code == code) {
                return self.request_run(ctx, i, b, notes, false);
            }
        }
        self.status = "The preview was out of date; click Run again".into();
    }

    /// Check that `block` can run, then run it, or ask first.
    fn request_run(&mut self, ctx: &egui::Context, i: usize, block: CodeBlock, notes: bool, capture: bool) {
        let Some(lang) = runblock::lang(&block.lang) else {
            let what = if block.lang.is_empty() { "a code block without a language".to_owned() } else { format!("{} blocks", block.lang) };
            self.status = format!("Strata doesn't run {what}; it runs bash, sh, zsh, fish, pwsh, powershell, cmd, python, node (js) and ruby blocks");
            return;
        };
        if !block.closed {
            self.status = "This code block has no closing fence, so it isn't run".into();
            return;
        }
        if block.code.trim().is_empty() {
            self.status = "This code block is empty".into();
            return;
        }
        if capture && block.indent.is_none() {
            self.status = "Output can only go under a code block whose fence starts its own line (not after a list marker or in a quote)".into();
            return;
        }
        if capture && self.code_run.running.is_some() {
            self.status = "A code block is still running; its output goes in first".into();
            return;
        }
        let d = &self.docs[i];
        let path = d.path.clone();
        let dest = if capture {
            let program = match runblock::capture_program(lang) {
                Ok(p) => p,
                Err(e) => {
                    self.status = e;
                    return;
                }
            };
            let dir = path.as_deref().and_then(Path::parent).filter(|p| p.is_dir()).map_or_else(|| self.terminal_dir(), Path::to_owned);
            Dest::Capture { program, dir }
        } else {
            self.terminal_dest(lang)
        };
        let req = Request { doc: d.editor.id(), path, block, lang, dest, notes };
        // Remembered only for a file on disk in a folder open in the side bar;
        // anything else (an untitled tab, a file from elsewhere) asks every time.
        let key = req.path.as_deref().and_then(runblock::file_key).filter(|k| self.in_open_folder(&k.path));
        // A block with characters that hide or reorder text always gets the
        // dialog, which spells them out and won't run it.
        let hidden = runblock::hidden(&req.block.code);
        if let Some(k) = &key
            && hidden.is_empty()
            && self.allowed().allowed(k)
        {
            return self.execute(ctx, req);
        }
        self.code_run.asking = Some(Ask { req, key, dont_ask: false, hidden });
        ctx.request_repaint();
    }

    /// `real` (a real path) is inside a folder open in the side bar.
    fn in_open_folder(&self, real: &Path) -> bool {
        self.sidebar.roots.iter().filter_map(|r| std::fs::canonicalize(r).ok()).any(|r| real.starts_with(r))
    }

    /// Where a block goes in the terminal: a shell block to the active tab
    /// (or the last shell tab when a REPL tab is active, so it can't be
    /// typed into Python), a language block to the REPL tab Run started for
    /// it, or a new tab.
    fn terminal_dest(&self, lang: Lang) -> Dest {
        let tabs = &self.terminal.tabs;
        match (lang.runs, lang.repl()) {
            (Runs::Repl, Some(program)) => match tabs.iter().rev().find(|t| t.repl() == Some(program)) {
                Some(t) => Dest::Repl { id: t.id(), program: program.to_owned() },
                None => Dest::NewRepl { program: program.to_owned() },
            },
            _ => {
                let shell = tabs.get(self.terminal.active).filter(|t| t.repl().is_none()).or_else(|| tabs.iter().rev().find(|t| t.repl().is_none()));
                match shell {
                    Some(t) => Dest::Terminal { id: t.id(), title: t.title().to_owned() },
                    None => Dest::NewShell,
                }
            }
        }
    }

    fn execute(&mut self, ctx: &egui::Context, req: Request) {
        if !runblock::hidden(&req.block.code).is_empty() {
            self.status = "Strata doesn't run a code block with hidden or reordering characters".into();
            return;
        }
        let code = req.block.code.clone();
        let name = req.lang.name;
        let tab = match req.dest {
            Dest::Capture { dir, .. } => return self.start_capture(ctx, req.doc, req.block, req.lang, req.notes, dir),
            Dest::Terminal { id, .. } | Dest::Repl { id, .. } => match self.terminal.index_of(id) {
                Some(t) => t,
                None => {
                    self.status = "That terminal tab has closed, so nothing was sent".into();
                    return;
                }
            },
            Dest::NewShell | Dest::NewRepl { .. } => {
                let dir = self.terminal_dir();
                let opened = match &req.dest {
                    Dest::NewRepl { program } => self.terminal.open_repl(ctx, &dir, program),
                    _ => self.terminal.open(ctx, &dir),
                };
                if let Err(e) = opened {
                    log::warn!("couldn't start a terminal in {}: {e}", dir.display());
                    self.status = e;
                    return;
                }
                self.terminal.active
            }
        };
        self.terminal.active = tab;
        self.set_terminal_visible(true);
        // Send to Terminal's way in: queued until a new shell or REPL shows
        // its prompt, one bracketed paste where the program takes one, then
        // Enter (and the empty line that ends an open Python block).
        self.terminal.tabs[tab].send(&code);
        self.status = format!("Sent the {name} block to the terminal");
    }

    fn start_capture(&mut self, ctx: &egui::Context, doc: u64, block: CodeBlock, lang: Lang, notes: bool, dir: PathBuf) {
        let timeout = std::time::Duration::from_secs(u64::from(self.settings.code_block_timeout_seconds.clamp(1, 3600)));
        let (tx, rx) = std::sync::mpsc::channel();
        let code = block.code.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(runblock::capture(lang, &code, &dir, timeout, runblock::CAPTURE_LIMIT));
            ctx2.request_repaint();
        });
        self.status = format!("Running the {} block…", lang.name);
        // Follow the block through edits made while it runs, so the output
        // goes under this block or nowhere (never under an identical one).
        // The closing fence's line break is left out: typing at the start of
        // the next line mustn't count as changing the block.
        let pin = self.docs.iter_mut().find(|d| d.editor.id() == doc).map(|d| {
            let text = d.editor.text_snapshot();
            let b = &d.editor.buffer;
            let end = block.lines.start + text[block.lines.clone()].trim_end_matches(['\r', '\n']).len();
            let range = b.byte_to_char(block.lines.start)..b.byte_to_char(end);
            let shown = b.slice(range.clone());
            (d.editor.pin(range), shown)
        });
        self.code_run.running = Some(Running { doc, block, notes, pin, rx });
    }

    /// Each frame: the confirmation, finished captures, REPLs that didn't start.
    pub(super) fn code_run_frame(&mut self, ctx: &egui::Context) {
        if let Some(program) = self.terminal.repl_failed.take() {
            self.status = format!("{program} stopped before the code block reached it; is {program} installed?");
        }
        if let Some(r) = &self.code_run.running
            && let Ok(result) = r.rx.try_recv()
        {
            let r = self.code_run.running.take().expect("checked");
            self.insert_output(r, result);
        }
        if self.code_run.asking.is_some() {
            // The dialog's buttons take the keyboard, not the editor.
            self.cancel_editor_focus();
            self.code_run_dialog(ctx);
        }
    }

    fn insert_output(&mut self, r: Running, result: Result<runblock::Captured, String>) {
        let lang = r.block.lang.clone();
        let doc = self.docs.iter().position(|d| d.editor.id() == r.doc);
        // Where the block is now, if it's still there exactly as it ran.
        let place = doc.and_then(|i| {
            let d = &mut self.docs[i];
            let (pin, shown) = r.pin.as_ref()?;
            let range = d.editor.pinned(*pin)?;
            d.editor.unpin(*pin);
            (d.editor.buffer.slice(range.clone()) == *shown).then_some((i, range.start))
        });
        let out = match result {
            Ok(out) => out,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let edit = place.and_then(|(i, start)| {
            let d = &mut self.docs[i];
            let text = d.editor.text_snapshot();
            let start = d.editor.buffer.char_to_byte(start);
            let blocks = runblock::code_blocks(&text, r.notes);
            let k = blocks.iter().position(|b| b.lines.start == start && b.code == r.block.code && b.lang == r.block.lang)?;
            runblock::output_edit(&text, &blocks, k, &out.text, d.editor.buffer.line_ending().as_str()).map(|e| (i, e))
        });
        let Some((i, (range, new))) = edit else {
            // Not under a block that may not be the one that ran: the output
            // goes to a tab of its own instead.
            let why = if doc.is_none() { "its tab was closed" } else { "it changed while it ran" };
            self.push_doc(crate::document::Document::untitled());
            let t = self.active;
            self.docs[t].editor.insert_text(&out.text);
            self.status = format!("The {lang} block finished, but {why}, so its output is in this new tab instead of the note");
            return;
        };
        let d = &mut self.docs[i];
        let b = &d.editor.buffer;
        let chars = b.byte_to_char(range.start)..b.byte_to_char(range.end);
        // One edit, so one Undo takes it back.
        d.editor.apply_edits(vec![(chars, new)]);
        self.status = format!("Put the output of the {lang} block under it ({:.1} s)", out.elapsed.as_secs_f64());
    }

    fn code_run_dialog(&mut self, ctx: &egui::Context) {
        let Some(ask) = &mut self.code_run.asking else { return };
        let size = self.settings.ui_font_size.clamp(9.0, 24.0);
        let (mut run, mut cancel) = (false, false);
        let capture = matches!(ask.req.dest, Dest::Capture { .. });
        let from = ask.req.path.as_ref().map_or_else(|| "an untitled tab".to_owned(), |p| p.display().to_string());
        let dest = ask.req.dest.describe(self.settings.code_block_timeout_seconds.clamp(1, 3600));
        crate::fonts::ensure_cjk(ctx, &ask.req.block.code);
        let screen = ctx.content_rect();
        let small = crate::fonts::secondary_size(size);
        egui::Modal::new(egui::Id::new("run-code-block")).show(ctx, |ui| {
            ui.set_width((screen.width() - 80.0).clamp(320.0, 720.0));
            let title = if capture { "Run this code block and capture its output?" } else { "Run this code block?" };
            ui.label(egui::RichText::new(title).strong().size(size * 1.1));
            ui.add_space(4.0);
            let line = |ui: &mut egui::Ui, label: &str, text: &str| {
                ui.add(egui::Label::new(egui::RichText::new(format!("{label} {text}")).size(small)).wrap());
            };
            line(ui, "From:", &from);
            line(ui, "Runs in:", &dest);
            if ask.key.is_none() {
                let why = if ask.req.path.is_some() { "This file isn't in a folder open in the side bar" } else { "This tab isn't a saved file" };
                ui.add(egui::Label::new(egui::RichText::new(format!("{why}, so Strata asks every time.")).size(small).color(ui.visuals().warn_fg_color)).wrap());
            }
            ui.add_space(6.0);
            ui.label(egui::RichText::new(format!("Exactly this ({} block):", ask.req.lang.name)).size(small));
            egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(4).inner_margin(8).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height((screen.height() * 0.45).max(120.0)).auto_shrink([false, true]).show(ui, |ui| {
                    let shown = runblock::visible(&ask.req.block.code);
                    let mut shown = shown.as_str();
                    ui.add(egui::TextEdit::multiline(&mut shown).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                });
            });
            ui.add_space(6.0);
            let blocked = !ask.hidden.is_empty();
            if blocked {
                let names: Vec<String> = ask.hidden.iter().map(|c| format!("U+{:04X}", *c as u32)).collect();
                let why = format!(
                    "Strata won't run this block: it holds characters that are invisible or change the order text is shown in ({}), so what you see above may not be what would run. Above, each is spelled out by its code. Remove them in the editor first.",
                    names.join(", ")
                );
                ui.add(egui::Label::new(egui::RichText::new(why).size(small).color(ui.visuals().error_fg_color)).wrap());
                ui.add_space(4.0);
            } else if ask.key.is_some() {
                ui.checkbox(&mut ask.dont_ask, "Don't ask again for this file").on_hover_text("Remembered in Strata's settings folder, not in the file (Notes: Forget Files Allowed to Run Code Blocks undoes it)");
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                run = ui.add_enabled(!blocked, egui::Button::new("Run")).clicked();
                cancel = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if cancel {
            self.code_run.asking = None;
            self.status = "Nothing was run".into();
        } else if run {
            let Ask { req, key, dont_ask, .. } = self.code_run.asking.take().expect("checked");
            if let Some(key) = key {
                // Confirmed once: the rest of this session doesn't ask again for this file.
                if dont_ask {
                    if let Err(e) = self.allowed().allow_always(key) {
                        self.status = e;
                    }
                } else {
                    self.allowed().allow_for_session(key);
                }
            }
            self.execute(ctx, req);
        }
    }

    /// Before Strata saves `path`: its key, if its blocks run without asking
    /// (saving replaces the file, which changes its identity on disk).
    pub(super) fn code_run_before_save(&mut self, path: &Path) {
        let key = runblock::file_key(path);
        self.code_run.saving = key.filter(|k| self.allowed().allowed(k));
    }

    /// After Strata saved `path`: what was allowed for the old file holds
    /// for the new one.
    pub(super) fn code_run_saved(&mut self, path: &Path) {
        let Some(old) = self.code_run.saving.take() else { return };
        if let Some(new) = runblock::file_key(path).filter(|k| k.path == old.path)
            && new != old
            && let Err(e) = self.allowed().moved(&old, &new)
        {
            log::warn!("{e}");
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::app::ui_tests::temp_dir;
    use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};

    /// One frame; the labelled accessibility nodes: (label, rect).
    fn frame(app: &mut StrataApp, ctx: &Context, events: Vec<Event>) -> Vec<(String, Rect)> {
        ctx.enable_accesskit();
        let time = Some(ctx.input(|i| i.time) + 0.1);
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1100.0, 700.0))), events, time, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            let mut frame = eframe::Frame::_new_kittest();
            eframe::App::ui(app, ui, &mut frame);
        });
        out.textures_delta.clear();
        let Some(update) = out.platform_output.accesskit_update else { return Vec::new() };
        update
            .nodes
            .iter()
            .filter_map(|(_, n)| {
                let label = n.label()?.to_owned();
                let b = n.bounds()?;
                Some((label, Rect::from_min_max(pos2(b.x0 as f32, b.y0 as f32), pos2(b.x1 as f32, b.y1 as f32))))
            })
            .collect()
    }

    fn find(nodes: &[(String, Rect)], label: &str) -> Option<Rect> {
        nodes.iter().find(|(l, _)| l == label).map(|(_, r)| *r)
    }

    fn labels(nodes: &[(String, Rect)]) -> Vec<&str> {
        nodes.iter().map(|n| n.0.as_str()).collect()
    }

    /// Click the node labelled `label` (drawn in the last frames).
    fn click(app: &mut StrataApp, ctx: &Context, label: &str) {
        // A dialog that just opened takes a frame or two to be laid out.
        frame(app, ctx, vec![]);
        frame(app, ctx, vec![]);
        let nodes = frame(app, ctx, vec![]);
        let at = find(&nodes, label).unwrap_or_else(|| panic!("no {label:?} in {:?}", labels(&nodes))).center();
        let b = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        frame(app, ctx, vec![Event::PointerMoved(at), b(true)]);
        frame(app, ctx, vec![b(false)]);
        frame(app, ctx, vec![]);
    }

    /// A window on folder `dir` showing `file` with the preview open, and a
    /// terminal running `program` (`sh -c`, echo off), once it prints READY.
    fn app_with_terminal(ctx: &Context, dir: &Path, file: &Path, program: &str, config: Option<PathBuf>) -> StrataApp {
        let mut app = StrataApp::new(Some(dir.to_owned()), config);
        app.settings.markdown_preview = true;
        // Not the login shell a window whose settings say the terminal was open would start.
        app.terminal_started = true;
        app.open_path(file.to_owned(), false);
        frame(&mut app, ctx, vec![]);
        let shell =("/bin/sh".to_owned(), vec!["-c".to_owned(), format!("stty -echo; {program}")]);
        app.terminal.tabs.push(crate::terminal::Terminal::spawn(Some(ctx.clone()), &std::env::temp_dir(), Some(shell), 100, 16).unwrap());
        app.terminal.active = app.terminal.tabs.len() - 1;
        app.terminal.visible = true;
        let s = wait_for(&mut app, ctx, "READY");
        assert!(s.contains("READY"), "{s}");
        app
    }

    fn screen(app: &StrataApp) -> String {
        app.terminal.active_terminal().map(|t| t.screen_text()).unwrap_or_default()
    }

    /// Run frames until the active terminal shows `needle`; returns its screen.
    fn wait_for(app: &mut StrataApp, ctx: &Context, needle: &str) -> String {
        wait_count(app, ctx, needle, 1)
    }

    /// Run frames until the active terminal shows `needle` `n` times.
    fn wait_count(app: &mut StrataApp, ctx: &Context, needle: &str, n: usize) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            frame(app, ctx, vec![]);
            let s = screen(app);
            if s.matches(needle).count() >= n || std::time::Instant::now() > deadline {
                return s;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Frames for a while, so anything wrongly sent would have arrived.
    fn idle(app: &mut StrataApp, ctx: &Context) {
        for _ in 0..15 {
            frame(app, ctx, vec![]);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn text(app: &StrataApp) -> String {
        let b = &app.docs[app.active].editor.buffer;
        b.slice(0..b.len_chars())
    }

    const RUNBOOK: &str = "# Deploy\n\nCheck first:\n\n```bash title=\"check\"\necho run-$((6*7))\nprintf '%s\\n' \"two words\"\n```\n\n```rust\nfn main() {}\n```\n";

    #[test]
    fn preview_run_button_sends_the_exact_text_after_confirmation() {
        let dir = temp_dir("run-preview");
        let file = dir.join("runbook.md");
        std::fs::write(&file, RUNBOOK).unwrap();
        let ctx = Context::default();
        // The program asks for bracketed paste, and `cat -v` shows exactly what arrives.
        let mut app = app_with_terminal(&ctx, &dir, &file, "printf '\\033[?2004h'; echo READY; cat -v", None);
        // Opening the file and drawing its preview runs nothing.
        idle(&mut app, &ctx);
        assert_eq!(screen(&app).trim(), "READY", "nothing arrives before a click");
        assert!(!app.code_run.asking());
        // One Run button: the bash block has one, the rust block doesn't.
        let nodes = frame(&mut app, &ctx, vec![]);
        assert_eq!(nodes.iter().filter(|(l, _)| l == "▶ Run").count(), 1, "{:?}", labels(&nodes));
        click(&mut app, &ctx, "▶ Run");
        assert!(app.code_run.asking(), "the click asks first");
        let nodes = frame(&mut app, &ctx, vec![]);
        assert!(find(&nodes, "Don't ask again for this file").is_some(), "a file in the open folder can be remembered");
        let dest = &app.code_run.asking.as_ref().unwrap().req.dest;
        assert!(matches!(dest, Dest::Terminal { id, .. } if *id == app.terminal.tabs[0].id()), "{dest:?}");
        // Nothing is sent while the question is up, and typing doesn't reach the editor.
        frame(&mut app, &ctx, vec![Event::Text("x".into())]);
        idle(&mut app, &ctx);
        assert_eq!(screen(&app).trim(), "READY", "nothing is sent before Run");
        assert_eq!(text(&app), RUNBOOK);
        click(&mut app, &ctx, "Run");
        assert!(!app.code_run.asking());
        let s = wait_for(&mut app, &ctx, "words\"^[[201~");
        assert!(s.contains("READY\n^[[200~echo run-$((6*7))\nprintf '%s\\n' \"two words\"^[[201~\n"), "{s}");
        assert!(!app.terminal.has_focus(&ctx), "the editor keeps the keyboard");
        // Confirmed once, the file doesn't ask again this session.
        click(&mut app, &ctx, "▶ Run");
        assert!(!app.code_run.asking());
        let s = wait_count(&mut app, &ctx, "two words\"^[[201~", 2);
        assert_eq!(s.matches("^[[200~echo run-").count(), 2, "{s}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_shell_runs_the_block() {
        let dir = temp_dir("run-shell");
        let file = dir.join("runbook.md");
        std::fs::write(&file, RUNBOOK).unwrap();
        let ctx = Context::default();
        let shell = if Path::new("/bin/zsh").exists() { "exec /bin/zsh -f" } else { "exec /bin/sh" };
        let mut app = app_with_terminal(&ctx, &dir, &file, &format!("echo READY; {shell}"), None);
        click(&mut app, &ctx, "▶ Run");
        click(&mut app, &ctx, "Run");
        let s = wait_for(&mut app, &ctx, "two words");
        let rows: Vec<&str> = s.lines().map(str::trim).collect();
        assert!(rows.contains(&"run-42") && rows.contains(&"two words"), "{s}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_and_escape_send_nothing() {
        let dir = temp_dir("run-cancel");
        let file = dir.join("runbook.md");
        std::fs::write(&file, RUNBOOK).unwrap();
        let ctx = Context::default();
        let mut app = app_with_terminal(&ctx, &dir, &file, "echo READY; cat -v", None);
        click(&mut app, &ctx, "▶ Run");
        click(&mut app, &ctx, "Cancel");
        assert!(!app.code_run.asking());
        click(&mut app, &ctx, "▶ Run");
        frame(&mut app, &ctx, vec![crate::app::ui_tests::press(egui::Key::Escape, Modifiers::NONE)]);
        assert!(!app.code_run.asking());
        idle(&mut app, &ctx);
        assert_eq!(screen(&app).trim(), "READY");
        // Cancelled isn't confirmed: it asks again.
        click(&mut app, &ctx, "▶ Run");
        assert!(app.code_run.asking());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_outside_the_open_folders_ask_every_time() {
        let dir = temp_dir("run-outside");
        let elsewhere = temp_dir("run-outside-file");
        let file = elsewhere.join("downloaded.md");
        std::fs::write(&file, RUNBOOK).unwrap();
        let ctx = Context::default();
        let mut app = app_with_terminal(&ctx, &dir, &file, "echo READY; cat -v", None);
        for n in 1..=2 {
            click(&mut app, &ctx, "▶ Run");
            assert!(app.code_run.asking(), "run {n} asks");
            let nodes = frame(&mut app, &ctx, vec![]);
            assert!(find(&nodes, "Don't ask again for this file").is_none(), "can't be remembered");
            click(&mut app, &ctx, "Run");
            wait_count(&mut app, &ctx, "two words", n);
        }
        assert_eq!(screen(&app).matches("echo run-").count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }

    #[test]
    fn dont_ask_again_is_kept_by_file_identity() {
        let dir = temp_dir("run-remember");
        let config = temp_dir("run-remember-config");
        let file = dir.join("runbook.md");
        std::fs::write(&file, RUNBOOK).unwrap();
        let ctx = Context::default();
        let mut app = app_with_terminal(&ctx, &dir, &file, "echo READY; cat -v", Some(config.clone()));
        click(&mut app, &ctx, "▶ Run");
        click(&mut app, &ctx, "Don't ask again for this file");
        click(&mut app, &ctx, "Run");
        let s = wait_for(&mut app, &ctx, "two words");
        assert!(s.contains("two words"), "{s} / {} / tabs {}", app.status, app.terminal.tabs.len());
        let saved = std::fs::read_to_string(config.join("code-blocks-allowed.json")).unwrap();
        assert!(saved.contains("runbook.md"), "{saved}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), RUNBOOK, "nothing is written into the file");
        // Strata's own save replaces the file; it stays allowed.
        app.docs[app.active].editor.insert_text("<!-- edited -->\n");
        app.run(&ctx, Cmd::Save);
        assert!(std::fs::read_to_string(&file).unwrap().contains("edited"), "{}", app.status);
        drop(app);
        // A new window doesn't ask.
        let ctx = Context::default();
        let mut app = app_with_terminal(&ctx, &dir, &file, "echo READY; cat -v", Some(config.clone()));
        click(&mut app, &ctx, "▶ Run");
        assert!(!app.code_run.asking(), "remembered across windows and Strata's own saves");
        let s = wait_for(&mut app, &ctx, "two words");
        assert!(s.contains("two words"), "{s} / {} / tabs {}", app.status, app.terminal.tabs.len());
        // Another program rewrites it in place, through a hard link: same
        // path and identity, different text, so it asks again.
        let alias = dir.join("alias.md");
        std::fs::hard_link(&file, &alias).unwrap();
        let rewritten = std::fs::read_to_string(&file).unwrap() + "\n<!-- changed elsewhere -->\n";
        std::fs::write(&alias, rewritten).unwrap();
        std::fs::remove_file(&alias).unwrap();
        click(&mut app, &ctx, "▶ Run");
        assert!(app.code_run.asking(), "a rewrite in place asks again");
        click(&mut app, &ctx, "Cancel");
        drop(app);
        // Another file put at the same path (a download, a checkout) asks again.
        let other = dir.join("incoming.md");
        std::fs::write(&other, RUNBOOK).unwrap();
        std::fs::rename(&other, &file).unwrap();
        let ctx = Context::default();
        let mut app = app_with_terminal(&ctx, &dir, &file, "echo READY; cat -v", Some(config.clone()));
        click(&mut app, &ctx, "▶ Run");
        assert!(app.code_run.asking(), "a different file at the same path asks");
        click(&mut app, &ctx, "Cancel");
        // Forgetting makes every file ask.
        app.run(&ctx, Cmd::NotesForgetCodeBlockFiles);
        assert!(app.status.starts_with("Forgot 1 file"), "{}", app.status);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&config);
    }

    /// Wait until the capture finished and its output went in.
    fn wait_capture(app: &mut StrataApp, ctx: &Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.code_run.running.is_some() && std::time::Instant::now() < deadline {
            frame(app, ctx, vec![]);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        frame(app, ctx, vec![]);
        assert!(app.code_run.running.is_none(), "the capture finished");
    }

    #[test]
    fn capture_puts_output_under_the_block_as_one_undo_step() {
        for eol in ["\n", "\r\n"] {
            let dir = temp_dir("run-capture");
            let file = dir.join("steps.md");
            // Runs in the file's folder; counts its runs there, so each output differs.
            let code = "echo captured-$((6*7))\necho to-stderr >&2\nn=$(($(cat count 2>/dev/null || echo 0) + 1)); echo $n > count; echo run-$n";
            let original = format!("# Steps\n\n```sh\n{code}\n```\n\nNext.\n").replace('\n', eol);
            let output = |n: usize| format!("# Steps\n\n```sh\n{code}\n```\n```output\ncaptured-42\nto-stderr\nrun-{n}\n```\n\nNext.\n").replace('\n', eol);
            std::fs::write(&file, &original).unwrap();
            let ctx = Context::default();
            let mut app = StrataApp::new(Some(dir.clone()), None);
            app.open_path(file.clone(), false);
            frame(&mut app, &ctx, vec![]);
            let cursor = app.docs[app.active].editor.buffer.byte_to_char(original.find("captured").unwrap());
            app.docs[app.active].editor.select_ranges(&[cursor..cursor], 0);
            app.run(&ctx, Cmd::NotesRunCodeBlockCapture);
            assert!(app.code_run.asking() && app.code_run.running.is_none(), "asks before running anything");
            let dest = &app.code_run.asking.as_ref().unwrap().req.dest;
            assert!(matches!(dest, Dest::Capture { dir: d, .. } if *d == dir), "{dest:?}");
            assert!(dest.describe(60).starts_with("A separate process, not your terminal: "), "{}", dest.describe(60));
            click(&mut app, &ctx, "Run");
            wait_capture(&mut app, &ctx);
            let once = text(&app);
            assert_eq!(once, output(1), "{}", app.status);
            assert!(app.terminal.tabs.is_empty(), "not the user's terminal");
            // Again: the output block is replaced. Confirmed once, it doesn't ask.
            app.docs[app.active].editor.select_ranges(&[cursor..cursor], 0);
            app.run(&ctx, Cmd::NotesRunCodeBlockCapture);
            assert!(!app.code_run.asking());
            wait_capture(&mut app, &ctx);
            assert_eq!(text(&app), output(2), "replaced, not added");
            // Undo: one step back each, byte for byte.
            app.docs[app.active].editor.undo();
            assert_eq!(text(&app), once);
            app.docs[app.active].editor.undo();
            assert_eq!(text(&app), original, "byte for byte ({eol:?})");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn capture_finds_the_block_after_edits_while_it_runs() {
        let dir = temp_dir("run-capture-edit");
        let file = dir.join("steps.md");
        std::fs::write(&file, "```sh\nsleep 0.5; echo late\n```\n").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(dir.clone()), None);
        app.open_path(file.clone(), false);
        frame(&mut app, &ctx, vec![]);
        app.docs[app.active].editor.select_ranges(&[8..8], 0);
        app.run(&ctx, Cmd::NotesRunCodeBlockCapture);
        click(&mut app, &ctx, "Run");
        // Typing above the block while it runs.
        app.docs[app.active].editor.select_ranges(&[0..0], 0);
        app.docs[app.active].editor.insert_text("Intro\n\n");
        wait_capture(&mut app, &ctx);
        assert_eq!(text(&app), "Intro\n\n```sh\nsleep 0.5; echo late\n```\n```output\nlate\n```\n", "{}", app.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cursor_must_be_in_a_runnable_closed_block() {
        let dir = temp_dir("run-refuse");
        let file = dir.join("notes.md");
        std::fs::write(&file, "text\n\n```rust\nfn main() {}\n```\n\n```sh\necho open\n").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(dir.clone()), None);
        app.open_path(file.clone(), false);
        frame(&mut app, &ctx, vec![]);
        for (pos, says) in [(1, "Put the cursor in a fenced code block"), (16, "doesn't run rust blocks"), (40, "no closing fence")] {
            app.docs[app.active].editor.select_ranges(&[pos..pos], 0);
            app.run(&ctx, Cmd::NotesRunCodeBlock);
            assert!(app.status.contains(says), "{pos}: {}", app.status);
            assert!(!app.code_run.asking() && app.terminal.tabs.is_empty());
        }
        // Not Markdown.
        let txt = dir.join("plain.txt");
        std::fs::write(&txt, "```sh\nls\n```\n").unwrap();
        app.open_path(txt, false);
        app.docs[app.active].editor.select_ranges(&[7..7], 0);
        app.run(&ctx, Cmd::NotesRunCodeBlock);
        assert!(app.status.contains("Markdown files"), "{}", app.status);
        assert!(!app.code_run.asking());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_python_block_starts_python_in_a_tab_of_its_own() {
        if !std::process::Command::new("python3").args(["-c", ""]).status().is_ok_and(|s| s.success()) {
            return;
        }
        let dir = temp_dir("run-python");
        let file = dir.join("py.md");
        std::fs::write(&file, "```python\nfor i in range(2):\n    print(i * 21)\n```\n\n```bash\necho shell-$((2+3))\n```\n").unwrap();
        let ctx = Context::default();
        let shell = if Path::new("/bin/zsh").exists() { "/bin/zsh -f" } else { "/bin/sh" };
        let mut app = app_with_terminal(&ctx, &dir, &file, &format!("echo READY; exec {shell}"), None);
        let shell_tab = app.terminal.tabs[0].id();
        app.docs[app.active].editor.select_ranges(&[12..12], 0);
        app.run(&ctx, Cmd::NotesRunCodeBlock);
        let dest = &app.code_run.asking.as_ref().unwrap().req.dest;
        assert_eq!(dest, &Dest::NewRepl { program: "python3".into() });
        click(&mut app, &ctx, "Run");
        assert_eq!(app.terminal.tabs.len(), 2);
        assert_eq!(app.terminal.active_terminal().unwrap().repl(), Some("python3"));
        assert_eq!(app.terminal.active_terminal().unwrap().title(), "python3", "the tab is named after the REPL");
        let s = wait_for(&mut app, &ctx, "\n21\n");
        assert!(s.lines().any(|l| l.trim() == "0") && s.lines().any(|l| l.trim() == "21"), "{s}");
        // The shell block goes to the shell tab, not into Python.
        app.docs[app.active].editor.select_ranges(&[60..60], 0);
        app.run(&ctx, Cmd::NotesRunCodeBlock);
        assert!(!app.code_run.asking(), "confirmed for this file already");
        assert_eq!(app.terminal.active_terminal().unwrap().id(), shell_tab);
        let s = wait_for(&mut app, &ctx, "shell-5");
        assert!(s.lines().any(|l| l.trim() == "shell-5"), "{s}");
        // A second Python block reuses the Python tab.
        app.docs[app.active].editor.select_ranges(&[12..12], 0);
        app.run(&ctx, Cmd::NotesRunCodeBlock);
        assert_eq!(app.terminal.tabs.len(), 2);
        assert_eq!(app.terminal.active_terminal().unwrap().repl(), Some("python3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `STRATA_RUN_SHOTS=<folder> cargo test --release run_block_shots`: whole
    /// frames rendered off screen (wgpu): a Python block run in its REPL tab,
    /// and the dialog refusing a block with a hidden character.
    #[test]
    fn run_block_shots() {
        let Some(out) = std::env::var_os("STRATA_RUN_SHOTS").map(PathBuf::from) else { return };
        let Some(mut shooter) = crate::app::headless::Shooter::new(1100, 720, 2.0) else { return };
        let dir = temp_dir("run-shots");
        let file = dir.join("Runbook.md");
        std::fs::write(&file, "# Runbook\n\n```python\nfor i in range(3):\n    print(\"step\", i * 7)\n```\n\n```sh\necho safe \u{202E}txt.hs\n```\n").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(dir.clone()), None);
        app.settings.markdown_preview = true;
        app.terminal_started = true;
        app.open_path(file, false);
        frame(&mut app, &ctx, vec![]);
        app.docs[app.active].editor.select_ranges(&[25..25], 0);
        app.run(&ctx, Cmd::NotesRunCodeBlock);
        click(&mut app, &ctx, "Run");
        let s = wait_for(&mut app, &ctx, "step 14");
        assert!(s.contains("step 14"), "{s}");
        wait_for(&mut app, &ctx, ">>> ");
        shooter.shoot(&ctx, &mut app, 4, vec![], &out.join("run-python-tab.ppm"));
        let sh = app.docs[app.active].editor.buffer.byte_to_char(80);
        app.docs[app.active].editor.select_ranges(&[sh..sh], 0);
        app.run(&ctx, Cmd::NotesRunCodeBlock);
        assert!(app.code_run.asking());
        shooter.shoot(&ctx, &mut app, 6, vec![], &out.join("run-hidden-refused.ppm"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blocks_with_hidden_characters_never_run() {
        let dir = temp_dir("run-hidden");
        let file = dir.join("runbook.md");
        // The first block looks like `echo safe` but a right-to-left override
        // reorders what follows it on screen.
        let src = "```sh\necho safe \u{202E}txt.hs\n```\n\n```sh\necho fine\n```\n";
        std::fs::write(&file, src).unwrap();
        let ctx = Context::default();
        let mut app = app_with_terminal(&ctx, &dir, &file, "echo READY; cat -v", None);
        // Confirm the ordinary block, so the file doesn't ask again this session.
        let fine = src.find("echo fine").unwrap();
        app.docs[app.active].editor.select_ranges(&[fine..fine], 0);
        app.run(&ctx, Cmd::NotesRunCodeBlock);
        click(&mut app, &ctx, "Run");
        wait_for(&mut app, &ctx, "echo fine");
        // The other still shows the dialog, with the character spelled out and Run refused.
        for capture in [false, true] {
            app.docs[app.active].editor.select_ranges(&[8..8], 0);
            app.run(&ctx, if capture { Cmd::NotesRunCodeBlockCapture } else { Cmd::NotesRunCodeBlock });
            let ask = app.code_run.asking.as_ref().expect("it shows what's wrong");
            assert_eq!(ask.hidden, vec!['\u{202E}']);
            assert_eq!(runblock::visible(&ask.req.block.code), "echo safe ⟨U+202E⟩txt.hs");
            click(&mut app, &ctx, "Run");
            assert!(app.code_run.asking(), "Run does nothing");
            click(&mut app, &ctx, "Cancel");
            assert!(app.code_run.running.is_none());
        }
        idle(&mut app, &ctx);
        assert!(!screen(&app).contains("safe"), "nothing was sent: {}", screen(&app));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Start capturing the first block of `src` (confirmed), returning the app.
    fn capturing(ctx: &Context, tag: &str, src: &str) -> (PathBuf, StrataApp) {
        let dir = temp_dir(tag);
        let file = dir.join("steps.md");
        std::fs::write(&file, src).unwrap();
        let mut app = StrataApp::new(Some(dir.clone()), None);
        app.open_path(file, false);
        frame(&mut app, ctx, vec![]);
        app.docs[app.active].editor.select_ranges(&[8..8], 0);
        app.run(ctx, Cmd::NotesRunCodeBlockCapture);
        click(&mut app, ctx, "Run");
        assert!(app.code_run.running.is_some());
        (dir, app)
    }

    const TWINS: &str = "```sh\nsleep 0.5; echo twin\n```\n\nBetween.\n\n```sh\nsleep 0.5; echo twin\n```\n";

    #[test]
    fn capture_output_lands_under_its_own_block_among_twins() {
        let ctx = Context::default();
        let (dir, mut app) = capturing(&ctx, "run-twins", TWINS);
        // Text added above both while it runs: the block moves, and is still found.
        app.docs[app.active].editor.select_ranges(&[0..0], 0);
        app.docs[app.active].editor.insert_text("Intro\n\n");
        wait_capture(&mut app, &ctx);
        assert_eq!(text(&app), format!("Intro\n\n```sh\nsleep 0.5; echo twin\n```\n```output\ntwin\n```\n{}", &TWINS[31..]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_never_lands_under_a_twin_when_its_block_is_gone_or_changed() {
        for change in ["delete", "edit"] {
            let ctx = Context::default();
            let (dir, mut app) = capturing(&ctx, "run-twins-gone", TWINS);
            let e = &mut app.docs[app.active].editor;
            if change == "delete" {
                // The first block (and the blank line after it) removed while it runs.
                e.select_ranges(&[0..32], 0);
                e.insert_text("");
            } else {
                e.select_ranges(&[17..17], 0);
                e.insert_text(" changed");
            }
            let before = text(&app);
            let tabs = app.docs.len();
            wait_capture(&mut app, &ctx);
            assert_eq!(app.docs.len(), tabs + 1, "{change}: the output went to a tab of its own");
            assert_eq!(text(&app), "twin", "{change}");
            let note = app.docs.iter().position(|d| d.path.as_ref().is_some_and(|p| p.ends_with("steps.md"))).unwrap();
            let b = &app.docs[note].editor.buffer;
            assert_eq!(b.slice(0..b.len_chars()), before, "{change}: the note wasn't touched");
            assert!(app.status.contains("changed while it ran"), "{change}: {}", app.status);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
