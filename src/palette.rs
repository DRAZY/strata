//! The quick panel: command palette (Cmd+Shift+P), Goto Anything (Cmd+P),
//! go to line (Ctrl+G), go to symbol (Cmd+R) and the theme/syntax pickers.
//!
//! In Goto Anything, `:42` jumps to a line and `@name` to a symbol, either
//! alone (current file) or after a file query.

use crate::commands::Cmd;
use crate::files::FileIndex;
use crate::fuzzy;
use crate::symbols::{self, Symbol};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Key, Modifiers, Sense, Stroke, vec2};
use std::path::PathBuf;
use std::sync::Arc;

const MAX_ROWS: usize = 60;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Commands,
    Files,
    Theme,
    Syntax,
}

/// What the app should do. `preview` events are undone by `Cancel`.
#[derive(Debug, PartialEq)]
pub enum Event {
    Run(Cmd),
    /// Run an editor command by name (see `editor_commands`).
    Editor(&'static str),
    Open(PathBuf),
    /// 0-based line in the current file.
    GotoLine { line: usize, preview: bool },
    Theme { name: String, preview: bool },
    Syntax(String),
    Cancel,
}

/// Snapshot of the current file, taken when the panel opens.
pub struct Context {
    pub text: Arc<String>,
    pub syntax: String,
    pub lines: usize,
    pub root: Option<PathBuf>,
    pub theme: String,
    /// Open files, so Goto Anything finds them without a folder.
    pub open: Vec<PathBuf>,
}

struct Row {
    label: String,
    hits: Vec<usize>,
    /// Shown dimmed at the right (shortcut, symbol kind, `:line` suffix).
    detail: String,
    pick: Pick,
}

#[derive(Clone)]
enum Pick {
    Cmd(Cmd),
    Editor(&'static str),
    File(PathBuf),
    Line(usize),
    Theme(String),
    Syntax(String),
}

pub struct Palette {
    pub open: bool,
    mode: Mode,
    text: String,
    selected: usize,
    focus: bool,
    ctx: Option<Context>,
    files: FileIndex,
    symbols: Option<Vec<Symbol>>,
    rows: Vec<Row>,
    /// (text, mode, files seen) the rows were built for.
    rows_key: Option<(String, Mode, usize)>,
    /// Row whose preview was last sent, so moving the selection previews once.
    previewed: Option<usize>,
    /// Editor commands as (id, palette name, shortcut label).
    editor_cmds: &'static [(&'static str, &'static str, &'static str)],
}

