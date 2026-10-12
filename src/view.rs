//! The editor widget: paints only visible lines, owns scrolling, and turns
//! keyboard/mouse input into `Editor` commands.

use crate::editor::Editor;
use crate::highlight::{Highlighter, Span};
use crate::lang::{self, CommentTokens};
use crate::wrap::{self, LineRows, WrapIndex};
use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{
    Align2, Color32, CursorIcon, Event, EventFilter, FontId, Galley, Id, Key, Modifiers,
    ImeEvent, PointerButton, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2,
};
use std::ops::Range;
use std::sync::Arc;

const SCROLLBAR_W: f32 = 10.0;
const MINIMAP_W: f32 = 110.0;
/// Minimap scale: pixels per line and per column.
const MM_ROW: f32 = 2.0;
const MM_COL: f32 = 1.0;
/// Caret blink half-period, and how long it blinks after the last activity.
const BLINK: f64 = 0.53;
const BLINK_FOR: f64 = 15.0;

pub struct EditorView {
    /// Distinct per view, so several editors can share a window (split panes).
    pub id: Id,
    scroll: Vec2,
    pub font_size: f32,
    /// Widest line seen so far; bounds horizontal scrolling.
    max_width: f32,
    scroll_to_cursor: bool,
    /// Center the cursor line if it's off screen (for jumps, not arrow keys).
    center_cursor: bool,
    wants_focus: bool,
    /// IME composition in progress (shown at the primary caret, not yet in the buffer).
    preedit: String,
    /// Screen rect of the primary caret, for placing the IME candidate window.
    caret_rect: Option<Rect>,
    /// Box-selection drag origin as (line, display column).
    box_from: Option<(usize, usize)>,
    /// Cmd/Ctrl+K pressed; the next key completes a chord.
    chord_k: bool,
    pub minimap: bool,
    /// Caret blink restarts whenever the text or cursor changes.
    blink_epoch: f64,
    blink_key: (u64, usize, usize),
    caret_on: bool,
    /// Soft wrap at the window edge (inactive on huge buffers, see `wrap_active`).
    pub wrap: bool,
    wrap_index: WrapIndex,
    /// Wrap width of the last frame, to keep the top line in place when it changes.
    last_wrap: Option<usize>,
    /// First line shown last frame.
    top_line: usize,
}

/// Per-frame geometry shared by input handling and painting.
struct Metrics {
    full: Rect,
    font: FontId,
    row_h: f32,
    char_w: f32,
    gutter: Rect,
    text: Rect,
    /// Screen x of display column 0, already scrolled.
    origin_x: f32,
    minimap: Option<Rect>,
    /// Wrap width in columns while wrapping.
    wrap: Option<usize>,
}

impl Metrics {
    fn row_top(&self, row: usize, scroll_y: f32) -> f32 {
        self.text.top() + row as f32 * self.row_h - scroll_y
    }

    fn line_rows(&self, ed: &Editor, line: usize) -> LineRows {
        LineRows::new(ed.buffer.line_chars(line), self.wrap, ed.tab_width)
    }
}

impl EditorView {
    pub fn new() -> Self {
        Self {
            id: Id::new("strata-editor"),
            scroll: Vec2::ZERO,
            font_size: 14.0,
            max_width: 0.0,
            scroll_to_cursor: false,
            center_cursor: false,
            wants_focus: true,
            preedit: String::new(),
            caret_rect: None,
            box_from: None,
            chord_k: false,
            minimap: true,
            blink_epoch: 0.0,
            blink_key: (0, 0, 0),
            caret_on: true,
            wrap: false,
            wrap_index: WrapIndex::default(),
            last_wrap: None,
            top_line: 0,
        }
    }

    /// Wrap is on and the buffer is small enough to wrap.
    pub fn wrap_active(&self, ed: &Editor) -> bool {
        self.wrap && ed.buffer.len_chars() <= wrap::MAX_CHARS
    }

    /// Scroll the primary cursor into view on the next frame.
    pub fn reveal(&mut self) {
        self.scroll_to_cursor = true;
    }

    /// Like `reveal`, but an off-screen cursor lands mid-screen (search jumps).
    pub fn reveal_centered(&mut self) {
        self.scroll_to_cursor = true;
        self.center_cursor = true;
    }

