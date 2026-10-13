//! Smart indentation rules per language, in the spirit of Sublime's
//! indentation settings (TextMate's increase/decrease indent patterns), done
//! as cheap line scans instead of regexes. Strings and comments are skipped
//! so brackets inside them don't count.

use crate::lang::comment_tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kind {
    /// Unmatched `{ ( [` indent the next line (C family, JS, Rust, CSS, JSON…).
    #[default]
    Brackets,
    /// Trailing `:` indents; `return`/`pass`/… dedent the next line.
    Python,
    /// Trailing `:` (or a block scalar `|` / `>`) indents.
    Yaml,
    /// `def`/`if`/`do` … indent, `end` dedents.
    Ruby,
    /// `function`/`then`/`do` … indent, `end`/`until` dedent.
    Lua,
    /// `then`/`do`/`in` indent, `fi`/`done`/`esac` dedent.
    Shell,
    /// An open tag indents, `</` dedents (HTML, XML).
    Markup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    pub kind: Kind,
    line_comment: Option<&'static str>,
    block_comment: Option<(&'static str, &'static str)>,
    quotes: &'static str,
}

impl Default for Rules {
    fn default() -> Self {
        rules_for("Plain Text")
    }
}

/// Rules for a syntect syntax name; unknown syntaxes get the bracket rule.
pub fn rules_for(syntax_name: &str) -> Rules {
    let kind = match syntax_name {
        "Python" | "Cython" => Kind::Python,
        "YAML" => Kind::Yaml,
        "Ruby" | "Ruby on Rails" | "Rakefile" => Kind::Ruby,
        "Lua" => Kind::Lua,
        "Bourne Again Shell (bash)" | "Shell-Unix-Generic" | "commands-builtin-shell-bash" => Kind::Shell,
        "HTML" | "HTML (ASP)" | "HTML (Erlang)" | "HTML (Rails)" | "HTML (Tcl)" | "XML" | "Java Server Page (JSP)"
        | "NAnt Build File" | "XSL" | "SVG" | "Vue Component" => Kind::Markup,
        _ => Kind::Brackets,
    };
    let quotes = match syntax_name {
        "JavaScript" | "TypeScript" | "TypeScriptReact" | "JavaScript (Rails)" | "Go" | "Bourne Again Shell (bash)"
        | "Shell-Unix-Generic" => "\"'`",
        _ if kind == Kind::Markup => "",
        _ => "\"'",
    };
    let (line_comment, block_comment) = comment_tokens(syntax_name);
    Rules { kind, line_comment, block_comment, quotes }
}

/// What a line (up to the caret) says about the next one.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LineInfo {
    /// The next line goes one level deeper.
    pub increase: bool,
    /// The next line goes one level shallower (Python `return`, …).
    pub dedent_next: bool,
    /// The last unmatched bracket opener, if any.
    pub open_bracket: Option<char>,
    /// An unclosed (non-void, non-self-closing) tag is open.
    pub open_tag: bool,
}

impl Rules {
    /// The code part of `line`: string contents become empty quote pairs and
    /// comments are dropped. An unclosed quote counts as a plain char (Rust
    /// lifetimes, apostrophes in prose).
    pub fn code(&self, line: &str) -> String {
        let chars: Vec<char> = line.chars().collect();
        let starts = |i: usize, tok: &str| tok.chars().enumerate().all(|(k, t)| chars.get(i + k) == Some(&t));
        let word_start_comment = matches!(self.kind, Kind::Shell | Kind::Yaml);
        let mut out = String::with_capacity(line.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if let Some(tok) = self.line_comment
                && starts(i, tok)
                && (!word_start_comment || i == 0 || chars[i - 1].is_whitespace())
            {
                break;
            }
            if let Some((open, close)) = self.block_comment
                && starts(i, open)
            {
                let from = i + open.chars().count();
                match (from..chars.len()).find(|&j| starts(j, close)) {
                    Some(j) => {
                        i = j + close.chars().count();
                        out.push(' ');
                        continue;
                    }
                    None => break,
                }
            }
            if self.quotes.contains(c) {
                let mut j = i + 1;
                while j < chars.len() && chars[j] != c {
                    j += if chars[j] == '\\' { 2 } else { 1 };
                }
                if j < chars.len() {
                    out.push(c);
                    out.push(c);
                    i = j + 1;
                    continue;
                }
            }
            out.push(c);
            i += 1;
        }
        out
    }

