//! The Compare tab: two texts side by side with their lines aligned (filler
//! rows where one side has none), changed lines tinted, changed words marked,
//! and one scroll position for both. The left side is a fixed text (another
//! tab, a file, the git HEAD version); the right side is the tab's own
//! `Editor`, a view of a real file you can edit and save. Edits re-run the
//! diff, at once for small files and after a short pause for big ones.

use crate::buffer::Buffer;
use crate::commands::Cmd;
use crate::diff::{self, Hunk, Row, RowKind};
use crate::editor::Editor;
use crate::highlight::{Highlighter, Span};
use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{Align2, Color32, CursorIcon, Event, EventFilter, FontId, Galley, Id, ImeEvent, Key, Modifiers, PointerButton, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

/// The toolbar row, then a row naming each side.
/// The header's text size (a point under the interface size) and its two rows' heights.
fn header_metrics(ui: &Ui) -> (f32, f32, f32) {
    let text = crate::fonts::secondary_size(egui::TextStyle::Body.resolve(ui.style()).size);
    (text, (text + 16.0).max(28.0), (text + 10.0).max(22.0))
}
const OVERVIEW_W: f32 = 14.0;
const DIVIDER_W: f32 = 6.0;
/// Re-diff after typing pauses this long, unless diffing is cheap.
const DEBOUNCE: f64 = 0.15;
/// A diff that took less than this (seconds) runs on every keystroke.
const CHEAP: f64 = 0.004;

/// The aligned diff of the two sides.
#[derive(Default)]
struct Aligned {
    hunks: Vec<Hunk>,
    rows: Vec<Row>,
    /// Row of each editable-side line.
    row_of_right: Vec<usize>,
    /// First row of each hunk.
    hunk_rows: Vec<usize>,
}

/// What the header buttons asked for (handled by the app, which owns undo,
/// focus and the status line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderAction {
    Next,
    Prev,
    Swap,
    IgnoreWhitespace,
    // Review Changes (see `app::review_ui`).
    Keep,
    Revert,
    KeepFile,
    RevertFile,
    Reviewed,
    NextFile,
}

pub struct Compare {
    base: Buffer,
    base_text: String,
    base_hl: Highlighter,
    /// Header label of the fixed side ("main.rs (HEAD)").
    pub base_name: String,
    /// Header label of the editable side.
    pub right_name: String,
    /// The fixed side follows this open tab's text (by view id), if any.
    pub base_view: Option<Id>,
    /// `Editor::version` of that tab when the text was last taken.
    pub base_version: u64,
    base_gen: u64,
    pub ignore_ws: bool,
    /// Draw the editable side on the left.
    pub swapped: bool,
    aligned: Aligned,
    /// (editor id, its version, ignore_ws, base generation) `aligned` was made for.
    diffed: Option<(u64, u64, bool, u64)>,
    /// When the right side last changed without a re-diff.
    edited_at: Option<f64>,
    last_cost: f64,
    /// Changed words per row: (fixed side, editable side) char ranges.
    inline: HashMap<usize, Words>,
    scroll: Vec2,
    max_width: f32,
    reveal: bool,
    /// Scroll so this row sits a third of the way down.
    show_row: Option<usize>,
    last_head: Option<usize>,
    /// Hunk the last Next/Previous Difference landed on.
    current: Option<usize>,
    wants_focus: bool,
    /// A Review Changes tab: the file under review (the header offers Keep
    /// and Revert).
    pub review: Option<std::path::PathBuf>,
    /// Header buttons pressed this frame.
    pub actions: Vec<HeaderAction>,
    /// Draw hidden characters as boxes, like the editor (`EditorView::show_hidden`).
    pub show_hidden: bool,
}

/// Changed words of a row: (fixed side, editable side) char ranges.
type Words = (Vec<Range<usize>>, Vec<Range<usize>>);

/// Split like a buffer: lines end at `\n`, `\r\n` or a lone `\r` (ropey's
/// `cr_lines`), and a final break is followed by an empty last line.
pub(crate) fn buffer_lines(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let mut lines = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < b.len() {
        let len = match (b[i], b.get(i + 1)) {
            (b'\r', Some(b'\n')) => 2,
            (b'\r' | b'\n', _) => 1,
            _ => 0,
        };
        if len == 0 {
            i += 1;
            continue;
        }
        lines.push(&s[start..i]);
        i += len;
        start = i;
    }
    lines.push(&s[start..]);
    lines
}

impl Compare {
    /// `base`: the fixed side's text, highlighted as `syntax`.
    pub fn new(base: &str, base_name: String, syntax: &str, right_name: String) -> Self {
        let mut base_hl = Highlighter::new();
        base_hl.set_syntax(syntax);
        Self {
            base: Buffer::from_str(base),
            base_text: base.to_owned(),
            base_hl,
            base_name,
            right_name,
            base_view: None,
            base_version: 0,
            base_gen: 0,
            ignore_ws: false,
            swapped: false,
            aligned: Aligned::default(),
            diffed: None,
            edited_at: None,
            last_cost: 0.0,
            inline: HashMap::new(),
            scroll: Vec2::ZERO,
            max_width: 0.0,
            reveal: false,
            show_row: None,
            last_head: None,
            current: None,
            review: None,
            wants_focus: true,
            actions: Vec::new(),
            show_hidden: true,
        }
    }

    /// Tab name.
    pub fn title(&self) -> String {
        format!("{} ↔ {}", self.base_name, self.right_name)
    }

    /// Replace the fixed side's text.
    pub fn set_base(&mut self, text: &str) {
        if text != self.base_text {
            self.base = Buffer::from_str(text);
            self.base_text = text.to_owned();
            self.base_hl.invalidate_from(0);
            self.base_gen += 1;
        }
    }

    pub fn base_text(&self) -> &str {
        &self.base_text
    }

    pub fn focus(&mut self) {
        self.wants_focus = true;
    }

    /// (changed blocks, lines only on the fixed side, lines only on the editable side).
    pub fn stats(&self) -> (usize, usize, usize) {
        diff::stats(&self.aligned.hunks)
    }