    pub fn has_focus(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.has_focus(self.id))
    }

    /// Give the editor keyboard focus on the next frame.
    pub fn focus(&mut self) {
        self.wants_focus = true;
    }

    /// Withdraw a pending `focus()` (another pane took it).
    pub fn cancel_focus(&mut self) {
        self.wants_focus = false;
    }

    /// `matches`: sorted find results to outline (empty when find is closed).
    pub fn show(&mut self, ui: &mut Ui, ed: &mut Editor, hl: &mut Highlighter, matches: &[Range<usize>]) {
        let id = self.id;
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let resp = ui.interact(rect, id, Sense::click_and_drag());

        if self.wants_focus || resp.clicked() || resp.drag_started() {
            resp.request_focus();
            self.wants_focus = false;
        }
        if resp.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
                )
            });
        }

        let mut m = self.metrics(ui, ed, rect);
        if resp.has_focus() {
            self.handle_keys(ui, ed, &m, lang::comment_tokens(hl.syntax_name()));
        }
        if let Some(line) = ed.take_changed_from() {
            hl.invalidate_from(line);
        }
        if let Some(cols) = m.wrap {
            self.wrap_index.update(&ed.buffer, ed.version(), cols, ed.tab_width);
        }
        if m.wrap != self.last_wrap {
            self.last_wrap = m.wrap;
            self.scroll.y = self.line_row(&m, self.top_line) as f32 * m.row_h;
        }
        self.handle_mouse(ui, ed, &resp, &m);
        if resp.hovered() {
            self.scroll -= ui.input(|i| i.smooth_scroll_delta);
        }
        if resp.hovered() && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| m.text.contains(p)) {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
        self.scrollbar(ui, ed, &m, id);
        self.minimap_input(ui, ed, &m, id);
        if std::mem::take(&mut self.scroll_to_cursor) {
            self.reveal_cursor(ui, ed, &m, hl);
        }
        self.clamp_scroll(ed, &m);
        m.origin_x = m.text.left() + m.char_w - self.scroll.x;
        self.top_line = self.row_line(ed, &m, (self.scroll.y / m.row_h) as usize).0;
        self.blink(ui, ed, resp.has_focus());

        self.paint(ui, ed, hl, &m, resp.has_focus(), matches);
        self.paint_minimap(ui, ed, hl, &m);

        if resp.has_focus() {
            // Enables the OS input method and positions its candidate window.
            if let Some(cursor_rect) = self.caret_rect {
                ui.output_mut(|o| {
                    o.ime = Some(egui::output::IMEOutput {
                        purpose: egui::IMEPurpose::Normal,
                        rect: m.text,
                        cursor_rect,
                        should_interrupt_composition: false,
                    })
                });
            }
        } else {
            self.preedit.clear();
        }
    }

    fn metrics(&self, ui: &Ui, ed: &Editor, rect: Rect) -> Metrics {
        let font = FontId::monospace(self.font_size);
        let (row_h, char_w) = ui.fonts_mut(|f| (f.row_height(&font), f.glyph_width(&font, 'M')));
        let digits = ed.buffer.len_lines().to_string().len().max(3);
        let gutter_w = (digits as f32 + 3.0) * char_w;
        let gutter = Rect::from_min_size(rect.min, vec2(gutter_w, rect.height()));
        // The minimap hides itself in narrow windows.
        let mm_w = if self.minimap && rect.width() > 640.0 { MINIMAP_W } else { 0.0 };
        let text = Rect::from_min_max(
            pos2(rect.left() + gutter_w, rect.top()),
            pos2(rect.right() - SCROLLBAR_W - mm_w, rect.bottom()),
        );
        let minimap = (mm_w > 0.0).then(|| Rect::from_min_max(pos2(text.right(), rect.top()), pos2(text.right() + mm_w, rect.bottom())));
        // Text starts one column in; keep one more free at the right edge.
        let wrap = self.wrap_active(ed).then(|| ((text.width() / char_w).floor() as usize).saturating_sub(2).max(20));
        let scroll_x = if wrap.is_some() { 0.0 } else { self.scroll.x };
        Metrics { full: rect, font, row_h, char_w, gutter, text, origin_x: text.left() + char_w - scroll_x, minimap, wrap }
    }

    // ---- visual rows (one per line without wrap) --------------------------

    fn total_rows(&self, ed: &Editor, m: &Metrics) -> usize {
        if m.wrap.is_some() { self.wrap_index.rows() } else { ed.buffer.len_lines() }
    }

    /// First visual row of `line`.
    fn line_row(&self, m: &Metrics, line: usize) -> usize {
        if m.wrap.is_some() { self.wrap_index.first_row(line) } else { line }
    }

    /// (line, row within the line) of visual row `row`.
    fn row_line(&self, ed: &Editor, m: &Metrics, row: usize) -> (usize, usize) {
        if m.wrap.is_some() {
            self.wrap_index.line_of_row(row)
        } else {
            (row.min(ed.buffer.len_lines() - 1), 0)
        }
    }

    /// Visual row of the caret at `pos`.
    fn pos_row(&self, ed: &Editor, m: &Metrics, pos: usize) -> usize {
        let line = ed.buffer.line_of(pos);
        if m.wrap.is_none() {
            return line;
        }
        self.line_row(m, line) + m.line_rows(ed, line).row_of(pos - ed.buffer.line_start(line))
    }

    fn page_lines(m: &Metrics) -> isize {
        ((m.text.height() / m.row_h) as isize - 1).max(1)
    }

    // ---- input -----------------------------------------------------------

    fn handle_keys(&mut self, ui: &Ui, ed: &mut Editor, m: &Metrics, comments: CommentTokens) {
        let events = ui.input(|i| i.events.clone());
        for event in events {
            match event {
                Event::Text(t) if !t.is_empty() && t != "\n" && t != "\r" => ed.type_char(&t),
                Event::Ime(ime) => {
                    if !self.handle_ime(ed, ime) {
                        continue;
                    }
                }
                // Keys mid-composition belong to the input method.
                Event::Key { .. } if !self.preedit.is_empty() => continue,
                Event::Paste(t) => ed.paste(&t),
                Event::Copy => ui.ctx().copy_text(ed.copy()),
                Event::Cut => ui.ctx().copy_text(ed.cut()),
                Event::Key { key, pressed: true, modifiers, .. } => {
                    if !self.handle_key(ed, key, modifiers, m, comments) {
                        continue;
                    }
                }
                _ => continue,
            }
            self.scroll_to_cursor = true;
        }
    }

    /// Returns true if the buffer changed.
    fn handle_ime(&mut self, ed: &mut Editor, ime: ImeEvent) -> bool {
        match ime {
            // Some platforms send empty events when no composition is active.
            ImeEvent::Preedit { text, .. } | ImeEvent::Commit(text)
                if text.is_empty() && self.preedit.is_empty() =>
            {
                false
            }
            ImeEvent::Preedit { text, .. } | ImeEvent::Commit(text) if text == "\n" || text == "\r" => false,
            ImeEvent::Preedit { text, .. } => {
                self.preedit = text;
                false
            }
            ImeEvent::Commit(text) => {
                self.preedit.clear();
                if !text.is_empty() {
                    ed.insert_text(&text);
                }
                true
            }
            _ => false,
        }
    }

    /// Returns true if the key did something.
    fn handle_key(&mut self, ed: &mut Editor, key: Key, mods: Modifiers, m: &Metrics, comments: CommentTokens) -> bool {
        let mac = cfg!(target_os = "macos");
        // Word jumps: Option on macOS, Ctrl on Windows. Line jumps: Cmd on macOS.
        let word = if mac { mods.alt } else { mods.ctrl };
        let cmd = mac && mods.mac_cmd;
        let shift = mods.shift;
        // Move lines: Ctrl+Cmd on macOS, Ctrl+Shift elsewhere.
        let move_lines = if mac { mods.mac_cmd && mods.ctrl } else { mods.ctrl && shift };
        // Add cursor above/below: Ctrl+Shift on macOS, Ctrl+Alt elsewhere.
        let add_cursor = if mac { mods.ctrl && shift && !mods.mac_cmd } else { mods.ctrl && mods.alt };
        // Sort lines: F5 on macOS, F9 elsewhere; with Ctrl, case-sensitive.
        let sort_key = if mac { Key::F5 } else { Key::F9 };
        if std::mem::take(&mut self.chord_k) && mods.command {
            match key {
                Key::U => ed.swap_case_upper(),
                Key::L => ed.swap_case_lower(),
                _ => return false,
            }
            return true;
        }
        match key {
            Key::ArrowUp | Key::ArrowDown if move_lines => ed.move_lines(key == Key::ArrowUp),
            Key::ArrowUp if add_cursor => ed.add_cursor_vertical(-1),
            Key::ArrowDown if add_cursor => ed.add_cursor_vertical(1),
            Key::ArrowLeft if cmd => ed.move_home(shift),
            Key::ArrowRight if cmd => ed.move_end(shift),
            Key::ArrowUp if cmd => ed.move_doc_start(shift),
            Key::ArrowDown if cmd => ed.move_doc_end(shift),
            Key::ArrowLeft => ed.move_left(word, shift),
            Key::ArrowRight => ed.move_right(word, shift),
            Key::ArrowUp => move_rows(ed, m, -1, shift),
            Key::ArrowDown => move_rows(ed, m, 1, shift),
            Key::Home if mods.command => ed.move_doc_start(shift),
            Key::End if mods.command => ed.move_doc_end(shift),
            Key::Home => ed.move_home(shift),
            Key::End => ed.move_end(shift),
            Key::PageUp | Key::PageDown => {
                let n = Self::page_lines(m) * if key == Key::PageUp { -1 } else { 1 };
                move_rows(ed, m, n, shift);
                self.scroll.y += n as f32 * m.row_h;
            }
            Key::Enter if mods.command && shift => ed.insert_line_before(),
            Key::Enter if mods.command => ed.insert_line_after(),
            Key::Enter => ed.newline(),
            Key::Tab if shift => ed.outdent(),
            Key::Tab if ed.selections().iter().any(|s| ed.buffer.line_of(s.start()) != ed.buffer.line_of(s.end())) => {
                ed.indent()
            }
            Key::Tab => ed.tab(),
            Key::CloseBracket if mods.command && !shift => ed.indent(),
            Key::OpenBracket if mods.command && !shift => ed.outdent(),
            Key::Slash if mods.command => match comments {
                (Some(token), _) => ed.toggle_comment(token),
                (None, Some((open, close))) => ed.toggle_block_comment(open, close),
                (None, None) => return false,
            },
            Key::D if mods.command && shift => ed.duplicate_lines(),
            Key::K if mods.ctrl && shift => ed.delete_lines(),
            Key::K if mods.command => {
                self.chord_k = true;
                return false;
            }
            k if k == sort_key => ed.sort_lines(mods.ctrl),
            Key::J if mods.command => ed.join_lines(),
            Key::L if mods.command && shift => ed.split_into_lines(),
            Key::L if mods.command => ed.expand_to_line(),
            Key::M if mods.ctrl && shift => ed.expand_to_brackets(),
            Key::M if mods.ctrl => ed.jump_to_bracket(),
            Key::Backspace if cmd => ed.delete_to_line_start(),
            Key::Backspace => ed.backspace(word),
            Key::Delete => ed.delete_forward(word),
            Key::Escape => ed.collapse_to_primary(),
            Key::A if mods.command => ed.select_all(),
            Key::D if mods.command => ed.select_next_occurrence(),
            // Find all occurrences: Ctrl+Cmd+G on macOS, Alt+F3 elsewhere.
            Key::G if mac && mods.mac_cmd && mods.ctrl => ed.select_all_occurrences(),
            Key::F3 if !mac && mods.alt => ed.select_all_occurrences(),
            Key::Z if mods.command && shift => ed.redo(),
            Key::Z if mods.command => ed.undo(),
            Key::Y if mods.command && !mac => ed.redo(),
            _ => return false,
        }
        true
    }

    fn handle_mouse(&mut self, ui: &Ui, ed: &mut Editor, resp: &egui::Response, m: &Metrics) {
        let Some(pointer) = resp.interact_pointer_pos() else {
            self.box_from = None;
            return;
        };
        let (pressed, middle, mods) = ui.input(|i| {
            let p = &i.pointer;
            (p.button_pressed(PointerButton::Primary), p.button_pressed(PointerButton::Middle), i.modifiers)
        });
        // Box selection: Alt+drag or middle-button drag.
        if ((pressed && mods.alt) || middle) && resp.is_pointer_button_down_on() {
            let (line, col) = self.pos_to_line_col(ui, ed, m, pointer);
            self.box_from = Some((line, col));
            ed.select_box(line, col, line, col);
            return;
        }
        if let Some((line, col)) = self.box_from {
            if resp.dragged() {
                let (to_line, to_col) = self.pos_to_line_col(ui, ed, m, pointer);
                ed.select_box(line, col, to_line, to_col);
                self.autoscroll(ui, m, pointer);
            } else if !resp.is_pointer_button_down_on() {
                self.box_from = None;
            }
            return;
        }
        let pos = self.pos_to_char(ui, ed, m, pointer);
        if resp.triple_clicked() {
            let line = ed.buffer.line_of(pos);
            ed.click(ed.buffer.line_start(line), false);
            ed.move_vertical(1, true);
        } else if resp.double_clicked() {
            ed.select_word_at(pos);
        } else if pressed && resp.is_pointer_button_down_on() {
            if !self.preedit.is_empty() {
                self.preedit.clear();
                ui.memory_mut(|mem| mem.interrupt_ime());
            }
            if mods.command {
                ed.add_cursor(pos);
            } else {
                ed.click(pos, mods.shift);
            }
        } else if resp.dragged() {
            ed.drag_to(pos);
            self.autoscroll(ui, m, pointer);
        }
    }

    /// Scroll while dragging past the top or bottom edge.
    fn autoscroll(&mut self, ui: &Ui, m: &Metrics, pointer: Pos2) {
        if pointer.y < m.text.top() {
            self.scroll.y -= m.row_h;
        } else if pointer.y > m.text.bottom() {
            self.scroll.y += m.row_h;
        }
        ui.ctx().request_repaint();
    }

    /// Visual row under screen y (negative above the first row).
    fn row_at(&self, m: &Metrics, y: f32) -> f32 {
        ((y - m.text.top() + self.scroll.y) / m.row_h).floor()
    }

    /// The line and its rows at visual row `row`, which row of the line it is,
    /// and that row laid out plain.
    fn hit_row(&self, ui: &Ui, ed: &Editor, m: &Metrics, row: usize) -> (usize, LineRows, usize, Arc<Galley>) {
        let (line, sub) = self.row_line(ed, m, row.min(self.total_rows(ed, m) - 1));
        let text = ed.buffer.line_text(line);
        let lr = LineRows::new(text.chars(), m.wrap, ed.tab_width);
        let sub = sub.min(lr.rows() - 1);
        let r = lr.range(sub);
        let bytes = byte_offsets(&text, &[r.start, r.end]);
        let style = Styled { text: &text, spans: None, font: &m.font, fg: Color32::WHITE, tab: ed.tab_width };
        let galley = layout_segment(ui, &style, bytes[0]..bytes[1], lr.dcols[r.start], None);
        (line, lr, sub, galley)
    }

    /// Line and display column under `p`, extending past line ends.
    fn pos_to_line_col(&self, ui: &Ui, ed: &Editor, m: &Metrics, p: Pos2) -> (usize, usize) {
        let row = self.row_at(m, p.y).max(0.0) as usize;
        let (line, lr, sub, galley) = self.hit_row(ui, ed, m, row);
        let r = lr.range(sub);
        let x = p.x - m.origin_x;
        let col = if x <= galley.size().x {
            galley.cursor_from_pos(vec2(x, m.row_h * 0.5)).index.0
        } else {
            lr.dcols[r.end] - lr.dcols[r.start] + ((x - galley.size().x) / m.char_w).round() as usize
        };
        (line, lr.dcols[r.start] + col)
    }

    fn pos_to_char(&self, ui: &Ui, ed: &Editor, m: &Metrics, p: Pos2) -> usize {
        let row = self.row_at(m, p.y);
        if row < 0.0 {
            return 0;
        }
        let (line, lr, sub, galley) = self.hit_row(ui, ed, m, row as usize);
        let col = galley.cursor_from_pos(vec2(p.x - m.origin_x, m.row_h * 0.5)).index.0;
        ed.buffer.line_start(line) + lr.off_at_col(sub, col)
    }

    // ---- scrolling -------------------------------------------------------

    fn content_height(&self, ed: &Editor, m: &Metrics) -> f32 {
        // Allow scrolling the last line up to the top, like Sublime.
        (self.total_rows(ed, m).saturating_sub(1)) as f32 * m.row_h + m.text.height()
    }

    fn clamp_scroll(&mut self, ed: &Editor, m: &Metrics) {
        let max_y = (self.content_height(ed, m) - m.text.height()).max(0.0);
        let max_x = if m.wrap.is_some() { 0.0 } else { (self.max_width + m.char_w * 4.0 - m.text.width()).max(0.0) };
        self.scroll.y = self.scroll.y.clamp(0.0, max_y);
        self.scroll.x = self.scroll.x.clamp(0.0, max_x);
    }

    fn reveal_cursor(&mut self, ui: &Ui, ed: &Editor, m: &Metrics, hl: &Highlighter) {
        let head = ed.primary().head;
        let line = ed.buffer.line_of(head);
        let top = self.pos_row(ed, m, head) as f32 * m.row_h;
        let off_screen = top < self.scroll.y || top + m.row_h > self.scroll.y + m.text.height();
        if std::mem::take(&mut self.center_cursor) && off_screen {
            self.scroll.y = top - (m.text.height() - m.row_h) / 2.0;
        } else if top < self.scroll.y {
            self.scroll.y = top;
        } else if top + m.row_h > self.scroll.y + m.text.height() {
            self.scroll.y = top + m.row_h - m.text.height();
        }
        if m.wrap.is_some() {
            return;
        }
        let text = ed.buffer.line_text(line);
        let galley = layout_line(ui, &text, None, &m.font, hl.foreground(), ed.tab_width);
        let x = galley.pos_from_cursor(CCursor::new(ed.display_col(head))).min.x + m.char_w;
        self.max_width = self.max_width.max(galley.size().x);
        let margin = m.char_w * 4.0;
        if x - margin < self.scroll.x {
            self.scroll.x = (x - margin).max(0.0);
        } else if x + margin > self.scroll.x + m.text.width() {
            self.scroll.x = x + margin - m.text.width();
        }
    }

    fn scrollbar(&mut self, ui: &Ui, ed: &Editor, m: &Metrics, id: Id) {
        let track = Rect::from_min_max(pos2(m.full.right() - SCROLLBAR_W, m.text.top()), pos2(m.full.right(), m.text.bottom()));
        let content = self.content_height(ed, m);
        let h = m.text.height();
        if content <= h + 1.0 {
            return;
        }
        let thumb_h = (h * h / content).max(24.0);
        let resp = ui.interact(track, id.with("vscroll"), Sense::click_and_drag());
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.y - track.top() - thumb_h / 2.0) / (h - thumb_h)).clamp(0.0, 1.0);
            self.scroll.y = t * (content - h);
        }
        let t = (self.scroll.y / (content - h)).clamp(0.0, 1.0);
        let thumb = Rect::from_min_size(pos2(track.left() + 2.0, track.top() + t * (h - thumb_h)), vec2(SCROLLBAR_W - 4.0, thumb_h));
        let alpha = if resp.hovered() || resp.dragged() { 110 } else { 60 };
        ui.painter().rect_filled(thumb, 3.0, Color32::from_white_alpha(alpha));
    }

    // ---- painting --------------------------------------------------------

    fn paint(&mut self, ui: &Ui, ed: &Editor, hl: &mut Highlighter, m: &Metrics, focused: bool, matches: &[Range<usize>]) {
        let painter = ui.painter();
        let text_painter = ui.painter_at(m.text);
        painter.rect_filled(m.full, 0.0, hl.background());

        let total = self.total_rows(ed, m);
        let first_row = ((self.scroll.y / m.row_h).floor() as usize).min(total - 1);
        let last_row = (first_row + (m.text.height() / m.row_h).ceil() as usize).min(total - 1);
        let (first, first_sub) = self.row_line(ed, m, first_row);
        let last = self.row_line(ed, m, last_row).0;
        if !hl.prepare(&ed.buffer, first..=last) {
            ui.ctx().request_repaint();
        }

        let fg = hl.foreground();
        let sel_color = hl.selection();
        let caret_color = hl.caret();
        let first_char = ed.buffer.line_start(first);
        let mut next_match = matches.partition_point(|r| r.end < first_char);
        let caret_lines: Vec<usize> = ed.selections().iter().map(|s| ed.buffer.line_of(s.head)).collect();
        let primary_head = ed.primary().head;
        let primary_line = ed.buffer.line_of(primary_head);
        let preedit_chars = self.preedit.chars().count();
        self.caret_rect = None;
        crate::fonts::ensure_cjk(ui.ctx(), &self.preedit);
        let line_hl = hl.line_highlight();
        let guide = hl.guide();
        let gutter_fg = hl.gutter_foreground().unwrap_or_else(|| fg.gamma_multiply(0.35));
        let bracket = ed.matching_bracket(primary_head).filter(|_| ed.primary().is_empty());
        let word = selected_text(ed);
        let tab = ed.tab_width.max(1);
        // Indent of the last non-blank line, so guides run through blank lines.
        let mut indent_above = 0;
        let all_empty = ed.selections().iter().all(|s| s.is_empty());

        let (mut row, mut line, mut sub0) = (first_row, first, first_sub);
        while row <= last_row && line < ed.buffer.len_lines() {
            let text = ed.buffer.line_text(line);
            crate::fonts::ensure_cjk(ui.ctx(), &text);
            let spans = hl.line(line);
            let style = Styled { text: &text, spans: spans.as_deref().map(|v| v.as_slice()), font: &m.font, fg, tab };
            let lr = LineRows::new(text.chars(), m.wrap, tab);
            let mut bytes = byte_offsets(&text, &lr.starts);
            bytes.push(text.len());
            let line_start = ed.buffer.line_start(line);
            let line_end = line_start + lr.len();
            let on_caret_line = caret_lines.contains(&line);

            // Indent guides at each tab stop inside the leading whitespace.
            let indent = if text.trim().is_empty() {
                indent_above
            } else {
                let mut col = 0;
                for c in text.chars().take_while(|c| *c == ' ' || *c == '\t') {
                    col = if c == '\t' { col + tab - col % tab } else { col + 1 };
                }
                indent_above = col;
                col
            };

            // Other occurrences of the selected text, to be outlined faintly.
            let occurrences: Vec<(usize, usize)> = word.as_ref().map_or_else(Vec::new, |w| {
                text.match_indices(w.as_str())
                    .map(|(byte, _)| {
                        let a = line_start + text[..byte].chars().count();
                        (a, a + w.chars().count())
                    })
                    .filter(|&(a, b)| !ed.selections().iter().any(|s| s.start() == a && s.end() == b))
                    .collect()
            });

            for sub in sub0..lr.rows() {
                if row > last_row {
                    break;
                }
                let y = m.row_top(row, self.scroll.y);
                row += 1;
                let rect = Rect::from_min_size(pos2(m.text.left(), y), vec2(m.text.width(), m.row_h));
                let r = lr.range(sub);
                let last_sub = lr.is_last(sub);
                let (row_start, row_end) = (line_start + r.start, line_start + r.end);
                let in_row = |pos: usize| pos >= row_start && (pos < row_end || (last_sub && pos == row_end));
                let dcol0 = lr.dcols[r.start];
                // The IME composition is drawn inline at the primary caret.
                let composing = line == primary_line && preedit_chars > 0 && in_row(primary_head);
                let insert = composing.then(|| {
                    let byte = byte_offsets(&text, &[primary_head - line_start])[0];
                    Insert { byte, text: &self.preedit }
                });
                let galley = layout_segment(ui, &style, bytes[sub]..bytes[sub + 1], dcol0, insert);
                if m.wrap.is_none() {
                    self.max_width = self.max_width.max(galley.size().x);
                }

                if on_caret_line && all_empty {
                    text_painter.rect_filled(rect, 0.0, line_hl);
                }
                if sub == 0 {
                    for k in (tab..indent).step_by(tab) {
                        let x = (m.origin_x + k as f32 * m.char_w).round() + 0.5;
                        text_painter.line_segment([pos2(x, y), pos2(x, y + m.row_h)], Stroke::new(1.0, guide));
                    }
                }

                let x_at = |pos: usize| {
                    let shift = if composing && pos >= primary_head { preedit_chars } else { 0 };
                    let dcol = lr.dcols[pos.saturating_sub(line_start).min(lr.len())] - dcol0 + shift;
                    m.origin_x + galley.pos_from_cursor(CCursor::new(dcol)).min.x
                };
                for s in ed.selections() {
                    if s.is_empty() || s.end() < row_start || s.start() > row_end {
                        continue;
                    }
                    let a = s.start().max(row_start);
                    let b = s.end().min(row_end);
                    // Show the selected line break as a little extra width.
                    let extra = if last_sub && s.end() > line_end { m.char_w * 0.6 } else { 0.0 };
                    if a == b && extra == 0.0 {
                        continue;
                    }
                    let r = Rect::from_min_max(pos2(x_at(a), y), pos2(x_at(b) + extra, y + m.row_h));
                    text_painter.rect_filled(r, 2.0, sel_color);
                }

                // Outline find matches on this row (a match may span rows and lines).
                while let Some(r) = matches.get(next_match) {
                    if r.start > row_end {
                        break;
                    }
                    let a = r.start.max(row_start);
                    let b = r.end.min(row_end);
                    if b > a {
                        let rect = Rect::from_min_max(pos2(x_at(a), y), pos2(x_at(b), y + m.row_h));
                        text_painter.rect_stroke(rect, 2.0, Stroke::new(1.0, fg.gamma_multiply(0.7)), egui::StrokeKind::Inside);
                    }
                    if r.end > row_end {
                        break;
                    }
                    next_match += 1;
                }

                for &(a, b) in &occurrences {
                    let (a, b) = (a.max(row_start), b.min(row_end));
                    if b > a {
                        let rect = Rect::from_min_max(pos2(x_at(a), y + 1.0), pos2(x_at(b), y + m.row_h - 1.0));
                        text_painter.rect_stroke(rect, 2.0, Stroke::new(1.0, fg.gamma_multiply(0.3)), egui::StrokeKind::Inside);
                    }
                }

                // The bracket pair around the caret is underlined.
                if let Some((a, b)) = bracket {
                    for p in [a, b] {
                        if p >= row_start && p < row_end {
                            let under = Rect::from_min_max(pos2(x_at(p), y + m.row_h - 2.0), pos2(x_at(p + 1), y + m.row_h - 0.5));
                            text_painter.rect_filled(under, 0.0, caret_color);
                        }
                    }
                }

                text_painter.galley(pos2(m.origin_x, y), galley.clone(), fg);

                for (s, &caret_line) in ed.selections().iter().zip(&caret_lines) {
                    if caret_line != line || !in_row(s.head) {
                        continue;
                    }
                    let x = x_at(s.head);
                    let caret = Rect::from_min_size(pos2(x - 1.0, y), vec2(2.0, m.row_h));
                    if s.head == primary_head {
                        self.caret_rect = Some(caret);
                    }
                    if focused {
                        if self.caret_on {
                            text_painter.rect_filled(caret, 0.0, caret_color);
                        }
                    } else {
                        text_painter.rect_stroke(caret, 0.0, Stroke::new(1.0, caret_color.gamma_multiply(0.4)), egui::StrokeKind::Inside);
                    }
                }

                if sub == 0 {
                    let num_color = if on_caret_line { fg } else { gutter_fg };
                    painter.text(
                        pos2(m.gutter.right() - m.char_w * 1.5, y),
                        Align2::RIGHT_TOP,
                        (line + 1).to_string(),
                        m.font.clone(),
                        num_color,
                    );
                }
            }
            sub0 = 0;
            line += 1;
        }
    }
}

