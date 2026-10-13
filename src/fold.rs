//! Code folding: where fold regions are, and which ones a view has folded.
//!
//! A region is a header line plus the lines `header + 1..=end` that fold
//! away under it. Regions come from indentation: the lines after a header that
//! are indented deeper than it (blank lines inside count, trailing blank lines
//! don't), so a closing bracket back at the header's indent stays visible.
//! Flat code falls back to brackets: a header ending in `{`, `[` or `(` with
//! nothing indented under it folds down to the line that starts with its
//! closer. Brackets are matched per line (last char opens, first char closes),
//! ignoring strings and comments.
//!
//! Folds are stored as line ranges and follow edits by line: given the first
//! changed line and the change in line count, folds after the edit shift
//! (confirmed by a fingerprint of the header line), folds the edit touched are
//! recomputed from the text, and anything that no longer matches is dropped.

use crate::buffer::Buffer;
use std::hash::{DefaultHasher, Hasher};

/// How far the bracket fallback looks for a closer.
const BRACKET_SCAN: usize = 20_000;
/// Blank lines skipped when checking whether a line starts a region.
const BLANK_SCAN: usize = 1_000;

/// Indent and outermost non-blank chars of one line.
#[derive(Clone, Copy)]
struct Info {
    indent: usize,
    first: char,
    last: char,
    blank: bool,
    /// Bytes, line break excluded.
    len: usize,
}

fn info<'a>(chunks: impl Iterator<Item = &'a str>, tab: usize) -> Info {
    // Plain `str` methods, not a char loop: this runs on every line for Fold All.
    let tab = tab.max(1);
    let (mut indent, mut first, mut last, mut len) = (0, None, ' ', 0);
    for chunk in chunks {
        len += chunk.trim_end_matches(['\n', '\r']).len();
        let body = if first.is_none() {
            let body = chunk.trim_start();
            for b in chunk[..chunk.len() - body.len()].bytes() {
                match b {
                    b' ' => indent += 1,
                    b'\t' => indent += tab - indent % tab,
                    _ => {}
                }
            }
            first = body.chars().next();
            body
        } else {
            chunk
        };
        if let Some(c) = body.trim_end().chars().next_back() {
            last = c;
        }
    }
    Info { indent, first: first.unwrap_or(' '), last, blank: first.is_none(), len }
}

fn closer(open: char) -> Option<char> {
    match open {
        '{' => Some('}'),
        '[' => Some(']'),
        '(' => Some(')'),
        _ => None,
    }
}

fn is_closer(c: char) -> bool {
    matches!(c, '}' | ']' | ')')
}

/// End of the region whose header is `header`, if it has one.
pub fn region_at(buf: &Buffer, header: usize, tab: usize) -> Option<usize> {
    let mut lines = buf.lines_from(header);
    let head = info(lines.next()?, tab);
    if head.blank {
        return None;
    }
    let mut end = None;
    for (j, l) in lines.enumerate() {
        let li = info(l, tab);
        if li.blank {
            continue;
        }
        if li.indent <= head.indent {
            break;
        }
        end = Some(header + 1 + j);
    }
    end.or_else(|| bracket_end(buf, header, head, tab))
}

/// Cheap check for a region at `line` (for gutter arrows).
pub fn has_region(buf: &Buffer, line: usize, tab: usize) -> bool {
    let mut lines = buf.lines_from(line);
    let Some(head) = lines.next().map(|l| info(l, tab)) else { return false };
    if head.blank {
        return false;
    }
    if let Some(next) = lines.take(BLANK_SCAN).map(|l| info(l, tab)).find(|i| !i.blank)
        && next.indent > head.indent
    {
        return true;
    }
    bracket_end(buf, line, head, tab).is_some()
}

/// Bracket fallback: the line before the one that closes `head`'s last char.
fn bracket_end(buf: &Buffer, header: usize, head: Info, tab: usize) -> Option<usize> {
    let close = closer(head.last)?;
    let mut stack: Vec<char> = Vec::new();
    for (j, l) in buf.lines_from(header + 1).take(BRACKET_SCAN).enumerate() {
        let li = info(l, tab);
        if li.blank {
            continue;
        }
        if is_closer(li.first) {
            match stack.last() {
                Some(&want) if want == li.first => {
                    stack.pop();
                }
                None if li.first == close => return (j >= 1).then_some(header + j),
                _ => return None,
            }
        }
        if let Some(c) = closer(li.last) {
            stack.push(c);
        }
    }
    None
}

