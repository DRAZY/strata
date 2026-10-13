//! Editing model: buffer + selections + undo history. No UI code, so it is
//! unit-testable. Every command applies to every selection, so multiple
//! cursors only need UI to create them.


use crate::buffer::Buffer;
use crate::find::{self, Query};
use crate::indent;
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
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

    pub fn goal(&self) -> Option<usize> {
        self.goal
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

/// Undo history and saved point: one per buffer, shared by every view of it
/// (File ▸ New View into File), so undo and "unsaved" follow the text.
struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    next_id: u64,
    saved_id: u64,
    /// `Editor::key` of the view that recorded the last step: typing only
    /// coalesces into a step its own view made.
    last_owner: u64,
}

impl History {
    fn new() -> Rc<RefCell<History>> {
        Rc::new(RefCell::new(History { undo: Vec::new(), redo: Vec::new(), next_id: 1, saved_id: 0, last_owner: 0 }))
    }
}

/// A process-unique `Editor::key`.
fn next_key() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// One text replacement as applied to the buffer, for mirroring an edit
/// into the other views of the same buffer (see `Editor::new_view`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub at: usize,
    pub removed: usize,
    pub text: String,
}

pub struct Editor {
    pub buffer: Buffer,
    sels: Vec<Selection>,
    primary: usize,
    history: Rc<RefCell<History>>,
    /// Identifies this view in `History::last_owner`.
    key: u64,
    /// Splices since the last `take_journal`, kept only while other views
    /// share the buffer.
    journal: Option<Vec<Splice>>,
    can_coalesce: bool,
    changed_from: Option<usize>,
    /// Bumped on every text change; keys caches like search results.
    version: u64,
    snapshot: Option<(u64, Arc<String>)>,
    /// Cmd+D started from a bare cursor, so it matches whole words only.
    next_whole_word: bool,
    pub tab_width: usize,
    /// Indent with `tab_width` spaces rather than a tab character.
    pub use_spaces: bool,
    /// Enter copies the current line's indentation (Sublime's `auto_indent`).
    pub auto_indent: bool,
    /// Language rules adjust it: deeper after `{` or `:`, shallower for
    /// closers (Sublime's `smart_indent`).
    pub smart_indent: bool,
    pub indent_rules: indent::Rules,
    /// Sorted char positions; they move with edits.
    bookmarks: Vec<usize>,
    /// Emacs-style marks, one per cursor at the time they were set; they
    /// move with edits like bookmarks.
    marks: Vec<usize>,
    /// Index into `marks` of the primary cursor's mark.
    mark_primary: usize,
    /// Selection-only changes, oldest first, for soft undo.
    sel_past: Vec<SelStep>,
    /// Selection changes stepped back over by soft undo, newest last.
    sel_future: Vec<SelStep>,
    /// Merge class of the selection change about to happen (see `SelStep`).
    sel_merge: u8,
    rng: Rng,
    /// Ranges that follow edits (snippet fields); see `set_tracked`.
    tracked: Vec<Range<usize>>,
    /// Edits are ignored (views like Show Unsaved Changes); selection still works.
    pub read_only: bool,
}

/// Soft-undo history entries kept at most.
const SEL_HISTORY: usize = 200;

/// One selection-only change. `state` names the text it applies to (see
/// `Editor::text_state`), so steps from text that no longer exists are skipped.
struct SelStep {
    state: (u64, usize),
    before: (Vec<Selection>, usize),
    after: (Vec<Selection>, usize),
    /// Consecutive changes of the same nonzero class (plain moves, extending
    /// moves, drags) fold into one step.
    merge: u8,
}

const MERGE_MOVE: u8 = 1;
const MERGE_EXTEND: u8 = 2;
const MERGE_DRAG: u8 = 3;

/// xorshift64*: tiny seedable PRNG for the shuffle commands.
#[derive(Clone, Copy)]
struct Rng(u64);

impl Rng {
    fn from_time() -> Self {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        Rng(t.map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Fisher-Yates.
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

/// A replacement: char range in the text before the edit, and its new text.
type Edit = (Range<usize>, String);

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

/// How far bracket matching scans before giving up.
const BRACKET_SCAN: usize = 1_000_000;
const BRACKETS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// (pair index, is opener) for a bracket char.
fn bracket_kind(c: char) -> Option<(usize, bool)> {
    BRACKETS.iter().enumerate().find_map(|(k, &(o, cl))| {
        if c == o {
            Some((k, true))
        } else if c == cl {
            Some((k, false))
        } else {
            None
        }
    })
}

/// The closer auto-inserted after `c`, if it opens a pair.
fn auto_closer(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' | '\'' | '`' => Some(c),
        _ => None,
    }
}

fn is_closer(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '"' | '\'' | '`')
}

fn is_indent(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_word = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if in_word {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            in_word = true;
        } else {
            // "don't" stays one word.
            in_word = in_word && (c == '\'' || c == '\u{2019}');
            out.push(c);
        }
    }
    out
}

fn swap_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_lowercase() {
            out.extend(c.to_uppercase());
        } else if c.is_uppercase() {
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn capitalize(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect(),
        None => String::new(),
    }
}

/// Word separators for the case conversions.
fn is_case_sep(c: char) -> bool {
    c == '_' || c == '-' || is_indent(c)
}

/// Words of an identifier-like phrase: split at separators and case changes
/// (`fooBar`, `FooBAR` → Foo BAR, `HTTPServer` → HTTP Server). Digits stay
/// with the word before them (`utf8Decoder` → utf8 Decoder).
fn split_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    for chunk in s.split(is_case_sep) {
        let cs: Vec<char> = chunk.chars().collect();
        let mut cur = String::new();
        for (i, &c) in cs.iter().enumerate() {
            let prev = i.checked_sub(1).map(|j| cs[j]);
            let next = cs.get(i + 1);
            let boundary = c.is_uppercase()
                && prev.is_some_and(|p| {
                    p.is_lowercase() || p.is_numeric() || (p.is_uppercase() && next.is_some_and(|n| n.is_lowercase()))
                });
            if boundary && !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            cur.push(c);
        }
        if !cur.is_empty() {
            words.push(cur);
        }
    }
    words
}

/// Re-case every run of words and separators in `s` with `join`; other
/// chars (punctuation, line breaks) and a run's edge separators (`__init__`)
/// are kept.
fn convert_phrases(s: &str, join: &dyn Fn(&[String]) -> String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut phrase = String::new();
    let flush = |phrase: &mut String, out: &mut String| {
        let body = phrase.trim_matches(is_case_sep);
        if body.is_empty() {
            out.push_str(phrase);
        } else {
            let lead = phrase.len() - phrase.trim_start_matches(is_case_sep).len();
            let trail = phrase.trim_end_matches(is_case_sep).len();
            out.push_str(&phrase[..lead]);
            out.push_str(&join(&split_words(body)));
            out.push_str(&phrase[trail..]);
        }
        phrase.clear();
    };
    for c in s.chars() {
        if c.is_alphanumeric() || is_case_sep(c) {
            phrase.push(c);
        } else {
            flush(&mut phrase, &mut out);
            out.push(c);
        }
    }
    flush(&mut phrase, &mut out);
    out
}

/// Chars that may make up a line-comment (or quote/bullet) marker.
fn is_comment_char(c: char) -> bool {
    "/#-;*%!>|".contains(c)
}

/// The comment marker shared by every line (already stripped of indent),
/// e.g. `//` or `#`; empty when there is none.
fn common_comment_prefix<'a>(bodies: &[&'a str]) -> &'a str {
    let marker = |b: &str| b.chars().take_while(|&c| is_comment_char(c) || is_indent(c)).count();
    let first = bodies[0];
    let mut n = marker(first);
    for b in &bodies[1..] {
        let m = marker(b);
        n = first[..n].chars().zip(b[..m].chars()).take_while(|(x, y)| x == y).count();
    }
    first[..n].trim_end()
}

/// Chars scanned on each side of the cursor for word completions.
const COMPLETION_SCAN: usize = 1_000_000;

/// Map `pos` through sorted edits (old coordinates). At an insertion point,
/// `after` puts it past the inserted text; inside a replaced range it lands
/// at the range's start (or end of the new text with `after`).
fn map_pos(edits: &[Edit], pos: usize, after: bool) -> usize {
    let mut delta: isize = 0;
    for (r, text) in edits {
        if pos < r.start {
            break;
        }
        let n = text.chars().count() as isize;
        let at_insert = r.is_empty() && pos == r.start;
        if pos >= r.end && !(at_insert && !after) {
            delta += n - r.len() as isize;
        } else {
            let start = r.start as isize + delta;
            return (start + if after { n } else { 0 }) as usize;
        }
    }
    (pos as isize + delta) as usize
}

/// A selection mapped through edits: cursors stick after insertions,
/// selections grow to include text inserted at either edge.
/// `r` after replacing `remove` chars at `at` with `n` chars (see `Editor::set_tracked`).
fn track_edit(r: &Range<usize>, at: usize, remove: usize, n: usize) -> Range<usize> {
    let old_end = at + remove;
    let start = if r.start < at {
        r.start
    } else if r.start == at {
        if remove == 0 && r.end > r.start { r.start + n } else { r.start }
    } else if r.start >= old_end {
        r.start + n - remove
    } else {
        at
    };
    let end = if r.end < at || (r.end == at && remove > 0) {
        r.end
    } else if r.end >= old_end {
        r.end + n - remove
    } else {
        at + n
    };
    start.min(end)..end
}

fn map_sel(edits: &[Edit], s: &Selection) -> Selection {
    if s.is_empty() {
        return Selection::cursor(map_pos(edits, s.head, true));
    }
    let (a, b) = (map_pos(edits, s.start(), false), map_pos(edits, s.end(), true));
    span(a, b, s.head >= s.anchor)
}

fn span(start: usize, end: usize, forward: bool) -> Selection {
    if forward {
        Selection { anchor: start, head: end, goal: None }
    } else {
        Selection { anchor: end, head: start, goal: None }
    }
}

impl Editor {
    pub fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            sels: vec![Selection::cursor(0)],
            primary: 0,
            history: History::new(),
            key: next_key(),
            journal: None,
            can_coalesce: false,
            changed_from: Some(0),
            version: 0,
            snapshot: None,
            next_whole_word: false,
            tab_width: 4,
            use_spaces: true,
            auto_indent: true,
            smart_indent: true,
            indent_rules: indent::Rules::default(),
            bookmarks: Vec::new(),
            marks: Vec::new(),
            mark_primary: 0,
            sel_past: Vec::new(),
            sel_future: Vec::new(),
            sel_merge: 0,
            rng: Rng::from_time(),
            tracked: Vec::new(),
            read_only: false,
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
        self.history.borrow().undo.last().map_or(0, |t| t.id)
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Undo steps recorded so far; drops when an undo pops one.
    pub fn history_len(&self) -> usize {
        self.history.borrow().undo.len()
    }

    /// Ranges kept in step with every later edit (including undo): text typed
    /// inside or at the end of a range grows it, text inserted right before a
    /// non-empty range shifts it, and an empty range absorbs text typed at it.
    pub fn set_tracked(&mut self, ranges: Vec<Range<usize>>) {
        self.tracked = ranges;
    }

    pub fn tracked(&self) -> &[Range<usize>] {
        &self.tracked
    }

    /// Whole text as one string, cached until the next edit (for regex search).
    pub fn text_snapshot(&mut self) -> Arc<String> {
        if let Some((v, s)) = &self.snapshot
            && *v == self.version
        {
            return s.clone();
        }
        let s = Arc::new(self.buffer.slice(0..self.buffer.len_chars()));
        self.snapshot = Some((self.version, s.clone()));
        s
    }

    pub fn is_dirty(&self) -> bool {
        self.current_id() != self.history.borrow().saved_id
    }

    pub fn mark_saved(&mut self) {
        let id = self.current_id();
        self.history.borrow_mut().saved_id = id;
    }

    // ---- views sharing one buffer ---------------------------------------

    /// Another view of this buffer (New View into File): same text, undo
    /// history and saved state, its own selections, marks and soft undo.
    /// Edits made in either show up in the other once the app passes
    /// `take_journal` of one to `apply_foreign` of the other.
    pub fn new_view(&mut self) -> Editor {
        let mut buffer = Buffer::from_str(&self.text_snapshot());
        buffer.set_line_ending(self.buffer.line_ending());
        buffer.lossy = self.buffer.lossy;
        let mut view = Editor::new(buffer);
        view.history = self.history.clone();
        (view.tab_width, view.use_spaces, view.auto_indent, view.smart_indent) = (self.tab_width, self.use_spaces, self.auto_indent, self.smart_indent);
        view.indent_rules = self.indent_rules;
        view.read_only = self.read_only;
        view.place_selections(self.sels.clone(), self.primary().head);
        view.bookmarks = self.bookmarks.clone();
        self.journal.get_or_insert_with(Vec::new);
        view.journal = Some(Vec::new());
        view
    }

    /// Both are views of one buffer (they share undo history).
    pub fn shares_history(&self, other: &Editor) -> bool {
        Rc::ptr_eq(&self.history, &other.history)
    }

    /// Views of this buffer, this one included.
    pub fn view_count(&self) -> usize {
        Rc::strong_count(&self.history)
    }

    /// Distinct for every editor ever made (caches keyed on `version` add it,
    /// since a replaced editor starts counting again).
    pub fn id(&self) -> u64 {
        self.key
    }

    /// Edits are being recorded for other views.
    pub fn journaling(&self) -> bool {
        self.journal.is_some()
    }

    /// Record splices for other views (`take_journal`), or stop recording.
    pub fn set_journal(&mut self, on: bool) {
        if on != self.journal.is_some() {
            self.journal = on.then(Vec::new);
        }
    }

    /// Text changes since the last call, in the order they were applied.
    pub fn take_journal(&mut self) -> Vec<Splice> {
        self.journal.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Replay another view's edits. Nothing is recorded for undo (the
    /// history is shared); selections, bookmarks and marks move with the
    /// text like they do for this view's own edits.
    pub fn apply_foreign(&mut self, splices: &[Splice]) {
        if splices.is_empty() {
            return;
        }
        let journal = self.journal.take();
        let mut sels = std::mem::take(&mut self.sels);
        for s in splices {
            let at = s.at.min(self.buffer.len_chars());
            let removed = s.removed.min(self.buffer.len_chars() - at);
            self.splice(at, removed, &s.text);
            let n = s.text.chars().count();
            let shift = |p: &mut usize| {
                if *p >= at + removed && *p > at {
                    *p = *p + n - removed;
                } else if *p > at {
                    *p = at + n;
                }
            };
            for sel in &mut sels {
                shift(&mut sel.anchor);
                shift(&mut sel.head);
                sel.goal = None;
            }
        }
        let head = sels.get(self.primary).map_or(0, |s| s.head);
        self.place_selections(sels, head);
        self.journal = journal;
        self.can_coalesce = false;
    }

    /// Leave the shared history: this view keeps the text but starts an
    /// empty undo history of its own. Returns whether it had unsaved changes.
    pub fn detach_history(&mut self) -> bool {
        let dirty = self.is_dirty();
        self.history = History::new();
        self.journal = None;
        dirty
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

    /// A selection-only change: recorded for soft undo.
    fn set_selections(&mut self, sels: Vec<Selection>, primary_head: usize) {
        let before = (self.sels.clone(), self.primary);
        let merge = std::mem::take(&mut self.sel_merge);
        self.place_selections(sels, primary_head);
        let same = |a: &[Selection], b: &[Selection]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.anchor == y.anchor && x.head == y.head)
        };
        if same(&before.0, &self.sels) && before.1 == self.primary {
            return;
        }
        self.sel_future.clear();
        let state = self.text_state();
        let after = (self.sels.clone(), self.primary);
        if let Some(last) = self.sel_past.last_mut()
            && merge != 0
            && last.merge == merge
            && last.state == state
            && same(&last.after.0, &before.0)
        {
            last.after = after;
            return;
        }
        if self.sel_past.len() >= SEL_HISTORY {
            self.sel_past.remove(0);
        }
        self.sel_past.push(SelStep { state, before, after, merge });
    }

    /// Selections set by an edit or undo; not a soft-undo step of its own.
    fn place_selections(&mut self, sels: Vec<Selection>, primary_head: usize) {
        self.next_whole_word = false;
        self.sels = sels;
        self.normalize(primary_head);
    }