impl EditorView {
    fn blink(&mut self, ui: &Ui, ed: &Editor, focused: bool) {
        let now = ui.input(|i| i.time);
        let key = (ed.version(), ed.primary().head, ed.selections().len());
        if key != self.blink_key || !self.preedit.is_empty() {
            self.blink_key = key;
            self.blink_epoch = now;
        }
        let t = now - self.blink_epoch;
        if !focused || t > BLINK_FOR {
            self.caret_on = true;
            return;
        }
        self.caret_on = ((t / BLINK) as u64).is_multiple_of(2);
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(BLINK - t % BLINK + 0.005));
    }

    /// First line shown in the minimap: it pans in step with the editor.
    fn minimap_first(&self, ed: &Editor, m: &Metrics, mm: Rect) -> usize {
        let rows = self.total_rows(ed, m);
        let cap = (mm.height() / MM_ROW) as usize;
        if rows <= cap {
            return 0;
        }
        let max_scroll = (self.content_height(ed, m) - m.text.height()).max(1.0);
        ((rows - cap) as f32 * (self.scroll.y / max_scroll).clamp(0.0, 1.0)) as usize
    }

    /// Click or drag in the minimap to scroll there.
    fn minimap_input(&mut self, ui: &Ui, ed: &Editor, m: &Metrics, id: Id) {
        let Some(mm) = m.minimap else { return };
        let resp = ui.interact(mm, id.with("minimap"), Sense::click_and_drag());
        if let Some(p) = resp.interact_pointer_pos() {
            let first = self.minimap_first(ed, m, mm);
            let row = first as f32 + (p.y - mm.top()) / MM_ROW;
            let visible = m.text.height() / m.row_h;
            self.scroll.y = (row - visible / 2.0) * m.row_h;
        }
    }

    fn paint_minimap(&self, ui: &Ui, ed: &Editor, hl: &mut Highlighter, m: &Metrics) {
        let Some(mm) = m.minimap else { return };
        let painter = ui.painter_at(mm);
        let fg = hl.foreground();
        painter.rect_filled(mm, 0.0, hl.background());
        let rows = self.total_rows(ed, m);
        let first = self.minimap_first(ed, m, mm);
        let last = (first + (mm.height() / MM_ROW) as usize).min(rows - 1);
        let (first_line, first_sub) = self.row_line(ed, m, first);
        let last_line = self.row_line(ed, m, last).0;
        if !hl.prepare(&ed.buffer, first_line..=last_line) {
            ui.ctx().request_repaint();
        }

        // The part the editor shows.
        let top = mm.top() + (self.scroll.y / m.row_h - first as f32) * MM_ROW;
        let view = Rect::from_min_size(pos2(mm.left(), top), vec2(mm.width(), m.text.height() / m.row_h * MM_ROW));
        painter.rect_filled(view, 0.0, fg.gamma_multiply(0.07));

        let max_cols = ((mm.width() - 8.0) / MM_COL) as usize;
        let tab = ed.tab_width.max(1);
        let (mut row, mut line, mut sub0) = (first, first_line, first_sub);
        while row <= last && line < ed.buffer.len_lines() {
            let text = ed.buffer.line_text(line);
            let spans = hl.line(line);
            let color_at = |byte: usize| {
                spans.as_ref().and_then(|v| v.iter().find(|s| s.range.contains(&byte))).map_or(fg, |s| s.color)
            };
            let starts = if m.wrap.is_some() { LineRows::new(text.chars(), m.wrap, tab).starts } else { vec![0] };
            let mut bytes = byte_offsets(&text, &starts);
            bytes.push(text.len());
            for sub in sub0..starts.len() {
                if row > last {
                    break;
                }
                let y = mm.top() + (row - first) as f32 * MM_ROW;
                row += 1;
                // Runs of non-blank chars of one color become one small bar.
                let mut col = 0;
                let mut run: Option<(usize, Color32)> = None;
                let flush = |run: &mut Option<(usize, Color32)>, end: usize| {
                    if let Some((start, c)) = run.take() {
                        let r = Rect::from_min_max(pos2(mm.left() + 4.0 + start as f32 * MM_COL, y), pos2(mm.left() + 4.0 + end as f32 * MM_COL, y + MM_ROW - 0.6));
                        painter.rect_filled(r, 0.0, c.gamma_multiply(0.75));
                    }
                };
                for (byte, c) in text[bytes[sub]..bytes[sub + 1]].char_indices() {
                    if col >= max_cols {
                        break;
                    }
                    let next = if c == '\t' { col + tab - col % tab } else { col + 1 };
                    if c.is_whitespace() {
                        flush(&mut run, col);
                    } else {
                        let color = color_at(bytes[sub] + byte);
                        if run.is_some_and(|(_, rc)| rc != color) {
                            flush(&mut run, col);
                        }
                        run.get_or_insert((col, color));
                    }
                    col = next;
                }
                flush(&mut run, col.min(max_cols));
            }
            sub0 = 0;
            line += 1;
        }
    }
}

