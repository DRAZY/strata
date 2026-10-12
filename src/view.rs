//! The editor widget: paints only visible lines, owns scrolling, and turns
//! keyboard/mouse input into `Editor` commands.

use crate::editor::Editor;
use crate::highlight::{Highlighter, Span};
use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{
    Align2, Color32, CursorIcon, Event, EventFilter, FontId, Galley, Id, Key, Modifiers,
    ImeEvent, PointerButton, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2,
};
use std::ops::Range;
use std::sync::Arc;

const SCROLLBAR_W: f32 = 10.0;

pub struct EditorView {
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
}

impl Metrics {
    fn line_top(&self, line: usize, scroll_y: f32) -> f32 {
        self.text.top() + line as f32 * self.row_h - scroll_y
    }
}

impl EditorView {
    pub fn new() -> Self {
        Self {
            scroll: Vec2::ZERO,
            font_size: 14.0,
            max_width: 0.0,
            scroll_to_cursor: false,
            center_cursor: false,
            wants_focus: true,
            preedit: String::new(),
            caret_rect: None,
        }
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

    /// Give the editor keyboard focus on the next frame.
    pub fn focus(&mut self) {
        self.wants_focus = true;
    }

    /// Call after loading a different file.
    pub fn reset(&mut self) {
        *self = Self { font_size: self.font_size, ..Self::new() };
    }

    /// `matches`: sorted find results to outline (empty when find is closed).
    pub fn show(&mut self, ui: &mut Ui, ed: &mut Editor, hl: &mut Highlighter, matches: &[Range<usize>]) {
        let id = Id::new("strata-editor");
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
            self.handle_keys(ui, ed, &m);
        }
        if let Some(line) = ed.take_changed_from() {
            hl.invalidate_from(line);
        }
        self.handle_mouse(ui, ed, &resp, &m);
        if resp.hovered() {
            self.scroll -= ui.input(|i| i.smooth_scroll_delta);
        }
        if resp.hovered() && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| m.text.contains(p)) {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
        self.scrollbar(ui, ed, &m, id);
        if std::mem::take(&mut self.scroll_to_cursor) {
            self.reveal_cursor(ui, ed, &m, hl);
        }
        self.clamp_scroll(ed, &m);
        m.origin_x = m.text.left() + m.char_w - self.scroll.x;

        self.paint(ui, ed, hl, &m, resp.has_focus(), matches);

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
        let text = Rect::from_min_max(
            pos2(rect.left() + gutter_w, rect.top()),
            pos2(rect.right() - SCROLLBAR_W, rect.bottom()),
        );
        Metrics { full: rect, font, row_h, char_w, gutter, text, origin_x: text.left() + char_w - self.scroll.x }
    }

    fn page_lines(m: &Metrics) -> isize {
        ((m.text.height() / m.row_h) as isize - 1).max(1)
    }

    // ---- input -----------------------------------------------------------

