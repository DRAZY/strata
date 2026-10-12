//! Editing model: buffer + selections + undo history. No UI code, so it is
//! unit-testable. Every command applies to every selection, so multiple
//! cursors only need UI to create them.

use crate::buffer::Buffer;
use crate::find::{self, Query};
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
    /// Display column remembered across vertical moves through short lines.
    goal: Option<usize>,
}

impl Selection {
    pub fn cursor(pos: usize) -> Self {
        Self { anchor: pos, head: pos, goal: None }
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }
}

/// One replacement, recorded in the coordinates at the time it was applied.
struct Change {
    at: usize,
    removed: String,
    inserted: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Typing,
    Deleting,
}

struct Transaction {
    id: u64,
    changes: Vec<Change>,
    before: Vec<Selection>,
    after: Vec<Selection>,
    kind: Option<EditKind>,
}

pub struct Editor {
    pub buffer: Buffer,
    sels: Vec<Selection>,
    primary: usize,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    next_id: u64,
    saved_id: u64,
    can_coalesce: bool,
    changed_from: Option<usize>,
    /// Bumped on every text change; keys caches like search results.
    version: u64,
    snapshot: Option<(u64, Arc<String>)>,
    /// Cmd+D started from a bare cursor, so it matches whole words only.
    next_whole_word: bool,
    pub tab_width: usize,
}

/// Where selections land after an edit.
enum Land {
    /// A cursor after each inserted text (one edit per selection).
    EachEnd,
    /// One cursor after the first edit.
    FirstEnd,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum CharClass {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

impl Editor {
    pub fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            sels: vec![Selection::cursor(0)],
            primary: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            next_id: 1,
            saved_id: 0,
            can_coalesce: false,
            changed_from: Some(0),
            version: 0,
            snapshot: None,
            next_whole_word: false,
            tab_width: 4,
        }
    }

    // ---- state queries -------------------------------------------------

    pub fn selections(&self) -> &[Selection] {
        &self.sels
    }

    pub fn primary(&self) -> Selection {
        self.sels[self.primary]
    }

    fn current_id(&self) -> u64 {
        self.undo.last().map_or(0, |t| t.id)
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Whole text as one string, cached until the next edit (for regex search).
    pub fn text_snapshot(&mut self) -> Arc<String> {
        if let Some((v, s)) = &self.snapshot {
            if *v == self.version {
                return s.clone();
            }
        }
        let s = Arc::new(self.buffer.slice(0..self.buffer.len_chars()));
        self.snapshot = Some((self.version, s.clone()));
        s
    }

    pub fn is_dirty(&self) -> bool {
        self.current_id() != self.saved_id
    }

    pub fn mark_saved(&mut self) {
        self.saved_id = self.current_id();
    }

    /// First line touched since the last call; the highlighter re-parses from there.
    pub fn take_changed_from(&mut self) -> Option<usize> {
        self.changed_from.take()
    }

    /// (line, column), both 1-based, of the primary cursor.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let head = self.primary().head;
        let line = self.buffer.line_of(head);
        (line + 1, head - self.buffer.line_start(line) + 1)
    }

    /// Visual column of `pos`, expanding tabs.
    pub fn display_col(&self, pos: usize) -> usize {
        let line = self.buffer.line_of(pos);
        let start = self.buffer.line_start(line);
        let mut col = 0;
        for c in self.buffer.slice(start..pos).chars() {
            col = self.advance_col(col, c);
        }
        col
    }

    pub fn advance_col(&self, col: usize, c: char) -> usize {
        if c == '\t' {
            col + self.tab_width - col % self.tab_width
        } else {
            col + 1
        }
    }

    /// Char index on `line` closest to visual column `col`.
    pub fn pos_at_display_col(&self, line: usize, col: usize) -> usize {
        let start = self.buffer.line_start(line);
        let mut cur = 0;
        let mut pos = start;
        for c in self.buffer.line_text(line).chars() {
            let next = self.advance_col(cur, c);
            if next > col {
                // Snap to whichever side of the char is nearer.
                if col - cur > (next - cur) / 2 {
                    pos += 1;
                }
                return pos;
            }
            cur = next;
            pos += 1;
        }
        pos
    }