/// The innermost indentation region holding `line` below its header.
pub fn enclosing(buf: &Buffer, line: usize, tab: usize) -> Option<(usize, usize)> {
    let mut at = line;
    let mut cur = info(buf.lines_from(line).next()?, tab);
    if cur.blank {
        let (k, prev) = buf.lines_before(line).map(|l| info(l, tab)).enumerate().find(|(_, i)| !i.blank)?;
        at = line - 1 - k;
        cur = prev;
        if let Some(end) = region_at(buf, at, tab).filter(|&e| e >= line) {
            return Some((at, end));
        }
    }
    // Walk out through less-indented headers until one's region reaches `line`.
    while cur.indent > 0 {
        let (k, head) = buf
            .lines_before(at)
            .map(|l| info(l, tab))
            .enumerate()
            .find(|(_, i)| !i.blank && i.indent < cur.indent)?;
        at -= 1 + k;
        cur = head;
        if let Some(end) = region_at(buf, at, tab).filter(|&e| e >= line) {
            return Some((at, end));
        }
    }
    None
}

/// Headers of the indentation scopes holding `line`, innermost first: each is
/// the nearest non-blank line above that is indented less than the scope
/// below it. A blank `line` takes the indent of the next non-blank line.
/// Looks back at most `limit` lines, so it stays cheap in huge blocks.
pub fn scope_headers(buf: &Buffer, line: usize, tab: usize, limit: usize) -> Vec<usize> {
    let Some(mut cur) = buf.lines_from(line).take(BLANK_SCAN).map(|l| info(l, tab)).find(|i| !i.blank).map(|i| i.indent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, l) in buf.lines_before(line).take(limit).enumerate() {
        if cur == 0 {
            break;
        }
        let li = info(l, tab);
        if !li.blank && li.indent < cur {
            out.push(line - 1 - k);
            cur = li.indent;
        }
    }
    out
}

/// A fold region and how many regions enclose it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub header: usize,
    pub end: usize,
    pub depth: usize,
    key: u64,
}

/// Every region in the buffer, in one pass.
pub fn all_regions(buf: &Buffer, tab: usize) -> Vec<Region> {
    let infos: Vec<Info> = buf.line_chunks().map(|l| info(l, tab)).collect();
    // Indentation headers: the next non-blank line is indented deeper.
    let mut header = vec![false; infos.len()];
    let mut next_indent = None;
    for (i, li) in infos.iter().enumerate().rev() {
        if !li.blank {
            header[i] = next_indent.is_some_and(|k| k > li.indent);
            next_indent = Some(li.indent);
        }
    }
    let mut out = Vec::new();
    // Open indentation headers: (line, indent).
    let mut open: Vec<(usize, usize)> = Vec::new();
    // Open brackets: (line, closer, depth, folds by bracket).
    let mut brackets: Vec<(usize, char, usize, bool)> = Vec::new();
    let mut last_text = 0;
    for (i, li) in infos.iter().enumerate() {
        if li.blank {
            continue;
        }
        while let Some(&(h, indent)) = open.last()
            && indent >= li.indent
        {
            open.pop();
            out.push(Region { header: h, end: last_text, depth: open.len(), key: infos[h].key() });
        }
        if is_closer(li.first)
            && let Some(&(h, want, depth, own)) = brackets.last()
            && want == li.first
        {
            brackets.pop();
            if own && i >= h + 2 {
                out.push(Region { header: h, end: i - 1, depth, key: infos[h].key() });
            }
        }
        if let Some(c) = closer(li.last) {
            brackets.push((i, c, open.len(), !header[i]));
        }
        if header[i] {
            open.push((i, li.indent));
        }
        last_text = i;
    }
    while let Some((h, _)) = open.pop() {
        out.push(Region { header: h, end: last_text, depth: open.len(), key: infos[h].key() });
    }
    out.sort_by_key(|r| r.header);
    out
}

/// A folded region: `header + 1..=end` are hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    pub header: usize,
    pub end: usize,
    key: u64,
}

/// One view's folds: sorted, disjoint, never hiding another fold's header.
#[derive(Default)]
pub struct Folds {
    list: Vec<Fold>,
    /// Text version and line count the folds were placed against.
    version: Option<u64>,
    lines: usize,
    /// Bumped whenever the set changes.
    pub generation: u64,
}

impl Info {
    /// Cheap fingerprint of a line, to recognize a fold's header after edits.
    fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        h.write_usize(self.len);
        h.write_u32(self.first as u32);
        h.write_u32(self.last as u32);
        h.finish()
    }
}

fn line_key(buf: &Buffer, line: usize) -> u64 {
    buf.lines_from(line).next().map_or(0, |l| info(l, 1).key())
}

