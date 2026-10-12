//! Text storage: a rope (O(log n) edits and line lookups on huge files).
//!
//! All positions are char indices into the rope. A "line end" is the index
//! just before the line break, so cursors never sit inside a `\r\n` pair.

use ropey::Rope;
use std::io::{BufWriter, Write};
use std::ops::Range;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }
}

pub struct Buffer {
    rope: Rope,
    line_ending: LineEnding,
}

impl Buffer {
    pub fn from_str(text: &str) -> Self {
        let rope = Rope::from_str(text);
        let line_ending = detect_line_ending(&rope);
        Self { rope, line_ending }
    }

    /// Invalid UTF-8 is replaced rather than refused, so any file opens.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self::from_str(&String::from_utf8_lossy(bytes))
    }

    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        let mut out = BufWriter::new(std::fs::File::create(path)?);
        for chunk in self.rope.chunks() {
            out.write_all(chunk.as_bytes())?;
        }
        out.flush()
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn char(&self, idx: usize) -> char {
        self.rope.char(idx)
    }

    pub fn char_to_byte(&self, idx: usize) -> usize {
        self.rope.char_to_byte(idx)
    }

    pub fn byte_to_char(&self, idx: usize) -> usize {
        self.rope.byte_to_char(idx)
    }

    pub fn line_of(&self, idx: usize) -> usize {
        self.rope.char_to_line(idx.min(self.len_chars()))
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line.min(self.len_lines()))
    }

    /// Index just before the line's break (or end of text on the last line).
    pub fn line_end(&self, line: usize) -> usize {
        let start = self.line_start(line);
        let slice = self.rope.line(line);
        let mut len = slice.len_chars();
        while len > 0 && matches!(slice.char(len - 1), '\n' | '\r') {
            len -= 1;
        }
        start + len
    }

    /// The line's text without its line break.
    pub fn line_text(&self, line: usize) -> String {
        self.rope
            .slice(self.line_start(line)..self.line_end(line))
            .to_string()
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        self.rope.slice(range).to_string()
    }

    pub fn insert(&mut self, at: usize, text: &str) {
        self.rope.insert(at, text);
    }

    pub fn remove(&mut self, range: Range<usize>) {
        if !range.is_empty() {
            self.rope.remove(range);
        }
    }
}

fn detect_line_ending(rope: &Rope) -> LineEnding {
    if rope.len_lines() > 1 {
        let first = rope.line(0);
        let n = first.len_chars();
        if n >= 2 && first.char(n - 2) == '\r' && first.char(n - 1) == '\n' {
            return LineEnding::CrLf;
        }
        return LineEnding::Lf;
    }
    // New/one-line files follow the platform convention.
    if cfg!(windows) { LineEnding::CrLf } else { LineEnding::Lf }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_end_excludes_crlf() {
        let b = Buffer::from_str("ab\r\ncd\n");
        assert_eq!(b.line_ending(), LineEnding::CrLf);
        assert_eq!(b.len_lines(), 3);
        assert_eq!(b.line_end(0), 2);
        assert_eq!(b.line_start(1), 4);
        assert_eq!(b.line_text(1), "cd");
        assert_eq!(b.line_text(2), "");
    }

    #[test]
    fn lossy_utf8_opens() {
        let b = Buffer::from_bytes(b"ok\xffok");
        assert_eq!(b.line_text(0), "ok\u{fffd}ok");
    }
}