    // ---- selection bookkeeping -----------------------------------------

    fn set_selections(&mut self, sels: Vec<Selection>, primary_head: usize) {
        self.next_whole_word = false;
        self.sels = sels;
        self.normalize(primary_head);
    }

    /// Sort, merge overlaps, and re-find the primary selection.
    fn normalize(&mut self, primary_head: usize) {
        let len = self.buffer.len_chars();
        for s in &mut self.sels {
            s.anchor = s.anchor.min(len);
            s.head = s.head.min(len);
        }
        self.sels.sort_by_key(|s| (s.start(), s.end()));
        let mut merged: Vec<Selection> = Vec::with_capacity(self.sels.len());
        for s in self.sels.drain(..) {
            if let Some(last) = merged.last_mut() {
                let overlaps = s.start() < last.end() || s.start() == last.start();
                if overlaps {
                    let (start, end) = (last.start(), last.end().max(s.end()));
                    let forward = last.head >= last.anchor;
                    *last = if forward {
                        Selection { anchor: start, head: end, goal: None }
                    } else {
                        Selection { anchor: end, head: start, goal: None }
                    };
                    continue;
                }
            }
            merged.push(s);
        }
        self.sels = merged;
        self.primary = self
            .sels
            .iter()
            .position(|s| s.start() <= primary_head && primary_head <= s.end())
            .unwrap_or(self.sels.len() - 1);
    }

    fn note_change(&mut self, pos: usize) {
        self.version += 1;
        let line = self.buffer.line_of(pos);
        self.changed_from = Some(self.changed_from.map_or(line, |l| l.min(line)));
    }

    // ---- the one edit primitive ------------------------------------------

    /// Apply one replacement per selection (same order as `self.sels`).
    /// Each selection collapses to the end of its inserted text.
    fn apply(&mut self, edits: Vec<(Range<usize>, String)>, kind: Option<EditKind>) {
        debug_assert_eq!(edits.len(), self.sels.len());
        self.apply_landing(edits, kind, Land::EachEnd);
    }

    /// Apply sorted, non-overlapping replacements as one undo step.
    fn apply_landing(&mut self, edits: Vec<(Range<usize>, String)>, kind: Option<EditKind>, land: Land) {
        let before = self.sels.clone();
        let primary_idx = self.primary;
        let mut delta: isize = 0;
        let mut prev_end = 0;
        let mut changes = Vec::new();
        let mut new_sels = Vec::with_capacity(edits.len());
        let mut primary_head = 0;

        for (i, (range, text)) in edits.into_iter().enumerate() {
            let start = range.start.max(prev_end);
            let end = range.end.max(start);
            prev_end = end;
            let at = (start as isize + delta) as usize;
            let n = text.chars().count();
            if end > start || n > 0 {
                self.note_change(at);
                let removed = self.buffer.slice(at..at + (end - start));
                self.buffer.remove(at..at + (end - start));
                self.buffer.insert(at, &text);
                changes.push(Change { at, removed, inserted: text });
            }
            let keep = match land {
                Land::EachEnd => true,
                Land::FirstEnd => i == 0,
            };
            if keep {
                new_sels.push(Selection::cursor(at + n));
            }
            if i == primary_idx || (i == 0 && matches!(land, Land::FirstEnd)) {
                primary_head = at + n;
            }
            delta += n as isize - (end - start) as isize;
        }

        if new_sels.is_empty() {
            new_sels = self.sels.clone();
            primary_head = self.primary().head;
        }
        self.set_selections(new_sels, primary_head);
        if changes.is_empty() {
            return;
        }
        self.redo.clear();

        let saved_id = self.saved_id;
        if let (Some(k), true, Some(last)) = (kind, self.can_coalesce, self.undo.last_mut()) {
            if last.kind == Some(k) && last.id != saved_id {
                last.changes.extend(changes);
                last.after = self.sels.clone();
                return;
            }
        }
        self.undo.push(Transaction {
            id: self.next_id,
            changes,
            before,
            after: self.sels.clone(),
            kind,
        });
        self.next_id += 1;
        self.can_coalesce = kind.is_some();
    }