    pub fn toggle_ignore_whitespace(&mut self) {
        self.ignore_ws = !self.ignore_ws;
    }

    pub fn swap(&mut self) {
        self.swapped = !self.swapped;
    }

    /// Bring the diff up to date with the editable side. Small diffs run on
    /// every change; slow ones wait for typing to pause (`force` skips the wait).
    /// Returns false while a re-diff is pending.
    pub fn refresh(&mut self, ed: &mut Editor, now: f64, force: bool) -> bool {
        let key = (ed.id(), ed.version(), self.ignore_ws, self.base_gen);
        if self.diffed == Some(key) {
            return true;
        }
        let structural = self.diffed.is_none_or(|(id, _, ws, base)| id != key.0 || ws != self.ignore_ws || base != self.base_gen);
        let since = *self.edited_at.get_or_insert(now);
        if !(force || structural || self.last_cost < CHEAP || now - since >= DEBOUNCE) {
            return false;
        }
        let t = std::time::Instant::now();
        let text = ed.text_snapshot();
        let (a, b) = (buffer_lines(&self.base_text), buffer_lines(&text));
        let hunks = diff::diff_lines(&a, &b, self.ignore_ws);
        let rows = diff::align(&hunks, a.len(), b.len());
        let mut row_of_right = vec![0; b.len()];
        let mut hunk_rows = Vec::with_capacity(hunks.len());
        for (r, row) in rows.iter().enumerate() {
            if let Some(l) = row.right {
                row_of_right[l] = r;
            }
            if let Some(h) = row.hunk
                && hunk_rows.len() == h
            {
                hunk_rows.push(r);
            }
        }
        self.aligned = Aligned { hunks, rows, row_of_right, hunk_rows };
        self.inline.clear();
        self.diffed = Some(key);
        self.edited_at = None;
        self.last_cost = t.elapsed().as_secs_f64();
        if self.current.is_some_and(|h| h >= self.aligned.hunks.len()) {
            self.current = None;
        }
        true
    }

    /// The row showing editable line `line` (clamped while a re-diff is pending).
    fn right_row(&self, line: usize) -> usize {
        let map = &self.aligned.row_of_right;
        match map.get(line) {
            Some(&r) => r,
            None => map.last().map_or(0, |&r| r + line + 1 - map.len()),
        }
    }

    /// Go to the next (or previous) changed block after the caret, wrapping
    /// around, and put the caret at its start. Returns (block, of blocks), or
    /// None if the sides are the same.
    pub fn step(&mut self, ed: &mut Editor, forward: bool) -> Option<(usize, usize)> {
        let n = self.aligned.hunks.len();
        if n == 0 {
            return None;
        }
        let here = self.right_row(ed.buffer.line_of(ed.primary().head));
        let rows = &self.aligned.hunk_rows;
        let k = match (forward, self.current) {
            // Repeated steps move from the block landed on, even if its caret line is shared.
            (true, Some(c)) if rows[c] == here || self.aligned.rows.get(here).is_some_and(|r| r.hunk == Some(c)) => (c + 1) % n,
            (false, Some(c)) if rows[c] == here || self.aligned.rows.get(here).is_some_and(|r| r.hunk == Some(c)) => (c + n - 1) % n,
            (true, _) => rows.iter().position(|&r| r > here).unwrap_or(0),
            (false, _) => rows.iter().rposition(|&r| r < here).unwrap_or(n - 1),
        };
        Some(self.go_to_hunk(ed, k))
    }

    /// Put the caret at the start of changed block `k` and scroll to it.
    /// Returns (block, of blocks), counting from 1.
    pub fn go_to_hunk(&mut self, ed: &mut Editor, k: usize) -> (usize, usize) {
        let n = self.aligned.hunks.len();
        let k = k.min(n.saturating_sub(1));
        if n == 0 {
            return (0, 0);
        }
        self.current = Some(k);
        let line = self.aligned.hunks[k].b.start.min(ed.buffer.len_lines() - 1);
        let at = ed.buffer.line_start(line);
        ed.select_ranges(&[at..at], 0);
        self.last_head = Some(at);
        self.show_row = Some(self.aligned.hunk_rows[k]);
        (k + 1, n)
    }

    /// The changed blocks, as of the last refresh.
    pub fn hunks(&self) -> &[Hunk] {
        &self.aligned.hunks
    }

    /// The changed block the caret is in (for a deletion, the line it sits
    /// at): the one Keep and Revert act on. The block last stepped to wins
    /// when blocks share the caret's line.
    pub fn hunk_at_caret(&self, ed: &Editor) -> Option<usize> {
        let line = ed.buffer.line_of(ed.primary().head);
        let last = ed.buffer.len_lines() - 1;
        let at = |h: &Hunk| h.b.contains(&line) || (h.b.is_empty() && h.b.start.min(last) == line);
        let hunks = &self.aligned.hunks;
        match self.current {
            Some(c) if c < hunks.len() && at(&hunks[c]) => Some(c),
            _ => hunks.iter().position(at),
        }
    }

    /// The block after (or before) the caret's, without wrapping around.
    pub fn next_hunk(&self, ed: &Editor, forward: bool) -> Option<usize> {
        let n = self.aligned.hunks.len();
        if let Some(k) = self.hunk_at_caret(ed) {
            return if forward { (k + 1 < n).then_some(k + 1) } else { k.checked_sub(1) };
        }
        let here = self.right_row(ed.buffer.line_of(ed.primary().head));
        let rows = &self.aligned.hunk_rows;
        if forward { rows.iter().position(|&r| r > here) } else { rows.iter().rposition(|&r| r < here) }
    }