/// Byte offsets of ascending char offsets into `text`.
fn byte_offsets(text: &str, offs: &[usize]) -> Vec<usize> {
    let mut bytes = text.char_indices().map(|(b, _)| b).chain(std::iter::once(text.len()));
    let (mut k, mut b) = (0, bytes.next());
    offs.iter()
        .map(|&o| {
            while k < o {
                b = bytes.next();
                k += 1;
            }
            b.unwrap_or(text.len())
        })
        .collect()
}

/// Move each caret by `n` visual rows, keeping its column within the row
/// (by buffer lines when not wrapping).
fn move_rows(ed: &mut Editor, m: &Metrics, n: isize, extend: bool) {
    if m.wrap.is_none() {
        return ed.move_vertical(n, extend);
    }
    ed.move_heads(extend, |e, s| {
        let mut line = e.buffer.line_of(s.head);
        let mut lr = m.line_rows(e, line);
        let off = s.head - e.buffer.line_start(line);
        let goal = s.goal().unwrap_or_else(|| lr.col_in_row(off));
        let mut sub = lr.row_of(off) as isize + n;
        while sub < 0 {
            if line == 0 {
                return (0, None);
            }
            line -= 1;
            lr = m.line_rows(e, line);
            sub += lr.rows() as isize;
        }
        while sub >= lr.rows() as isize {
            if line + 1 >= e.buffer.len_lines() {
                return (e.buffer.len_chars(), None);
            }
            sub -= lr.rows() as isize;
            line += 1;
            lr = m.line_rows(e, line);
        }
        (e.buffer.line_start(line) + lr.off_at_col(sub as usize, goal), Some(goal))
    });
}