    pub fn undo(&mut self) {
        let Some(t) = self.undo.pop() else { return };
        for c in t.changes.iter().rev() {
            self.note_change(c.at);
            self.buffer.remove(c.at..c.at + c.inserted.chars().count());
            self.buffer.insert(c.at, &c.removed);
        }
        let head = t.before.first().map_or(0, |s| s.head);
        self.set_selections(t.before.clone(), head);
        self.redo.push(t);
        self.can_coalesce = false;
    }

    pub fn redo(&mut self) {
        let Some(t) = self.redo.pop() else { return };
        for c in &t.changes {
            self.note_change(c.at);
            self.buffer.remove(c.at..c.at + c.removed.chars().count());
            self.buffer.insert(c.at, &c.inserted);
        }
        let head = t.after.first().map_or(0, |s| s.head);
        self.set_selections(t.after.clone(), head);
        self.undo.push(t);
        self.can_coalesce = false;
    }

    // ---- text input ------------------------------------------------------

    pub fn insert_text(&mut self, text: &str) {
        let edits = self.sels.iter().map(|s| (s.range(), text.to_owned())).collect();
        self.apply(edits, Some(EditKind::Typing));
    }

    /// Newline that keeps the current line's indentation.
    pub fn newline(&mut self) {
        let ending = self.buffer.line_ending().as_str();
        let edits = self
            .sels
            .iter()
            .map(|s| {
                let line = self.buffer.line_of(s.start());
                let before_cursor = self.buffer.slice(self.buffer.line_start(line)..s.start());
                let indent: String =
                    before_cursor.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                (s.range(), format!("{ending}{indent}"))
            })
            .collect();
        self.apply(edits, None);
    }

    /// Soft tab: spaces up to the next tab stop.
    pub fn tab(&mut self) {
        let edits = self
            .sels
            .iter()
            .map(|s| {
                let col = self.display_col(s.start());
                let n = self.tab_width - col % self.tab_width;
                (s.range(), " ".repeat(n))
            })
            .collect();
        self.apply(edits, Some(EditKind::Typing));
    }

    pub fn backspace(&mut self, word: bool) {
        let edits = self
            .sels
            .iter()
            .map(|s| {
                let range = if !s.is_empty() {
                    s.range()
                } else if word {
                    self.word_left(s.head)..s.head
                } else {
                    self.left(s.head)..s.head
                };
                (range, String::new())
            })
            .collect();
        self.apply(edits, Some(EditKind::Deleting));
    }

    pub fn delete_forward(&mut self, word: bool) {
        let edits = self
            .sels
            .iter()
            .map(|s| {
                let range = if !s.is_empty() {
                    s.range()
                } else if word {
                    s.head..self.word_right(s.head)
                } else {
                    s.head..self.right(s.head)
                };
                (range, String::new())
            })
            .collect();
        self.apply(edits, Some(EditKind::Deleting));
    }

    pub fn delete_to_line_start(&mut self) {
        let edits = self
            .sels
            .iter()
            .map(|s| {
                let start = self.buffer.line_start(self.buffer.line_of(s.head));
                let range = if s.is_empty() { start..s.head } else { s.range() };
                (range, String::new())
            })
            .collect();
        self.apply(edits, None);
    }

    // ---- clipboard -------------------------------------------------------

    fn all_empty(&self) -> bool {
        self.sels.iter().all(Selection::is_empty)
    }

    /// Selected text; with no selection, the cursor lines (like Sublime).
    pub fn copy(&self) -> String {
        let ending = self.buffer.line_ending().as_str();
        if self.all_empty() {
            self.sels
                .iter()
                .map(|s| self.buffer.line_text(self.buffer.line_of(s.head)) + ending)
                .collect()
        } else {
            self.sels
                .iter()
                .map(|s| self.buffer.slice(s.range()))
                .collect::<Vec<_>>()
                .join(ending)
        }
    }

