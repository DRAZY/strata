//! Snippets in Sublime Text's `.sublime-snippet` format, so existing snippet
//! collections drop straight in:
//!
//! ```xml
//! <snippet>
//!     <content><![CDATA[fn ${1:name}(${2}) {
//!     $0
//! }]]></content>
//!     <tabTrigger>fn</tabTrigger>
//!     <scope>source.rust</scope>
//!     <description>Function</description>
//! </snippet>
//! ```
//!
//! Bodies use TextMate-style fields: `$1`…`$n` and `$0` (the final caret),
//! placeholders `${1:default}` (nestable), mirrors (the same number twice;
//! edited together with one cursor per copy), variables `$TM_FILENAME` /
//! `${TM_SELECTED_TEXT:default}`, transforms `${1/regex/format/flags}` and
//! escapes `\$`, `\\`, `\}`.
//!
//! The pieces here are UI-free: [`Library`] loads files, [`expand`] inserts a
//! snippet at every cursor and returns a [`Session`] that Tab / Shift+Tab walk
//! through the fields.

use crate::editor::Editor;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    pub trigger: String,
    pub description: String,
    /// Scope selector (`source.rust`, `source.js, source.ts`, `text.html - source`);
    /// empty means everywhere.
    pub scope: String,
    pub content: String,
    /// The file it came from; None for built-ins.
    pub path: Option<PathBuf>,
}

impl Snippet {
    /// Name shown in lists: the description, else the trigger, else the file name.
    pub fn label(&self) -> String {
        if !self.description.is_empty() {
            return self.description.clone();
        }
        if !self.trigger.is_empty() {
            return self.trigger.clone();
        }
        self.path
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(|| "Snippet".to_owned(), |s| s.to_string_lossy().into_owned())
    }
}

// ---- .sublime-snippet files ------------------------------------------------

/// Parse a `.sublime-snippet` document.
pub fn parse_xml(src: &str) -> Result<Snippet, String> {
    let mut r = XmlReader { s: src.trim_start_matches('\u{feff}'), i: 0 };
    r.skip_misc()?;
    if !r.rest().starts_with('<') {
        return Err("expected a <snippet> element".into());
    }
    let root = r.element(0)?;
    if root.name != "snippet" {
        return Err(format!("the root element is <{}>, expected <snippet>", root.name));
    }
    let content = root.child("content").ok_or("missing <content>")?.text();
    // Content conventionally starts and ends on its own line inside the CDATA.
    let content = content.strip_prefix("\r\n").or_else(|| content.strip_prefix('\n')).unwrap_or(&content);
    let content = content.strip_suffix("\r\n").or_else(|| content.strip_suffix('\n')).unwrap_or(content);
    let field = |name: &str| root.child(name).map(|e| e.text().trim().to_owned()).unwrap_or_default();
    Ok(Snippet {
        trigger: field("tabTrigger"),
        description: field("description"),
        scope: field("scope"),
        content: content.to_owned(),
        path: None,
    })
}

struct XmlElement {
    name: String,
    /// Text runs: (from CDATA?, text).
    parts: Vec<(bool, String)>,
    children: Vec<XmlElement>,
}

impl XmlElement {
    /// Character data. Next to a CDATA section, whitespace-only runs are layout.
    fn text(&self) -> String {
        let cdata = self.parts.iter().any(|p| p.0);
        self.parts.iter().filter(|(c, t)| *c || !cdata || !t.trim().is_empty()).map(|p| p.1.as_str()).collect()
    }

    fn child(&self, name: &str) -> Option<&XmlElement> {
        self.children.iter().find(|c| c.name == name)
    }
}