/// The primary selection's text, if it's a short single-line string worth
/// highlighting elsewhere in the view.
fn selected_text(ed: &Editor) -> Option<String> {
    let p = ed.primary();
    if p.is_empty() || p.end() - p.start() > 200 {
        return None;
    }
    let t = ed.buffer.slice(p.range());
    (!t.contains('\n') && !t.trim().is_empty()).then_some(t)
}

/// Text spliced into a line for display only (IME composition).
struct Insert<'a> {
    byte: usize,
    text: &'a str,
}

/// A line's text and how to color it.
struct Styled<'a> {
    text: &'a str,
    spans: Option<&'a [Span]>,
    font: &'a FontId,
    fg: Color32,
    tab: usize,
}

/// Lay out one line with tabs expanded, colored by `spans` when available.
fn layout_line(ui: &Ui, text: &str, spans: Option<&[Span]>, font: &FontId, fg: Color32, tab: usize) -> Arc<Galley> {
    let style = Styled { text, spans, font, fg, tab };
    layout_segment(ui, &style, 0..text.len(), 0, None)
}

/// Lay out the `range` bytes of a line, which start at display column `col`
/// (so tabs land on the line's tab stops). `insert` is in line bytes.
fn layout_segment(ui: &Ui, s: &Styled, range: Range<usize>, col: usize, insert: Option<Insert>) -> Arc<Galley> {
    let (text, font, fg) = (s.text, s.font, s.fg);
    // Flatten into (byte range, format) segments, then splice the insert in.
    let valid = s.spans.filter(|s| !s.is_empty() && s.last().is_none_or(|l| l.range.end <= text.len()));
    let mut segments: Vec<(Range<usize>, TextFormat)> = match valid {
        Some(spans) => spans
            .iter()
            .filter(|span| span.range.end > range.start && span.range.start < range.end)
            .map(|span| {
                let mut fmt = TextFormat::simple(font.clone(), span.color);
                fmt.italics = span.italic;
                if span.underline {
                    fmt.underline = Stroke::new(1.0, span.color);
                }
                (span.range.start.max(range.start)..span.range.end.min(range.end), fmt)
            })
            .collect(),
        None => vec![(range.clone(), TextFormat::simple(font.clone(), fg))],
    };
    let ime = || {
        let mut ime = TextFormat::simple(font.clone(), fg);
        ime.underline = Stroke::new(1.0, fg);
        ime
    };
    let mut pieces: Vec<(&str, TextFormat)> = Vec::with_capacity(segments.len() + 2);
    let mut insert = insert.filter(|i| i.byte >= range.start && i.byte <= range.end);
    for (r, fmt) in segments.drain(..) {
        if let Some(ins) = insert.as_ref().filter(|i| i.byte >= r.start && i.byte <= r.end) {
            pieces.push((&text[r.start..ins.byte], fmt.clone()));
            pieces.push((ins.text, ime()));
            pieces.push((&text[ins.byte..r.end], fmt));
            insert = None;
        } else {
            pieces.push((&text[r], fmt));
        }
    }
    if let Some(ins) = insert {
        pieces.push((ins.text, ime()));
    }

    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let tab = s.tab.max(1);
    let mut col = col;
    let mut push = |job: &mut LayoutJob, piece: &str, fmt: TextFormat| {
        let mut out = String::with_capacity(piece.len());
        for c in piece.chars() {
            if c == '\t' {
                let n = tab - col % tab;
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            } else {
                out.push(c);
                col += 1;
            }
        }
        job.append(&out, 0.0, fmt);
    };
    for (piece, fmt) in pieces {
        if !piece.is_empty() {
            push(&mut job, piece, fmt);
        }
    }
    ui.fonts_mut(|f| f.layout_job(job))
}