impl Palette {
    pub fn new(editor_cmds: &'static [(&'static str, &'static str, &'static str)]) -> Self {
        Self {
            open: false,
            mode: Mode::Commands,
            text: String::new(),
            selected: 0,
            focus: false,
            ctx: None,
            files: FileIndex::default(),
            symbols: None,
            rows: Vec::new(),
            rows_key: None,
            previewed: None,
            editor_cmds,
        }
    }

    /// `prefix` seeds the query: ":" for go to line, "@" for symbols.
    pub fn show_mode(&mut self, mode: Mode, prefix: &str, ctx: Context, egui_ctx: &egui::Context) {
        if mode == Mode::Files {
            // Re-list the folder each time so new files show up; it streams in quickly.
            match &ctx.root {
                Some(root) => self.files.start(root, Some(egui_ctx.clone())),
                None => self.files = FileIndex::default(),
            }
        }
        self.open = true;
        self.mode = mode;
        self.text = prefix.to_owned();
        self.selected = 0;
        self.focus = true;
        self.symbols = None;
        self.rows_key = None;
        self.previewed = None;
        self.ctx = Some(ctx);
        if mode == Mode::Theme {
            // Start on the current theme.
            self.rebuild();
            let current = self.ctx.as_ref().map(|c| c.theme.clone()).unwrap_or_default();
            self.selected = self.rows.iter().position(|r| r.label == current).unwrap_or(0);
            self.previewed = Some(self.selected);
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.files.stop();
        self.ctx = None;
        self.symbols = None;
    }

    fn symbols(&mut self) -> &[Symbol] {
        let ctx = self.ctx.as_ref().expect("open palette has a context");
        self.symbols.get_or_insert_with(|| symbols::symbols(&ctx.syntax, &ctx.text))
    }

    /// Recompute the rows when the query, mode or file list changed.
    fn rebuild(&mut self) {
        self.files.poll();
        let key = (self.text.clone(), self.mode, self.files.paths().len());
        if self.rows_key.as_ref() == Some(&key) {
            return;
        }
        let first_build = self.rows_key.is_none();
        let kept = self.rows.get(self.selected).map(|r| r.label.clone());
        self.rows_key = Some(key);
        self.rows = match self.mode {
            Mode::Commands => self.command_rows(),
            Mode::Files => self.goto_rows(),
            Mode::Theme => {
                let names = crate::highlight::theme_names();
                rank_rows(&self.text, &names, |n| n, |n| Pick::Theme(n.to_string()), |_| String::new())
            }
            Mode::Syntax => {
                let names = crate::highlight::syntax_names();
                rank_rows(&self.text, &names, |n| n, |n| Pick::Syntax(n.to_string()), |_| String::new())
            }
        };
        // Streaming file results shouldn't yank the selection around.
        self.selected = if first_build { 0 } else { kept.and_then(|k| self.rows.iter().position(|r| r.label == k)).unwrap_or(0) };
    }

    fn command_rows(&self) -> Vec<Row> {
        let mut items: Vec<(String, String, Pick)> = Cmd::ALL
            .iter()
            .filter(|c| **c != Cmd::CommandPalette)
            .map(|&c| (c.name().to_owned(), c.shortcut_label().unwrap_or_default(), Pick::Cmd(c)))
            .collect();
        items.extend(self.editor_cmds.iter().map(|&(id, name, keys)| (name.to_owned(), keys.to_owned(), Pick::Editor(id))));
        items.extend(crate::highlight::syntax_names().into_iter().map(|s| (format!("Set Syntax: {s}"), String::new(), Pick::Syntax(s.to_owned()))));
        rank_rows(&self.text, &items, |i| &i.0, |i| i.2.clone(), |i| i.1.clone())
    }

    /// Goto Anything: `file`, `file:line`, `:line`, `@symbol`, `file@symbol`.
    fn goto_rows(&mut self) -> Vec<Row> {
        let text = self.text.clone();
        let (file_q, rest) = match text.find([':', '@']) {
            Some(i) => (&text[..i], Some(&text[i..])),
            None => (text.as_str(), None),
        };
        if file_q.is_empty() {
            match rest {
                Some(r) if r.starts_with(':') => {
                    let lines = self.ctx.as_ref().map_or(1, |c| c.lines);
                    return match r[1..].trim().parse::<usize>() {
                        Ok(n) if n >= 1 => {
                            let n = n.min(lines);
                            vec![Row { label: format!("Go to line {n}"), hits: vec![], detail: format!("of {lines}"), pick: Pick::Line(n - 1) }]
                        }
                        _ => vec![Row { label: format!("Type a line number between 1 and {lines}"), hits: vec![], detail: String::new(), pick: Pick::Line(usize::MAX) }],
                    };
                }
                Some(r) => {
                    let q = r[1..].to_owned();
                    let syms = self.symbols().to_vec();
                    return rank_rows(&q, &syms, |s| &s.name, |s| Pick::Line(s.line), |s| format!("{}  :{}", s.kind, s.line + 1));
                }
                None => {}
            }
        }
        // Files under the folder, plus open files outside it.
        let root = self.ctx.as_ref().and_then(|c| c.root.clone());
        let mut paths: Vec<(String, PathBuf)> = self.files.paths().iter().map(|p| (p.clone(), self.files.root().join(p))).collect();
        for p in self.ctx.as_ref().map(|c| c.open.clone()).unwrap_or_default() {
            if root.as_ref().is_none_or(|r| !p.starts_with(r)) {
                paths.push((p.display().to_string(), p));
            }
        }
        let mut rows = rank_rows(file_q, &paths, |p| &p.0, |p| Pick::File(p.1.clone()), |_| String::new());
        // Keep the `:line` / `@symbol` suffix: it applies after the file opens.
        for r in &mut rows {
            if let Some(suffix) = rest {
                r.detail = suffix.to_owned();
            }
        }
        rows
    }

    /// Draw the panel; returns what the app should do.
    pub fn show(&mut self, ctx: &egui::Context) -> Vec<Event> {
        let mut events = Vec::new();
        if !self.open {
            return events;
        }
        self.rebuild();
        let (up, down, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        let n = self.rows.len();
        if n > 0 {
            if up {
                self.selected = (self.selected + n - 1) % n;
            }
            if down {
                self.selected = (self.selected + 1) % n;
            }
        }
        let mut picked = enter.then_some(self.selected);

        let screen = ctx.content_rect();
        let width = (screen.width() - 32.0).min(620.0);
        let visuals = ctx.global_style().visuals.clone();
        let fg = visuals.text_color();
        let accent = visuals.selection.bg_fill;
        let mut clicked_outside = false;
        let area = egui::Area::new(egui::Id::new("palette"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(screen.center().x - width / 2.0, screen.top() + 44.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(8.0).corner_radius(8.0).show(ui, |ui| {
                    ui.set_width(width);
                    let hint = match self.mode {
                        Mode::Commands => "Type a command",
                        Mode::Files => "Go to file  ·  :line  ·  @symbol",
                        Mode::Theme => "Color scheme",
                        Mode::Syntax => "Syntax",
                    };
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.text)
                            .id(egui::Id::new("palette-query"))
                            .hint_text(hint)
                            .font(FontId::proportional(16.0))
                            .desired_width(f32::INFINITY)
                            .margin(vec2(8.0, 6.0)),
                    );
                    if std::mem::take(&mut self.focus) {
                        field.request_focus();
                    }
                    if !field.has_focus() && !self.focus {
                        field.request_focus();
                    }
                    if self.mode == Mode::Files && !self.files.is_ready() && self.ctx.as_ref().is_some_and(|c| c.root.is_some()) {
                        ui.label(egui::RichText::new(format!("Indexing… {} files", self.files.paths().len())).small().weak());
                    } else if self.mode == Mode::Files && self.files.is_truncated() {
                        ui.label(egui::RichText::new(format!("Only the first {} files are indexed", crate::files::FILE_LIMIT)).small().weak());
                    }
                    ui.add_space(4.0);
                    let row_h = 26.0;
                    egui::ScrollArea::vertical().max_height(row_h * 12.0).auto_shrink([false, true]).show(ui, |ui| {
                        for (i, row) in self.rows.iter().take(MAX_ROWS).enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                            if i == self.selected {
                                ui.painter().rect_filled(rect, 4.0, accent);
                                if up || down {
                                    ui.scroll_to_rect(rect, None);
                                }
                            } else if resp.hovered() {
                                ui.painter().rect_filled(rect, 4.0, accent.gamma_multiply(0.4));
                            }
                            let galley = ui.painter().layout_job(highlighted(&row.label, &row.hits, fg, accent_text(&visuals)));
                            ui.painter().galley(egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, fg);
                            if !row.detail.is_empty() {
                                ui.painter().text(
                                    egui::pos2(rect.right() - 8.0, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    &row.detail,
                                    FontId::proportional(12.0),
                                    fg.gamma_multiply(0.55),
                                );
                            }
                            if resp.clicked() {
                                picked = Some(i);
                            }
                        }
                        if self.rows.is_empty() {
                            ui.label(egui::RichText::new("No matches").weak());
                        }
                    });
                });
            });
        if ctx.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer() {
            clicked_outside = true;
        }

        if esc || clicked_outside {
            self.close();
            events.push(Event::Cancel);
            return events;
        }
        if let Some(i) = picked {
            if let Some(row) = self.rows.get(i).cloned_pick() {
                events.extend(self.finish(row));
            }
            return events;
        }
        // Live preview: jump to lines/symbols and try themes as you move.
        if self.previewed != Some(self.selected) {
            self.previewed = Some(self.selected);
            match self.rows.get(self.selected).map(|r| &r.pick) {
                Some(Pick::Line(l)) if *l != usize::MAX => events.push(Event::GotoLine { line: *l, preview: true }),
                Some(Pick::Theme(t)) => events.push(Event::Theme { name: t.clone(), preview: true }),
                _ => {}
            }
        }
        events
    }

    fn finish(&mut self, (pick, detail): (Pick, String)) -> Vec<Event> {
        let mut out = Vec::new();
        match pick {
            Pick::Line(usize::MAX) => return out, // no valid line typed yet
            Pick::Cmd(c) => out.push(Event::Run(c)),
            Pick::Editor(id) => out.push(Event::Editor(id)),
            Pick::Line(l) => out.push(Event::GotoLine { line: l, preview: false }),
            Pick::Theme(t) => out.push(Event::Theme { name: t, preview: false }),
            Pick::Syntax(s) => out.push(Event::Syntax(s)),
            Pick::File(p) => {
                out.push(Event::Open(p));
                // "file:42" → line 42 once the file is open; "file@x" → symbol picker in it.
                if let Some(n) = detail.strip_prefix(':').and_then(|n| n.trim().parse::<usize>().ok()) {
                    out.push(Event::GotoLine { line: n.saturating_sub(1), preview: false });
                } else if detail.starts_with('@') {
                    out.push(Event::Run(Cmd::GotoSymbol));
                }
            }
        }
        self.close();
        out
    }
}

