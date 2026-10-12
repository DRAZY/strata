//! Soft wrap: where each line breaks into visual rows, and an index of the
//! first visual row of every line so rows map to lines by binary search.
//!
//! Widths are monospace columns: a tab runs to the next tab stop, CJK takes
//! two columns, anything else one. Rows break after the last whitespace that
//! fits; a word wider than a row is cut where the row is full.

use crate::buffer::Buffer;
use crate::fonts::is_cjk;
use std::ops::Range;

/// Wrapping scans the whole text on every edit, so huge buffers stay unwrapped.
pub const MAX_CHARS: usize = 8 << 20;

/// Feeds one line's chars and reports where rows break.
struct Wrapper {
    cols: usize,
    tab: usize,
    /// Index of the next char.
    i: usize,
    /// Tab-stop column (tabs expanded, every other char 1), as the line is laid out.
    col: usize,
    /// Width of the current row.
    width: usize,
    row_start: usize,
    /// Just after the last whitespace on this row, and the row width there.
    space: Option<(usize, usize)>,
}

impl Wrapper {
    fn new(cols: usize, tab: usize) -> Self {
        Self { cols: cols.max(1), tab: tab.max(1), i: 0, col: 0, width: 0, row_start: 0, space: None }
    }

    /// Calls `brk` with the char offset of each row that starts before `c` ends.
    fn push(&mut self, c: char, mut brk: impl FnMut(usize)) {
        let i = self.i;
        self.i += 1;
        if c == '\n' || c == '\r' {
            return;
        }
        let adv = if c == '\t' { self.tab - self.col % self.tab } else { 1 };
        let w = if c == '\t' { adv } else if is_cjk(c) { 2 } else { 1 };
        let blank = c == ' ' || c == '\t';
        // One whitespace char may hang past the edge rather than start a row.
        while self.width + w > self.cols && i > self.row_start && !(blank && self.width <= self.cols) {
            let at = match self.space.take() {
                Some((at, width)) => {
                    self.width -= width;
                    at
                }
                None => {
                    self.width = 0;
                    i
                }
            };
            self.row_start = at;
            brk(at);
        }
        self.col += adv;
        self.width += w;
        if blank {
            self.space = Some((i + 1, self.width));
        }
    }
}

/// One line's rows plus its display columns (what the editor's galleys index by).
pub struct LineRows {
    /// Char offset of each row's first char; the first is 0.
    pub starts: Vec<usize>,
    /// `dcols[k]`: display column before char `k` (tabs expanded); one extra at the end.
    pub dcols: Vec<usize>,
}

impl LineRows {
    /// `cols: None` is a single row (wrap off).
    pub fn new(chars: impl Iterator<Item = char>, cols: Option<usize>, tab: usize) -> Self {
        let tab = tab.max(1);
        let mut starts = vec![0];
        let mut dcols = vec![0];
        let mut wrapper = cols.map(|c| Wrapper::new(c, tab));
        let mut col = 0;
        for c in chars {
            if let Some(w) = &mut wrapper {
                w.push(c, |at| starts.push(at));
            }
            col += if c == '\t' { tab - col % tab } else { 1 };
            dcols.push(col);
        }
        Self { starts, dcols }
    }

    pub fn rows(&self) -> usize {
        self.starts.len()
    }

    /// Chars in the line.
    pub fn len(&self) -> usize {
        self.dcols.len() - 1
    }

    /// Row holding char offset `off`; a row boundary belongs to the later row.
    pub fn row_of(&self, off: usize) -> usize {
        self.starts.partition_point(|&s| s <= off).saturating_sub(1)
    }

    /// Char offsets of `row`.
    pub fn range(&self, row: usize) -> Range<usize> {
        self.starts[row]..self.starts.get(row + 1).copied().unwrap_or(self.len())
    }

    pub fn is_last(&self, row: usize) -> bool {
        row + 1 >= self.rows()
    }

    /// Display column of `off`, counted from the start of its row.
    pub fn col_in_row(&self, off: usize) -> usize {
        let off = off.min(self.len());
        self.dcols[off] - self.dcols[self.starts[self.row_of(off)]]
    }

    /// Offset in `row` nearest to row-relative column `col`. A wrapped row
    /// stops before its last char, since its end is the next row's start.
    pub fn off_at_col(&self, row: usize, col: usize) -> usize {
        let r = self.range(row);
        let max = if self.is_last(row) { r.end } else { r.end - 1 };
        let goal = self.dcols[r.start] + col;
        for k in r.start..max {
            let (cur, next) = (self.dcols[k], self.dcols[k + 1]);
            if next > goal {
                return if goal - cur > (next - cur) / 2 { k + 1 } else { k };
            }
        }
        max
    }
}

/// First visual row of every line, for one wrap width.
#[derive(Default)]
pub struct WrapIndex {
    /// (version, cols, tab, lines, chars) the index was built for.
    key: Option<(u64, usize, usize, usize, usize)>,
    /// `starts[line]` = first row of `line`; one extra entry = total rows.
    starts: Vec<usize>,
}

impl WrapIndex {
    /// Rebuild if the text or wrap settings changed since the last build.
    pub fn update(&mut self, buf: &Buffer, version: u64, cols: usize, tab: usize) {
        let key = (version, cols, tab, buf.len_lines(), buf.len_chars());
        if self.key != Some(key) {
            self.key = Some(key);
            self.starts = build(buf, cols, tab);
        }
    }

