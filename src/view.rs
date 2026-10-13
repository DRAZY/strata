//! The editor widget: paints only visible lines, owns scrolling, and turns
//! keyboard/mouse input into `Editor` commands.

use crate::editor::Editor;
use crate::fold::{self, Folds};
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

const SCROLLBAR_W: f32 = 14.0;
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
    /// Pointer over (or dragging) the scrollbar: draw it brighter.
    scrollbar_hot: bool,
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
    /// Wrap at this column when it's narrower than the window.
    pub wrap_column: Option<usize>,
    /// Columns to draw vertical rulers at.
    pub rulers: Vec<usize>,
    folds: Folds,
    fold_rows: FoldRows,
    /// Fold generation the scroll position was last anchored to.
    fold_gen: u64,
    /// First line changed since the highlighter was last told.
    changed_from: Option<usize>,
    /// Rows to scroll by on the next frame (`scroll_lines`).
    pending_scroll: isize,
    /// Center the cursor line even if it's on screen.
    center_always: bool,
    /// "…" pills painted last frame, with their header lines.
    pills: Vec<(Rect, usize)>,
    gutter_hover: bool,
}

/// Visual rows hidden by folds, per fold (in `Folds::list` order).
#[derive(Default)]
struct FoldRows {
    /// Visual row (folds applied) just after each fold's header.
    pos: Vec<usize>,
    /// `cum[i]`: rows hidden by folds before `i`; one extra entry = all.
    cum: Vec<usize>,
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
            scrollbar_hot: false,
            blink_epoch: 0.0,
            blink_key: (0, 0, 0),
            caret_on: true,
            wrap: false,
            wrap_index: WrapIndex::default(),
            last_wrap: None,
            top_line: 0,
            wrap_column: None,
            rulers: Vec::new(),
            folds: Folds::default(),
            fold_rows: FoldRows::default(),
            fold_gen: 0,
            changed_from: None,
            pending_scroll: 0,
            center_always: false,
            pills: Vec::new(),
            gutter_hover: false,
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

