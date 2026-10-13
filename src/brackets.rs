//! Rainbow brackets: the nesting depth of every `( [ {` and closer on the
//! lines a view shows, without scanning the whole file each frame.
//!
//! A small lexer walks the text keeping a stack of open brackets. Its state is
//! checkpointed every `STEP` lines; an edit drops checkpoints from the edited
//! line down, and work per frame is capped like the highlighter's, so a huge
//! file fills in over a few frames instead of stalling one.
//!
//! The lexer is a heuristic: it skips quoted strings that close on the same
//! line, line comments, and block comments (which carry across lines), using
//! the syntax's comment tokens.

use crate::buffer::Buffer;
use crate::lang::CommentTokens;
use std::collections::HashMap;
use std::ops::RangeInclusive;

/// Lines between checkpoints.
const STEP: usize = 256;
/// Bytes scanned per frame before yielding…
const BUDGET: usize = 4 << 20;
/// …or time spent: a jump to the end of a 50 MB file catches up over a
/// second of frames rather than stalling them.
const TIME_BUDGET: std::time::Duration = std::time::Duration::from_millis(2);
/// Longer lines are skipped (no marks, state unchanged), like the highlighter.
const MAX_LINE: usize = 20_000;
/// Deeper nesting than this forgets its outermost brackets.
const MAX_STACK: usize = 4_096;
/// Cached lines kept while scrolling.
const KEEP: usize = 8_192;
/// Visible lines further than this past the last checkpoint start from a
/// guess (see `prepare`); `GUESS_BACK` is how far up it looks for a start.
const FAR: usize = 2_000;
const GUESS_BACK: usize = 200;

/// A bracket at `byte` in its line; `depth` is `None` for a closer without a
/// matching opener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub byte: usize,
    pub depth: Option<usize>,
}

/// Lexer state at a line start.
#[derive(Clone, Default)]
struct State {
    stack: Vec<u8>,
    block: bool,
}

/// How a syntax spells comments and quotes.
#[derive(Clone, Default)]
struct Lexer {
    line: Option<&'static str>,
    block: Option<(&'static str, &'static str)>,
    /// `'` is a quote; with `chars`, only around one char (Rust lifetimes).
    single: bool,
    chars: bool,
}

impl Lexer {
    fn new(syntax: &str, tokens: CommentTokens) -> Self {
        let chars = matches!(syntax, "Rust" | "OCaml" | "Haskell" | "Literate Haskell");
        let single = !matches!(syntax, "Lisp" | "Clojure" | "Batch File" | "ASP") || chars;
        Self { line: tokens.0, block: tokens.1, single, chars }
    }