    fn handle_keys(&mut self, ui: &Ui, ed: &mut Editor, m: &Metrics) {
        let events = ui.input(|i| i.events.clone());
        for event in events {
            match event {
                Event::Text(t) if !t.is_empty() && t != "\n" && t != "\r" => ed.insert_text(&t),
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
                    if !self.handle_key(ed, key, modifiers, m) {
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
    fn handle_key(&mut self, ed: &mut Editor, key: Key, mods: Modifiers, m: &Metrics) -> bool {
        let mac = cfg!(target_os = "macos");
        // Word jumps: Option on macOS, Ctrl on Windows. Line jumps: Cmd on macOS.
        let word = if mac { mods.alt } else { mods.ctrl };
        let cmd = mac && mods.mac_cmd;
        let shift = mods.shift;
        match key {
            Key::ArrowLeft if cmd => ed.move_home(shift),
            Key::ArrowRight if cmd => ed.move_end(shift),
            Key::ArrowUp if cmd => ed.move_doc_start(shift),
            Key::ArrowDown if cmd => ed.move_doc_end(shift),
            Key::ArrowLeft => ed.move_left(word, shift),
            Key::ArrowRight => ed.move_right(word, shift),
            Key::ArrowUp => ed.move_vertical(-1, shift),
            Key::ArrowDown => ed.move_vertical(1, shift),
            Key::Home if mods.command => ed.move_doc_start(shift),
            Key::End if mods.command => ed.move_doc_end(shift),
            Key::Home => ed.move_home(shift),
            Key::End => ed.move_end(shift),
            Key::PageUp | Key::PageDown => {
                let n = Self::page_lines(m) * if key == Key::PageUp { -1 } else { 1 };
                ed.move_vertical(n, shift);
                self.scroll.y += n as f32 * m.row_h;
            }
            Key::Enter => ed.newline(),
            Key::Tab if !shift => ed.tab(),
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
        let Some(pointer) = resp.interact_pointer_pos() else { return };
        let (pressed, mods) = ui.input(|i| (i.pointer.button_pressed(PointerButton::Primary), i.modifiers));
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
            // Auto-scroll while dragging past the edges.
            if pointer.y < m.text.top() {
                self.scroll.y -= m.row_h;
            } else if pointer.y > m.text.bottom() {
                self.scroll.y += m.row_h;
            }
            ui.ctx().request_repaint();
        }
    }

    fn pos_to_char(&self, ui: &Ui, ed: &Editor, m: &Metrics, p: Pos2) -> usize {
        let raw = ((p.y - m.text.top() + self.scroll.y) / m.row_h).floor();
        let last = ed.buffer.len_lines() - 1;
        if raw < 0.0 {
            return 0;
        }
        let line = (raw as usize).min(last);
        let text = ed.buffer.line_text(line);
        let galley = layout_line(ui, &text, None, &m.font, Color32::WHITE, ed.tab_width);
        let dcol = galley.cursor_from_pos(vec2(p.x - m.origin_x, m.row_h * 0.5)).index.0;
        ed.pos_at_display_col(line, dcol)
    }

    // ---- scrolling -------------------------------------------------------

    fn content_height(ed: &Editor, m: &Metrics) -> f32 {
        // Allow scrolling the last line up to the top, like Sublime.
        (ed.buffer.len_lines().saturating_sub(1)) as f32 * m.row_h + m.text.height()
    }

    fn clamp_scroll(&mut self, ed: &Editor, m: &Metrics) {
        let max_y = (Self::content_height(ed, m) - m.text.height()).max(0.0);
        let max_x = (self.max_width + m.char_w * 4.0 - m.text.width()).max(0.0);
        self.scroll.y = self.scroll.y.clamp(0.0, max_y);
        self.scroll.x = self.scroll.x.clamp(0.0, max_x);
    }

    fn reveal_cursor(&mut self, ui: &Ui, ed: &Editor, m: &Metrics, hl: &Highlighter) {
        let head = ed.primary().head;
        let line = ed.buffer.line_of(head);
        let top = line as f32 * m.row_h;
        let off_screen = top < self.scroll.y || top + m.row_h > self.scroll.y + m.text.height();
        if std::mem::take(&mut self.center_cursor) && off_screen {
            self.scroll.y = top - (m.text.height() - m.row_h) / 2.0;
        } else if top < self.scroll.y {
            self.scroll.y = top;
        } else if top + m.row_h > self.scroll.y + m.text.height() {
            self.scroll.y = top + m.row_h - m.text.height();
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
        let track = Rect::from_min_max(pos2(m.text.right(), m.text.top()), pos2(m.text.right() + SCROLLBAR_W, m.text.bottom()));
        let content = Self::content_height(ed, m);
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

        let lines = ed.buffer.len_lines();
        let first = (self.scroll.y / m.row_h).floor() as usize;
        let last = (first + (m.text.height() / m.row_h).ceil() as usize).min(lines - 1);
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

        for line in first..=last {
            let y = m.line_top(line, self.scroll.y);
            let row = Rect::from_min_size(pos2(m.text.left(), y), vec2(m.text.width(), m.row_h));
            let text = ed.buffer.line_text(line);
            crate::fonts::ensure_cjk(ui.ctx(), &text);
            let spans = hl.line(line);
            let line_start = ed.buffer.line_start(line);
            // The IME composition is drawn inline at the primary caret.
            let composing = line == primary_line && preedit_chars > 0;
            let insert = composing.then(|| {
                let byte = text.char_indices().nth(primary_head - line_start).map_or(text.len(), |(b, _)| b);
                Insert { byte, text: &self.preedit }
            });
            let galley = layout_line_with(ui, &text, spans.as_deref().map(|v| v.as_slice()), &m.font, fg, ed.tab_width, insert);
            self.max_width = self.max_width.max(galley.size().x);

            let on_caret_line = caret_lines.contains(&line);
            if on_caret_line && ed.selections().iter().all(|s| s.is_empty()) {
                text_painter.rect_filled(row, 0.0, Color32::from_white_alpha(8));
            }

            let line_end = ed.buffer.line_end(line);
            let x_at = |pos: usize| {
                let shift = if composing && pos >= primary_head { preedit_chars } else { 0 };
                let dcol = ed.display_col(pos) + shift;
                m.origin_x + galley.pos_from_cursor(CCursor::new(dcol)).min.x
            };
            for s in ed.selections() {
                if s.is_empty() || s.end() < line_start || s.start() > line_end {
                    continue;
                }
                let a = s.start().max(line_start);
                let b = s.end().min(line_end);
                // Show the selected line break as a little extra width.
                let extra = if s.end() > line_end { m.char_w * 0.6 } else { 0.0 };
                let r = Rect::from_min_max(pos2(x_at(a), y), pos2(x_at(b) + extra, y + m.row_h));
                text_painter.rect_filled(r, 2.0, sel_color);
            }

            // Outline find matches on this line (a match may span lines).
            while let Some(r) = matches.get(next_match) {
                if r.start > line_end {
                    break;
                }
                let a = r.start.max(line_start);
                let b = r.end.min(line_end);
                if b > a {
                    let rect = Rect::from_min_max(pos2(x_at(a), y), pos2(x_at(b), y + m.row_h));
                    text_painter.rect_stroke(rect, 2.0, Stroke::new(1.0, fg.gamma_multiply(0.7)), egui::StrokeKind::Inside);
                }
                if r.end > line_end {
                    break;
                }
                next_match += 1;
            }

            text_painter.galley(pos2(m.origin_x, y), galley.clone(), fg);

            for s in ed.selections() {
                if ed.buffer.line_of(s.head) != line {
                    continue;
                }
                let x = x_at(s.head);
                let caret = Rect::from_min_size(pos2(x - 1.0, y), vec2(2.0, m.row_h));
                if s.head == primary_head {
                    self.caret_rect = Some(caret);
                }
                if focused {
                    text_painter.rect_filled(caret, 0.0, caret_color);
                } else {
                    text_painter.rect_stroke(caret, 0.0, Stroke::new(1.0, caret_color.gamma_multiply(0.4)), egui::StrokeKind::Inside);
                }
            }

            let num_color = if on_caret_line { fg } else { fg.gamma_multiply(0.35) };
            painter.text(
                pos2(m.gutter.right() - m.char_w * 1.5, y),
                Align2::RIGHT_TOP,
                (line + 1).to_string(),
                m.font.clone(),
                num_color,
            );
        }
    }
}

/// Text spliced into a line for display only (IME composition).
struct Insert<'a> {
    byte: usize,
    text: &'a str,
}

/// Lay out one line with tabs expanded, colored by `spans` when available.
fn layout_line(ui: &Ui, text: &str, spans: Option<&[Span]>, font: &FontId, fg: Color32, tab: usize) -> Arc<Galley> {
    layout_line_with(ui, text, spans, font, fg, tab, None)
}

fn layout_line_with(
    ui: &Ui,
    text: &str,
    spans: Option<&[Span]>,
    font: &FontId,
    fg: Color32,
    tab: usize,
    insert: Option<Insert>,
) -> Arc<Galley> {
    // Flatten into (byte range, format) segments, then splice the insert in.
    let valid = spans.filter(|s| !s.is_empty() && s.last().is_none_or(|l| l.range.end <= text.len()));
    let mut segments: Vec<(std::ops::Range<usize>, TextFormat)> = match valid {
        Some(spans) => spans
            .iter()
            .map(|span| {
                let mut fmt = TextFormat::simple(font.clone(), span.color);
                fmt.italics = span.italic;
                if span.underline {
                    fmt.underline = Stroke::new(1.0, span.color);
                }
                (span.range.clone(), fmt)
            })
            .collect(),
        None => vec![(0..text.len(), TextFormat::simple(font.clone(), fg))],
    };
    let mut pieces: Vec<(&str, TextFormat)> = Vec::with_capacity(segments.len() + 2);
    let mut insert = insert.filter(|i| i.byte <= text.len());
    for (range, fmt) in segments.drain(..) {
        if let Some(ins) = insert.as_ref().filter(|i| i.byte >= range.start && i.byte <= range.end) {
            pieces.push((&text[range.start..ins.byte], fmt.clone()));
            let mut ime = TextFormat::simple(font.clone(), fg);
            ime.underline = Stroke::new(1.0, fg);
            pieces.push((ins.text, ime));
            pieces.push((&text[ins.byte..range.end], fmt));
            insert = None;
        } else {
            pieces.push((&text[range], fmt));
        }
    }

    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let mut col = 0;
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

        fn frame_with(&mut self, mut events: Vec<Event>, modifiers: Modifiers) {
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
        click(&mut h, pos2(700.0, row * 1.5), Modifiers::NONE);
        assert_eq!(h.ed.primary().head, 12);
        click(&mut h, pos2(700.0, row * 2.5), Modifiers::COMMAND);
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
}