    pub fn cut(&mut self) -> String {
        let text = self.copy();
        if self.all_empty() {
            let edits = self
                .sels
                .iter()
                .map(|s| {
                    let line = self.buffer.line_of(s.head);
                    let range = if line + 1 < self.buffer.len_lines() {
                        self.buffer.line_start(line)..self.buffer.line_start(line + 1)
                    } else if line > 0 {
                        self.buffer.line_end(line - 1)..self.buffer.line_end(line)
                    } else {
                        0..self.buffer.line_end(0)
                    };
                    (range, String::new())
                })
                .collect();
            self.apply(edits, None);
        } else {
            let edits = self.sels.iter().map(|s| (s.range(), String::new())).collect();
            self.apply(edits, None);
        }
        text
    }

    /// One line per cursor when the counts match, else the whole text at each.
    pub fn paste(&mut self, text: &str) {
        let normalized = text
            .replace("\r\n", "\n")
            .replace('\n', self.buffer.line_ending().as_str());
        let lines: Vec<&str> = text.lines().collect();
        let edits = if self.sels.len() > 1 && lines.len() == self.sels.len() {
            self.sels
                .iter()
                .zip(lines)
                .map(|(s, l)| (s.range(), l.to_owned()))
                .collect()
        } else {
            self.sels.iter().map(|s| (s.range(), normalized.clone())).collect()
        };
        self.apply(edits, None);
    }

    // ---- movement --------------------------------------------------------

    fn left(&self, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        let line = self.buffer.line_of(pos);
        if pos == self.buffer.line_start(line) {
            self.buffer.line_end(line - 1)
        } else {
            pos - 1
        }
    }

    fn right(&self, pos: usize) -> usize {
        let line = self.buffer.line_of(pos);
        if pos >= self.buffer.line_end(line) {
            if line + 1 < self.buffer.len_lines() {
                self.buffer.line_start(line + 1)
            } else {
                pos
            }
        } else {
            pos + 1
        }
    }

    fn word_left(&self, pos: usize) -> usize {
        let start = self.buffer.line_start(self.buffer.line_of(pos));
        if pos == start {
            return self.left(pos);
        }
        let mut i = pos;
        while i > start && class(self.buffer.char(i - 1)) == CharClass::Space {
            i -= 1;
        }
        if i > start {
            let c = class(self.buffer.char(i - 1));
            while i > start && class(self.buffer.char(i - 1)) == c {
                i -= 1;
            }
        }
        i
    }

    fn word_right(&self, pos: usize) -> usize {
        let end = self.buffer.line_end(self.buffer.line_of(pos));
        if pos >= end {
            return self.right(pos);
        }
        let mut i = pos;
        while i < end && class(self.buffer.char(i)) == CharClass::Space {
            i += 1;
        }
        if i < end {
            let c = class(self.buffer.char(i));
            while i < end && class(self.buffer.char(i)) == c {
                i += 1;
            }
        }
        i
    }