    pub fn rows(&self) -> usize {
        self.starts.last().copied().unwrap_or(0).max(1)
    }

    pub fn first_row(&self, line: usize) -> usize {
        self.starts.get(line.min(self.starts.len().saturating_sub(2))).copied().unwrap_or(0)
    }

    /// (line, row within the line) of visual row `row`.
    pub fn line_of_row(&self, row: usize) -> (usize, usize) {
        if self.starts.len() < 2 {
            return (0, 0);
        }
        let row = row.min(self.rows() - 1);
        let line = (self.starts.partition_point(|&s| s <= row) - 1).min(self.starts.len() - 2);
        (line, row - self.starts[line])
    }
}

fn build(buf: &Buffer, cols: usize, tab: usize) -> Vec<usize> {
    let mut starts = Vec::with_capacity(buf.len_lines() + 1);
    let mut row = 0;
    for chunks in buf.line_chunks() {
        starts.push(row);
        let mut w = Wrapper::new(cols, tab);
        row += 1;
        for chunk in chunks {
            for c in chunk.chars() {
                w.push(c, |_| row += 1);
            }
        }
    }
    starts.push(row);
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, cols: usize) -> Vec<String> {
        let lr = LineRows::new(text.chars(), Some(cols), 4);
        let chars: Vec<char> = text.chars().collect();
        (0..lr.rows()).map(|r| chars[lr.range(r)].iter().collect()).collect()
    }

    #[test]
    fn breaks_after_whitespace() {
        assert_eq!(rows("the quick brown fox", 10), ["the quick ", "brown fox"]);
        assert_eq!(rows("aaaa bbbb cccc", 8), ["aaaa ", "bbbb ", "cccc"]);
        // The space that overflows hangs on the row instead of starting the next.
        assert_eq!(rows("aaaa bbbb cccc", 9), ["aaaa bbbb ", "cccc"]);
        assert_eq!(rows("short", 10), ["short"]);
        assert_eq!(rows("", 10), [""]);
    }

    #[test]
    fn hard_breaks_long_words() {
        assert_eq!(rows("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
        assert_eq!(rows("ab abcdefghij", 5), ["ab ", "abcde", "fghij"]);
    }

    #[test]
    fn tabs_and_cjk_are_wide() {
        // A tab at column 0 is 4 wide, so "\tab" fills 6 columns.
        assert_eq!(rows("\tab\tcd", 7), ["\tab\t", "cd"]);
        assert_eq!(rows("你好世界", 5), ["你好", "世界"]);
        assert_eq!(rows("a你好", 4), ["a你", "好"]);
    }

    #[test]
    fn row_offsets_and_columns() {
        let lr = LineRows::new("the quick brown fox".chars(), Some(10), 4);
        assert_eq!(lr.starts, [0, 10]);
        assert_eq!(lr.row_of(9), 0);
        assert_eq!(lr.row_of(10), 1, "a row boundary belongs to the next row");
        assert_eq!(lr.col_in_row(12), 2);
        assert_eq!(lr.off_at_col(1, 2), 12);
        assert_eq!(lr.off_at_col(0, 50), 9, "stays on the wrapped row");
        assert_eq!(lr.off_at_col(1, 50), 19);
        let unwrapped = LineRows::new("a\tb".chars(), None, 4);
        assert_eq!(unwrapped.rows(), 1);
        assert_eq!(unwrapped.dcols, [0, 1, 4, 5]);
    }

    #[test]
    fn index_maps_rows_to_lines() {
        let buf = Buffer::from_str("the quick brown fox\nshort\r\n\nabcdefghijkl");
        let mut idx = WrapIndex::default();
        idx.update(&buf, 0, 10, 4);
        // Rows: 2 + 1 + 1 + 2.
        assert_eq!(idx.rows(), 6);
        assert_eq!(idx.first_row(1), 2);
        assert_eq!(idx.first_row(3), 4);
        assert_eq!(idx.line_of_row(1), (0, 1));
        assert_eq!(idx.line_of_row(3), (2, 0));
        assert_eq!(idx.line_of_row(5), (3, 1));
        assert_eq!(idx.line_of_row(99), (3, 1));
        // Agrees with per-line wrapping.
        for line in 0..buf.len_lines() {
            let lr = LineRows::new(buf.line_chars(line), Some(10), 4);
            let next = if line + 1 < buf.len_lines() { idx.first_row(line + 1) } else { idx.rows() };
            assert_eq!(next - idx.first_row(line), lr.rows(), "line {line}");
        }
    }

    #[test]
    fn index_build_is_fast() {
        let line = "    let value = some_function(argument_one, argument_two) + 42; // comment here\n";
        let text = line.repeat(5_000_000 / line.len());
        let buf = Buffer::from_str(&text);
        let start = std::time::Instant::now();
        let mut idx = WrapIndex::default();
        idx.update(&buf, 0, 40, 4);
        let took = start.elapsed();
        assert!(idx.rows() > 2 * buf.len_lines());
        assert!(took.as_secs_f64() < 1.0, "building took {took:?}");
    }
}