    /// Screen rect of the primary caret as last painted (for popups).
    pub fn caret_rect(&self) -> Option<Rect> {
        self.caret_rect
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

    /// Cmd/Ctrl+K was pressed and the view waits for the chord's second key;
    /// app-level shortcuts on that key (Cmd+1…9, Cmd+0) should stand aside.
    #[allow(dead_code)] // for the app's shortcut handling
    pub fn chord_pending(&self) -> bool {
        self.chord_k
    }

    /// Scroll the view by `n` lines (negative = up), leaving the cursor alone.
    pub fn scroll_lines(&mut self, n: isize) {
        self.pending_scroll += n;
    }

    /// Scroll so the primary cursor's line is mid-screen.
    #[allow(dead_code)] // for the app's menus
    pub fn center_on_cursor(&mut self) {
        self.scroll_to_cursor = true;
        self.center_always = true;
    }

    // ---- folding ---------------------------------------------------------

    /// Fold the region at each cursor (its own region if the cursor line starts
    /// one, else the one around it), or the lines of each multi-line selection.
    pub fn fold(&mut self, ed: &mut Editor) {
        self.sync_folds(ed);
        let (buf, tab) = (&ed.buffer, ed.tab_width);
        let ranges: Vec<(usize, usize)> = ed
            .selections()
            .iter()
            .filter_map(|s| {
                let a = buf.line_of(s.start());
                let mut b = buf.line_of(s.end());
                if b > a && s.end() == buf.line_start(b) {
                    b -= 1;
                }
                if b > a {
                    return Some((a, b));
                }
                if self.folds.at_header(a).is_none()
                    && let Some(end) = fold::region_at(buf, a, tab)
                {
                    return Some((a, end));
                }
                fold::enclosing(buf, a, tab)
            })
            .collect();
        self.folds.add(buf, ranges);
        self.park_carets(ed);
    }

    /// Unfold folds on or around each selection's lines.
    pub fn unfold(&mut self, ed: &mut Editor) {
        self.sync_folds(ed);
        let lines: Vec<(usize, usize)> =
            ed.selections().iter().map(|s| (ed.buffer.line_of(s.start()), ed.buffer.line_of(s.end()))).collect();
        self.folds.retain(|f| !lines.iter().any(|&(a, b)| f.header <= b && f.end >= a));
    }

    pub fn unfold_all(&mut self) {
        self.folds.clear();
    }

    /// Fold every outermost region.
    pub fn fold_all(&mut self, ed: &mut Editor) {
        self.fold_level(ed, 1);
    }

    /// Unfold everything, then fold each region nested `level - 1` or more deep
    /// (Fold Level N: level 1 folds everything, 2 keeps the top level open).
    pub fn fold_level(&mut self, ed: &mut Editor, level: usize) {
        self.sync_folds(ed);
        let depth = level.saturating_sub(1);
        let regions = fold::all_regions(&ed.buffer, ed.tab_width);
        self.folds.clear();
        self.folds.add_regions(&ed.buffer, regions.into_iter().filter(|r| r.depth >= depth));
        self.park_carets(ed);
    }

    /// `line` is hidden inside a fold.
    #[allow(dead_code)] // for the app's menus
    pub fn is_folded_line(&self, line: usize) -> bool {
        self.folds.hiding(line).is_some()
    }

    /// `line` is the visible header of a fold.
    #[allow(dead_code)] // for the app's menus
    pub fn is_fold_header(&self, line: usize) -> bool {
        self.folds.at_header(line).is_some()
    }

    /// Fold or unfold the region starting at `line` (gutter arrow).
    fn toggle_fold_at(&mut self, ed: &mut Editor, line: usize) {
        self.sync_folds(ed);
        if let Some(i) = self.folds.at_header(line) {
            let header = self.folds.list()[i].header;
            self.folds.retain(|f| f.header != header);
        } else if let Some(end) = fold::region_at(&ed.buffer, line, ed.tab_width) {
            self.folds.add(&ed.buffer, [(line, end)]);
            self.park_carets(ed);
        }
    }

    /// Bring the folds up to date with edits since they were last placed.
    fn sync_folds(&mut self, ed: &mut Editor) {
        let changed = ed.take_changed_from();
        if let Some(line) = changed {
            self.changed_from = Some(self.changed_from.map_or(line, |l| l.min(line)));
        }
        self.folds.sync(&ed.buffer, ed.version(), changed, ed.tab_width);
    }

    /// Move carets out of hidden lines to the end of their fold's header.
    fn park_carets(&self, ed: &mut Editor) {
        let folds = &self.folds;
        if !ed.selections().iter().any(|s| folds.hiding(ed.buffer.line_of(s.head)).is_some()) {
            return;
        }
        let park = |pos: usize| match folds.hiding(ed.buffer.line_of(pos)) {
            Some(i) => ed.buffer.line_end(folds.list()[i].header),
            None => pos,
        };
        // Reversed ranges keep each selection's direction.
        let ranges: Vec<std::ops::Range<usize>> = ed.selections().iter().map(|s| park(s.anchor)..park(s.head)).collect();
        let primary = ed.selections().iter().position(|s| *s == ed.primary()).unwrap_or(0);
        ed.select_ranges(&ranges, primary);
    }

    /// Unfold whatever hides a caret (it moved there by search, undo, ...).
    fn unfold_carets(&mut self, ed: &Editor) {
        if self.folds.is_empty() {
            return;
        }
        let heads: Vec<usize> = ed.selections().iter().map(|s| ed.buffer.line_of(s.head)).collect();
        if heads.iter().any(|&l| self.folds.hiding(l).is_some()) {
            self.folds.retain(|f| !heads.iter().any(|&l| f.header < l && l <= f.end));
        }
    }

    /// Keep the top line in place when wrapping or folds change the rows above it.
    fn keep_top_line(&mut self, ed: &Editor, m: &Metrics) {
        if m.wrap != self.last_wrap || self.folds.generation != self.fold_gen {
            self.last_wrap = m.wrap;
            self.fold_gen = self.folds.generation;
            self.scroll.y = self.line_row(ed, m, self.top_line) as f32 * m.row_h;
        }
    }

    /// Fold edits, wrap index and fold rows, before rows are used.
    fn sync(&mut self, ed: &mut Editor, m: &Metrics) {
        self.sync_folds(ed);
        if let Some(cols) = m.wrap {
            self.wrap_index.update(&ed.buffer, ed.version(), cols, ed.tab_width);
        }
        self.unfold_carets(ed);
        let (mut pos, mut cum) = (Vec::with_capacity(self.folds.list().len()), vec![0]);
        for f in self.folds.list() {
            let after = self.base_row(ed, m, f.header + 1);
            let hidden = self.base_row(ed, m, f.end + 1) - after;
            pos.push(after - cum[cum.len() - 1]);
            cum.push(cum[cum.len() - 1] + hidden);
        }
        self.fold_rows = FoldRows { pos, cum };
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
        self.sync(ed, &m);
        if resp.has_focus() {
            self.handle_keys(ui, ed, &m, lang::comment_tokens(hl.syntax_name()));
        }
        self.sync(ed, &m);
        if let Some(line) = self.changed_from.take() {
            hl.invalidate_from(line);
        }
        self.keep_top_line(ed, &m);
        self.handle_mouse(ui, ed, &resp, &m);
        self.sync(ed, &m);
        self.keep_top_line(ed, &m);
        if resp.hovered() {
            self.scroll -= ui.input(|i| i.smooth_scroll_delta);
        }
        self.scroll.y += std::mem::take(&mut self.pending_scroll) as f32 * m.row_h;
        let hover = ui.input(|i| i.pointer.hover_pos()).filter(|_| resp.hovered());
        self.gutter_hover = hover.is_some_and(|p| m.gutter.contains(p));
        if hover.is_some_and(|p| self.pills.iter().any(|(r, _)| r.contains(p))) {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        } else if hover.is_some_and(|p| m.text.contains(p)) {
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
        self.paint_scrollbar(ui, ed, hl, &m);

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
        let wrap = self.wrap_active(ed).then(|| {
            let fit = ((text.width() / char_w).floor() as usize).saturating_sub(2).max(20);
            self.wrap_column.map_or(fit, |n| fit.min(n.max(1)))
        });
        let scroll_x = if wrap.is_some() { 0.0 } else { self.scroll.x };
        Metrics { full: rect, font, row_h, char_w, gutter, text, origin_x: text.left() + char_w - scroll_x, minimap, wrap }
    }

    // ---- visual rows (one per line without wrap, none for folded lines) ----

    /// First row of `line` ignoring folds; past the end, the row count.
    fn base_row(&self, ed: &Editor, m: &Metrics, line: usize) -> usize {
        match m.wrap {
            _ if line >= ed.buffer.len_lines() => self.base_rows(ed, m),
            Some(_) => self.wrap_index.first_row(line),
            None => line,
        }
    }

    fn base_rows(&self, ed: &Editor, m: &Metrics) -> usize {
        if m.wrap.is_some() { self.wrap_index.rows() } else { ed.buffer.len_lines() }
    }

    fn hidden_rows(&self, folds_before: usize) -> usize {
        self.fold_rows.cum.get(folds_before).copied().unwrap_or(0)
    }

    fn total_rows(&self, ed: &Editor, m: &Metrics) -> usize {
        let all = self.fold_rows.cum.last().copied().unwrap_or(0);
        self.base_rows(ed, m).saturating_sub(all).max(1)
    }

    /// First visual row of `line` (of its fold's header if it's hidden).
    fn line_row(&self, ed: &Editor, m: &Metrics, line: usize) -> usize {
        let line = self.folds.hiding(line).map_or(line, |i| self.folds.list()[i].header);
        let before = self.folds.list().partition_point(|f| f.end < line);
        self.base_row(ed, m, line) - self.hidden_rows(before)
    }

    /// (line, row within the line) of visual row `row`.
    fn row_line(&self, ed: &Editor, m: &Metrics, row: usize) -> (usize, usize) {
        let base = row + self.hidden_rows(self.fold_rows.pos.partition_point(|&p| p <= row));
        let last = ed.buffer.len_lines() - 1;
        if m.wrap.is_some() {
            let (line, sub) = self.wrap_index.line_of_row(base);
            (line.min(last), sub)
        } else {
            (base.min(last), 0)
        }
    }

    /// Visual row of the caret at `pos`.
    fn pos_row(&self, ed: &Editor, m: &Metrics, pos: usize) -> usize {
        let line = ed.buffer.line_of(pos);
        let row = self.line_row(ed, m, line);
        if m.wrap.is_none() || self.folds.hiding(line).is_some() {
            return row;
        }
        row + m.line_rows(ed, line).row_of(pos - ed.buffer.line_start(line))
    }

    /// The line painted after `line`: folds are skipped.
    fn next_line(&self, line: usize) -> usize {
        self.folds.at_header(line).map_or(line + 1, |i| self.folds.list()[i].end + 1)
    }

    /// Move each caret by `n` visual rows, keeping its column within the row
    /// (by buffer lines when not wrapping); folded lines take no rows.
    fn move_rows(&self, ed: &mut Editor, m: &Metrics, n: isize, extend: bool) {
        if m.wrap.is_none() && self.folds.is_empty() {
            return ed.move_vertical(n, extend);
        }
        let total = self.total_rows(ed, m) as isize;
        ed.move_heads(extend, |e, s| {
            let line = e.buffer.line_of(s.head);
            let lr = m.line_rows(e, line);
            let off = s.head - e.buffer.line_start(line);
            let goal = s.goal().unwrap_or_else(|| lr.col_in_row(off));
            let row = self.pos_row(e, m, s.head) as isize + n;
            if row < 0 {
                return (0, None);
            }
            if row >= total {
                return (e.buffer.len_chars(), None);
            }
            let (line, sub) = self.row_line(e, m, row as usize);
            let lr = m.line_rows(e, line);
            (e.buffer.line_start(line) + lr.off_at_col(sub.min(lr.rows() - 1), goal), Some(goal))
        });
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
        // Fold/unfold: Alt+Cmd+[ ] on macOS, Ctrl+Shift+[ ] elsewhere.
        let fold_mods = if mac { mods.command && mods.alt && !shift } else { mods.ctrl && shift && !mods.alt };
        // Scroll a line: Ctrl+Alt+Up/Down on macOS, Ctrl+Up/Down elsewhere.
        let scroll_line = if mac { mods.ctrl && mods.alt && !mods.command && !shift } else { mods.ctrl && !shift && !mods.alt };
        if matches!(key, Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown) {
            self.sync(ed, m);
        }
        if std::mem::take(&mut self.chord_k) && mods.command {
            match key {
                Key::U => ed.swap_case_upper(),
                Key::L => ed.swap_case_lower(),
                Key::Num1 => self.fold_all(ed),
                Key::J | Key::Num0 => self.unfold_all(),
                Key::Num2 => self.fold_level(ed, 2),
                Key::Num3 => self.fold_level(ed, 3),
                Key::Num4 => self.fold_level(ed, 4),
                Key::Num5 => self.fold_level(ed, 5),
                Key::Num6 => self.fold_level(ed, 6),
                Key::Num7 => self.fold_level(ed, 7),
                Key::Num8 => self.fold_level(ed, 8),
                Key::Num9 => self.fold_level(ed, 9),
                _ => return false,
            }
            return true;
        }
        match key {
            Key::OpenBracket | Key::OpenCurlyBracket if fold_mods => self.fold(ed),
            Key::CloseBracket | Key::CloseCurlyBracket if fold_mods => self.unfold(ed),
            Key::ArrowUp | Key::ArrowDown if move_lines => ed.move_lines(key == Key::ArrowUp),
            Key::ArrowUp if add_cursor => ed.add_cursor_vertical(-1),
            Key::ArrowDown if add_cursor => ed.add_cursor_vertical(1),
            Key::ArrowUp | Key::ArrowDown if scroll_line => {
                // Moves the view only, so don't reveal the cursor.
                self.scroll_lines(if key == Key::ArrowUp { -1 } else { 1 });
                return false;
            }
            Key::ArrowLeft if cmd => ed.move_home(shift),
            Key::ArrowRight if cmd => ed.move_end(shift),
            Key::ArrowUp if cmd => ed.move_doc_start(shift),
            Key::ArrowDown if cmd => ed.move_doc_end(shift),
            Key::ArrowLeft => ed.move_left(word, shift),
            Key::ArrowRight => ed.move_right(word, shift),
            Key::ArrowUp => self.move_rows(ed, m, -1, shift),
            Key::ArrowDown => self.move_rows(ed, m, 1, shift),
            Key::Home if mods.command => ed.move_doc_start(shift),
            Key::End if mods.command => ed.move_doc_end(shift),
            Key::Home => ed.move_home(shift),
            Key::End => ed.move_end(shift),
            Key::PageUp | Key::PageDown => {
                let n = Self::page_lines(m) * if key == Key::PageUp { -1 } else { 1 };
                self.move_rows(ed, m, n, shift);
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
        if pressed && let Some(&(_, line)) = self.pills.iter().find(|(r, _)| r.contains(pointer)) {
            self.toggle_fold_at(ed, line);
            return;
        }
        // The fold arrow sits between the line numbers and the text.
        if pressed && pointer.x >= m.gutter.right() - m.char_w * 1.5 && pointer.x < m.text.left() {
            let row = self.row_at(m, pointer.y);
            if row >= 0.0 && (row as usize) < self.total_rows(ed, m) {
                let line = self.row_line(ed, m, row as usize).0;
                if self.folds.at_header(line).is_some() || fold::has_region(&ed.buffer, line, ed.tab_width) {
                    self.toggle_fold_at(ed, line);
                    return;
                }
            }
        }
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
            // Through to the next visible line, so a folded block is taken whole.
            ed.click(ed.buffer.line_start(self.next_line(line)).min(ed.buffer.len_chars()), true);
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
        let center = std::mem::take(&mut self.center_cursor) && off_screen;
        if std::mem::take(&mut self.center_always) || center {
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
        self.scrollbar_hot = resp.hovered() || resp.dragged();
    }

    /// The scrollbar, drawn last: a tinted track, a thumb shaded from the
    /// theme's caret color into its selection color, and a tick at the cursor.
    fn paint_scrollbar(&self, ui: &Ui, ed: &Editor, hl: &Highlighter, m: &Metrics) {
        let track = Rect::from_min_max(pos2(m.full.right() - SCROLLBAR_W, m.text.top()), pos2(m.full.right(), m.text.bottom()));
        let content = self.content_height(ed, m);
        let h = m.text.height();
        if content <= h + 1.0 {
            return;
        }
        let painter = ui.painter();
        let (bg, fg, accent, sel) = (hl.background(), hl.foreground(), hl.caret(), hl.selection());
        painter.rect_filled(track, 0.0, blend(bg, fg, 0.04));
        painter.vline(track.left() + 0.5, track.y_range(), Stroke::new(1.0, blend(bg, fg, 0.08)));

        let thumb_h = (h * h / content).max(40.0);
        let t = (self.scroll.y / (content - h)).clamp(0.0, 1.0);
        let thumb = Rect::from_min_size(pos2(track.left() + 3.0, track.top() + t * (h - thumb_h)), vec2(SCROLLBAR_W - 6.0, thumb_h));
        let strength = if self.scrollbar_hot { 1.0 } else { 0.7 };
        let top = blend(bg, accent, 0.85 * strength);
        let bottom = blend(bg, blend(accent, sel, 0.6), 0.75 * strength);
        gradient_pill(painter, thumb, top, bottom);

        // Where the primary cursor is in the file.
        let row = self.pos_row(ed, m, ed.primary().head) as f32;
        let y = track.top() + (row * m.row_h / content).clamp(0.0, 1.0) * h;
        painter.hline(track.left() + 2.0..=track.right() - 2.0, y.round() + 0.5, Stroke::new(2.0, accent));
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
        self.pills.clear();

        for &col in &self.rulers {
            let x = (m.origin_x + col as f32 * m.char_w).round() + 0.5;
            text_painter.line_segment([pos2(x, m.text.top()), pos2(x, m.text.bottom())], Stroke::new(1.0, guide));
        }

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

                // A folded header ends in a "…" pill; clicking it unfolds.
                if last_sub && self.folds.at_header(line).is_some() {
                    let x = m.origin_x + galley.size().x + m.char_w * 0.6;
                    let pill = Rect::from_min_size(pos2(x, y + m.row_h * 0.15), vec2(m.char_w * 2.4, m.row_h * 0.7));
                    text_painter.rect_filled(pill, m.row_h * 0.3, fg.gamma_multiply(0.18));
                    text_painter.text(pill.center(), Align2::CENTER_CENTER, "…", m.font.clone(), fg.gamma_multiply(0.8));
                    self.pills.push((pill, line));
                }

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
                    // Bookmarked lines get a marker at the left of the gutter.
                    let marks = ed.bookmarks();
                    let k = marks.partition_point(|&b| b < line_start);
                    if marks.get(k).is_some_and(|&b| b <= line_end) {
                        let c = pos2(m.gutter.left() + m.char_w * 0.9, y + m.row_h / 2.0);
                        painter.circle_filled(c, (m.row_h * 0.2).max(3.0), caret_color);
                    }
                    // Fold arrows: always on folds, on other regions while the gutter is hovered.
                    let folded = self.folds.at_header(line).is_some();
                    if folded || (self.gutter_hover && fold::has_region(&ed.buffer, line, ed.tab_width)) {
                        let c = pos2(m.gutter.right() - m.char_w * 0.75, y + m.row_h / 2.0);
                        let r = (m.row_h * 0.18).max(3.0);
                        let points = if folded {
                            vec![pos2(c.x - r * 0.6, c.y - r), pos2(c.x + r * 0.7, c.y), pos2(c.x - r * 0.6, c.y + r)]
                        } else {
                            vec![pos2(c.x - r, c.y - r * 0.6), pos2(c.x + r, c.y - r * 0.6), pos2(c.x, c.y + r * 0.7)]
                        };
                        let color = if folded { fg.gamma_multiply(0.8) } else { gutter_fg };
                        painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
                    }
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
            line = self.next_line(line);
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

        // The part the editor shows, outlined only while the pointer is over the
        // minimap (Sublime's default), so short files don't get a gray block.
        let hovered = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| mm.contains(p));
        if hovered {
            let top = mm.top() + (self.scroll.y / m.row_h - first as f32) * MM_ROW;
            let view = Rect::from_min_size(pos2(mm.left(), top), vec2(mm.width(), m.text.height() / m.row_h * MM_ROW));
            painter.rect_filled(view, 0.0, fg.gamma_multiply(0.07));
        }

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
            line = self.next_line(line);
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

/// Blend toward `to` by `t` (0 = `from`).
fn blend(from: Color32, to: Color32, t: f32) -> Color32 {
    let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(m(from.r(), to.r()), m(from.g(), to.g()), m(from.b(), to.b()))
}

/// A vertical pill filled with a top-to-bottom gradient.
fn gradient_pill(painter: &egui::Painter, r: Rect, top: Color32, bottom: Color32) {
    let radius = r.width() / 2.0;
    let body = Rect::from_min_max(pos2(r.left(), r.top() + radius), pos2(r.right(), r.bottom() - radius));
    let mut mesh = egui::Mesh::default();
    let i = mesh.vertices.len() as u32;
    for (p, c) in [(body.left_top(), top), (body.right_top(), top), (body.right_bottom(), bottom), (body.left_bottom(), bottom)] {
        mesh.colored_vertex(p, c);
    }
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
    painter.add(mesh);
    painter.circle_filled(pos2(r.center().x, body.top()), radius, top);
    painter.circle_filled(pos2(r.center().x, body.bottom()), radius, bottom);
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

    const RUST: &str = "fn main() {\n    let x = 1;\n\n    if x > 0 {\n        go();\n    }\n}\n\nfn other() {}\n";
    const PY: &str = "class A:\n    def f(self):\n        return 1\n\n    def g(self):\n        pass\n\nx = 1\n";

    fn fold_mods() -> Modifiers {
        if cfg!(target_os = "macos") { Modifiers::COMMAND | Modifiers::ALT } else { Modifiers::CTRL | Modifiers::SHIFT }
    }

    fn hidden(h: &Harness) -> Vec<usize> {
        (0..h.ed.buffer.len_lines()).filter(|&l| h.view.is_folded_line(l)).collect()
    }

    fn head_line(h: &Harness) -> usize {
        h.ed.buffer.line_of(h.ed.primary().head)
    }

    #[test]
    fn fold_and_unfold_rust_block_at_cursor() {
        let mut h = Harness::new(RUST);
        h.frame(vec![key(Key::OpenBracket, fold_mods())]);
        assert_eq!(hidden(&h), [1, 2, 3, 4, 5], "the closing brace stays visible");
        assert!(h.view.is_fold_header(0));
        h.frame(vec![key(Key::ArrowDown, Modifiers::NONE)]);
        assert_eq!(head_line(&h), 6, "down from the header skips the fold");
        h.frame(vec![key(Key::ArrowUp, Modifiers::NONE)]);
        assert_eq!(head_line(&h), 0);
        h.frame(vec![key(Key::CloseBracket, fold_mods())]);
        assert!(hidden(&h).is_empty());
    }

    #[test]
    fn fold_python_method_then_its_class() {
        let mut h = Harness::new(PY);
        h.ed.click(h.ed.buffer.line_start(2) + 9, false);
        h.frame(vec![key(Key::OpenBracket, fold_mods())]);
        assert_eq!(hidden(&h), [2]);
        assert_eq!(h.ed.primary().head, h.ed.buffer.line_end(1), "the caret parks on the header");
        // Folding again on a folded header folds the region around it.
        h.frame(vec![key(Key::OpenBracket, fold_mods())]);
        assert_eq!(hidden(&h), [1, 2, 3, 4, 5]);
        assert_eq!(head_line(&h), 0);
        h.frame(vec![key(Key::ArrowDown, Modifiers::NONE)]);
        assert_eq!(head_line(&h), 6);
    }

    #[test]
    fn fold_all_unfold_all_and_fold_level_chords() {
        let mut h = Harness::new(PY);
        h.frame(vec![key(Key::K, Modifiers::COMMAND), key(Key::Num1, Modifiers::COMMAND)]);
        assert_eq!(hidden(&h), [1, 2, 3, 4, 5]);
        h.frame(vec![key(Key::K, Modifiers::COMMAND), key(Key::J, Modifiers::COMMAND)]);
        assert!(hidden(&h).is_empty());
        h.frame(vec![key(Key::K, Modifiers::COMMAND), key(Key::Num2, Modifiers::COMMAND)]);
        assert_eq!(hidden(&h), [2, 5], "level 2 keeps the class open");
        h.view.unfold_all();
        h.view.fold_all(&mut h.ed);
        assert_eq!(hidden(&h), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn multiline_selection_folds_its_lines() {
        let mut h = Harness::new("a\nb\nc\nd\ne\n");
        h.ed.select_ranges(&[0..h.ed.buffer.line_start(3)], 0);
        h.frame(vec![key(Key::OpenBracket, fold_mods())]);
        assert_eq!(hidden(&h), [1, 2], "a selection ending at column 0 leaves that line out");
    }

    #[test]
    fn click_below_a_fold_hits_the_right_line() {
        let mut h = Harness::new(RUST);
        h.view.fold(&mut h.ed);
        h.frame(vec![]);
        let row = h.row_h();
        // Rows: 0 header, 1 "}", 2 blank, 3 "fn other() {}".
        click(&mut h, pos2(600.0, row * 3.5), Modifiers::NONE);
        assert_eq!(h.ed.primary().head, h.ed.buffer.line_end(8));
        assert_eq!(hidden(&h), [1, 2, 3, 4, 5], "the fold stays");
    }

    #[test]
    fn pill_and_gutter_arrow_unfold() {
        let mut h = Harness::new(RUST);
        h.view.fold(&mut h.ed);
        h.frame(vec![]);
        let pill = h.view.pills.first().expect("pill painted").0;
        click(&mut h, pill.center(), Modifiers::NONE);
        assert!(hidden(&h).is_empty());
        assert_eq!(h.ed.primary().head, 0, "the click didn't move the caret");
        // The gutter arrow folds it again.
        let row = h.row_h();
        let caret = h.view.caret_rect.expect("caret painted");
        let char_w = h.ctx.fonts_mut(|f| f.glyph_width(&FontId::monospace(14.0), 'M'));
        click(&mut h, pos2(caret.left() - char_w * 1.4, row * 0.5), Modifiers::NONE);
        assert_eq!(hidden(&h), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn caret_moving_into_a_fold_unfolds_it() {
        let mut h = Harness::new(RUST);
        h.view.fold(&mut h.ed);
        h.frame(vec![]);
        h.ed.click(h.ed.buffer.line_start(4), false);
        h.frame(vec![]);
        assert!(hidden(&h).is_empty());
    }

    #[test]
    fn typing_above_a_fold_keeps_it() {
        let mut h = Harness::new(RUST);
        h.ed.click(h.ed.buffer.line_start(3) + 4, false);
        h.view.fold(&mut h.ed);
        assert_eq!(hidden(&h), [4]);
        h.ed.click(0, false);
        h.frame(vec![Event::Text("// hi".into()), key(Key::Enter, Modifiers::NONE)]);
        h.frame(vec![]);
        assert!(h.view.is_fold_header(4));
        assert_eq!(hidden(&h), [5]);
        // Deleting everything never panics and drops the fold.
        h.frame(vec![key(Key::A, Modifiers::COMMAND), key(Key::Backspace, Modifiers::NONE)]);
        assert!(hidden(&h).is_empty());
    }

    #[test]
    fn folds_with_wrap() {
        let text = format!("fn a() {{\n    {}\n    {}\n}}\ntail", long_line(60), long_line(60));
        let (mut h, cols) = wrapped(&text);
        assert!(rows_of(&h, 1, cols).rows() > 2);
        h.view.fold(&mut h.ed);
        h.frame(vec![]);
        assert_eq!(hidden(&h), [1, 2]);
        let row = h.row_h();
        // Rows: 0 header, 1 "}", 2 "tail".
        click(&mut h, pos2(600.0, row * 2.5), Modifiers::NONE);
        assert_eq!(head_line(&h), 4);
        h.ed.click(0, false);
        h.frame(vec![key(Key::ArrowDown, Modifiers::NONE)]);
        assert_eq!(head_line(&h), 3);
        h.view.unfold_all();
        h.frame(vec![key(Key::ArrowUp, Modifiers::NONE)]);
        assert_eq!(head_line(&h), 2, "unfolded, up lands on the last row of the long line");
    }

    #[test]
    fn rulers_and_wrap_column() {
        let (mut h, cols) = wrapped(&long_line(60));
        h.view.rulers = vec![0, 80, 120, 10_000];
        h.view.wrap_column = Some(30);
        h.frame(vec![]);
        assert!(cols > 30);
        assert_eq!(h.view.last_wrap, Some(30));
        h.view.wrap_column = Some(500);
        h.frame(vec![]);
        assert_eq!(h.view.last_wrap, Some(cols), "the window is narrower");
    }

    #[test]
    fn scroll_lines_moves_view_not_cursor() {
        let text: String = (0..2_000).map(|i| format!("line {i}\n")).collect();
        let mut h = Harness::new(&text);
        let row = h.row_h();
        h.view.scroll_lines(5);
        h.frame(vec![]);
        assert_eq!(h.view.scroll.y, 5.0 * row);
        let mods = if cfg!(target_os = "macos") { Modifiers::CTRL | Modifiers::ALT } else { Modifiers::CTRL };
        h.frame(vec![key(Key::ArrowDown, mods), key(Key::ArrowDown, mods)]);
        assert_eq!(h.view.scroll.y, 7.0 * row);
        h.frame(vec![key(Key::ArrowUp, mods)]);
        assert_eq!(h.view.scroll.y, 6.0 * row);
        assert_eq!(h.ed.primary().head, 0);
    }

    #[test]
    fn center_on_cursor_centers_even_when_visible() {
        let text: String = (0..2_000).map(|i| format!("line {i}\n")).collect();
        let mut h = Harness::new(&text);
        h.ed.click(h.ed.buffer.line_start(1_000), false);
        h.view.reveal();
        h.frame(vec![]);
        let before = h.view.caret_rect.expect("caret").center().y;
        h.view.center_on_cursor();
        h.frame(vec![]);
        let after = h.view.caret_rect.expect("caret").center().y;
        assert!(before > 500.0, "reveal leaves it at the bottom: {before}");
        assert!((after - 300.0).abs() < 2.0 * h.row_h(), "{after}");
    }

    #[test]
    fn fold_all_on_a_large_file_is_fast() {
        let block = "fn f() {\n    let x = 1;\n    if x {\n        y();\n    }\n}\n\n";
        let text = block.repeat(200_000 / 7);
        let mut h = Harness::new(&text);
        let start = std::time::Instant::now();
        h.view.fold_all(&mut h.ed);
        let took = start.elapsed();
        assert!(took.as_secs_f64() < 1.0, "fold_all took {took:?}");
        assert!(h.view.is_folded_line(4) && !h.view.is_folded_line(5) && h.view.is_fold_header(7));
        h.frame(vec![key(Key::End, Modifiers::COMMAND)]);
        h.frame(vec![]);
        assert_eq!(head_line(&h), h.ed.buffer.len_lines() - 1);
        assert!(h.view.caret_rect.is_some());
        h.view.unfold_all();
        h.frame(vec![]);
    }
}