trait ClonedPick {
    fn cloned_pick(self) -> Option<(Pick, String)>;
}

impl ClonedPick for Option<&Row> {
    fn cloned_pick(self) -> Option<(Pick, String)> {
        self.map(|r| (r.pick.clone(), r.detail.clone()))
    }
}

fn accent_text(v: &egui::Visuals) -> Color32 {
    if v.dark_mode { Color32::from_rgb(0x5e, 0xea, 0xd4) } else { Color32::from_rgb(0x0e, 0x74, 0x90) }
}

fn rank_rows<T>(
    query: &str,
    items: &[T],
    key: impl Fn(&T) -> &str,
    pick: impl Fn(&T) -> Pick,
    detail: impl Fn(&T) -> String,
) -> Vec<Row> {
    fuzzy::rank(query.trim(), items, &key, MAX_ROWS)
        .into_iter()
        .map(|(i, _, hits)| Row { label: key(&items[i]).to_owned(), hits, detail: detail(&items[i]), pick: pick(&items[i]) })
        .collect()
}

/// The label with matched chars bold and tinted.
fn highlighted(label: &str, hits: &[usize], fg: Color32, accent: Color32) -> LayoutJob {
    let font = FontId::proportional(14.0);
    let normal = TextFormat::simple(font.clone(), fg);
    let mut hit = TextFormat::simple(font, accent);
    hit.underline = Stroke::new(1.0, accent.gamma_multiply(0.5));
    let mut job = LayoutJob::default();
    let mut buf = String::new();
    let mut in_hit = false;
    for (i, c) in label.chars().enumerate() {
        let h = hits.binary_search(&i).is_ok();
        if h != in_hit && !buf.is_empty() {
            job.append(&std::mem::take(&mut buf), 0.0, if in_hit { hit.clone() } else { normal.clone() });
        }
        in_hit = h;
        buf.push(c);
    }
    job.append(&buf, 0.0, if in_hit { hit } else { normal });
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(text: &str, syntax: &str) -> Context {
        Context {
            text: Arc::new(text.into()),
            syntax: syntax.into(),
            lines: text.lines().count().max(1),
            root: None,
            theme: crate::highlight::DEFAULT_THEME.into(),
            open: vec![PathBuf::from("/tmp/x/main.rs"), PathBuf::from("/tmp/x/notes.md")],
        }
    }

    fn palette(mode: Mode, prefix: &str, c: Context) -> Palette {
        let mut p = Palette::new(&[("duplicate_lines", "Edit: Duplicate Line", "⇧⌘D")]);
        p.show_mode(mode, prefix, c, &egui::Context::default());
        p.rebuild();
        p
    }

    #[test]
    fn commands_include_app_editor_and_syntax_entries() {
        let mut p = palette(Mode::Commands, "", ctx("", "Plain Text"));
        p.text = "dupl".into();
        p.rebuild();
        assert_eq!(p.rows[0].label, "Edit: Duplicate Line");
        p.text = "save all".into();
        p.rebuild();
        assert_eq!(p.rows[0].label, "File: Save All");
        p.text = "set syntax rust".into();
        p.rebuild();
        assert!(p.rows[0].label.starts_with("Set Syntax: Rust"));
    }

    #[test]
    fn goto_line_and_symbol_queries() {
        let text = "fn alpha() {}\nstruct Beta;\nfn gamma() {}\n";
        let mut p = palette(Mode::Files, ":2", ctx(text, "Rust"));
        assert!(matches!(p.rows[0].pick, Pick::Line(1)));
        p.text = ":99".into();
        p.rebuild();
        assert!(matches!(p.rows[0].pick, Pick::Line(2)), "clamped to the last line");
        p.text = "@gam".into();
        p.rebuild();
        assert_eq!(p.rows[0].label, "gamma");
        assert!(matches!(p.rows[0].pick, Pick::Line(2)));
    }

    #[test]
    fn files_without_a_folder_come_from_open_tabs_and_keep_line_suffix() {
        let mut p = palette(Mode::Files, "", ctx("", "Plain Text"));
        p.text = "main:12".into();
        p.rebuild();
        assert!(p.rows[0].label.ends_with("main.rs"));
        let events = p.finish(p.rows.first().cloned_pick().unwrap());
        assert_eq!(events, vec![Event::Open("/tmp/x/main.rs".into()), Event::GotoLine { line: 11, preview: false }]);
        assert!(!p.open);
    }
}