    /// Identifies the current text in undo history: the top transaction and
    /// how many changes it holds (typing coalesces into it).
    fn text_state(&self) -> (u64, usize) {
        (self.current_id(), self.history.borrow().undo.last().map_or(0, |t| t.changes.len()))
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

    /// Replace `remove` chars at `at` with `text`; bookmarks and marks inside
    /// the removed span collapse to its start, later ones shift.
    fn splice(&mut self, at: usize, remove: usize, text: &str) {
        self.note_change(at);
        if let Some(j) = &mut self.journal {
            j.push(Splice { at, removed: remove, text: text.to_owned() });
        }
        self.buffer.remove(at..at + remove);
        self.buffer.insert(at, text);
        let n = text.chars().count();
        for r in &mut self.tracked {
            *r = track_edit(r, at, remove, n);
        }
        if self.bookmarks.is_empty() && self.marks.is_empty() {
            return;
        }
        let shift = |b: &mut usize| {
            if *b >= at + remove && *b > at {
                *b = *b + n - remove;
            } else if *b > at {
                *b = at;
            }
        };
        self.bookmarks.iter_mut().for_each(shift);
        self.marks.iter_mut().for_each(shift);
        self.bookmarks.dedup();
    }

    // ---- the one edit primitive ------------------------------------------

    /// Apply one replacement per selection (same order as `self.sels`).
    /// Each selection collapses to the end of its inserted text.
    fn apply(&mut self, edits: Vec<Edit>, kind: Option<EditKind>) {
        debug_assert_eq!(edits.len(), self.sels.len());
        self.apply_landing(edits, kind, Land::EachEnd);
    }

    /// Apply sorted, non-overlapping replacements as one undo step.
    fn apply_landing(&mut self, edits: Vec<Edit>, kind: Option<EditKind>, land: Land) {
        if self.read_only {
            return;
        }
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
                let removed = self.buffer.slice(at..at + (end - start));
                self.splice(at, end - start, &text);
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
        self.place_selections(new_sels, primary_head);
        self.record(changes, before, kind);
    }

    /// Apply edits (sorted by start; overlaps are clipped) as one undo step,
    /// then select `sels`, given in post-edit coordinates.
    fn apply_then(
        &mut self,
        mut edits: Vec<Edit>,
        kind: Option<EditKind>,
        sels: Vec<Selection>,
        primary_head: usize,
    ) {
        if self.read_only {
            return;
        }
        edits.sort_by_key(|(r, _)| r.start);
        let before = self.sels.clone();
        let mut delta: isize = 0;
        let mut prev_end = 0;
        let mut changes = Vec::new();
        for (range, text) in edits {
            let start = range.start.max(prev_end);
            let end = range.end.max(start);
            prev_end = end;
            let at = (start as isize + delta) as usize;
            let removed = self.buffer.slice(at..at + (end - start));
            if removed == text {
                continue;
            }
            self.splice(at, end - start, &text);
            delta += text.chars().count() as isize - (end - start) as isize;
            changes.push(Change { at, removed, inserted: text });
        }
        self.place_selections(sels, primary_head);
        self.record(changes, before, kind);
    }

    /// Apply edits as one undo step, keeping each selection on the same text.
    fn apply_mapped(&mut self, mut edits: Vec<Edit>, kind: Option<EditKind>) {
        edits.sort_by_key(|(r, _)| r.start);
        let sels: Vec<Selection> = self.sels.iter().map(|s| map_sel(&edits, s)).collect();
        let head = sels[self.primary].head;
        self.apply_then(edits, kind, sels, head);
    }

    /// Push applied changes as one undo step, or fold them into the last one.
    fn record(&mut self, changes: Vec<Change>, before: Vec<Selection>, kind: Option<EditKind>) {
        if changes.is_empty() {
            return;
        }
        self.sel_future.clear();
        let mut h = self.history.borrow_mut();
        h.redo.clear();
        let (saved_id, mine) = (h.saved_id, h.last_owner == self.key);
        if let (Some(k), true, true, Some(last)) = (kind, self.can_coalesce, mine, h.undo.last_mut())
            && last.kind == Some(k)
            && last.id != saved_id
        {
            last.changes.extend(changes);
            last.after = self.sels.clone();
            return;
        }
        let id = h.next_id;
        h.undo.push(Transaction { id, changes, before, after: self.sels.clone(), kind });
        h.next_id += 1;
        h.last_owner = self.key;
        drop(h);
        self.can_coalesce = kind.is_some();
    }

    pub fn undo(&mut self) {
        let Some(t) = self.history.borrow_mut().undo.pop() else { return };
        for c in t.changes.iter().rev() {
            self.splice(c.at, c.inserted.chars().count(), &c.removed);
        }
        let head = t.before.first().map_or(0, |s| s.head);
        self.place_selections(t.before.clone(), head);
        self.history.borrow_mut().redo.push(t);
        self.can_coalesce = false;
    }

    pub fn redo(&mut self) {
        let Some(t) = self.history.borrow_mut().redo.pop() else { return };
        for c in &t.changes {
            self.splice(c.at, c.removed.chars().count(), &c.inserted);
        }
        let head = t.after.first().map_or(0, |s| s.head);
        self.place_selections(t.after.clone(), head);
        self.history.borrow_mut().undo.push(t);
        self.can_coalesce = false;
    }

    // ---- text input ------------------------------------------------------

    pub fn insert_text(&mut self, text: &str) {
        let edits = self.sels.iter().map(|s| (s.range(), text.to_owned())).collect();
        self.apply(edits, Some(EditKind::Typing));
    }

    /// Typed text: auto-pairs brackets and quotes, wraps selections in them,
    /// and types over a closer that is already next.
    pub fn type_char(&mut self, text: &str) {
        let mut chars = text.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return self.insert_text(text);
        };
        enum Act {
            Plain,
            Skip,
            Pair(char),
            Wrap(char),
            /// Re-indent the line while typing (see `typed_dedent`).
            Dedent(Range<usize>, String),
        }
        let acts: Vec<Act> = self
            .sels
            .iter()
            .map(|s| {
                let at_line_end = s.head >= self.buffer.line_end(self.buffer.line_of(s.head));
                let next = (!at_line_end).then(|| self.buffer.char(s.head));
                if s.is_empty() && is_closer(c) && next == Some(c) {
                    return Act::Skip;
                }
                let Some(close) = auto_closer(c) else {
                    return match s.is_empty().then(|| self.typed_dedent(s.head, c)).flatten() {
                        Some((r, t)) => Act::Dedent(r, t),
                        None => Act::Plain,
                    };
                };
                if !s.is_empty() {
                    return Act::Wrap(close);
                }
                let next_ok = next.is_none_or(|n| n.is_whitespace() || ")]},;".contains(n));
                let line_start = self.buffer.line_start(self.buffer.line_of(s.head));
                let prev = (s.head > line_start).then(|| self.buffer.char(s.head - 1));
                let quote_ok = close != c || prev.is_none_or(|p| class(p) != CharClass::Word && p != c);
                if next_ok && quote_ok { Act::Pair(close) } else { Act::Plain }
            })
            .collect();
        if acts.iter().all(|a| matches!(a, Act::Plain)) {
            return self.insert_text(text);
        }
        let mut edits = Vec::new();
        for (s, act) in self.sels.iter().zip(&acts) {
            match *act {
                Act::Plain => edits.push((s.range(), c.to_string())),
                Act::Skip => {}
                Act::Pair(close) => edits.push((s.head..s.head, format!("{c}{close}"))),
                Act::Dedent(ref r, ref t) => edits.push((r.clone(), t.clone())),
                Act::Wrap(close) => {
                    edits.push((s.start()..s.start(), c.to_string()));
                    edits.push((s.end()..s.end(), close.to_string()));
                }
            }
        }
        let sels: Vec<Selection> = self
            .sels
            .iter()
            .zip(&acts)
            .map(|(s, act)| match act {
                Act::Plain | Act::Dedent(..) => Selection::cursor(map_pos(&edits, s.start(), true)),
                Act::Skip | Act::Pair(_) => Selection::cursor(map_pos(&edits, s.head, false) + 1),
                Act::Wrap(_) => span(map_pos(&edits, s.start(), true), map_pos(&edits, s.end(), false), s.head >= s.anchor),
            })
            .collect();
        let head = sels[self.primary].head;
        let wraps = acts.iter().any(|a| matches!(a, Act::Wrap(_)));
        self.apply_then(edits, (!wraps).then_some(EditKind::Typing), sels, head);
    }

    /// Cursor sits between an auto-paired opener and its closer.
    fn inside_pair(&self, pos: usize) -> bool {
        pos > 0
            && pos < self.buffer.len_chars()
            && auto_closer(self.buffer.char(pos - 1)) == Some(self.buffer.char(pos))
    }

    /// Enter. With `auto_indent` the new line keeps this line's indentation;
    /// `smart_indent` goes a level deeper after an opener (`{`, Python's `:`,
    /// `<div>`…), shallower after `return` & co. or before a closer, and
    /// re-indents a line that ends a block (`end`, `fi`, `</div>`). Between a
    /// pair (`{|}`, `<a>|</a>`) the closer moves down and the caret lands on
    /// an indented line between. A whitespace-only line left behind is
    /// emptied. One undo step for every cursor.
    pub fn newline(&mut self) {
        let ending = self.buffer.line_ending().as_str();
        if !self.auto_indent {
            let edits = self.sels.iter().map(|s| (s.range(), ending.to_owned())).collect();
            return self.apply(edits, None);
        }
        let smart = self.smart_indent;
        let rules = self.indent_rules;
        let mut edits = Vec::with_capacity(self.sels.len());
        let mut sels = Vec::with_capacity(self.sels.len());
        let (mut primary_head, mut prev_end, mut delta) = (0, 0, 0isize);
        for (i, s) in self.sels.iter().enumerate() {
            let line = self.buffer.line_of(s.start());
            let ls = self.buffer.line_start(line);
            let before = self.buffer.slice(ls..s.start());
            let line_end = self.buffer.line_end(self.buffer.line_of(s.end()));
            let limit = self.sels.get(i + 1).map_or(line_end, |n| n.start().min(line_end)).max(s.end());
            let after = self.buffer.slice(s.end()..limit);
            let ws_after = after.chars().take_while(|&c| is_indent(c)).count();
            let rest: String = after.chars().skip(ws_after).collect();
            let mut indent: String = before.chars().take_while(|&c| is_indent(c)).collect();
            // Indent chars are ASCII, so byte and char counts agree.
            let body = &before[indent.len()..];

            // New text for the line up to the caret, when it changes.
            let mut prefix = None;
            if ls >= prev_end {
                if body.is_empty() && !indent.is_empty() {
                    prefix = Some(String::new());
                } else if smart
                    && rules.is_dedent_line(body)
                    && let Some(t) = self.dedent_target(line, &indent)
                {
                    prefix = Some(format!("{t}{body}"));
                    indent = t;
                }
            }

            let info = rules.analyze(&before);
            let split = info.open_bracket.and_then(auto_closer).is_some_and(|c| rest.starts_with(c))
                || (info.open_tag && rest.starts_with("</"));
            let shallower =
                |e: &Self, ind: &str| e.indent_string(e.indent_width(ind).saturating_sub(e.tab_width), e.use_spaces);
            let inner = if split || (smart && info.increase) {
                indent.clone() + &self.indent_unit()
            } else if smart && info.dedent_next {
                shallower(self, &indent)
            } else if smart && rest.starts_with([')', ']', '}']) {
                // The text moving down starts with a closer: line it up with its opener.
                let opener = rest.chars().next().and_then(|c| self.unmatched_opener(s.start(), c));
                opener.map_or_else(|| indent.clone(), |o| self.indent_of(self.buffer.line_of(o)))
            } else if smart && !rest.is_empty() && rules.is_dedent_line(&rest) {
                shallower(self, &indent)
            } else {
                indent.clone()
            };

            let start = if prefix.is_some() { ls } else { s.start() };
            let end = s.end() + ws_after;
            let mut text = prefix.unwrap_or_default();
            text.push_str(ending);
            text.push_str(&inner);
            let caret = text.chars().count();
            if split {
                text.push_str(ending);
                text.push_str(&indent);
            }
            let head = (start as isize + delta) as usize + caret;
            if i == self.primary {
                primary_head = head;
            }
            sels.push(Selection::cursor(head));
            delta += text.chars().count() as isize - (end - start) as isize;
            prev_end = end;
            edits.push((start..end, text));
        }
        self.apply_then(edits, None, sels, primary_head);
    }

    /// Indentation for line `line` that ends a block (`end`, `fi`, `</div>`):
    /// the previous line's when that opened the block, else one level
    /// shallower. None unless it is shallower than `current`.
    fn dedent_target(&self, line: usize, current: &str) -> Option<String> {
        let p = (0..line).rev().find(|&p| !self.is_blank(p))?;
        let prev = self.indent_of(p);
        let target = if self.indent_rules.analyze(&self.buffer.line_text(p)).increase {
            prev
        } else {
            self.indent_string(self.indent_width(&prev).saturating_sub(self.tab_width), self.use_spaces)
        };
        (self.indent_width(&target) < self.indent_width(current)).then_some(target)
    }

    /// Python `else:`/`elif`/`except`/`finally` lines up with the nearest
    /// shallower header it can close (`if`, `try`…), unless a header at the
    /// same depth already pairs with it.
    fn python_header_indent(&self, line: usize, current: &str, headers: &[&str]) -> Option<String> {
        let w = self.indent_width(current);
        for p in (0..line).rev().take(10_000) {
            let t = self.buffer.line_text(p);
            if t.trim().is_empty() {
                continue;
            }
            let pw = self.indent_width(&t);
            let header = headers.contains(&indent::first_word(t.trim_start()));
            if pw == w && header {
                return None;
            }
            if pw < w {
                return header.then(|| self.indent_of(p));
            }
        }
        None
    }

    /// The opener `closer` would close if typed at `pos`.
    fn unmatched_opener(&self, pos: usize, closer: char) -> Option<usize> {
        let (o, c) = BRACKETS[bracket_kind(closer)?.0];
        let text = self.buffer.slice(pos.saturating_sub(BRACKET_SCAN)..pos);
        let mut depth = 0usize;
        for (i, ch) in text.chars().rev().enumerate() {
            if ch == c {
                depth += 1;
            } else if ch == o {
                if depth == 0 {
                    return Some(pos - 1 - i);
                }
                depth -= 1;
            }
        }
        None
    }

    /// Typing `c` at `pos` completes a line that belongs shallower: a closing
    /// bracket alone on its line lines up with its opener, Python's `else:`
    /// & co. with their header, `</` with the block it closes. The edit
    /// rewrites the line up to the caret, `c` included.
    fn typed_dedent(&self, pos: usize, c: char) -> Option<(Range<usize>, String)> {
        if !(self.auto_indent && self.smart_indent) {
            return None;
        }
        let line = self.buffer.line_of(pos);
        let ls = self.buffer.line_start(line);
        let before = self.buffer.slice(ls..pos);
        let indent: String = before.chars().take_while(|&c| is_indent(c)).collect();
        let body = &before[indent.len()..];
        let target = match c {
            ')' | ']' | '}' if body.is_empty() => {
                let o = self.unmatched_opener(pos, c)?;
                self.indent_of(self.buffer.line_of(o))
            }
            ':' => {
                let headers = self.indent_rules.python_partner(body)?;
                self.python_header_indent(line, &indent, headers)?
            }
            '/' if body == "<" && self.indent_rules.kind == indent::Kind::Markup => self.dedent_target(line, &indent)?,
            _ => return None,
        };
        (target != indent).then(|| (ls..pos, format!("{target}{body}{c}")))
    }

