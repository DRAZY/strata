//! Fuzzy matching for Goto Anything and the command palette.
//!
//! A query matches when its chars appear in order in the candidate
//! (case-insensitively). Among all such alignments a small DP picks the
//! best-scoring one, rewarding runs and word starts and charging for gaps.
//! Space-separated query terms must each match, in any order; when the
//! candidate holds the words themselves in the order typed, they are also
//! scored as one phrase, which ranks it higher ("rename note" prefers
//! "Rename Note" to "Note … Rename").

use std::cmp::Reverse;

const MATCH: i32 = 16;
const BONUS_START: i32 = 12;
/// Right after `/` or `\`.
const BONUS_PATH: i32 = 12;
/// After `_ - . space` and other punctuation.
const BONUS_SEP: i32 = 9;
const BONUS_CAMEL: i32 = 8;
const BONUS_DIGIT: i32 = 4;
/// Per matched char in the file-name part (after the last separator).
const BONUS_FILENAME: i32 = 4;
const BONUS_CONSECUTIVE: i32 = 6;
const BONUS_EXACT_CASE: i32 = 1;
const GAP_OPEN: i32 = 5;
const GAP_EXTEND: i32 = 1;
/// One point off per this many candidate chars.
const LENGTH_DIVISOR: i32 = 16;
const NEG: i32 = i32::MIN / 4;

/// Score `candidate` against `query`: the score (higher is better) and the
/// matched char indices in `candidate`, ascending. `None` if it doesn't match.
#[cfg(test)]
pub fn score(query: &str, candidate: &str) -> Option<(i32, Vec<usize>)> {
    let pattern = Pattern::new(query);
    let mut scratch = Scratch::default();
    let mut positions = Vec::new();
    let s = pattern.score(candidate, &mut scratch, Some(&mut positions))?;
    Some((s, positions))
}

/// The best `limit` items for `query`, best first, as (index, score, matched
/// char indices). Ties go to the shorter candidate, then the earlier item.
/// An empty query returns the first `limit` items in order with score 0.
pub fn rank<T>(query: &str, items: &[T], key: impl Fn(&T) -> &str, limit: usize) -> Vec<(usize, i32, Vec<usize>)> {
    let pattern = Pattern::new(query);
    if pattern.terms.is_empty() {
        return (0..items.len().min(limit)).map(|i| (i, 0, Vec::new())).collect();
    }
    let mut scratch = Scratch::default();
    // Score-only pass over everything; positions only for the survivors.
    let mut hits: Vec<(Reverse<i32>, usize, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let text = key(item);
            pattern.score(text, &mut scratch, None).map(|s| (Reverse(s), text.len(), i))
        })
        .collect();
    if limit == 0 {
        return Vec::new();
    }
    if hits.len() > limit {
        hits.select_nth_unstable(limit - 1);
        hits.truncate(limit);
    }
    hits.sort_unstable();
    hits.into_iter()
        .map(|(Reverse(s), _, i)| {
            let mut positions = Vec::new();
            pattern.score(key(&items[i]), &mut scratch, Some(&mut positions));
            (i, s, positions)
        })
        .collect()
}

struct Term {
    chars: Vec<char>,
    folded: Vec<char>,
}

struct Pattern {
    terms: Vec<Term>,
    /// The terms joined by single spaces, when there are several.
    phrase: Option<Term>,
}

/// Buffers reused across candidates so scoring doesn't allocate per item.
#[derive(Default)]
struct Scratch {
    chars: Vec<char>,
    folded: Vec<char>,
    bonus: Vec<i32>,
    table: Vec<i32>,
    term_positions: Vec<usize>,
    /// Phrase alignments run (tests).
    #[cfg(test)]
    phrase_runs: usize,
}

fn fold(c: char) -> char {
    if c.is_ascii() { c.to_ascii_lowercase() } else { c.to_lowercase().next().unwrap_or(c) }
}

fn is_path_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

impl Pattern {
    fn new(query: &str) -> Self {
        let term = |t: &str| Term { chars: t.chars().collect(), folded: t.chars().map(fold).collect() };
        let words: Vec<&str> = query.split_whitespace().collect();
        let phrase = (words.len() > 1).then(|| term(&words.join(" ")));
        Self { terms: words.into_iter().map(term).collect(), phrase }
    }

