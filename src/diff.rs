//! Text diffing for Compare: a line diff (patience anchors, then linear-space
//! Myers between them), side-by-side row alignment with filler rows, and
//! word-level intra-line differences. Pure functions; no UI.
//!
//! Patience diff matches lines that occur exactly once on each side first,
//! which keeps big files fast (the expensive part only ever runs on the gaps
//! between those anchors) and gives readable hunks for code (a moved `}` does
//! not glue unrelated blocks together). Inside a gap, Myers' O(ND) algorithm
//! finds a shortest edit script; a work budget turns a pathological gap into
//! one "changed" block instead of a stall.

use std::collections::HashMap;
use std::ops::Range;

/// A changed block: lines `a` of the left text became lines `b` of the right.
/// Either range may be empty (a pure insertion or deletion); `a.start` /
/// `b.start` are then where the block sits on that side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub a: Range<usize>,
    pub b: Range<usize>,
}

/// Myers work units (diagonal steps) allowed per gap before giving up on it.
const GAP_BUDGET: usize = 4_000_000;

/// How lines compare: as is, or with all whitespace ignored.
fn key(line: &str, ignore_ws: bool) -> std::borrow::Cow<'_, str> {
    if ignore_ws && line.chars().any(char::is_whitespace) {
        std::borrow::Cow::Owned(line.chars().filter(|c| !c.is_whitespace()).collect())
    } else {
        std::borrow::Cow::Borrowed(line)
    }
}

/// Number the distinct lines of both sides, so comparing is integer equality.
fn intern<'a>(a: &[&'a str], b: &[&'a str], ignore_ws: bool) -> (Vec<u32>, Vec<u32>) {
    let mut ids: HashMap<std::borrow::Cow<'a, str>, u32> = HashMap::with_capacity(a.len() + b.len());
    let mut id = |l: &'a str| {
        let n = ids.len() as u32;
        *ids.entry(key(l, ignore_ws)).or_insert(n)
    };
    let ai = a.iter().map(|l| id(l)).collect();
    let bi = b.iter().map(|l| id(l)).collect();
    (ai, bi)
}

/// Changed blocks turning lines `a` into lines `b`, in order.
pub fn diff_lines(a: &[&str], b: &[&str], ignore_ws: bool) -> Vec<Hunk> {
    let (ai, bi) = intern(a, b, ignore_ws);
    hunks_from_matches(&matches(&ai, &bi), ai.len(), bi.len())
}

/// Matched (a, b) index pairs, strictly increasing in both.
pub fn matches(a: &[u32], b: &[u32]) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(a.len().min(b.len()));
    patience(a, b, 0, 0, &mut out);
    out
}

fn hunks_from_matches(m: &[(usize, usize)], a_len: usize, b_len: usize) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    for &(x, y) in m.iter().chain(std::iter::once(&(a_len, b_len))) {
        if x > i || y > j {
            hunks.push(Hunk { a: i..x, b: j..y });
        }
        (i, j) = (x + 1, y + 1);
    }
    hunks
}

/// Match the common prefix and suffix, then lines unique on both sides
/// (their longest increasing run), recursing into the gaps.
fn patience(a: &[u32], b: &[u32], ao: usize, bo: usize, out: &mut Vec<(usize, usize)>) {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    out.extend((0..pre).map(|k| (ao + k, bo + k)));
    let (a, b) = (&a[pre..], &b[pre..]);
    let (ao, bo) = (ao + pre, bo + pre);
    let suf = a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[..a.len() - suf], &b[..b.len() - suf]);
    if !am.is_empty() && !bm.is_empty() {
        let anchors = unique_lcs(am, bm);
        if anchors.is_empty() {
            let mut budget = GAP_BUDGET;
            myers(am, bm, ao, bo, out, &mut budget);
        } else {
            let (mut i, mut j) = (0, 0);
            for (x, y) in anchors {
                patience(&am[i..x], &bm[j..y], ao + i, bo + j, out);
                out.push((ao + x, bo + y));
                (i, j) = (x + 1, y + 1);
            }
            patience(&am[i..], &bm[j..], ao + i, bo + j, out);
        }
    }
    let (sa, sb) = (ao + am.len(), bo + bm.len());
    out.extend((0..suf).map(|k| (sa + k, sb + k)));
}

