//! EditorConfig (<https://spec.editorconfig.org>): `.editorconfig` files
//! from a file's folder up to the root (or the first `root = true`), their
//! section globs, and the properties Strata applies.
//!
//! Parsed files are cached with their modification time and size; `Cache::check`
//! re-reads the ones that changed (and notices new ones).

use crate::buffer::LineEnding;
use crate::encoding::Encoding;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

pub const FILE_NAME: &str = ".editorconfig";

/// One parsed `.editorconfig`.
#[derive(Debug, Default, PartialEq)]
pub struct File {
    /// `root = true` in the preamble: folders above aren't looked at.
    pub root: bool,
    pub sections: Vec<Section>,
}

#[derive(Debug, PartialEq)]
pub struct Section {
    pub glob: String,
    /// Keys lower-cased; values of the spec's properties lower-cased too.
    pub pairs: Vec<(String, String)>,
}

/// Properties whose values are case-insensitive (the spec lower-cases them).
const KNOWN: &[&str] = &["indent_style", "indent_size", "tab_width", "end_of_line", "charset", "trim_trailing_whitespace", "insert_final_newline", "root", "max_line_length"];

impl File {
    /// INI as the spec has it: `[glob]` sections, `key = value` pairs,
    /// whole-line `#` / `;` comments. Lines that are none of these are skipped.
    pub fn parse(text: &str) -> Self {
        let mut file = File::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') && line.len() >= 2 {
                file.sections.push(Section { glob: line[1..line.len() - 1].to_owned(), pairs: Vec::new() });
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let key = key.trim().to_ascii_lowercase();
            let mut value = value.trim().to_owned();
            if KNOWN.contains(&key.as_str()) {
                value.make_ascii_lowercase();
            }
            if key.is_empty() {
                continue;
            }
            match file.sections.last_mut() {
                Some(s) => s.pairs.push((key, value)),
                None if key == "root" => file.root = value == "true",
                None => {}
            }
        }
        file
    }
}

/// Does `glob` (from a section of the `.editorconfig` in `dir`) match `path`?
/// A glob without a `/` matches the file name at any depth; one with a `/`
/// is relative to `dir`.
pub fn section_matches(glob: &str, dir: &Path, path: &Path) -> bool {
    let dir = slashes(dir);
    let path = slashes(path);
    let glob = if glob_has_slash(glob) { format!("/{}", glob.strip_prefix('/').unwrap_or(glob)) } else { format!("/**/{glob}") };
    let (re, ranges) = translate(&glob);
    let Ok(re) = regex::Regex::new(&format!("(?s)^{}{re}\\z", regex::escape(dir.trim_end_matches('/')))) else { return false };
    let Some(caps) = re.captures(&path) else { return false };
    // `{n1..n2}`: the number must be in range, written without leading zeros.
    ranges.iter().enumerate().all(|(k, &(lo, hi))| {
        let Some(m) = caps.get(k + 1) else { return false };
        let digits = m.as_str().trim_start_matches(['+', '-']);
        (digits == "0" || !digits.starts_with('0')) && m.as_str().parse::<i64>().is_ok_and(|n| lo <= n && n <= hi)
    })
}

/// Path as text with `/` separators (EditorConfig globs always use `/`).
fn slashes(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) { s.replace('\\', "/") } else { s.into_owned() }
}

/// A `/` outside `[…]`.
fn glob_has_slash(glob: &str) -> bool {
    let mut depth = false;
    let mut escaped = false;
    for c in glob.chars() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '[' => depth = true,
            ']' => depth = false,
            '/' if !depth => return true,
            _ => {}
        }
    }
    false
}