    /// Does `folded` contain each term whole, one after another, as typed?
    fn words_in_order(&self, folded: &[char]) -> bool {
        let mut at = 0;
        self.terms.iter().all(|t| match folded[at..].windows(t.folded.len()).position(|w| w == t.folded.as_slice()) {
            Some(i) => {
                at += i + t.folded.len();
                true
            }
            None => false,
        })
    }

    fn score(&self, candidate: &str, s: &mut Scratch, mut positions: Option<&mut Vec<usize>>) -> Option<i32> {
        // Cheap subsequence check first; most candidates fail here.
        if !self.terms.iter().all(|t| is_subsequence(&t.folded, candidate)) {
            return None;
        }
        s.chars.clear();
        s.chars.extend(candidate.chars());
        s.folded.clear();
        s.folded.extend(s.chars.iter().map(|&c| fold(c)));
        compute_bonus(&s.chars, &mut s.bonus);
        if let Some(p) = positions.as_deref_mut() {
            p.clear();
        }
        let want = positions.is_some();
        let mut total = 0;
        for term in &self.terms {
            total += match_term(term, s, want)?;
            if let Some(p) = positions.as_deref_mut() {
                p.extend_from_slice(&s.term_positions);
            }
        }
        // The whole query in order: its spaces must match whitespace too, so
        // words that follow each other in the candidate earn the run bonuses.
        // Only tried when the candidate holds the words themselves in this
        // order: a second alignment for every candidate would slow big folders.
        if let Some(phrase) = self.phrase.as_ref().filter(|_| self.words_in_order(&s.folded))
            && let Some(best) = match_phrase(phrase, s, want).filter(|&b| b > total)
        {
            total = best;
            if let Some(p) = positions.as_deref_mut() {
                p.clear();
                // The spaces between the words aren't highlighted.
                p.extend(s.term_positions.iter().filter(|&&j| !s.chars[j].is_whitespace()));
            }
        }
        if let Some(p) = positions {
            p.sort_unstable();
            p.dedup();
        }
        Some(total - s.chars.len() as i32 / LENGTH_DIVISOR)
    }
}

fn match_phrase(phrase: &Term, s: &mut Scratch, want: bool) -> Option<i32> {
    #[cfg(test)]
    {
        s.phrase_runs += 1;
    }
    match_term(phrase, s, want)
}

fn is_subsequence(query: &[char], candidate: &str) -> bool {
    let mut q = query.iter().peekable();
    for c in candidate.chars() {
        match q.peek() {
            Some(&&want) if fold(c) == want => {
                q.next();
            }
            Some(_) => {}
            None => break,
        }
    }
    q.peek().is_none()
}

fn compute_bonus(chars: &[char], bonus: &mut Vec<i32>) {
    let name_start = chars.iter().rposition(|&c| is_path_sep(c)).map_or(0, |i| i + 1);
    bonus.clear();
    bonus.extend(chars.iter().enumerate().map(|(j, &c)| {
        let b = if j == 0 {
            BONUS_START
        } else {
            let prev = chars[j - 1];
            if is_path_sep(prev) {
                BONUS_PATH
            } else if !prev.is_alphanumeric() && c.is_alphanumeric() {
                BONUS_SEP
            } else if prev.is_lowercase() && c.is_uppercase() {
                BONUS_CAMEL
            } else if !prev.is_ascii_digit() && c.is_ascii_digit() {
                BONUS_DIGIT
            } else {
                0
            }
        };
        if j >= name_start { b + BONUS_FILENAME } else { b }
    }));
}