/// Lines occurring exactly once in each side, as (a, b) positions forming the
/// longest chain increasing on both sides (patience sorting).
fn unique_lcs(a: &[u32], b: &[u32]) -> Vec<(usize, usize)> {
    // id -> (count in a, position in a, count in b, position in b)
    let mut seen: HashMap<u32, (u32, usize, u32, usize)> = HashMap::new();
    for (i, &l) in a.iter().enumerate() {
        let e = seen.entry(l).or_insert((0, i, 0, 0));
        e.0 += 1;
    }
    for (j, &l) in b.iter().enumerate() {
        if let Some(e) = seen.get_mut(&l) {
            e.2 += 1;
            e.3 = j;
        }
    }
    let mut pairs: Vec<(usize, usize)> = seen.values().filter(|e| e.0 == 1 && e.2 == 1).map(|e| (e.1, e.3)).collect();
    if pairs.is_empty() {
        return pairs;
    }
    pairs.sort_unstable();
    // Longest increasing subsequence of b positions (O(n log n)).
    let mut tails: Vec<usize> = Vec::new(); // index into pairs
    let mut prev: Vec<Option<usize>> = vec![None; pairs.len()];
    for (k, &(_, y)) in pairs.iter().enumerate() {
        let at = tails.partition_point(|&t| pairs[t].1 < y);
        if at > 0 {
            prev[k] = Some(tails[at - 1]);
        }
        if at == tails.len() {
            tails.push(k);
        } else {
            tails[at] = k;
        }
    }
    let mut chain = Vec::with_capacity(tails.len());
    let mut cur = tails.last().copied();
    while let Some(k) = cur {
        chain.push(pairs[k]);
        cur = prev[k];
    }
    chain.reverse();
    chain
}

/// Linear-space Myers (bisection on the middle snake). Out of budget, the
/// rest of the gap is left unmatched (shown as one changed block).
fn myers(a: &[u32], b: &[u32], ao: usize, bo: usize, out: &mut Vec<(usize, usize)>, budget: &mut usize) {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    out.extend((0..pre).map(|k| (ao + k, bo + k)));
    let (a, b) = (&a[pre..], &b[pre..]);
    let (ao, bo) = (ao + pre, bo + pre);
    let suf = a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[..a.len() - suf], &b[..b.len() - suf]);
    if !am.is_empty()
        && !bm.is_empty()
        && let Some((x, y)) = bisect(am, bm, budget)
    {
        myers(&am[..x], &bm[..y], ao, bo, out, budget);
        myers(&am[x..], &bm[y..], ao + x, bo + y, out, budget);
    }
    let (sa, sb) = (ao + am.len(), bo + bm.len());
    out.extend((0..suf).map(|k| (sa + k, sb + k)));
}

/// A point on a shortest edit path splitting it in two halves of about equal
/// cost (Myers 1986, section 4b). None: no common line, or out of budget.
fn bisect(a: &[u32], b: &[u32], budget: &mut usize) -> Option<(usize, usize)> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max_d = (n + m + 1) / 2;
    let off = max_d + 1;
    let len = (2 * max_d + 3) as usize;
    let mut v1 = vec![-1isize; len];
    let mut v2 = vec![-1isize; len];
    v1[(off + 1) as usize] = 0;
    v2[(off + 1) as usize] = 0;
    let delta = n - m;
    let front = delta % 2 != 0;
    let (mut k1start, mut k1end, mut k2start, mut k2end) = (0, 0, 0, 0);
    for d in 0..max_d {
        let work = (2 * d + 1) as usize;
        if *budget < work {
            *budget = 0;
            return None;
        }
        *budget -= work;
        let mut k1 = -d + k1start;
        while k1 <= d - k1end {
            let k1o = (off + k1) as usize;
            let mut x1 = if k1 == -d || (k1 != d && v1[k1o - 1] < v1[k1o + 1]) { v1[k1o + 1] } else { v1[k1o - 1] + 1 };
            let mut y1 = x1 - k1;
            while x1 < n && y1 < m && a[x1 as usize] == b[y1 as usize] {
                x1 += 1;
                y1 += 1;
            }
            v1[k1o] = x1;
            if x1 > n {
                k1end += 2;
            } else if y1 > m {
                k1start += 2;
            } else if front {
                let k2o = off + delta - k1;
                if k2o >= 0 && (k2o as usize) < len && v2[k2o as usize] != -1 && x1 >= n - v2[k2o as usize] {
                    return Some((x1 as usize, y1 as usize));
                }
            }
            k1 += 2;
        }
        let mut k2 = -d + k2start;
        while k2 <= d - k2end {
            let k2o = (off + k2) as usize;
            let mut x2 = if k2 == -d || (k2 != d && v2[k2o - 1] < v2[k2o + 1]) { v2[k2o + 1] } else { v2[k2o - 1] + 1 };
            let mut y2 = x2 - k2;
            while x2 < n && y2 < m && a[(n - x2 - 1) as usize] == b[(m - y2 - 1) as usize] {
                x2 += 1;
                y2 += 1;
            }
            v2[k2o] = x2;
            if x2 > n {
                k2end += 2;
            } else if y2 > m {
                k2start += 2;
            } else if !front {
                let k1o = off + delta - k2;
                if k1o >= 0 && (k1o as usize) < len && v1[k1o as usize] != -1 {
                    let x1 = v1[k1o as usize];
                    let y1 = off + x1 - k1o;
                    if x1 >= n - x2 {
                        return Some((x1 as usize, y1 as usize));
                    }
                }
            }
            k2 += 2;
        }
    }
    None
}