/// The glob as a regular expression (unanchored), and the `{n1..n2}` ranges
/// whose numbers are its capture groups, in order. Follows the reference
/// implementation (editorconfig-core-py's `fnmatch.translate`).
fn translate(pat: &str) -> (String, Vec<(i64, i64)>) {
    let chars: Vec<char> = pat.chars().collect();
    let n = chars.len();
    let unescaped = |c: char| (0..n).filter(|&i| chars[i] == c && (i == 0 || chars[i - 1] != '\\')).count();
    let matching_braces = unescaped('{') == unescaped('}');
    let mut out = String::new();
    let mut ranges = Vec::new();
    let (mut i, mut braces, mut in_brackets, mut escaped) = (0, 0usize, false, false);
    let lit = |c: char| regex::escape(&c.to_string());
    while i < n {
        let c = chars[i];
        i += 1;
        match c {
            _ if escaped => out.push_str(&lit(c)),
            '\\' => {}
            '*' => out.push_str(if chars.get(i) == Some(&'*') { ".*" } else { "[^/]*" }),
            '?' => out.push_str("[^/]"),
            '[' if in_brackets => out.push_str("\\["),
            '[' => {
                // A class can't hold a `/`: then the `[` is literal.
                let close = (i..n).find(|&p| chars[p] == ']');
                let slash = (i..close.unwrap_or(n)).any(|p| chars[p] == '/' && chars[p - 1] != '\\');
                if close.is_none() {
                    out.push_str("\\[");
                } else if slash {
                    let end = close.unwrap_or(n);
                    out.push_str("\\[");
                    chars[i..end].iter().for_each(|&c| out.push_str(&lit(c)));
                    out.push_str("\\]");
                    i = end + 1;
                } else {
                    if matches!(chars.get(i), Some('!' | '^')) {
                        i += 1;
                        out.push_str("[^");
                    } else {
                        out.push('[');
                    }
                    in_brackets = true;
                }
            }
            '-' if in_brackets => out.push('-'),
            ']' if in_brackets => {
                out.push(']');
                in_brackets = false;
            }
            '{' => {
                // `{a,b}` alternatives, `{n1..n2}` numbers; `{single}` is literal.
                let mut p = i;
                let (mut comma, mut esc) = (false, false);
                while p < n && (chars[p] != '}' || esc) {
                    if chars[p] == ',' && !esc {
                        comma = true;
                        break;
                    }
                    esc = chars[p] == '\\' && !esc;
                    p += 1;
                }
                if !comma && p < n {
                    let inner: String = chars[i..p].iter().collect();
                    match numeric_range(&inner) {
                        Some(r) => {
                            ranges.push(r);
                            out.push_str(r"([+-]?\d+)");
                        }
                        None => {
                            let (re, inner_ranges) = translate(&inner);
                            out.push_str(&format!("\\{{{re}\\}}"));
                            ranges.extend(inner_ranges);
                        }
                    }
                    i = p + 1;
                } else if matching_braces {
                    out.push_str("(?:");
                    braces += 1;
                } else {
                    out.push_str("\\{");
                }
            }
            ',' if braces > 0 => out.push('|'),
            '}' if braces > 0 => {
                out.push(')');
                braces -= 1;
            }
            '/' if !in_brackets && chars[i..].starts_with(&['*', '*', '/']) => {
                out.push_str("(?:/|/.*/)");
                i += 3;
            }
            '/' => out.push('/'),
            _ => out.push_str(&lit(c)),
        }
        escaped = c == '\\' && !escaped;
    }
    if in_brackets {
        // An unclosed class can't happen (checked above); keep the regex valid regardless.
        out.push(']');
    }
    (out, ranges)
}

fn numeric_range(s: &str) -> Option<(i64, i64)> {
    let (a, b) = s.split_once("..")?;
    let num = |t: &str| {
        let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
        (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then(|| t.parse::<i64>().ok()).flatten()
    };
    Some((num(a)?, num(b)?))
}

/// The properties that apply to one file, after every matching section.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Properties {
    /// Every pair (keys lower-cased), `unset` ones removed.
    pub pairs: BTreeMap<String, String>,
    /// The `.editorconfig` files with a section that matched, nearest last.
    pub files: Vec<PathBuf>,
}