/// Best alignment of one term; fills `s.term_positions` when `want`.
///
/// `table[i][c]` is the best score for the term's first `i + 1` chars with
/// char `i` matched at candidate column `c` (offset into the window).
fn match_term(term: &Term, s: &mut Scratch, want: bool) -> Option<i32> {
    let (q, n) = (&term.folded, s.folded.len());
    let m = q.len();
    // Narrow to the window between the earliest possible start and the last
    // occurrence of the final query char.
    let first = s.folded.iter().position(|&c| c == q[0])?;
    let last = first + s.folded[first..].iter().rposition(|&c| c == q[m - 1])?;
    if last + 1 - first < m || n == 0 {
        return None;
    }
    let w = last + 1 - first;
    s.table.clear();
    s.table.resize(m * w, NEG);
    let match_score = |i: usize, j: usize| {
        let case = if s.chars[j] == term.chars[i] && term.chars[i].is_uppercase() { BONUS_EXACT_CASE } else { 0 };
        MATCH + s.bonus[j] + case
    };
    for i in 0..m {
        let (prev, row) = s.table.split_at_mut(i * w);
        let row = &mut row[..w];
        let prev = if i > 0 { &prev[(i - 1) * w..] } else { &[][..] };
        let mut run = NEG;
        for c in i..w {
            let j = first + c;
            if i > 0 && c >= 2 {
                run = (run - GAP_EXTEND).max(prev[c - 2] - GAP_OPEN);
            }
            if s.folded[j] != q[i] {
                continue;
            }
            let ms = match_score(i, j);
            row[c] = if i == 0 {
                ms
            } else {
                let diag = if c >= 1 { prev[c - 1] + BONUS_CONSECUTIVE } else { NEG };
                let best = diag.max(run);
                if best > NEG / 2 { best + ms } else { NEG }
            };
        }
    }
    let last_row = &s.table[(m - 1) * w..];
    let (mut c, &best) = last_row.iter().enumerate().rev().max_by_key(|&(_, v)| *v)?;
    if best <= NEG / 2 {
        return None;
    }
    s.term_positions.clear();
    if want {
        for i in (0..m).rev() {
            s.term_positions.push(first + c);
            if i == 0 {
                break;
            }
            let prev = &s.table[(i - 1) * w..i * w];
            let target = s.table[i * w + c] - match_score(i, first + c);
            c = if c >= 1 && prev[c - 1] > NEG / 2 && prev[c - 1] + BONUS_CONSECUTIVE == target {
                c - 1
            } else {
                (0..c.saturating_sub(1))
                    .rev()
                    .find(|&k| prev[k] > NEG / 2 && prev[k] - GAP_OPEN - GAP_EXTEND * (c - 2 - k) as i32 == target)
                    .expect("DP backtrack")
            };
        }
        s.term_positions.reverse();
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(query: &str, items: &[&str]) -> Vec<String> {
        rank(query, items, |s| s, usize::MAX).into_iter().map(|(i, _, _)| items[i].to_owned()).collect()
    }

    fn positions(query: &str, candidate: &str) -> Vec<usize> {
        score(query, candidate).unwrap().1
    }

    #[test]
    fn requires_subsequence_case_insensitively() {
        assert!(score("abc", "xaybzc").is_some());
        assert!(score("ABC", "abc").is_some());
        assert!(score("abc", "ABC").is_some());
        assert!(score("acb", "abc").is_none());
        assert!(score("abcd", "abc").is_none());
        assert!(score("x", "").is_none());
    }

    #[test]
    fn exact_name_beats_longer_prefix_match() {
        assert_eq!(top("app", &["src/apple_pie_parser.rs", "src/app.rs"])[0], "src/app.rs");
    }

    #[test]
    fn abbreviations_hit_word_starts() {
        let items = ["src/sidebar.rs", "src/search.rs", "src/buffer.rs", "scrub_bar.txt"];
        assert_eq!(top("sbr", &items)[0], "src/buffer.rs");
        assert_eq!(positions("sbr", "src/buffer.rs"), vec![0, 4, 11]);
    }

    #[test]
    fn spans_the_extension_dot() {
        let items = ["src/domain/rules.rs", "README.md", "src/main.rs", "src/maintenance/lib.rs"];
        assert_eq!(top("mainrs", &items)[0], "src/main.rs");
        assert_eq!(positions("mainrs", "src/main.rs"), vec![4, 5, 6, 7, 9, 10]);
    }

    #[test]
    fn prefers_file_name_over_directory() {
        assert_eq!(top("foo", &["foo/bar.rs", "bar/foo.rs"])[0], "bar/foo.rs");
    }

    #[test]
    fn prefers_consecutive_and_camel_humps() {
        assert_eq!(top("abc", &["axbxc", "xabcx"])[0], "xabcx");
        assert_eq!(top("gf", &["goodfellow", "getFile"])[0], "getFile");
        assert_eq!(positions("gf", "getFile"), vec![0, 3]);
    }

    #[test]
    fn separators_count_as_word_starts() {
        assert_eq!(top("ss", &["classes", "save_session"])[0], "save_session");
        assert_eq!(top("ft", &["after", "File: Toggle"])[0], "File: Toggle");
    }

    #[test]
    fn terms_match_independently() {
        assert!(score("rs main", "src/main.rs").is_some());
        assert!(score("main zz", "src/main.rs").is_none());
        let (_, p) = score("main rs", "src/main.rs").unwrap();
        assert_eq!(p, vec![4, 5, 6, 7, 9, 10]);
    }

    #[test]
    fn terms_in_order_as_a_phrase_win() {
        assert_eq!(top("rename note", &["Notes: Undo Rename", "Notes: Rename Note…"])[0], "Notes: Rename Note…");
        assert_eq!(top("new term", &["Terminal: New Terminal", "Terminal: Send to Terminal (new)"])[0], "Terminal: New Terminal");
        // The space between the words isn't highlighted.
        assert_eq!(positions("rename note", "Notes: Rename Note…"), vec![7, 8, 9, 10, 11, 12, 14, 15, 16, 17]);
    }

    /// The phrase pass is a second alignment per candidate, so it runs only
    /// for names holding the words themselves, in the order typed. A big
    /// folder whose paths have every letter and space in order (but not the
    /// words) costs only the words' own passes.
    #[test]
    fn the_phrase_pass_runs_only_for_the_words_in_order() {
        let runs = |query: &str, items: &[String]| {
            let pattern = Pattern::new(query);
            let mut s = Scratch::default();
            let matched = items.iter().filter(|c| pattern.score(c, &mut s, None).is_some()).count();
            (matched, s.phrase_runs)
        };
        let items: Vec<String> = (0..100_000).map(|i| format!("docs {i}/f-i t-e m-e n-o.md")).collect();
        assert_eq!(runs("fi te me no", &items), (100_000, 0));
        assert_eq!(runs("t-e m-e", &items), (100_000, 100_000));
        let names = ["Notes: Rename Note…", "Notes: Undo Rename", "Rename the Note"].map(String::from);
        assert_eq!(runs("rename note", &names), (3, 2), "words apart still count, in order");
    }

    #[test]
    fn ties_go_to_shorter_then_earlier() {
        let items = ["b/x.txt", "a/x.txt", "a/x.t"];
        let r = rank("x", &items, |s| s, 10);
        assert_eq!(r.iter().map(|r| r.0).collect::<Vec<_>>(), vec![2, 0, 1]);
    }

    #[test]
    fn empty_query_keeps_order_and_limit() {
        let items = ["c", "b", "a"];
        let r = rank("  ", &items, |s| s, 2);
        assert_eq!(r, vec![(0, 0, vec![]), (1, 0, vec![])]);
    }

    #[test]
    fn limit_keeps_the_best() {
        let items: Vec<String> = (0..1000).map(|i| format!("dir{i}/file{i}.rs")).chain(["main.rs".into()]).collect();
        let r = rank("main", &items, |s| s.as_str(), 3);
        assert_eq!(r.len(), 1);
        let r = rank("file", &items, |s| s.as_str(), 5);
        assert_eq!(r.len(), 5);
        assert!(r.windows(2).all(|w| w[0].1 >= w[1].1));
        assert_eq!(r[0].0, 0);
        assert_eq!(r[0].2, vec![5, 6, 7, 8]);
    }

    #[test]
    fn rank_scores_agree_with_score() {
        let items = ["src/app.rs", "src/apple_pie_parser.rs", "lib/a/p/p.rs"];
        for (i, s, p) in rank("app", &items, |s| s, 10) {
            assert_eq!(score("app", items[i]), Some((s, p)));
        }
    }

    #[test]
    fn indices_are_chars_not_bytes() {
        assert_eq!(positions("ße", "grüße"), vec![3, 4]);
        assert_eq!(positions("Ü", "grüße"), vec![2]);
    }

    #[test]
    fn exact_case_is_a_small_bonus() {
        let (a, _) = score("F", "F").unwrap();
        let (b, _) = score("F", "f").unwrap();
        assert!(a > b);
    }
}