/// Just enough XML for snippet files: elements, text, entities, CDATA,
/// comments, processing instructions and a doctype; attributes are skipped.
struct XmlReader<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> XmlReader<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }

    fn skip_past(&mut self, end: &str) -> Result<(), String> {
        match self.rest().find(end) {
            Some(k) => {
                self.i += k + end.len();
                Ok(())
            }
            None => Err(format!("missing \"{end}\"")),
        }
    }

    /// Whitespace, comments, `<?xml ?>` and `<!DOCTYPE>` before the root.
    fn skip_misc(&mut self) -> Result<(), String> {
        loop {
            let trimmed = self.rest().trim_start();
            self.i = self.s.len() - trimmed.len();
            if trimmed.starts_with("<!--") {
                self.skip_past("-->")?;
            } else if trimmed.starts_with("<?") {
                self.skip_past("?>")?;
            } else if trimmed.starts_with("<!") {
                self.skip_past(">")?;
            } else {
                return Ok(());
            }
        }
    }

    fn name(&mut self) -> String {
        let r = self.rest();
        let n = r.find(|c: char| c.is_whitespace() || c == '/' || c == '>').unwrap_or(r.len());
        self.i += n;
        r[..n].to_owned()
    }

    /// The element starting at `<`.
    fn element(&mut self, depth: usize) -> Result<XmlElement, String> {
        if depth > 32 {
            return Err("elements nested too deeply".into());
        }
        self.i += 1;
        let name = self.name();
        if name.is_empty() {
            return Err("a tag without a name".into());
        }
        let mut el = XmlElement { name, parts: Vec::new(), children: Vec::new() };
        // Attributes (ignored) up to the end of the start tag.
        loop {
            let Some(c) = self.rest().chars().next() else { return Err(format!("<{}> is not closed", el.name)) };
            self.i += c.len_utf8();
            match c {
                '"' | '\'' => self.skip_past(&c.to_string())?,
                '/' if self.rest().starts_with('>') => {
                    self.i += 1;
                    return Ok(el);
                }
                '>' => break,
                _ => {}
            }
        }
        loop {
            let r = self.rest();
            if r.is_empty() {
                return Err(format!("<{}> is not closed", el.name));
            } else if let Some(body) = r.strip_prefix("<![CDATA[") {
                let end = body.find("]]>").ok_or("a CDATA section is not closed")?;
                el.parts.push((true, body[..end].to_owned()));
                self.i += 9 + end + 3;
            } else if r.starts_with("<!--") {
                self.skip_past("-->")?;
            } else if r.starts_with("<?") {
                self.skip_past("?>")?;
            } else if r.starts_with("</") {
                self.i += 2;
                let close = self.name();
                self.skip_past(">")?;
                if close != el.name {
                    return Err(format!("<{}> is closed by </{close}>", el.name));
                }
                return Ok(el);
            } else if r.starts_with('<') {
                el.children.push(self.element(depth + 1)?);
            } else {
                let end = r.find('<').unwrap_or(r.len());
                el.parts.push((false, decode_entities(&r[..end])));
                self.i += end;
            }
        }
    }
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(k) = rest.find('&') {
        out.push_str(&rest[..k]);
        rest = &rest[k..];
        let decoded = rest.find(';').filter(|&e| e <= 10).and_then(|e| {
            let c = match &rest[1..e] {
                "lt" => '<',
                "gt" => '>',
                "amp" => '&',
                "quot" => '"',
                "apos" => '\'',
                n if n.starts_with("#x") || n.starts_with("#X") => char::from_u32(u32::from_str_radix(&n[2..], 16).ok()?)?,
                n if n.starts_with('#') => char::from_u32(n[1..].parse().ok()?)?,
                _ => return None,
            };
            Some((c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---- library -----------------------------------------------------------------

/// Built-in snippets: (trigger, description, scope, content).
const BUILTIN: &[(&str, &str, &str, &str)] = &[
    ("fn", "Function", "source.rust", "fn ${1:name}(${2}) {\n\t$0\n}"),
    ("impl", "Impl block", "source.rust", "impl ${1:Type} {\n\t$0\n}"),
    ("test", "Test function", "source.rust", "#[test]\nfn ${1:name}() {\n\t$0\n}"),
    ("match", "Match expression", "source.rust", "match ${1:expr} {\n\t${2:pattern} => ${3:todo!()},\n\t_ => ${0:todo!()},\n}"),
    ("struct", "Struct", "source.rust", "struct ${1:Name} {\n\t${2:field}: ${3:Type},$0\n}"),
    ("def", "Function", "source.python", "def ${1:name}(${2}):\n\t${0:pass}"),
    ("class", "Class", "source.python", "class ${1:Name}:\n\tdef __init__(self${2}):\n\t\t${0:pass}"),
    ("ifmain", "if __name__ == \"__main__\"", "source.python", "if __name__ == \"__main__\":\n\t${0:main()}"),
    ("for", "For loop", "source.python", "for ${1:item} in ${2:items}:\n\t${0:pass}"),
    ("fn", "Function", "source.js, source.ts, source.jsx, source.tsx", "function ${1:name}(${2}) {\n\t$0\n}"),
    ("arrow", "Arrow function", "source.js, source.ts, source.jsx, source.tsx", "const ${1:name} = (${2}) => {\n\t$0\n};"),
    ("log", "console.log", "source.js, source.ts, source.jsx, source.tsx", "console.log(${1:$TM_SELECTED_TEXT});$0"),
    ("imp", "Import", "source.js, source.ts, source.jsx, source.tsx", "import ${2:name} from \"${1:module}\";$0"),
    ("func", "Function", "source.go", "func ${1:name}(${2}) ${3:error} {\n\t$0\n}"),
    ("iferr", "if err != nil", "source.go", "if err != nil {\n\treturn ${1:err}\n}$0"),
    ("link", "Link", "text.html.markdown", "[${1:$TM_SELECTED_TEXT}](${2:https://})$0"),
    ("img", "Image", "text.html.markdown", "![${1:alt text}](${2:path})$0"),
    ("code", "Code fence", "text.html.markdown", "```${1:lang}\n${2:$TM_SELECTED_TEXT}\n```$0"),
    ("table", "Table", "text.html.markdown", "| ${1:Column} | ${2:Column} |\n| --- | --- |\n| ${3:value} | ${4:value} |$0"),
    (
        "html5",
        "HTML5 document",
        "text.html.basic",
        "<!DOCTYPE html>\n<html lang=\"${1:en}\">\n<head>\n\t<meta charset=\"UTF-8\">\n\t<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n\t<title>${2:Document}</title>\n</head>\n<body>\n\t$0\n</body>\n</html>",
    ),
    ("a", "Link", "text.html.basic", "<a href=\"${1:#}\">${2:$TM_SELECTED_TEXT}</a>$0"),
    ("div", "div", "text.html.basic", "<div${1: class=\"${2:name}\"}>\n\t${0:$TM_SELECTED_TEXT}\n</div>"),
    ("shebang", "#!/usr/bin/env", "source.shell", "#!/usr/bin/env ${1:bash}\n$0"),
    ("if", "if … fi", "source.shell", "if [[ ${1:condition} ]]; then\n\t$0\nfi"),
    ("for", "for … done", "source.shell", "for ${1:item} in ${2:list}; do\n\t$0\ndone"),
];

pub fn builtins() -> Vec<Snippet> {
    BUILTIN
        .iter()
        .map(|&(trigger, description, scope, content)| Snippet {
            trigger: trigger.into(),
            description: description.into(),
            scope: scope.into(),
            content: content.into(),
            path: None,
        })
        .collect()
}

/// Built-ins plus `<config_dir>/Snippets/**/*.sublime-snippet`; user
/// snippets come first so they win a tie with a built-in trigger.
pub struct Library {
    pub snippets: Vec<Snippet>,
    /// Files that didn't parse, as "path: reason".
    pub errors: Vec<String>,
    dir: Option<PathBuf>,
    /// (file, modified, size) when last loaded, to notice changes.
    stamp: Vec<(PathBuf, Option<SystemTime>, u64)>,
}

impl Default for Library {
    fn default() -> Self {
        Self::load(None)
    }
}

impl Library {
    pub fn load(config_dir: Option<&Path>) -> Self {
        let mut lib = Library { snippets: Vec::new(), errors: Vec::new(), dir: config_dir.map(|d| d.join("Snippets")), stamp: Vec::new() };
        lib.reload();
        lib
    }

    /// `<config_dir>/Snippets`, where user snippets live.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    fn scan(&self) -> Vec<(PathBuf, Option<SystemTime>, u64)> {
        let mut files = Vec::new();
        if let Some(dir) = &self.dir {
            collect_files(dir, 0, &mut files);
        }
        files
            .into_iter()
            .map(|p| {
                let meta = std::fs::metadata(&p).ok();
                let stamp = (meta.as_ref().and_then(|m| m.modified().ok()), meta.map_or(0, |m| m.len()));
                (p, stamp.0, stamp.1)
            })
            .collect()
    }

    pub fn reload(&mut self) {
        self.stamp = self.scan();
        self.errors.clear();
        self.snippets.clear();
        for (path, ..) in &self.stamp {
            let parsed = std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|s| parse_xml(&s));
            match parsed {
                Ok(mut s) => {
                    s.path = Some(path.clone());
                    self.snippets.push(s);
                }
                Err(e) => self.errors.push(format!("{}: {e}", path.display())),
            }
        }
        self.snippets.extend(builtins());
    }

    /// Reload if a snippet file was added, removed or changed. True if it did.
    pub fn refresh(&mut self) -> bool {
        if self.scan() == self.stamp {
            return false;
        }
        self.reload();
        true
    }

    /// Snippets whose scope selector matches `scopes`.
    pub fn available(&self, scopes: &[String]) -> Vec<&Snippet> {
        self.snippets.iter().filter(|s| scope_matches(&s.scope, scopes)).collect()
    }

    /// Snippets for the completion list: triggers starting with `prefix` (any case).
    pub fn completions(&self, prefix: &str, scopes: &[String]) -> Vec<&Snippet> {
        let lower = prefix.to_lowercase();
        let mut seen = HashSet::new();
        self.snippets
            .iter()
            .filter(|s| !s.trigger.is_empty() && s.trigger.to_lowercase().starts_with(&lower) && scope_matches(&s.scope, scopes))
            .filter(|s| seen.insert(s.trigger.clone()))
            .collect()
    }

    /// Snippets whose trigger the text `before` the caret ends with, at a word
    /// boundary; longest trigger first.
    pub fn ending(&self, before: &str) -> Vec<&Snippet> {
        let mut found: Vec<&Snippet> = self.snippets.iter().filter(|s| ends_with_trigger(before, &s.trigger)).collect();
        found.sort_by_key(|s| std::cmp::Reverse(s.trigger.chars().count()));
        found
    }
}

fn collect_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            if depth < 8 {
                collect_files(&p, depth + 1, out);
            }
        } else if p.extension().is_some_and(|e| e == "sublime-snippet") {
            out.push(p);
        }
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn ends_with_trigger(before: &str, trigger: &str) -> bool {
    if trigger.is_empty() || !before.ends_with(trigger) {
        return false;
    }
    let prev = before[..before.len() - trigger.len()].chars().next_back();
    !(trigger.starts_with(is_word) && prev.is_some_and(is_word))
}

// ---- scope selectors ---------------------------------------------------------

/// Does a Sublime scope selector match a scope stack (outermost first)?
/// Supports `,`/`|` alternatives, space-separated descendant paths, `-`
/// exclusion and `&` intersection; an empty selector matches everywhere.
pub fn scope_matches(selector: &str, scopes: &[String]) -> bool {
    if selector.trim().is_empty() {
        return true;
    }
    selector.split([',', '|']).any(|alt| {
        let mut terms: Vec<(bool, Vec<&str>)> = vec![(false, Vec::new())];
        for word in alt.split_whitespace() {
            let word = word.trim_matches(|c| c == '(' || c == ')');
            let (op, atom) = if let Some(rest) = word.strip_prefix('-') {
                (Some(true), rest)
            } else if let Some(rest) = word.strip_prefix('&') {
                (Some(false), rest)
            } else {
                (None, word)
            };
            if let Some(negated) = op {
                terms.push((negated, Vec::new()));
            }
            // Sublime's left/right side prefixes ("L:source.php") don't apply here.
            let atom = match atom.split_once(':') {
                Some(("L" | "R" | "B", rest)) => rest,
                _ => atom,
            };
            if !atom.is_empty() {
                terms.last_mut().expect("one term").1.push(atom);
            }
        }
        if terms.iter().all(|(_, path)| path.is_empty()) {
            return false;
        }
        terms.iter().all(|(negated, path)| {
            if path.is_empty() {
                return true;
            }
            path_matches(path, scopes) != *negated
        })
    })
}

/// Each atom matches a later scope than the one before it.
fn path_matches(path: &[&str], scopes: &[String]) -> bool {
    let mut at = 0;
    for atom in path {
        match scopes[at..].iter().position(|s| atom_matches(atom, s)) {
            Some(k) => at += k + 1,
            None => return false,
        }
    }
    true
}

fn atom_matches(atom: &str, scope: &str) -> bool {
    scope.strip_prefix(atom).is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

// ---- snippet bodies ------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Text(String),
    /// `$1`, `${1}`, `${1:placeholder}` (the body may hold more fields).
    Field { index: u32, body: Vec<Node> },
    /// `${1/regex/format/flags}`: field 1's text, transformed.
    Transform { index: u32, tr: Transform },
    /// `$NAME`, `${NAME:default}`, `${NAME/regex/format/flags}`.
    Var { name: String, default: Vec<Node>, tr: Option<Transform> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    pub regex: String,
    pub format: String,
    pub flags: String,
}

/// Parse a snippet body. Malformed `$…` sequences are kept as literal text.
pub fn parse_body(s: &str) -> Vec<Node> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    parse_nodes(&chars, &mut i, false, 0)
}

fn parse_nodes(c: &[char], i: &mut usize, nested: bool, depth: usize) -> Vec<Node> {
    let mut out = Vec::new();
    let mut text = String::new();
    while *i < c.len() {
        match c[*i] {
            '\\' if matches!(c.get(*i + 1), Some('$' | '\\' | '}')) => {
                text.push(c[*i + 1]);
                *i += 2;
            }
            '}' if nested => break,
            '$' => match parse_dollar(c, *i, depth) {
                Some((node, next)) => {
                    if !text.is_empty() {
                        out.push(Node::Text(std::mem::take(&mut text)));
                    }
                    out.push(node);
                    *i = next;
                }
                None => {
                    text.push('$');
                    *i += 1;
                }
            },
            ch => {
                text.push(ch);
                *i += 1;
            }
        }
    }
    if !text.is_empty() {
        out.push(Node::Text(text));
    }
    out
}

fn read_while(c: &[char], i: &mut usize, f: impl Fn(char) -> bool) -> String {
    let start = *i;
    while *i < c.len() && f(c[*i]) {
        *i += 1;
    }
    c[start..*i].iter().collect()
}

fn is_var_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_var_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// The `$…` construct at `start`, and where it ends.
fn parse_dollar(c: &[char], start: usize, depth: usize) -> Option<(Node, usize)> {
    let mut i = start + 1;
    let first = *c.get(i)?;
    if first.is_ascii_digit() {
        let index = read_while(c, &mut i, |ch| ch.is_ascii_digit()).parse().ok()?;
        return Some((Node::Field { index, body: Vec::new() }, i));
    }
    if is_var_start(first) {
        let name = read_while(c, &mut i, is_var_char);
        return Some((Node::Var { name, default: Vec::new(), tr: None }, i));
    }
    if first != '{' || depth > 16 {
        return None;
    }
    i += 1;
    let head = *c.get(i)?;
    let index: Option<u32> = if head.is_ascii_digit() {
        Some(read_while(c, &mut i, |ch| ch.is_ascii_digit()).parse().ok()?)
    } else if is_var_start(head) {
        None
    } else {
        return None;
    };
    let name = if index.is_none() { read_while(c, &mut i, is_var_char) } else { String::new() };
    let node = match *c.get(i)? {
        '}' => match index {
            Some(index) => Node::Field { index, body: Vec::new() },
            None => Node::Var { name, default: Vec::new(), tr: None },
        },
        ':' => {
            i += 1;
            let body = parse_nodes(c, &mut i, true, depth + 1);
            if c.get(i) != Some(&'}') {
                return None;
            }
            match index {
                Some(index) => Node::Field { index, body },
                None => Node::Var { name, default: body, tr: None },
            }
        }
        '/' => {
            i += 1;
            let regex = read_part(c, &mut i, '/')?;
            let format = read_part(c, &mut i, '/')?;
            let flags = read_while(c, &mut i, |ch| ch != '}');
            if c.get(i) != Some(&'}') {
                return None;
            }
            let tr = Transform { regex, format, flags };
            match index {
                Some(index) => Node::Transform { index, tr },
                None => Node::Var { name, default: Vec::new(), tr: Some(tr) },
            }
        }
        _ => return None,
    };
    Some((node, i + 1))
}

/// Text up to an unescaped `end` (consumed). `\/` becomes `/`; other escapes
/// are kept for the regex / format parser.
fn read_part(c: &[char], i: &mut usize, end: char) -> Option<String> {
    let mut out = String::new();
    while *i < c.len() {
        match c[*i] {
            '\\' if c.get(*i + 1) == Some(&end) => {
                out.push(end);
                *i += 2;
            }
            '\\' if *i + 1 < c.len() => {
                out.push('\\');
                out.push(c[*i + 1]);
                *i += 2;
            }
            ch if ch == end => {
                *i += 1;
                return Some(out);
            }
            ch => {
                out.push(ch);
                *i += 1;
            }
        }
    }
    None
}

/// Apply `${…/regex/format/flags}` to `value`. Flags: `g` (all matches),
/// `i`, `m`, `s`, `x`. An invalid regex leaves the value unchanged.
pub fn apply_transform(value: &str, tr: &Transform) -> String {
    let inline: String = tr.flags.chars().filter(|f| "imsx".contains(*f)).collect();
    let pattern = if inline.is_empty() { tr.regex.clone() } else { format!("(?{inline}){}", tr.regex) };
    let Ok(re) = regex::Regex::new(&pattern) else { return value.to_owned() };
    let global = tr.flags.contains('g');
    let format: Vec<char> = tr.format.chars().collect();
    let mut out = String::new();
    let mut last = 0;
    for caps in re.captures_iter(value) {
        let m = caps.get(0).expect("whole match");
        out.push_str(&value[last..m.start()]);
        let mut f = Format::default();
        let mut i = 0;
        format_seq(&format, &mut i, &caps, &mut f, &[]);
        out.push_str(&f.out);
        last = m.end();
        if !global {
            break;
        }
    }
    out.push_str(&value[last..]);
    out
}

/// Output of a format string with its case-folding state (`\u \l \U \L \E`).
#[derive(Default)]
struct Format {
    out: String,
    /// Fold the next char: Some(true) upper, Some(false) lower.
    once: Option<bool>,
    /// Fold everything until `\E`.
    mode: Option<bool>,
}

impl Format {
    fn push(&mut self, ch: char) {
        match self.once.take().or(self.mode) {
            Some(true) => self.out.extend(ch.to_uppercase()),
            Some(false) => self.out.extend(ch.to_lowercase()),
            None => self.out.push(ch),
        }
    }
}

/// Format string up to (not past) one of `stops`: `$n`, `${n}`, escapes,
/// case folding and `(?n:if:else)` conditionals.
fn format_seq(c: &[char], i: &mut usize, caps: &regex::Captures, f: &mut Format, stops: &[char]) {
    while *i < c.len() {
        let ch = c[*i];
        if stops.contains(&ch) {
            return;
        }
        match ch {
            '\\' if *i + 1 < c.len() => {
                match c[*i + 1] {
                    'n' => f.push('\n'),
                    't' => f.push('\t'),
                    'u' => f.once = Some(true),
                    'l' => f.once = Some(false),
                    'U' => f.mode = Some(true),
                    'L' => f.mode = Some(false),
                    'E' => f.mode = None,
                    other => f.push(other),
                }
                *i += 2;
            }
            '$' => {
                let mut j = *i + 1;
                let braced = c.get(j) == Some(&'{');
                if braced {
                    j += 1;
                }
                let digits = read_while(c, &mut j, |d| d.is_ascii_digit());
                let closed = !braced || c.get(j) == Some(&'}');
                match digits.parse::<usize>() {
                    Ok(n) if closed => {
                        for g in caps.get(n).map_or("", |m| m.as_str()).chars() {
                            f.push(g);
                        }
                        *i = j + usize::from(braced);
                    }
                    _ => {
                        f.push('$');
                        *i += 1;
                    }
                }
            }
            '(' if c.get(*i + 1) == Some(&'?') => {
                let mut j = *i + 2;
                let digits = read_while(c, &mut j, |d| d.is_ascii_digit());
                let (Ok(n), Some(':')) = (digits.parse::<usize>(), c.get(j)) else {
                    f.push('(');
                    *i += 1;
                    continue;
                };
                *i = j + 1;
                let matched = caps.get(n).is_some_and(|m| !m.as_str().is_empty());
                let mut skipped = Format::default();
                format_seq(c, i, caps, if matched { &mut *f } else { &mut skipped }, &[':', ')']);
                if c.get(*i) == Some(&':') {
                    *i += 1;
                    format_seq(c, i, caps, if matched { &mut skipped } else { &mut *f }, &[')']);
                }
                if c.get(*i) == Some(&')') {
                    *i += 1;
                }
            }
            _ => {
                f.push(ch);
                *i += 1;
            }
        }
    }
}

// ---- rendering -------------------------------------------------------------------

/// What a body expands to at one cursor.
pub struct Rendered {
    pub text: String,
    /// Editable fields, in document order, as char ranges of `text`.
    pub fields: Vec<RenderedField>,
    /// `${n/…/…/}` copies, refreshed when Tab leaves a field.
    pub mirrors: Vec<(u32, Range<usize>, Transform)>,
}

pub struct RenderedField {
    pub index: u32,
    pub range: Range<usize>,
    /// The field this one sits inside, if any.
    pub parent: Option<u32>,
}

/// Inputs for rendering at one cursor.
pub struct Env<'a> {
    pub vars: &'a HashMap<String, String>,
    pub newline: &'a str,
    /// Indentation added after each newline (the current line's).
    pub indent: &'a str,
    /// What a tab in the snippet becomes (spaces if translating tabs).
    pub tab: &'a str,
}

pub fn render(nodes: &[Node], env: &Env) -> Rendered {
    let mut defaults = HashMap::new();
    collect_defaults(nodes, &mut defaults);
    let mut r = Renderer { out: String::new(), len: 0, fields: Vec::new(), mirrors: Vec::new(), env, defaults: &defaults, defined: HashSet::new() };
    r.nodes(nodes, None, true, 0);
    Rendered { text: r.out, fields: r.fields, mirrors: r.mirrors }
}

/// The first placeholder given for each field number (mirrors show it too).
fn collect_defaults<'a>(nodes: &'a [Node], out: &mut HashMap<u32, &'a [Node]>) {
    for n in nodes {
        match n {
            Node::Field { index, body } => {
                if !body.is_empty() {
                    out.entry(*index).or_insert(body);
                }
                collect_defaults(body, out);
            }
            Node::Var { default, .. } => collect_defaults(default, out),
            _ => {}
        }
    }
}