#[cfg(test)]
mod tests {
    //! Headless frames: real egui input handling and layout, no window or GPU.
    use super::*;
    use crate::buffer::Buffer;
    use egui::{CentralPanel, Context, RawInput};

    struct Harness {
        ctx: Context,
        view: EditorView,
        ed: Editor,
        hl: Highlighter,
    }

    impl Harness {
        fn new(text: &str) -> Self {
            let mut h = Self {
                ctx: Context::default(),
                view: EditorView::new(),
                ed: Editor::new(Buffer::from_str(text)),
                hl: Highlighter::new(),
            };
            h.frame(vec![]); // takes focus
            h.frame(vec![]);
            h
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.frame_with(events, Modifiers::NONE);
        }

        fn frame_with(&mut self, events: Vec<Event>, modifiers: Modifiers) {
            self.frame_out(events, modifiers);
        }

        fn frame_out(&mut self, mut events: Vec<Event>, modifiers: Modifiers) -> egui::FullOutput {
            events.insert(0, Event::ModifiersChanged(modifiers));
            let raw = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                events,
                ..Default::default()
            };
            let (view, ed, hl) = (&mut self.view, &mut self.ed, &mut self.hl);
            let mut out = self.ctx.run_ui(raw, |ui| {
                CentralPanel::default().show(ui, |ui| view.show(ui, ed, hl, &[]));
            });
            // No GPU here: discard font-atlas uploads instead of applying them.
            out.textures_delta.clear();
            out
        }

