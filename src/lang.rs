//! Per-language facts keyed by syntect syntax name.

/// Line comment token and block comment pair for a syntax.
pub type CommentTokens = (Option<&'static str>, Option<(&'static str, &'static str)>);

const C_BLOCK: Option<(&str, &str)> = Some(("/*", "*/"));
const XML_BLOCK: Option<(&str, &str)> = Some(("<!--", "-->"));

pub fn comment_tokens(syntax_name: &str) -> CommentTokens {
    match syntax_name {
        "Rust" | "C" | "C++" | "C#" | "Java" | "JavaScript" | "JavaScript (JSX)" | "TypeScript" | "TypeScriptReact"
        | "JavaScript (Rails)" | "Go" | "Scala" | "Objective-C" | "Objective-C++" | "D" | "Groovy" | "ActionScript"
        | "PHP" | "PHP Source" | "Swift" | "Kotlin" | "Dart" | "Zig" | "Protocol Buffer" | "Protocol Buffer (TEXT)"
        | "SCSS" | "Less" | "Stylus" | "Solidity" | "GLSL" | "WGSL" | "Odin" | "Verilog" | "SystemVerilog" | "QML"
        | "jsonnet" | "JSON" | "Typst" | "CMake C Header" | "CMake C++ Header" | "Vyper" => (Some("//"), C_BLOCK),
        "Sass" | "Gomod" => (Some("//"), None),
        "CSS" => (None, C_BLOCK),
        "Nix" | "Terraform" => (Some("#"), C_BLOCK),
        "PowerShell" => (Some("#"), Some(("<#", "#>"))),
        "Julia" => (Some("#"), Some(("#=", "=#"))),
        "Python" | "Ruby" | "Perl" | "R" | "R Console" | "Makefile" | "YAML" | "TOML" | "Tcl"
        | "Bourne Again Shell (bash)" | "Shell-Unix-Generic" | "Graphviz (DOT)" | "Java Properties" | "Dockerfile"
        | "Dockerfile (with bash)" | "Nim" | "Elixir" | "Ruby Haml" | "DotENV" | "CMake" | "CMakeCache" | "nginx"
        | "Fish" | "GraphQL" | "Crystal" | "CoffeeScript" | "Crontab" | "Apache Conf" | "Puppet" | "Rego"
        | "Requirements.txt" | "Git Config" | "Git Ignore" | "Git Attributes" | "SSH Config" | "SSHD Config"
        | "GDScript (Godot Engine)" | "AWK" | "Ninja" | "Robot Framework" | "fstab" | "gnuplot" => (Some("#"), None),
        "Cabal" => (Some("--"), Some(("{-", "-}"))),
        "INI" | "NSIS" | "x86_64 Assembly" | "LLVM" => (Some(";"), None),
        "Lua" => (Some("--"), Some(("--[[", "]]"))),
        "SQL" | "SQL (Rails)" => (Some("--"), C_BLOCK),
        "Haskell" | "Literate Haskell" | "Elm" | "PureScript" | "Idris" => (Some("--"), Some(("{-", "-}"))),
        "Lean 4" => (Some("--"), Some(("/-", "-/"))),
        "Ada" | "VHDL" => (Some("--"), None),
        "F#" => (Some("//"), Some(("(*", "*)"))),
        "Lisp" | "Clojure" | "Racket" => (Some(";"), None),
        "Erlang" | "LaTeX" | "TeX" | "MATLAB" => (Some("%"), None),
        "Fortran (Modern)" => (Some("!"), None),
        "VimL" => (Some("\""), None),
        "Batch File" => (Some("REM"), None),
        "AppleScript" => (Some("--"), Some(("(*", "*)"))),
        "OCaml" | "OCamllex" | "OCamlyacc" | "camlp4" | "SML" => (None, Some(("(*", "*)"))),
        "Pascal" => (Some("//"), Some(("{", "}"))),
        "Jinja2" | "HTML (Jinja2)" | "HTML (Twig)" => (None, Some(("{#", "#}"))),
        "HTML" | "HTML (ASP)" | "HTML (Erlang)" | "HTML (Rails)" | "HTML (Tcl)" | "HTML (EEx)" | "XML" | "Markdown"
        | "MultiMarkdown" | "Java Server Page (JSP)" | "NAnt Build File" | "Vue Component" | "Svelte" => {
            (None, XML_BLOCK)
        }
        "ASP" => (Some("'"), None),
        _ => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_syntaxes_have_tokens() {
        let names = [
            "Rust", "C", "Python", "Bourne Again Shell (bash)", "HTML", "CSS", "SQL", "YAML", "Lisp", "TypeScript",
            "TypeScriptReact", "JavaScript (JSX)", "TOML", "Swift", "Kotlin", "Dockerfile", "Dart", "Elixir", "Zig",
            "Nix", "Terraform", "GraphQL", "Protocol Buffer", "SCSS", "Sass", "Less", "PowerShell", "INI", "DotENV",
            "CMake", "nginx", "Fish", "Vue Component", "Svelte", "Lua", "Makefile", "Groovy", "Haskell", "Julia",
        ];
        let known = crate::highlight::syntax_names();
        for name in names {
            assert!(known.contains(&name), "{name} is a bundled syntax name");
            assert_ne!(comment_tokens(name), (None, None), "{name}");
        }
        assert_eq!(comment_tokens("Rust").0, Some("//"));
        assert_eq!(comment_tokens("CSS"), (None, Some(("/*", "*/"))));
        assert_eq!(comment_tokens("Plain Text"), (None, None));
        for (name, line) in [
            ("TOML", "#"),
            ("Swift", "//"),
            ("Kotlin", "//"),
            ("TypeScript", "//"),
            ("Dockerfile", "#"),
            ("SQL", "--"),
            ("Lua", "--"),
            ("Terraform", "#"),
            ("PowerShell", "#"),
            ("INI", ";"),
        ] {
            assert_eq!(comment_tokens(name).0, Some(line), "{name}");
        }
        assert_eq!(comment_tokens("TypeScript").1, Some(("/*", "*/")));
        assert_eq!(comment_tokens("Terraform").1, Some(("/*", "*/")));
    }
}