impl Properties {
    /// Pairs from `files` (farthest first, as (folder, file)) for `path`:
    /// later sections and nearer files win; `unset` removes a property.
    pub fn resolve(path: &Path, files: &[(PathBuf, Arc<File>)]) -> Self {
        let mut props = Properties::default();
        for (dir, file) in files {
            let mut used = false;
            for s in file.sections.iter().filter(|s| section_matches(&s.glob, dir, path)) {
                used = true;
                for (k, v) in &s.pairs {
                    if v == "unset" {
                        props.pairs.remove(k);
                    } else {
                        props.pairs.insert(k.clone(), v.clone());
                    }
                }
            }
            if used {
                props.files.push(dir.join(FILE_NAME));
            }
        }
        props
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.pairs.get(key).map(String::as_str)
    }

    fn flag(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }

    fn number(&self, key: &str) -> Option<usize> {
        self.get(key)?.parse().ok().filter(|&n| n > 0)
    }

    /// `indent_style`: Some(true) for spaces.
    pub fn use_spaces(&self) -> Option<bool> {
        match self.get("indent_style")? {
            "space" => Some(true),
            "tab" => Some(false),
            _ => None,
        }
    }

    /// The indentation width: `indent_size`, or `tab_width` when that is
    /// `tab` or missing (Strata has one width for both).
    pub fn width(&self) -> Option<usize> {
        match self.get("indent_size") {
            Some("tab") | None => self.number("tab_width").or_else(|| self.number("indent_size")),
            Some(_) => self.number("indent_size"),
        }
    }

    /// `end_of_line`. `cr` (classic Mac OS) isn't supported and is ignored.
    pub fn end_of_line(&self) -> Option<LineEnding> {
        match self.get("end_of_line")? {
            "lf" => Some(LineEnding::Lf),
            "crlf" => Some(LineEnding::CrLf),
            _ => None,
        }
    }

    /// `charset`. UTF-16 is written with a byte order mark.
    pub fn charset(&self) -> Option<Encoding> {
        match self.get("charset")? {
            "utf-8" => Some(Encoding::UTF8),
            "utf-8-bom" => Some(Encoding::UTF8_BOM),
            "latin1" => Some(Encoding::ISO_8859_1),
            "utf-16le" => Some(Encoding::UTF16LE_BOM),
            "utf-16be" => Some(Encoding::UTF16BE_BOM),
            _ => None,
        }
    }

    pub fn trim_trailing_whitespace(&self) -> Option<bool> {
        self.flag("trim_trailing_whitespace")
    }

    pub fn insert_final_newline(&self) -> Option<bool> {
        self.flag("insert_final_newline")
    }

    /// `max_line_length` (`off` and anything not a number: None).
    pub fn max_line_length(&self) -> Option<usize> {
        self.number("max_line_length")
    }
}

/// Parsed `.editorconfig` files by path; missing ones are remembered too,
/// so `check` notices when one appears.
#[derive(Default)]
pub struct Cache {
    files: HashMap<PathBuf, Cached>,
    /// Goes up whenever a cached file changes: what was resolved before is stale.
    pub rev: u64,
}

struct Cached {
    stamp: Option<(SystemTime, u64)>,
    parsed: Option<Arc<File>>,
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    Some((m.modified().ok()?, m.len()))
}

fn read(path: &Path) -> Cached {
    let stamp = stamp(path);
    let parsed = stamp.and_then(|_| std::fs::read(path).ok()).map(|b| Arc::new(File::parse(&String::from_utf8_lossy(&b))));
    Cached { stamp, parsed }
}

impl Cache {
    /// The properties for `path`, reading (once) each `.editorconfig` from
    /// its folder up to the root or the first with `root = true`.
    pub fn properties(&mut self, path: &Path) -> Properties {
        let mut chain = Vec::new();
        let mut dir = path.parent();
        while let Some(d) = dir {
            let file = d.join(FILE_NAME);
            let entry = self.files.entry(file.clone()).or_insert_with(|| read(&file));
            if let Some(parsed) = &entry.parsed {
                chain.push((d.to_owned(), parsed.clone()));
                if parsed.root {
                    break;
                }
            }
            dir = d.parent();
        }
        chain.reverse();
        Properties::resolve(path, &chain)
    }