// ---- side by side ------------------------------------------------------------------

/// What a side-by-side row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// The same line on both sides.
    Same,
    /// A line on each side, paired inside a changed block.
    Changed,
    /// Left only (deleted); the right shows a filler row.
    Removed,
    /// Right only (added); the left shows a filler row.
    Added,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub kind: RowKind,
    /// Index of the hunk the row belongs to (None for unchanged rows).
    pub hunk: Option<usize>,
}

/// One row per line pair: unchanged lines side by side, each changed block's
/// lines paired top to bottom, and filler on the shorter side.
pub fn align(hunks: &[Hunk], a_len: usize, b_len: usize) -> Vec<Row> {
    let mut rows = Vec::with_capacity(a_len.max(b_len) + 16);
    let (mut i, mut j) = (0, 0);
    let same = |rows: &mut Vec<Row>, i: &mut usize, j: &mut usize, until_a: usize| {
        while *i < until_a {
            rows.push(Row { left: Some(*i), right: Some(*j), kind: RowKind::Same, hunk: None });
            *i += 1;
            *j += 1;
        }
    };
    for (h, hunk) in hunks.iter().enumerate() {
        same(&mut rows, &mut i, &mut j, hunk.a.start);
        let (na, nb) = (hunk.a.len(), hunk.b.len());
        for k in 0..na.max(nb) {
            let (left, right) = ((k < na).then(|| hunk.a.start + k), (k < nb).then(|| hunk.b.start + k));
            let kind = match (left, right) {
                (Some(_), Some(_)) => RowKind::Changed,
                (Some(_), None) => RowKind::Removed,
                _ => RowKind::Added,
            };
            rows.push(Row { left, right, kind, hunk: Some(h) });
        }
        (i, j) = (hunk.a.end, hunk.b.end);
    }
    same(&mut rows, &mut i, &mut j, a_len);
    debug_assert_eq!(j, b_len);
    rows
}

/// Totals for a header line: changed blocks, lines removed and added.
pub fn stats(hunks: &[Hunk]) -> (usize, usize, usize) {
    hunks.iter().fold((0, 0, 0), |(n, r, a), h| (n + 1, r + h.a.len(), a + h.b.len()))
}

// ---- inside a line -----------------------------------------------------------------

/// Lines longer than this (chars) are marked changed as a whole.
const MAX_INLINE: usize = 4_000;

/// Word-ish tokens as char ranges: runs of letters/digits/_, runs of
/// whitespace, and single other characters.
fn tokens(chars: &[char]) -> Vec<Range<usize>> {
    let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let k = class(chars[i]);
        let mut j = i + 1;
        if k != 2 {
            while j < chars.len() && class(chars[j]) == k {
                j += 1;
            }
        }
        out.push(i..j);
        i = j;
    }
    out
}

/// The char ranges that differ inside a changed pair of lines, (left, right),
/// at word granularity. Whitespace-only differences are dropped when
/// `ignore_ws` is set.
pub fn inline_diff(a: &str, b: &str, ignore_ws: bool) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let (ac, bc): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if ac.len() > MAX_INLINE || bc.len() > MAX_INLINE {
        return (vec![0..ac.len()], vec![0..bc.len()]);
    }
    let (at, bt) = (tokens(&ac), tokens(&bc));
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut id = |chars: &[char], r: &Range<usize>| {
        let s: String = chars[r.clone()].iter().collect();
        let n = ids.len() as u32;
        *ids.entry(s).or_insert(n)
    };
    let ai: Vec<u32> = at.iter().map(|r| id(&ac, r)).collect();
    let bi: Vec<u32> = bt.iter().map(|r| id(&bc, r)).collect();
    let mut m = Vec::new();
    let mut budget = GAP_BUDGET / 16;
    myers(&ai, &bi, 0, 0, &mut m, &mut budget);
    let mut left = Vec::new();
    let mut right = Vec::new();
    for h in hunks_from_matches(&m, ai.len(), bi.len()) {
        let span = |toks: &[Range<usize>], r: &Range<usize>| toks.get(r.start).map(|s| s.start).unwrap_or(0)..toks.get(r.end.saturating_sub(1)).map_or(0, |e| e.end);
        let blank = |chars: &[char], r: &Range<usize>| chars[r.clone()].iter().all(|c| c.is_whitespace());
        let (l, r) = (span(&at, &h.a), span(&bt, &h.b));
        if ignore_ws && blank(&ac, &l) && blank(&bc, &r) {
            continue;
        }
        if !h.a.is_empty() {
            push_merged(&mut left, l);
        }
        if !h.b.is_empty() {
            push_merged(&mut right, r);
        }
    }
    (left, right)
}

