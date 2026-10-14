//! Search over the buffer: query → regex, match lists, replacements.
//! No UI here; the find bar lives in `app.rs`.

use crate::editor::Editor;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// Stop collecting matches past this; the count shows as "100000+".
pub const MATCH_LIMIT: usize = 100_000;

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Query {
    pub text: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

impl Query {
    pub fn literal(text: &str, whole_word: bool) -> Self {
        Self { text: text.to_owned(), regex: false, case_sensitive: true, whole_word }
    }

    pub fn build(&self) -> Result<Regex, regex::Error> {
        let pattern = if self.regex { any_line_break(&self.text) } else { regex::escape(&self.text) };
        let pattern = if self.whole_word { format!(r"\b(?:{pattern})\b") } else { pattern };
        // CRLF mode: `^`/`$` treat `\r\n` as one line break and `.` doesn't
        // match `\r`, so a pattern behaves the same in CRLF and LF files.
        RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .crlf(true)
            .build()
    }
}

/// `pattern` with each `\n` outside a character class made to match a
/// whole line break, `\r\n` too, so `(\w+)\n` works in CRLF files.
fn any_line_break(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    let mut class = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n') if class == 0 => out.push_str(r"(?:\r?\n)"),
                Some(e) => {
                    out.push('\\');
                    out.push(e);
                }
                None => out.push('\\'),
            },
            '[' => {
                class += 1;
                out.push(c);
                // A `]` (or `^]`) right after `[` is a literal.
                let rest = chars.as_str();
                let lit = if rest.starts_with("^]") { 2 } else { usize::from(rest.starts_with(']')) };
                for _ in 0..lit {
                    out.extend(chars.next());
                }
            }
            ']' if class > 0 => {
                class -= 1;
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Is byte `i` of `text` between the `\r` and `\n` of a line break?
fn inside_crlf(text: &str, i: usize) -> bool {
    let b = text.as_bytes();
    i > 0 && i < b.len() && b[i - 1] == b'\r' && b[i] == b'\n'
}

/// `r` grown to whole `\r\n` pairs, so a match never takes half a line
/// break (`\n` finds the whole `\r\n`; `\s` at a line end takes both).
pub fn whole_line_breaks(text: &str, r: Range<usize>) -> Range<usize> {
    let start = if inside_crlf(text, r.start) { r.start - 1 } else { r.start };
    let end = if inside_crlf(text, r.end) { r.end + 1 } else { r.end };
    start..end
}

/// The byte range a regex replacement should replace: the match grown to
/// whole line breaks, or None when one of its (non-empty) groups holds half
/// a `\r\n` (expanding it would write a lone `\r` or `\n`).
pub fn replaceable(text: &str, caps: &regex::Captures) -> Option<Range<usize>> {
    let m = caps.get(0)?;
    let splits = |g: regex::Match| !g.is_empty() && (inside_crlf(text, g.start()) || inside_crlf(text, g.end()));
    if caps.iter().skip(1).flatten().any(splits) {
        return None;
    }
    Some(whole_line_breaks(text, m.range()))
}

/// The find bar's toggles (and its last text), kept in the session.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Hash, Debug)]
#[serde(default)]
pub struct FindOptions {
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Search and replace only inside the selection captured when this was turned on.
    pub in_selection: bool,
    /// Find Next / Previous continue from the other end of the file.
    pub wrap: bool,
    /// Replacements take the case of the text they replace ("foo"→"bar" makes "Foo" "Bar").
    pub preserve_case: bool,
    /// Outline every match in the file while the bar is open.
    pub highlight_matches: bool,
    pub text: String,
    pub replacement: String,
}

impl Default for FindOptions {
    fn default() -> Self {
        Self {
            regex: false,
            case_sensitive: false,
            whole_word: false,
            in_selection: false,
            wrap: true,
            preserve_case: false,
            highlight_matches: true,
            text: String::new(),
            replacement: String::new(),
        }
    }
}

/// Non-empty matches as char ranges, in document order.
pub fn find_all(re: &Regex, text: &str, limit: usize) -> (Vec<Range<usize>>, bool) {
    find_in(re, text, &[0..text.len()], limit)
}

/// Non-empty matches inside `scope` (sorted, disjoint byte ranges of
/// `text`) as char ranges. A match must fit inside one range; `^`, `$` and
/// `\b` still see the text before a range.
pub fn find_in(re: &Regex, text: &str, scope: &[Range<usize>], limit: usize) -> (Vec<Range<usize>>, bool) {
    let mut out = Vec::new();
    // Matches arrive in order, so byte→char conversion is one running count.
    let (mut byte, mut chr) = (0, 0);
    for m in scope.iter().flat_map(|r| matches_in(re, text, r.clone())) {
        if out.len() == limit {
            return (out, true);
        }
        chr += text[byte..m.start].chars().count();
        let start = chr;
        chr += text[m.clone()].chars().count();
        byte = m.end;
        out.push(start..chr);
    }
    (out, false)
}

/// Non-empty matches (byte ranges) of `re` lying inside `range` of `text`.
pub fn matches_in<'a>(re: &'a Regex, text: &'a str, range: Range<usize>) -> impl Iterator<Item = Range<usize>> + 'a {
    let hay = &text[..range.end];
    let mut at = range.start;
    // End of the last match given out (a grown match may reach past `at`).
    let mut done = range.start;
    std::iter::from_fn(move || {
        while at <= hay.len() {
            let m = re.find_at(hay, at)?;
            if m.start() == m.end() {
                // Step over an empty match, to the next char boundary.
                at = m.end() + hay[m.end()..].chars().next().map_or(1, char::len_utf8);
                continue;
            }
            at = m.end();
            let r = whole_line_breaks(text, m.range());
            if r.start < done || r.end > range.end {
                continue;
            }
            (at, done) = (at.max(r.end), r.end);
            return Some(r);
        }
        None
    })
}

/// Char ranges → sorted, merged, non-empty byte ranges of the editor's text.
pub fn scope_bytes(ed: &Editor, scope: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut sorted: Vec<Range<usize>> = scope.iter().filter(|r| r.start < r.end).cloned().collect();
    sorted.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(sorted.len());
    for r in sorted {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    let len = ed.buffer.len_chars();
    merged.into_iter().map(|r| ed.buffer.char_to_byte(r.start.min(len))..ed.buffer.char_to_byte(r.end.min(len))).collect()
}

/// `replacement` in the case pattern of `matched`, like Sublime's Preserve
/// Case: ALL CAPS stays all caps, lower case lower, and a capitalized word
/// capitalizes the replacement. Words joined by `-` or `_` are matched part
/// for part when both have the same number of parts.
pub fn preserve_case(matched: &str, replacement: &str) -> String {
    for sep in ['-', '_'] {
        let (a, b): (Vec<&str>, Vec<&str>) = (matched.split(sep).collect(), replacement.split(sep).collect());
        if a.len() > 1 && a.len() == b.len() {
            return a.iter().zip(&b).map(|(m, r)| preserve_case_word(m, r)).collect::<Vec<_>>().join(&sep.to_string());
        }
    }
    preserve_case_word(matched, replacement)
}

fn preserve_case_word(matched: &str, replacement: &str) -> String {
    let letters: Vec<char> = matched.chars().filter(|c| c.is_alphabetic()).collect();
    let upper = letters.iter().filter(|c| c.is_uppercase()).count();
    let lower = letters.iter().filter(|c| c.is_lowercase()).count();
    if letters.len() > 1 && upper > 0 && lower == 0 {
        return replacement.to_uppercase();
    }
    if lower > 0 && upper == 0 {
        return replacement.to_lowercase();
    }
    if letters.first().is_some_and(|c| c.is_uppercase()) {
        let mut chars = replacement.chars();
        return match chars.next() {
            Some(f) => f.to_uppercase().chain(chars).collect(),
            None => String::new(),
        };
    }
    replacement.to_owned()
}

/// What a match list was computed for: text version, query, scope.
type FinderKey = (u64, Query, Option<Vec<Range<usize>>>);

/// Cached match list for the find bar; recomputed only when the text, the
/// query or the In Selection scope changes.
#[derive(Default)]
pub struct Finder {
    key: Option<FinderKey>,
    pub matches: Vec<Range<usize>>,
    pub truncated: bool,
    pub error: Option<String>,
}

impl Finder {
    /// Returns true if the match list changed. With a `scope` (char ranges,
    /// Find ▸ In Selection) only matches inside it count.
    pub fn update(&mut self, ed: &mut Editor, q: &Query, scope: Option<&[Range<usize>]>) -> bool {
        let key = (ed.version(), q.clone(), scope.map(<[_]>::to_vec));
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        self.key = Some(key);
        self.matches.clear();
        self.truncated = false;
        self.error = None;
        if q.text.is_empty() {
            return true;
        }
        match q.build() {
            Ok(re) => {
                let text = ed.text_snapshot();
                (self.matches, self.truncated) = match scope {
                    Some(scope) => find_in(&re, &text, &scope_bytes(ed, scope), MATCH_LIMIT),
                    None => find_all(&re, &text, MATCH_LIMIT),
                };
            }
            Err(e) => self.error = Some(short_error(&e)),
        }
        true
    }

    /// Index of the first match starting at or after `pos`, wrapping to the top.
    pub fn next_from(&self, pos: usize) -> Option<usize> {
        self.next(pos, true)
    }

    /// The first match starting at or after `pos`; past the last one, the
    /// first match if `wrap`, else None.
    pub fn next(&self, pos: usize, wrap: bool) -> Option<usize> {
        let i = self.matches.partition_point(|m| m.start < pos);
        if i < self.matches.len() {
            Some(i)
        } else {
            (wrap && !self.matches.is_empty()).then_some(0)
        }
    }

    /// The last match ending at or before `pos`; before the first one, the
    /// last match if `wrap`, else None.
    pub fn prev(&self, pos: usize, wrap: bool) -> Option<usize> {
        match self.matches.partition_point(|m| m.end <= pos) {
            0 => (wrap && !self.matches.is_empty()).then(|| self.matches.len() - 1),
            i => Some(i - 1),
        }
    }

    pub fn index_of(&self, range: &Range<usize>) -> Option<usize> {
        self.matches.binary_search_by_key(&range.start, |m| m.start).ok().filter(|&i| self.matches[i] == *range)
    }
}

fn short_error(e: &regex::Error) -> String {
    // regex errors are multi-line with a caret diagram; keep the last line.
    e.to_string().lines().last().unwrap_or("invalid regex").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_query_escapes_regex_chars() {
        let re = Query::literal("a.b", false).build().unwrap();
        let (m, _) = find_all(&re, "axb a.b", 10);
        assert_eq!(m, vec![4..7]);
    }

    #[test]
    fn a_newline_in_a_pattern_matches_crlf_too() {
        assert_eq!(any_line_break(r"a\nb"), r"a(?:\r?\n)b");
        assert_eq!(any_line_break(r"[\n]\\n\n+"), r"[\n]\\n(?:\r?\n)+", "not in a class, not after an escaped backslash");
        assert_eq!(any_line_break(r"[]\n]\n[^]x]\n"), r"[]\n](?:\r?\n)[^]x](?:\r?\n)");
        let q = Query { text: r"(\w+)\n".into(), regex: true, ..Default::default() };
        assert_eq!(find_all(&q.build().unwrap(), "one\r\ntwo\n", 10).0, vec![0..5, 5..9]);
    }

    #[test]
    fn matches_never_split_a_crlf() {
        let q = |p: &str| Query { text: p.into(), regex: true, ..Default::default() }.build().unwrap();
        assert_eq!(find_all(&q(r"\s+$"), "a  \r\nb", 10).0, vec![1..3]);
        assert_eq!(find_all(&q("^.*$"), "ab\r\ncd", 10).0, vec![0..2, 4..6], "`.` and `$` stop before \\r");
        assert_eq!(find_all(&q(r"\r"), "a\r\nb", 10).0, vec![1..3], "grown to the whole break");
        assert_eq!(find_all(&q(r"\r|\n"), "a\r\nb", 10).0, vec![1..3], "once, not twice");
    }

    #[test]
    fn whole_word_and_case() {
        let q = Query { text: "foo".into(), whole_word: true, ..Default::default() };
        let (m, _) = find_all(&q.build().unwrap(), "foo food FOO", 10);
        assert_eq!(m, vec![0..3, 9..12]);
    }

    #[test]
    fn char_ranges_with_multibyte_text() {
        let re = Query::literal("x", false).build().unwrap();
        let (m, _) = find_all(&re, "é x 😀 x", 10);
        assert_eq!(m, vec![2..3, 6..7]);
    }

    #[test]
    fn limit_reports_truncation() {
        let re = Query::literal("a", false).build().unwrap();
        let (m, truncated) = find_all(&re, "aaaa", 3);
        assert_eq!(m.len(), 3);
        assert!(truncated);
    }

    #[test]
    fn next_and_prev_wrap() {
        let f = Finder { matches: vec![2..3, 6..7], ..Default::default() };
        assert_eq!(f.next_from(4), Some(1));
        assert_eq!(f.next_from(7), Some(0));
        assert_eq!(f.prev(6, true), Some(0));
        assert_eq!(f.prev(1, true), Some(1));
    }

    #[test]
    fn without_wrap_stepping_stops_at_the_ends() {
        let f = Finder { matches: vec![2..3, 6..7], ..Default::default() };
        assert_eq!(f.next(4, false), Some(1));
        assert_eq!(f.next(7, false), None);
        assert_eq!(f.prev(6, false), Some(0));
        assert_eq!(f.prev(1, false), None);
        assert_eq!(Finder::default().next(0, true), None);
        assert_eq!(Finder::default().prev(0, true), None);
    }

    #[test]
    fn scoped_search_keeps_matches_inside_the_ranges() {
        let re = Query::literal("ab", false).build().unwrap();
        let text = "ab ab ab ab é ab";
        // Bytes 3..7 hold the 2nd "ab" and half the 3rd; 15.. is after "é ".
        let (m, _) = find_in(&re, text, &[3..7, 15..text.len()], 10);
        assert_eq!(m, vec![3..5, 14..16], "char ranges; a match must fit in a range");
        // An empty-matching regex doesn't loop.
        let re = Query { text: "x*".into(), regex: true, ..Default::default() }.build().unwrap();
        assert_eq!(find_in(&re, "axxb", &[0..4], 10).0, vec![1..3]);
    }

    #[test]
    fn scope_ranges_are_sorted_merged_and_converted_to_bytes() {
        let ed = Editor::new(crate::buffer::Buffer::from_str("é bc def"));
        assert_eq!(scope_bytes(&ed, &[5..8, 0..2, 1..3, 4..4]), vec![0..4, 6..9]);
    }

    #[test]
    fn finder_honors_a_scope_and_recomputes_when_it_changes() {
        let mut ed = Editor::new(crate::buffer::Buffer::from_str("foo foo\nfoo foo\n"));
        let q = Query::literal("foo", false);
        let mut f = Finder::default();
        f.update(&mut ed, &q, None);
        assert_eq!(f.matches.len(), 4);
        assert!(f.update(&mut ed, &q, Some(&[4..11])));
        assert_eq!(f.matches, vec![4..7, 8..11]);
        assert!(!f.update(&mut ed, &q, Some(&[4..11])), "cached");
        f.update(&mut ed, &q, Some(&[]));
        assert!(f.matches.is_empty(), "an empty selection finds nothing");
    }

    #[test]
    fn preserve_case_follows_the_matched_text() {
        assert_eq!(preserve_case("foo", "bar"), "bar");
        assert_eq!(preserve_case("Foo", "bar"), "Bar");
        assert_eq!(preserve_case("FOO", "bar"), "BAR");
        assert_eq!(preserve_case("foo", "BAR"), "bar");
        assert_eq!(preserve_case("fOO", "bar"), "bar", "mixed case: as typed");
        assert_eq!(preserve_case("F", "bar"), "Bar", "one capital letter capitalizes");
        assert_eq!(preserve_case("123", "bar"), "bar", "no letters: as typed");
        assert_eq!(preserve_case("Foo-BAR", "baz-qux"), "Baz-QUX", "part for part");
        assert_eq!(preserve_case("FOO_BAR", "new_name"), "NEW_NAME");
        assert_eq!(preserve_case("Über", "straße"), "Straße");
    }
}