struct Renderer<'a> {
    out: String,
    /// Chars in `out`.
    len: usize,
    fields: Vec<RenderedField>,
    mirrors: Vec<(u32, Range<usize>, Transform)>,
    env: &'a Env<'a>,
    defaults: &'a HashMap<u32, &'a [Node]>,
    /// Fields whose placeholder (with its nested fields) was laid down.
    defined: HashSet<u32>,
}

impl Renderer<'_> {
    fn push_raw(&mut self, s: &str) {
        self.out.push_str(s);
        self.len += s.chars().count();
    }

    /// Snippet text: newlines keep the indentation, tabs follow the tab setting.
    fn push_text(&mut self, s: &str) {
        for ch in s.chars() {
            match ch {
                '\r' => {}
                '\n' => {
                    self.push_raw(self.env.newline);
                    self.push_raw(self.env.indent);
                }
                '\t' => self.push_raw(self.env.tab),
                _ => {
                    self.out.push(ch);
                    self.len += 1;
                }
            }
        }
    }

    /// `nodes` rendered without recording fields (mirrors, transform input).
    fn flat(&self, nodes: &[Node], depth: usize) -> String {
        let mut r = Renderer { out: String::new(), len: 0, fields: Vec::new(), mirrors: Vec::new(), env: self.env, defaults: self.defaults, defined: HashSet::new() };
        r.nodes(nodes, None, false, depth);
        r.out
    }

    fn default_of(&self, index: u32) -> &[Node] {
        self.defaults.get(&index).copied().unwrap_or(&[])
    }

    fn nodes(&mut self, nodes: &[Node], parent: Option<u32>, track: bool, depth: usize) {
        if depth > 16 {
            return;
        }
        for n in nodes {
            match n {
                Node::Text(t) => self.push_text(t),
                Node::Field { index, body } => {
                    let start = self.len;
                    let defining = track && !self.defined.contains(index) && (!body.is_empty() || self.default_of(*index).is_empty());
                    if defining {
                        self.defined.insert(*index);
                        self.nodes(body, Some(*index), true, depth + 1);
                    } else {
                        let text = self.flat(if body.is_empty() { self.default_of(*index) } else { body }, depth + 1);
                        self.push_raw(&text);
                    }
                    if track {
                        self.fields.push(RenderedField { index: *index, range: start..self.len, parent });
                    }
                }
                Node::Transform { index, tr } => {
                    let source = self.flat(self.default_of(*index), depth + 1);
                    let start = self.len;
                    self.push_raw(&apply_transform(&source, tr));
                    if track {
                        self.mirrors.push((*index, start..self.len, tr.clone()));
                    }
                }
                Node::Var { name, default, tr } => {
                    let value = self.env.vars.get(name.as_str()).filter(|v| !v.is_empty());
                    match (value, tr) {
                        (v, Some(tr)) => {
                            let out = apply_transform(v.map_or("", String::as_str), tr);
                            self.push_raw(&out);
                        }
                        (Some(v), None) => {
                            let v = v.clone();
                            self.push_raw(&v);
                        }
                        (None, None) => self.nodes(default, parent, track, depth + 1),
                    }
                }
            }
        }
    }
}