    /// The header, both sides and the overview. `id` is the tab's view id, so
    /// focus checks and context menus work as for an editor.
    pub fn show(&mut self, ui: &mut Ui, ed: &mut Editor, hl: &mut Highlighter, id: Id, font_size: f32) -> egui::Response {
        let now = ui.input(|i| i.time);
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        let (_, toolbar_h, names_h) = header_metrics(ui);
        let header = Rect::from_min_size(full.min, vec2(full.width(), toolbar_h + names_h));
        let body = Rect::from_min_max(pos2(full.left(), header.bottom()), full.max);
        let resp = ui.interact(body, id, Sense::click_and_drag());
        if std::mem::take(&mut self.wants_focus) || resp.clicked() || resp.drag_started() {
            resp.request_focus();
        }
        if resp.has_focus() {
            ui.memory_mut(|m| m.set_focus_lock_filter(id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        }
        if let Some(line) = ed.take_changed_from() {
            hl.invalidate_from(line);
        }
        if !self.base_hl.is_showing(hl.theme_name()) {
            self.base_hl.set_theme(hl.theme_name());
        }

        let font = FontId::monospace(font_size);
        let (text_h, char_w) = ui.fonts_mut(|f| (f.row_height(&font), f.glyph_width(&font, 'M')));
        let row_h = (text_h * 1.25).round();
        let overview = Rect::from_min_max(pos2(body.right() - OVERVIEW_W, body.top()), body.max);
        let cols = Rect::from_min_max(body.min, pos2(overview.left(), body.bottom()));
        let half = ((cols.width() - DIVIDER_W) / 2.0).max(0.0);
        let left_rect = Rect::from_min_size(cols.min, vec2(half, cols.height()));
        let right_rect = Rect::from_min_max(pos2(left_rect.right() + DIVIDER_W, cols.top()), cols.max);
        let (fixed_rect, edit_rect) = if self.swapped { (right_rect, left_rect) } else { (left_rect, right_rect) };
        let digits = ed.buffer.len_lines().max(self.base.len_lines()).to_string().len().max(3);
        let gutter_w = (digits as f32 + 2.0) * char_w;
        let geo = Geo { row_h, char_w, gutter_w, font: font.clone(), tab: ed.tab_width.max(1), hidden: self.show_hidden };

        if resp.has_focus() {
            self.handle_keys(ui, ed);
        }
        let ready = self.refresh(ed, now, false);
        if !ready {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(DEBOUNCE));
        }
        self.handle_mouse(ui, ed, &resp, edit_rect, &geo);
        self.overview_input(ui, id, overview, &geo, body.height());

        let wheel = ui.input(|i| i.smooth_scroll_delta);
        if resp.hovered() && wheel != Vec2::ZERO {
            self.scroll -= wheel;
        }
        let head = ed.primary().head;
        if self.last_head != Some(head) {
            // The caret moved (typing, find, goto line…): keep it in view.
            self.last_head = Some(head);
            self.reveal = true;
        }
        let total = self.aligned.rows.len().max(1);
        if let Some(r) = self.show_row.take() {
            self.scroll.y = r as f32 * row_h - body.height() / 3.0;
        } else if std::mem::take(&mut self.reveal) {
            let line = ed.buffer.line_of(head);
            let y = self.right_row(line) as f32 * row_h;
            if y < self.scroll.y {
                self.scroll.y = y;
            } else if y + row_h > self.scroll.y + body.height() {
                self.scroll.y = y + row_h - body.height();
            }
            let text = ed.buffer.line_text(line);
            let (_, cols) = expand(&text, geo.tab, geo.hidden);
            let x = cols[head - ed.buffer.line_start(line)] as f32 * char_w;
            let width = edit_rect.width() - gutter_w - char_w;
            if x < self.scroll.x {
                self.scroll.x = (x - 4.0 * char_w).max(0.0);
            } else if x > self.scroll.x + width - 2.0 * char_w {
                self.scroll.x = x - width + 4.0 * char_w;
            }
        }
        let max_y = ((total.saturating_sub(1)) as f32 * row_h).max(0.0);
        self.scroll.y = self.scroll.y.clamp(0.0, max_y);
        self.scroll.x = self.scroll.x.clamp(0.0, (self.max_width - half + gutter_w + 4.0 * char_w).max(0.0));

        // ---- paint
        let painter = ui.painter().clone();
        let bg = hl.background();
        let fg = hl.foreground();
        let light = hl.is_light();
        painter.rect_filled(body, 0.0, bg);
        let first = ((self.scroll.y / row_h).floor() as usize).min(total - 1);
        let last = (first + (body.height() / row_h).ceil() as usize + 1).min(total - 1);
        let rows: Vec<(usize, Row)> = (first..=last).filter_map(|r| self.aligned.rows.get(r).map(|row| (r, *row))).collect();
        let (lf, ll) = line_span(rows.iter().filter_map(|(_, r)| r.left));
        let (rf, rl) = line_span(rows.iter().filter_map(|(_, r)| r.right));
        let mut busy = false;
        if let (Some(a), Some(b)) = (lf, ll) {
            busy |= !self.base_hl.prepare(&self.base, a..=b);
        }
        if let (Some(a), Some(b)) = (rf, rl) {
            busy |= !hl.prepare(&ed.buffer, a..=b);
        }
        if busy {
            ui.ctx().request_repaint();
        }
        let colors = Palette::new(bg, fg, light);
        for (r, row) in &rows {
            if row.kind == RowKind::Changed && !self.inline.contains_key(r) {
                let (a, b) = (self.base.line_text(row.left.unwrap_or(0)), ed.buffer.line_text(row.right.unwrap_or(0).min(ed.buffer.len_lines() - 1)));
                self.inline.insert(*r, diff::inline_diff(&a, &b, self.ignore_ws));
            }
        }
        let focused = resp.has_focus();
        let mut widest: f32 = 0.0;
        let warn = crate::view::marker_color(2, light).unwrap_or(fg);
        let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|_| resp.hovered());
        let mut tip = None;
        for (fixed, rect) in [(true, fixed_rect), (false, edit_rect)] {
            let side = SideCtx { rect, fixed, swapped: self.swapped };
            let painter = painter.with_clip_rect(rect);
            for &(r, row) in &rows {
                let y = rect.top() + r as f32 * row_h - self.scroll.y;
                let row_rect = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), row_h));
                let line = if fixed { row.left } else { row.right.filter(|&l| l < ed.buffer.len_lines()) };
                let kind_bg = colors.row_bg(row.kind, side.removed_side());
                let Some(line) = line else {
                    paint_filler(&painter, row_rect, &colors);
                    continue;
                };
                if let Some(c) = kind_bg {
                    painter.rect_filled(row_rect, 0.0, c);
                }
                let (buf, high) = if fixed { (&self.base, &self.base_hl) } else { (&ed.buffer, &*hl) };
                let text = buf.line_text(line);
                crate::fonts::ensure_cjk(ui.ctx(), &text);
                let words = self.inline.get(&r).map(|(a, b)| if fixed { a.as_slice() } else { b.as_slice() }).unwrap_or(&[]);
                let spans = high.line(line);
                let (job, cols) = line_job(&text, spans.as_deref().map(|v| v.as_slice()), &geo, fg, words, colors.word_bg(side.removed_side()));
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                widest = widest.max(galley.size().x);
                let text_x = rect.left() + geo.gutter_w + char_w - self.scroll.x;
                let text_y = y + (row_h - text_h) / 2.0;
                if !fixed {
                    paint_selections(&painter, ed, line, &galley, &cols, pos2(text_x, y), row_h, hl.selection(), focused.then(|| hl.caret()));
                }
                painter.galley(pos2(text_x, text_y), galley.clone(), fg);
                // After the text: changed-word tints are part of the galley and would cover the boxes.
                if geo.hidden && !text.is_ascii() {
                    for (k, c) in text.chars().enumerate().filter(|(_, c)| crate::hidden::is_hidden(*c)) {
                        let (x0, x1) = (galley.pos_from_cursor(CCursor::new(cols[k])).min.x, galley.pos_from_cursor(CCursor::new(cols[k + 1])).min.x);
                        let cell = Rect::from_min_max(pos2(text_x + x0, text_y), pos2(text_x + x1, text_y + text_h));
                        crate::hidden::paint(&painter, cell, c, warn, font_size);
                        if pointer.is_some_and(|p| cell.contains(p) && rect.contains(p)) {
                            tip = crate::hidden::describe(c);
                        }
                    }
                }
                // Gutter over the text (it scrolls under it horizontally).
                let gutter = Rect::from_min_size(row_rect.min, vec2(geo.gutter_w, row_h));
                painter.rect_filled(gutter, 0.0, kind_bg.map_or(bg, |c| blend_over(bg, c)));
                let marker = match row.kind {
                    RowKind::Same => None,
                    _ if side.removed_side() => Some('−'),
                    _ => Some('+'),
                };
                let num_color = if marker.is_some() { fg.gamma_multiply(0.75) } else { fg.gamma_multiply(0.35) };
                painter.text(pos2(gutter.right() - char_w, y + row_h / 2.0), Align2::RIGHT_CENTER, (line + 1).to_string(), FontId::monospace(font_size * 0.9), num_color);
                if let Some(m) = marker {
                    painter.text(pos2(gutter.left() + 2.0, y + row_h / 2.0), Align2::LEFT_CENTER, m, FontId::monospace(font_size * 0.9), colors.marker(side.removed_side()));
                }
            }
            // The rows below the last line.
            let end = rect.top() + total as f32 * row_h - self.scroll.y;
            if end < rect.bottom() {
                painter.rect_filled(Rect::from_min_max(pos2(rect.left(), end.max(rect.top())), rect.max), 0.0, bg);
            }
        }
        self.max_width = self.max_width.max(widest);
        if let Some(tip) = tip {
            resp.clone().on_hover_text_at_pointer(tip);
        }
        // Divider, with a tick per changed row band.
        let divider = Rect::from_min_max(pos2(left_rect.right(), body.top()), pos2(right_rect.left(), body.bottom()));
        painter.rect_filled(divider, 0.0, blend(bg, fg, 0.06));
        for &(r, row) in &rows {
            if row.kind != RowKind::Same {
                let y = body.top() + r as f32 * row_h - self.scroll.y;
                painter.rect_filled(Rect::from_min_size(pos2(divider.left() + 1.0, y), vec2(DIVIDER_W - 2.0, row_h)), 0.0, colors.divider(row.kind));
            }
        }
        self.paint_overview(&painter, overview, &colors, ed, row_h, body.height());
        self.header(ui, header, hl, fixed_rect, edit_rect);
        if hover_in(ui, &resp, edit_rect) {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
        if focused {
            let cursor_rect = self.caret_rect(ed, edit_rect, &geo);
            ui.output_mut(|o| o.ime = Some(egui::output::IMEOutput { purpose: egui::IMEPurpose::Normal, rect: edit_rect, cursor_rect, should_interrupt_composition: false }));
        }
        resp
    }

    fn caret_rect(&self, ed: &Editor, rect: Rect, geo: &Geo) -> Rect {
        let head = ed.primary().head;
        let line = ed.buffer.line_of(head);
        let (_, cols) = expand(&ed.buffer.line_text(line), geo.tab, geo.hidden);
        let y = rect.top() + self.right_row(line) as f32 * geo.row_h - self.scroll.y;
        let x = rect.left() + geo.gutter_w + geo.char_w - self.scroll.x + cols[head - ed.buffer.line_start(line)] as f32 * geo.char_w;
        Rect::from_min_size(pos2(x, y), vec2(2.0, geo.row_h))
    }

    fn header(&mut self, ui: &mut Ui, rect: Rect, hl: &Highlighter, fixed: Rect, edit: Rect) {
        let (bg, fg) = (hl.background(), hl.foreground());
        ui.painter().rect_filled(rect, 0.0, blend(bg, fg, 0.05));
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, blend(bg, fg, 0.12)));
        let (text_size, toolbar_h, _) = header_metrics(ui);
        let small = FontId::proportional(text_size);
        let weak = ui.visuals().weak_text_color();
        let names = Rect::from_min_max(pos2(rect.left(), rect.top() + toolbar_h), rect.max);
        ui.painter().rect_filled(names, 0.0, blend(bg, fg, 0.025));
        ui.painter().hline(rect.x_range(), names.top() + 0.5, Stroke::new(1.0, blend(bg, fg, 0.08)));
        let label = |ui: &Ui, r: Rect, text: &str, color: Color32| {
            crate::fonts::ensure_cjk(ui.ctx(), text);
            let mut job = LayoutJob::simple_singleline(text.to_owned(), small.clone(), color);
            job.wrap.max_width = (r.width() - 16.0).max(10.0);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            let g = ui.fonts_mut(|f| f.layout_job(job));
            ui.painter().galley(pos2(r.left() + 8.0, names.center().y - g.size().y / 2.0), g, color);
        };
        label(ui, fixed, &format!("{}  ·  read-only", self.base_name), weak);
        label(ui, edit, &self.right_name, fg);
        let (n, removed, added) = self.stats();
        let summary = if n == 0 {
            if self.ignore_ws { "No differences (ignoring whitespace)".to_owned() } else { "No differences".to_owned() }
        } else {
            let at = self.current.map(|c| format!("{} of ", c + 1)).unwrap_or_default();
            format!("{at}{n} change{}  ·  −{removed}  +{added}", if n == 1 { "" } else { "s" })
        };
        let bar = Rect::from_min_size(rect.min, vec2(rect.width(), toolbar_h));
        let mut actions = Vec::new();
        let summary_w = ui.painter().text(pos2(bar.left() + 8.0, bar.center().y), Align2::LEFT_CENTER, &summary, small.clone(), fg).width();
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(8.0, 3.0))).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.style_mut().override_font_id = Some(small.clone());
            let mut ws = self.ignore_ws;
            if ui.checkbox(&mut ws, "Ignore whitespace").changed() {
                actions.push(HeaderAction::IgnoreWhitespace);
            }
            if self.review.is_some() {
                // Right to left: the file's buttons, then the change's next to ▲▼.
                let key = |c: Cmd| c.shortcut_label().map(|k| format!(" ({k})")).unwrap_or_default();
                // In a narrow tab the file's buttons give way (they're also in
                // Tools ▸ Review Changes); Keep, Revert, ▲▼ and the summary stay.
                let width = |ui: &Ui, s: &str| ui.painter().layout_no_wrap(s.to_owned(), small.clone(), fg).size().x + 2.0 * ui.spacing().button_padding.x + 6.0;
                let reserve = summary_w + 24.0 + width(ui, "Keep") + width(ui, "Revert") + 2.0 * width(ui, "▲") + 12.0;
                let fits = |ui: &Ui, s: &str| ui.available_width() - width(ui, s) > reserve;
                let file_buttons: [(&str, String, HeaderAction); 4] = [
                    ("Next File", format!("Open the next file to review{}", key(Cmd::ReviewNextFile)), HeaderAction::NextFile),
                    ("Mark Reviewed", format!("Tick this file off in the Changes list{}", key(Cmd::ReviewMarkReviewed)), HeaderAction::Reviewed),
                    ("Revert File", "Put the whole file back as it was in the base (undoable; save to keep it)".into(), HeaderAction::RevertFile),
                    ("Keep File", "Accept every change in this file".into(), HeaderAction::KeepFile),
                ];
                let mut room = true;
                for (label, hint, action) in file_buttons {
                    room = room && fits(ui, label);
                    if room && ui.small_button(label).on_hover_text(hint).clicked() {
                        actions.push(action);
                    }
                }
                ui.separator();
                if ui.add_enabled(n > 0, egui::Button::new("Revert").small()).on_hover_text(format!("Undo the change at the caret (undoable; save to keep it){}", key(Cmd::ReviewRevertChange))).clicked() {
                    actions.push(HeaderAction::Revert);
                }
                if ui.add_enabled(n > 0, egui::Button::new("Keep").small()).on_hover_text(format!("Accept the change at the caret{}", key(Cmd::ReviewKeepChange))).clicked() {
                    actions.push(HeaderAction::Keep);
                }
            } else if ui.small_button("Swap Sides").on_hover_text("Show the other side on the left").clicked() {
                actions.push(HeaderAction::Swap);
            }
            if ui.add_enabled(n > 0, egui::Button::new("▼").small()).on_hover_text("Next difference").clicked() {
                actions.push(HeaderAction::Next);
            }
            if ui.add_enabled(n > 0, egui::Button::new("▲").small()).on_hover_text("Previous difference").clicked() {
                actions.push(HeaderAction::Prev);
            }
        });
        if !actions.is_empty() {
            self.wants_focus = true;
        }
        self.actions.extend(actions);
    }

    fn paint_overview(&self, painter: &egui::Painter, rect: Rect, colors: &Palette, ed: &Editor, row_h: f32, view_h: f32) {
        painter.rect_filled(rect, 0.0, colors.track);
        let content = (self.aligned.rows.len().max(1) as f32 * row_h).max(1.0);
        let scale = rect.height() / content.max(view_h);
        for (h, hunk) in self.aligned.hunks.iter().enumerate() {
            let r = self.aligned.hunk_rows[h];
            let n = hunk.a.len().max(hunk.b.len()).max(1);
            let y0 = rect.top() + r as f32 * row_h * scale;
            let y1 = (y0 + (n as f32 * row_h * scale)).max(y0 + 2.0);
            let kind = match (hunk.a.is_empty(), hunk.b.is_empty()) {
                (true, _) => RowKind::Added,
                (_, true) => RowKind::Removed,
                _ => RowKind::Changed,
            };
            painter.rect_filled(Rect::from_min_max(pos2(rect.left() + 3.0, y0), pos2(rect.right() - 3.0, y1)), 1.0, colors.divider(kind));
        }
        let thumb = Rect::from_min_max(pos2(rect.left() + 1.0, rect.top() + self.scroll.y * scale), pos2(rect.right() - 1.0, rect.top() + (self.scroll.y + view_h) * scale));
        painter.rect_stroke(thumb, 2.0, Stroke::new(1.0, colors.thumb), egui::StrokeKind::Inside);
        let caret = rect.top() + self.right_row(ed.buffer.line_of(ed.primary().head)) as f32 * row_h * scale;
        painter.hline(rect.left()..=rect.right(), caret.round() + 0.5, Stroke::new(1.5, colors.thumb));
    }

    /// Click or drag on the overview to scroll there.
    fn overview_input(&mut self, ui: &Ui, id: Id, rect: Rect, geo: &Geo, view_h: f32) {
        let r = ui.interact(rect, id.with("compare-overview"), Sense::click_and_drag());
        if let Some(p) = r.interact_pointer_pos().filter(|_| r.clicked() || r.dragged()) {
            let content = (self.aligned.rows.len().max(1) as f32 * geo.row_h).max(view_h);
            let y = (p.y - rect.top()) / rect.height() * content;
            self.scroll.y = y - view_h / 2.0;
        }
    }

    fn handle_keys(&mut self, ui: &Ui, ed: &mut Editor) {
        let events = ui.input(|i| i.events.clone());
        for event in events {
            match event {
                Event::Text(t) if !t.is_empty() && t != "\n" && t != "\r" => ed.type_char(&t),
                Event::Ime(ImeEvent::Commit(t)) if !t.is_empty() && t != "\n" && t != "\r" => ed.insert_text(&t),
                Event::Paste(t) => ed.paste(&t),
                Event::Copy => ui.ctx().copy_text(ed.copy()),
                Event::Cut => ui.ctx().copy_text(ed.cut()),
                Event::Key { key, pressed: true, modifiers, .. } => {
                    if !handle_key(ed, key, modifiers, self.page_lines(ui)) {
                        continue;
                    }
                }
                _ => continue,
            }
            self.reveal = true;
        }
    }

    fn page_lines(&self, ui: &Ui) -> isize {
        let h = ui.available_rect_before_wrap().height().max(200.0);
        ((h / 20.0) as isize - 1).max(1)
    }

    /// Clicks and drags on the editable side move its caret; the fixed side only scrolls.
    fn handle_mouse(&mut self, ui: &Ui, ed: &mut Editor, resp: &egui::Response, rect: Rect, geo: &Geo) {
        let Some(p) = resp.interact_pointer_pos() else { return };
        let (pressed, mods) = ui.input(|i| (i.pointer.button_pressed(PointerButton::Primary), i.modifiers));
        let inside = rect.contains(p);
        if pressed && !inside {
            return;
        }
        let Some(pos) = self.pos_at(ui, ed, rect, geo, p) else { return };
        if resp.triple_clicked() && inside {
            let line = ed.buffer.line_of(pos);
            ed.click(ed.buffer.line_start(line), false);
            ed.click(ed.buffer.line_start((line + 1).min(ed.buffer.len_lines())).min(ed.buffer.len_chars()), true);
        } else if resp.double_clicked() && inside {
            ed.select_word_at(pos);
        } else if pressed && resp.is_pointer_button_down_on() {
            if mods.command {
                ed.add_cursor(pos);
            } else {
                ed.click(pos, mods.shift);
            }
        } else if resp.dragged_by(PointerButton::Primary) {
            ed.drag_to(pos);
            if p.y < rect.top() {
                self.scroll.y -= geo.row_h;
            } else if p.y > rect.bottom() {
                self.scroll.y += geo.row_h;
            }
            ui.ctx().request_repaint();
        }
        self.last_head = Some(ed.primary().head);
    }

    /// Char position on the editable side under `p`; filler rows hit the
    /// nearest line below them.
    fn pos_at(&self, ui: &Ui, ed: &Editor, rect: Rect, geo: &Geo, p: Pos2) -> Option<usize> {
        let rows = &self.aligned.rows;
        if rows.is_empty() {
            return Some(0);
        }
        let r = (((p.y - rect.top() + self.scroll.y) / geo.row_h).floor().max(0.0) as usize).min(rows.len() - 1);
        let last = ed.buffer.len_lines() - 1;
        let line = match rows[r].right {
            Some(l) => l.min(last),
            None => match rows[r..].iter().find_map(|row| row.right) {
                Some(l) => return Some(ed.buffer.line_start(l.min(last))),
                None => return Some(ed.buffer.len_chars()),
            },
        };
        let text = ed.buffer.line_text(line);
        let (expanded, cols) = expand(&text, geo.tab, geo.hidden);
        let galley = ui.fonts_mut(|f| f.layout_job(LayoutJob::simple_singleline(expanded, geo.font.clone(), Color32::WHITE)));
        let x = p.x - (rect.left() + geo.gutter_w + geo.char_w - self.scroll.x);
        let idx = galley.cursor_from_pos(vec2(x, geo.row_h / 2.0)).index.0;
        let col = cols.partition_point(|&c| c < idx).min(cols.len() - 1);
        Some(ed.buffer.line_start(line) + col)
    }
}

