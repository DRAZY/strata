//! Spell checking: the bundled Hunspell dictionaries (SCOWL en_US and
//! en_GB, read by the `spellbook` crate), which words in a line are worth
//! checking, Sublime's `spelling_selector`, and the per-view cache of
//! misspelled words the editor underlines.
//!
//! Only prose is checked: the highlighter reports, per line, the byte
//! ranges whose scope matches the selector (comments and strings in code;
//! Markdown, plain text and commit messages apart from code and links). In
//! there, words are split at camelCase and snake_case boundaries, and
//! anything that looks like code (digits, dots, slashes, URLs, emails,
//! backticks) is skipped.

use crate::buffer::Buffer;
use crate::highlight::Highlighter;
use spellbook::Dictionary;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::{Range, RangeInclusive};
use std::sync::{Arc, Mutex};
use syntect::parsing::{Scope, ScopeStack};

pub const DEFAULT_DICTIONARY: &str = "en_US";

/// Sublime's default `spelling_selector`, except that code in Markdown
/// (`markup.raw`) is skipped rather than checked, and so is everything in
/// source files but comments and strings (`source` in the last group).
pub const DEFAULT_SELECTOR: &str = "source string.quoted - punctuation - meta.preprocessor.include, \
    source comment - source comment.block.preprocessor, \
    -(source, constant, keyword, storage, support, variable, markup.underline.link, markup.raw, meta.tag)";

/// The bundled dictionaries: (setting value, menu label).
pub const DICTIONARIES: &[(&str, &str)] = &[("en_US", "English (United States)"), ("en_GB", "English (United Kingdom)")];

/// The dictionaries' license (SCOWL and its sources), shown in Licenses and Attribution.
pub const LICENSE: &str = include_str!("../assets/dictionaries/LICENSE-SCOWL.txt");

static AFF: &str = include_str!("../assets/dictionaries/en.aff");
static EN_US: &str = include_str!("../assets/dictionaries/en_US.dic");
static EN_GB: &str = include_str!("../assets/dictionaries/en_GB.dic");

/// Suggestions offered at most (Sublime shows a handful).
pub const MAX_SUGGESTIONS: usize = 5;

// ---- dictionaries ----------------------------------------------------------------

enum Load {
    Loading,
    Ready(Arc<Dictionary>),
    Failed(String),
}

static LOADED: Mutex<Vec<(String, Load)>> = Mutex::new(Vec::new());

fn parse(name: &str) -> Result<Dictionary, String> {
    let dic = match name {
        "en_US" => EN_US,
        "en_GB" => EN_GB,
        other => return Err(format!("there is no dictionary \"{other}\" (en_US and en_GB are bundled)")),
    };
    Dictionary::new(AFF, dic).map_err(|e| format!("dictionary {name}: {e}"))
}

/// The dictionary, if loaded. The first call starts loading it on a worker
/// thread (it takes a moment) and returns None; `ctx` repaints when it's in.
pub fn dictionary(name: &str, ctx: Option<&egui::Context>) -> Result<Option<Arc<Dictionary>>, String> {
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    match loaded.iter().find(|(n, _)| n == name).map(|(_, l)| l) {
        Some(Load::Ready(d)) => return Ok(Some(d.clone())),
        Some(Load::Failed(e)) => return Err(e.clone()),
        Some(Load::Loading) => return Ok(None),
        None => {}
    }
    loaded.push((name.to_owned(), Load::Loading));
    let (name, ctx) = (name.to_owned(), ctx.cloned());
    std::thread::spawn(move || {
        let load = match parse(&name) {
            Ok(d) => Load::Ready(Arc::new(d)),
            Err(e) => Load::Failed(e),
        };
        let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = loaded.iter_mut().find(|(n, _)| *n == name) {
            slot.1 = load;
        }
        drop(loaded);
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    });
    Ok(None)
}