// ---- expansion and field sessions ----------------------------------------------

/// The snippet whose trigger ends right before every cursor (all cursors
/// empty) and whose scope matches at the primary's trigger. `scope_at` gives
/// the scope stack at a char position.
pub fn trigger_at<'a>(lib: &'a Library, ed: &Editor, scope_at: impl Fn(usize) -> Vec<String>) -> Option<&'a Snippet> {
    if ed.selections().iter().any(|s| !s.is_empty()) {
        return None;
    }
    let before = |h: usize| {
        let ls = ed.buffer.line_start(ed.buffer.line_of(h));
        ed.buffer.slice(ls.max(h.saturating_sub(64))..h)
    };
    let head = ed.primary().head;
    lib.ending(&before(head)).into_iter().find(|s| {
        let n = s.trigger.chars().count();
        scope_matches(&s.scope, &scope_at(head - n))
            && ed.selections().iter().all(|sel| ends_with_trigger(&before(sel.head), &s.trigger))
    })
}

/// Variables for a snippet replacing `range` (the trigger or a selection).
fn variables(ed: &Editor, range: &Range<usize>, path: Option<&Path>) -> HashMap<String, String> {
    let b = &ed.buffer;
    let line = b.line_of(range.start);
    let (ls, le) = (b.line_start(line), b.line_end(line));
    let head: Vec<char> = b.slice(ls..range.start).chars().collect();
    let tail: Vec<char> = b.slice(range.end.min(le).max(range.start)..le.max(range.start)).chars().collect();
    let word_start = head.iter().rposition(|c| !is_word(*c)).map_or(0, |k| k + 1);
    let word_end = tail.iter().position(|c| !is_word(*c)).unwrap_or(tail.len());
    let word: String = head[word_start..].iter().chain(&tail[..word_end]).collect();
    let mut v = HashMap::new();
    let mut set = |k: &str, val: String| {
        v.insert(k.to_owned(), val);
    };
    if let Some(p) = path {
        set("TM_FILEPATH", p.display().to_string());
        set("TM_FILENAME", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        set("TM_DIRECTORY", p.parent().map(|d| d.display().to_string()).unwrap_or_default());
    }
    let selected = b.slice(range.clone());
    let selected = if selected.chars().count() == 0 || range.is_empty() { String::new() } else { selected };
    set("TM_SELECTED_TEXT", selected.clone());
    set("SELECTION", selected);
    set("TM_CURRENT_LINE", head.iter().chain(&tail).collect());
    set("TM_CURRENT_WORD", word);
    set("TM_LINE_INDEX", head.len().to_string());
    set("TM_LINE_NUMBER", (line + 1).to_string());
    set("TM_SOFT_TABS", if ed.use_spaces { "YES" } else { "NO" }.to_owned());
    set("TM_TAB_SIZE", ed.tab_width.to_string());
    v
}

/// Insert `content` at every cursor as one undo step, replacing each
/// selection, or the `prefix` (any case) just before an empty cursor. Returns
/// a session when there are fields to Tab through; otherwise the caret is at
/// `$0` (or the end).
pub fn expand(ed: &mut Editor, content: &str, prefix: &str, path: Option<&Path>) -> Option<Session> {
    let nodes = parse_body(content);
    let newline = ed.buffer.line_ending().as_str();
    let tab = if ed.use_spaces { " ".repeat(ed.tab_width.max(1)) } else { "\t".to_owned() };
    let prefix_lower = prefix.to_lowercase();
    let n = prefix.chars().count();
    let primary_head = ed.primary().head;
    let mut primary_cursor = 0;
    let mut edits = Vec::new();
    let mut renders = Vec::new();
    for (k, sel) in ed.selections().iter().enumerate() {
        if sel.start() <= primary_head && primary_head <= sel.end() {
            primary_cursor = k;
        }
        let range = if !sel.is_empty() {
            sel.range()
        } else {
            let h = sel.head;
            let ls = ed.buffer.line_start(ed.buffer.line_of(h));
            let own = n > 0 && h >= ls + n && ed.buffer.slice(h - n..h).to_lowercase() == prefix_lower;
            if own { h - n..h } else { h..h }
        };
        let line = ed.buffer.line_text(ed.buffer.line_of(range.start));
        let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let vars = variables(ed, &range, path);
        let env = Env { vars: &vars, newline, indent: &indent, tab: &tab };
        let r = render(&nodes, &env);
        edits.push((range, r.text.clone()));
        renders.push(r);
    }

    // Post-edit start of each cursor's snippet.
    let mut bases = Vec::new();
    let mut delta: isize = 0;
    for (range, text) in &edits {
        bases.push((range.start as isize + delta) as usize);
        delta += text.chars().count() as isize - range.len() as isize;
    }
    let mut indices: Vec<u32> = renders.iter().flat_map(|r| r.fields.iter().map(|f| f.index)).filter(|&i| i > 0).collect();
    indices.sort_unstable();
    indices.dedup();
    indices.push(0);

    let mut tracked: Vec<Range<usize>> = renders.iter().zip(&bases).map(|(r, &b)| b..b + r.text.chars().count()).collect();
    let regions = tracked.len();
    let mut groups = Vec::new();
    for &index in &indices {
        let mut occs = Vec::new();
        for (k, (r, &base)) in renders.iter().zip(&bases).enumerate() {
            let mut any = false;
            for f in r.fields.iter().filter(|f| f.index == index) {
                any = true;
                let text: String = r.text.chars().skip(f.range.start).take(f.range.len()).collect();
                occs.push(Occ { tracked: tracked.len(), cursor: k, empty: f.range.is_empty(), parent: f.parent, original: text });
                tracked.push(base + f.range.start..base + f.range.end);
            }
            if index == 0 && !any {
                let end = base + r.text.chars().count();
                occs.push(Occ { tracked: tracked.len(), cursor: k, empty: true, parent: None, original: String::new() });
                tracked.push(end..end);
            }
        }
        groups.push(Group { index, occs });
    }
    let mut mirrors = Vec::new();
    for (k, (r, &base)) in renders.iter().zip(&bases).enumerate() {
        for (index, range, tr) in &r.mirrors {
            let gi = indices.iter().position(|i| i == index);
            let source = gi.and_then(|g| groups[g].occs.iter().find(|o| o.cursor == k)).map(|o| o.tracked);
            mirrors.push(Mirror { tracked: tracked.len(), source, tr: tr.clone() });
            tracked.push(base + range.start..base + range.end);
        }
    }

    ed.replace_ranges(edits);
    ed.set_tracked(tracked);
    let session = Session { groups, current: 0, base: ed.history_len(), regions, mirrors, primary_cursor };
    if session.groups.len() == 1 {
        session.select(ed);
        ed.set_tracked(Vec::new());
        return None;
    }
    session.select(ed);
    Some(session)
}

struct Occ {
    /// Index into the editor's tracked ranges.
    tracked: usize,
    /// Which cursor's copy of the snippet it belongs to.
    cursor: usize,
    /// Started empty: the caret goes to its end instead of selecting it
    /// (empty fields soak up text typed next to them).
    empty: bool,
    parent: Option<u32>,
    original: String,
}

/// One tab stop: every copy of field n, across all cursors.
struct Group {
    index: u32,
    occs: Vec<Occ>,
}

struct Mirror {
    tracked: usize,
    /// The field copy it transforms.
    source: Option<usize>,
    tr: Transform,
}

/// Fields of an expanded snippet, in Tab order ($1, $2, …, then $0).
pub struct Session {
    groups: Vec<Group>,
    current: usize,
    /// Undo depth right after expanding; undoing past it ends the session.
    base: usize,
    /// The first `regions` tracked ranges are the inserted snippets.
    regions: usize,
    mirrors: Vec<Mirror>,
    primary_cursor: usize,
}

impl Session {
    /// Still editing fields: the expansion wasn't undone and the caret is
    /// inside a snippet.
    pub fn alive(&self, ed: &Editor) -> bool {
        let t = ed.tracked();
        let h = ed.primary().head;
        ed.history_len() >= self.base
            && t.len() == self.regions + self.groups.iter().map(|g| g.occs.len()).sum::<usize>() + self.mirrors.len()
            && t[..self.regions].iter().any(|r| r.start <= h && h <= r.end)
    }

    /// (1-based field being edited, fields before $0).
    pub fn position(&self) -> (usize, usize) {
        (self.current + 1, self.groups.len() - 1)
    }

    /// Tab: the next field. False when that was the final stop ($0).
    pub fn next(&mut self, ed: &mut Editor) -> bool {
        self.update_mirrors(ed);
        let last = self.groups.len() - 1;
        loop {
            self.current += 1;
            if self.current >= last || !self.skipped(ed, self.current) {
                break;
            }
        }
        self.current = self.current.min(last);
        self.select(ed);
        self.current < last
    }

    /// Shift+Tab: the previous field (stays on the first).
    pub fn prev(&mut self, ed: &mut Editor) {
        self.update_mirrors(ed);
        let mut c = self.current;
        while c > 0 {
            c -= 1;
            if !self.skipped(ed, c) {
                self.current = c;
                break;
            }
        }
        self.select(ed);
    }

    /// Stop following the fields.
    pub fn end(self, ed: &mut Editor) {
        ed.set_tracked(Vec::new());
    }

    fn text(ed: &Editor, r: &Range<usize>) -> String {
        let len = ed.buffer.len_chars();
        ed.buffer.slice(r.start.min(len)..r.end.min(len))
    }

    /// A nested field whose enclosing placeholder was typed over is gone.
    fn skipped(&self, ed: &Editor, g: usize) -> bool {
        let t = ed.tracked();
        self.groups[g].occs.iter().all(|o| {
            let parent = o.parent.and_then(|p| self.groups.iter().find(|g| g.index == p)).and_then(|g| g.occs.iter().find(|po| po.cursor == o.cursor));
            parent.is_some_and(|po| Self::text(ed, &t[po.tracked]) != po.original)
        })
    }

    fn select(&self, ed: &mut Editor) {
        let t = ed.tracked().to_vec();
        let occs = &self.groups[self.current].occs;
        let ranges: Vec<Range<usize>> = occs.iter().map(|o| if o.empty { t[o.tracked].end..t[o.tracked].end } else { t[o.tracked].clone() }).collect();
        let primary = occs.iter().position(|o| o.cursor == self.primary_cursor).unwrap_or(0);
        ed.select_ranges(&ranges, primary);
    }

    /// Recompute `${n/…/…/}` copies from their fields.
    fn update_mirrors(&self, ed: &mut Editor) {
        let t = ed.tracked().to_vec();
        let mut edits: Vec<(Range<usize>, String)> = self
            .mirrors
            .iter()
            .filter_map(|m| {
                let source = m.source.map(|s| Self::text(ed, &t[s])).unwrap_or_default();
                let new = apply_transform(&source, &m.tr);
                (new != Self::text(ed, &t[m.tracked])).then(|| (t[m.tracked].clone(), new))
            })
            .collect();
        edits.sort_by_key(|(r, _)| r.start);
        let mut end = 0;
        edits.retain(|(r, _)| {
            let ok = r.start >= end;
            end = end.max(r.end);
            ok
        });
        if !edits.is_empty() {
            ed.replace_ranges(edits);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    fn field(index: u32, body: Vec<Node>) -> Node {
        Node::Field { index, body }
    }

    fn text(s: &str) -> Node {
        Node::Text(s.into())
    }

    fn editor(src: &str, cursors: &[usize]) -> Editor {
        let mut ed = Editor::new(Buffer::from_str(src));
        let ranges: Vec<Range<usize>> = cursors.iter().map(|&c| c..c).collect();
        ed.select_ranges(&ranges, 0);
        ed
    }

    fn all(ed: &Editor) -> String {
        ed.buffer.slice(0..ed.buffer.len_chars())
    }

    fn selected(ed: &Editor) -> Vec<String> {
        ed.selections().iter().map(|s| ed.buffer.slice(s.range())).collect()
    }

    fn scopes(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    // ---- body parser

    #[test]
    fn parses_fields_placeholders_and_nesting() {
        assert_eq!(parse_body("a $1 ${2} b"), vec![text("a "), field(1, vec![]), text(" "), field(2, vec![]), text(" b")]);
        assert_eq!(
            parse_body("${1:foo ${2:bar}}$0"),
            vec![field(1, vec![text("foo "), field(2, vec![text("bar")])]), field(0, vec![])]
        );
        assert_eq!(parse_body("${10:x}"), vec![field(10, vec![text("x")])]);
    }

    #[test]
    fn parses_variables_transforms_and_escapes() {
        assert_eq!(parse_body("$TM_FILENAME"), vec![Node::Var { name: "TM_FILENAME".into(), default: vec![], tr: None }]);
        assert_eq!(
            parse_body("${TM_SELECTED_TEXT:${1:x}}"),
            vec![Node::Var { name: "TM_SELECTED_TEXT".into(), default: vec![field(1, vec![text("x")])], tr: None }]
        );
        let tr = Transform { regex: r"(\w+)\.rs".into(), format: "$1".into(), flags: "g".into() };
        assert_eq!(parse_body(r"${TM_FILENAME/(\w+)\.rs/$1/g}"), vec![Node::Var { name: "TM_FILENAME".into(), default: vec![], tr: Some(tr) }]);
        let tr = Transform { regex: "a/b".into(), format: "c".into(), flags: String::new() };
        assert_eq!(parse_body(r"${1/a\/b/c/}"), vec![Node::Transform { index: 1, tr }]);
        assert_eq!(parse_body(r"\$1 \\ \} ${1:a\}b}"), vec![text(r"$1 \ } "), field(1, vec![text("a}b")])]);
        // Malformed constructs stay literal.
        assert_eq!(parse_body("$ ${ ${1:open $5.00"), vec![text("$ ${ ${1:open "), field(5, vec![]), text(".00")]);
        assert_eq!(parse_body("price: ${x"), vec![text("price: ${x")]);
    }

    #[test]
    fn transforms_support_groups_case_and_conditionals() {
        let t = |regex: &str, format: &str, flags: &str| Transform { regex: regex.into(), format: format.into(), flags: flags.into() };
        assert_eq!(apply_transform("my_file.rs", &t(r"(\w+)\.rs", "$1", "")), "my_file");
        assert_eq!(apply_transform("a-b-c", &t("-", "_", "g")), "a_b_c");
        assert_eq!(apply_transform("a-b-c", &t("-", "_", "")), "a_b-c");
        assert_eq!(apply_transform("hello world", &t(r"^(\w)(\w*)", r"\u$1\U$2\E!", "")), "HELLO! world");
        assert_eq!(apply_transform("ABC", &t("abc", "x", "i")), "x");
        assert_eq!(apply_transform("name", &t("(n)?(x)?", "(?1:yes:no)(?2:YES:NO)", "")), "yesNOame");
        assert_eq!(apply_transform("keep", &t("(", "x", "")), "keep", "a bad regex changes nothing");
    }

    // ---- rendering

    fn render_with(body: &str, vars: &[(&str, &str)]) -> Rendered {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        render(&parse_body(body), &Env { vars: &vars, newline: "\n", indent: "  ", tab: "    " })
    }

    #[test]
    fn renders_mirrors_variables_and_indentation() {
        let r = render_with("<${1:div}>\n\t$0\n</$1>", &[]);
        assert_eq!(r.text, "<div>\n      \n  </div>");
        let ranges: Vec<(u32, Range<usize>)> = r.fields.iter().map(|f| (f.index, f.range.clone())).collect();
        assert_eq!(ranges, vec![(1, 1..4), (0, 12..12), (1, 17..20)]);

        let r = render_with("// ${TM_FILENAME/\\.rs$//} by ${USER:someone} ${TM_SELECTED_TEXT:${1:x}}", &[("TM_FILENAME", "main.rs")]);
        assert_eq!(r.text, "// main by someone x");
        assert_eq!(r.fields.len(), 1, "the default's field is live");

        let r = render_with("${1:a ${2:b}} $1", &[]);
        assert_eq!(r.text, "a b a b");
        assert_eq!(r.fields.iter().find(|f| f.index == 2).unwrap().parent, Some(1));
        assert_eq!(r.fields.len(), 3, "the mirror's copy of $2 isn't a field");

        let r = render_with("${1:$1}", &[]);
        assert_eq!(r.text, "", "self-reference terminates");
    }

    // ---- XML

    #[test]
    fn parses_sublime_snippet_files() {
        let src = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- a comment -->
<snippet>
    <content><![CDATA[
if ${1:x} < 3 && y > 2 {
	$0
}
]]></content>
    <tabTrigger>if</tabTrigger>
    <scope>source.rust - comment</scope>
    <description>If &amp; only &#105;f &lt;3</description>
</snippet>"#;
        let s = parse_xml(src).unwrap();
        assert_eq!(s.content, "if ${1:x} < 3 && y > 2 {\n\t$0\n}");
        assert_eq!(s.trigger, "if");
        assert_eq!(s.scope, "source.rust - comment");
        assert_eq!(s.description, "If & only if <3");

        let plain = parse_xml("<snippet><content>a &lt;b&gt;</content><tabTrigger attr='x'>t</tabTrigger><extra/></snippet>").unwrap();
        assert_eq!((plain.content.as_str(), plain.trigger.as_str(), plain.scope.as_str()), ("a <b>", "t", ""));

        assert!(parse_xml("<snippet><tabTrigger>x</tabTrigger></snippet>").unwrap_err().contains("content"));
        assert!(parse_xml("<snippet><content>x</snippet>").is_err());
        assert!(parse_xml("<plist></plist>").unwrap_err().contains("<plist>"));
        assert!(parse_xml("").is_err());
    }

    #[test]
    fn library_loads_user_files_and_notices_changes() {
        let dir = std::env::temp_dir().join(format!("strata-snippets-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let snippets = dir.join("Snippets/nested");
        std::fs::create_dir_all(&snippets).unwrap();
        let file = snippets.join("a.sublime-snippet");
        std::fs::write(&file, "<snippet><content>mine</content><tabTrigger>fn</tabTrigger><scope>source.rust</scope></snippet>").unwrap();
        std::fs::write(snippets.join("bad.sublime-snippet"), "<snippet>").unwrap();
        std::fs::write(snippets.join("ignored.txt"), "x").unwrap();
        let mut lib = Library::load(Some(&dir));
        assert_eq!(lib.errors.len(), 1);
        assert!(lib.errors[0].contains("bad.sublime-snippet"));
        let rust = scopes("source.rust");
        assert_eq!(lib.ending("let fn").first().unwrap().content, "mine", "user snippets win over built-ins");
        assert_eq!(lib.ending("let fn").first().unwrap().path.as_deref(), Some(file.as_path()));
        assert!(lib.available(&rust).iter().any(|s| s.trigger == "impl"));
        assert!(!lib.available(&rust).iter().any(|s| s.trigger == "def"));
        assert!(!lib.refresh());

        std::fs::write(&file, "<snippet><content>changed!</content><tabTrigger>fn</tabTrigger></snippet>").unwrap();
        assert!(lib.refresh());
        assert_eq!(lib.ending("fn")[0].content, "changed!");
        std::fs::remove_file(&file).unwrap();
        assert!(lib.refresh());
        assert!(lib.ending("fn").iter().all(|s| s.path.is_none()));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn triggers_need_a_word_boundary() {
        let lib = Library::default();
        assert!(lib.ending("x = fn").iter().any(|s| s.trigger == "fn"));
        assert!(lib.ending("myfn").is_empty());
        assert!(lib.ending("").is_empty());
        assert_eq!(lib.ending("ifmain")[0].trigger, "ifmain");
    }

    // ---- selectors

    #[test]
    fn scope_selectors() {
        let rs = scopes("source.rust meta.function.rust string.quoted.double.rust");
        assert!(scope_matches("", &rs));
        assert!(scope_matches("source.rust", &rs));
        assert!(scope_matches("source", &rs));
        assert!(!scope_matches("source.rus", &rs), "atoms match whole dotted parts");
        assert!(scope_matches("source.python, source.rust", &rs));
        assert!(scope_matches("source.python | source.rust", &rs));
        assert!(scope_matches("source.rust string", &rs));
        assert!(!scope_matches("string source.rust", &rs), "descendants keep their order");
        assert!(!scope_matches("source.rust - string", &rs));
        assert!(scope_matches("source.rust - comment", &rs));
        assert!(!scope_matches("- string", &rs));
        assert!(scope_matches("source.rust & meta.function", &rs));
        assert!(!scope_matches("source.rust & comment", &rs));
        assert!(scope_matches("L:source.rust", &rs));
        assert!(scope_matches("(source.rust)", &rs));
        assert!(scope_matches("source.objective-c, source.rust", &rs), "hyphens inside names aren't exclusions");
        assert!(!scope_matches(",", &rs));
    }

    // ---- expansion on an editor

    #[test]
    fn expands_and_tabs_through_fields() {
        let mut ed = editor("    fn", &[6]);
        let mut s = expand(&mut ed, "fn ${1:name}(${2:args}) {\n\t$0\n}", "fn", None).unwrap();
        assert_eq!(all(&ed), "    fn name(args) {\n        \n    }");
        assert_eq!(selected(&ed), ["name"]);
        ed.insert_text("run");
        assert!(s.alive(&ed));
        assert!(s.next(&mut ed));
        assert_eq!(selected(&ed), ["args"]);
        ed.backspace(false);
        s.prev(&mut ed);
        assert_eq!(selected(&ed), ["run"]);
        assert!(s.next(&mut ed));
        assert!(!s.next(&mut ed), "the last stop is $0");
        assert_eq!(ed.primary().head, "    fn run() {\n        ".chars().count());
        s.end(&mut ed);
        assert!(ed.tracked().is_empty());
        for _ in 0..3 {
            ed.undo();
        }
        assert_eq!(all(&ed), "    fn", "the expansion is one undo step");
    }

    #[test]
    fn mirrors_are_edited_together() {
        let mut ed = editor("", &[0]);
        let mut s = expand(&mut ed, "<${1:p}>$2</$1>", "", None).unwrap();
        assert_eq!(ed.selections().len(), 2);
        ed.insert_text("span");
        assert_eq!(all(&ed), "<span></span>");
        assert!(s.next(&mut ed));
        assert_eq!(ed.primary().head, 6);
        ed.insert_text("hi");
        assert!(!s.next(&mut ed));
        assert_eq!(all(&ed), "<span>hi</span>");
        assert_eq!(ed.primary().head, 15, "implicit $0 at the end");
    }

    #[test]
    fn transform_mirrors_update_when_tab_leaves_the_field() {
        let mut ed = editor("", &[0]);
        let mut s = expand(&mut ed, "${1:name} -> ${1/(.*)/\\U$1/}$0", "", None).unwrap();
        assert_eq!(all(&ed), "name -> NAME");
        ed.insert_text("abc");
        assert!(!s.next(&mut ed));
        assert_eq!(all(&ed), "abc -> ABC");
    }

    #[test]
    fn expands_at_every_cursor() {
        let mut ed = editor("log\nlog\n", &[3, 7]);
        let mut s = expand(&mut ed, "console.log(${1:x});$0", "log", None).unwrap();
        assert_eq!(all(&ed), "console.log(x);\nconsole.log(x);\n");
        assert_eq!(selected(&ed), ["x", "x"]);
        ed.insert_text("value");
        assert!(!s.next(&mut ed));
        assert_eq!(all(&ed), "console.log(value);\nconsole.log(value);\n");
        let heads: Vec<usize> = ed.selections().iter().map(|s| s.head).collect();
        assert_eq!(heads, [19, 39]);
    }

    #[test]
    fn nested_fields_vanish_with_their_placeholder() {
        let mut ed = editor("", &[0]);
        let mut s = expand(&mut ed, "<div${1: class=\"${2:c}\"}>$3</div>", "", None).unwrap();
        assert_eq!(selected(&ed), [" class=\"c\""]);
        assert!(s.next(&mut ed));
        assert_eq!(selected(&ed), ["c"], "kept placeholder: its nested field is next");
        s.prev(&mut ed);
        ed.backspace(false);
        assert!(s.next(&mut ed));
        assert_eq!(ed.primary().head, 5, "typed over: $2 is skipped for $3");
    }

    #[test]
    fn variables_tabs_and_selection() {
        let mut ed = editor("\tlet word = 1;", &[0]);
        ed.use_spaces = false;
        ed.select_ranges(&[5..9], 0);
        let path = Path::new("/src/demo/main.rs");
        let s = expand(&mut ed, "[$TM_SELECTED_TEXT|$TM_FILENAME|$TM_LINE_NUMBER|$TM_LINE_INDEX|${TM_FILENAME/\\.rs//}]\n\tx", "", Some(path));
        assert!(s.is_none(), "no fields: just inserted");
        assert_eq!(all(&ed), "\tlet [word|main.rs|1|5|main]\n\t\tx = 1;");
        assert_eq!(ed.primary().head, all(&ed).find(" = 1;").unwrap());

        let mut ed = editor("ab cd", &[3]);
        expand(&mut ed, "<$TM_CURRENT_WORD|$TM_CURRENT_LINE>", "", None);
        assert_eq!(all(&ed), "ab <cd|ab cd>cd");
    }

    #[test]
    fn undo_or_leaving_ends_the_session() {
        let mut ed = editor("x\n", &[0]);
        let s = expand(&mut ed, "a(${1:b})$0", "", None).unwrap();
        assert!(s.alive(&ed));
        ed.insert_text("q");
        ed.undo();
        assert!(s.alive(&ed), "undoing typing inside the field is fine");
        ed.undo();
        assert!(!s.alive(&ed), "undoing the expansion ends it");

        let mut ed = editor("x\n", &[0]);
        let s = expand(&mut ed, "a(${1:b})$0", "", None).unwrap();
        ed.select_ranges(&[7..7], 0);
        assert!(!s.alive(&ed), "the caret left the snippet");
    }

    #[test]
    fn trigger_lookup_checks_scope_and_every_cursor() {
        let lib = Library::default();
        let rust = |_: usize| scopes("source.rust");
        let ed = editor("fn", &[2]);
        assert_eq!(trigger_at(&lib, &ed, rust).map(|s| s.description.as_str()), Some("Function"));
        assert!(trigger_at(&lib, &ed, |_| scopes("source.python")).is_none());
        let ed = editor("fn\nfn\n", &[2, 5]);
        assert!(trigger_at(&lib, &ed, rust).is_some());
        let ed = editor("fn\nxx\n", &[2, 5]);
        assert!(trigger_at(&lib, &ed, rust).is_none(), "every cursor needs the trigger");
        let mut ed = editor("fn", &[2]);
        ed.select_ranges(&[0..2], 0);
        assert!(trigger_at(&lib, &ed, rust).is_none(), "a selection indents instead");
    }

    #[test]
    fn every_builtin_renders() {
        for s in builtins() {
            let r = render_with(&s.content, &[]);
            assert!(!r.text.contains("${"), "{}: {}", s.trigger, r.text);
            assert!(!s.description.is_empty() && !s.scope.is_empty());
        }
    }
}
