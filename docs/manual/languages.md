# Languages and Syntaxes

Strata highlights about 190 languages and file formats. This page explains how it picks a syntax for a file, how to change it, how to add your own, and which editing features know about specific languages.

## How Strata picks a syntax

When you open a file, or save it under a new name, Strata chooses a syntax from its file name:

1. **Well-known file names** come first. For example `Dockerfile`, `Dockerfile.dev` and `Containerfile` are Dockerfile; `.env` and `.env.local` are DotENV; `.bashrc`, `.zshrc`, `.profile` and `PKGBUILD` are shell scripts; `.babelrc`, `.eslintrc`, `.prettierrc` and `flake.lock` are JSON.
2. **The whole file name**, when a syntax lists it, such as `Makefile` or `Cargo.lock`.
3. **The extension.** Compound extensions are tried longest first, so `page.html.erb` matches `html.erb` before `erb`, and `types.d.ts` falls back to `ts`. A few extensions are mapped specially: `.jsonc` and `.json5` are JSON, `.mjs` and `.cjs` are JavaScript, `.h` is C++, and `.fs`, `.fsi` and `.fsx` are F#.
4. **The first line**, when the name gives no answer. A shebang such as `#!/usr/bin/env python3` or `#!/bin/bash`, or a first line like `<?xml` or `<html>`, picks the matching syntax.