/// Typing and caret keys for the editable side; commands (undo, comment,
/// duplicate…) come from the keymap like in any editor. True if handled.
fn handle_key(ed: &mut Editor, key: Key, mods: Modifiers, page: isize) -> bool {
    let mac = cfg!(target_os = "macos");
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
        Key::PageUp => ed.move_vertical(-page, shift),
        Key::PageDown => ed.move_vertical(page, shift),
        Key::Enter if !mods.command => ed.newline(),
        Key::Tab if mods.command => return false,
        Key::Tab if shift => ed.outdent(),
        Key::Tab if ed.selections().iter().any(|s| ed.buffer.line_of(s.start()) != ed.buffer.line_of(s.end())) => ed.indent(),
        Key::Tab => ed.tab(),
        Key::Backspace => ed.backspace(word),
        Key::Delete => ed.delete_forward(word),
        Key::Escape => ed.collapse_to_primary(),
        _ => return false,
    }
    true
}

struct Geo {
    row_h: f32,
    char_w: f32,
    gutter_w: f32,
    font: FontId,
    tab: usize,
    hidden: bool,
}

/// Which side is being painted, and where.
struct SideCtx {
    #[allow(dead_code)]
    rect: Rect,
    fixed: bool,
    swapped: bool,
}

impl SideCtx {
    /// This side shows the "old" text (its lines read as removed). The fixed
    /// side is old unless the sides are swapped.
    fn removed_side(&self) -> bool {
        self.fixed != self.swapped
    }
}