    /// Advance `st` over one line, pushing its brackets to `marks`.
    fn scan(&self, text: &[u8], st: &mut State, mut marks: Option<&mut Vec<Mark>>) {
        let mut i = 0;
        while i < text.len() {
            if st.block {
                let close = self.block.map_or("", |b| b.1).as_bytes();
                match find(&text[i..], close) {
                    Some(k) if !close.is_empty() => {
                        st.block = false;
                        i += k + close.len();
                        continue;
                    }
                    _ => return,
                }
            }
            if let Some((open, _)) = self.block
                && token_at(text, i, open)
            {
                st.block = true;
                i += open.len();
                continue;
            }
            if self.line.is_some_and(|t| token_at(text, i, t)) {
                return;
            }
            let c = text[i];
            match c {
                b'"' | b'`' => i = closing(text, i, c).unwrap_or(i),
                b'\'' if self.chars => i = char_literal(text, i).unwrap_or(i),
                b'\'' if self.single && !(i > 0 && text[i - 1].is_ascii_alphanumeric()) => {
                    i = closing(text, i, c).unwrap_or(i)
                }
                b'(' | b'[' | b'{' => {
                    if let Some(m) = marks.as_deref_mut() {
                        m.push(Mark { byte: i, depth: Some(st.stack.len()) });
                    }
                    if st.stack.len() >= MAX_STACK {
                        st.stack.drain(..MAX_STACK / 4);
                    }
                    st.stack.push(c);
                }
                b')' | b']' | b'}' => {
                    let open = match c {
                        b')' => b'(',
                        b']' => b'[',
                        _ => b'{',
                    };
                    let depth = (st.stack.last() == Some(&open)).then(|| {
                        st.stack.pop();
                        st.stack.len()
                    });
                    if let Some(m) = marks.as_deref_mut() {
                        m.push(Mark { byte: i, depth });
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `tok` starts at `i`; a word token (`REM`) must stand alone.
fn token_at(text: &[u8], i: usize, tok: &str) -> bool {
    let t = tok.as_bytes();
    if !text[i..].starts_with(t) {
        return false;
    }
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    !(t[0].is_ascii_alphabetic() && (i > 0 && word(text[i - 1]) || text.get(i + t.len()).is_some_and(|&b| word(b))))
}

/// Index of the unescaped `q` closing the quote at `i`, if it's on this line.
fn closing(text: &[u8], i: usize, q: u8) -> Option<usize> {
    let mut k = i + 1;
    while k < text.len() {
        match text[k] {
            b'\\' => k += 1,
            b if b == q => return Some(k),
            _ => {}
        }
        k += 1;
    }
    None
}

/// End of a char literal at `i` (`'x'`, `'\n'`, `'\u{1F600}'`); `None` for a lifetime.
fn char_literal(text: &[u8], i: usize) -> Option<usize> {
    if text.get(i + 1) == Some(&b'\\') {
        return closing(text, i, b'\'').filter(|&k| k <= i + 12);
    }
    // One char, which may take several bytes.
    let len = match *text.get(i + 1)? {
        b if b < 0x80 => 1,
        b if b >= 0xF0 => 4,
        b if b >= 0xE0 => 3,
        _ => 2,
    };
    (text.get(i + 1 + len) == Some(&b'\'')).then_some(i + 1 + len)
}

/// Bracket depths for one buffer, cached by line.
#[derive(Default)]
pub struct Brackets {
    lexer: Lexer,
    syntax: Option<String>,
    /// Editor version the cache matches.
    version: Option<u64>,
    /// An edit was reported since the version last changed.
    told: bool,
    /// `checkpoints[k]` = state at the start of line `k * STEP`.
    checkpoints: Vec<State>,
    lines: HashMap<usize, Vec<Mark>>,
}

impl Brackets {
    /// Forget everything from `line` down (it was edited).
    pub fn invalidate_from(&mut self, line: usize) {
        self.checkpoints.truncate(line / STEP + 1);
        self.lines.retain(|&l, _| l < line);
        self.told = true;
    }

    fn reset(&mut self) {
        self.checkpoints.clear();
        self.lines.clear();
    }

    /// The marks of `line`, if `prepare` reached it.
    pub fn line(&self, line: usize) -> Option<&[Mark]> {
        self.lines.get(&line).map(Vec::as_slice)
    }

    /// Compute `visible` lines. Returns false if the budget ran out (the caller
    /// should repaint; finished work is kept for the next frame).
    pub fn prepare(&mut self, buf: &Buffer, version: u64, syntax: &str, tokens: CommentTokens, visible: RangeInclusive<usize>) -> bool {
        if self.syntax.as_deref() != Some(syntax) {
            self.syntax = Some(syntax.into());
            self.lexer = Lexer::new(syntax, tokens);
            self.reset();
        }
        if self.version != Some(version) {
            // An edit nobody reported: start over rather than show stale depths.
            if !self.told {
                self.reset();
            }
            self.version = Some(version);
        }
        self.told = false;
        let last = (*visible.end()).min(buf.len_lines().saturating_sub(1));
        let first = (*visible.start()).min(last);
        if (first..=last).all(|l| self.lines.contains_key(&l)) {
            return true;
        }
        if self.lines.len() > KEEP {
            self.lines.retain(|&l, _| l + 2_000 >= first && l <= last + 2_000);
        }
        if self.checkpoints.is_empty() {
            self.checkpoints.push(State::default());
        }
        let k = (first / STEP).min(self.checkpoints.len() - 1);
        let mut st = self.checkpoints[k].clone();
        if first - k * STEP > FAR {
            // Far past what's been scanned (a jump into a big file): start
            // fresh at an unindented line just above, where code is usually
            // at depth 0, instead of scanning everything before it.
            let floor = first.saturating_sub(GUESS_BACK);
            let start = (floor..=first).rev().find(|&l| buf.line_chars(l).next().is_some_and(|c| !c.is_whitespace())).unwrap_or(first);
            let mut st = State::default();
            let mut text = Vec::new();
            for (line, chunks) in (start..=last).zip(buf.lines_from(start)) {
                text.clear();
                chunks.for_each(|c| text.extend_from_slice(c.as_bytes()));
                let mut marks = Vec::new();
                if text.len() <= MAX_LINE {
                    self.lexer.scan(&text, &mut st, (line >= first).then_some(&mut marks));
                }
                if line >= first {
                    self.lines.insert(line, marks);
                }
            }
            return true;
        }
        let mut budget = BUDGET;
        let started = std::time::Instant::now();
        let time = crate::highlight::time_budget(TIME_BUDGET);
        let mut text: Vec<u8> = Vec::new();
        for (line, chunks) in (k * STEP..).zip(buf.lines_from(k * STEP)) {
            if line > last {
                break;
            }
            // Past the first checkpoint made here, so every call keeps some progress.
            if line % 64 == 0 && line > (k + 1) * STEP && started.elapsed() > time {
                return false;
            }
            if line.is_multiple_of(STEP) && line / STEP == self.checkpoints.len() {
                self.checkpoints.push(st.clone());
            }
            text.clear();
            let mut long = false;
            for c in chunks {
                if text.len() + c.len() > MAX_LINE {
                    long = true;
                    break;
                }
                text.extend_from_slice(c.as_bytes());
            }
            let cost = text.len() + 16;
            if budget < cost {
                return false;
            }
            budget -= cost;
            let mut marks = Vec::new();
            if !long {
                self.lexer.scan(&text, &mut st, (line >= first).then_some(&mut marks));
            }
            if line >= first {
                self.lines.insert(line, marks);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST: (Option<&str>, Option<(&str, &str)>) = (Some("//"), Some(("/*", "*/")));

    fn depths(b: &Brackets, line: usize) -> Vec<Option<usize>> {
        b.line(line).unwrap().iter().map(|m| m.depth).collect()
    }

    #[test]
    fn depth_across_lines_and_unmatched_closers() {
        let buf = Buffer::from_str("fn f(a: [u8; 2]) {\n    g(\"(\", '[', x);\n}\n) // (\n");
        let mut b = Brackets::default();
        assert!(b.prepare(&buf, 0, "Rust", RUST, 0..=3));
        assert_eq!(depths(&b, 0), [Some(0), Some(1), Some(1), Some(0), Some(0)]);
        assert_eq!(depths(&b, 1), [Some(1), Some(1)], "brackets in strings and char literals are skipped");
        assert_eq!(depths(&b, 2), [Some(0)]);
        assert_eq!(depths(&b, 3), [None], "an unmatched closer; the comment is skipped");
    }

    #[test]
    fn lifetimes_quotes_and_block_comments() {
        let buf = Buffer::from_str("fn f<'a>(x: &'a str) {\n/* {\n ( */ }\n");
        let mut b = Brackets::default();
        assert!(b.prepare(&buf, 0, "Rust", RUST, 0..=2));
        assert_eq!(depths(&b, 0), [Some(0), Some(0), Some(0)]);
        assert_eq!(depths(&b, 1), []);
        assert_eq!(depths(&b, 2), [Some(0)]);
        // Python: single quotes are strings, but not apostrophes in words.
        let buf = Buffer::from_str("x = ['(', \"don't\"]  # (\n");
        let mut b = Brackets::default();
        assert!(b.prepare(&buf, 0, "Python", (Some("#"), None), 0..=0));
        assert_eq!(depths(&b, 0), [Some(0), Some(0)]);
    }

    #[test]
    fn edits_invalidate_from_the_changed_line() {
        let mut text = "{\n".to_string();
        text.push_str(&"x\n".repeat(1_000));
        text.push_str("}\n");
        let mut buf = Buffer::from_str(&text);
        let mut b = Brackets::default();
        assert!(b.prepare(&buf, 0, "Rust", RUST, 1_001..=1_001));
        assert_eq!(depths(&b, 1_001), [Some(0)]);
        // A second opener on line 500 pushes the closer one level in.
        buf.insert(buf.line_start(500), "(");
        b.invalidate_from(500);
        assert_eq!(b.checkpoints.len(), 500 / STEP + 1);
        assert!(b.prepare(&buf, 1, "Rust", RUST, 1_001..=1_001));
        assert_eq!(depths(&b, 1_001), [None], "the closing brace no longer matches `(`");
        // Unreported edits drop the whole cache.
        buf.remove(buf.line_start(500)..buf.line_start(500) + 1);
        assert!(b.prepare(&buf, 2, "Rust", RUST, 1_001..=1_001));
        assert_eq!(depths(&b, 1_001), [Some(0)]);
    }

    #[test]
    fn deep_file_fills_in_within_a_few_frames() {
        let block = "fn f() {\n    if x { g(a[0], (1, 2)); }\n}\n";
        let text = block.repeat(200_000 / 3);
        let buf = Buffer::from_str(&text);
        let mut b = Brackets::default();
        let end = buf.len_lines() - 2;
        let start = std::time::Instant::now();
        let mut frames = 1;
        while !b.prepare(&buf, 0, "Rust", RUST, end - 50..=end) {
            frames += 1;
        }
        let took = start.elapsed();
        // Far past any checkpoint: started fresh at the `fn` line above, at once.
        assert_eq!(frames, 1);
        assert!(took.as_secs_f64() < 2.0, "{took:?}");
        assert_eq!(depths(&b, end), [Some(0)]);
        // Scrolling back to the middle resumes from a checkpoint.
        let start = std::time::Instant::now();
        assert!(b.prepare(&buf, 0, "Rust", RUST, 100_000..=100_050));
        assert!(start.elapsed().as_secs_f64() < 0.05);
        // Line 100_000 is a block's middle line, one `{` deep.
        let want = [1, 2, 3, 3, 3, 3, 2, 1].map(Some);
        assert_eq!(depths(&b, 100_000), want);
    }
}