If nothing matches, the file is Plain Text. Very large files and files with very long lines open as Plain Text whatever their name; choose a syntax to highlight them anyway (see [Very large files](troubleshooting.md#very-large-files)).

## Choosing a syntax yourself

You can change the syntax of the current file at any time:

- **View ▸ Syntax** lists every syntax Strata has, in alphabetical order, with a check next to the current one. This menu is also the place to see the complete list.
- The syntax name at the right of the status bar opens the same list.
- In the command palette, run **View: Syntax…** ({{key:select_syntax}}) and type to filter.

The choice applies to that file only. It is remembered with your open tabs across restarts.

## Bundled languages

The bundled syntaxes cover the common programming languages, markup and config formats, including:

- **Programming:** C, C++, C#, D, Dart, Elixir, Elm, Erlang, F#, Go, Groovy, Haskell, Java, JavaScript, JavaScript (JSX), Julia, Kotlin, Lisp, Lua, Nim, Objective-C, OCaml, Odin, Pascal, Perl, PHP, PowerShell, Python, R, Racket, Ruby, Rust, Scala, Solidity, Swift, TypeScript, TypeScriptReact, Zig, and shell scripts (Bourne Again Shell (bash) and Fish).
- **Web:** HTML (plus templates such as Jinja2, Twig, ERB and EEx), CSS, SCSS, Sass, Less, Stylus, Vue Component, Svelte, GraphQL.
- **Data and config:** JSON, YAML, TOML, XML, INI, DotENV, CSV and other separated values, SQL, Protocol Buffer, Terraform, Nix, nginx, Apache Conf, SSH Config, Dockerfile, Makefile, CMake, Ninja, Git config, ignore and commit files.
- **Writing:** Markdown, MultiMarkdown, reStructuredText, AsciiDoc (Asciidoctor), Textile, LaTeX, TeX, BibTeX, Typst, orgmode.
- **Other:** Diff, Regular Expression, log files, x86_64 Assembly, LLVM, GLSL, WGSL, Verilog, SystemVerilog, VHDL.

The licenses for the bundled grammars are in **Help ▸ Licenses and Attribution…**.

## Code in Markdown

Fenced code blocks in Markdown files are highlighted in their own language, both in the editor and in the Markdown preview. Write the language name right after the opening backticks, for example `python` or `rust`.

In the preview, the language can be a syntax name, a file extension or a common alias, such as `py`, `ts`, `typescript`, `tsx`, `jsx`, `js`, `javascript`, `sh`, `shell`, `bash`, `zsh`, `console`, `golang`, `powershell`, `pwsh`, `csharp`, `fsharp`, `hcl`, `terraform`, `proto`, `docker`, `make`, `dotenv`, `jsonc` or `json5`. The preview shows a block with an unknown language as plain text. See [Markdown](markdown.md) for more on the preview.

## Adding your own syntaxes

Syntax definitions use the `.sublime-syntax` YAML format, so existing grammars work unchanged. To add a language, or to replace a bundled one:

1. Find Strata's config folder ([Troubleshooting](troubleshooting.md) shows where it is on each system).
2. Create a folder called `Syntaxes` inside it, if there isn't one.
3. Put your `.sublime-syntax` files directly in `Syntaxes` (not in subfolders).
4. Restart Strata.

Syntaxes are loaded once, when Strata starts, so changes need a restart. Your syntax then appears in View ▸ Syntax and is chosen automatically for the file extensions and first lines it lists. If your syntax has the same name or claims the same extension as a bundled one, yours wins.

If a file can't be used, Strata skips it and names it in the status bar at startup, for example when the file isn't valid YAML or contains a regular expression Strata can't compile. The other syntaxes still load.

> [!NOTE]
> Strata doesn't read `.tmPreferences` files, so Toggle Comment only works for languages whose comment markers Strata already knows (see [Comments](#comments)). A syntax you add under a new name has none. Color schemes are added the same way, in a `Themes` folder; see [Appearance](appearance.md#importing-color-schemes).

## Word wrap for prose

Files in these syntaxes are soft-wrapped even when the `word_wrap` setting is off: **Markdown**, **MultiMarkdown**, **Plain Text**, **reStructuredText**, **Textile** and **LaTeX**. Use **View ▸ Word Wrap** ({{key:toggle_word_wrap}}) to turn wrapping off for one file. See [Settings Reference](settings.md#all-settings) for `word_wrap` and `wrap_column`.

## Comments

**Edit ▸ Comment ▸ Toggle Comment** ({{key:toggle_comment}}) adds or removes a line comment on every selected line, and **Toggle Block Comment** ({{key:toggle_block_comment}}) wraps the selection in a block comment. Strata knows the markers for many languages. Some examples:

| Comment style | Languages |
|---------------|-----------|
| `//` and `/* */` | C, C++, C#, Java, JavaScript, TypeScript, Go, Rust, Swift, Kotlin, Dart, Scala, PHP, Zig, SCSS, Less, JSON |
| `/* */` only | CSS |
| `#` | Python, Ruby, Perl, R, shell scripts, Fish, YAML, TOML, Makefile, Dockerfile, CMake, nginx, Elixir, Nim, GraphQL, DotENV |
| `#` and `/* */` | Nix, Terraform |
| `#` and `<# #>` | PowerShell |
| `--` | SQL (also `/* */`), Lua (also `--[[ ]]`), Haskell and Elm (also `{- -}`), Ada, VHDL |
| `;` | INI, Lisp, Clojure, Racket, Assembly |
| `%` | Erlang, LaTeX, TeX, MATLAB |
| `<!-- -->` | HTML, XML, Markdown, Vue Component, Svelte |

When a language has only block comments (CSS, HTML, OCaml), Toggle Comment uses the block markers. If Strata doesn't know a language's comment markers, the status bar says "No comment syntax for" and the syntax name.

## Smart indentation

With the `smart_indent` setting on (the default), pressing Enter and typing closers follow the language's rules:

| Languages | Behavior |
|-----------|----------|
| Most languages | Indent after an opening `(`, `[` or `{`. A closing bracket typed on its own line lines up with its opener. |
| Python | Also indent after a line ending in `:`, outdent after `return`, `pass`, `break`, `continue` and `raise`, and line up `else:`, `elif`, `except` and `finally` with their header as you type the colon. |
| YAML | Indent after `key:` and after block scalars such as `key: \|` or `key: >-`. |
| Ruby | Indent after `def`, `class`, `module`, `if`, `unless`, `while`, `case`, `begin`, `do` blocks and similar; `end`, `else`, `elsif`, `when`, `rescue` and `ensure` move back out. |
| Lua | Indent after `then`, `do`, `function`, `else` and `repeat`; `end`, `else`, `elseif` and `until` move back out. |
| Shell scripts (bash) | Indent after `then`, `do`, `else` and `in`; `fi`, `done`, `esac`, `else` and `elif` move back out. |
| HTML, XML, Vue Component | Indent after an opening tag; typing `</` on its own line lines up with the tag it closes. |

Brackets, colons and tags inside strings and comments are ignored. With `smart_indent` off, Enter simply keeps the current line's indentation (if `auto_indent` is on). Either way, pressing Enter between a pair like `{}` moves the closer down to its own line. **Edit ▸ Line ▸ Reindent** ({{key:reindent}}) re-indents the selected lines using the same rules.

## Goto Symbol

**Goto ▸ Goto Symbol…** ({{key:goto_symbol}}) lists the functions, types and other definitions in the current file, and **Goto Symbol in Project…** ({{key:goto_symbol_in_project}}) searches the files in the open folders (skipping ignored files and files over 1 MB; without an open folder it works like Goto Symbol). Strata finds symbols by recognizing common definition patterns line by line. It understands these languages:

| Languages | What is listed |
|-----------|----------------|
| Rust | Functions, structs, enums, unions, traits, impl blocks, modules, macros, constants, statics, type aliases |
| Python | Functions and classes |
| JavaScript, TypeScript (including JSX and TSX) | Functions, classes, interfaces, enums, type aliases, arrow functions assigned to variables, namespaces, methods |
| Go | Functions, methods, structs, interfaces, types |
| C, C++, Objective-C | Functions, structs, classes, unions, enums, namespaces, `#define` macros, Objective-C interfaces, implementations, protocols and methods |
| Java, C#, Kotlin, Scala, Groovy, Dart | Classes, interfaces, enums, records, objects, traits, functions, methods |
| Ruby, Crystal | Methods, classes, modules |
| PHP | Functions, classes, interfaces, traits, enums, namespaces |
| Lua | Functions |
| Swift | Functions, classes, structs, enums, protocols, extensions, actors, initializers |
| CSS, SCSS, Sass, Less | Selectors, `@media` and `@keyframes` |
| YAML | Top-level keys |
| Markdown | Headings |
| Shell scripts (bash) | Functions |

For other languages, Strata lists lines that start with a common definition keyword such as `function`, `func`, `fn`, `def`, `sub`, `proc`, `class`, `struct`, `interface` or `module`.

## Language servers

A language server is a program, made by a language's community, that understands the language the way its compiler does. When one is installed for a file's language, Strata starts it in the background and uses it for:

- **Problems.** Errors get a red wavy underline, warnings an amber one and notes a blue one, with a dot in the gutter on their line. Rest the mouse pointer on the underline or the dot to read the message. The status bar counts the current file's errors and warnings; click it, or run **LSP: Show Problems…** ({{key:lsp_problems}}), to list every problem the server reports in the project. **LSP: Next Problem** ({{key:lsp_next_problem}}) and **LSP: Previous Problem** ({{key:lsp_prev_problem}}) select the next or previous problem in the current file and show its message.
- **Completions.** The server's suggestions come first in the completion list, with a short description at the right (the function's signature, say) and its documentation beside the list. They appear as you type a word and after a `.` (or whatever the language uses); {{key:auto_complete}} asks for them right away. Choosing one may also add what it needs, such as an import at the top of the file.
- **Hover information.** Rest the mouse pointer on a name for half a second to see its type and documentation, or run **LSP: Show Hover Info** ({{key:lsp_hover}}) for the name at the cursor. Moving the pointer away, typing or pressing Esc closes it.
- **Go to Definition and Go to Reference.** {{key:goto_definition}} and {{key:goto_reference}} ask the server first (see [Go to Definition](find.md#go-to-definition)).
- **Rename Symbol.** **LSP: Rename Symbol…** ({{key:lsp_rename}}) renames the name at the cursor everywhere it's used. Type the new name and press Enter (Esc cancels). Files that aren't open are opened so you can look over the changes; nothing is saved until you save. Each file's change is one step of Undo in its tab.
- **Format Document.** **LSP: Format Document** ({{key:lsp_format}}) has the server reformat the file, as one step of Undo. Cursors stay on the text they were on.
- **Parameter hints.** Typing `(` or `,` in a call shows the function's parameters above the cursor, the current one underlined. **LSP: Show Parameter Hints** ({{key:lsp_signature_help}}) shows them on request; Esc closes them.

These commands are also in **Tools ▸ Language Server**.

The status bar shows the server for the current file and how it's doing: "starting…", what it's working on (rust-analyzer says "Indexing" while it reads a project), the problem counts once it's ready, or "restarting" if it stopped. A server that stops on its own is started again after a short pause (longer each time); after five stops in a few minutes Strata waits until you run **LSP: Restart Language Servers** or click the status bar. Messages about a server appear in the status bar, never in a dialog.

Strata talks to servers only on your computer, through their input and output: it doesn't download servers, and makes no network connections for them. Large files opened in [large-file mode](troubleshooting.md#very-large-files) aren't sent to servers.

### Built-in servers

Strata knows these servers and uses them when their program is installed (on your `PATH`, or in a usual place such as `~/.cargo/bin`, `/opt/homebrew/bin` or `~/go/bin`; apps opened from the Finder on macOS don't see your shell's `PATH`, so Strata adds those folders itself):

| Name | Server | Files | How to install it |
|------|--------|-------|-------------------|
| `rust` | rust-analyzer | `.rs` | `rustup component add rust-analyzer` |
| `typescript` | typescript-language-server | `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs`, `.mts`, `.cts` | `npm install -g typescript-language-server typescript` |
| `python` | pyright | `.py`, `.pyi` | `npm install -g pyright` |
| `go` | gopls | `.go` | `go install golang.org/x/tools/gopls@latest` |
| `c` | clangd | C, C++ and Objective-C files | Xcode's command line tools (macOS) or LLVM |

One server runs per language and project. The project is the nearest folder above the file that holds one of the server's marker files: for Rust `Cargo.lock`, then `Cargo.toml` (so a workspace's crates share one server); for TypeScript `tsconfig.json`, `jsconfig.json` or `package.json`; for Python `pyrightconfig.json`, `pyproject.toml`, `setup.py`, `setup.cfg` or `requirements.txt`; for Go `go.work` or `go.mod`; for C `.clangd`, `compile_commands.json` or `compile_flags.txt`. Without one, it's the open folder that holds the file, or else the file's own folder.

### Setting up servers

`lsp_servers` in `settings.json` changes the built-in servers and adds your own. Each entry is named; an entry with a built-in name changes only the fields it gives:

```json
"lsp_servers": {
    "rust": { "settings": { "rust-analyzer": { "check": { "command": "clippy" } } } },
    "python": { "enabled": false },
    "zig": { "command": "zls", "file_patterns": ["*.zig"], "root_markers": ["build.zig"] }
}
```

| Field | What it does |
|-------|--------------|
| `command` | The server's program: a name looked up on `PATH`, or a full path. Required for a new server. |
| `args` | Arguments for it, for example `["--stdio"]`. |
| `file_patterns` | The file names it handles, as patterns such as `*.zig`. Required for a new server. A new server is tried before the built-in ones, so it can take over their files. |
| `root_markers` | File names that mark a project's top folder, tried in order. |
| `initialization_options` | A value sent to the server when it starts (see its documentation). |
| `settings` | The server's settings, which it asks for by section; for rust-analyzer they go under `"rust-analyzer"`. |
| `language_id` | The language name sent to the server; by default it follows the file extension (`rust`, `typescriptreact`, `cpp`…). |
| `env` | Environment variables for the server, for example `{"RUST_LOG": "info"}`. |
| `enabled` | `false` turns the server off. |

If the program of a server you added (or whose `command` you changed) can't be found, the status bar says so. Built-in servers that aren't installed are skipped quietly. Changes apply when you save `settings.json`: the running servers stop and start again as files need them.

Set `lsp_enabled` to `false` (**Language servers** in the Settings window's Editor section) to turn language servers off altogether; Go to Definition and Go to Reference then use Strata's own search.