impl Folds {
    pub fn list(&self) -> &[Fold] {
        &self.list
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn clear(&mut self) {
        if !self.list.is_empty() {
            self.list.clear();
            self.generation += 1;
        }
    }

    /// Fold each `(header, end)`; outer folds swallow inner ones.
    pub fn add(&mut self, buf: &Buffer, ranges: impl IntoIterator<Item = (usize, usize)>) {
        let folds = ranges.into_iter().map(|(header, end)| Fold { header, end, key: line_key(buf, header) });
        self.insert(buf, folds);
    }

    /// Fold `regions` (from `all_regions` on the current text).
    pub fn add_regions(&mut self, buf: &Buffer, regions: impl IntoIterator<Item = Region>) {
        self.insert(buf, regions.into_iter().map(|r| Fold { header: r.header, end: r.end, key: r.key }));
    }

    fn insert(&mut self, buf: &Buffer, folds: impl Iterator<Item = Fold>) {
        let before = self.list.len();
        self.list.extend(folds);
        if self.list.len() != before {
            self.normalize(buf.len_lines());
            self.generation += 1;
        }
    }

    /// Unfold every fold that `keep` rejects.
    pub fn retain(&mut self, keep: impl FnMut(&Fold) -> bool) {
        let before = self.list.len();
        self.list.retain(keep);
        if self.list.len() != before {
            self.generation += 1;
        }
    }

    /// Index of the fold whose header is `line`.
    pub fn at_header(&self, line: usize) -> Option<usize> {
        self.list.binary_search_by_key(&line, |f| f.header).ok()
    }

    /// Index of the fold hiding `line`.
    pub fn hiding(&self, line: usize) -> Option<usize> {
        let i = self.list.partition_point(|f| f.end < line);
        self.list.get(i).is_some_and(|f| f.header < line).then_some(i)
    }

    /// Follow an edit: `changed_from` is the first line it touched.
    pub fn sync(&mut self, buf: &Buffer, version: u64, changed_from: Option<usize>, tab: usize) {
        if self.version == Some(version) {
            return;
        }
        let lines = buf.len_lines();
        let delta = lines as isize - self.lines as isize;
        (self.version, self.lines) = (Some(version), lines);
        if self.list.is_empty() {
            return;
        }
        let from = changed_from.unwrap_or(0);
        let zone_end = from + delta.max(0) as usize;
        let recompute = |header: usize, key: u64| region_at(buf, header, tab).map(|end| Fold { header, end, key });
        let old = std::mem::take(&mut self.list);
        for &f in &old {
            let kept = if f.end < from {
                Some(f)
            } else if f.header < from {
                recompute(f.header, f.key)
            } else {
                let shifted = f.header.checked_add_signed(delta).map(|h| (h, delta));
                let found = [shifted, Some((f.header, 0))]
                    .into_iter()
                    .flatten()
                    .find(|&(h, _)| h < lines && line_key(buf, h) == f.key);
                match found {
                    Some((h, _)) if h <= zone_end => recompute(h, f.key),
                    Some((h, d)) => f.end.checked_add_signed(d).map(|end| Fold { header: h, end, key: f.key }),
                    None if f.header == from && from < lines => recompute(from, line_key(buf, from)),
                    None => None,
                }
            };
            self.list.extend(kept);
        }
        self.normalize(lines);
        if self.list != old {
            self.generation += 1;
        }
    }

    /// Sort, drop empty or out-of-range folds, and let outer folds win.
    fn normalize(&mut self, lines: usize) {
        self.list.retain(|f| f.end > f.header && f.end < lines);
        self.list.sort_by_key(|f| (f.header, std::cmp::Reverse(f.end)));
        let mut last_end = None;
        self.list.retain(|f| {
            let keep = last_end.is_none_or(|e| f.header > e);
            if keep {
                last_end = Some(f.end);
            }
            keep
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST: &str = "fn main() {\n    let x = 1;\n\n    if x > 0 {\n        go();\n    }\n}\n\nfn other() {}\n";
    const PY: &str = "class A:\n    def f(self):\n        return 1\n\n    def g(self):\n        pass\n\nx = 1\n";

    #[test]
    fn indentation_regions_keep_the_closer_visible() {
        let buf = Buffer::from_str(RUST);
        assert_eq!(region_at(&buf, 0, 4), Some(5), "the blank line inside belongs; `}}` stays");
        assert_eq!(region_at(&buf, 3, 4), Some(4));
        assert_eq!(region_at(&buf, 1, 4), None);
        assert_eq!(region_at(&buf, 8, 4), None);
        let buf = Buffer::from_str(PY);
        assert_eq!(region_at(&buf, 0, 4), Some(5), "the trailing blank line stays out");
        assert_eq!(region_at(&buf, 1, 4), Some(2));
        assert!(has_region(&buf, 4, 4) && !has_region(&buf, 5, 4));
    }

    #[test]
    fn flat_brackets_fold_to_their_closer() {
        let buf = Buffer::from_str("x = {\na: 1,\nb: [\n2\n],\n}\ny\n");
        assert_eq!(region_at(&buf, 0, 4), Some(4));
        assert_eq!(region_at(&buf, 2, 4), Some(3));
        assert_eq!(region_at(&buf, 6, 4), None);
        let regions = all_regions(&buf, 4);
        assert_eq!(regions.iter().map(|r| (r.header, r.end)).collect::<Vec<_>>(), [(0, 4), (2, 3)]);
        // An empty pair has nothing to fold.
        assert_eq!(region_at(&Buffer::from_str("a {\n}\n"), 0, 4), None);
    }

    #[test]
    fn enclosing_region() {
        let buf = Buffer::from_str(PY);
        assert_eq!(enclosing(&buf, 2, 4), Some((1, 2)));
        assert_eq!(enclosing(&buf, 1, 4), Some((0, 5)));
        assert_eq!(enclosing(&buf, 3, 4), Some((0, 5)), "a blank line inside the class");
        assert_eq!(enclosing(&buf, 7, 4), None);
    }

    #[test]
    fn scope_headers_walk_out() {
        let buf = Buffer::from_str(PY);
        assert_eq!(scope_headers(&buf, 2, 4, 100), [1, 0]);
        assert_eq!(scope_headers(&buf, 3, 4, 100), [0], "a blank line takes the next line's indent");
        assert_eq!(scope_headers(&buf, 7, 4, 100), Vec::<usize>::new());
        let buf = Buffer::from_str(RUST);
        assert_eq!(scope_headers(&buf, 4, 4, 100), [3, 0]);
        assert_eq!(scope_headers(&buf, 5, 4, 100), [0], "the inner closer is back in `main`");
        assert_eq!(scope_headers(&buf, 6, 4, 100), Vec::<usize>::new());
        assert_eq!(scope_headers(&buf, 4, 4, 1), [3], "the scan stops at the limit");
    }

    #[test]
    fn all_regions_have_depths() {
        let buf = Buffer::from_str(PY);
        let r: Vec<_> = all_regions(&buf, 4).iter().map(|r| (r.header, r.end, r.depth)).collect();
        assert_eq!(r, [(0, 5, 0), (1, 2, 1), (4, 5, 1)]);
        let buf = Buffer::from_str(RUST);
        let r: Vec<_> = all_regions(&buf, 4).iter().map(|r| (r.header, r.end, r.depth)).collect();
        assert_eq!(r, [(0, 5, 0), (3, 4, 1)]);
        for r in all_regions(&buf, 4) {
            assert_eq!(region_at(&buf, r.header, 4), Some(r.end), "agrees with the lazy scan");
        }
    }

    #[test]
    fn folds_follow_edits() {
        let mut buf = Buffer::from_str(PY);
        let mut folds = Folds::default();
        folds.sync(&buf, 0, None, 4);
        folds.add(&buf, [(4, 5), (1, 2)]);
        assert_eq!(folds.list().iter().map(|f| (f.header, f.end)).collect::<Vec<_>>(), [(1, 2), (4, 5)]);
        // Two lines inserted above everything.
        buf.insert(0, "import os\n\n");
        folds.sync(&buf, 1, Some(0), 4);
        assert_eq!(folds.list().iter().map(|f| (f.header, f.end)).collect::<Vec<_>>(), [(3, 4), (6, 7)]);
        assert_eq!(folds.hiding(4), Some(0));
        assert_eq!(folds.hiding(3), None);
        assert_eq!(folds.at_header(6), Some(1));
        // Renaming a header keeps its fold.
        let at = buf.line_start(6) + 8;
        buf.insert(at, "x");
        folds.sync(&buf, 2, Some(6), 4);
        assert_eq!(folds.list().len(), 2);
        // Deleting a folded block's body drops its fold.
        buf.remove(buf.line_start(3)..buf.line_start(5));
        folds.sync(&buf, 3, Some(3), 4);
        assert_eq!(folds.list().iter().map(|f| (f.header, f.end)).collect::<Vec<_>>(), [(4, 5)]);
        // Outer folds swallow inner ones.
        folds.add(&buf, [(2, 5)]);
        assert_eq!(folds.list().iter().map(|f| (f.header, f.end)).collect::<Vec<_>>(), [(2, 5)]);
    }
}