/// The dictionary, loading it on this thread if needed (tests, commands).
pub fn dictionary_now(name: &str) -> Result<Arc<Dictionary>, String> {
    loop {
        if let Some(d) = dictionary(name, None)? {
            return Ok(d);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

// ---- spelling_selector -----------------------------------------------------------

/// A Sublime scope selector: alternatives separated by `,` (or `|`), each a
/// scope path with optional `- exclusion`s, where an exclusion (or the
/// path) may be a parenthesized list of alternatives: `-(constant, keyword)`.
/// An alternative with no path (`-(source, …)`) matches every scope but the
/// excluded ones.
#[derive(Debug)]
pub struct Selector {
    alts: Vec<Alt>,
}

#[derive(Debug)]
struct Alt {
    /// Any of these matches (empty: everything does).
    any: Vec<ScopeStack>,
    /// Each group excludes when any of its paths matches.
    not: Vec<Vec<ScopeStack>>,
}

/// Split at top-level `seps` (outside parentheses).
fn split_top(s: &str, seps: &[char]) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && seps.contains(&c) {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// `a b`, or `(a, b c)`, as alternatives.
fn paths(s: &str) -> Result<Vec<ScopeStack>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let inner = s.strip_prefix('(').and_then(|r| r.strip_suffix(')'));
    let parts = match inner {
        Some(inner) => split_top(inner, &[',', '|']),
        None => vec![s.to_owned()],
    };
    parts
        .iter()
        .filter(|p| !p.trim().is_empty())
        .map(|p| p.trim().parse::<ScopeStack>().map_err(|e| format!("spelling_selector: \"{}\": {e:?}", p.trim())))
        .collect()
}

impl std::str::FromStr for Selector {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let mut alts = Vec::new();
        for alt in split_top(s, &[',', '|']) {
            if alt.trim().is_empty() {
                continue;
            }
            // `-` excludes when it starts the alternative or follows a space
            // (scope names can contain hyphens).
            let (mut parts, mut cur, mut depth, mut prev) = (Vec::new(), String::new(), 0i32, ' ');
            for c in alt.trim().chars() {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                if c == '-' && depth == 0 && prev.is_whitespace() {
                    parts.push(std::mem::take(&mut cur));
                } else {
                    cur.push(c);
                }
                prev = c;
            }
            parts.push(cur);
            let any = paths(&parts[0])?;
            let not = parts[1..].iter().map(|p| paths(p)).collect::<Result<Vec<_>, _>>()?;
            alts.push(Alt { any, not });
        }
        Ok(Selector { alts })
    }
}

impl Selector {
    pub fn matches(&self, stack: &[Scope]) -> bool {
        let hit = |p: &ScopeStack| p.does_match(stack).is_some();
        self.alts.iter().any(|a| (a.any.is_empty() || a.any.iter().any(hit)) && !a.not.iter().any(|g| g.iter().any(hit)))
    }
}

// ---- words -------------------------------------------------------------------------

/// Characters that make a chunk look like code (or a path, URL, email…).
const CODE_CHARS: &str = ".:/\\@`=<>{}[]()#$%&*+|~^\"";
/// Sentence punctuation (and Markdown emphasis) a word may sit between.
const PUNCTUATION: &str = ".,;:!?'\"()[]{}<>*_‘’“”«»…-–—";

/// Words worth checking in `text` (byte ranges). Whitespace-separated chunks
/// lose surrounding punctuation; a chunk that still holds code-ish
/// characters (or a call like `f()`) is skipped, the rest split at `,;-_`
/// and camelCase. Words with digits, single letters, short ALL-CAPS words
/// (acronyms), hex-looking words and non-Latin words are left alone.
pub fn words(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let start = i + rest.find(|c: char| !c.is_whitespace()).unwrap_or(rest.len());
        let end = start + text[start..].find(char::is_whitespace).unwrap_or(text.len() - start);
        i = end;
        let chunk = &text[start..end];
        let punct = |c: char| PUNCTUATION.contains(c);
        let lead = chunk.len() - chunk.trim_start_matches(punct).len();
        let trimmed = chunk[lead..].trim_end_matches(punct);
        if trimmed.is_empty() || trimmed.contains(|c| CODE_CHARS.contains(c)) || chunk.contains("()") {
            continue;
        }
        let base = start + lead;
        for (offset, part) in pieces(trimmed) {
            out.extend(camel_words(part).into_iter().map(|r| base + offset + r.start..base + offset + r.end).filter(|r| worth_checking(&text[r.clone()])));
        }
    }
    out
}

/// A chunk split at `,`, `;`, `-`, `–`, `_` and `'` / `’` that don't sit between letters.
fn pieces(chunk: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    let chars: Vec<(usize, char)> = chunk.char_indices().collect();
    for (k, &(b, c)) in chars.iter().enumerate() {
        let apostrophe = matches!(c, '\'' | '’');
        let between = k > 0 && chars.get(k + 1).is_some_and(|n| n.1.is_alphabetic()) && chars[k - 1].1.is_alphabetic();
        if matches!(c, ',' | ';' | '-' | '–' | '_' | '!' | '?') || (apostrophe && !between) {
            if b > from {
                out.push((from, &chunk[from..b]));
            }
            from = b + c.len_utf8();
        }
    }
    if from < chunk.len() {
        out.push((from, &chunk[from..]));
    }
    out
}

/// camelCase / PascalCase / HTTPServer → its words (byte ranges in `s`).
fn camel_words(s: &str) -> Vec<Range<usize>> {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut out = Vec::new();
    let mut start = 0;
    for k in 1..chars.len() {
        let (b, c) = chars[k];
        let prev = chars[k - 1].1;
        let next_lower = chars.get(k + 1).is_some_and(|n| n.1.is_lowercase());
        // fooBar | FOOBar (before the last capital of a run followed by lower case)
        if c.is_uppercase() && (prev.is_lowercase() || (prev.is_uppercase() && next_lower)) {
            out.push(start..b);
            start = b;
        }
    }
    out.push(start..s.len());
    out
}

fn worth_checking(w: &str) -> bool {
    let letters = w.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 2 || w.chars().any(|c| c.is_numeric()) {
        return false;
    }
    // Latin script only: an English dictionary has nothing to say about others.
    if w.chars().any(|c| c.is_alphabetic() && c as u32 > 0x24F) {
        return false;
    }
    if letters <= 4 && w.chars().all(|c| !c.is_lowercase()) {
        return false;
    }
    let hexish = w.len() >= 8 && w.chars().all(|c| c.is_ascii_hexdigit());
    !hexish
}

// ---- checking ----------------------------------------------------------------------

/// The checker in effect: dictionary, added and ignored words, selector,
/// and a cache of words already looked up.
pub struct Speller {
    dictionary: String,
    added: Vec<String>,
    added_set: HashSet<String>,
    ignored: HashSet<String>,
    selector_text: String,
    selector: Option<Arc<Selector>>,
    /// Problem with the selector or dictionary, for the status bar.
    pub error: Option<String>,
    known: HashMap<String, bool>,
    /// Bumped whenever an answer may have changed (views drop their caches).
    epoch: u64,
    suggestions: Option<(String, Vec<String>)>,
}

impl Default for Speller {
    fn default() -> Self {
        Self {
            dictionary: String::new(),
            added: Vec::new(),
            added_set: HashSet::new(),
            ignored: HashSet::new(),
            selector_text: String::new(),
            selector: None,
            error: None,
            known: HashMap::new(),
            epoch: 1,
            suggestions: None,
        }
    }
}

impl Speller {
    /// Follow the settings; cheap when nothing changed.
    pub fn configure(&mut self, dictionary: &str, added: &[String], selector: &str) {
        if self.dictionary != dictionary || self.added != added {
            self.dictionary = dictionary.to_owned();
            self.added = added.to_vec();
            self.added_set = added.iter().map(|w| w.trim().to_owned()).filter(|w| !w.is_empty()).collect();
            self.forget();
        }
        if self.selector_text != selector || self.selector.is_none() && self.error.is_none() {
            self.selector_text = selector.to_owned();
            self.error = None;
            self.selector = match selector.parse::<Selector>() {
                Ok(s) => Some(Arc::new(s)),
                Err(e) => {
                    self.error = Some(e);
                    DEFAULT_SELECTOR.parse().ok().map(Arc::new)
                }
            };
            self.epoch += 1;
        }
    }

    fn forget(&mut self) {
        self.known.clear();
        self.suggestions = None;
        self.epoch += 1;
    }

    pub fn selector(&self) -> Option<Arc<Selector>> {
        self.selector.clone()
    }

    pub fn dictionary_name(&self) -> &str {
        &self.dictionary
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Ignore Word: accepted until Strata quits.
    pub fn ignore(&mut self, word: &str) {
        self.ignored.insert(word.to_owned());
        self.forget();
    }

    /// Is `word` spelled right? Added and ignored words always are (as
    /// written, or in lower case for a capitalized word).
    pub fn check(&mut self, dict: &Dictionary, word: &str) -> bool {
        if let Some(&ok) = self.known.get(word) {
            return ok;
        }
        let lower = word.to_lowercase();
        let listed = |set: &HashSet<String>| set.contains(word) || set.contains(&lower);
        let ok = listed(&self.added_set) || listed(&self.ignored) || dict.check(&word.replace('’', "'"));
        if self.known.len() > 200_000 {
            self.known.clear();
        }
        self.known.insert(word.to_owned(), ok);
        ok
    }

    /// Up to `MAX_SUGGESTIONS` corrections (the last word's are kept).
    pub fn suggest(&mut self, dict: &Dictionary, word: &str) -> Vec<String> {
        if let Some((w, s)) = &self.suggestions
            && w == word
        {
            return s.clone();
        }
        let mut out = Vec::new();
        dict.suggest(word, &mut out);
        out.retain(|s| s != word);
        out.truncate(MAX_SUGGESTIONS);
        self.suggestions = Some((word.to_owned(), out.clone()));
        out
    }

    /// Misspelled words of `text` (one line) inside `prose` (its byte
    /// ranges to check), as char ranges in the line.
    pub fn misspelled(&mut self, dict: &Dictionary, text: &str, prose: &[Range<usize>]) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let (mut byte, mut chr) = (0, 0);
        for p in prose {
            let (a, b) = (p.start.min(text.len()), p.end.min(text.len()));
            if a >= b || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
                continue;
            }
            for w in words(&text[a..b]) {
                let w = a + w.start..a + w.end;
                if self.check(dict, &text[w.clone()]) {
                    continue;
                }
                chr += text[byte..w.start].chars().count();
                let start = chr;
                chr += text[w.clone()].chars().count();
                byte = w.end;
                out.push(start..chr);
            }
        }
        out
    }
}

