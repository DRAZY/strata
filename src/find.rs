//! Search over the buffer: query → regex, match lists, replacements.
//! No UI here; the find bar lives in `app.rs`.

use crate::editor::Editor;
use regex::{Regex, RegexBuilder};
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
        let pattern = if self.regex { self.text.clone() } else { regex::escape(&self.text) };
        let pattern = if self.whole_word { format!(r"\b(?:{pattern})\b") } else { pattern };
        RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .build()
    }
}

/// Non-empty matches as char ranges, in document order.
pub fn find_all(re: &Regex, text: &str, limit: usize) -> (Vec<Range<usize>>, bool) {
    let mut out = Vec::new();
    // Matches arrive in order, so byte→char conversion is one running count.
    let (mut byte, mut chr) = (0, 0);
    for m in re.find_iter(text) {
        if m.start() == m.end() {
            continue;
        }
        if out.len() == limit {
            return (out, true);
        }
        chr += text[byte..m.start()].chars().count();
        let start = chr;
        chr += text[m.start()..m.end()].chars().count();
        byte = m.end();
        out.push(start..chr);
    }
    (out, false)
}

/// Cached match list for the find bar; recomputed only when the text or query changes.
#[derive(Default)]
pub struct Finder {
    key: Option<(u64, Query)>,
    pub matches: Vec<Range<usize>>,
    pub truncated: bool,
    pub error: Option<String>,
}

impl Finder {
    /// Returns true if the match list changed.
    pub fn update(&mut self, ed: &mut Editor, q: &Query) -> bool {
        let key = (ed.version(), q.clone());
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
                (self.matches, self.truncated) = find_all(&re, &text, MATCH_LIMIT);
            }
            Err(e) => self.error = Some(short_error(&e)),
        }
        true
    }

    /// Index of the first match starting at or after `pos`, wrapping to the top.
    pub fn next_from(&self, pos: usize) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let i = self.matches.partition_point(|m| m.start < pos);
        Some(if i == self.matches.len() { 0 } else { i })
    }

    /// Index of the last match ending at or before `pos`, wrapping to the bottom.
    pub fn prev_before(&self, pos: usize) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let i = self.matches.partition_point(|m| m.end <= pos);
        Some(if i == 0 { self.matches.len() - 1 } else { i - 1 })
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
        assert_eq!(f.prev_before(6), Some(0));
        assert_eq!(f.prev_before(1), Some(1));
    }
}
