//! Soft wrap: where each line breaks into visual rows, and an index of the
//! first visual row of every line so rows map to lines by binary search.
//!
//! Widths are monospace columns: a tab runs to the next tab stop, CJK takes
//! two columns, a shown hidden character its box's (see `hidden::cols`),
//! anything else one. Rows break after the last whitespace that
//! fits; a word wider than a row is cut where the row is full.

use crate::buffer::Buffer;
use crate::fonts::is_cjk;
use crate::hidden;
use std::ops::Range;

/// Wrapping scans the whole text on every edit, so huge buffers stay unwrapped.
pub const MAX_CHARS: usize = 8 << 20;

/// Display columns before the next char, after `c` at column `col`. With
/// `hidden`, a hidden character takes its box's columns (like a tab, so
/// the galley, the caret and clicks all agree on where things are).
pub fn advance(c: char, col: usize, tab: usize, hidden: bool) -> usize {
    match c {
        '\t' => col + tab - col % tab,
        _ if hidden && hidden::is_hidden(c) => col + hidden::cols(c),
        _ => col + 1,
    }
}

/// Feeds one line's chars and reports where rows break.
struct Wrapper {
    cols: usize,
    tab: usize,
    hidden: bool,
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
    fn new(cols: usize, tab: usize, hidden: bool) -> Self {
        Self { cols: cols.max(1), tab: tab.max(1), hidden, i: 0, col: 0, width: 0, row_start: 0, space: None }
    }

    /// Calls `brk` with the char offset of each row that starts before `c` ends.
    fn push(&mut self, c: char, mut brk: impl FnMut(usize)) {
        let i = self.i;
        self.i += 1;
        if c == '\n' || c == '\r' {
            return;
        }
        let adv = advance(c, self.col, self.tab, self.hidden) - self.col;
        let w = if adv == 1 && is_cjk(c) { 2 } else { adv };
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
    /// `cols: None` is a single row (wrap off). `hidden`: hidden characters
    /// are shown as boxes (see `advance`).
    pub fn new(chars: impl Iterator<Item = char>, cols: Option<usize>, tab: usize, hidden: bool) -> Self {
        let tab = tab.max(1);
        let mut starts = vec![0];
        let mut dcols = vec![0];
        let mut wrapper = cols.map(|c| Wrapper::new(c, tab, hidden));
        let mut col = 0;
        for c in chars {
            if let Some(w) = &mut wrapper {
                w.push(c, |at| starts.push(at));
            }
            col = advance(c, col, tab, hidden);
            dcols.push(col);
        }
        Self { starts, dcols }
    }

    /// One unwrapped row of part of a line that starts at display column
    /// `col` (so tabs land on the line's tab stops).
    pub fn starting_at(chars: impl Iterator<Item = char>, col: usize, tab: usize, hidden: bool) -> Self {
        let tab = tab.max(1);
        let mut dcols = vec![col];
        let mut col = col;
        for c in chars {
            col = advance(c, col, tab, hidden);
            dcols.push(col);
        }
        Self { starts: vec![0], dcols }
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
    /// (version, cols, tab, hidden shown, lines, chars) the index was built for.
    key: Option<(u64, usize, usize, bool, usize, usize)>,
    /// `starts[line]` = first row of `line`; one extra entry = total rows.
    starts: Vec<usize>,
}

impl WrapIndex {
    /// Rebuild if the text or wrap settings changed since the last build.
    pub fn update(&mut self, buf: &Buffer, version: u64, cols: usize, tab: usize, hidden: bool) {
        let key = (version, cols, tab, hidden, buf.len_lines(), buf.len_chars());
        if self.key != Some(key) {
            self.key = Some(key);
            self.starts = build(buf, cols, tab, hidden);
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

fn build(buf: &Buffer, cols: usize, tab: usize, hidden: bool) -> Vec<usize> {
    let mut starts = Vec::with_capacity(buf.len_lines() + 1);
    let mut row = 0;
    for chunks in buf.line_chunks() {
        starts.push(row);
        let mut w = Wrapper::new(cols, tab, hidden);
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
        let lr = LineRows::new(text.chars(), Some(cols), 4, true);
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
    fn hidden_characters_take_their_boxes_columns() {
        // RLO's box is 3 columns: "a" + 3 + "bc" fills 6.
        assert_eq!(rows("a\u{202E}bc def", 6), ["a\u{202E}bc ", "def"]);
        let lr = LineRows::new("a\u{202E}b\tc".chars(), None, 4, true);
        assert_eq!(lr.dcols, [0, 1, 4, 5, 8, 9]);
        // Shown or not, it stays one char: a click past it lands after it.
        assert_eq!(lr.off_at_col(0, 3), 2);
        assert_eq!(lr.off_at_col(0, 2), 1);
        let plain = LineRows::new("a\u{202E}b".chars(), None, 4, false);
        assert_eq!(plain.dcols, [0, 1, 2, 3]);
    }

    #[test]
    fn row_offsets_and_columns() {
        let lr = LineRows::new("the quick brown fox".chars(), Some(10), 4, true);
        assert_eq!(lr.starts, [0, 10]);
        assert_eq!(lr.row_of(9), 0);
        assert_eq!(lr.row_of(10), 1, "a row boundary belongs to the next row");
        assert_eq!(lr.col_in_row(12), 2);
        assert_eq!(lr.off_at_col(1, 2), 12);
        assert_eq!(lr.off_at_col(0, 50), 9, "stays on the wrapped row");
        assert_eq!(lr.off_at_col(1, 50), 19);
        let unwrapped = LineRows::new("a\tb".chars(), None, 4, true);
        assert_eq!(unwrapped.rows(), 1);
        assert_eq!(unwrapped.dcols, [0, 1, 4, 5]);
    }

    #[test]
    fn index_maps_rows_to_lines() {
        let buf = Buffer::from_str("the quick brown fox\nshort\r\n\nabcdefghijkl");
        let mut idx = WrapIndex::default();
        idx.update(&buf, 0, 10, 4, true);
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
            let lr = LineRows::new(buf.line_chars(line), Some(10), 4, true);
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
        idx.update(&buf, 0, 40, 4, true);
        let took = start.elapsed();
        assert!(idx.rows() > 2 * buf.len_lines());
        assert!(took.as_secs_f64() < 1.0, "building took {took:?}");
    }
}