/// Diff colors for the current scheme.
struct Palette {
    removed: Color32,
    added: Color32,
    removed_word: Color32,
    added_word: Color32,
    filler: Color32,
    filler_line: Color32,
    track: Color32,
    thumb: Color32,
}

impl Palette {
    fn new(bg: Color32, fg: Color32, light: bool) -> Self {
        let red = if light { Color32::from_rgb(0xd7, 0x3a, 0x49) } else { Color32::from_rgb(0xf8, 0x51, 0x49) };
        let green = if light { Color32::from_rgb(0x28, 0xa7, 0x45) } else { Color32::from_rgb(0x3f, 0xb9, 0x50) };
        let (row, word) = if light { (0.10, 0.28) } else { (0.16, 0.38) };
        Self {
            removed: blend(bg, red, row),
            added: blend(bg, green, row),
            removed_word: blend(bg, red, word),
            added_word: blend(bg, green, word),
            filler: blend(bg, fg, if light { 0.035 } else { 0.05 }),
            filler_line: blend(bg, fg, if light { 0.08 } else { 0.1 }),
            track: blend(bg, fg, 0.04),
            thumb: blend(bg, fg, 0.45),
        }
    }

    fn row_bg(&self, kind: RowKind, removed_side: bool) -> Option<Color32> {
        match kind {
            RowKind::Same => None,
            _ if removed_side => Some(self.removed),
            _ => Some(self.added),
        }
    }