        fn text(&self) -> String {
            self.ed.buffer.slice(0..self.ed.buffer.len_chars())
        }

        fn row_h(&self) -> f32 {
            self.ctx.fonts_mut(|f| f.row_height(&FontId::monospace(14.0)))
        }
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    /// Press and release on separate frames, like a real mouse.
    fn click(h: &mut Harness, pos: Pos2, modifiers: Modifiers) {
        let button = |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers };
        h.frame_with(vec![Event::PointerMoved(pos), button(true)], modifiers);
        h.frame_with(vec![button(false)], modifiers);
    }

    #[test]
    fn typing_enter_backspace_and_undo() {
        let mut h = Harness::new("");
        h.frame(vec![
            Event::Text("fn main() {".into()),
            key(Key::Enter, Modifiers::NONE),
            key(Key::Tab, Modifiers::NONE),
            Event::Text("x;".into()),
            key(Key::Backspace, Modifiers::NONE),
        ]);
        let nl = h.ed.buffer.line_ending().as_str();
        assert_eq!(h.text(), format!("fn main() {{{nl}    x"));
        // Deleting is its own undo step, separate from the typing before it.
        h.frame(vec![key(Key::Z, Modifiers::COMMAND)]);
        assert_eq!(h.text(), format!("fn main() {{{nl}    x;"));
        // The indent and "x;" were typed together, so they undo together.
        h.frame(vec![key(Key::Z, Modifiers::COMMAND)]);
        assert_eq!(h.text(), format!("fn main() {{{nl}"));
    }

    #[test]
    fn platform_word_and_line_jumps() {
        let mut h = Harness::new("alpha beta gamma");
        let word = if cfg!(target_os = "macos") { Modifiers::ALT } else { Modifiers::CTRL };
        h.frame(vec![key(Key::ArrowRight, word), key(Key::ArrowRight, word)]);
        assert_eq!(h.ed.primary().head, 10);
        h.frame(vec![key(Key::End, Modifiers::NONE)]);
        assert_eq!(h.ed.primary().head, 16);
        h.frame(vec![key(Key::Home, Modifiers::SHIFT)]);
        assert_eq!(h.ed.primary().range(), 0..16);
    }

    #[test]
    fn click_places_caret_and_command_click_adds_cursor() {
        let mut h = Harness::new("first\nsecond\nthird");
        let row = h.row_h();
        // Far right of line 2 → end of "second".
        click(&mut h, pos2(600.0, row * 1.5), Modifiers::NONE);
        assert_eq!(h.ed.primary().head, 12);
        click(&mut h, pos2(600.0, row * 2.5), Modifiers::COMMAND);
        assert_eq!(h.ed.selections().len(), 2);
        h.frame(vec![Event::Text("!".into())]);
        assert_eq!(h.text(), "first\nsecond!\nthird!");
    }

    #[test]
    fn cmd_d_through_the_widget() {
        let mut h = Harness::new("let a = a + a;");
        h.frame(vec![key(Key::ArrowRight, Modifiers::NONE); 4]);
        h.frame(vec![key(Key::D, Modifiers::COMMAND); 3]);
        assert_eq!(h.ed.selections().len(), 3);
        h.frame(vec![Event::Text("b".into())]);
        assert_eq!(h.text(), "let b = b + b;");
    }

    #[test]
    fn command_slash_comments_rust_line() {
        let mut h = Harness::new("    let x = 1;\n");
        h.hl.detect(Some(std::path::Path::new("main.rs")), "");
        h.frame(vec![key(Key::Slash, Modifiers::COMMAND)]);
        assert_eq!(h.text(), "    // let x = 1;\n");
        h.frame(vec![key(Key::Slash, Modifiers::COMMAND)]);
        assert_eq!(h.text(), "    let x = 1;\n");
    }

    #[test]
    fn command_slash_without_comment_syntax_does_nothing() {
        let mut h = Harness::new("plain\n");
        h.frame(vec![key(Key::Slash, Modifiers::COMMAND)]);
        assert_eq!(h.text(), "plain\n");
    }

    #[test]
    fn tab_indents_multiline_selection_and_shift_tab_outdents() {
        let mut h = Harness::new("a\n        b\n");
        h.frame(vec![key(Key::ArrowDown, Modifiers::SHIFT)]);
        h.frame(vec![key(Key::Tab, Modifiers::NONE)]);
        assert_eq!(h.text(), "    a\n        b\n", "selection ends at column 0 of line 2");
        // Collapse to the selection end (start of line 2), then outdent it.
        h.frame(vec![key(Key::ArrowRight, Modifiers::NONE), key(Key::Tab, Modifiers::SHIFT)]);
        assert_eq!(h.text(), "    a\n    b\n");
    }

    #[test]
    fn typing_auto_pairs_and_types_over_closer() {
        let mut h = Harness::new("");
        h.frame(vec![Event::Text("f(".into()), Event::Text("x".into()), Event::Text(")".into())]);
        assert_eq!(h.text(), "f(x)");
        assert_eq!(h.ed.primary().head, 4);
    }

    #[test]
    fn move_line_and_case_chord_keys() {
        let mut h = Harness::new("one\ntwo");
        let move_mods = if cfg!(target_os = "macos") {
            Modifiers { ctrl: true, mac_cmd: true, command: true, ..Modifiers::NONE }
        } else {
            Modifiers::CTRL | Modifiers::SHIFT
        };
        h.frame(vec![key(Key::ArrowDown, move_mods)]);
        assert_eq!(h.text(), "two\none");
        h.frame(vec![key(Key::K, Modifiers::COMMAND), key(Key::U, Modifiers::COMMAND)]);
        assert_eq!(h.text(), "two\nONE");
    }

    #[test]
    fn ime_composition_then_commit() {
        let mut h = Harness::new("ab");
        h.frame(vec![key(Key::ArrowRight, Modifiers::NONE)]);
        let preedit = |t: &str| Event::Ime(ImeEvent::Preedit { text: t.into(), active_range_chars: None });
        h.frame(vec![preedit("n"), preedit("ni")]);
        assert_eq!(h.text(), "ab", "composition is not in the buffer yet");
        assert_eq!(h.view.preedit, "ni");
        // Keys during composition go to the input method, not the editor.
        h.frame(vec![key(Key::Backspace, Modifiers::NONE)]);
        assert_eq!(h.text(), "ab");
        h.frame(vec![Event::Ime(ImeEvent::Commit("你".into()))]);
        assert_eq!(h.text(), "a你b");
        assert!(h.view.preedit.is_empty());
        assert_eq!(h.ed.primary().head, 2);
    }

    #[test]
    fn focused_editor_requests_ime_at_caret() {
        let mut h = Harness::new("hello");
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            ..Default::default()
        };
        let (view, ed, hl) = (&mut h.view, &mut h.ed, &mut h.hl);
        let mut out = h.ctx.run_ui(raw, |ui| {
            CentralPanel::default().show(ui, |ui| view.show(ui, ed, hl, &[]));
        });
        out.textures_delta.clear();
        let ime = out.platform_output.ime.expect("IME enabled while focused");
        assert!(ime.rect.contains(ime.cursor_rect.center()));
    }

    #[test]
    fn minimap_click_scrolls_and_narrow_windows_hide_it() {
        let text: String = (0..2_000).map(|i| format!("line {i}\n")).collect();
        let mut h = Harness::new(&text);
        // The minimap sits between the text and the scrollbar.
        click(&mut h, pos2(800.0 - 10.0 - 50.0, 590.0), Modifiers::NONE);
        assert!(h.view.scroll.y > 0.0, "clicking low in the minimap scrolls down");
        assert_eq!(h.ed.primary().head, 0, "and doesn't move the caret");
        let mut hidden = false;
        let mut out = h.ctx.run_ui(RawInput::default(), |ui| {
            hidden = h.view.metrics(ui, &h.ed, Rect::from_min_size(Pos2::ZERO, vec2(500.0, 400.0))).minimap.is_none();
        });
        out.textures_delta.clear();
        assert!(hidden);
    }

    #[test]
    fn selected_text_ignores_multiline_and_blank() {
        let mut ed = Editor::new(Buffer::from_str("ab ab\ncd"));
        ed.select_ranges(&[0..2], 0);
        assert_eq!(selected_text(&ed).as_deref(), Some("ab"));
        ed.select_ranges(&[0..7], 0);
        assert_eq!(selected_text(&ed), None);
        ed.select_ranges(&[2..3], 0);
        assert_eq!(selected_text(&ed), None);
    }

    #[test]
    fn caret_stays_visible_in_large_file() {
        let text: String = (0..100_000).map(|i| format!("line {i}\n")).collect();
        let mut h = Harness::new(&text);
        h.frame(vec![key(Key::End, Modifiers::COMMAND)]);
        h.frame(vec![]);
        let row = h.row_h();
        let last = h.ed.buffer.len_lines() - 1;
        assert_eq!(h.ed.buffer.line_of(h.ed.primary().head), last);
        assert!(h.view.scroll.y >= (last as f32 + 1.0) * row - 600.0);
        h.frame(vec![key(Key::Home, Modifiers::COMMAND)]);
        assert_eq!(h.view.scroll.y, 0.0);
    }

    /// A buffer of long lines with wrap on; returns the wrap width.
    fn wrapped(text: &str) -> (Harness, usize) {
        let mut h = Harness::new(text);
        h.view.wrap = true;
        h.frame(vec![]);
        let cols = h.view.last_wrap.expect("wrapping");
        (h, cols)
    }

    fn rows_of(h: &Harness, line: usize, cols: usize) -> LineRows {
        LineRows::new(h.ed.buffer.line_chars(line), Some(cols), h.ed.tab_width)
    }

    fn long_line(words: usize) -> String {
        (0..words).map(|i| format!("word{i:03} ")).collect()
    }

    #[test]
    fn click_on_continuation_row() {
        let (mut h, cols) = wrapped(&long_line(60));
        let lr = rows_of(&h, 0, cols);
        assert!(lr.rows() > 2);
        let caret = h.view.caret_rect.expect("caret painted");
        let row = h.row_h();
        let char_w = h.ctx.fonts_mut(|f| f.glyph_width(&FontId::monospace(14.0), 'M'));
        // Column 3 of the second row.
        click(&mut h, pos2(caret.center().x + char_w * 3.0, caret.top() + row * 1.5), Modifiers::NONE);
        assert_eq!(h.ed.primary().head, lr.starts[1] + 3);
        // Far right of the second row stays on that row (fresh view: no double click).
        let (mut h, _) = wrapped(&long_line(60));
        click(&mut h, pos2(caret.center().x + char_w * (cols as f32 + 0.6), caret.top() + row * 1.5), Modifiers::NONE);
        assert_eq!(h.ed.primary().head, lr.starts[2] - 1);
    }

    #[test]
    fn down_moves_by_visual_row() {
        let text = format!("{}\nshort", long_line(30));
        let (mut h, cols) = wrapped(&text);
        let lr = rows_of(&h, 0, cols);
        h.frame(vec![key(Key::ArrowRight, Modifiers::NONE); 4]);
        h.frame(vec![key(Key::ArrowDown, Modifiers::NONE)]);
        assert_eq!(h.ed.primary().head, lr.starts[1] + 4, "same line, next row, same column");
        h.frame(vec![key(Key::ArrowDown, Modifiers::SHIFT)]);
        assert_eq!(h.ed.primary().range(), lr.starts[1] + 4..lr.starts[2] + 4);
        for _ in 2..lr.rows() {
            h.frame(vec![key(Key::ArrowDown, Modifiers::NONE)]);
        }
        let short = h.ed.buffer.line_start(1);
        assert_eq!(h.ed.primary().head, short + 4, "the next line keeps the goal column");
        h.frame(vec![key(Key::ArrowUp, Modifiers::NONE)]);
        assert_eq!(h.ed.buffer.line_of(h.ed.primary().head), 0);
        assert_eq!(h.ed.primary().head, lr.starts[lr.rows() - 1] + 4);
    }

    #[test]
    fn end_of_buffer_stays_visible_when_wrapped() {
        let text: String = (0..200).map(|_| long_line(40) + "\n").collect::<String>() + &long_line(40);
        let (mut h, _) = wrapped(&text);
        h.frame(vec![key(Key::End, Modifiers::COMMAND)]);
        h.frame(vec![]);
        assert_eq!(h.ed.primary().head, h.ed.buffer.len_chars());
        let caret = h.view.caret_rect.expect("caret painted");
        assert!(caret.bottom() <= 600.5 && caret.top() >= 0.0, "{caret:?}");
        assert_eq!(h.view.scroll.x, 0.0);
        h.frame(vec![key(Key::Home, Modifiers::COMMAND)]);
        assert_eq!(h.view.scroll.y, 0.0);
    }

    #[test]
    fn selection_across_row_boundary_paints_both_rows() {
        let (mut h, cols) = wrapped(&long_line(30));
        let lr = rows_of(&h, 0, cols);
        let sel = h.hl.selection();
        let count_sel_rects = |h: &mut Harness| {
            let out = h.frame_out(vec![], Modifiers::NONE);
            out.shapes.iter().filter(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.fill == sel)).count()
        };
        h.ed.select_ranges(&[lr.starts[1] - 3..lr.starts[1] + 3], 0);
        assert_eq!(count_sel_rects(&mut h), 2);
        h.ed.select_ranges(&[lr.starts[1]..lr.starts[1] + 3], 0);
        assert_eq!(count_sel_rects(&mut h), 1, "a selection starting a row stays on it");
    }

    #[test]
    fn wrap_off_restores_horizontal_scroll() {
        let (mut h, _) = wrapped(&long_line(60));
        h.frame(vec![key(Key::End, Modifiers::NONE)]);
        assert_eq!(h.view.scroll.x, 0.0);
        assert!(h.view.scroll.y >= 0.0);
        h.view.wrap = false;
        h.frame(vec![key(Key::Home, Modifiers::NONE), key(Key::End, Modifiers::NONE)]);
        assert!(h.view.scroll.x > 0.0, "long line scrolls sideways again");
        assert!(h.view.last_wrap.is_none());
    }
}