    fn move_each(&mut self, extend: bool, f: impl Fn(&Self, &Selection) -> (usize, Option<usize>)) {
        let primary_before = self.primary;
        let mut primary_head = 0;
        let sels: Vec<Selection> = self
            .sels
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (head, goal) = f(self, s);
                if i == primary_before {
                    primary_head = head;
                }
                let anchor = if extend { s.anchor } else { head };
                Selection { anchor, head, goal }
            })
            .collect();
        self.set_selections(sels, primary_head);
        self.can_coalesce = false;
    }

    pub fn move_left(&mut self, word: bool, extend: bool) {
        self.move_each(extend, |e, s| {
            if !extend && !s.is_empty() && !word {
                (s.start(), None)
            } else if word {
                (e.word_left(s.head), None)
            } else {
                (e.left(s.head), None)
            }
        });
    }

    pub fn move_right(&mut self, word: bool, extend: bool) {
        self.move_each(extend, |e, s| {
            if !extend && !s.is_empty() && !word {
                (s.end(), None)
            } else if word {
                (e.word_right(s.head), None)
            } else {
                (e.right(s.head), None)
            }
        });
    }

    /// Move by `lines` (negative = up), keeping the goal column.
    pub fn move_vertical(&mut self, lines: isize, extend: bool) {
        self.move_each(extend, |e, s| {
            let goal = s.goal.unwrap_or_else(|| e.display_col(s.head));
            let line = e.buffer.line_of(s.head) as isize;
            let last = e.buffer.len_lines() as isize - 1;
            let target = line + lines;
            if target < 0 {
                (0, None)
            } else if target > last {
                (e.buffer.len_chars(), None)
            } else {
                (e.pos_at_display_col(target as usize, goal), Some(goal))
            }
        });
    }

    /// Smart home: first non-blank character, then column 0.
    pub fn move_home(&mut self, extend: bool) {
        self.move_each(extend, |e, s| {
            let line = e.buffer.line_of(s.head);
            let start = e.buffer.line_start(line);
            let indent = e
                .buffer
                .line_text(line)
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .count();
            let first = start + indent;
            (if s.head == first { start } else { first }, None)
        });
    }

    pub fn move_end(&mut self, extend: bool) {
        self.move_each(extend, |e, s| (e.buffer.line_end(e.buffer.line_of(s.head)), None));
    }

    pub fn move_doc_start(&mut self, extend: bool) {
        self.move_each(extend, |_, _| (0, None));
    }

    pub fn move_doc_end(&mut self, extend: bool) {
        self.move_each(extend, |e, _| (e.buffer.len_chars(), None));
    }

    pub fn select_all(&mut self) {
        let len = self.buffer.len_chars();
        self.set_selections(vec![Selection { anchor: 0, head: len, goal: None }], len);
        self.can_coalesce = false;
    }

    // ---- find / Cmd+D -------------------------------------------------

    /// Replace the selections with `ranges`; `primary` indexes into them.
    pub fn select_ranges(&mut self, ranges: &[Range<usize>], primary: usize) {
        if ranges.is_empty() {
            return;
        }
        let sels = ranges.iter().map(|r| Selection { anchor: r.start, head: r.end, goal: None }).collect();
        self.set_selections(sels, ranges[primary.min(ranges.len() - 1)].end);
        self.can_coalesce = false;
    }

    /// Replace several ranges (sorted, non-overlapping) as one undo step.
    pub fn replace_ranges(&mut self, edits: Vec<(Range<usize>, String)>) {
        if !edits.is_empty() {
            self.apply_landing(edits, None, Land::FirstEnd);
        }
    }

    /// Cmd+D. A bare cursor expands to its word; otherwise the next
    /// occurrence of the primary selection is added (wrapping at the end).
    pub fn select_next_occurrence(&mut self) {
        if self.sels.iter().all(Selection::is_empty) {
            let ranges: Vec<_> = self.sels.iter().map(|s| self.word_range_at(s.head)).collect();
            let primary = self.primary;
            self.select_ranges(&ranges, primary);
            self.next_whole_word = true;
            return;
        }
        let whole_word = self.next_whole_word;
        let p = self.primary();
        let needle = self.buffer.slice(p.range());
        let Ok(re) = Query::literal(&needle, whole_word).build() else { return };
        let text = self.text_snapshot();
        let from = self.buffer.char_to_byte(p.end());
        let taken = |start: usize, sels: &[Selection]| sels.iter().any(|s| s.start() == start);
        let found = re
            .find_iter(&text[from..])
            .map(|m| (m.start() + from, m.end() + from))
            .chain(re.find_iter(&text[..from]).map(|m| (m.start(), m.end())))
            .map(|(a, b)| (self.buffer.byte_to_char(a), self.buffer.byte_to_char(b)))
            .find(|&(a, b)| b > a && !taken(a, &self.sels));
        if let Some((a, b)) = found {
            let mut sels = self.sels.clone();
            sels.push(Selection { anchor: a, head: b, goal: None });
            self.set_selections(sels, b);
        }
        self.next_whole_word = whole_word;
        self.can_coalesce = false;
    }

    /// Ctrl+Cmd+G / Alt+F3: every occurrence of the primary selection.
    pub fn select_all_occurrences(&mut self) {
        let mut whole_word = self.next_whole_word;
        if self.primary().is_empty() {
            let r = self.word_range_at(self.primary().head);
            self.select_ranges(&[r], 0);
            whole_word = true;
        }
        let p = self.primary();
        if p.is_empty() {
            return;
        }
        let Ok(re) = Query::literal(&self.buffer.slice(p.range()), whole_word).build() else { return };
        let text = self.text_snapshot();
        let (ranges, _) = find::find_all(&re, &text, find::MATCH_LIMIT);
        let primary = ranges.iter().position(|r| r.start == p.start()).unwrap_or(0);
        self.select_ranges(&ranges, primary);
        self.next_whole_word = whole_word;
    }

    /// Escape: drop extra cursors, keeping the primary one.
    pub fn collapse_to_primary(&mut self) {
        let p = self.primary();
        self.set_selections(vec![p], p.head);
    }

    // ---- mouse -----------------------------------------------------------

    /// Click: single cursor, or extend the primary selection with shift.
    pub fn click(&mut self, pos: usize, extend: bool) {
        let anchor = if extend { self.primary().anchor } else { pos };
        self.set_selections(vec![Selection { anchor, head: pos, goal: None }], pos);
        self.can_coalesce = false;
    }

    /// Cmd/Ctrl+click: add a cursor.
    pub fn add_cursor(&mut self, pos: usize) {
        let mut sels = self.sels.clone();
        sels.push(Selection::cursor(pos));
        self.set_selections(sels, pos);
        self.can_coalesce = false;
    }

    pub fn drag_to(&mut self, pos: usize) {
        let mut sels = self.sels.clone();
        sels[self.primary].head = pos;
        sels[self.primary].goal = None;
        self.set_selections(sels, pos);
    }

    pub fn select_word_at(&mut self, pos: usize) {
        let r = self.word_range_at(pos);
        self.set_selections(vec![Selection { anchor: r.start, head: r.end, goal: None }], r.end);
    }

    /// The run of same-class characters at `pos` (prefers the char after it).
    fn word_range_at(&self, pos: usize) -> Range<usize> {
        let line = self.buffer.line_of(pos);
        let (start, end) = (self.buffer.line_start(line), self.buffer.line_end(line));
        let probe = if pos < end { pos } else if pos > start { pos - 1 } else { pos };
        if probe >= end {
            return pos..pos;
        }
        // Next to a word, prefer it over adjacent punctuation/space.
        let probe = if class(self.buffer.char(probe)) != CharClass::Word
            && probe > start
            && class(self.buffer.char(probe - 1)) == CharClass::Word
        {
            probe - 1
        } else {
            probe
        };
        let c = class(self.buffer.char(probe));
        let mut a = probe;
        while a > start && class(self.buffer.char(a - 1)) == c {
            a -= 1;
        }
        let mut b = probe;
        while b < end && class(self.buffer.char(b)) == c {
            b += 1;
        }
        a..b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str) -> Editor {
        Editor::new(Buffer::from_str(text))
    }

    fn text(e: &Editor) -> String {
        e.buffer.slice(0..e.buffer.len_chars())
    }

    #[test]
    fn typing_and_coalesced_undo() {
        let mut e = ed("");
        for c in ["a", "b", "c"] {
            e.insert_text(c);
        }
        assert_eq!(text(&e), "abc");
        assert!(e.is_dirty());
        e.undo();
        assert_eq!(text(&e), "");
        assert!(!e.is_dirty());
        e.redo();
        assert_eq!(text(&e), "abc");
        assert_eq!(e.primary().head, 3);
    }

    #[test]
    fn save_point_breaks_coalescing() {
        let mut e = ed("");
        e.insert_text("a");
        e.mark_saved();
        e.insert_text("b");
        assert!(e.is_dirty());
        e.undo();
        assert_eq!(text(&e), "a");
        assert!(!e.is_dirty());
    }

    #[test]
    fn backspace_joins_crlf_lines_as_one_step() {
        let mut e = ed("ab\r\ncd");
        e.click(4, false); // start of "cd"
        e.backspace(false);
        assert_eq!(text(&e), "abcd");
        assert_eq!(e.primary().head, 2);
        e.move_right(false, false);
        e.newline();
        assert_eq!(text(&e), "abc\r\nd");
    }

    #[test]
    fn newline_keeps_indent() {
        // Trailing "\n" pins the line ending to LF on every platform.
        let mut e = ed("    foo\n");
        e.click(7, false);
        e.newline();
        e.insert_text("bar");
        assert_eq!(text(&e), "    foo\n    bar\n");
    }

    #[test]
    fn multi_cursor_edits_every_selection() {
        let mut e = ed("one\ntwo\nthree");
        e.click(0, false);
        e.add_cursor(4);
        e.add_cursor(8);
        e.insert_text("> ");
        assert_eq!(text(&e), "> one\n> two\n> three");
        e.backspace(false);
        e.backspace(false);
        assert_eq!(text(&e), "one\ntwo\nthree");
        assert_eq!(e.selections().len(), 3);
    }

    #[test]
    fn vertical_move_remembers_goal_column() {
        let mut e = ed("long line\nab\nlong line");
        e.click(7, false);
        e.move_vertical(1, false);
        assert_eq!(e.primary().head, 12); // end of "ab"
        e.move_vertical(1, false);
        assert_eq!(e.primary().head, 20); // column 7 again
    }

    #[test]
    fn word_motion() {
        let mut e = ed("foo_bar  baz.qux");
        e.move_right(true, false);
        assert_eq!(e.primary().head, 7);
        e.move_right(true, false);
        assert_eq!(e.primary().head, 12);
        e.move_left(true, false);
        assert_eq!(e.primary().head, 9);
    }

    #[test]
    fn tabs_use_display_columns() {
        let mut e = ed("\tx\n    y");
        e.click(1, false); // after the tab = display col 4
        assert_eq!(e.display_col(1), 4);
        e.move_vertical(1, false);
        assert_eq!(e.primary().head, 7); // before "y"
    }

    #[test]
    fn paste_distributes_lines_across_cursors() {
        let mut e = ed("a\nb");
        e.click(1, false);
        e.add_cursor(3);
        e.paste("1\n2");
        assert_eq!(text(&e), "a1\nb2");
    }

    #[test]
    fn cut_without_selection_removes_line() {
        let mut e = ed("one\ntwo\nthree");
        e.click(5, false);
        assert_eq!(e.cut(), "two\n");
        assert_eq!(text(&e), "one\nthree");
    }

    #[test]
    fn cmd_d_expands_then_adds_whole_word_occurrences() {
        let mut e = ed("foo food foo\nfoo");
        e.click(1, false);
        e.select_next_occurrence();
        assert_eq!(e.primary().range(), 0..3);
        e.select_next_occurrence(); // skips "food"
        assert_eq!(e.primary().range(), 9..12);
        e.select_next_occurrence();
        e.select_next_occurrence(); // wraps; nothing new left
        assert_eq!(e.selections().len(), 3);
        e.insert_text("bar");
        assert_eq!(text(&e), "bar food bar\nbar");
        e.undo();
        assert_eq!(text(&e), "foo food foo\nfoo");
    }

    #[test]
    fn cmd_d_on_manual_selection_matches_substrings() {
        let mut e = ed("foo food");
        e.click(0, false);
        e.move_right(false, true);
        e.move_right(false, true);
        e.move_right(false, true);
        e.select_next_occurrence();
        assert_eq!(e.selections().len(), 2);
        assert_eq!(e.primary().range(), 4..7);
    }

    #[test]
    fn select_all_occurrences_and_replace_ranges() {
        let mut e = ed("a x a x a");
        e.click(0, false);
        e.select_all_occurrences();
        assert_eq!(e.selections().len(), 3);
        e.replace_ranges(vec![(2..3, "yy".into()), (6..7, "yy".into())]);
        assert_eq!(text(&e), "a yy a yy a");
        assert_eq!(e.selections().len(), 1);
        e.undo();
        assert_eq!(text(&e), "a x a x a");
    }

    #[test]
    fn snapshot_tracks_edits() {
        let mut e = ed("abc");
        let v = e.version();
        assert_eq!(*e.text_snapshot(), "abc");
        e.insert_text("z");
        assert!(e.version() > v);
        assert_eq!(*e.text_snapshot(), "zabc");
    }

    #[test]
    fn changed_from_tracks_first_edited_line() {
        let mut e = ed("a\nb\nc");
        e.take_changed_from();
        e.click(4, false);
        e.insert_text("x");
        assert_eq!(e.take_changed_from(), Some(2));
        assert_eq!(e.take_changed_from(), None);
    }
}