    fn word_bg(&self, removed_side: bool) -> Color32 {
        if removed_side { self.removed_word } else { self.added_word }
    }

    fn marker(&self, removed_side: bool) -> Color32 {
        if removed_side { self.removed_word } else { self.added_word }
    }

    fn divider(&self, kind: RowKind) -> Color32 {
        match kind {
            RowKind::Removed => self.removed_word,
            RowKind::Added => self.added_word,
            _ => blend(self.removed_word, self.added_word, 0.5),
        }
    }
}

/// A row with no line on this side: hatched.
fn paint_filler(painter: &egui::Painter, r: Rect, c: &Palette) {
    painter.rect_filled(r, 0.0, c.filler);
    let step = 8.0;
    let mut x = r.left() - r.height();
    let p = painter.with_clip_rect(r.intersect(painter.clip_rect()));
    while x < r.right() {
        p.line_segment([pos2(x, r.bottom()), pos2(x + r.height(), r.top())], Stroke::new(1.0, c.filler_line));
        x += step;
    }
}

fn line_span(lines: impl Iterator<Item = usize>) -> (Option<usize>, Option<usize>) {
    lines.fold((None, None), |(a, b): (Option<usize>, Option<usize>), l| (Some(a.map_or(l, |a| a.min(l))), Some(b.map_or(l, |b| b.max(l)))))
}