    /// Re-read files that changed, appeared or went away. True (and `rev`
    /// goes up) if any did.
    pub fn check(&mut self) -> bool {
        let mut changed = false;
        for (path, entry) in &mut self.files {
            if stamp(path) != entry.stamp {
                *entry = read(path);
                changed = true;
            }
        }
        if changed {
            self.rev += 1;
        }
        changed
    }

    /// `path` was just written (saved in Strata): read it again.
    pub fn saved(&mut self, path: &Path) {
        if path.file_name().is_some_and(|n| n == FILE_NAME) {
            self.files.remove(path);
            self.rev += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Does `glob` in /p/.editorconfig match /p/`file`?
    fn m(glob: &str, file: &str) -> bool {
        section_matches(glob, Path::new("/p"), &Path::new("/p").join(file))
    }

    #[test]
    fn star_and_question_mark() {
        assert!(m("*", "a.py") && m("*", "sub/dir/a.py"));
        assert!(m("*.py", "a.py") && m("*.py", "x/y/a.py"));
        assert!(!m("*.py", "a.pyc") && !m("*.py", "a.js"));
        assert!(m("a?c.txt", "abc.txt") && !m("a?c.txt", "ac.txt") && !m("a?c.txt", "abbc.txt"));
        // `*` and `?` never cross a `/`.
        assert!(m("a*.txt", "ab.txt") && !m("a*.txt", "ab/c.txt"));
        assert!(!m("dir/*.c", "dir/sub/a.c") && m("dir/*.c", "dir/a.c"));
        assert!(!m("a?b", "a/b"));
    }

    #[test]
    fn double_star_crosses_folders() {
        assert!(m("dir/**/a.c", "dir/a.c"), "**/ matches zero folders");
        assert!(m("dir/**/a.c", "dir/x/a.c") && m("dir/**/a.c", "dir/x/y/a.c"));
        assert!(m("**.c", "a.c") && m("**.c", "x/y/a.c"));
        assert!(m("dir/**", "dir/x/y/z.txt") && !m("dir/**", "other/z.txt"));
    }

    #[test]
    fn slash_anchors_to_the_editorconfig_folder() {
        assert!(m("lib/*.js", "lib/a.js") && !m("lib/*.js", "src/lib/a.js"));
        assert!(m("/lib/*.js", "lib/a.js"), "a leading slash is the same");
        assert!(m("a.js", "src/lib/a.js"), "no slash: any depth");
        // Another folder's .editorconfig never matches files outside it.
        assert!(!section_matches("*", Path::new("/p/q"), Path::new("/p/a.txt")));
        // Characters in the folder name are literal.
        assert!(section_matches("*.c", Path::new("/w/[x]{y}"), Path::new("/w/[x]{y}/a.c")));
    }

    #[test]
    fn brackets() {
        assert!(m("[abc].txt", "a.txt") && m("[abc].txt", "c.txt") && !m("[abc].txt", "d.txt"));
        assert!(m("[a-c].txt", "b.txt") && !m("[a-c].txt", "e.txt"));
        assert!(m("[!abc].txt", "d.txt") && !m("[!abc].txt", "a.txt"));
        assert!(m("[^abc].txt", "d.txt") && !m("[^abc].txt", "b.txt"));
        // A class can't hold a slash: the brackets are literal.
        assert!(m("ab[e/]cd.i", "ab[e/]cd.i") && !m("ab[e/]cd.i", "abecd.i"));
        // An unclosed bracket is literal.
        assert!(m("[ab", "[ab") && !m("[ab", "a"));
        assert!(m("a[-]b", "a-b"));
    }

    #[test]
    fn braces() {
        assert!(m("*.{py,js}", "a.py") && m("*.{py,js}", "b.js") && !m("*.{py,js}", "c.rs"));
        assert!(m("{Makefile,*.mk}", "Makefile") && m("{Makefile,*.mk}", "x/rules.mk"));
        assert!(m("a{b,c{d,e}}f", "acef") && m("a{b,c{d,e}}f", "abf") && !m("a{b,c{d,e}}f", "acf"));
        assert!(m("{single}.b", "{single}.b") && !m("{single}.b", "single.b"), "one choice is literal");
        assert!(m("{}.c", "{}.c"));
        assert!(m("{a,}.c", "a.c") && m("{a,}.c", ".c"), "an empty alternative");
        assert!(m("{.foo", "{.foo"), "unmatched braces are literal");
        assert!(m("a\\{b,c\\}", "a{b,c}"), "escaped braces are literal");
        assert!(m("{word,{also},this}.g", "word.g") && m("{word,{also},this}.g", "{also}.g"));
    }

    #[test]
    fn numeric_ranges() {
        assert!(m("{3..120}", "3") && m("{3..120}", "15") && m("{3..120}", "120"));
        assert!(!m("{3..120}", "1") && !m("{3..120}", "121") && !m("{3..120}", "060"), "out of range, or a leading zero");
        assert!(m("file{-3..3}.txt", "file-2.txt") && m("file{-3..3}.txt", "file0.txt") && !m("file{-3..3}.txt", "file4.txt"));
        assert!(m("a{1..2}b{5..6}", "a1b6") && !m("a{1..2}b{5..6}", "a1b7"));
        assert!(m("{1..a}", "{1..a}") && !m("{1..a}", "1"), "not numbers: literal");
    }

    #[test]
    fn escapes_and_specials() {
        assert!(m("a\\*b", "a*b") && !m("a\\*b", "axb"));
        assert!(m("a\\?b", "a?b") && !m("a\\?b", "axb"));
        assert!(m("a.b", "a.b") && !m("a.b", "axb"), "dots are literal");
        assert!(m("a+b(c)$", "a+b(c)$"));
        assert!(m("a\\[b]", "a[b]"));
    }

    #[test]
    fn parsing_ini() {
        let f = File::parse("; top\nroot = TRUE\n\n# comment\n[*]\nIndent_Style = Space\nindent_size=2\nmy_key = KeepCase\n[*.md]\n  trim_trailing_whitespace = false  \nnot a pair\n");
        assert!(f.root);
        assert_eq!(f.sections.len(), 2);
        assert_eq!(f.sections[0].pairs, vec![("indent_style".into(), "space".into()), ("indent_size".into(), "2".into()), ("my_key".into(), "KeepCase".into())]);
        assert_eq!(f.sections[1].glob, "*.md");
        assert_eq!(f.sections[1].pairs, vec![("trim_trailing_whitespace".into(), "false".into())]);
        // root only counts before the first section; inline comments are part of the value.
        let f = File::parse("[*]\nroot = true\nindent_size = 4 # four\n");
        assert!(!f.root);
        assert_eq!(f.sections[0].pairs[1].1, "4 # four");
    }

    fn props(files: &[(&str, &str)], path: &str) -> Properties {
        let chain: Vec<(PathBuf, Arc<File>)> = files.iter().map(|(d, t)| (PathBuf::from(d), Arc::new(File::parse(t)))).collect();
        Properties::resolve(Path::new(path), &chain)
    }

    #[test]
    fn later_sections_and_nearer_files_win() {
        let p = props(&[("/p", "[*]\nindent_size = 4\nindent_style = space\n[*.py]\nindent_size = 8\n"), ("/p/sub", "[*.py]\nindent_style = tab\n")], "/p/sub/a.py");
        assert_eq!(p.width(), Some(8));
        assert_eq!(p.use_spaces(), Some(false));
        assert_eq!(p.files, vec![PathBuf::from("/p/.editorconfig"), PathBuf::from("/p/sub/.editorconfig")]);
        // Only the files with a matching section are listed.
        let p = props(&[("/p", "[*.rs]\nindent_size = 4\n"), ("/p/sub", "[*.py]\nindent_style = tab\n")], "/p/sub/a.py");
        assert_eq!(p.files, vec![PathBuf::from("/p/sub/.editorconfig")]);
        assert_eq!(p.width(), None);
    }

    #[test]
    fn unset_removes_a_property() {
        let p = props(&[("/p", "[*]\nindent_size = 2\ncharset = utf-8\n[*.bin]\nindent_size = unset\n")], "/p/x.bin");
        assert_eq!(p.width(), None);
        assert_eq!(p.charset(), Some(Encoding::UTF8));
        let p = props(&[("/p", "[*]\ntrim_trailing_whitespace = true\n"), ("/p/q", "[*]\ntrim_trailing_whitespace = UNSET\n")], "/p/q/a");
        assert_eq!(p.trim_trailing_whitespace(), None);
        assert!(p.pairs.is_empty());
    }

    #[test]
    fn property_values() {
        let p = props(&[("/p", "[*]\nindent_style = tab\nindent_size = tab\ntab_width = 8\nend_of_line = CRLF\ncharset = utf-16le\ninsert_final_newline = true\nmax_line_length = 100\n")], "/p/a");
        assert_eq!((p.use_spaces(), p.width(), p.end_of_line(), p.charset()), (Some(false), Some(8), Some(LineEnding::CrLf), Some(Encoding::UTF16LE_BOM)));
        assert_eq!((p.insert_final_newline(), p.max_line_length()), (Some(true), Some(100)));
        let p = props(&[("/p", "[*]\nindent_size = 3\ntab_width = 8\nend_of_line = cr\nmax_line_length = off\ncharset = latin1\nindent_style = weird\n")], "/p/a");
        assert_eq!((p.use_spaces(), p.width(), p.end_of_line(), p.max_line_length(), p.charset()), (None, Some(3), None, None, Some(Encoding::ISO_8859_1)));
        let p = props(&[("/p", "[*]\nindent_size = tab\n")], "/p/a");
        assert_eq!(p.width(), None, "tab with no tab_width: the editor's own width");
        let p = props(&[("/p", "[*]\ntab_width = 6\ncharset = utf-8-bom\nend_of_line = lf\n")], "/p/a");
        assert_eq!((p.width(), p.charset(), p.end_of_line()), (Some(6), Some(Encoding::UTF8_BOM), Some(LineEnding::Lf)));
    }

    #[test]
    fn cache_stops_at_root_and_notices_changes() {
        let dir = std::env::temp_dir().join(format!("strata-ec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("proj/src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.join(FILE_NAME), "[*]\nindent_style = tab\nend_of_line = crlf\n").unwrap();
        std::fs::write(dir.join("proj").join(FILE_NAME), "root = true\n[*.rs]\nindent_style = space\nindent_size = 4\n").unwrap();
        let file = sub.join("main.rs");
        let mut cache = Cache::default();
        let p = cache.properties(&file);
        assert_eq!((p.use_spaces(), p.width()), (Some(true), Some(4)));
        assert_eq!(p.end_of_line(), None, "the folder above root = true is ignored");
        assert!(!cache.check(), "nothing changed");
        // A new nearer file is picked up by check.
        std::fs::write(sub.join(FILE_NAME), "[*]\nindent_size = 2\n").unwrap();
        assert!(cache.check());
        assert_eq!(cache.properties(&file).width(), Some(2));
        // An edit with the same size in the same second: `saved` forces it.
        std::fs::write(sub.join(FILE_NAME), "[*]\nindent_size = 3\n").unwrap();
        let rev = cache.rev;
        cache.saved(&sub.join(FILE_NAME));
        assert!(cache.rev > rev);
        assert_eq!(cache.properties(&file).width(), Some(3));
        // root = false (or gone): the folder above counts again.
        std::fs::write(dir.join("proj").join(FILE_NAME), "[*.rs]\nindent_size = 4\n").unwrap();
        cache.saved(&dir.join("proj").join(FILE_NAME));
        assert_eq!(cache.properties(&file).end_of_line(), Some(LineEnding::CrLf));
        std::fs::remove_dir_all(dir).ok();
    }
}