    /// Indentation facts for the text of a line before the caret.
    pub fn analyze(&self, before: &str) -> LineInfo {
        let code = self.code(before);
        let stack = bracket_stack(&code);
        let open_bracket = stack.last().copied();
        let t = code.trim();
        let first = first_word(t);
        let mut info = LineInfo { open_bracket, ..LineInfo::default() };
        let keyword = match self.kind {
            Kind::Brackets => false,
            Kind::Python => t.ends_with(':'),
            Kind::Yaml => {
                t.ends_with(':')
                    || t.rsplit_once(':').is_some_and(|(_, v)| matches!(v.trim(), "|" | ">" | "|-" | ">-" | "|+" | ">+"))
            }
            Kind::Ruby => {
                let opener = matches!(
                    first,
                    "if" | "unless" | "while" | "until" | "def" | "class" | "module" | "case" | "begin" | "for" | "else"
                        | "elsif" | "when" | "rescue" | "ensure"
                );
                (opener && last_token(t) != "end") || ends_with_word(t, "do") || ends_with_block_params(t)
            }
            Kind::Lua => {
                let tail = last_token(t);
                matches!(first, "else" | "repeat")
                    || ends_with_word(t, "then")
                    || ends_with_word(t, "do")
                    || (has_word(t, "function") && tail != "end")
            }
            Kind::Shell => {
                let t = t.trim_end_matches(';').trim_end();
                ["then", "do", "else", "in"].iter().any(|w| ends_with_word(t, w))
            }
            Kind::Markup => {
                info.open_tag = !tag_stack(&code).is_empty();
                info.open_tag
            }
        };
        info.increase = keyword || open_bracket.is_some();
        info.dedent_next = !info.increase
            && self.kind == Kind::Python
            && matches!(first, "return" | "pass" | "break" | "continue" | "raise");
        info
    }

    /// The line belongs one level shallower than the block it ends, judged
    /// from its start (`end`, `fi`, `</div>`…). Brackets are matched exactly
    /// elsewhere, so they don't count here.
    pub fn is_dedent_line(&self, line: &str) -> bool {
        let t = line.trim_start();
        let first = first_word(t);
        match self.kind {
            Kind::Ruby => matches!(first, "end" | "else" | "elsif" | "when" | "rescue" | "ensure"),
            Kind::Lua => matches!(first, "end" | "else" | "elseif" | "until"),
            Kind::Shell => matches!(first, "fi" | "done" | "esac" | "else" | "elif"),
            Kind::Markup => t.starts_with("</"),
            _ => false,
        }
    }

    /// Python block keywords that line up with an earlier header: the
    /// headers they may close (`else` → `if`, `for`, `try`…).
    pub fn python_partner(&self, line: &str) -> Option<&'static [&'static str]> {
        if self.kind != Kind::Python {
            return None;
        }
        Some(match first_word(line.trim_start()) {
            "else" => &["if", "elif", "for", "while", "try", "except"],
            "elif" => &["if", "elif"],
            "except" => &["try", "except"],
            "finally" => &["try", "except", "else"],
            _ => return None,
        })
    }
}

/// Leading identifier of `s` (empty if it starts with something else).
pub fn first_word(s: &str) -> &str {
    let n = s.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(s.len());
    &s[..n]
}

/// The last identifier anywhere in `s`.
fn last_token(s: &str) -> &str {
    s.rsplit(|c: char| !(c.is_alphanumeric() || c == '_')).find(|w| !w.is_empty()).unwrap_or("")
}

fn has_word(s: &str, w: &str) -> bool {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|t| t == w)
}