// ---- marks: what a view underlines ---------------------------------------------------

fn hash_of<T: Hash + ?Sized>(t: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

type Ranges = Arc<Vec<Range<usize>>>;

/// Misspelled words for one editor view: char ranges within each line,
/// keyed by the line's text so a line that just changed shows nothing
/// until it's checked again (rather than marks in the wrong place).
#[derive(Default)]
pub struct Marks {
    lines: HashMap<usize, (u64, Ranges)>,
    by_text: HashMap<u64, Ranges>,
    /// (line text hash, prose ranges hash) → result, for the speller's `epoch`.
    cache: HashMap<(u64, u64), Ranges>,
    epoch: u64,
}

impl Marks {
    /// Nothing to underline.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.lines.values().all(|(_, r)| r.is_empty())
    }

    pub fn clear(&mut self) {
        if !self.lines.is_empty() || !self.cache.is_empty() {
            *self = Self::default();
        }
    }

    /// Misspellings to underline on `line`, whose text is `text`.
    pub fn for_line(&self, line: usize, text: &str) -> Option<&[Range<usize>]> {
        if self.lines.is_empty() {
            return None;
        }
        let h = hash_of(text);
        let found = match self.lines.get(&line) {
            Some((lh, r)) if *lh == h => Some(r),
            // Lines moved (Enter above them): the same text is still checked.
            _ => self.by_text.get(&h),
        };
        found.map(|r| r.as_slice()).filter(|r| !r.is_empty())
    }

    /// Check the `visible` lines. Lines the highlighter hasn't reached keep
    /// their old marks. Returns true if anything to draw changed.
    pub fn refresh(&mut self, buffer: &Buffer, hl: &Highlighter, visible: RangeInclusive<usize>, speller: &mut Speller, dict: &Dictionary) -> bool {
        if self.epoch != speller.epoch() {
            self.cache.clear();
            self.epoch = speller.epoch();
        }
        if self.cache.len() > 50_000 {
            self.cache.clear();
        }
        let last = (*visible.end()).min(buffer.len_lines().saturating_sub(1));
        let mut lines = HashMap::with_capacity(last.saturating_sub(*visible.start()) + 1);
        for line in *visible.start()..=last {
            let text = buffer.line_text(line);
            let h = hash_of(&text);
            let Some(prose) = hl.prose(line) else {
                if let Some(old) = self.lines.get(&line).filter(|(lh, _)| *lh == h) {
                    lines.insert(line, old.clone());
                }
                continue;
            };
            let key = (h, hash_of(prose.as_slice()));
            let found = match self.cache.get(&key) {
                Some(r) => r.clone(),
                None => {
                    let r = Arc::new(speller.misspelled(dict, &text, &prose));
                    self.cache.insert(key, r.clone());
                    r
                }
            };
            lines.insert(line, (h, found));
        }
        let same = lines.len() == self.lines.len() && lines.iter().all(|(l, (h, r))| self.lines.get(l).is_some_and(|(oh, or)| oh == h && Arc::ptr_eq(or, r)));
        if !same {
            self.by_text = lines.values().map(|(h, r)| (*h, r.clone())).collect();
            self.lines = lines;
        }
        !same
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(s: &str) -> Vec<Scope> {
        s.parse::<ScopeStack>().unwrap().as_slice().to_vec()
    }

    fn word_list(text: &str) -> Vec<&str> {
        words(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn both_dictionaries_load_and_differ_on_british_spellings() {
        let us = dictionary_now("en_US").unwrap();
        let gb = dictionary_now("en_GB").unwrap();
        for w in ["hello", "editor", "Strings", "don't", "it's"] {
            assert!(us.check(w) && gb.check(w), "{w}");
        }
        assert!(us.check("color") && !us.check("colour"));
        assert!(gb.check("colour") && !gb.check("color"));
        assert!(!us.check("speling"));
        assert!(dictionary_now("tlh_KL").unwrap_err().contains("no dictionary"));
    }

    /// `cargo test --release spell_frame_cost -- --ignored --nocapture`: the
    /// per-frame cost of spell check on a 1 MB Markdown file.
    #[test]
    #[ignore]
    fn spell_frame_cost() {
        use std::time::Instant;
        let para = "Spell checking keeps typing fast: only the lines on screen are looked at, and each line's result is cached by its text. Some wrod and `inline code` and a [link](https://example.com) here.\n\n";
        let mut text = String::from("# A large document\n\n");
        while text.len() < 1_000_000 {
            text.push_str(para);
        }
        let buf = Buffer::from_str(&text);
        let t = Instant::now();
        let dict = dictionary_now("en_US").unwrap();
        let load = t.elapsed().as_secs_f64() * 1000.0;
        // Uncached checks: 1,000 distinct words, half of them misspelled.
        let words: Vec<String> = EN_US.lines().skip(1).step_by(40).take(1000).enumerate().map(|(k, l)| {
            let w = l.split('/').next().unwrap_or(l);
            if k % 2 == 0 { w.to_owned() } else { format!("{w}x") }
        }).collect();
        let t = Instant::now();
        let bad = words.iter().filter(|w| !dict.check(w)).count();
        let per_word = t.elapsed().as_secs_f64() * 1e6 / words.len() as f64;
        let t = Instant::now();
        let mut out = Vec::new();
        dict.suggest("speling", &mut out);
        let suggest = t.elapsed().as_secs_f64() * 1000.0;
        eprintln!("dictionary load {load:.0} ms; uncached check {per_word:.2} µs/word ({bad} of 1000 rejected); suggestions {suggest:.1} ms");
        let mut sp = Speller::default();
        sp.configure("en_US", &[], DEFAULT_SELECTOR);
        let (mut plain, mut prose) = (Highlighter::new(), Highlighter::new());
        for h in [&mut plain, &mut prose] {
            h.detect(Some(std::path::Path::new("big.md")), "");
        }
        prose.set_prose_selector(sp.selector());
        let mid = buf.len_lines() / 2;
        let view = mid..=mid + 60;
        // Highlight up to the middle first (checkpoints), as scrolling there would.
        for h in [&mut plain, &mut prose] {
            while !h.prepare(&buf, view.clone()) {}
        }
        let time = |f: &mut dyn FnMut()| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        };
        // Re-highlighting the visible lines after an edit, with and without prose ranges.
        let hl_plain = time(&mut || {
            plain.invalidate_from(mid);
            plain.prepare(&buf, view.clone());
        });
        let hl_prose = time(&mut || {
            prose.invalidate_from(mid);
            prose.prepare(&buf, view.clone());
        });
        let mut marks = Marks::default();
        let first = time(&mut || {
            marks.refresh(&buf, &prose, view.clone(), &mut sp, &dict);
        });
        let n = 200;
        let steady = time(&mut || {
            for _ in 0..n {
                marks.refresh(&buf, &prose, view.clone(), &mut sp, &dict);
            }
        }) / n as f64;
        let marked = (mid..=mid + 60).filter(|&l| marks.for_line(l, &buf.line_text(l)).is_some()).count();
        eprintln!(
            "1 MB markdown, 61 visible lines: highlight after an edit {hl_plain:.2} ms plain / {hl_prose:.2} ms with prose ranges; \
             spelling first pass {first:.2} ms, steady per frame {steady:.3} ms; {marked} lines underlined"
        );
        assert!(steady < 2.0, "per-frame spelling cost {steady} ms");
    }

    #[test]
    fn the_settings_page_shows_the_default_selector() {
        assert!(include_str!("../docs/manual/settings.md").contains(&format!("`\"{DEFAULT_SELECTOR}\"`")));
        assert!(DEFAULT_SELECTOR.parse::<Selector>().is_ok());
    }

    #[test]
    fn the_dictionary_license_is_reproduced_verbatim() {
        // UKACD's notice asks for the whole text; Ispell's BSD terms for the copyright.
        for needle in ["Copyright 2000-2018 by Kevin Atkinson", "UK Advanced Cryptics Dictionary", "Copyright 1993, Geoff Kuenning", "WordNet 1.6 Copyright 1997"] {
            assert!(LICENSE.contains(needle), "{needle}");
        }
    }

    #[test]
    fn tokenizer_splits_identifiers_and_skips_code() {
        assert_eq!(word_list("Hello, world! (it's fine)"), ["Hello", "world", "it's", "fine"]);
        assert_eq!(word_list("camelCaseWord snake_case_word HTTPServer"), ["camel", "Case", "Word", "snake", "case", "word", "Server"]);
        assert_eq!(word_list("see https://example.com or me@example.com and src/main.rs"), ["see", "or", "and"]);
        assert_eq!(word_list("file.rs foo() a=b `code` x2 utf8 0xff #tag"), Vec::<&str>::new());
        assert_eq!(word_list("NASA JSON ASCII deadbeefcafe wellknown"), ["ASCII", "wellknown"], "short acronyms and hex skipped");
        assert_eq!(word_list("well-known users' 'quoted'"), ["well", "known", "users", "quoted"]);
        assert_eq!(word_list("Привет naïve"), ["naïve"], "non-Latin words skipped");
        // Byte ranges point into the text even after multibyte characters.
        let t = "é wrod";
        assert_eq!(&t[words(t)[0].clone()], "wrod");
    }

    #[test]
    fn selector_parses_sublime_syntax() {
        let s: Selector = DEFAULT_SELECTOR.parse().unwrap();
        assert!(s.matches(&stack("source.rust comment.line.double-slash.rust")));
        assert!(s.matches(&stack("source.python string.quoted.double.python")));
        assert!(!s.matches(&stack("source.python string.quoted.double.python punctuation.definition.string.begin.python")));
        assert!(!s.matches(&stack("source.rust meta.function.rust entity.name.function.rust")), "code isn't prose");
        assert!(!s.matches(&stack("source.rust keyword.control.rust")));
        assert!(s.matches(&stack("text.plain")));
        assert!(s.matches(&stack("text.html.markdown meta.paragraph.markdown")));
        assert!(!s.matches(&stack("text.html.markdown markup.raw.inline.markdown")), "inline code");
        assert!(!s.matches(&stack("text.html.markdown markup.underline.link.markdown")));
        assert!(s.matches(&stack("text.html.markdown markup.raw.code-fence.markdown source.rust comment.line.rust")), "comments in fenced code");
        assert!(!s.matches(&stack("source.c meta.preprocessor.include.c string.quoted.other.lt-gt.include.c")));
        // A hyphen inside a scope name isn't an exclusion.
        let s: Selector = "comment.line.double-slash".parse().unwrap();
        assert!(s.matches(&stack("source.rust comment.line.double-slash.rust")));
        assert!(!s.matches(&stack("source.rust comment.block.rust")));
        let s: Selector = "text, source - (comment | string)".parse().unwrap();
        assert!(s.matches(&stack("source.c")) && !s.matches(&stack("source.c string.quoted")) && s.matches(&stack("text.plain")));
    }

    #[test]
    fn speller_checks_added_and_ignored_words_and_suggests() {
        let dict = dictionary_now("en_US").unwrap();
        let mut sp = Speller::default();
        sp.configure("en_US", &["Zorblax".into(), "egui".into()], DEFAULT_SELECTOR);
        assert!(sp.check(&dict, "Zorblax") && sp.check(&dict, "egui") && sp.check(&dict, "Egui"));
        assert!(!sp.check(&dict, "wrod"));
        let before = sp.epoch();
        sp.ignore("wrod");
        assert!(sp.check(&dict, "wrod") && sp.epoch() > before);
        let s = sp.suggest(&dict, "speling");
        assert!(s.iter().any(|w| w == "spelling"), "{s:?}");
        assert!(s.len() <= MAX_SUGGESTIONS);
        sp.configure("en_US", &[], DEFAULT_SELECTOR);
        assert!(!sp.check(&dict, "Zorblax"), "removing an added word takes effect");
    }

    #[test]
    fn a_bad_selector_is_reported_and_the_default_used() {
        let mut sp = Speller::default();
        sp.configure("en_US", &[], "comment - (");
        assert!(sp.selector().is_some());
        sp.configure("en_US", &[], DEFAULT_SELECTOR);
        assert!(sp.error.is_none());
    }

    #[test]
    fn misspelled_reports_char_ranges_inside_prose_only() {
        let dict = dictionary_now("en_US").unwrap();
        let mut sp = Speller::default();
        sp.configure("en_US", &[], DEFAULT_SELECTOR);
        let line = "let wrod = 1; // é teh wrod";
        let comment = line.find("//").unwrap();
        assert_eq!(sp.misspelled(&dict, line, &[comment..line.len()]), vec![19..22, 23..27]);
        assert!(sp.misspelled(&dict, line, &[]).is_empty());
    }

    #[test]
    fn marks_check_highlighted_prose_and_follow_line_text() {
        let dict = dictionary_now("en_US").unwrap();
        let mut sp = Speller::default();
        sp.configure("en_US", &[], DEFAULT_SELECTOR);
        let buf = Buffer::from_str("fn main() {} // a wrod here\nlet speling = 1;\n");
        let mut hl = Highlighter::new();
        hl.detect(Some(std::path::Path::new("a.rs")), "");
        hl.set_prose_selector(sp.selector());
        assert!(hl.prepare(&buf, 0..=2));
        let mut marks = Marks::default();
        assert!(marks.refresh(&buf, &hl, 0..=2, &mut sp, &dict));
        assert_eq!(marks.for_line(0, &buf.line_text(0)), Some(&[18..22][..]));
        assert_eq!(marks.for_line(1, &buf.line_text(1)), None, "identifiers in code aren't checked");
        assert_eq!(marks.for_line(0, "changed text"), None);
        // The same line moved down keeps its marks.
        assert_eq!(marks.for_line(5, &buf.line_text(0)), Some(&[18..22][..]));
        assert!(!marks.refresh(&buf, &hl, 0..=2, &mut sp, &dict), "nothing changed");
        sp.ignore("wrod");
        assert!(marks.refresh(&buf, &hl, 0..=2, &mut sp, &dict));
        assert_eq!(marks.for_line(0, &buf.line_text(0)), None);
    }

    #[test]
    fn markdown_checks_text_but_not_code_or_links() {
        let dict = dictionary_now("en_US").unwrap();
        let mut sp = Speller::default();
        sp.configure("en_US", &[], DEFAULT_SELECTOR);
        let text = "# Teh title\n\nSome wrod and `fn mispeled()` and [a link](http://exmaple.com).\n\n```rust\nlet speling = 1; // commnet\n```\n";
        let buf = Buffer::from_str(text);
        let mut hl = Highlighter::new();
        hl.detect(Some(std::path::Path::new("a.md")), "");
        hl.set_prose_selector(sp.selector());
        assert!(hl.prepare(&buf, 0..=8));
        let mut marks = Marks::default();
        marks.refresh(&buf, &hl, 0..=8, &mut sp, &dict);
        let flagged: Vec<String> = (0..buf.len_lines())
            .flat_map(|l| {
                let t = buf.line_text(l);
                marks.for_line(l, &t).unwrap_or(&[]).iter().map(|r| t.chars().skip(r.start).take(r.len()).collect::<String>()).collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(flagged, ["Teh", "wrod", "commnet"]);
    }
}
