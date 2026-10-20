//! Positions in the protocol's terms: 0-based line and column, the column
//! counted in UTF-16 code units (the default) or UTF-8 bytes (when the
//! server agrees to it), never in chars as Strata counts.

use ropey::Rope;
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Encoding {
    Utf8,
    #[default]
    Utf16,
}

impl Encoding {
    /// The `positionEncoding` a server chose (`utf-16` when it says nothing).
    pub fn from_name(name: Option<&str>) -> Self {
        match name {
            Some("utf-8") => Encoding::Utf8,
            _ => Encoding::Utf16,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, PartialOrd, Ord)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

impl Pos {
    #[cfg(test)]
    pub fn new(line: u32, col: u32) -> Self {
        Self { line, col }
    }

    pub fn to_json(self) -> Value {
        json!({"line": self.line, "character": self.col})
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self { line: v.get("line")?.as_u64()?.min(u32::MAX as u64) as u32, col: v.get("character")?.as_u64()?.min(u32::MAX as u64) as u32 })
    }
}

/// `{"start": …, "end": …}`.
pub fn range_from_json(v: &Value) -> Option<(Pos, Pos)> {
    Some((Pos::from_json(v.get("start")?)?, Pos::from_json(v.get("end")?)?))
}

pub fn range_to_json(r: (Pos, Pos)) -> Value {
    json!({"start": r.0.to_json(), "end": r.1.to_json()})
}

/// Chars in `line` before its line break (`\n`, `\r\n` or `\r`, the breaks
/// the protocol and the rope both count).
fn content_chars(rope: &Rope, line: usize) -> usize {
    let slice = rope.line(line);
    let mut n = slice.len_chars();
    while n > 0 && matches!(slice.char(n - 1), '\n' | '\r') {
        n -= 1;
    }
    n
}

/// The protocol position of char `idx`. A position inside a line break
/// is put at the end of the line's text.
pub fn to_lsp(rope: &Rope, idx: usize, enc: Encoding) -> Pos {
    let idx = idx.min(rope.len_chars());
    let line = rope.char_to_line(idx);
    let start = rope.line_to_char(line);
    let idx = idx.min(start + content_chars(rope, line));
    let col = match enc {
        Encoding::Utf16 => rope.char_to_utf16_cu(idx) - rope.char_to_utf16_cu(start),
        Encoding::Utf8 => rope.char_to_byte(idx) - rope.char_to_byte(start),
    };
    Pos { line: line as u32, col: col as u32 }
}

/// The char index of a protocol position. Past the end of a line means its
/// end, past the last line the end of the text; a column inside a
/// character (half a surrogate pair, part of a UTF-8 sequence) means that
/// character's start.
pub fn from_lsp(rope: &Rope, pos: Pos, enc: Encoding) -> usize {
    let line = pos.line as usize;
    if line >= rope.len_lines() {
        return rope.len_chars();
    }
    let start = rope.line_to_char(line);
    let text = rope.slice(start..start + content_chars(rope, line));
    let off = match enc {
        Encoding::Utf16 => {
            let units = (pos.col as usize).min(text.len_utf16_cu());
            let c = text.utf16_cu_to_char(units);
            // Ropey rounds a unit inside a pair up to the next char; keep the char it's in.
            if c > 0 && text.char_to_utf16_cu(c) > units { c - 1 } else { c }
        }
        Encoding::Utf8 => text.byte_to_char((pos.col as usize).min(text.len_bytes())),
    };
    start + off
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(text: &str, enc: Encoding) {
        let rope = Rope::from_str(text);
        for idx in 0..=rope.len_chars() {
            let p = to_lsp(&rope, idx, enc);
            let back = from_lsp(&rope, p, enc);
            // Inside a \r\n pair maps to the line's end, the pair's start.
            let inside_crlf = idx > 0 && idx < rope.len_chars() && rope.char(idx - 1) == '\r' && rope.char(idx) == '\n';
            assert_eq!(back, if inside_crlf { idx - 1 } else { idx }, "{text:?} at {idx} ({p:?})");
        }
    }

    #[test]
    fn utf16_counts_surrogate_pairs_and_cjk() {
        // 👋 is two UTF-16 units and four UTF-8 bytes; 日本 are one unit and three bytes each.
        let rope = Rope::from_str("a👋b\n日本x\n");
        assert_eq!(to_lsp(&rope, 2, Encoding::Utf16), Pos::new(0, 3));
        assert_eq!(to_lsp(&rope, 3, Encoding::Utf16), Pos::new(0, 4));
        assert_eq!(to_lsp(&rope, 2, Encoding::Utf8), Pos::new(0, 5));
        assert_eq!(to_lsp(&rope, 6, Encoding::Utf16), Pos::new(1, 2));
        assert_eq!(to_lsp(&rope, 6, Encoding::Utf8), Pos::new(1, 6));
        assert_eq!(from_lsp(&rope, Pos::new(0, 3), Encoding::Utf16), 2);
        assert_eq!(from_lsp(&rope, Pos::new(1, 2), Encoding::Utf16), 6);
        assert_eq!(from_lsp(&rope, Pos::new(1, 6), Encoding::Utf8), 6);
        // Half a surrogate pair, or the middle of a UTF-8 sequence: that character.
        assert_eq!(from_lsp(&rope, Pos::new(0, 2), Encoding::Utf16), 1);
        assert_eq!(from_lsp(&rope, Pos::new(0, 3), Encoding::Utf8), 1);
        assert_eq!(from_lsp(&rope, Pos::new(1, 4), Encoding::Utf8), 5);
        for text in ["a👋b\n日本x\n", "👩‍👩‍👧 family\r\nnext 𝄞\rlast", "", "\n\n", "x\r\n\r\ny"] {
            round_trip(text, Encoding::Utf16);
            round_trip(text, Encoding::Utf8);
        }
    }

    #[test]
    fn out_of_range_positions_clamp() {
        let rope = Rope::from_str("ab\r\ncd");
        assert_eq!(from_lsp(&rope, Pos::new(0, 99), Encoding::Utf16), 2, "past the line's end: before its break");
        assert_eq!(from_lsp(&rope, Pos::new(1, 99), Encoding::Utf16), 6);
        assert_eq!(from_lsp(&rope, Pos::new(7, 0), Encoding::Utf16), 6, "past the last line: the end");
        assert_eq!(to_lsp(&rope, 3, Encoding::Utf16), Pos::new(0, 2), "between \\r and \\n");
        assert_eq!(to_lsp(&rope, 99, Encoding::Utf16), Pos::new(1, 2));
        // A lone \r ends a line too.
        let rope = Rope::from_str("a\rb");
        assert_eq!(to_lsp(&rope, 2, Encoding::Utf16), Pos::new(1, 0));
    }

    #[test]
    fn json_shapes() {
        let p = Pos::from_json(&json!({"line": 3, "character": 7})).unwrap();
        assert_eq!(p, Pos::new(3, 7));
        assert_eq!(range_from_json(&range_to_json((Pos::new(1, 2), Pos::new(3, 4)))), Some((Pos::new(1, 2), Pos::new(3, 4))));
        assert_eq!(Pos::from_json(&json!({"line": 1})), None);
        assert_eq!(Encoding::from_name(Some("utf-8")), Encoding::Utf8);
        assert_eq!(Encoding::from_name(Some("utf-32")), Encoding::Utf16);
    }
}
