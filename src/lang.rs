//! Per-language facts keyed by syntect syntax name.

/// Line comment token and block comment pair for a syntax.
pub type CommentTokens = (Option<&'static str>, Option<(&'static str, &'static str)>);

const C_BLOCK: Option<(&str, &str)> = Some(("/*", "*/"));
const XML_BLOCK: Option<(&str, &str)> = Some(("<!--", "-->"));

pub fn comment_tokens(syntax_name: &str) -> CommentTokens {
    match syntax_name {
        "Rust" | "C" | "C++" | "C#" | "Java" | "JavaScript" | "TypeScript" | "TypeScriptReact"
        | "JavaScript (Rails)" | "Go" | "Scala" | "Objective-C" | "Objective-C++" | "D" | "Groovy"
        | "ActionScript" | "PHP" | "PHP Source" | "Swift" | "Kotlin" | "Dart" => {
            (Some("//"), C_BLOCK)
        }
        "CSS" => (None, C_BLOCK),
        "Python" | "Ruby" | "Perl" | "R" | "R Console" | "Makefile" | "YAML" | "TOML" | "Tcl"
        | "Bourne Again Shell (bash)" | "Shell-Unix-Generic" | "Graphviz (DOT)" | "Java Properties"
        | "Dockerfile" | "Nim" | "Elixir" | "PowerShell" | "Ruby Haml" => (Some("#"), None),
        "Lua" => (Some("--"), Some(("--[[", "]]"))),
        "SQL" | "SQL (Rails)" => (Some("--"), C_BLOCK),
        "Haskell" | "Literate Haskell" => (Some("--"), Some(("{-", "-}"))),
        "Lisp" | "Clojure" => (Some(";"), None),
        "Erlang" | "LaTeX" | "TeX" | "MATLAB" => (Some("%"), None),
        "Batch File" => (Some("REM"), None),
        "AppleScript" => (Some("--"), Some(("(*", "*)"))),
        "OCaml" | "OCamllex" | "OCamlyacc" | "camlp4" => (None, Some(("(*", "*)"))),
        "Pascal" => (Some("//"), Some(("{", "}"))),
        "HTML" | "HTML (ASP)" | "HTML (Erlang)" | "HTML (Rails)" | "HTML (Tcl)" | "XML" | "Markdown"
        | "MultiMarkdown" | "Java Server Page (JSP)" | "NAnt Build File" => (None, XML_BLOCK),
        "ASP" => (Some("'"), None),
        _ => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syntect::parsing::SyntaxSet;

    #[test]
    fn common_syntaxes_have_tokens() {
        let set = SyntaxSet::load_defaults_newlines();
        let names = ["Rust", "C", "Python", "Bourne Again Shell (bash)", "HTML", "CSS", "SQL", "YAML", "Lisp"];
        for name in names {
            assert!(set.find_syntax_by_name(name).is_some(), "{name} is a syntect name");
            assert_ne!(comment_tokens(name), (None, None), "{name}");
        }
        assert_eq!(comment_tokens("Rust").0, Some("//"));
        assert_eq!(comment_tokens("CSS"), (None, Some(("/*", "*/"))));
        assert_eq!(comment_tokens("Plain Text"), (None, None));
    }
}
