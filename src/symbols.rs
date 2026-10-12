//! Goto Symbol: line-based, regex-heuristic symbol extraction keyed by
//! syntect syntax name. Not a parser; it aims for the useful 95%.

use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// "function", "method", "class", "struct", "enum", "trait", "impl",
    /// "interface", "type", "module", "macro", "const", "static",
    /// "heading", "selector", "key", ...
    pub kind: &'static str,
    /// 0-based.
    pub line: usize,
}

/// Symbols in `text`, in line order, for the syntax named `syntax_name`.
pub fn symbols(syntax_name: &str, text: &str) -> Vec<Symbol> {
    match lang_for(syntax_name) {
        Some(Special::Markdown) => markdown(text),
        Some(Special::Rules(lang)) => by_rules(lang, text),
        None => by_rules(&GENERIC, text),
    }
}

/// A line rule: the `name` group is the symbol; `kind` is fixed, or when
/// empty derived from the `kw` group.
struct Rule {
    re: Regex,
    kind: &'static str,
}

struct Lang {
    rules: Vec<Rule>,
    /// Lines starting with these (after indentation) are skipped.
    comments: &'static [&'static str],
    /// Reject names and leading words that are control-flow keywords.
    keyword_filter: bool,
}

impl Lang {
    fn new(rules: &[(&str, &'static str)], comments: &'static [&'static str], keyword_filter: bool) -> Self {
        let rules = rules.iter().map(|&(re, kind)| Rule { re: Regex::new(re).expect("symbol regex"), kind }).collect();
        Self { rules, comments, keyword_filter }
    }
}

enum Special {
    Markdown,
    Rules(&'static Lang),
}

fn lang_for(syntax: &str) -> Option<Special> {
    let s = syntax.to_ascii_lowercase();
    let lang: &'static LazyLock<Lang> = match s.as_str() {
        "rust" => &RUST,
        "python" | "cython" => &PYTHON,
        "javascript" | "javascript (babel)" | "jsx" | "typescript" | "typescriptreact" | "tsx" | "json" => &JS,
        "go" => &GO,
        "c" | "c++" | "objective-c" | "objective-c++" | "cuda" => &C,
        "java" | "c#" | "kotlin" | "scala" | "groovy" | "dart" => &JAVA,
        "ruby" | "ruby on rails" | "crystal" => &RUBY,
        "php" | "php source" => &PHP,
        "lua" => &LUA,
        "swift" => &SWIFT,
        "css" | "scss" | "sass" | "less" => &CSS,
        "yaml" => &YAML,
        "markdown" | "multimarkdown" | "markdown gfm" => return Some(Special::Markdown),
        _ if s.contains("shell") || s.contains("bash") || s.contains("zsh") || s == "sh" => &SHELL,
        _ if s.starts_with("javascript") || s.starts_with("typescript") => &JS,
        _ if s.starts_with("markdown") => return Some(Special::Markdown),
        _ => return None,
    };
    Some(Special::Rules(lang))
}

fn kw_kind(kw: &str) -> &'static str {
    match kw.split_whitespace().next().unwrap_or("") {
        "struct" => "struct",
        "class" | "actor" | "@implementation" | "implementation" | "object" => "class",
        "enum" => "enum",
        "union" => "union",
        "interface" | "@interface" => "interface",
        "protocol" | "@protocol" => "protocol",
        "trait" => "trait",
        "record" => "record",
        "extension" => "extension",
        "type" => "type",
        "module" | "mod" | "namespace" => "module",
        "const" => "const",
        "static" => "static",
        _ => "function",
    }
}

const KEYWORDS: &[&str] = &[
    "if", "else", "for", "foreach", "while", "do", "switch", "case", "catch", "return", "throw", "new", "delete",
    "sizeof", "typeof", "await", "yield", "goto", "using", "until", "unless", "elif", "with", "match", "loop",
];

fn by_rules(lang: &Lang, text: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    for (line, content) in text.lines().enumerate() {
        let trimmed = content.trim_start();
        if trimmed.is_empty() || lang.comments.iter().any(|c| trimmed.starts_with(c)) {
            continue;
        }
        if lang.keyword_filter {
            let first = trimmed.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap_or("");
            if KEYWORDS.contains(&first) {
                continue;
            }
        }
        for rule in &lang.rules {
            let Some(caps) = rule.re.captures(content) else { continue };
            let Some(name) = caps.name("name") else { continue };
            let name = name.as_str().trim();
            if name.is_empty() || (lang.keyword_filter && KEYWORDS.contains(&name)) {
                continue;
            }
            let kind = if rule.kind.is_empty() { caps.name("kw").map_or("function", |k| kw_kind(k.as_str())) } else { rule.kind };
            out.push(Symbol { name: name.to_owned(), kind, line });
            break;
        }
    }
    out
}

/// ATX and setext headings, skipping fenced code and YAML front matter.
fn markdown(text: &str) -> Vec<Symbol> {
    static ATX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ {0,3}#{1,6}(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$").unwrap());
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    let mut front_matter = false;
    let mut prev: Option<(usize, &str)> = None;
    for (line, content) in text.lines().enumerate() {
        let trimmed = content.trim();
        if line == 0 && trimmed == "---" {
            front_matter = true;
            continue;
        }
        if front_matter {
            front_matter = !(trimmed == "---" || trimmed == "...");
            continue;
        }
        let lead = content.trim_start();
        if let Some(f) = fence {
            if lead.starts_with(f) && lead.trim_start_matches(&f[..1]).trim().is_empty() {
                fence = None;
            }
            continue;
        }
        if lead.starts_with("```") || lead.starts_with("~~~") {
            fence = Some(&lead[..3]);
            prev = None;
            continue;
        }
        if let Some(caps) = ATX.captures(content) {
            let name = caps.get(1).map_or("", |m| m.as_str());
            if !name.is_empty() {
                out.push(Symbol { name: name.to_owned(), kind: "heading", line });
            }
            prev = None;
            continue;
        }
        let underline = !trimmed.is_empty() && (trimmed.bytes().all(|b| b == b'=') || (trimmed.len() >= 2 && trimmed.bytes().all(|b| b == b'-')));
        if underline && content.len() - lead.len() < 4 && let Some((l, name)) = prev.take() {
            out.push(Symbol { name: name.to_owned(), kind: "heading", line: l });
            continue;
        }
        let paragraph = !trimmed.is_empty() && content.len() - lead.len() < 4 && !lead.starts_with(['-', '*', '+', '>', '|', '<']);
        prev = paragraph.then_some((line, trimmed));
    }
    out
}

// Shared fragments.
const ID: &str = r"[A-Za-z_][A-Za-z0-9_]*";
const JS_ID: &str = r"[A-Za-z_$][\w$]*";

fn p(s: &str) -> String {
    s.replace("{ID}", ID).replace("{JS_ID}", JS_ID)
}

macro_rules! lang {
    ($name:ident, [$(($re:expr, $kind:expr)),* $(,)?], $comments:expr, $filter:expr) => {
        static $name: LazyLock<Lang> = LazyLock::new(|| {
            let rules: Vec<(String, &'static str)> = vec![$((p($re), $kind)),*];
            let refs: Vec<(&str, &'static str)> = rules.iter().map(|(r, k)| (r.as_str(), *k)).collect();
            Lang::new(&refs, $comments, $filter)
        });
    };
}

const RUST_VIS: &str = r"(?:pub(?:\s*\([^)]*\))?\s+)?";

lang!(RUST, [
    (&format!(r"^\s*{RUST_VIS}(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+(?:{q}[^{q}]*{q}\s+)?)?fn\s+(?P<name>{{ID}})", q = '"'), "function"),
    (&format!(r"^\s*{RUST_VIS}(?:unsafe\s+)?(?:auto\s+)?(?P<kw>struct|enum|union|trait)\s+(?P<name>{{ID}})"), ""),
    (r"^\s*(?:unsafe\s+)?impl\b(?:\s*<[^{]*?>)?\s+(?P<name>[^{]+?)\s*(?:\bwhere\b.*)?\{?\s*$", "impl"),
    (&format!(r"^\s*{RUST_VIS}mod\s+(?P<name>{{ID}})"), "module"),
    (r"^\s*(?:#\[macro_export\]\s*)?macro_rules!\s*(?P<name>{ID})", "macro"),
    (&format!(r"^(?:{RUST_VIS}|\s+pub(?:\s*\([^)]*\))?\s+)(?P<kw>const|static)\s+(?:mut\s+)?(?P<name>{{ID}})\s*:"), ""),
    (&format!(r"^(?:{RUST_VIS}|\s+pub(?:\s*\([^)]*\))?\s+)type\s+(?P<name>{{ID}})"), "type"),
], &["//"], false);

lang!(PYTHON, [
    (r"^\s*(?:async\s+)?def\s+(?P<name>{ID})", "function"),
    (r"^\s*class\s+(?P<name>{ID})", "class"),
], &["#"], false);

const JS_MODS: &str = r"(?:(?:public|private|protected|static|async|readonly|override|abstract|get|set|declare)\s+)*";

lang!(JS, [
    (r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\b\s*\*?\s*(?P<name>{JS_ID})", "function"),
    (r"^\s*(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:abstract\s+)?(?P<kw>class|interface|enum)\s+(?P<name>{JS_ID})", ""),
    (r"^\s*(?:export\s+)?(?:declare\s+)?type\s+(?P<name>{JS_ID})\s*(?:<[^=]*>)?\s*=", "type"),
    (r"^\s*(?:export\s+)?(?:const|let|var)\s+(?P<name>{JS_ID})\s*(?::[^=]+)?=\s*(?:async\s+)?(?:function\b|(?:<[^>]*>\s*)?\([^)]*\)\s*(?::[^=]+)?=>|{JS_ID}\s*=>)", "function"),
    (r"^\s*(?:export\s+)?namespace\s+(?P<name>[\w$.]+)", "module"),
    (&format!(r"^\s+{JS_MODS}\*?(?P<name>{{JS_ID}})\s*(?:<[^>]*>)?\s*\([^)]*\)\s*(?::\s*[^{{=;]+)?\{{\s*(?:\}}\s*)?$"), "method"),
], &["//", "/*", "*"], true);

lang!(GO, [
    (r"^func\s+\([^)]*\)\s*(?P<name>{ID})", "method"),
    (r"^func\s+(?P<name>{ID})", "function"),
    (r"^(?:type\s+|\s+)(?P<name>{ID})(?:\[[^\]]*\])?\s+(?P<kw>struct|interface)\b", ""),
    (r"^type\s+(?P<name>{ID})\b", "type"),
], &["//"], false);

lang!(C, [
    (r"^\s*#\s*define\s+(?P<name>{ID})", "macro"),
    (r"^\s*(?P<kw>@interface|@implementation|@protocol)\s+(?P<name>{ID})", ""),
    (r"^[-+]\s*\([^)]*\)\s*(?P<name>{ID}(?::\s*\([^)]*\)\s*{ID}(?:\s+{ID}:\s*\([^)]*\)\s*{ID})*)?)", "method"),
    (r"^\s*(?:typedef\s+)?(?:template\s*<.*>\s*)?(?P<kw>struct|class|union|enum)(?:\s+(?:class|struct))?\s+(?:[A-Z_][A-Z0-9_]*\s+)?(?P<name>{ID})\s*(?:final\s*)?(?::[^;]*)?(?:\{[^;]*)?$", ""),
    (r"^\s*namespace\s+(?P<name>[\w:]+)\s*\{?\s*$", "module"),
    (r"^(?:[A-Za-z_][\w:<>,*&\s]*?[\s*&])?(?P<name>~?{ID}(?:::~?{ID})*|operator\s*\S+)\s*\([^;]*$", "function"),
], &["//", "/*", "*"], true);

const JAVA_MODS: &str = r"(?:(?:public|private|protected|internal|static|abstract|final|sealed|partial|readonly|unsafe|new|strictfp|non-sealed|data|open|inner|virtual|override|async|synchronized|native|extern|default|suspend|@\w+(?:\([^)]*\))?)\s+)*";

lang!(JAVA, [
    (&format!(r"^\s*{JAVA_MODS}(?P<kw>class|interface|@interface|enum|record|struct|object|trait)\s+(?P<name>{{ID}})"), ""),
    (&format!(r"^\s*{JAVA_MODS}fun\s+(?:<[^>]*>\s*)?(?:[\w.]+\.)?(?P<name>{{ID}})"), "function"),
    (&format!(r"^\s*{JAVA_MODS}def\s+(?P<name>{{ID}})"), "function"),
    (&format!(r"^\s+{JAVA_MODS}(?:<[^>]+>\s+)?(?:[\w<>\[\],.?]+(?:\s*<[^=;(]*>)?(?:\[\])*\s+)?(?P<name>{{ID}})\s*\([^;=]*$"), "method"),
], &["//", "/*", "*"], true);

lang!(RUBY, [
    (r"^\s*def\s+(?P<name>(?:self\.)?[\w.]+[?!=]?|[^\s(]+)", "method"),
    (r"^\s*class\s+(?P<name>[A-Z][\w:]*)", "class"),
    (r"^\s*module\s+(?P<name>[A-Z][\w:]*)", "module"),
], &["#"], false);

lang!(SHELL, [
    (r"^\s*function\s+(?P<name>[\w.:-]+)", "function"),
    (r"^\s*(?P<name>[\w.:-]+)\s*\(\s*\)", "function"),
], &["#"], false);

lang!(PHP, [
    (r"^\s*(?:(?:public|private|protected|static|abstract|final)\s+)*function\s+&?(?P<name>{ID})", "function"),
    (r"^\s*(?:(?:abstract|final|readonly)\s+)*(?P<kw>class|interface|trait|enum)\s+(?P<name>{ID})", ""),
    (r"^\s*namespace\s+(?P<name>[\w\\]+)", "module"),
], &["//", "/*", "*", "#"], false);

lang!(LUA, [
    (r"^\s*(?:local\s+)?function\s+(?P<name>[\w.:]+)", "function"),
    (r"^\s*(?:local\s+)?(?P<name>[\w.]+)\s*=\s*function\b", "function"),
], &["--"], false);

lang!(SWIFT, [
    (r"^\s*(?:(?:public|private|fileprivate|internal|open|static|class|final|override|mutating|nonmutating|convenience|required|@\w+)\s+)*func\s+(?P<name>[^\s(<]+)", "function"),
    (r"^\s*(?:(?:public|private|fileprivate|internal|open|final|indirect|@\w+)\s+)*(?P<kw>class|struct|enum|protocol|extension|actor)\s+(?P<name>[\w.]+)", ""),
    (r"^\s*(?:(?:public|private|fileprivate|internal|open|convenience|required|override)\s+)*(?P<name>init[?!]?)\s*\(", "method"),
], &["//", "/*", "*"], false);

lang!(CSS, [
    (r"^(?P<name>@media[^{]*|@keyframes\s+[\w-]+|[^\s{}@/;][^{};]*?)\s*\{", "selector"),
], &["/*", "*", "//"], false);

lang!(YAML, [
    (r#"^(?P<name>[A-Za-z0-9_"'$][^:#]*?)\s*:(?:\s|$)"#, "key"),
], &["#", "-", "%"], false);

lang!(GENERIC, [
    (r"^\s*(?:(?:export|public|private|protected|static|async|pub|local|default)\s+)*(?P<kw>function|func|fn|def|sub|proc|procedure|class|struct|interface|module)\s+(?P<name>[A-Za-z_$][\w$.:]*)", ""),
], &["//", "#", "--", ";"], false);

#[cfg(test)]
mod tests {
    use super::*;

    fn syms(syntax: &str, text: &str) -> Vec<(String, &'static str, usize)> {
        symbols(syntax, text).into_iter().map(|s| (s.name, s.kind, s.line)).collect()
    }

    fn names(syntax: &str, text: &str) -> Vec<String> {
        symbols(syntax, text).into_iter().map(|s| s.name).collect()
    }

    fn s(name: &str, kind: &'static str, line: usize) -> (String, &'static str, usize) {
        (name.to_owned(), kind, line)
    }

    #[test]
    fn rust() {
        let src = r#"
pub mod editor;
mod tests {
}
pub(crate) struct Buffer<T> {
    len: usize,
}
enum Mode { A, B }
pub trait Draw {
    fn draw(&self);
}
impl<T: Into<String>> Buffer<T> {
    pub async fn load(&mut self) -> Result<(), E> {
        let x = 1;
        const LOCAL: u8 = 1;
    }
}
impl fmt::Display for Mode {
unsafe extern "C" fn raw() {}
macro_rules! hello {
pub const MAX: usize = 10;
static mut COUNTER: u32 = 0;
pub type Result<T> = std::result::Result<T, Error>;
// fn commented() {}
"#;
        assert_eq!(syms("Rust", src), vec![
            s("editor", "module", 1),
            s("tests", "module", 2),
            s("Buffer", "struct", 4),
            s("Mode", "enum", 7),
            s("Draw", "trait", 8),
            s("draw", "function", 9),
            s("Buffer<T>", "impl", 11),
            s("load", "function", 12),
            s("fmt::Display for Mode", "impl", 17),
            s("raw", "function", 18),
            s("hello", "macro", 19),
            s("MAX", "const", 20),
            s("COUNTER", "static", 21),
            s("Result", "type", 22),
        ]);
    }

    #[test]
    fn python() {
        let src = "class Foo(Base):\n    def __init__(self):\n        pass\n    async def run(self):\n# def no():\ndef top():\n";
        assert_eq!(syms("Python", src), vec![
            s("Foo", "class", 0),
            s("__init__", "function", 1),
            s("run", "function", 3),
            s("top", "function", 5),
        ]);
    }

    #[test]
    fn javascript_and_typescript() {
        let src = "\
export default async function main() {}
function* gen() {}
export class Widget extends Base {
  constructor(props) {
  static async render(x, y) {
    if (x) {
    for (const a of b) {
  }
}
const handler = async (e) => { };
export const add = (a: number, b: number): number => a + b;
const one = x => x;
const value = compute(1);
export interface Props { a: string }
type Id = string;
enum Color { Red }
// function nope() {}
";
        assert_eq!(syms("JavaScript", src), vec![
            s("main", "function", 0),
            s("gen", "function", 1),
            s("Widget", "class", 2),
            s("constructor", "method", 3),
            s("render", "method", 4),
            s("handler", "function", 9),
            s("add", "function", 10),
            s("one", "function", 11),
            s("Props", "interface", 13),
            s("Id", "type", 14),
            s("Color", "enum", 15),
        ]);
        assert_eq!(names("TypeScriptReact", "export function App() {}"), vec!["App"]);
    }

    #[test]
    fn go() {
        let src = "package main\nfunc main() {\n}\nfunc (s *Server) Serve(addr string) error {\ntype Server struct {\ntype Reader interface {\ntype ID int\n";
        assert_eq!(syms("Go", src), vec![
            s("main", "function", 1),
            s("Serve", "method", 3),
            s("Server", "struct", 4),
            s("Reader", "interface", 5),
            s("ID", "type", 6),
        ]);
    }

    #[test]
    fn c_family() {
        let src = "\
#include <stdio.h>
#define MAX(a, b) ((a) > (b) ? (a) : (b))
typedef struct node {
struct point;
static int add(int a, int b) {
    return add(a, b);
    if (x) {
}
int main(void)
void Widget::paint(Painter &p) const {
Widget::~Widget() {
class Widget : public Base {
enum class Mode {
int x = foo(1);
void declared(int);
";
        assert_eq!(syms("C++", src), vec![
            s("MAX", "macro", 1),
            s("node", "struct", 2),
            s("add", "function", 4),
            s("main", "function", 8),
            s("Widget::paint", "function", 9),
            s("Widget::~Widget", "function", 10),
            s("Widget", "class", 11),
            s("Mode", "enum", 12),
        ]);
    }

    #[test]
    fn objective_c() {
        let src = "@interface Foo : NSObject\n- (void)doThing:(int)a with:(int)b {\n+ (instancetype)shared;\n@implementation Foo\n";
        assert_eq!(syms("Objective-C", src), vec![
            s("Foo", "interface", 0),
            s("doThing:(int)a with:(int)b", "method", 1),
            s("shared", "method", 2),
            s("Foo", "class", 3),
        ]);
    }

    #[test]
    fn java_and_csharp() {
        let src = "\
public class Main {
    public static void main(String[] args) {
        System.out.println(args);
        return foo(x);
        if (x) {
    }
    private Map<String, List<Integer>> build(int n) throws IOException {
    public Main() {
    int x = compute(1);
}
interface Shape {}
public enum Color { RED }
";
        assert_eq!(syms("Java", src), vec![
            s("Main", "class", 0),
            s("main", "method", 1),
            s("build", "method", 6),
            s("Main", "method", 7),
            s("Shape", "interface", 10),
            s("Color", "enum", 11),
        ]);
        assert_eq!(syms("C#", "public sealed partial class Foo {\n    public async Task<int> RunAsync() {\n"), vec![
            s("Foo", "class", 0),
            s("RunAsync", "method", 1),
        ]);
    }

    #[test]
    fn ruby() {
        let src = "module Strata\n  class Buffer < Base\n    def self.open(path)\n    def empty?\n  end\nend\n";
        assert_eq!(syms("Ruby", src), vec![
            s("Strata", "module", 0),
            s("Buffer", "class", 1),
            s("self.open", "method", 2),
            s("empty?", "method", 3),
        ]);
    }

    #[test]
    fn shell() {
        let src = "#!/bin/bash\nbuild() {\nfunction deploy {\n# fake() {\nfoo_bar () {\necho hi\n";
        assert_eq!(names("Bourne Again Shell (bash)", src), vec!["build", "deploy", "foo_bar"]);
        assert_eq!(names("Shell-Unix-Generic", "x() { :; }"), vec!["x"]);
    }

    #[test]
    fn markdown() {
        let src = "---\ntitle: x\n---\n# Title #\nText\n## Section\n```sh\n# not a heading\n```\nSetext\n======\nSub\n---\n- item\n---\n####### too deep\n";
        assert_eq!(syms("Markdown", src), vec![
            s("Title", "heading", 3),
            s("Section", "heading", 5),
            s("Setext", "heading", 9),
            s("Sub", "heading", 11),
        ]);
    }

    #[test]
    fn css() {
        let src = "body {\n  color: red;\n}\n.a, .b:hover {\n@media (max-width: 600px) {\n  .inner {\n/* .x { */\n";
        assert_eq!(names("CSS", src), vec!["body", ".a, .b:hover", "@media (max-width: 600px)"]);
    }

    #[test]
    fn yaml() {
        let src = "name: strata\non:\n  push:\n    branches: [main]\n# comment: x\njobs:\n- item: 1\n\"quoted key\": 2\n";
        assert_eq!(names("YAML", src), vec!["name", "on", "jobs", "\"quoted key\""]);
    }

    #[test]
    fn php_lua_swift() {
        let php = "<?php\nnamespace App\\Http;\nfinal class Controller {\n    public static function index() {\nfunction helper() {}\n";
        assert_eq!(syms("PHP", php), vec![
            s("App\\Http", "module", 1),
            s("Controller", "class", 2),
            s("index", "function", 3),
            s("helper", "function", 4),
        ]);
        let lua = "local function setup()\nfunction M.run(x)\nM.stop = function() end\n-- function no()\n";
        assert_eq!(names("Lua", lua), vec!["setup", "M.run", "M.stop"]);
        let swift = "public struct Point {\n    init(x: Int) {\n    mutating func move(by d: Int) {\nextension Point: Equatable {\n";
        assert_eq!(syms("Swift", swift), vec![
            s("Point", "struct", 0),
            s("init", "method", 1),
            s("move", "function", 2),
            s("Point", "extension", 3),
        ]);
    }

    #[test]
    fn unknown_syntax_falls_back() {
        let src = "sub greet {\nfunction foo()\nclass Bar\nplain text\n";
        assert_eq!(syms("Perl", src), vec![s("greet", "function", 0), s("foo", "function", 1), s("Bar", "class", 2)]);
        assert!(symbols("Plain Text", "nothing here\n").is_empty());
    }

    #[test]
    fn handles_crlf_and_big_files() {
        assert_eq!(names("Rust", "fn a() {}\r\nfn b() {}\r\n"), vec!["a", "b"]);
        let big: String = (0..100_000).map(|i| if i % 10 == 0 { format!("fn f{i}() {{\n") } else { "    let x = y + 1;\n".into() }).collect();
        let t = std::time::Instant::now();
        assert_eq!(symbols("Rust", &big).len(), 10_000);
        assert!(t.elapsed().as_secs() < 5);
    }
}