/// `s` ends with the whole word `w`.
fn ends_with_word(s: &str, w: &str) -> bool {
    s.strip_suffix(w).is_some_and(|rest| !rest.ends_with(|c: char| c.is_alphanumeric() || c == '_'))
}

/// Ruby `… do |a, b|`.
fn ends_with_block_params(s: &str) -> bool {
    let Some(inner) = s.strip_suffix('|') else { return false };
    inner.rfind('|').is_some_and(|p| ends_with_word(inner[..p].trim_end(), "do"))
}

/// Unmatched openers left on a line; stray closers are ignored.
fn bracket_stack(code: &str) -> Vec<char> {
    let mut stack = Vec::new();
    for c in code.chars() {
        match c {
            '(' | '[' | '{' => stack.push(c),
            ')' | ']' | '}' => {
                let open = match c {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                if stack.last() == Some(&open) {
                    stack.pop();
                }
            }
            _ => {}
        }
    }
    stack
}

const VOID_TAGS: [&str; 16] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr",
    "keygen", "command",
];

/// Tags opened on a line and not closed on it.
fn tag_stack(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut stack: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '<' {
            i += 1;
            continue;
        }
        let closing = chars.get(i + 1) == Some(&'/');
        let mut j = i + 1 + closing as usize;
        let name_start = j;
        while j < chars.len() && (chars[j].is_alphanumeric() || "-_:.".contains(chars[j])) {
            j += 1;
        }
        let name: String = chars[name_start..j].iter().collect::<String>().to_ascii_lowercase();
        // Find the tag's '>' outside attribute quotes.
        let mut quote = None;
        while j < chars.len() {
            match (quote, chars[j]) {
                (None, '"' | '\'') => quote = Some(chars[j]),
                (Some(q), c) if c == q => quote = None,
                (None, '>') => break,
                _ => {}
            }
            j += 1;
        }
        if j >= chars.len() {
            break;
        }
        if name.is_empty() {
            // `<!doctype>`, `<?xml?>`, `<!-- -->`, stray `<`.
        } else if closing {
            if let Some(p) = stack.iter().rposition(|t| *t == name) {
                stack.truncate(p);
            }
        } else if chars[j - 1] != '/' && !VOID_TAGS.contains(&name.as_str()) {
            stack.push(name);
        }
        i = j + 1;
    }
    stack
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inc(syntax: &str, line: &str) -> bool {
        rules_for(syntax).analyze(line).increase
    }

    #[test]
    fn brackets_for_c_family_and_unknown() {
        for s in ["Rust", "C", "C++", "JavaScript", "TypeScript", "Go", "Java", "Swift", "Kotlin", "C#", "CSS", "JSON", "Plain Text", "Whatever"] {
            assert!(inc(s, "fn main() {"), "{s}");
            assert!(inc(s, "foo(a,"), "{s}");
            assert!(inc(s, "let v = ["), "{s}");
            assert!(inc(s, "} else {"), "{s}");
            assert!(!inc(s, "foo(a, b);"), "{s}");
            assert!(!inc(s, "})"), "{s}");
            assert!(!inc(s, "x = 1;"), "{s}");
        }
        assert_eq!(rules_for("Rust").analyze("call({").open_bracket, Some('{'));
    }

    #[test]
    fn strings_and_comments_are_ignored() {
        assert!(!inc("Rust", r#"let s = "{";"#));
        assert!(!inc("Rust", r#"let s = "a\"{";"#));
        assert!(!inc("Rust", "foo(); // {"));
        assert!(!inc("C", "foo(); /* { */"));
        assert!(inc("C", "if (x) { /* note */"));
        assert!(!inc("JavaScript", "let s = `{`;"));
        assert!(!inc("Python", "x = '('  # ("));
        // Rust lifetimes are not strings.
        assert!(inc("Rust", "fn f<'a>(x: &'a str) -> &'a str {"));
        assert!(inc("Rust", "impl<'a> Foo<'a> {"));
    }

    #[test]
    fn python_rules() {
        let r = rules_for("Python");
        assert!(r.analyze("def f(x):").increase);
        assert!(r.analyze("    if x:  # comment").increase);
        assert!(r.analyze("d = {").increase);
        assert!(!r.analyze("x = 'a:'").increase);
        assert!(!r.analyze("x = 1").increase);
        for kw in ["return", "return x", "pass", "break", "continue", "raise ValueError()"] {
            let i = r.analyze(&format!("        {kw}"));
            assert!(i.dedent_next && !i.increase, "{kw}");
        }
        assert!(!r.analyze("returned = 1").dedent_next);
        assert!(r.analyze("return foo(").increase);
        assert!(!r.analyze("return foo(").dedent_next);
        assert_eq!(r.python_partner("    else:").map(|h| h.contains(&"if")), Some(true));
        assert_eq!(r.python_partner("except ValueError as e:"), Some(&["try", "except"][..]));
        assert_eq!(r.python_partner("elsewhere = 1"), None);
        assert_eq!(rules_for("Rust").python_partner("else"), None);
    }

    #[test]
    fn yaml_rules() {
        assert!(inc("YAML", "key:"));
        assert!(inc("YAML", "- name:"));
        assert!(inc("YAML", "script: |"));
        assert!(inc("YAML", "text: >-"));
        assert!(!inc("YAML", "key: value"));
        assert!(!inc("YAML", "key: value # note:"));
        assert!(inc("YAML", "key: # note"));
    }

    #[test]
    fn ruby_lua_shell_rules() {
        assert!(inc("Ruby", "def foo(a)"));
        assert!(inc("Ruby", "  if x > 1"));
        assert!(inc("Ruby", "items.each do |x|"));
        assert!(inc("Ruby", "loop do"));
        assert!(!inc("Ruby", "def foo; end"));
        assert!(!inc("Ruby", "return if x"));
        assert!(!inc("Ruby", "x = done"));
        assert!(inc("Lua", "function f(x)"));
        assert!(inc("Lua", "local g = function()"));
        assert!(inc("Lua", "if x then"));
        assert!(inc("Lua", "for i = 1, 3 do"));
        assert!(inc("Lua", "repeat"));
        assert!(!inc("Lua", "local f = function() return 1 end"));
        assert!(inc("Bourne Again Shell (bash)", "if [ -f x ]; then"));
        assert!(inc("Bourne Again Shell (bash)", "for f in *; do"));
        assert!(inc("Bourne Again Shell (bash)", "case $x in"));
        assert!(inc("Bourne Again Shell (bash)", "else"));
        assert!(inc("Bourne Again Shell (bash)", "f() {"));
        assert!(!inc("Bourne Again Shell (bash)", "echo done"));
        assert!(!inc("Bourne Again Shell (bash)", "echo hi # then"));
        assert!(inc("Bourne Again Shell (bash)", "n=${#arr[@]}; f() {"));

        let sh = rules_for("Bourne Again Shell (bash)");
        assert!(sh.is_dedent_line("  fi"));
        assert!(sh.is_dedent_line("done < file"));
        assert!(!sh.is_dedent_line("  find . -name x"));
        assert!(rules_for("Ruby").is_dedent_line("    end"));
        assert!(!rules_for("Ruby").is_dedent_line("    ending = 1"));
        assert!(rules_for("Lua").is_dedent_line("end)"));
        assert!(!rules_for("Rust").is_dedent_line("end"));
    }

    #[test]
    fn markup_rules() {
        assert!(inc("HTML", "<div>"));
        assert!(inc("HTML", r#"  <div class="a>b">"#));
        assert!(inc("XML", "<root><child>"));
        assert!(!inc("HTML", "<p>text</p>"));
        assert!(!inc("HTML", "<br>"));
        assert!(!inc("HTML", "<img src=x />"));
        assert!(!inc("XML", "<item/>"));
        assert!(!inc("HTML", "<!DOCTYPE html>"));
        assert!(!inc("HTML", "<!-- <div> -->"));
        assert!(!inc("HTML", "</div>"));
        assert!(inc("HTML", "<ul><li>a</li>"));
        assert!(inc("HTML", "don't <div>"));
        assert!(rules_for("HTML").is_dedent_line("  </div>"));
    }
}