    /// Soft tab (spaces up to the next tab stop), or '\t' without `use_spaces`.
    pub fn tab(&mut self) {
        let edits = self
            .sels
            .iter()
            .map(|s| {
                if !self.use_spaces {
                    return (s.range(), "\t".to_owned());
                }
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
                } else if self.inside_pair(s.head) {
                    s.head - 1..s.head + 1
                } else if let Some(stop) = self.tab_stop_before(s.head) {
                    stop..s.head
                } else {
                    self.left(s.head)..s.head
                };
                (range, String::new())
            })
            .collect();
        self.apply(edits, Some(EditKind::Deleting));
    }

    /// With `use_spaces`, Backspace in leading spaces goes back to the
    /// previous tab stop.
    fn tab_stop_before(&self, pos: usize) -> Option<usize> {
        let start = self.buffer.line_start(self.buffer.line_of(pos));
        let n = pos - start;
        let spaces = n > 1
            && self.use_spaces
            && self.tab_width > 1
            && self.buffer.slice(start..pos).chars().all(|c| c == ' ');
        spaces.then(|| pos - ((n - 1) % self.tab_width + 1))
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

    /// Edit ▸ Text ▸ Delete Word Forward.
    pub fn delete_word_forward(&mut self) {
        self.delete_forward(true);
    }

    /// Edit ▸ Text ▸ Delete Word Backward.
    pub fn delete_word_backward(&mut self) {
        self.backspace(true);
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

    // ---- line helpers ----------------------------------------------------

    /// Lines a selection covers; a selection ending at column 0 leaves that line out.
    fn sel_lines(&self, s: &Selection) -> Range<usize> {
        let first = self.buffer.line_of(s.start());
        let mut last = self.buffer.line_of(s.end());
        if last > first && s.end() == self.buffer.line_start(last) {
            last -= 1;
        }
        first..last + 1
    }

    /// Every covered line once, ascending.
    fn covered_lines(&self) -> Vec<usize> {
        let mut lines: Vec<usize> = self.sels.iter().flat_map(|s| self.sel_lines(s)).collect();
        lines.dedup();
        lines
    }

    /// Covered lines merged into contiguous blocks.
    fn line_blocks(&self) -> Vec<Range<usize>> {
        let mut blocks: Vec<Range<usize>> = Vec::new();
        for s in &self.sels {
            let r = self.sel_lines(s);
            match blocks.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => blocks.push(r),
            }
        }
        blocks
    }

    /// Lines the sort/unique/trim transforms act on: the selected blocks, or
    /// the whole buffer (minus a trailing empty line) for a lone cursor.
    fn transform_blocks(&self) -> (Vec<Range<usize>>, bool) {
        if self.sels.len() == 1 && self.sels[0].is_empty() {
            let mut n = self.buffer.len_lines();
            if n > 1 && self.buffer.line_start(n - 1) == self.buffer.len_chars() {
                n -= 1;
            }
            (std::iter::once(0..n).collect(), true)
        } else {
            (self.line_blocks(), false)
        }
    }

    fn indent_of(&self, line: usize) -> String {
        self.buffer.line_text(line).chars().take_while(|&c| is_indent(c)).collect()
    }

    fn indent_unit(&self) -> String {
        if self.use_spaces { " ".repeat(self.tab_width) } else { "\t".to_owned() }
    }

    /// Char index within `line`'s indentation where display column `col` starts.
    fn pos_in_indent(&self, line: usize, col: usize) -> usize {
        let mut pos = self.buffer.line_start(line);
        let mut cur = 0;
        for c in self.buffer.line_text(line).chars() {
            if cur >= col || !is_indent(c) {
                break;
            }
            cur = self.advance_col(cur, c);
            pos += 1;
        }
        pos
    }

    // ---- line commands ---------------------------------------------------

    /// Indent every covered line by one unit (skipping empty lines unless only one).
    pub fn indent(&mut self) {
        let unit = self.indent_unit();
        let lines = self.covered_lines();
        let keep_empty = lines.len() == 1;
        let edits = lines
            .into_iter()
            .filter(|&l| keep_empty || self.buffer.line_end(l) > self.buffer.line_start(l))
            .map(|l| {
                let p = self.buffer.line_start(l);
                (p..p, unit.clone())
            })
            .collect();
        self.apply_mapped(edits, None);
    }

    /// Remove up to one indent unit from every covered line.
    pub fn outdent(&mut self) {
        let edits = self
            .covered_lines()
            .into_iter()
            .filter_map(|l| {
                let text = self.buffer.line_text(l);
                let n = if text.starts_with('\t') {
                    1
                } else {
                    text.chars().take(self.tab_width).take_while(|&c| c == ' ').count()
                };
                let start = self.buffer.line_start(l);
                (n > 0).then(|| (start..start + n, String::new()))
            })
            .collect();
        self.apply_mapped(edits, None);
    }

    /// Line comments: uncomment if every non-blank covered line is commented,
    /// else comment them all at their shared minimum indent.
    pub fn toggle_comment(&mut self, token: &str) {
        let tok_len = token.chars().count();
        let lines: Vec<(usize, String)> =
            self.covered_lines().into_iter().map(|l| (l, self.buffer.line_text(l))).collect();
        let filled: Vec<&(usize, String)> = lines.iter().filter(|(_, t)| !t.trim().is_empty()).collect();
        let insert = format!("{token} ");
        let edits = if filled.is_empty() {
            lines
                .iter()
                .map(|(l, _)| {
                    let p = self.buffer.line_end(*l);
                    (p..p, insert.clone())
                })
                .collect()
        } else if filled.iter().all(|(_, t)| t.trim_start().starts_with(token)) {
            filled
                .iter()
                .map(|(l, t)| {
                    let ws = t.chars().take_while(|c| c.is_whitespace()).count();
                    let start = self.buffer.line_start(*l) + ws;
                    let space = t.chars().nth(ws + tok_len) == Some(' ');
                    (start..start + tok_len + space as usize, String::new())
                })
                .collect()
        } else {
            let col = filled
                .iter()
                .map(|(l, t)| {
                    let ws = t.chars().take_while(|&c| is_indent(c)).count();
                    self.display_col(self.buffer.line_start(*l) + ws)
                })
                .min()
                .unwrap_or(0);
            filled
                .iter()
                .map(|(l, _)| {
                    let p = self.pos_in_indent(*l, col);
                    (p..p, insert.clone())
                })
                .collect()
        };
        self.apply_mapped(edits, None);
    }

    /// Block comments: wrap each selection (or its line's text when empty),
    /// or unwrap it if it is already wrapped.
    pub fn toggle_block_comment(&mut self, open: &str, close: &str) {
        let (o, c) = (open.chars().count(), close.chars().count());
        let mut edits = Vec::new();
        let mut last_line = None;
        for s in &self.sels {
            let (range, pad) = if s.is_empty() {
                let line = self.buffer.line_of(s.head);
                if last_line == Some(line) {
                    continue;
                }
                last_line = Some(line);
                let text = self.buffer.line_text(line);
                let start = self.buffer.line_start(line);
                let lead = text.chars().take_while(|c| c.is_whitespace()).count();
                let end = start + text.trim_end().chars().count();
                if end <= start + lead {
                    continue;
                }
                (start + lead..end, " ")
            } else {
                (s.range(), "")
            };
            let text = self.buffer.slice(range.clone());
            if text.chars().count() >= o + c && text.starts_with(open) && text.ends_with(close) {
                let mut a = range.start + o;
                let mut b = range.end - c;
                if a < b && self.buffer.char(a) == ' ' {
                    a += 1;
                }
                if a < b && self.buffer.char(b - 1) == ' ' {
                    b -= 1;
                }
                edits.push((range.start..a, String::new()));
                edits.push((b..range.end, String::new()));
            } else {
                edits.push((range.start..range.start, format!("{open}{pad}")));
                edits.push((range.end..range.end, format!("{pad}{close}")));
            }
        }
        self.apply_mapped(edits, None);
    }

    /// A cursor duplicates its line below and moves onto the copy; a
    /// selection duplicates its text right after itself and selects the copy.
    pub fn duplicate_lines(&mut self) {
        let ending = self.buffer.line_ending().as_str();
        let mut edits = Vec::new();
        let mut last_line = None;
        for s in &self.sels {
            if s.is_empty() {
                let line = self.buffer.line_of(s.head);
                if last_line != Some(line) {
                    last_line = Some(line);
                    let p = self.buffer.line_end(line);
                    edits.push((p..p, format!("{ending}{}", self.buffer.line_text(line))));
                }
            } else {
                edits.push((s.end()..s.end(), self.buffer.slice(s.range())));
            }
        }
        let sels: Vec<Selection> = self
            .sels
            .iter()
            .map(|s| {
                if s.is_empty() {
                    let line = self.buffer.line_of(s.head);
                    let n = ending.chars().count() + self.buffer.line_end(line) - self.buffer.line_start(line);
                    Selection::cursor(map_pos(&edits, s.head, false) + n)
                } else {
                    let a = map_pos(&edits, s.end(), false);
                    span(a, a + s.end() - s.start(), s.head >= s.anchor)
                }
            })
            .collect();
        let head = sels[self.primary].head;
        self.apply_then(edits, None, sels, head);
    }

    /// Delete every covered line, line break included.
    pub fn delete_lines(&mut self) {
        let (lines, len) = (self.buffer.len_lines(), self.buffer.len_chars());
        let edits: Vec<Edit> = self
            .line_blocks()
            .into_iter()
            .map(|b| {
                let range = if b.end < lines {
                    self.buffer.line_start(b.start)..self.buffer.line_start(b.end)
                } else if b.start > 0 {
                    self.buffer.line_end(b.start - 1)..len
                } else {
                    0..len
                };
                (range, String::new())
            })
            .collect();
        let sels = edits.iter().map(|(r, _)| Selection::cursor(map_pos(&edits, r.start, false))).collect();
        let head = map_pos(&edits, self.primary().head, false);
        self.apply_then(edits, None, sels, head);
    }

    /// Swap each covered line block with the line above (or below).
    pub fn move_lines(&mut self, up: bool) {
        let blocks = self.line_blocks();
        let lines = self.buffer.len_lines();
        let at_edge = match (blocks.first(), blocks.last()) {
            (Some(first), Some(last)) => {
                if up { first.start == 0 } else { last.end >= lines }
            }
            _ => true,
        };
        if at_edge {
            return;
        }
        let ending = self.buffer.line_ending().as_str();
        let b = &self.buffer;
        // (moved char span in old coordinates, its shift, the edit)
        let moves: Vec<(Range<usize>, isize, Edit)> = blocks
            .iter()
            .map(|bl| {
                let moved = b.line_start(bl.start)..b.line_start(bl.end);
                let block = b.slice(b.line_start(bl.start)..b.line_end(bl.end - 1));
                if up {
                    let other = bl.start - 1;
                    let range = b.line_start(other)..b.line_end(bl.end - 1);
                    let shift = b.line_start(other) as isize - b.line_start(bl.start) as isize;
                    (moved, shift, (range, format!("{block}{ending}{}", b.line_text(other))))
                } else {
                    let other = b.line_text(bl.end);
                    let range = b.line_start(bl.start)..b.line_end(bl.end);
                    let shift = (other.chars().count() + ending.chars().count()) as isize;
                    (moved, shift, (range, format!("{other}{ending}{block}")))
                }
            })
            .collect();
        let shift_pos = |p: usize| {
            let mut delta: isize = 0;
            for (moved, shift, (range, text)) in &moves {
                if moved.start <= p && p <= moved.end {
                    return (p as isize + delta + shift) as usize;
                }
                delta += text.chars().count() as isize - range.len() as isize;
            }
            (p as isize + delta) as usize
        };
        let sels: Vec<Selection> = self
            .sels
            .iter()
            .map(|s| Selection { anchor: shift_pos(s.anchor), head: shift_pos(s.head), goal: s.goal })
            .collect();
        let head = sels[self.primary].head;
        let edits = moves.into_iter().map(|(_, _, e)| e).collect();
        self.apply_then(edits, None, sels, head);
    }

    /// Join each covered line with the next (a cursor's line with the one
    /// below), replacing the break and leading indent with one space.
    pub fn join_lines(&mut self) {
        let lines = self.buffer.len_lines();
        let mut joins: Vec<usize> = Vec::new();
        for s in &self.sels {
            let first = self.buffer.line_of(s.start());
            let last = self.buffer.line_of(s.end()).max(first + 1);
            joins.extend(first..last);
        }
        joins.sort_unstable();
        joins.dedup();
        joins.retain(|&l| l + 1 < lines);
        let edits: Vec<Edit> = joins
            .iter()
            .map(|&l| {
                let next = self.buffer.line_text(l + 1);
                let ws = next.chars().take_while(|&c| is_indent(c)).count();
                let first = next.chars().nth(ws);
                let bare = self.buffer.line_end(l) == self.buffer.line_start(l);
                let space = if bare || first.is_none_or(|c| ")]}".contains(c)) { "" } else { " " };
                (self.buffer.line_end(l)..self.buffer.line_start(l + 1) + ws, space.to_owned())
            })
            .collect();
        let sels: Vec<Selection> = self
            .sels
            .iter()
            .map(|s| {
                if s.is_empty() {
                    let end = self.buffer.line_end(self.buffer.line_of(s.head));
                    Selection::cursor(map_pos(&edits, end, false))
                } else {
                    map_sel(&edits, s)
                }
            })
            .collect();
        let head = sels[self.primary].head;
        self.apply_then(edits, None, sels, head);
    }

    /// Select whole lines, line break included; repeating adds the next line.
    pub fn expand_to_line(&mut self) {
        let sels: Vec<Selection> = self
            .sels
            .iter()
            .map(|s| {
                let lines = self.sel_lines(s);
                let start = self.buffer.line_start(lines.start);
                let mut end = self.buffer.line_start(lines.end);
                if !s.is_empty() && s.start() == start && s.end() == end {
                    end = self.buffer.line_start(lines.end + 1);
                }
                Selection { anchor: start, head: end, goal: None }
            })
            .collect();
        let head = sels[self.primary].head;
        self.set_selections(sels, head);
        self.can_coalesce = false;
    }

    /// Split each multi-line selection into one selection per line.
    pub fn split_into_lines(&mut self) {
        let mut sels = Vec::new();
        for s in &self.sels {
            let (first, last) = (self.buffer.line_of(s.start()), self.buffer.line_of(s.end()));
            if first == last {
                sels.push(*s);
                continue;
            }
            for line in first..=last {
                if line == last && s.end() == self.buffer.line_start(line) {
                    break;
                }
                let a = s.start().max(self.buffer.line_start(line));
                let b = s.end().min(self.buffer.line_end(line));
                sels.push(Selection { anchor: a, head: b, goal: None });
            }
        }
        let head = self.primary().head;
        self.set_selections(sels, head);
        self.can_coalesce = false;
    }

    pub fn insert_line_after(&mut self) {
        self.insert_line(false);
    }

    pub fn insert_line_before(&mut self) {
        self.insert_line(true);
    }

    /// Open an indented line below (or above) each cursor's line.
    fn insert_line(&mut self, before: bool) {
        let ending = self.buffer.line_ending().as_str();
        let line_of = |s: &Selection| self.buffer.line_of(if before { s.start() } else { s.end() });
        let mut lines: Vec<usize> = self.sels.iter().map(line_of).collect();
        lines.dedup();
        let indents: Vec<String> = lines.iter().map(|&l| self.indent_of(l)).collect();
        let edits: Vec<Edit> = lines
            .iter()
            .zip(&indents)
            .map(|(&l, indent)| {
                if before {
                    let p = self.buffer.line_start(l);
                    (p..p, format!("{indent}{ending}"))
                } else {
                    let p = self.buffer.line_end(l);
                    (p..p, format!("{ending}{indent}"))
                }
            })
            .collect();
        let sels: Vec<Selection> = edits
            .iter()
            .zip(&indents)
            .map(|((r, _), indent)| {
                Selection::cursor(if before {
                    map_pos(&edits, r.start, false) + indent.chars().count()
                } else {
                    map_pos(&edits, r.start, true)
                })
            })
            .collect();
        let primary_line = line_of(&self.primary());
        let head = sels[lines.iter().position(|&l| l == primary_line).unwrap_or(0)].head;
        self.apply_then(edits, None, sels, head);
    }

    /// Sort the selected lines (or the whole buffer for a lone cursor).
    pub fn sort_lines(&mut self, case_sensitive: bool) {
        self.transform_lines(|mut lines| {
            if case_sensitive {
                lines.sort();
            } else {
                lines.sort_by_cached_key(|l| l.to_lowercase());
            }
            lines
        });
    }

    pub fn reverse_lines(&mut self) {
        self.transform_lines(|mut lines| {
            lines.reverse();
            lines
        });
    }

    /// Drop repeated lines, keeping the first of each.
    pub fn unique_lines(&mut self) {
        self.transform_lines(|lines| {
            let mut seen = std::collections::HashSet::new();
            lines.into_iter().filter(|l| seen.insert(l.clone())).collect()
        });
    }

    fn transform_lines(&mut self, mut f: impl FnMut(Vec<String>) -> Vec<String>) {
        let ending = self.buffer.line_ending().as_str();
        let (blocks, whole) = self.transform_blocks();
        let edits: Vec<Edit> = blocks
            .iter()
            .map(|b| {
                let lines = b.clone().map(|l| self.buffer.line_text(l)).collect();
                let range = self.buffer.line_start(b.start)..self.buffer.line_end(b.end - 1);
                (range, f(lines).join(ending))
            })
            .collect();
        let (sels, head) = if whole {
            let p = self.primary();
            (vec![p], p.head)
        } else {
            let sels: Vec<Selection> = edits
                .iter()
                .map(|(r, t)| {
                    let a = map_pos(&edits, r.start, false);
                    span(a, a + t.chars().count(), true)
                })
                .collect();
            let head = sels.last().map_or(0, |s| s.head);
            (sels, head)
        };
        self.apply_then(edits, None, sels, head);
    }

    /// Strip trailing whitespace from the covered lines (whole buffer for a lone cursor).
    pub fn trim_trailing_whitespace(&mut self) {
        let (blocks, _) = self.transform_blocks();
        let edits = blocks
            .into_iter()
            .flatten()
            .filter_map(|l| {
                let text = self.buffer.line_text(l);
                let end = self.buffer.line_end(l);
                let start = self.buffer.line_start(l) + text.trim_end().chars().count();
                (start < end).then(|| (start..end, String::new()))
            })
            .collect();
        self.apply_mapped(edits, None);
    }

    pub fn swap_case_upper(&mut self) {
        self.transform_selected(|s| s.to_uppercase());
    }

    pub fn swap_case_lower(&mut self) {
        self.transform_selected(|s| s.to_lowercase());
    }

    /// Rewrite each selection's text (a cursor's word when empty).
    fn transform_selected(&mut self, f: impl Fn(&str) -> String) {
        self.transform_selected_with(Self::word_range_at, f);
    }

    /// Rewrite each selection's text (a cursor's `range_at` when empty).
    fn transform_selected_with(&mut self, range_at: fn(&Self, usize) -> Range<usize>, f: impl Fn(&str) -> String) {
        let mut ranges: Vec<Range<usize>> = Vec::new();
        for s in &self.sels {
            let r = if s.is_empty() { range_at(self, s.head) } else { s.range() };
            if !r.is_empty() && ranges.last().is_none_or(|last| r.start >= last.end) {
                ranges.push(r);
            }
        }
        let edits: Vec<Edit> =
            ranges.into_iter().map(|r| (r.clone(), f(&self.buffer.slice(r)))).collect();
        let sels: Vec<Selection> = self
            .sels
            .iter()
            .map(|s| {
                if !s.is_empty() {
                    return map_sel(&edits, s);
                }
                let Some((r, t)) = edits.iter().find(|(r, _)| r.start <= s.head && s.head <= r.end) else {
                    return Selection::cursor(map_pos(&edits, s.head, false));
                };
                let offset = (s.head - r.start).min(t.chars().count());
                Selection::cursor(map_pos(&edits, r.start, false) + offset)
            })
            .collect();
        let head = sels[self.primary].head;
        self.apply_then(edits, None, sels, head);
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

    /// Move every head to `f`'s position with its new goal column; without
    /// `extend` the anchor follows.
    pub fn move_heads(&mut self, extend: bool, f: impl Fn(&Self, &Selection) -> (usize, Option<usize>)) {
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
        self.sel_merge = if extend { MERGE_EXTEND } else { MERGE_MOVE };
        self.set_selections(sels, primary_head);
        self.can_coalesce = false;
    }

    pub fn move_left(&mut self, word: bool, extend: bool) {
        self.move_heads(extend, |e, s| {
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
        self.move_heads(extend, |e, s| {
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
        self.move_heads(extend, |e, s| {
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
        self.move_heads(extend, |e, s| {
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
        self.move_heads(extend, |e, s| (e.buffer.line_end(e.buffer.line_of(s.head)), None));
    }

    pub fn move_doc_start(&mut self, extend: bool) {
        self.move_heads(extend, |_, _| (0, None));
    }

    pub fn move_doc_end(&mut self, extend: bool) {
        self.move_heads(extend, |e, _| (e.buffer.len_chars(), None));
    }

    pub fn select_all(&mut self) {
        let len = self.buffer.len_chars();
        self.set_selections(vec![Selection { anchor: 0, head: len, goal: None }], len);
        self.can_coalesce = false;
    }

    // ---- find / Cmd+D -------------------------------------------------

    /// Replace the selections with `(anchor, head)` pairs (direction kept),
    /// clamped to the text; `primary` indexes into them.
    pub fn set_selection_pairs(&mut self, sels: &[(usize, usize)], primary: usize) {
        if sels.is_empty() {
            return;
        }
        let len = self.buffer.len_chars();
        let list = sels.iter().map(|&(a, h)| Selection { anchor: a.min(len), head: h.min(len), goal: None }).collect();
        self.set_selections(list, sels[primary.min(sels.len() - 1)].1.min(len));
        self.can_coalesce = false;
    }

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
    pub fn replace_ranges(&mut self, edits: Vec<Edit>) {
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
        self.sel_merge = MERGE_DRAG;
        self.set_selections(sels, pos);
    }

    pub fn select_word_at(&mut self, pos: usize) {
        let r = self.word_range_at(pos);
        self.set_selections(vec![Selection { anchor: r.start, head: r.end, goal: None }], r.end);
    }

    /// Box selection in display columns: one selection per line, skipping
    /// lines shorter than the left column (keeping at least one cursor).
    pub fn select_box(&mut self, from_line: usize, from_col: usize, to_line: usize, to_col: usize) {
        let last = self.buffer.len_lines() - 1;
        let (from_line, to_line) = (from_line.min(last), to_line.min(last));
        let left = from_col.min(to_col);
        let mut sels = Vec::new();
        let mut primary_head = None;
        for line in from_line.min(to_line)..=from_line.max(to_line) {
            if self.display_col(self.buffer.line_end(line)) < left {
                continue;
            }
            let anchor = self.pos_at_display_col(line, from_col);
            let head = self.pos_at_display_col(line, to_col);
            if line == to_line {
                primary_head = Some(head);
            }
            sels.push(Selection { anchor, head, goal: None });
        }
        if sels.is_empty() {
            sels.push(Selection::cursor(self.pos_at_display_col(to_line, to_col)));
        }
        let head = primary_head.unwrap_or(sels[sels.len() - 1].head);
        self.set_selections(sels, head);
        self.can_coalesce = false;
    }

    /// Add a cursor on the line above the topmost (or below the bottommost)
    /// cursor, at the primary's goal column. The new cursor becomes primary.
    pub fn add_cursor_vertical(&mut self, dir: isize) {
        let p = self.primary();
        let goal = p.goal.unwrap_or_else(|| self.display_col(p.head));
        let edge = if dir < 0 { self.sels[0] } else { self.sels[self.sels.len() - 1] };
        let line = self.buffer.line_of(edge.head) as isize + dir.signum();
        if line < 0 || line >= self.buffer.len_lines() as isize {
            return;
        }
        let pos = self.pos_at_display_col(line as usize, goal);
        let mut sels = self.sels.clone();
        sels.push(Selection { anchor: pos, head: pos, goal: Some(goal) });
        self.set_selections(sels, pos);
        self.can_coalesce = false;
    }

    // ---- more text commands ------------------------------------------------

    /// Paste, shifting the pasted lines so the first one takes the target
    /// line's indentation (relative indentation inside the paste is kept).
    pub fn paste_and_indent(&mut self, text: &str) {
        let src = text.replace("\r\n", "\n");
        let lines: Vec<&str> = src.split('\n').collect();
        let blank = |l: &str| l.trim().is_empty();
        let Some(first) = lines.iter().find(|l| !blank(l)) else {
            return self.paste(text);
        };
        let ending = self.buffer.line_ending().as_str();
        let base = self.indent_width(first);
        let whole_lines = src.ends_with('\n');
        let edits = self
            .sels
            .iter()
            .map(|s| {
                let line = self.buffer.line_of(s.start());
                let ls = self.buffer.line_start(line);
                let target = self.indent_width(&self.buffer.line_text(line));
                let at_indent = self.buffer.slice(ls..s.start()).chars().all(is_indent);
                let range = if !at_indent {
                    s.range()
                } else if whole_lines && s.is_empty() {
                    ls..ls
                } else {
                    ls..s.end()
                };
                let out: Vec<String> = lines
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let body = l.trim_start_matches(is_indent);
                        if blank(l) {
                            String::new()
                        } else if i == 0 && !at_indent {
                            body.to_owned()
                        } else {
                            let w = (self.indent_width(l) + target).saturating_sub(base);
                            self.indent_string(w, self.use_spaces) + body
                        }
                    })
                    .collect();
                (range, out.join(ending))
            })
            .collect();
        self.apply(edits, None);
    }

    /// Delete to the end of the line; at the end, delete the line break.
    pub fn delete_to_line_end(&mut self) {
        let edits = self
            .sels
            .iter()
            .map(|s| {
                let end = self.buffer.line_end(self.buffer.line_of(s.head));
                let range = if !s.is_empty() {
                    s.range()
                } else if s.head < end {
                    s.head..end
                } else {
                    s.head..self.right(s.head)
                };
                (range, String::new())
            })
            .collect();
        self.apply(edits, None);
    }

    /// Cursors: swap the chars around each (the last two at a line end) and
    /// step right. Selections: each takes the next selection's text.
    pub fn transpose(&mut self) {
        if !self.all_empty() {
            return self.permute_selections(|mut t| {
                t.rotate_left(1);
                t
            });
        }
        let mut edits: Vec<Edit> = Vec::new();
        let mut heads = Vec::with_capacity(self.sels.len());
        for s in &self.sels {
            let line = self.buffer.line_of(s.head);
            let (ls, le) = (self.buffer.line_start(line), self.buffer.line_end(line));
            let mid = if s.head >= le { le.saturating_sub(1) } else { s.head };
            let r = mid.saturating_sub(1)..mid + 1;
            if mid <= ls || edits.last().is_some_and(|(last, _)| last.end > r.start) {
                heads.push(s.head);
                continue;
            }
            let (a, b) = (self.buffer.char(mid - 1), self.buffer.char(mid));
            heads.push(r.end);
            edits.push((r, format!("{b}{a}")));
        }
        let sels: Vec<Selection> = heads.into_iter().map(Selection::cursor).collect();
        let head = sels[self.primary].head;
        self.apply_then(edits, None, sels, head);
    }

    pub fn title_case(&mut self) {
        self.transform_selected(title_case);
    }

    pub fn swap_case(&mut self) {
        self.transform_selected(swap_case);
    }

    /// Shuffle the selected lines (or the whole buffer for a lone cursor).
    pub fn shuffle_lines(&mut self) {
        let mut rng = self.rng;
        self.transform_lines(|mut lines| {
            rng.shuffle(&mut lines);
            lines
        });
        self.rng = rng;
    }

    pub fn shuffle_selections(&mut self) {
        let mut rng = self.rng;
        self.permute_selections(|mut t| {
            rng.shuffle(&mut t);
            t
        });
        self.rng = rng;
    }

    pub fn sort_selections(&mut self) {
        self.permute_selections(|mut t| {
            t.sort();
            t
        });
    }

    /// Sort the selections' texts ignoring case (ties keep their order).
    pub fn sort_selections_case_insensitive(&mut self) {
        self.permute_selections(|mut t| {
            t.sort_by_cached_key(|x| x.to_lowercase());
            t
        });
    }

    pub fn reverse_selections(&mut self) {
        self.permute_selections(|mut t| {
            t.reverse();
            t
        });
    }

    /// Repeated selection texts become empty, keeping the first of each.
    pub fn unique_selections(&mut self) {
        self.permute_selections(|t| {
            let mut seen = std::collections::HashSet::new();
            t.into_iter().map(|x| if seen.insert(x.clone()) { x } else { String::new() }).collect()
        });
    }

    /// Rearrange the selections' texts among themselves (needs two or more);
    /// each selection then covers its new text.
    fn permute_selections(&mut self, f: impl FnOnce(Vec<String>) -> Vec<String>) {
        if self.sels.len() < 2 {
            return;
        }
        let texts = self.sels.iter().map(|s| self.buffer.slice(s.range())).collect();
        let edits: Vec<Edit> = self.sels.iter().zip(f(texts)).map(|(s, t)| (s.range(), t)).collect();
        let sels: Vec<Selection> = self
            .sels
            .iter()
            .zip(&edits)
            .map(|(s, (r, t))| {
                let a = map_pos(&edits, r.start, false);
                span(a, a + t.chars().count(), s.head >= s.anchor)
            })
            .collect();
        let head = sels[self.primary].head;
        self.apply_then(edits, None, sels, head);
    }

    // ---- indentation -------------------------------------------------------

    /// Display width of a line's leading whitespace.
    fn indent_width(&self, line_text: &str) -> usize {
        line_text.chars().take_while(|&c| is_indent(c)).fold(0, |col, c| self.advance_col(col, c))
    }

    /// Indentation `width` columns wide, as spaces or as tabs (plus spaces for the remainder).
    fn indent_string(&self, width: usize, spaces: bool) -> String {
        if spaces {
            " ".repeat(width)
        } else {
            "\t".repeat(width / self.tab_width) + &" ".repeat(width % self.tab_width)
        }
    }

    /// Re-indent leading whitespace with tabs or spaces: every line, or the
    /// covered lines when something is selected. Also sets `use_spaces`.
    pub fn convert_indentation(&mut self, to_spaces: bool) {
        let lines: Vec<usize> =
            if self.all_empty() { (0..self.buffer.len_lines()).collect() } else { self.covered_lines() };
        let edits = lines
            .into_iter()
            .filter_map(|l| {
                let old = self.indent_of(l);
                let new = self.indent_string(self.indent_width(&old), to_spaces);
                let start = self.buffer.line_start(l);
                (new != old).then(|| (start..start + old.chars().count(), new))
            })
            .collect();
        self.use_spaces = to_spaces;
        self.apply_mapped(edits, None);
    }

    /// (indent with spaces?, width) from up to 1000 indented lines; None if
    /// nothing is indented.
    pub fn guess_indentation(&self) -> Option<(bool, usize)> {
        let (mut tabs, mut spaces) = (0usize, 0usize);
        let mut steps = [0usize; 9];
        let mut prev: Option<usize> = Some(0);
        for l in 0..self.buffer.len_lines().min(200_000) {
            if tabs + spaces >= 1000 {
                break;
            }
            let t = self.buffer.line_text(l);
            if t.trim().is_empty() {
                continue;
            }
            if t.starts_with('\t') {
                tabs += 1;
                prev = None;
                continue;
            }
            let n = t.chars().take_while(|&c| c == ' ').count();
            spaces += (n > 0) as usize;
            if let Some(p) = prev
                && matches!(n.saturating_sub(p), 2 | 3 | 4 | 8)
            {
                steps[n - p] += 1;
            }
            prev = Some(n);
        }
        if tabs + spaces == 0 {
            return None;
        }
        if tabs > spaces {
            return Some((false, self.tab_width));
        }
        // Most common step; ties go to the earlier (more usual) width.
        let mut best = None;
        for w in [4, 2, 8, 3] {
            if steps[w] > 0 && best.is_none_or(|b: usize| steps[w] > steps[b]) {
                best = Some(w);
            }
        }
        Some((true, best.unwrap_or(self.tab_width)))
    }

    /// Indent each covered line like the previous non-blank line, one level
    /// deeper after an opener (`{ ( [ :`), one shallower before a closer.
    pub fn reindent(&mut self) {
        let mut done: Vec<(usize, usize)> = Vec::new();
        let mut edits = Vec::new();
        for l in self.covered_lines() {
            let text = self.buffer.line_text(l);
            if text.trim().is_empty() {
                continue;
            }
            let mut w = match (0..l).rev().find(|&p| !self.is_blank(p)) {
                None => 0,
                Some(p) => {
                    let pt = self.buffer.line_text(p);
                    let base = done.iter().rev().find(|(dl, _)| *dl == p).map_or_else(|| self.indent_width(&pt), |d| d.1);
                    let opens = pt.trim_end().ends_with(['{', '(', '[', ':']);
                    base + if opens { self.tab_width } else { 0 }
                }
            };
            if text.trim_start().starts_with(['}', ')', ']']) {
                w = w.saturating_sub(self.tab_width);
            }
            done.push((l, w));
            let old = self.indent_of(l);
            let new = self.indent_string(w, self.use_spaces);
            if new != old {
                let start = self.buffer.line_start(l);
                edits.push((start..start + old.chars().count(), new));
            }
        }
        self.apply_mapped(edits, None);
    }

    // ---- paragraphs ----------------------------------------------------------

    fn is_blank(&self, line: usize) -> bool {
        self.buffer.line_text(line).trim().is_empty()
    }

    /// The run of non-blank lines around `lines`; None if they are all blank.
    fn paragraph_around(&self, lines: Range<usize>) -> Option<Range<usize>> {
        if lines.clone().all(|l| self.is_blank(l)) {
            return None;
        }
        let (mut a, mut b) = (lines.start, lines.end);
        while a > 0 && !self.is_blank(a - 1) {
            a -= 1;
        }
        while b < self.buffer.len_lines() && !self.is_blank(b) {
            b += 1;
        }
        Some(a..b)
    }

    /// Reflow the paragraph(s) under each selection to `width` columns,
    /// keeping the first line's indentation and a shared comment marker.
    pub fn wrap_paragraph(&mut self, width: usize) {
        let mut paras: Vec<Range<usize>> = Vec::new();
        for s in &self.sels {
            for l in self.sel_lines(s) {
                if paras.last().is_some_and(|p| p.contains(&l)) {
                    continue;
                }
                if let Some(p) = self.paragraph_around(l..l + 1) {
                    paras.push(p);
                }
            }
        }
        let ending = self.buffer.line_ending().as_str();
        let edits = paras
            .into_iter()
            .map(|p| {
                let lines: Vec<String> = p.clone().map(|l| self.buffer.line_text(l)).collect();
                let range = self.buffer.line_start(p.start)..self.buffer.line_end(p.end - 1);
                (range, self.wrap_lines(&lines, width).join(ending))
            })
            .collect();
        self.apply_mapped(edits, None);
    }

    fn wrap_lines(&self, lines: &[String], width: usize) -> Vec<String> {
        let bodies: Vec<&str> = lines.iter().map(|l| l.trim_start_matches(is_indent)).collect();
        let marker = common_comment_prefix(&bodies);
        let indent = &lines[0][..lines[0].len() - bodies[0].len()];
        // Keep the first line's spacing after the marker (usually one space).
        let gap: String = bodies[0][marker.len()..].chars().take_while(|&c| is_indent(c)).collect();
        let lead = if marker.is_empty() { indent.to_owned() } else { format!("{indent}{marker}{gap}") };
        let lead_w = self.indent_width(indent) + marker.chars().count() + gap.chars().count();
        let mut out = Vec::new();
        let mut words: Vec<&str> = Vec::new();
        let flush = |words: &mut Vec<&str>, out: &mut Vec<String>| {
            let mut cur = String::new();
            let mut cur_w = 0;
            for w in words.drain(..) {
                let n = w.chars().count();
                if !cur.is_empty() && lead_w + cur_w + 1 + n > width {
                    out.push(format!("{lead}{cur}"));
                    cur.clear();
                    cur_w = 0;
                }
                if !cur.is_empty() {
                    cur.push(' ');
                    cur_w += 1;
                }
                cur.push_str(w);
                cur_w += n;
            }
            if !cur.is_empty() {
                out.push(format!("{lead}{cur}"));
            }
        };
        for b in &bodies {
            let rest = &b[marker.len()..];
            if rest.trim().is_empty() {
                // A bare marker line (`//`) separates paragraphs in a comment.
                flush(&mut words, &mut out);
                out.push(format!("{indent}{marker}"));
            } else {
                words.extend(rest.split_whitespace());
            }
        }
        flush(&mut words, &mut out);
        out
    }

    // ---- selection commands ------------------------------------------------

    /// Select the gaps between the selections (empty gaps dropped).
    pub fn invert_selection(&mut self) {
        let len = self.buffer.len_chars();
        let mut gaps = Vec::new();
        let mut from = 0;
        let mut primary = 0;
        for (i, s) in self.sels.iter().enumerate() {
            if s.start() > from {
                gaps.push(from..s.start());
            }
            if i == self.primary {
                primary = gaps.len();
            }
            from = s.end();
        }
        if len > from {
            gaps.push(from..len);
        }
        if gaps.is_empty() {
            let h = self.primary().head;
            self.click(h, false);
        } else {
            self.select_ranges(&gaps, primary);
        }
    }

    /// Each bare cursor selects the word under it; selections stay.
    pub fn expand_to_word(&mut self) {
        self.map_selections(|e, s| {
            if s.is_empty() {
                let r = e.word_range_at(s.head);
                span(r.start, r.end, true)
            } else {
                *s
            }
        });
    }

    /// Select the non-blank lines around each selection, trailing break included.
    pub fn expand_to_paragraph(&mut self) {
        self.map_selections(|e, s| match e.paragraph_around(e.sel_lines(s)) {
            Some(p) => span(e.buffer.line_start(p.start), e.buffer.line_start(p.end), true),
            None => *s,
        });
    }

    /// Select the lines around each selection's first line indented at least
    /// as deeply (blank lines inside are kept, at the edges dropped).
    pub fn expand_to_indentation(&mut self) {
        self.map_selections(|e, s| {
            let line = e.buffer.line_of(s.start());
            if e.is_blank(line) {
                return *s;
            }
            let w = e.indent_width(&e.buffer.line_text(line));
            let fits = |l: usize| e.is_blank(l) || e.indent_width(&e.buffer.line_text(l)) >= w;
            let (mut a, mut b) = (line, line);
            while a > 0 && fits(a - 1) {
                a -= 1;
            }
            while b + 1 < e.buffer.len_lines() && fits(b + 1) {
                b += 1;
            }
            while e.is_blank(a) {
                a += 1;
            }
            while e.is_blank(b) {
                b -= 1;
            }
            span(e.buffer.line_start(a), e.buffer.line_end(b), true)
        });
    }

    /// Replace every selection with `f(selection)`, no text change.
    fn map_selections(&mut self, f: impl Fn(&Self, &Selection) -> Selection) {
        let sels: Vec<Selection> = self.sels.iter().map(|s| f(self, s)).collect();
        let head = sels[self.primary].head;
        self.set_selections(sels, head);
        self.can_coalesce = false;
    }

    // ---- bookmarks ---------------------------------------------------------

    pub fn bookmarks(&self) -> &[usize] {
        &self.bookmarks
    }

    /// Per cursor line: drop its bookmarks, or bookmark the cursor if it has none.
    pub fn toggle_bookmark(&mut self) {
        let mut last_line = None;
        let heads: Vec<usize> = self.sels.iter().map(|s| s.head).collect();
        for h in heads {
            let line = self.buffer.line_of(h);
            if last_line == Some(line) {
                continue;
            }
            last_line = Some(line);
            let before = self.bookmarks.len();
            let buffer = &self.buffer;
            self.bookmarks.retain(|&b| buffer.line_of(b) != line);
            if self.bookmarks.len() == before {
                self.bookmarks.push(h);
            }
        }
        self.bookmarks.sort_unstable();
        self.bookmarks.dedup();
    }

    /// Jump to the next bookmark after the primary cursor, wrapping.
    pub fn next_bookmark(&mut self) {
        let h = self.primary().head;
        if let Some(&b) = self.bookmarks.iter().find(|&&b| b > h).or(self.bookmarks.first()) {
            self.click(b, false);
        }
    }

    pub fn prev_bookmark(&mut self) {
        let h = self.primary().head;
        if let Some(&b) = self.bookmarks.iter().rev().find(|&&b| b < h).or(self.bookmarks.last()) {
            self.click(b, false);
        }
    }

    pub fn clear_bookmarks(&mut self) {
        self.bookmarks.clear();
    }

    /// A cursor at every bookmark.
    pub fn select_all_bookmarks(&mut self) {
        let Some(&first) = self.bookmarks.first() else { return };
        let sels = self.bookmarks.iter().map(|&b| Selection::cursor(b)).collect();
        self.set_selections(sels, first);
        self.can_coalesce = false;
    }

    // ---- mark (Edit ▸ Mark) ------------------------------------------------

    pub fn has_mark(&self) -> bool {
        !self.marks.is_empty()
    }

    /// Set a mark at each cursor, replacing the old marks.
    pub fn set_mark(&mut self) {
        self.marks = self.sels.iter().map(|s| s.head).collect();
        self.mark_primary = self.primary;
    }

    pub fn clear_mark(&mut self) {
        self.marks.clear();
        self.mark_primary = 0;
    }

    /// The mark paired with selection `i`: the one at the same index when
    /// there are as many marks as selections, else the primary mark.
    fn mark_for(&self, i: usize) -> usize {
        let k = if self.marks.len() == self.sels.len() { i } else { self.mark_primary };
        let m = self.marks.get(k).or(self.marks.last());
        m.map_or(0, |&m| m.min(self.buffer.len_chars()))
    }

    /// Each selection runs from its mark to its head.
    pub fn select_to_mark(&mut self) {
        if self.marks.is_empty() {
            return;
        }
        let sels: Vec<Selection> = (0..self.sels.len())
            .map(|i| Selection { anchor: self.mark_for(i), head: self.sels[i].head, goal: None })
            .collect();
        let head = sels[self.primary].head;
        self.set_selections(sels, head);
        self.can_coalesce = false;
    }

    /// Delete from each cursor to its mark as one step; returns the deleted
    /// text (pieces joined by line breaks) for the yank buffer.
    pub fn delete_to_mark(&mut self) -> String {
        if self.marks.is_empty() {
            return String::new();
        }
        let spans: Vec<Range<usize>> = (0..self.sels.len())
            .map(|i| {
                let (h, m) = (self.sels[i].head, self.mark_for(i));
                h.min(m)..h.max(m)
            })
            .collect();
        let mut sorted = spans.clone();
        sorted.sort_by_key(|r| r.start);
        let mut merged: Vec<Range<usize>> = Vec::new();
        for r in sorted {
            match merged.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
        }
        merged.retain(|r| !r.is_empty());
        if merged.is_empty() {
            return String::new();
        }
        let ending = self.buffer.line_ending().as_str();
        let text = merged.iter().map(|r| self.buffer.slice(r.clone())).collect::<Vec<_>>().join(ending);
        let edits: Vec<Edit> = merged.into_iter().map(|r| (r, String::new())).collect();
        let sels: Vec<Selection> = spans.iter().map(|r| Selection::cursor(map_pos(&edits, r.start, false))).collect();
        let head = sels[self.primary].head;
        self.apply_then(edits, None, sels, head);
        text
    }

    /// Cursors jump to their marks; the marks move to where the cursors were.
    pub fn swap_with_mark(&mut self) {
        if self.marks.is_empty() {
            return;
        }
        let sels: Vec<Selection> = (0..self.sels.len()).map(|i| Selection::cursor(self.mark_for(i))).collect();
        let head = sels[self.primary].head;
        self.marks = self.sels.iter().map(|s| s.head).collect();
        self.mark_primary = self.primary;
        self.set_selections(sels, head);
        self.can_coalesce = false;
    }

    // ---- soft undo -----------------------------------------------------------

    fn restore_selections(&mut self, (sels, primary): (Vec<Selection>, usize)) {
        self.next_whole_word = false;
        self.primary = primary.min(sels.len().saturating_sub(1));
        self.sels = sels;
        self.can_coalesce = false;
    }

    /// Step back one selection change if that came last, else undo an edit.
    pub fn soft_undo(&mut self) {
        let state = self.text_state();
        if self.sel_past.last().is_some_and(|s| s.state == state)
            && let Some(step) = self.sel_past.pop()
        {
            self.restore_selections(step.before.clone());
            self.sel_future.push(step);
            return;
        }
        self.undo();
        // Steps recorded on text that no longer exists can't be restored.
        let state = self.text_state();
        while self.sel_past.last().is_some_and(|s| s.state != state) {
            self.sel_past.pop();
        }
    }

    /// Redo what `soft_undo` stepped back over, selection changes and edits in order.
    pub fn soft_redo(&mut self) {
        let state = self.text_state();
        if self.sel_future.last().is_some_and(|s| s.state == state)
            && let Some(step) = self.sel_future.pop()
        {
            self.restore_selections(step.after.clone());
            self.sel_past.push(step);
            return;
        }
        self.redo();
        let state = self.text_state();
        while self.sel_future.last().is_some_and(|s| s.state != state) {
            self.sel_future.pop();
        }
    }

    // ---- case conversion ------------------------------------------------------

    pub fn lower_camel_case(&mut self) {
        self.convert_case(|w| {
            w.iter().enumerate().map(|(i, w)| if i == 0 { w.to_lowercase() } else { capitalize(w) }).collect()
        });
    }

    pub fn upper_camel_case(&mut self) {
        self.convert_case(|w| w.iter().map(|w| capitalize(w)).collect());
    }

    pub fn snake_case(&mut self) {
        self.convert_case(|w| w.iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join("_"));
    }

    pub fn kebab_case(&mut self) {
        self.convert_case(|w| w.iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join("-"));
    }

    /// Rejoin the words of each selection (a cursor's identifier when empty).
    fn convert_case(&mut self, join: impl Fn(&[String]) -> String) {
        self.transform_selected_with(Self::ident_range_at, |s| convert_phrases(s, &join));
    }

    /// The identifier at `pos`, `-` included (kebab-case) except at its edges.
    fn ident_range_at(&self, pos: usize) -> Range<usize> {
        let line = self.buffer.line_of(pos);
        let (start, end) = (self.buffer.line_start(line), self.buffer.line_end(line));
        let part = |c: char| is_ident(c) || c == '-';
        let (mut a, mut b) = (pos, pos);
        while a > start && part(self.buffer.char(a - 1)) {
            a -= 1;
        }
        while b < end && part(self.buffer.char(b)) {
            b += 1;
        }
        while a < b && self.buffer.char(a) == '-' {
            a += 1;
        }
        while b > a && self.buffer.char(b - 1) == '-' {
            b -= 1;
        }
        if a == b { self.word_range_at(pos) } else { a..b }
    }

    // ---- expand selection -----------------------------------------------------

    /// The smallest candidate strictly containing `s`, as a forward selection; else `s`.
    fn smallest_around(s: &Selection, candidates: Vec<Range<usize>>) -> Selection {
        let r = s.range();
        candidates
            .into_iter()
            .filter(|c| c.start <= r.start && r.end <= c.end && c.len() > r.len())
            .min_by_key(|c| c.len())
            .map_or(*s, |c| span(c.start, c.end, true))
    }

    /// Bracket pairs (only `{}` with `curly`) enclosing all of `r`, innermost first.
    fn enclosing_pairs(&self, r: &Range<usize>, curly: bool) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut from = r.start;
        for _ in 0..64 {
            let Some((o, c)) = self.enclosing_brackets(from) else { break };
            if c >= r.end && (!curly || self.buffer.char(o) == '{') {
                out.push((o, c));
            }
            from = o;
        }
        out
    }

    /// Expansion targets for the pair `o`..=`c` around `r`: its contents, then
    /// the pair itself. When the brackets sit alone at the ends of their lines,
    /// the contents are the lines between (`full_lines`: with the last break),
    /// matching the indentation block; the raw contents stay a target only
    /// when `r` reaches outside those lines.
    fn pair_candidates(&self, o: usize, c: usize, r: &Range<usize>, full_lines: bool, out: &mut Vec<Range<usize>>) {
        let (lo, lc) = (self.buffer.line_of(o), self.buffer.line_of(c));
        let own_lines = lc > lo + 1
            && self.buffer.slice(o + 1..self.buffer.line_end(lo)).trim().is_empty()
            && self.buffer.slice(self.buffer.line_start(lc)..c).trim().is_empty();
        let inner = if !own_lines {
            o + 1..c
        } else if full_lines {
            self.buffer.line_start(lo + 1)..self.buffer.line_start(lc)
        } else {
            self.buffer.line_start(lo + 1)..self.buffer.line_end(lc - 1)
        };
        if !(inner.start <= r.start && r.end <= inner.end) {
            out.push(o + 1..c);
        }
        out.extend([inner, o..c + 1]);
    }

    /// The quote pair on `r`'s line around it (open before, close at or after).
    fn enclosing_quotes(&self, r: &Range<usize>) -> Option<(usize, usize)> {
        let line = self.buffer.line_of(r.start);
        if self.buffer.line_of(r.end) != line {
            return None;
        }
        let ls = self.buffer.line_start(line);
        let mut open: Option<(usize, char)> = None;
        let mut escaped = false;
        for (i, c) in self.buffer.line_chars(line).enumerate() {
            let p = ls + i;
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = open.is_some();
            } else if let Some((o, q)) = open {
                if c == q {
                    if o < r.start && p >= r.end {
                        return Some((o, p));
                    }
                    open = None;
                }
            } else if matches!(c, '"' | '\'' | '`') {
                open = Some((p, c));
            }
        }
        None
    }

    /// Indentation blocks around `lines`, innermost first, as (first line,
    /// last line, header line above with less indent). Blank lines inside
    /// count, at the edges they don't.
    fn indent_blocks(&self, lines: Range<usize>) -> Vec<(usize, usize, Option<usize>)> {
        let width = |l: usize| self.indent_width(&self.buffer.line_text(l));
        let Some(mut w) = lines.clone().filter(|&l| !self.is_blank(l)).map(width).min() else {
            return Vec::new();
        };
        let last = self.buffer.len_lines() - 1;
        let (mut a, mut b) = (lines.start, lines.end - 1);
        let mut out = Vec::new();
        for _ in 0..64 {
            let fits = |l: usize| self.is_blank(l) || width(l) >= w;
            while a > 0 && fits(a - 1) {
                a -= 1;
            }
            while b < last && fits(b + 1) {
                b += 1;
            }
            while a < b && self.is_blank(a) {
                a += 1;
            }
            while b > a && self.is_blank(b) {
                b -= 1;
            }
            let header = (0..a).rev().find(|&l| !self.is_blank(l));
            out.push((a, b, header));
            let Some(h) = header else { break };
            w = width(h);
            a = h;
        }
        out
    }

    /// Grow each selection to the next larger unit: word, inside the nearest
    /// quotes/brackets, including them, whole lines, the indentation block
    /// (then with its header line), the whole buffer.
    pub fn expand_selection(&mut self) {
        self.map_selections(|e, s| {
            let r = s.range();
            let mut cands = vec![e.word_range_at(r.start)];
            if let Some((o, c)) = e.enclosing_quotes(&r) {
                cands.extend([o + 1..c, o..c + 1]);
            }
            for (o, c) in e.enclosing_pairs(&r, false) {
                e.pair_candidates(o, c, &r, true, &mut cands);
            }
            let lines = e.sel_lines(s);
            cands.push(e.buffer.line_start(lines.start)..e.buffer.line_start(lines.end));
            for (a, b, header) in e.indent_blocks(lines) {
                let end = e.buffer.line_start(b + 1);
                cands.push(e.buffer.line_start(a)..end);
                if let Some(h) = header {
                    cands.push(e.buffer.line_start(h)..end);
                }
            }
            cands.push(0..e.buffer.len_chars());
            Self::smallest_around(s, cands)
        });
    }

    /// Like `expand_to_line`, but repeating adds the line above.
    pub fn expand_to_line_upward(&mut self) {
        self.map_selections(|e, s| {
            let lines = e.sel_lines(s);
            let mut start = e.buffer.line_start(lines.start);
            let end = e.buffer.line_start(lines.end);
            if !s.is_empty() && s.start() == start && s.end() == end && lines.start > 0 {
                start = e.buffer.line_start(lines.start - 1);
            }
            span(start, end, false)
        });
    }

    /// Select the contents of the innermost enclosing `{}` or indentation
    /// block; again to include the braces (or the block's header line).
    pub fn expand_to_block(&mut self) {
        self.map_selections(|e, s| {
            let r = s.range();
            let mut cands = Vec::new();
            for (o, c) in e.enclosing_pairs(&r, true) {
                e.pair_candidates(o, c, &r, false, &mut cands);
            }
            for (a, b, header) in e.indent_blocks(e.sel_lines(s)) {
                let end = e.buffer.line_end(b);
                cands.push(e.buffer.line_start(a)..end);
                if let Some(h) = header {
                    cands.push(e.buffer.line_start(h)..end);
                }
            }
            Self::smallest_around(s, cands)
        });
    }

    // ---- word completion ---------------------------------------------------

    /// Start and text of the identifier chars just left of the primary cursor.
    pub fn word_prefix_at_cursor(&self) -> Option<(usize, String)> {
        let h = self.primary().head;
        let ls = self.buffer.line_start(self.buffer.line_of(h));
        let mut a = h;
        while a > ls && is_ident(self.buffer.char(a - 1)) {
            a -= 1;
        }
        (a < h).then(|| (a, self.buffer.slice(a..h)))
    }

    /// Distinct identifiers near the cursor starting with its prefix (any
    /// case): same-case matches first, then nearest first.
    pub fn completions(&self, limit: usize) -> Vec<String> {
        let Some((_, prefix)) = self.word_prefix_at_cursor() else { return Vec::new() };
        let lower = prefix.to_lowercase();
        let h = self.primary().head;
        let lo = h.saturating_sub(COMPLETION_SCAN);
        let hi = (h + COMPLETION_SCAN).min(self.buffer.len_chars());
        let mut best: std::collections::HashMap<String, (bool, usize)> = std::collections::HashMap::new();
        let mut word = String::new();
        let mut start = 0;
        for (i, c) in self.buffer.slice(lo..hi).chars().chain(std::iter::once(' ')).enumerate() {
            if is_ident(c) {
                if word.is_empty() {
                    start = lo + i;
                }
                word.push(c);
                continue;
            }
            if word.is_empty() {
                continue;
            }
            let at_cursor = start <= h && h <= lo + i;
            if !at_cursor
                && word != prefix
                && !word.starts_with(|c: char| c.is_ascii_digit())
                && word.to_lowercase().starts_with(&lower)
            {
                let key = (!word.starts_with(&prefix), start.abs_diff(h));
                match best.get_mut(&word) {
                    Some(k) => *k = (*k).min(key),
                    None => {
                        best.insert(word.clone(), key);
                    }
                }
            }
            word.clear();
        }
        let mut found: Vec<(String, (bool, usize))> = best.into_iter().collect();
        found.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        found.into_iter().take(limit).map(|(w, _)| w).collect()
    }

    /// Insert `word` at every cursor, replacing the cursor's prefix when it
    /// matches the primary's (any case); selections are replaced.
    pub fn complete(&mut self, word: &str) {
        let prefix = self.word_prefix_at_cursor().map(|(_, p)| p.to_lowercase()).unwrap_or_default();
        let n = prefix.chars().count();
        let edits = self
            .sels
            .iter()
            .map(|s| {
                if !s.is_empty() {
                    return (s.range(), word.to_owned());
                }
                let h = s.head;
                let ls = self.buffer.line_start(self.buffer.line_of(h));
                let own = n > 0 && h >= ls + n && self.buffer.slice(h - n..h).to_lowercase() == prefix;
                (if own { h - n..h } else { h..h }, word.to_owned())
            })
            .collect();
        self.apply(edits, None);
    }

    // ---- brackets --------------------------------------------------------

    /// Position of the bracket matching the one at `at`, same kind, nesting-aware.
    fn find_match(&self, at: usize) -> Option<usize> {
        let (k, open) = bracket_kind(self.buffer.char(at))?;
        let (o, c) = BRACKETS[k];
        let mut depth = 0usize;
        let mut step = |ch: char, inner: char, outer: char| {
            if ch == inner {
                depth += 1;
            } else if ch == outer {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            false
        };
        if open {
            let end = (at + 1 + BRACKET_SCAN).min(self.buffer.len_chars());
            let text = self.buffer.slice(at + 1..end);
            text.chars().position(|ch| step(ch, o, c)).map(|i| at + 1 + i)
        } else {
            let text = self.buffer.slice(at.saturating_sub(BRACKET_SCAN)..at);
            text.chars().rev().position(|ch| step(ch, c, o)).map(|i| at - 1 - i)
        }
    }

    /// A bracket right after (else right before) `pos`, and its match.
    pub fn matching_bracket(&self, pos: usize) -> Option<(usize, usize)> {
        let after = (pos < self.buffer.len_chars()).then_some(pos);
        [after, pos.checked_sub(1)]
            .into_iter()
            .flatten()
            .find_map(|b| self.find_match(b).map(|m| (b, m)))
    }

    /// Innermost bracket pair with its opener before `pos` and closer at or after it.
    fn enclosing_brackets(&self, pos: usize) -> Option<(usize, usize)> {
        let mut depth = [0usize; 3];
        let text = self.buffer.slice(pos.saturating_sub(BRACKET_SCAN)..pos);
        for (i, ch) in text.chars().rev().enumerate() {
            match bracket_kind(ch) {
                Some((k, false)) => depth[k] += 1,
                Some((k, true)) if depth[k] > 0 => depth[k] -= 1,
                Some((_, true)) => {
                    let o = pos - 1 - i;
                    return self.find_match(o).map(|c| (o, c));
                }
                None => {}
            }
        }
        None
    }

    /// Ctrl+M: move to the matching bracket, keeping the caret on the same
    /// side (inside/outside) of the pair; else to the enclosing closer.
    pub fn jump_to_bracket(&mut self) {
        self.move_heads(false, |e, s| {
            if let Some((b, m)) = e.matching_bracket(s.head) {
                let opener = m > b;
                let outside = (b == s.head) == opener;
                // The match is a closer when the bracket is an opener.
                let land = if outside == opener { m + 1 } else { m };
                return (land, None);
            }
            (e.enclosing_brackets(s.head).map_or(s.head, |(_, c)| c), None)
        });
    }

    /// Select the enclosing brackets' contents; again to include the brackets.
    pub fn expand_to_brackets(&mut self) {
        let sels: Vec<Selection> = self
            .sels
            .iter()
            .map(|s| {
                let mut from = s.start();
                for _ in 0..64 {
                    let Some((o, c)) = self.enclosing_brackets(from) else { break };
                    if o + 1 == s.start() && c == s.end() {
                        return Selection { anchor: o, head: c + 1, goal: None };
                    }
                    if o < s.start() && c >= s.end() {
                        return Selection { anchor: o + 1, head: c, goal: None };
                    }
                    from = o;
                }
                *s
            })
            .collect();
        let head = sels[self.primary].head;
        self.set_selections(sels, head);
        self.can_coalesce = false;
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

    fn sel(e: &mut Editor, a: usize, b: usize) {
        let r = a..b;
        e.select_ranges(&[r], 0);
    }

    fn ranges(e: &Editor) -> Vec<Range<usize>> {
        e.selections().iter().map(|s| s.range()).collect()
    }

    /// One undo restores `before` exactly; redo reapplies; dirty tracks it.
    fn assert_one_step(e: &mut Editor, before: &str) {
        let after = text(e);
        e.undo();
        assert_eq!(text(e), before);
        assert!(!e.is_dirty());
        e.redo();
        assert_eq!(text(e), after);
        assert!(e.is_dirty());
    }

    #[test]
    fn auto_pairs_brackets_and_quotes() {
        let mut e = ed("x\n");
        e.type_char("(");
        assert_eq!(text(&e), "(x\n", "no pair before a word char");
        let mut e = ed("\n");
        e.type_char("(");
        e.type_char("a");
        assert_eq!(text(&e), "(a)\n");
        assert_eq!(e.primary().head, 2);
        e.type_char(")");
        assert_eq!(text(&e), "(a)\n", "typed over the closer");
        assert_eq!(e.primary().head, 3);
        e.undo();
        assert_eq!(text(&e), "\n", "pair and typing are one step");

        let mut e = ed("it\n");
        e.click(2, false);
        e.type_char("'");
        assert_eq!(text(&e), "it'\n", "no quote pair after a word char");
        let mut e = ed("a = ;\n");
        e.click(4, false);
        e.type_char("\"");
        assert_eq!(text(&e), "a = \"\";\n");
    }

    #[test]
    fn auto_pair_with_multiple_cursors() {
        let mut e = ed("a\nb\n");
        e.click(1, false);
        e.add_cursor(3);
        e.type_char("[");
        assert_eq!(text(&e), "a[]\nb[]\n");
        assert_eq!(ranges(&e), vec![2..2, 6..6]);
        e.backspace(false);
        assert_eq!(text(&e), "a\nb\n", "backspace removes both halves");
    }

    #[test]
    fn opener_wraps_selection() {
        let mut e = ed("foo bar\n");
        sel(&mut e, 4, 7);
        e.type_char("(");
        assert_eq!(text(&e), "foo (bar)\n");
        assert_eq!(e.primary().range(), 5..8);
        e.type_char("\"");
        assert_eq!(text(&e), "foo (\"bar\")\n");
        e.undo();
        assert_eq!(text(&e), "foo (bar)\n", "each wrap is its own step");
    }

    #[test]
    fn indent_and_outdent_keep_selection() {
        let mut e = ed("a\n\n  b\nc\n");
        sel(&mut e, 0, 5);
        e.indent();
        assert_eq!(text(&e), "    a\n\n      b\nc\n", "empty line left alone");
        assert_eq!(e.primary().range(), 0..13);
        assert_one_step(&mut e, "a\n\n  b\nc\n");
        e.outdent();
        e.outdent();
        assert_eq!(text(&e), "a\n\nb\nc\n");
        let mut e = ed("\tx\n");
        e.use_spaces = false;
        e.click(2, false);
        e.indent();
        assert_eq!(text(&e), "\t\tx\n");
        assert_eq!(e.primary().head, 3);
        e.outdent();
        assert_eq!(text(&e), "\tx\n");
    }

    #[test]
    fn indent_dedupes_cursors_on_one_line() {
        let mut e = ed("ab\n");
        e.click(0, false);
        e.add_cursor(2);
        e.indent();
        assert_eq!(text(&e), "    ab\n");
        assert_eq!(ranges(&e), vec![4..4, 6..6]);
    }

    #[test]
    fn toggle_line_comment() {
        let src = "fn f() {\n    a();\n\n  b();\n}\n";
        let mut e = ed(src);
        sel(&mut e, 9, 24);
        e.toggle_comment("//");
        assert_eq!(text(&e), "fn f() {\n  //   a();\n\n  // b();\n}\n", "at the minimum indent");
        assert_one_step(&mut e, src);
        e.toggle_comment("//");
        assert_eq!(text(&e), src);
        // Mixed: one commented, one not → comment both.
        let mut e = ed("# a\nb\n");
        sel(&mut e, 0, 5);
        e.toggle_comment("#");
        assert_eq!(text(&e), "# # a\n# b\n");
    }

    #[test]
    fn toggle_comment_multi_cursor_same_line_once() {
        let mut e = ed("let x;\n");
        e.click(1, false);
        e.add_cursor(4);
        e.toggle_comment("//");
        assert_eq!(text(&e), "// let x;\n");
        assert_eq!(ranges(&e), vec![4..4, 7..7]);
        e.toggle_comment("//");
        assert_eq!(text(&e), "let x;\n");
    }

    #[test]
    fn toggle_block_comment_wraps_and_unwraps() {
        let mut e = ed("  a { b }\n");
        e.click(4, false);
        e.toggle_block_comment("/*", "*/");
        assert_eq!(text(&e), "  /* a { b } */\n");
        e.toggle_block_comment("/*", "*/");
        assert_eq!(text(&e), "  a { b }\n");
        sel(&mut e, 6, 7);
        e.toggle_block_comment("<!--", "-->");
        assert_eq!(text(&e), "  a { <!--b--> }\n");
        assert_eq!(e.primary().range(), 6..14);
        e.toggle_block_comment("<!--", "-->");
        assert_eq!(text(&e), "  a { b }\n");
    }

    #[test]
    fn duplicate_line_and_selection() {
        let mut e = ed("ab\r\ncd");
        e.click(1, false);
        e.duplicate_lines();
        assert_eq!(text(&e), "ab\r\nab\r\ncd");
        assert_eq!(e.primary().head, 5);
        assert_one_step(&mut e, "ab\r\ncd");
        sel(&mut e, 8, 10);
        e.duplicate_lines();
        assert_eq!(text(&e), "ab\r\nab\r\ncdcd");
        assert_eq!(e.primary().range(), 10..12);
    }

    #[test]
    fn duplicate_two_cursors_on_one_line_once() {
        let mut e = ed("ab\nz");
        e.click(0, false);
        e.add_cursor(2);
        e.duplicate_lines();
        assert_eq!(text(&e), "ab\nab\nz");
        assert_eq!(ranges(&e), vec![3..3, 5..5]);
    }

    #[test]
    fn delete_lines_under_selections() {
        let mut e = ed("a\nb\nc\nd");
        e.click(0, false);
        e.add_cursor(1);
        e.add_cursor(6);
        e.delete_lines();
        assert_eq!(text(&e), "b\nc");
        assert_eq!(e.selections().len(), 2);
        assert_one_step(&mut e, "a\nb\nc\nd");
        e.select_all();
        e.delete_lines();
        assert_eq!(text(&e), "");
    }

    #[test]
    fn move_lines_up_and_down() {
        let mut e = ed("a\r\nbb\r\nc");
        e.click(4, false); // "bb", column 1
        e.move_lines(true);
        assert_eq!(text(&e), "bb\r\na\r\nc");
        assert_eq!(e.primary().head, 1);
        e.move_lines(true);
        assert_eq!(text(&e), "bb\r\na\r\nc", "no-op at the top");
        e.move_lines(false);
        e.move_lines(false);
        assert_eq!(text(&e), "a\r\nc\r\nbb");
        assert_eq!(e.primary().head, 7);
        e.undo();
        assert_eq!(text(&e), "a\r\nbb\r\nc", "each move is one step");
    }

    #[test]
    fn move_selected_block_keeps_selection() {
        let mut e = ed("1\n2\n3\n4\n");
        sel(&mut e, 2, 6); // lines "2" and "3" incl. break
        e.move_lines(false);
        assert_eq!(text(&e), "1\n4\n2\n3\n");
        assert_eq!(e.buffer.slice(e.primary().range()), "2\n3\n");
        e.move_lines(true);
        assert_eq!(text(&e), "1\n2\n3\n4\n");
    }

    #[test]
    fn join_lines_trims_indent() {
        let mut e = ed("fn f() {\r\n    x\r\n}");
        e.click(0, false);
        e.join_lines();
        assert_eq!(text(&e), "fn f() { x\r\n}");
        assert_eq!(e.primary().head, 8);
        assert_one_step(&mut e, "fn f() {\r\n    x\r\n}");
        e.select_all();
        e.join_lines();
        assert_eq!(text(&e), "fn f() { x}", "no space before a closer");
    }

    #[test]
    fn join_with_cursors_on_adjacent_lines() {
        let mut e = ed("a\nb\nc\n\nd");
        e.click(0, false);
        e.add_cursor(2);
        e.add_cursor(6);
        e.join_lines();
        assert_eq!(text(&e), "a b c\nd");
    }

    #[test]
    fn expand_to_line_repeats() {
        let mut e = ed("ab\ncd\nef");
        e.click(1, false);
        e.expand_to_line();
        assert_eq!(e.primary().range(), 0..3);
        e.expand_to_line();
        assert_eq!(e.primary().range(), 0..6);
        e.expand_to_line();
        assert_eq!(e.primary().range(), 0..8);
    }

    #[test]
    fn split_selection_into_lines() {
        let mut e = ed("ab\ncd\nef\n");
        sel(&mut e, 1, 9);
        e.split_into_lines();
        assert_eq!(ranges(&e), vec![1..2, 3..5, 6..8]);
    }

    #[test]
    fn insert_line_after_and_before() {
        let mut e = ed("  ab\r\n");
        e.click(1, false);
        e.insert_line_after();
        assert_eq!(text(&e), "  ab\r\n  \r\n");
        assert_eq!(e.primary().head, 8);
        e.undo();
        e.click(3, false);
        e.insert_line_before();
        assert_eq!(text(&e), "  \r\n  ab\r\n");
        assert_eq!(e.primary().head, 2);
    }

    #[test]
    fn insert_line_after_with_two_cursors() {
        let mut e = ed("a\nb");
        e.click(0, false);
        e.add_cursor(2);
        e.insert_line_after();
        assert_eq!(text(&e), "a\n\nb\n");
        assert_eq!(ranges(&e), vec![2..2, 5..5]);
        assert_one_step(&mut e, "a\nb");
    }

    #[test]
    fn add_cursor_above_and_below_at_goal_column() {
        let mut e = ed("abcd\nx\nabcd\nabcd");
        e.click(8, false); // line 2, col 1
        e.add_cursor_vertical(-1);
        e.add_cursor_vertical(-1);
        assert_eq!(ranges(&e), vec![1..1, 6..6, 8..8]);
        e.add_cursor_vertical(1);
        assert_eq!(e.selections().len(), 4);
        assert_eq!(e.primary().head, 13);
        e.add_cursor_vertical(1);
        assert_eq!(e.selections().len(), 4, "no line below");
    }

    #[test]
    fn bracket_matching_and_jump() {
        let mut e = ed("f(a[1], {b})");
        assert_eq!(e.matching_bracket(1), Some((1, 11)));
        assert_eq!(e.matching_bracket(12), Some((11, 1)));
        assert_eq!(e.matching_bracket(4), Some((3, 5)));
        assert_eq!(e.matching_bracket(0), None);
        e.click(1, false);
        e.jump_to_bracket();
        assert_eq!(e.primary().head, 12);
        e.jump_to_bracket();
        assert_eq!(e.primary().head, 1);
        e.click(9, false); // just inside '{' → just inside '}'
        e.jump_to_bracket();
        assert_eq!(e.primary().head, 10);
        e.jump_to_bracket();
        assert_eq!(e.primary().head, 9);
        e.click(2, false); // just inside '(' → before ')'
        e.jump_to_bracket();
        assert_eq!(e.primary().head, 11);
    }

    #[test]
    fn expand_to_brackets_grows_outward() {
        let mut e = ed("f(a, [b, c])");
        e.click(7, false);
        e.expand_to_brackets();
        assert_eq!(e.primary().range(), 6..10);
        e.expand_to_brackets();
        assert_eq!(e.primary().range(), 5..11);
        e.expand_to_brackets();
        assert_eq!(e.primary().range(), 2..11);
        e.expand_to_brackets();
        assert_eq!(e.primary().range(), 1..12);
        e.expand_to_brackets();
        assert_eq!(e.primary().range(), 1..12, "nothing further out");
    }

    #[test]
    fn case_transforms() {
        let mut e = ed("foo bar");
        e.click(1, false);
        e.add_cursor(5);
        e.swap_case_upper();
        assert_eq!(text(&e), "FOO BAR");
        assert_eq!(ranges(&e), vec![1..1, 5..5]);
        assert_one_step(&mut e, "foo bar");
        sel(&mut e, 0, 3);
        e.swap_case_lower();
        assert_eq!(text(&e), "foo BAR");
        assert_eq!(e.primary().range(), 0..3);
    }

    #[test]
    fn sort_reverse_unique_whole_buffer() {
        let mut e = ed("b\nA\nc\nb\n");
        e.sort_lines(false);
        assert_eq!(text(&e), "A\nb\nb\nc\n");
        assert_one_step(&mut e, "b\nA\nc\nb\n");
        e.sort_lines(true);
        assert_eq!(text(&e), "A\nb\nb\nc\n");
        e.unique_lines();
        assert_eq!(text(&e), "A\nb\nc\n");
        e.reverse_lines();
        assert_eq!(text(&e), "c\nb\nA\n");
    }

    #[test]
    fn sort_only_selected_lines() {
        let mut e = ed("z\nc\nb\na\n");
        sel(&mut e, 2, 7); // "c", "b", and into "a"
        e.sort_lines(true);
        assert_eq!(text(&e), "z\na\nb\nc\n");
        assert_eq!(e.primary().range(), 2..7);
        let mut e = ed("b\na\n");
        e.sort_lines(true);
        e.sort_lines(true);
        e.undo();
        assert_eq!(text(&e), "b\na\n", "an already-sorted sort is no step");
    }

    #[test]
    fn trim_trailing_whitespace_lines() {
        let mut e = ed("a  \r\nb\t\r\n  \r\n");
        e.click(0, false);
        e.trim_trailing_whitespace();
        assert_eq!(text(&e), "a\r\nb\r\n\r\n");
        assert_one_step(&mut e, "a  \r\nb\t\r\n  \r\n");
    }

    #[test]
    fn box_selection_skips_short_lines() {
        let mut e = ed("abcdef\na\nabcdef\n\tx");
        e.select_box(0, 2, 2, 4);
        assert_eq!(ranges(&e), vec![2..4, 11..13]);
        assert_eq!(e.primary().range(), 11..13);
        e.select_box(1, 5, 1, 6);
        assert_eq!(ranges(&e), vec![8..8], "keeps one cursor");
        e.select_box(2, 4, 3, 4);
        assert_eq!(ranges(&e), vec![13..13, 17..17], "tab expands to column 4");
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

    #[test]
    fn paste_and_indent_shifts_to_target() {
        let src = "fn f() {\n    x();\n}\n";
        let mut e = ed(src);
        e.click(13, false); // after "    " on the x() line
        e.paste_and_indent("if a {\n  b();\n}");
        assert_eq!(text(&e), "fn f() {\n    if a {\n      b();\n    }x();\n}\n");
        assert_one_step(&mut e, src);
        // Whole lines go above the cursor line, at its indentation.
        let mut e = ed("\t\tz\r\n");
        e.use_spaces = false;
        e.click(2, false);
        e.paste_and_indent("a\n    b\n");
        assert_eq!(text(&e), "\t\ta\r\n\t\t\tb\r\n\t\tz\r\n");
    }

    #[test]
    fn paste_and_indent_multi_cursor_inline() {
        let mut e = ed("  a = \n    b = \n");
        e.click(6, false);
        e.add_cursor(15);
        e.paste_and_indent("[\n    1,\n]");
        assert_eq!(text(&e), "  a = [\n      1,\n  ]\n    b = [\n        1,\n    ]\n");
        assert_one_step(&mut e, "  a = \n    b = \n");
    }

    #[test]
    fn delete_to_line_end_then_break() {
        let mut e = ed("abc\r\ndef\r\nxy");
        e.click(1, false);
        e.add_cursor(6);
        e.delete_to_line_end();
        assert_eq!(text(&e), "a\r\nd\r\nxy");
        assert_one_step(&mut e, "abc\r\ndef\r\nxy");
        e.click(1, false);
        e.delete_to_line_end();
        assert_eq!(text(&e), "ad\r\nxy", "at the end it joins the next line");
    }

    #[test]
    fn transpose_chars_and_selections() {
        let mut e = ed("abc\nxy\n");
        e.click(1, false);
        e.add_cursor(6); // end of "xy"
        e.transpose();
        assert_eq!(text(&e), "bac\nyx\n");
        assert_eq!(ranges(&e), vec![2..2, 6..6]);
        assert_one_step(&mut e, "abc\nxy\n");
        let mut e = ed("one two three");
        e.select_ranges(&[0..3, 4..7, 8..13], 0);
        e.transpose();
        assert_eq!(text(&e), "two three one");
        assert_eq!(ranges(&e), vec![0..3, 4..9, 10..13]);
        assert_one_step(&mut e, "one two three");
    }

    #[test]
    fn title_and_swap_case() {
        let mut e = ed("hello wORLD don't");
        e.select_all();
        e.title_case();
        assert_eq!(text(&e), "Hello World Don't");
        assert_one_step(&mut e, "hello wORLD don't");
        let mut e = ed("aB cD");
        e.click(0, false);
        e.add_cursor(4);
        e.swap_case();
        assert_eq!(text(&e), "Ab Cd");
        assert_one_step(&mut e, "aB cD");
    }

    #[test]
    fn shuffle_lines_is_seeded_permutation() {
        let src = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let run = |seed| {
            let mut e = ed(src);
            e.rng = Rng(seed);
            e.shuffle_lines();
            text(&e)
        };
        let a = run(42);
        assert_eq!(a, run(42), "same seed, same order");
        let mut lines: Vec<&str> = a.lines().collect();
        lines.sort();
        assert_eq!(lines.join("\n") + "\n", src);
        assert_ne!(a, src);
        let mut e = ed(src);
        e.rng = Rng(42);
        e.shuffle_lines();
        assert_one_step(&mut e, src);
    }

    #[test]
    fn permute_selection_texts() {
        let src = "b, a, c, a";
        let pick = |e: &mut Editor| e.select_ranges(&[0..1, 3..4, 6..7, 9..10], 0);
        let mut e = ed(src);
        pick(&mut e);
        e.sort_selections();
        assert_eq!(text(&e), "a, a, b, c");
        assert_eq!(e.selections().len(), 4);
        assert_one_step(&mut e, src);
        let mut e = ed(src);
        pick(&mut e);
        e.reverse_selections();
        assert_eq!(text(&e), "a, c, a, b");
        let mut e = ed(src);
        pick(&mut e);
        e.unique_selections();
        assert_eq!(text(&e), "b, a, c, ");
        assert_eq!(ranges(&e), vec![0..1, 3..4, 6..7, 9..9]);
        let mut e = ed(src);
        pick(&mut e);
        e.rng = Rng(7);
        e.shuffle_selections();
        let mut got: Vec<char> = text(&e).chars().filter(|c| c.is_alphabetic()).collect();
        got.sort();
        assert_eq!(got, vec!['a', 'a', 'b', 'c']);
    }

    #[test]
    fn invert_selection_selects_gaps() {
        let mut e = ed("abcdef");
        e.select_ranges(&[0..1, 3..4], 1);
        e.invert_selection();
        assert_eq!(ranges(&e), vec![1..3, 4..6]);
        assert_eq!(e.primary().range(), 4..6);
        e.select_all();
        e.invert_selection();
        assert_eq!(ranges(&e), vec![6..6], "nothing left: a cursor");
    }

    #[test]
    fn expand_to_word_paragraph_indentation() {
        let mut e = ed("foo bar\nbaz\n\nqux\n");
        e.click(5, false);
        e.add_cursor(14);
        e.expand_to_word();
        assert_eq!(ranges(&e), vec![4..7, 13..16]);
        e.click(9, false);
        e.expand_to_paragraph();
        assert_eq!(ranges(&e), vec![0..12]);
        let src = "fn f() {\n    a;\n\n    if x {\n        b;\n    }\n}\n";
        let mut e = ed(src);
        e.click(24, false); // in "if x {"
        e.expand_to_indentation();
        assert_eq!(e.buffer.slice(e.primary().range()), "    a;\n\n    if x {\n        b;\n    }");
    }

    #[test]
    fn wrap_paragraph_keeps_indent_and_comment_marker() {
        let src = "  // one two three four\n  // five\n\nsix seven eight\n";
        let mut e = ed(src);
        e.click(5, false);
        e.wrap_paragraph(14);
        assert_eq!(text(&e), "  // one two\n  // three\n  // four five\n\nsix seven eight\n");
        assert_one_step(&mut e, src);
        let mut e = ed("a b c d\r\n\r\ne f g h");
        e.click(0, false);
        e.add_cursor(12);
        e.wrap_paragraph(3);
        assert_eq!(text(&e), "a b\r\nc d\r\n\r\ne f\r\ng h");
    }

    #[test]
    fn convert_indentation_both_ways() {
        let src = "\tx\n\t  y\n        z\n";
        let mut e = ed(src);
        e.convert_indentation(true);
        assert_eq!(text(&e), "    x\n      y\n        z\n");
        assert!(e.use_spaces);
        assert_one_step(&mut e, src);
        e.convert_indentation(false);
        assert_eq!(text(&e), "\tx\n\t  y\n\t\tz\n");
        let mut e = ed("\ta\n\tb\n");
        sel(&mut e, 4, 5);
        e.convert_indentation(true);
        assert_eq!(text(&e), "\ta\n    b\n", "only the selected line");
    }

    #[test]
    fn guess_indentation_from_text() {
        assert_eq!(ed("a\nb\n").guess_indentation(), None);
        assert_eq!(ed("a {\n\tb\n\t\tc\n").guess_indentation(), Some((false, 4)));
        assert_eq!(ed("a\n  b\n    c\n  d\n").guess_indentation(), Some((true, 2)));
        assert_eq!(ed("a\n    b\n     * c\n    d\n").guess_indentation(), Some((true, 4)));
    }

    #[test]
    fn reindent_follows_previous_line() {
        let src = "fn f() {\nx;\n      y;\n}\n";
        let mut e = ed(src);
        e.select_all();
        e.reindent();
        assert_eq!(text(&e), "fn f() {\n    x;\n    y;\n}\n");
        assert_one_step(&mut e, src);
        let mut e = ed("if a:\nb\nc\n");
        e.click(6, false);
        e.add_cursor(8);
        e.reindent();
        assert_eq!(text(&e), "if a:\n    b\n    c\n");
    }

    #[test]
    fn bookmarks_toggle_jump_and_follow_edits() {
        let mut e = ed("aa\nbb\ncc\ndd\n");
        e.click(4, false);
        e.add_cursor(10);
        e.toggle_bookmark();
        assert_eq!(e.bookmarks(), &[4, 10]);
        e.click(0, false);
        e.next_bookmark();
        assert_eq!(e.primary().head, 4);
        e.next_bookmark();
        e.next_bookmark();
        assert_eq!(e.primary().head, 4, "wraps");
        e.prev_bookmark();
        assert_eq!(e.primary().head, 10, "wraps back");
        // Edits move them; deleting a span collapses to its start.
        e.click(0, false);
        e.insert_text("xx");
        assert_eq!(e.bookmarks(), &[6, 12]);
        e.undo();
        assert_eq!(e.bookmarks(), &[4, 10]);
        sel(&mut e, 3, 11);
        e.backspace(false);
        assert_eq!(e.bookmarks(), &[3]);
        e.undo();
        e.select_all_bookmarks();
        assert_eq!(ranges(&e), vec![3..3]);
        e.click(3, false);
        e.toggle_bookmark();
        assert!(e.bookmarks().is_empty(), "toggled off by line");
        e.toggle_bookmark();
        e.clear_bookmarks();
        assert!(e.bookmarks().is_empty());
    }

    #[test]
    fn word_completion() {
        let mut e = ed("foobar Football fooBaz foo\nfo");
        e.click(29, false);
        assert_eq!(e.word_prefix_at_cursor(), Some((27, "fo".into())));
        assert_eq!(e.completions(10), vec!["foo", "fooBaz", "foobar", "Football"]);
        assert_eq!(e.completions(1), vec!["foo"]);
        let mut e = ed("x fo\nFo");
        e.click(1, false);
        e.add_cursor(4);
        e.add_cursor(7); // primary: prefix "Fo"
        e.complete("foo");
        assert_eq!(text(&e), "xfoo foo\nfoo", "matching prefixes replaced, else inserted");
        assert_eq!(ranges(&e), vec![4..4, 8..8, 12..12]);
        assert_one_step(&mut e, "x fo\nFo");
        let mut e = ed("x  ");
        e.click(2, false);
        assert_eq!(e.word_prefix_at_cursor(), None);
        assert!(e.completions(5).is_empty());
    }

    #[test]
    fn split_words_on_case_and_separators() {
        let w = |s: &str| split_words(s);
        assert_eq!(w("fooBar"), ["foo", "Bar"]);
        assert_eq!(w("FooBAR"), ["Foo", "BAR"]);
        assert_eq!(w("HTTPServer"), ["HTTP", "Server"]);
        assert_eq!(w("utf8Decoder"), ["utf8", "Decoder"]);
        assert_eq!(w("HTTP2Server"), ["HTTP2", "Server"]);
        assert_eq!(w("foo_bar-baz  qux"), ["foo", "bar", "baz", "qux"]);
    }

    #[test]
    fn case_conversions() {
        let src = "let fooBar = HTTPServer;\n";
        let mut e = ed(src);
        e.click(5, false);
        e.add_cursor(15);
        e.snake_case();
        assert_eq!(text(&e), "let foo_bar = http_server;\n");
        assert_eq!(ranges(&e), vec![5..5, 16..16], "cursors keep their offset in the word");
        assert_one_step(&mut e, src);
        e.kebab_case();
        assert_eq!(text(&e), "let foo-bar = http-server;\n");
        e.upper_camel_case();
        assert_eq!(text(&e), "let FooBar = HttpServer;\n", "a cursor takes the whole kebab word");
        e.lower_camel_case();
        assert_eq!(text(&e), "let fooBar = httpServer;\n");

        let src = "Some words here\nmore_words.__private_thing";
        let mut e = ed(src);
        e.select_all();
        e.snake_case();
        assert_eq!(text(&e), "some_words_here\nmore_words.__private_thing");
        e.kebab_case();
        assert_eq!(text(&e), "some-words-here\nmore-words.__private-thing", "edge separators kept");
        assert_eq!(e.primary().range(), 0..text(&e).chars().count());
        e.lower_camel_case();
        assert_eq!(text(&e), "someWordsHere\nmoreWords.__privateThing");
    }

    #[test]
    fn sort_selections_ignoring_case() {
        let src = "b, a, C, A";
        let pick = |e: &mut Editor| e.select_ranges(&[0..1, 3..4, 6..7, 9..10], 0);
        let mut e = ed(src);
        pick(&mut e);
        e.sort_selections_case_insensitive();
        assert_eq!(text(&e), "a, A, b, C", "ties keep their order");
        assert_one_step(&mut e, src);
        let mut e = ed(src);
        pick(&mut e);
        e.sort_selections();
        assert_eq!(text(&e), "A, C, a, b");
    }

    #[test]
    fn expand_selection_grows_through_units() {
        let src = "fn f() {\n    call(\"hi there\", x);\n    y;\n}\n";
        let mut e = ed(src);
        e.click(23, false); // in "there"
        let mut got = Vec::new();
        for _ in 0..8 {
            e.expand_selection();
            got.push(e.primary().range());
        }
        assert_eq!(
            got,
            vec![
                22..27, // word
                19..27, // inside the quotes
                18..28, // with the quotes
                18..31, // inside the parens
                17..32, // with the parens
                9..34,  // the line
                9..41,  // the indentation block (= inside the braces)
                7..42,  // with the braces
            ]
        );
        e.expand_selection();
        assert_eq!(e.primary().range(), 0..43, "the whole buffer");
        e.expand_selection();
        assert_eq!(e.primary().range(), 0..43);

        let mut e = ed("ab cd\nef");
        e.click(1, false);
        e.add_cursor(4);
        e.expand_selection();
        assert_eq!(ranges(&e), vec![0..2, 3..5]);
        e.expand_selection();
        assert_eq!(ranges(&e), vec![0..6], "both grow to the line and merge");
    }

    #[test]
    fn expand_to_line_upward_repeats_upward() {
        let mut e = ed("ab\ncd\nef");
        e.click(4, false);
        e.expand_to_line_upward();
        assert_eq!(e.primary().range(), 3..6);
        assert_eq!(e.primary().head, 3, "head at the top");
        e.expand_to_line_upward();
        assert_eq!(e.primary().range(), 0..6);
        e.expand_to_line_upward();
        assert_eq!(e.primary().range(), 0..6, "nothing above");
        let mut e = ed("a\nb\nc\nd\n");
        e.click(2, false);
        e.add_cursor(6);
        e.expand_to_line_upward();
        e.expand_to_line_upward();
        assert_eq!(ranges(&e), vec![0..4, 4..8]);
    }

    #[test]
    fn expand_to_block_braces_and_indentation() {
        let src = "fn f() {\n    call(\"hi there\", x);\n    y;\n}\n";
        let mut e = ed(src);
        e.click(36, false); // in "y;"
        e.expand_to_block();
        assert_eq!(e.primary().range(), 9..40, "the lines inside the braces");
        e.expand_to_block();
        assert_eq!(e.primary().range(), 7..42, "with the braces");

        let mut e = ed("def f():\n    a\n\n    b\nc\n");
        e.click(13, false);
        e.expand_to_block();
        assert_eq!(e.primary().range(), 9..21, "indented block, blank line inside");
        e.expand_to_block();
        assert_eq!(e.primary().range(), 0..21, "with its header");

        let mut e = ed("{ a }\n{ b }\n");
        e.click(2, false);
        e.add_cursor(8);
        e.expand_to_block();
        assert_eq!(ranges(&e), vec![1..4, 7..10]);
        e.expand_to_block();
        assert_eq!(ranges(&e), vec![0..5, 6..11]);
    }

    #[test]
    fn mark_select_swap_delete() {
        let src = "one two three\n";
        let mut e = ed(src);
        assert!(!e.has_mark());
        e.click(4, false);
        e.set_mark();
        assert!(e.has_mark());
        e.click(13, false);
        e.select_to_mark();
        assert_eq!(e.primary(), Selection { anchor: 4, head: 13, goal: None });
        // The mark follows edits, undo and redo.
        e.click(0, false);
        e.insert_text("xx");
        e.click(15, false);
        e.select_to_mark();
        assert_eq!(e.primary().range(), 6..15);
        e.undo();
        e.click(13, false);
        e.select_to_mark();
        assert_eq!(e.primary().range(), 4..13);
        e.redo();
        e.click(15, false);
        e.select_to_mark();
        assert_eq!(e.primary().range(), 6..15);
        e.undo();
        e.click(13, false);
        e.swap_with_mark();
        assert_eq!(ranges(&e), vec![4..4]);
        e.swap_with_mark();
        assert_eq!(ranges(&e), vec![13..13], "the mark moved to the old cursor");
        assert_eq!(e.delete_to_mark(), "two three");
        assert_eq!(text(&e), "one \n");
        assert_eq!(ranges(&e), vec![4..4]);
        assert_one_step(&mut e, src);
        e.clear_mark();
        assert!(!e.has_mark());
        e.click(2, false);
        e.select_to_mark();
        assert_eq!(ranges(&e), vec![2..2], "no mark: nothing happens");
        assert_eq!(e.delete_to_mark(), "");
    }

    #[test]
    fn mark_per_cursor() {
        let src = "ab\ncd\n";
        let mut e = ed(src);
        e.click(0, false);
        e.add_cursor(3);
        e.set_mark();
        e.move_end(false);
        e.select_to_mark();
        assert_eq!(ranges(&e), vec![0..2, 3..5]);
        e.move_end(false);
        assert_eq!(e.delete_to_mark(), "ab\ncd");
        assert_eq!(text(&e), "\n\n");
        assert_eq!(ranges(&e), vec![0..0, 1..1]);
        assert_one_step(&mut e, src);
        e.undo();
        // One cursor, two marks: the primary mark (the last added cursor's) is used.
        e.click(5, false);
        e.select_to_mark();
        assert_eq!(e.primary().range(), 3..5);
    }

    #[test]
    fn soft_undo_steps_through_selections_and_edits() {
        let mut e = ed("abc def\nghi\n");
        e.click(1, false);
        e.insert_text("X");
        e.move_right(false, false);
        e.move_right(false, false);
        e.move_right(false, false); // consecutive moves: one step
        e.click(10, false);
        let heads = |e: &Editor| e.selections().iter().map(|s| s.head).collect::<Vec<_>>();
        assert_eq!(heads(&e), [10]);
        e.soft_undo();
        assert_eq!(heads(&e), [5]);
        e.soft_undo();
        assert_eq!(heads(&e), [2]);
        e.soft_undo();
        assert_eq!(text(&e), "abc def\nghi\n", "then the edit");
        assert_eq!(heads(&e), [1]);
        e.soft_undo();
        assert_eq!(heads(&e), [0]);
        e.soft_redo();
        assert_eq!(heads(&e), [1]);
        e.soft_redo();
        assert_eq!(text(&e), "aXbc def\nghi\n");
        assert_eq!(heads(&e), [2]);
        e.soft_redo();
        assert_eq!(heads(&e), [5]);
        e.soft_redo();
        assert_eq!(heads(&e), [10]);
        e.soft_redo();
        assert_eq!(heads(&e), [10], "nothing left to redo");
        // Plain undo ignores selection history.
        e.undo();
        assert_eq!(text(&e), "abc def\nghi\n");
    }

    #[test]
    fn soft_undo_removes_added_cursors() {
        let mut e = ed("a\nb\nc\n");
        e.click(0, false);
        e.add_cursor(2);
        e.add_cursor(4);
        e.insert_text("-");
        assert_eq!(text(&e), "-a\n-b\n-c\n");
        e.soft_undo();
        assert_eq!(text(&e), "a\nb\nc\n");
        assert_eq!(ranges(&e), vec![0..0, 2..2, 4..4]);
        e.soft_undo();
        assert_eq!(ranges(&e), vec![0..0, 2..2]);
        e.soft_undo();
        assert_eq!(ranges(&e), vec![0..0]);
        // A new selection change drops the selection redo side; the edit stays redoable.
        e.click(4, false);
        e.soft_redo();
        assert_eq!(text(&e), "-a\n-b\n-c\n");
        assert_eq!(ranges(&e), vec![1..1, 4..4, 7..7]);
        // Bounded history.
        for i in 0..300 {
            e.click(i % 6, false);
        }
        assert_eq!(e.sel_past.len(), SEL_HISTORY);
    }

    #[test]
    fn soft_undo_interleaves_and_skips_stale_steps() {
        let mut e = ed("");
        e.insert_text("a");
        e.select_word_at(0);
        e.click(1, false);
        e.insert_text("b"); // a new step: the click broke coalescing
        e.soft_undo();
        assert_eq!(text(&e), "a");
        e.soft_undo();
        assert_eq!(e.primary().range(), 0..1, "back to the word selection");
        e.soft_undo();
        assert_eq!(ranges(&e), vec![1..1]);
        e.soft_undo();
        assert_eq!(text(&e), "");
        // A drag inside coalesced typing leaves no restorable text state.
        let mut e = ed("");
        e.insert_text("a");
        e.drag_to(0);
        e.insert_text("b");
        assert_eq!(text(&e), "b");
        e.soft_undo();
        assert_eq!(text(&e), "");
        assert!(e.sel_past.is_empty());
    }

    #[test]
    fn delete_word_wrappers() {
        let mut e = ed("foo bar baz");
        e.click(4, false);
        e.delete_word_forward();
        assert_eq!(text(&e), "foo  baz");
        e.delete_word_backward();
        assert_eq!(text(&e), " baz");
        assert_eq!(ranges(&e), vec![0..0]);
    }
}

#[cfg(test)]
mod indent_tests {
    use super::*;

    const CARET: char = '‸';

    /// An editor for `syntax` with a cursor at every `‸`.
    fn ed(syntax: &str, marked: &str) -> Editor {
        let mut cursors = Vec::new();
        let mut plain = String::new();
        for c in marked.chars() {
            if c == CARET {
                cursors.push(plain.chars().count());
            } else {
                plain.push(c);
            }
        }
        let mut e = Editor::new(Buffer::from_str(&plain));
        e.indent_rules = indent::rules_for(syntax);
        e.click(cursors[0], false);
        for &c in &cursors[1..] {
            e.add_cursor(c);
        }
        e
    }

    /// Text with a `‸` at every cursor.
    fn shown(e: &Editor) -> String {
        let mut out = String::new();
        let heads: Vec<usize> = e.selections().iter().map(|s| s.head).collect();
        for (i, c) in e.buffer.slice(0..e.buffer.len_chars()).chars().enumerate() {
            if heads.contains(&i) {
                out.push(CARET);
            }
            out.push(c);
        }
        if heads.contains(&e.buffer.len_chars()) {
            out.push(CARET);
        }
        out
    }

    fn enter(syntax: &str, marked: &str) -> String {
        let mut e = ed(syntax, marked);
        e.newline();
        shown(&e)
    }

    fn typed(syntax: &str, marked: &str, s: &str) -> String {
        let mut e = ed(syntax, marked);
        for c in s.chars() {
            e.type_char(&c.to_string());
        }
        shown(&e)
    }

    #[test]
    fn enter_after_opener_indents() {
        assert_eq!(enter("Rust", "fn main() {‸\n"), "fn main() {\n    ‸\n");
        assert_eq!(enter("JavaScript", "  foo(a,‸\n"), "  foo(a,\n      ‸\n");
        assert_eq!(enter("JSON", "\"a\": [‸\n"), "\"a\": [\n    ‸\n");
        assert_eq!(enter("CSS", "a { color: red; }‸\n"), "a { color: red; }\n‸\n");
        assert_eq!(enter("Plain Text", "x {‸\n"), "x {\n    ‸\n");
        // Brackets in strings and comments don't count.
        assert_eq!(enter("Rust", "let s = \"{\";‸\n"), "let s = \"{\";\n‸\n");
        assert_eq!(enter("C", "f(); // (‸\n"), "f(); // (\n‸\n");
        // Only the text before the caret counts, and spaces after it go.
        assert_eq!(enter("Rust", "if x {‸   y\n"), "if x {\n    ‸y\n");
    }

    #[test]
    fn enter_between_pair_splits_into_three_lines() {
        assert_eq!(enter("Rust", "fn main() {‸}\n"), "fn main() {\n    ‸\n}\n");
        assert_eq!(enter("Python", "    f(‸)\n"), "    f(\n        ‸\n    )\n");
        assert_eq!(enter("JavaScript", "x = [‸];\n"), "x = [\n    ‸\n];\n");
        assert_eq!(enter("Go", "a {‸  }\n"), "a {\n    ‸\n}\n");
        assert_eq!(enter("HTML", "  <div class=\"a\">‸</div>\n"), "  <div class=\"a\">\n      ‸\n  </div>\n");
        // Mismatched pair: no split.
        assert_eq!(enter("Rust", "(‸]\n"), "(\n    ‸]\n");
    }

    #[test]
    fn split_is_one_undo_step_with_multiple_cursors() {
        let mut e = ed("Rust", "a {‸}\n  b(‸)\nc\n");
        e.newline();
        assert_eq!(shown(&e), "a {\n    ‸\n}\n  b(\n      ‸\n  )\nc\n");
        e.insert_text("x");
        assert_eq!(shown(&e), "a {\n    x‸\n}\n  b(\n      x‸\n  )\nc\n");
        e.undo();
        e.undo();
        assert_eq!(shown(&e), "a {‸}\n  b(‸)\nc\n");
        e.redo();
        assert_eq!(shown(&e), "a {\n    ‸\n}\n  b(\n      ‸\n  )\nc\n");
    }

    #[test]
    fn multiple_cursors_on_one_line() {
        assert_eq!(enter("Rust", "    a‸b‸c\n"), "    a\n    ‸b\n    ‸c\n");
        // Two cursors inside the same leading whitespace.
        assert_eq!(enter("Rust", "  ‸  ‸x\n"), "\n  ‸\n    ‸x\n");
    }

    #[test]
    fn enter_on_whitespace_only_line_empties_it() {
        assert_eq!(enter("Rust", "    foo\n    ‸\n"), "    foo\n\n    ‸\n");
        assert_eq!(enter("Rust", "    ‸foo\n"), "\n    ‸foo\n");
    }

    #[test]
    fn enter_before_closer_lines_it_up_with_its_opener() {
        assert_eq!(enter("Rust", "fn f() {\n    x();‸}\n"), "fn f() {\n    x();\n‸}\n");
        assert_eq!(enter("Rust", "foo(\n        a,‸)\n"), "foo(\n        a,\n‸)\n");
    }

    #[test]
    fn python_rules() {
        assert_eq!(enter("Python", "def f(x):‸\n"), "def f(x):\n    ‸\n");
        assert_eq!(enter("Python", "    if x:  # yes‸\n"), "    if x:  # yes\n        ‸\n");
        assert_eq!(enter("Python", "x = {'a:': 1}‸\n"), "x = {'a:': 1}\n‸\n");
        for kw in ["return x", "pass", "break", "continue", "raise E"] {
            assert_eq!(enter("Python", &format!("    if y:\n        {kw}‸\n")), format!("    if y:\n        {kw}\n    ‸\n"));
        }
        assert_eq!(enter("Python", "        return f(‸\n"), "        return f(\n            ‸\n");
    }

    #[test]
    fn python_block_keywords_dedent_when_typed() {
        let src = "if a:\n    if b:\n        foo()\n        else‸\n";
        assert_eq!(typed("Python", src, ":"), "if a:\n    if b:\n        foo()\n    else:‸\n");
        // Already lined up with `if b`: left alone.
        let src = "if a:\n    if b:\n        foo()\n    else‸\n";
        assert_eq!(typed("Python", src, ":"), "if a:\n    if b:\n        foo()\n    else:‸\n");
        let src = "try:\n    x()\n    except ValueError‸\n";
        assert_eq!(typed("Python", src, ":"), "try:\n    x()\nexcept ValueError:‸\n");
        let src = "if a:\n    x()\n    elif b‸\n";
        assert_eq!(typed("Python", src, ":"), "if a:\n    x()\nelif b:‸\n");
        let src = "try:\n    x()\nexcept E:\n    y()\n    finally‸\n";
        assert_eq!(typed("Python", src, ":"), "try:\n    x()\nexcept E:\n    y()\nfinally:‸\n");
        // `else` with nothing to pair with stays.
        let src = "def f():\n    x()\n    else‸\n";
        assert_eq!(typed("Python", src, ":"), "def f():\n    x()\n    else:‸\n");
        // Other languages don't do this.
        let src = "if a:\n    x()\n    else‸\n";
        assert_eq!(typed("Rust", src, ":"), "if a:\n    x()\n    else:‸\n");
    }

    #[test]
    fn yaml_rules() {
        let mut e = ed("YAML", "root:‸\n");
        e.tab_width = 2;
        e.newline();
        assert_eq!(shown(&e), "root:\n  ‸\n");
        e.insert_text("- name:");
        e.newline();
        assert_eq!(shown(&e), "root:\n  - name:\n    ‸\n");
        assert_eq!(enter("YAML", "a: b‸\n"), "a: b\n‸\n");
        assert_eq!(enter("YAML", "run: |‸\n"), "run: |\n    ‸\n");
    }

    #[test]
    fn ruby_lua_shell_blocks() {
        assert_eq!(enter("Ruby", "def f‸\n"), "def f\n    ‸\n");
        assert_eq!(enter("Ruby", "xs.each do |x|‸\n"), "xs.each do |x|\n    ‸\n");
        // A block end re-indents its own line on Enter.
        assert_eq!(enter("Ruby", "def f\n    x\n    end‸\n"), "def f\n    x\nend\n‸\n");
        assert_eq!(enter("Ruby", "if a\n    x\n    else‸\n"), "if a\n    x\nelse\n    ‸\n");
        assert_eq!(enter("Lua", "function f()‸\n"), "function f()\n    ‸\n");
        assert_eq!(enter("Lua", "if x then\n    end‸\n"), "if x then\nend\n‸\n");
        let sh = "Bourne Again Shell (bash)";
        assert_eq!(enter(sh, "if [ -f x ]; then‸\n"), "if [ -f x ]; then\n    ‸\n");
        assert_eq!(enter(sh, "for f in *; do\n    echo $f\n    done‸\n"), "for f in *; do\n    echo $f\ndone\n‸\n");
        // Already at the right depth: unchanged.
        assert_eq!(enter(sh, "if x; then\n    y\nfi‸\n"), "if x; then\n    y\nfi\n‸\n");
        // Only whole keywords count.
        assert_eq!(enter(sh, "if x; then\n    y\n    find .‸\n"), "if x; then\n    y\n    find .\n    ‸\n");
        // The text moving down starts with `end`: one level up.
        assert_eq!(enter("Ruby", "def f\n    x‸end\n"), "def f\n    x\n‸end\n");
    }

    #[test]
    fn markup_tags() {
        assert_eq!(enter("HTML", "<ul>‸\n"), "<ul>\n    ‸\n");
        assert_eq!(enter("HTML", "<p>hi</p>‸\n"), "<p>hi</p>\n‸\n");
        assert_eq!(enter("HTML", "<br>‸\n"), "<br>\n‸\n");
        assert_eq!(enter("XML", "<a><b/>‸</a>\n"), "<a><b/>\n    ‸\n</a>\n");
        let src = "<div>\n    <p>a</p>\n    <‸\n";
        assert_eq!(typed("HTML", src, "/"), "<div>\n    <p>a</p>\n</‸\n");
        let src = "<div>\n    <‸\n";
        assert_eq!(typed("HTML", src, "/"), "<div>\n</‸\n");
        // `/` elsewhere is just a slash.
        assert_eq!(typed("HTML", "<div>\n    a <‸\n", "/"), "<div>\n    a </‸\n");
        assert_eq!(typed("Rust", "{\n    <‸\n", "/"), "{\n    </‸\n");
    }

    #[test]
    fn typed_closer_dedents_to_its_opener() {
        assert_eq!(typed("Rust", "fn f() {\n    x();\n    ‸\n", "}"), "fn f() {\n    x();\n}‸\n");
        assert_eq!(typed("Rust", "  foo(\n        a,\n        ‸\n", ")"), "  foo(\n        a,\n  )‸\n");
        assert_eq!(typed("Rust", "{\n    [\n        1,\n        ‸\n", "]"), "{\n    [\n        1,\n    ]‸\n");
        // Text before it on the line: no change.
        assert_eq!(typed("Rust", "{\n    x‸\n", "}"), "{\n    x}‸\n");
        // No opener: no change.
        assert_eq!(typed("Rust", "    ‸\n", "}"), "    }‸\n");
        // A closer already there is typed over.
        assert_eq!(typed("Rust", "{\n    ‸}\n", "}"), "{\n    }‸\n");
        // Undo puts the indentation back.
        let mut e = ed("Rust", "{\n    ‸\n");
        e.type_char("}");
        e.undo();
        assert_eq!(shown(&e), "{\n    ‸\n");
    }

    #[test]
    fn typed_closer_with_multiple_cursors() {
        let src = "{\n    ‸\n}\n(\n        ‸\n";
        assert_eq!(typed("Rust", src, ")"), "{\n    )‸\n}\n(\n)‸\n");
    }

    #[test]
    fn tabs_mode() {
        let mut e = ed("Rust", "\tif x {‸}\n");
        e.use_spaces = false;
        e.newline();
        assert_eq!(shown(&e), "\tif x {\n\t\t‸\n\t}\n");
        let mut e = ed("Python", "\t\tif y:\n\t\t\treturn‸\n");
        e.use_spaces = false;
        e.newline();
        assert_eq!(shown(&e), "\t\tif y:\n\t\t\treturn\n\t\t‸\n");
        // Backspace removes one tab character.
        e.backspace(false);
        assert_eq!(shown(&e), "\t\tif y:\n\t\t\treturn\n\t‸\n");
    }

    #[test]
    fn crlf_buffer() {
        assert_eq!(enter("Rust", "if x {‸}\r\n"), "if x {\r\n    ‸\r\n}\r\n");
        assert_eq!(enter("Rust", "  a\r\n  ‸\r\n"), "  a\r\n\r\n  ‸\r\n");
        let mut e = ed("Rust", "{\r\n    ‸\r\n");
        e.type_char("}");
        assert_eq!(shown(&e), "{\r\n}‸\r\n");
    }

    #[test]
    fn settings_switch_behavior_off() {
        let mut e = ed("Rust", "    if x {‸}\n");
        e.auto_indent = false;
        e.newline();
        assert_eq!(shown(&e), "    if x {\n‸}\n");

        // smart_indent off: indentation is copied; a pair still splits.
        let mut e = ed("Python", "    def f():‸\n");
        e.smart_indent = false;
        e.newline();
        assert_eq!(shown(&e), "    def f():\n    ‸\n");
        let mut e = ed("Rust", "  {‸}\n");
        e.smart_indent = false;
        e.newline();
        assert_eq!(shown(&e), "  {\n      ‸\n  }\n");
        let mut e = ed("Rust", "{\n    ‸\n");
        e.smart_indent = false;
        e.type_char("}");
        assert_eq!(shown(&e), "{\n    }‸\n");
    }

    #[test]
    fn backspace_goes_to_previous_tab_stop() {
        let mut e = ed("Rust", "      ‸x\n");
        e.backspace(false);
        assert_eq!(shown(&e), "    ‸x\n");
        e.backspace(false);
        assert_eq!(shown(&e), "‸x\n");
        // Consecutive tab-stop deletions undo together.
        e.undo();
        assert_eq!(shown(&e), "      ‸x\n");

        let mut e = ed("Rust", "        ‸\n");
        e.tab_width = 2;
        e.backspace(false);
        assert_eq!(shown(&e), "      ‸\n");

        // Outside leading indentation: one char.
        let mut e = ed("Rust", "a       ‸b\n");
        e.backspace(false);
        assert_eq!(shown(&e), "a      ‸b\n");
        // Mixed tabs and spaces: one char.
        let mut e = ed("Rust", "\t    ‸x\n");
        e.backspace(false);
        assert_eq!(shown(&e), "\t   ‸x\n");
        // A single space.
        let mut e = ed("Rust", " ‸x\n");
        e.backspace(false);
        assert_eq!(shown(&e), "‸x\n");
        // Multiple cursors.
        let mut e = ed("Rust", "    ‸a\n     ‸b\n");
        e.backspace(false);
        assert_eq!(shown(&e), "‸a\n    ‸b\n");
        // Tabs mode is untouched.
        let mut e = ed("Rust", "        ‸x\n");
        e.use_spaces = false;
        e.backspace(false);
        assert_eq!(shown(&e), "       ‸x\n");
    }
}