fn push_merged(v: &mut Vec<Range<usize>>, r: Range<usize>) {
    match v.last_mut() {
        Some(last) if last.end >= r.start => last.end = last.end.max(r.end),
        _ => v.push(r),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applying the hunks to `a` gives `b`, and unchanged stretches really match.
    fn check(a: &[&str], b: &[&str]) -> Vec<Hunk> {
        let hunks = diff_lines(a, b, false);
        let mut out: Vec<&str> = Vec::new();
        let (mut i, mut j) = (0, 0);
        for h in &hunks {
            assert!(h.a.start >= i && h.b.start >= j, "hunks in order: {hunks:?}");
            assert_eq!(h.a.start - i, h.b.start - j, "equal stretch lengths");
            assert_eq!(&a[i..h.a.start], &b[j..h.b.start], "unchanged lines match");
            assert!(!h.a.is_empty() || !h.b.is_empty());
            out.extend(&a[i..h.a.start]);
            out.extend(&b[h.b.clone()]);
            (i, j) = (h.a.end, h.b.end);
        }
        assert_eq!(&a[i..], &b[j..]);
        out.extend(&a[i..]);
        assert_eq!(out, b);
        let rows = align(&hunks, a.len(), b.len());
        assert_eq!(rows.iter().filter(|r| r.left.is_some()).count(), a.len());
        assert_eq!(rows.iter().filter(|r| r.right.is_some()).count(), b.len());
        hunks
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    const WORDS: &[&str] = &["a", "b", "c", "d", "}", "", "fn x() {", "return;", "e", "f"];

    #[test]
    fn random_edits_round_trip() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..2_000 {
            let n = rng.below(40);
            let a: Vec<&str> = (0..n).map(|_| WORDS[rng.below(WORDS.len())]).collect();
            let mut b = a.clone();
            for _ in 0..rng.below(8) {
                match rng.below(3) {
                    0 if !b.is_empty() => {
                        let k = rng.below(b.len());
                        b.remove(k);
                    }
                    1 => {
                        let k = rng.below(b.len() + 1);
                        b.insert(k, WORDS[rng.below(WORDS.len())]);
                    }
                    _ if !b.is_empty() => {
                        let k = rng.below(b.len());
                        b[k] = WORDS[rng.below(WORDS.len())];
                    }
                    _ => {}
                }
            }
            check(&a, &b);
            check(&b, &a);
        }
    }

    #[test]
    fn myers_alone_is_minimal_on_small_inputs() {
        // Compare the edit count with a plain LCS by dynamic programming.
        let mut rng = Rng(42);
        for _ in 0..500 {
            let a: Vec<u32> = (0..rng.below(14)).map(|_| rng.below(4) as u32).collect();
            let b: Vec<u32> = (0..rng.below(14)).map(|_| rng.below(4) as u32).collect();
            let mut m = Vec::new();
            let mut budget = usize::MAX;
            myers(&a, &b, 0, 0, &mut m, &mut budget);
            let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
            for i in (0..a.len()).rev() {
                for j in (0..b.len()).rev() {
                    dp[i][j] = if a[i] == b[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
                }
            }
            assert_eq!(m.len(), dp[0][0], "{a:?} {b:?}");
            assert!(m.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1));
            assert!(m.iter().all(|&(x, y)| a[x] == b[y]));
        }
    }

    #[test]
    fn simple_hunks() {
        assert_eq!(check(&["a", "b", "c"], &["a", "b", "c"]), vec![]);
        assert_eq!(check(&["a", "b", "c"], &["a", "c"]), vec![Hunk { a: 1..2, b: 1..1 }]);
        assert_eq!(check(&["a", "c"], &["a", "b", "c"]), vec![Hunk { a: 1..1, b: 1..2 }]);
        assert_eq!(check(&["a", "b", "c"], &["a", "B", "c"]), vec![Hunk { a: 1..2, b: 1..2 }]);
        assert_eq!(check(&[], &["x"]), vec![Hunk { a: 0..0, b: 0..1 }]);
        assert_eq!(check(&["x", "y"], &[]), vec![Hunk { a: 0..2, b: 0..0 }]);
    }

    #[test]
    fn whitespace_can_be_ignored() {
        let (a, b) = (["fn x() {", "  return 1;", "}"], ["fn x() {", "\treturn  1;", "}"]);
        assert_eq!(diff_lines(&a, &b, false).len(), 1);
        assert!(diff_lines(&a, &b, true).is_empty());
    }

    #[test]
    fn rows_pair_changes_and_pad_the_short_side() {
        let a = ["one", "two", "three", "four"];
        let b = ["one", "TWO", "2b", "four", "five"];
        let hunks = check(&a, &b);
        let rows = align(&hunks, a.len(), b.len());
        let kinds: Vec<RowKind> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, [RowKind::Same, RowKind::Changed, RowKind::Changed, RowKind::Same, RowKind::Added]);
        assert_eq!(rows[4], Row { left: None, right: Some(4), kind: RowKind::Added, hunk: Some(1) });
        assert_eq!(stats(&hunks), (2, 2, 3));
    }

    #[test]
    fn patience_keeps_functions_apart() {
        // Myers alone may match the lone `}` lines across functions; patience
        // anchors on the unique signatures.
        let a = ["fn a() {", "  x();", "}", "", "fn b() {", "  y();", "}"];
        let b = ["fn a() {", "  x();", "}", "", "fn new() {", "  z();", "}", "", "fn b() {", "  y();", "}"];
        let hunks = check(&a, &b);
        assert_eq!(hunks, vec![Hunk { a: 4..4, b: 4..8 }]);
    }

    #[test]
    fn inline_diff_marks_changed_words() {
        let (l, r) = inline_diff("let total = price * qty;", "let total = price * quantity + tax;", false);
        assert_eq!(l, vec![20..23]);
        assert_eq!(r, vec![20..34]);
        let (l, r) = inline_diff("a  b", "a b", true);
        assert!(l.is_empty() && r.is_empty());
        let (l, r) = inline_diff("same", "same", false);
        assert!(l.is_empty() && r.is_empty());
        let (l, r) = inline_diff("", "new text", false);
        assert!(l.is_empty());
        assert_eq!(r, vec![0..8]);
        // Non-ASCII: ranges are chars, not bytes.
        let (l, r) = inline_diff("café au lait", "café noir", false);
        assert_eq!((l, r), (vec![5..12], vec![5..9]));
    }

    /// 50k-line files with scattered edits diff well within the 200 ms budget
    /// (checked in release builds; debug builds get a looser bound).
    #[test]
    fn big_files_are_fast() {
        let mut rng = Rng(7);
        let a: Vec<String> = (0..50_000).map(|i| if i % 7 == 0 { "}".to_owned() } else { format!("    let v{} = f({});", rng.below(5_000), i % 97) }).collect();
        let mut b = a.clone();
        for _ in 0..500 {
            let k = rng.below(b.len());
            match rng.below(3) {
                0 => {
                    b.remove(k);
                }
                1 => b.insert(k, format!("inserted {k}")),
                _ => b[k] = format!("changed {k}"),
            }
        }
        let (ar, br): (Vec<&str>, Vec<&str>) = (a.iter().map(String::as_str).collect(), b.iter().map(String::as_str).collect());
        let t = std::time::Instant::now();
        let hunks = diff_lines(&ar, &br, false);
        let ms = t.elapsed().as_millis();
        eprintln!("diff of 50k lines, 500 edits: {} hunks in {ms} ms", hunks.len());
        check(&ar, &br);
        let limit = if cfg!(debug_assertions) { 3_000 } else { 200 };
        assert!(ms < limit, "50k lines took {ms} ms");
        assert!(!hunks.is_empty());

        // Completely different files: the budget keeps it bounded.
        let c: Vec<String> = (0..50_000).map(|i| format!("other {}", i % 3)).collect();
        let cr: Vec<&str> = c.iter().map(String::as_str).collect();
        let t = std::time::Instant::now();
        check(&ar, &cr);
        let ms = t.elapsed().as_millis();
        eprintln!("diff of two unrelated 50k-line files: {ms} ms");
        assert!(ms < limit * 5, "unrelated 50k-line files took {ms} ms");
    }
}