/// `text` with tabs (and shown hidden characters, see `wrap::advance`)
/// expanded to spaces, and the expanded column of each char (plus the end).
fn expand(text: &str, tab: usize, hidden: bool) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut cols = Vec::with_capacity(text.len() + 1);
    let mut col = 0;
    for c in text.chars() {
        cols.push(col);
        let next = crate::wrap::advance(c, col, tab, hidden);
        if c == '\t' || next - col > 1 {
            out.extend(std::iter::repeat_n(' ', next - col));
            col = next;
        } else {
            out.push(c);
            col += 1;
        }
    }
    cols.push(col);
    (out, cols)
}

/// One line laid out with syntax colors, its changed words (`words`, char
/// ranges) on a stronger tint. Returns the job and each char's column.
fn line_job(text: &str, spans: Option<&[Span]>, geo: &Geo, fg: Color32, words: &[Range<usize>], word_bg: Color32) -> (LayoutJob, Vec<usize>) {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let mut cols = Vec::with_capacity(text.len() + 1);
    let mut col = 0;
    let (mut span_i, mut word_i) = (0, 0);
    let mut run = String::new();
    let mut run_fmt: Option<TextFormat> = None;
    for (ci, (bi, c)) in text.char_indices().enumerate() {
        cols.push(col);
        let mut fmt = TextFormat::simple(geo.font.clone(), fg);
        if let Some(spans) = spans {
            while span_i < spans.len() && spans[span_i].range.end <= bi {
                span_i += 1;
            }
            if let Some(s) = spans.get(span_i).filter(|s| s.range.start <= bi) {
                fmt.color = s.color;
                fmt.italics = s.italic;
            }
        }
        while word_i < words.len() && words[word_i].end <= ci {
            word_i += 1;
        }
        if words.get(word_i).is_some_and(|w| w.start <= ci) {
            fmt.background = word_bg;
        }
        if run_fmt.as_ref() != Some(&fmt) {
            if let Some(f) = run_fmt.take() {
                job.append(&std::mem::take(&mut run), 0.0, f);
            }
            run_fmt = Some(fmt);
        }
        let next = crate::wrap::advance(c, col, geo.tab, geo.hidden);
        if c == '\t' || next - col > 1 {
            run.extend(std::iter::repeat_n(' ', next - col));
            col = next;
        } else {
            run.push(c);
            col += 1;
        }
    }
    cols.push(col);
    if let Some(f) = run_fmt {
        job.append(&run, 0.0, f);
    }
    (job, cols)
}

/// Selections and carets of `ed` on `line`, laid out as `galley` at `origin`.
#[allow(clippy::too_many_arguments)]
fn paint_selections(painter: &egui::Painter, ed: &Editor, line: usize, galley: &Arc<Galley>, cols: &[usize], origin: Pos2, row_h: f32, sel: Color32, caret: Option<Color32>) {
    let start = ed.buffer.line_start(line);
    let end = start + cols.len() - 1;
    let x_of = |pos: usize| origin.x + galley.pos_from_cursor(CCursor::new(cols[pos - start])).min.x;
    for s in ed.selections() {
        if !s.is_empty() && s.start() <= end && s.end() >= start {
            let (a, b) = (s.start().max(start), s.end().min(end));
            let mut x1 = x_of(b);
            if s.end() > end {
                // The selection runs past the line end: show the newline.
                x1 += row_h * 0.3;
            }
            painter.rect_filled(Rect::from_min_max(pos2(x_of(a), origin.y), pos2(x1, origin.y + row_h)), 2.0, sel);
        }
        if let Some(c) = caret
            && (start..=end).contains(&s.head)
        {
            let x = x_of(s.head);
            painter.rect_filled(Rect::from_min_size(pos2(x - 1.0, origin.y + 1.0), vec2(2.0, row_h - 2.0)), 0.0, c);
        }
    }
}

fn hover_in(ui: &Ui, resp: &egui::Response, rect: Rect) -> bool {
    resp.hovered() && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p))
}

fn blend(from: Color32, to: Color32, t: f32) -> Color32 {
    let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(l(from.r(), to.r()), l(from.g(), to.g()), l(from.b(), to.b()))
}

/// An opaque version of `over` drawn on `bg` (gutter cells cover the text).
fn blend_over(bg: Color32, over: Color32) -> Color32 {
    let a = over.a() as f32 / 255.0;
    if a >= 1.0 { over } else { blend(bg, over.to_opaque(), a) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> Editor {
        Editor::new(Buffer::from_str(text))
    }

    #[test]
    fn lines_split_like_the_buffer_with_lone_carriage_returns() {
        for s in ["a\rb\r\nc\nd", "\r\r\n\n", "x\r", "", "one\ntwo\n"] {
            let lines = buffer_lines(s);
            let b = Buffer::from_str(s);
            assert_eq!(lines.len(), b.len_lines(), "{s:?}");
            for (i, l) in lines.iter().enumerate() {
                assert_eq!(*l, b.line_text(i), "{s:?} line {i}");
            }
        }
        // Old Mac line endings: same text both sides, so nothing differs.
        let text = "a\rb\rc\r";
        let mut c = Compare::new(text, "old".into(), "Plain Text", "new".into());
        let mut ed = editor(text);
        c.refresh(&mut ed, 0.0, true);
        assert_eq!(c.stats(), (0, 0, 0));
        assert_eq!(c.aligned.rows.len(), ed.buffer.len_lines());
    }

    #[test]
    fn rows_follow_edits_and_steps_wrap_around() {
        let mut c = Compare::new("a\nb\nc\nd\n", "old".into(), "Plain Text", "new".into());
        let mut ed = editor("a\nB\nc\nd\ne\n");
        assert!(c.refresh(&mut ed, 0.0, false));
        assert_eq!(c.stats(), (2, 1, 2));
        assert_eq!(c.step(&mut ed, true), Some((1, 2)));
        assert_eq!(ed.cursor_line_col(), (2, 1));
        assert_eq!(c.step(&mut ed, true), Some((2, 2)));
        assert_eq!(ed.cursor_line_col(), (5, 1));
        assert_eq!(c.step(&mut ed, true), Some((1, 2)), "wraps to the first");
        assert_eq!(c.step(&mut ed, false), Some((2, 2)), "and back");

        // Fix the right side: the diff follows (cheap diffs re-run at once).
        ed.select_ranges(&[2..3], 0);
        ed.insert_text("b");
        assert!(c.refresh(&mut ed, 1.0, false));
        assert_eq!(c.stats(), (1, 0, 1));
        c.toggle_ignore_whitespace();
        assert!(c.refresh(&mut ed, 1.0, false));
        c.set_base("a\nb\nc\nd\ne\n");
        assert!(c.refresh(&mut ed, 1.0, false));
        assert_eq!(c.stats(), (0, 0, 0));
        assert_eq!(c.step(&mut ed, true), None);
    }

    #[test]
    fn slow_diffs_wait_for_typing_to_pause() {
        let mut c = Compare::new("x\n", "old".into(), "Plain Text", "new".into());
        let mut ed = editor("x\n");
        assert!(c.refresh(&mut ed, 0.0, false));
        c.last_cost = 1.0;
        ed.insert_text("y");
        assert!(!c.refresh(&mut ed, 10.0, false), "waits");
        assert!(!c.refresh(&mut ed, 10.1, false));
        assert!(c.refresh(&mut ed, 10.2, false), "runs once typing pauses");
        assert_eq!(c.stats(), (1, 1, 1));
    }

    #[test]
    fn tabs_expand_to_tab_stops() {
        let (s, cols) = expand("a\tb", 4, true);
        assert_eq!(s, "a   b");
        assert_eq!(cols, [0, 1, 4, 5]);
        // A hidden character becomes spaces as wide as its box, and never reaches the shaper.
        let (s, cols) = expand("a\u{202E}b", 4, true);
        assert_eq!(s, "a   b");
        assert_eq!(cols, [0, 1, 4, 5]);
        assert_eq!(expand("a\u{202E}b", 4, false).0, "a\u{202E}b");
        let geo = Geo { row_h: 10.0, char_w: 5.0, gutter_w: 20.0, font: FontId::monospace(10.0), tab: 4, hidden: true };
        let (job, cols) = line_job("é\tx", None, &geo, Color32::WHITE, &[2..3], Color32::RED);
        assert_eq!(job.text, "é   x");
        assert_eq!(cols, [0, 1, 4, 5]);
        assert_eq!(job.sections.last().unwrap().format.background, Color32::RED);
    }

    #[test]
    fn headless_frame_paints_and_takes_typing() {
        let ctx = egui::Context::default();
        let mut c = Compare::new("one\ntwo\nthree\n", "a.txt".into(), "Plain Text", "b.txt".into());
        let mut ed = editor("one\n2\nthree\nfour\n");
        let mut hl = Highlighter::new();
        let id = Id::new("compare-test");
        let mut frame = |c: &mut Compare, ed: &mut Editor, events: Vec<Event>| {
            let input = egui::RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 500.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    c.show(ui, ed, &mut hl, id, 14.0);
                });
            });
            out.textures_delta.clear();
            out.shapes.len()
        };
        assert!(frame(&mut c, &mut ed, vec![]) > 10);
        frame(&mut c, &mut ed, vec![Event::Text("X".into())]);
        assert!(ed.buffer.line_text(0).starts_with('X'), "the editable side has focus and takes typing");
        assert_eq!(c.stats(), (2, 2, 3));
    }
}
