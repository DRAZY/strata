//! Build systems: Tools → Build (⌘B), Build With… (⇧⌘B), Cancel Build, and the
//! Build Results panel with next/previous result (F4 / ⇧F4).
//!
//! # Embedding
//!
//! ```ignore
//! // App state: one runner, created with the config dir (user build systems live in
//! // `<config_dir>/build_systems/*.json`).
//! let mut build = build::Build::new(config_dir.clone());
//!
//! // Every frame (cheap; drains streamed output even while the panel is hidden):
//! build.poll(ctx);
//! if build.visible {
//!     let action = egui::Panel::bottom("build").resizable(true).default_size(180.0)
//!         .show(ui, |ui| build.show(ui)).inner;
//!     if let Some(build::BuildAction::Goto(path, line, col)) = action { /* open at 0-based line/col */ }
//! }
//!
//! // ⌘B (save dirty files first, like Sublime):
//! if let Err(e) = build.build(&bctx) { status = e }
//! // ⇧⌘B: list `build.choices(&bctx)` in the palette, then `build.build_with(&choice, &bctx)`.
//! // Cancel Build: `build.cancel()`.  F4 / ⇧F4: `build.next_result()` / `build.prev_result()`.
//! ```
//!
//! # User build systems
//!
//! Each `<config_dir>/build_systems/*.json` holds one build system object (or an array
//! of them). User systems with the same name as a built-in replace it.
//!
//! ```json
//! {
//!   "name": "My Make",
//!   "cmd": ["make", "-j8"],
//!   "working_dir": "$project_path",
//!   "selector": { "syntaxes": ["C", "C++"], "extensions": ["c", "h"], "files": ["Makefile"] },
//!   "file_regex": "^(..[^:]*):([0-9]+):([0-9]+): (.*)$",
//!   "env": { "CFLAGS": "-O2", "PATH": "/opt/tools/bin:$PATH" },
//!   "shell": false,
//!   "priority": 0,
//!   "variants": [
//!     { "name": "Clean", "cmd": ["make", "clean"] },
//!     { "name": "Run", "cmd": ["make run && ./a.out"], "shell": true }
//!   ]
//! }
//! ```
//!
//! - `cmd`: program and arguments. With `"shell": true` the elements are joined with
//!   spaces and run by `sh -c` (Unix) or `cmd /S /C` (Windows); substituted variable
//!   values are then quoted for that shell.
//! - `selector`: when the system applies. `syntaxes` (syntect names such as "Rust") and
//!   `extensions` (no dot) match the current file; if either list is non-empty one of
//!   them must match. `files` are marker files (e.g. "Cargo.toml"); if non-empty one must
//!   exist in the file's folder or an ancestor up to the open folder. An empty selector
//!   always applies. "Automatic" picks the highest score: +2 for a file match, +2 for a
//!   marker, plus `priority`; ties go to user systems, then list order.
//! - `working_dir`: default `$project_path` when the selector has marker files, else
//!   `$file_path`, else `$folder`.
//! - `file_regex`: matches result lines. Named groups `file`, `line`, `col`, `message`
//!   are used when present, else positional groups 1-4 in that order. Line and column are
//!   1-based in the output. A match without a message takes the latest `error…`/
//!   `warning…` line (rustc prints the message above its `-->` location).
//! - `env`: extra environment variables (values are expanded).
//! - `variants`: named overrides of `cmd`, `working_dir`, `env` (merged), `shell` and
//!   `file_regex`.
//!
//! Variables, usable in `cmd`, `working_dir` and `env` as `$name` or `${name}` (`$$` is a
//! literal `$`):
//!
//! | Variable | Value |
//! |---|---|
//! | `$file` | full path of the current file |
//! | `$file_path` | its directory |
//! | `$file_name` | its name, e.g. `main.rs` |
//! | `$file_base_name` | its name without the extension, e.g. `main` |
//! | `$file_extension` | its extension without the dot, e.g. `rs` |
//! | `$folder` | the sidebar folder holding the file (else the first one) |
//! | `$project_path` | the directory holding the selector's marker file, else `$folder`, else `$file_path` |
//! | `$project_name` | the last component of `$project_path` |
//! | `$exe` | `.exe` on Windows, empty elsewhere |
//!
//! Any other name falls back to the process environment (`$PATH`, `$HOME`), and is left
//! as written if unset. Using a `$file…` variable with an unsaved buffer, or `$folder`
//! with no folder open, is an error reported instead of running.

use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// Oldest lines are dropped beyond this many.
pub const MAX_LINES: usize = 10_000;

/// Matches rustc/cargo locations: `  --> src/main.rs:2:5`.
const RUSTC_REGEX: &str = r"^\s*-->\s+(?P<file>.+?):(?P<line>\d+):(?P<col>\d+)\s*$";
/// gcc/clang/go/make style: `path:line:col: message` (col optional). `..` lets a
/// Windows drive letter (`C:`) through.
const GCC_REGEX: &str = r"^(?P<file>..[^:\n]*?):(?P<line>\d+):(?:(?P<col>\d+):)?\s+(?P<message>.*)$";
/// Python tracebacks: `  File "x.py", line 3, in <module>`.
const PYTHON_REGEX: &str = r#"^\s*File "(?P<file>[^<"][^"]*)", line (?P<line>\d+)"#;
/// Node stack frames `    at f (/x/a.js:3:5)` and the first `/x/a.js:3` line.
const NODE_REGEX: &str = r"^(?:\s+at .*?\(?)?(?P<file>(?:[A-Za-z]:)?[^\s():]+\.[cm]?[jt]sx?):(?P<line>\d+)(?::(?P<col>\d+))?";
/// Ruby: `x.rb:3:in 'foo': message` and `\tfrom x.rb:7:in ...`.
const RUBY_REGEX: &str = r"^\s*(?:from )?(?P<file>(?:[A-Za-z]:)?[^\s:]+\.rb):(?P<line>\d+):(?P<message>.*)$";
/// bash: `x.sh: line 3: message`.
const SHELL_REGEX: &str = r"^(?P<file>.+?): line (?P<line>\d+): (?P<message>.*)$";
/// tsc-style `src/a.ts(3,5): error TS…` or gcc style, for npm scripts.
const NPM_REGEX: &str =
    r"^(?P<file>(?:[A-Za-z]:)?[^\s:(]+)(?:\((?P<line>\d+),(?P<col>\d+)\)|:(?P<line2>\d+):(?P<col2>\d+)):?\s*(?P<message>.*)$";
/// Terminal output: `path:line[:col]` or MSVC's `path(line[,col])` anywhere in
/// a line, such as a panic's `panicked at src/x.rs:3:5:`. Quotes, brackets and
/// the like end a path, so `(a.rs:3)` or `'a.rs:3'` give `a.rs`; a `file://`
/// prefix is skipped.
const ANYWHERE_REGEX: &str = r#"(?:file://)?(?P<file>(?:[A-Za-z]:[\\/])?[^\s:()\[\]{}<>"'`|,;=]+)(?::(?P<line>\d+)(?::(?P<col>\d+))?|\((?P<line2>\d+)(?:, ?(?P<col2>\d+))?\))"#;

// ---------------------------------------------------------------------------
// Definitions
// ---------------------------------------------------------------------------

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Selector {
    /// syntect syntax names, e.g. "Rust", "Python".
    pub syntaxes: Vec<String>,
    /// File extensions without the dot.
    pub extensions: Vec<String>,
    /// Marker files, e.g. "Cargo.toml", "Makefile".
    pub files: Vec<String>,
}

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Variant {
    pub name: String,
    pub cmd: Option<Vec<String>>,
    pub working_dir: Option<String>,
    pub env: BTreeMap<String, String>,
    pub shell: Option<bool>,
    pub file_regex: Option<String>,
}

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BuildSystem {
    pub name: String,
    pub cmd: Vec<String>,
    pub working_dir: Option<String>,
    pub selector: Selector,
    pub file_regex: Option<String>,
    pub env: BTreeMap<String, String>,
    pub shell: bool,
    pub priority: i32,
    pub variants: Vec<Variant>,
    /// The JSON file it came from; None for built-ins.
    #[serde(skip)]
    pub source: Option<PathBuf>,
}

/// What the build runs against.
#[derive(Clone, Debug, Default)]
pub struct BuildContext {
    /// The current file (None for an unsaved buffer).
    pub file: Option<PathBuf>,
    /// The sidebar folder holding the file, else the first one.
    pub folder: Option<PathBuf>,
    /// The current syntax name, e.g. "Rust".
    pub syntax: Option<String>,
}

/// One entry for "Build With…": a system, or one of its variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildChoice {
    /// "Cargo" or "Cargo - Test", for the palette.
    pub label: String,
    pub system: String,
    pub variant: Option<String>,
}

/// A command ready to run.
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    /// "Cargo - Test".
    pub title: String,
    /// Program and args; with `shell`, a single command string.
    pub argv: Vec<String>,
    pub shell: bool,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub file_regex: Option<String>,
}

impl BuildSystem {
    fn new(name: &str, cmd: &[&str]) -> Self {
        Self { name: name.into(), cmd: strings(cmd), ..Default::default() }
    }

    fn syntaxes(mut self, s: &[&str]) -> Self {
        self.selector.syntaxes = strings(s);
        self
    }

    fn extensions(mut self, s: &[&str]) -> Self {
        self.selector.extensions = strings(s);
        self
    }

    fn markers(mut self, s: &[&str]) -> Self {
        self.selector.files = strings(s);
        self
    }

    fn regex(mut self, r: &str) -> Self {
        self.file_regex = Some(r.into());
        self
    }

    fn shell(mut self) -> Self {
        self.shell = true;
        self
    }

    fn variant(mut self, name: &str, cmd: &[&str], shell: bool) -> Self {
        self.variants.push(Variant { name: name.into(), cmd: Some(strings(cmd)), shell: Some(shell), ..Default::default() });
        self
    }

    /// The directory holding one of the selector's marker files, nearest the file first.
    fn marker_dir(&self, ctx: &BuildContext) -> Option<PathBuf> {
        if self.selector.files.is_empty() {
            return None;
        }
        let start = ctx.file.as_ref().and_then(|f| f.parent()).or(ctx.folder.as_deref())?;
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
        let mut dir = Some(start);
        while let Some(d) = dir {
            // Never treat the home directory or the filesystem root as a project.
            if d.parent().is_none() || Some(d) == home.as_deref() {
                break;
            }
            if self.selector.files.iter().any(|m| d.join(m).is_file()) {
                return Some(d.to_owned());
            }
            if ctx.folder.as_deref() == Some(d) {
                break;
            }
            dir = d.parent();
        }
        None
    }

    fn file_matches(&self, ctx: &BuildContext) -> bool {
        let sel = &self.selector;
        let syntax = ctx.syntax.as_deref().is_some_and(|s| sel.syntaxes.iter().any(|x| x.eq_ignore_ascii_case(s)));
        let ext = ctx
            .file
            .as_ref()
            .and_then(|f| f.extension())
            .is_some_and(|e| sel.extensions.iter().any(|x| x.trim_start_matches('.').eq_ignore_ascii_case(&e.to_string_lossy())));
        syntax || ext
    }

    /// None if the system doesn't apply to `ctx`, else its Automatic score.
    pub fn score(&self, ctx: &BuildContext) -> Option<i32> {
        let sel = &self.selector;
        let mut score = self.priority;
        if !sel.syntaxes.is_empty() || !sel.extensions.is_empty() {
            if !self.file_matches(ctx) {
                return None;
            }
            score += 2;
        }
        if !sel.files.is_empty() {
            self.marker_dir(ctx)?;
            score += 2;
        }
        Some(score)
    }

    /// Expand variables and pick the variant (by name, case-insensitive).
    pub fn prepare(&self, variant: Option<&str>, ctx: &BuildContext) -> Result<Prepared, String> {
        let var = match variant {
            Some(v) => Some(
                self.variants
                    .iter()
                    .find(|x| x.name.eq_ignore_ascii_case(v))
                    .ok_or_else(|| format!("{} has no variant \"{v}\"", self.name))?,
            ),
            None => None,
        };
        let title = match var {
            Some(v) => format!("{} - {}", self.name, v.name),
            None => self.name.clone(),
        };
        let cmd = var.and_then(|v| v.cmd.as_ref()).unwrap_or(&self.cmd);
        if cmd.is_empty() {
            return Err(format!("{title}: empty cmd"));
        }
        let shell = var.and_then(|v| v.shell).unwrap_or(self.shell);
        let vars = variables(ctx, self.marker_dir(ctx));
        let default_dir = if !self.selector.files.is_empty() {
            "$project_path"
        } else if ctx.file.is_some() {
            "$file_path"
        } else {
            "$folder"
        };
        let wd = var.and_then(|v| v.working_dir.as_deref()).or(self.working_dir.as_deref()).unwrap_or(default_dir);
        let argv = if shell {
            let parts = cmd.iter().map(|c| expand(c, &vars, Quote::Shell)).collect::<Result<Vec<_>, _>>()?;
            vec![parts.join(" ")]
        } else {
            cmd.iter().map(|c| expand(c, &vars, Quote::None)).collect::<Result<Vec<_>, _>>()?
        };
        let cwd = PathBuf::from(expand(wd, &vars, Quote::None)?);
        if !cwd.is_dir() {
            return Err(format!("{title}: working directory {} does not exist", cwd.display()));
        }
        let mut env: BTreeMap<&String, &String> = self.env.iter().collect();
        if let Some(v) = var {
            env.extend(v.env.iter());
        }
        let env = env.into_iter().map(|(k, v)| Ok((k.clone(), expand(v, &vars, Quote::None)?))).collect::<Result<_, String>>()?;
        let file_regex = var.and_then(|v| v.file_regex.clone()).or_else(|| self.file_regex.clone());
        Ok(Prepared { title, argv, shell, cwd, env, file_regex })
    }
}

fn strings(s: &[&str]) -> Vec<String> {
    s.iter().map(|&x| x.to_owned()).collect()
}

/// The built-in systems, in Automatic tie-break order.
pub fn builtin() -> Vec<BuildSystem> {
    let win = cfg!(windows);
    let run_bin = |name: &str| if win { format!("{name}.exe") } else { format!("./{name}") };
    let rust_run = format!("rustc $file && {}", run_bin("$file_base_name"));
    let c_cc = if win { "gcc" } else { "cc" };
    let cpp_cc = if win { "g++" } else { "c++" };
    let c_run = format!("{c_cc} $file -o $file_base_name$exe && {}", run_bin("$file_base_name"));
    let cpp_run = format!("{cpp_cc} -std=c++17 $file -o $file_base_name$exe && {}", run_bin("$file_base_name"));
    let python = if win { "py" } else { "python3" };
    vec![
        BuildSystem::new("Cargo", &["cargo", "build"])
            .markers(&["Cargo.toml"])
            .regex(RUSTC_REGEX)
            .variant("Run", &["cargo", "run"], false)
            .variant("Test", &["cargo", "test"], false)
            .variant("Clippy", &["cargo", "clippy", "--all-targets"], false)
            .variant("Check", &["cargo", "check"], false)
            .variant("Release", &["cargo", "build", "--release"], false),
        BuildSystem::new("Rust", &["rustc", "$file"])
            .syntaxes(&["Rust"])
            .extensions(&["rs"])
            .regex(RUSTC_REGEX)
            .variant("Run", &[&rust_run], true),
        BuildSystem::new("Go", &["go", "run", "$file"])
            .syntaxes(&["Go"])
            .extensions(&["go"])
            .regex(GCC_REGEX)
            .variant("Build", &["go", "build"], false)
            .variant("Test", &["go", "test"], false),
        BuildSystem::new("Python", &[python, "-u", "$file"]).syntaxes(&["Python"]).extensions(&["py", "pyw"]).regex(PYTHON_REGEX),
        BuildSystem::new("Node", &["node", "$file"])
            .syntaxes(&["JavaScript"])
            .extensions(&["js", "mjs", "cjs"])
            .regex(NODE_REGEX),
        BuildSystem::new("Ruby", &["ruby", "$file"]).syntaxes(&["Ruby"]).extensions(&["rb"]).regex(RUBY_REGEX),
        BuildSystem::new("Shell", &["bash", "$file"])
            .syntaxes(&["Bourne Again Shell (bash)", "Shell-Unix-Generic"])
            .extensions(&["sh", "bash"])
            .regex(SHELL_REGEX),
        BuildSystem::new("Make", &["make"])
            .markers(&["Makefile", "makefile", "GNUmakefile"])
            .regex(GCC_REGEX)
            .variant("Clean", &["make", "clean"], false)
            .variant("Test", &["make", "test"], false),
        BuildSystem::new("C", &[c_cc, "$file", "-o", "$file_base_name$exe"])
            .syntaxes(&["C"])
            .extensions(&["c"])
            .regex(GCC_REGEX)
            .variant("Run", &[&c_run], true),
        BuildSystem::new("C++", &[cpp_cc, "-std=c++17", "$file", "-o", "$file_base_name$exe"])
            .syntaxes(&["C++"])
            .extensions(&["cpp", "cc", "cxx", "c++"])
            .regex(GCC_REGEX)
            .variant("Run", &[&cpp_run], true),
        // npm is npm.cmd on Windows, which only a shell can launch.
        BuildSystem::new("npm", &["npm run build"])
            .shell()
            .markers(&["package.json"])
            .regex(NPM_REGEX)
            .variant("Test", &["npm test"], true)
            .variant("Start", &["npm start"], true),
    ]
}

/// All known build systems: user ones first, then built-ins they don't replace.
#[derive(Clone, Debug, Default)]
pub struct BuildSystems {
    pub systems: Vec<BuildSystem>,
    /// Problems loading user JSON files, for the status bar.
    pub errors: Vec<String>,
}

/// A build system file: one object, or a list of them. Errors name the
/// field, its line and the shape it should have.
fn parse_file(text: &str) -> Result<Vec<BuildSystem>, String> {
    let parsed = match text.trim_start().chars().next() {
        Some('{') => serde_json::from_str::<BuildSystem>(text).map(|s| vec![s]),
        Some('[') => serde_json::from_str::<Vec<BuildSystem>>(text),
        _ => {
            return Err(match serde_json::from_str::<serde_json::Value>(text) {
                Err(e) => e.to_string(),
                Ok(_) => "the file should hold a build system object ({\"name\": \"…\", \"cmd\": [\"…\"]}) or a list of them in [ ]".into(),
            });
        }
    };
    parsed.map_err(|e| explain_error(text, &e))
}

/// serde's message for a bad build system file, in terms of its fields.
fn explain_error(text: &str, e: &serde_json::Error) -> String {
    let full = e.to_string().replace('`', "\"");
    if !e.is_data() {
        return full; // a JSON syntax error: serde says where
    }
    let line = e.line();
    let msg = full.rsplit_once(" at line ").map_or(full.as_str(), |(m, _)| m);
    if msg.starts_with("unknown field") {
        return match msg.split_once(", expected one of ") {
            Some((field, known)) => format!("{field} (line {line}); the fields are {known}"),
            None => format!("{msg} (line {line})"),
        };
    }
    // The key whose value is at the error position.
    let offset = text.split_inclusive('\n').take(line.saturating_sub(1)).map(str::len).sum::<usize>() + e.column();
    let before = &text[..offset.min(text.len())];
    let key = regex::Regex::new(r#""([A-Za-z_]+)"\s*:"#).ok().and_then(|re| re.captures_iter(before).last()).map(|c| c[1].to_owned());
    let Some(key) = key else { return format!("{msg} (line {line})") };
    let shape = match key.as_str() {
        "cmd" => "a list of strings, like [\"make\", \"-j8\"]",
        "syntaxes" | "extensions" | "files" => "a list of strings, like [\"rs\", \"toml\"]",
        "name" | "working_dir" | "file_regex" => "a string",
        "env" => "an object of strings, like {\"CFLAGS\": \"-O2\"}",
        "shell" => "true or false",
        "priority" => "a whole number",
        "selector" => "an object, like {\"extensions\": [\"c\"]}",
        "variants" => "a list of objects, like [{\"name\": \"Run\", \"cmd\": [\"make\", \"run\"]}]",
        _ => return format!("\"{key}\" (line {line}): {msg}"),
    };
    let found = msg.strip_prefix("invalid type: ").and_then(|m| m.split_once(", expected")).map(|(f, _)| format!("; found {f}")).unwrap_or_default();
    format!("\"{key}\" (line {line}) should be {shape}{found}")
}

impl BuildSystems {
    /// Built-ins plus `<config_dir>/build_systems/*.json`.
    pub fn load(config_dir: Option<&Path>) -> Self {
        let mut systems = Vec::new();
        let mut errors = Vec::new();
        if let Some(dir) = config_dir.map(|d| d.join("build_systems"))
            && let Ok(rd) = std::fs::read_dir(&dir)
        {
            let mut paths: Vec<PathBuf> = rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")))
                .collect();
            paths.sort();
            for path in paths {
                let parsed = std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|s| parse_file(&s));
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                match parsed {
                    Ok(v) => systems.extend(v),
                    Err(e) => {
                        errors.push(format!("{name}: {e}"));
                        continue;
                    }
                }
                for s in systems.iter_mut().filter(|s| s.source.is_none()) {
                    s.source = Some(path.clone());
                }
            }
            systems.retain(|s| {
                let bad = s.name.trim().is_empty() || s.cmd.is_empty();
                if bad {
                    let file = s.source.as_ref().and_then(|p| p.file_name()).unwrap_or_default().to_string_lossy();
                    errors.push(format!("{file}: a build system needs a \"name\" and a \"cmd\""));
                }
                !bad
            });
            for s in &systems {
                let regexes = s.file_regex.iter().chain(s.variants.iter().filter_map(|v| v.file_regex.as_ref()));
                for r in regexes {
                    if let Err(e) = Regex::new(r) {
                        errors.push(format!("{}: bad file_regex: {}", s.name, e.to_string().lines().last().unwrap_or("").trim()));
                    }
                }
            }
        }
        for b in builtin() {
            if !systems.iter().any(|s| s.name.eq_ignore_ascii_case(&b.name)) {
                systems.push(b);
            }
        }
        Self { systems, errors }
    }

    pub fn get(&self, name: &str) -> Option<&BuildSystem> {
        self.systems.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Systems that apply to `ctx`, best first.
    pub fn applicable(&self, ctx: &BuildContext) -> Vec<&BuildSystem> {
        let mut v: Vec<(i32, usize, &BuildSystem)> =
            self.systems.iter().enumerate().filter_map(|(i, s)| Some((s.score(ctx)?, i, s))).collect();
        v.sort_by_key(|&(score, i, _)| (std::cmp::Reverse(score), i));
        v.into_iter().map(|(_, _, s)| s).collect()
    }

    /// "Automatic": the best system for `ctx`.
    pub fn automatic(&self, ctx: &BuildContext) -> Option<&BuildSystem> {
        self.applicable(ctx).into_iter().next()
    }

    /// Entries for "Build With…": each applicable system and its variants, best first.
    pub fn choices(&self, ctx: &BuildContext) -> Vec<BuildChoice> {
        let mut out = Vec::new();
        for s in self.applicable(ctx) {
            out.push(BuildChoice { label: s.name.clone(), system: s.name.clone(), variant: None });
            for v in &s.variants {
                out.push(BuildChoice {
                    label: format!("{} - {}", s.name, v.name),
                    system: s.name.clone(),
                    variant: Some(v.name.clone()),
                });
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Variables
// ---------------------------------------------------------------------------

/// name → Ok(value) or Err(why it's unavailable).
type Vars = HashMap<&'static str, Result<String, String>>;

fn variables(ctx: &BuildContext, marker_dir: Option<PathBuf>) -> Vars {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let no_file = || Err("this build needs a saved file".to_owned());
    let mut v: Vars = HashMap::new();
    let file = ctx.file.as_deref();
    let file_dir = file.and_then(|f| f.parent());
    v.insert("file", file.map(s).ok_or(()).or_else(|_| no_file()));
    v.insert("file_path", file_dir.map(s).ok_or(()).or_else(|_| no_file()));
    v.insert(
        "file_name",
        file.and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()).ok_or(()).or_else(|_| no_file()),
    );
    v.insert(
        "file_base_name",
        file.and_then(|f| f.file_stem()).map(|n| n.to_string_lossy().into_owned()).ok_or(()).or_else(|_| no_file()),
    );
    v.insert(
        "file_extension",
        file.map(|f| f.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()).ok_or(()).or_else(|_| no_file()),
    );
    v.insert("folder", ctx.folder.as_deref().map(s).ok_or_else(|| "this build needs an open folder".to_owned()));
    let project = marker_dir.or_else(|| ctx.folder.clone()).or_else(|| file_dir.map(Path::to_owned));
    v.insert("project_path", project.as_deref().map(s).ok_or_else(|| "this build needs a saved file or an open folder".to_owned()));
    v.insert(
        "project_name",
        project
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| "this build needs a saved file or an open folder".to_owned()),
    );
    v.insert("exe", Ok(if cfg!(windows) { ".exe" } else { "" }.to_owned()));
    v
}

#[derive(Clone, Copy, PartialEq)]
enum Quote {
    None,
    Shell,
}

/// Quote `s` for `sh -c` / `cmd /C` when it has anything a shell would split or expand.
fn shell_quote(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_-./:=+,@%\\".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        return s.to_owned();
    }
    if cfg!(windows) { format!("\"{}\"", s.replace('"', "\"\"")) } else { format!("'{}'", s.replace('\'', r"'\''")) }
}

/// Substitute `$name`, `${name}` and `$$` in `s`.
fn expand(s: &str, vars: &Vars, quote: Quote) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix('$') {
            out.push('$');
            rest = r;
            continue;
        }
        let (name, raw, after) = if let Some(r) = rest.strip_prefix('{') {
            match r.find('}') {
                Some(end) => (&r[..end], &rest[..end + 2], &r[end + 1..]),
                None => ("", "", rest),
            }
        } else {
            let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
            (&rest[..end], &rest[..end], &rest[end..])
        };
        if name.is_empty() {
            out.push('$');
            continue;
        }
        let value = match vars.get(name) {
            Some(Ok(v)) => Some(v.clone()),
            Some(Err(e)) => return Err(format!("${name}: {e}")),
            None => std::env::var(name).ok(),
        };
        match value {
            Some(v) if quote == Quote::Shell => out.push_str(&shell_quote(&v)),
            Some(v) => out.push_str(&v),
            None => {
                out.push('$');
                out.push_str(raw);
            }
        }
        rest = after;
    }
    out.push_str(rest);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// A location parsed from the output. `line` and `col` are 0-based.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildResult {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub message: String,
    /// Absolute output line it came from.
    output_line: usize,
}

/// Parse one output line with `re`; relative paths resolve against `base`.
/// Returns (path, 0-based line, 0-based col, message).
pub fn parse_result(re: &Regex, line: &str, base: &Path) -> Option<(PathBuf, usize, usize, String)> {
    let f = fields(re, &re.captures(line)?)?;
    let path = normalize(&base.join(f.file.as_str().trim()));
    Some((path, f.line, f.col, f.message.trim().to_owned()))
}

/// What a `file_regex` match says: the file as matched, 0-based line and
/// column, the message, and where the location's text ends.
struct Fields<'t> {
    file: regex::Match<'t>,
    line: usize,
    col: usize,
    message: &'t str,
    /// Where the line number starts, and where the line or column ends.
    line_start: usize,
    end: usize,
}

/// Named groups `file`, `line`, `col`, `message` (or `line2`/`col2` in an
/// alternative), else positional groups 1-4.
fn fields<'t>(re: &Regex, caps: &regex::Captures<'t>) -> Option<Fields<'t>> {
    let named = re.capture_names().any(|n| n == Some("file"));
    let get = |name: &str, alt: &str, pos: usize| -> Option<regex::Match<'t>> {
        let m = if named { caps.name(name).or_else(|| caps.name(alt)) } else { caps.get(pos) };
        m.filter(|m| !m.as_str().is_empty())
    };
    let file = get("file", "file", 1)?;
    let line = get("line", "line2", 2)?;
    let col = get("col", "col2", 3);
    let line_no: usize = line.as_str().parse().ok()?;
    let col_no: usize = col.and_then(|c| c.as_str().parse().ok()).unwrap_or(1);
    let message = get("message", "message", 4).map_or("", |m| m.as_str());
    let end = col.map_or(line.end(), |c| c.end());
    Some(Fields { file, line: line_no.saturating_sub(1), col: col_no.saturating_sub(1), message, line_start: line.start(), end })
}

/// A file location in a line of terminal output. `line` and `col` are
/// 0-based; `span` is the byte range of the text naming it (path through
/// line and column), for underlining.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextLocation {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub span: std::ops::Range<usize>,
}

/// The patterns the terminal looks for: the build systems' rustc, Python,
/// Node and tsc ones (which know their stack-trace lines), then a location anywhere.
static TEXT_PATTERNS: std::sync::LazyLock<Vec<Regex>> =
    std::sync::LazyLock::new(|| [RUSTC_REGEX, PYTHON_REGEX, NODE_REGEX, NPM_REGEX, ANYWHERE_REGEX].iter().map(|r| Regex::new(r).expect("built-in regex")).collect());

/// Every file location in `text`, in order, without checking that the files
/// exist. Where patterns overlap the earlier one in `TEXT_PATTERNS` wins.
pub fn find_locations(text: &str) -> Vec<TextLocation> {
    let mut found: Vec<TextLocation> = Vec::new();
    for re in TEXT_PATTERNS.iter() {
        for caps in re.captures_iter(text) {
            let Some(f) = fields(re, &caps) else { continue };
            let raw = f.file.as_str();
            let file = raw.trim();
            let start = f.file.start() + raw.len() - raw.trim_start().len();
            // `path(3,5)`: the closing parenthesis belongs to the location.
            let end = if text[..f.line_start].ends_with('(') && text[f.end..].starts_with(')') { f.end + 1 } else { f.end };
            let span = start..end;
            if found.iter().any(|l| l.span.start < span.end && span.start < l.span.end) {
                continue;
            }
            found.push(TextLocation { file: file.to_owned(), line: f.line, col: f.col, span });
        }
    }
    found.sort_by_key(|l| l.span.start);
    found
}

/// Drop `.` components and fold `..` lexically, so `./src/a.rs` matches open tabs.
pub(crate) fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.components().next_back(), Some(Component::Normal(_))) => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Output buffer: ANSI stripping, \r collapsing, line cap, result parsing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Level {
    Normal,
    Warning,
    Error,
    /// "[Finished in 1.2s]" and friends.
    Info,
}

struct Line {
    text: String,
    level: Level,
    /// Absolute result id.
    result: Option<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum Ansi {
    Text,
    Esc,
    Csi,
    Osc,
    OscEsc,
    /// `ESC ( B` and friends: one more char to drop.
    Charset,
}

struct Output {
    lines: VecDeque<Line>,
    /// Absolute index of `lines[0]`.
    first: usize,
    /// The last line is still being written (no newline yet).
    open: bool,
    /// A `\r` was seen; the next char decides between CRLF and overwrite.
    cr: bool,
    esc: Ansi,
    results: VecDeque<BuildResult>,
    results_dropped: usize,
    regex: Option<Regex>,
    base: PathBuf,
    /// Latest `error…` / `warning…` line, the message for location-only matches.
    heading: Option<(String, Level)>,
}

impl Output {
    fn new(regex: Option<Regex>, base: PathBuf) -> Self {
        Self {
            lines: VecDeque::new(),
            first: 0,
            open: false,
            cr: false,
            esc: Ansi::Text,
            results: VecDeque::new(),
            results_dropped: 0,
            regex,
            base,
            heading: None,
        }
    }

    fn current(&mut self) -> &mut String {
        if !self.open {
            self.lines.push_back(Line { text: String::new(), level: Level::Normal, result: None });
            self.open = true;
            self.trim();
        }
        &mut self.lines.back_mut().expect("open line").text
    }

    fn push_str(&mut self, s: &str) {
        for c in s.chars() {
            match self.esc {
                Ansi::Esc => {
                    self.esc = match c {
                        '[' => Ansi::Csi,
                        ']' => Ansi::Osc,
                        '(' | ')' | '*' | '+' => Ansi::Charset,
                        _ => Ansi::Text,
                    };
                    continue;
                }
                Ansi::Charset => {
                    self.esc = Ansi::Text;
                    continue;
                }
                Ansi::Csi => {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        self.esc = Ansi::Text;
                    }
                    continue;
                }
                Ansi::Osc => {
                    match c {
                        '\u{7}' => self.esc = Ansi::Text,
                        '\u{1b}' => self.esc = Ansi::OscEsc,
                        _ => {}
                    }
                    continue;
                }
                Ansi::OscEsc => {
                    self.esc = if c == '\\' { Ansi::Text } else { Ansi::Osc };
                    continue;
                }
                Ansi::Text => {}
            }
            if self.cr && c != '\n' {
                // Bare carriage return: the next text overwrites the line (progress bars).
                self.cr = false;
                if self.open {
                    self.current().clear();
                }
            }
            match c {
                '\u{1b}' => self.esc = Ansi::Esc,
                '\r' => self.cr = true,
                '\n' => {
                    self.cr = false;
                    self.current();
                    self.close_line();
                }
                '\u{8}' => {
                    if self.open {
                        self.current().pop();
                    }
                }
                '\t' => self.current().push_str("    "),
                c if c.is_control() => {}
                c => self.current().push(c),
            }
        }
    }

    /// End the current line and classify it.
    fn close_line(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        let Some(line) = self.lines.back() else { return };
        let text = line.text.clone();
        let lower = text.trim_start().to_ascii_lowercase();
        let mut level = if lower.starts_with("error")
            || lower.starts_with("fatal")
            || lower.starts_with("traceback")
            || lower.contains(": error")
            || lower.contains(" error:")
            || lower.contains("error ts")
        {
            Level::Error
        } else if lower.starts_with("warning") || lower.contains(": warning") {
            Level::Warning
        } else {
            Level::Normal
        };
        if (lower.starts_with("error") || lower.starts_with("warning")) && level != Level::Normal {
            self.heading = Some((text.trim().to_owned(), level));
        }
        let mut result = None;
        if let Some(re) = &self.regex
            && let Some((path, ln, col, mut message)) = parse_result(re, &text, &self.base)
        {
            if message.is_empty()
                && let Some((h, l)) = &self.heading
            {
                message = h.clone();
                level = *l;
            }
            if level == Level::Normal {
                level = Level::Error;
            }
            let id = self.results_dropped + self.results.len();
            let output_line = self.first + self.lines.len() - 1;
            self.results.push_back(BuildResult { path, line: ln, col, message, output_line });
            result = Some(id);
        }
        let line = self.lines.back_mut().expect("line");
        line.level = level;
        line.result = result;
    }

    /// A whole line of our own ("[Finished in 1.2s]").
    fn info(&mut self, text: &str) {
        if self.open {
            self.close_line();
        }
        self.lines.push_back(Line { text: text.to_owned(), level: Level::Info, result: None });
        self.trim();
    }

    fn trim(&mut self) {
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
            self.first += 1;
        }
        while self.results.front().is_some_and(|r| r.output_line < self.first) {
            self.results.pop_front();
            self.results_dropped += 1;
        }
    }

    fn result(&self, id: usize) -> Option<&BuildResult> {
        id.checked_sub(self.results_dropped).and_then(|i| self.results.get(i))
    }

    fn text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

enum Msg {
    Text(String),
    /// The process could not be started.
    Failed(String),
    Exit { code: Option<i32>, cancelled: bool, elapsed: Duration },
}

/// What the panel asks the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildAction {
    /// Open `path` at 0-based `line`, `col` (chars).
    Goto(PathBuf, usize, usize),
}

/// The build runner and its results panel.
pub struct Build {
    pub systems: BuildSystems,
    /// Whether the app should show the panel. Set by starting a build; the panel's ×
    /// clears it.
    pub visible: bool,
    config_dir: Option<PathBuf>,
    /// Remembered by `build_with` and reused by `build` (like Sublime's last choice).
    last: Option<BuildChoice>,
    title: String,
    command: String,
    out: Output,
    running: bool,
    started: Option<Instant>,
    /// Final status line once done, e.g. "[Finished in 1.2s]".
    finished: Option<String>,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    repaint: Option<egui::Context>,
    /// Absolute result id last jumped to.
    current: Option<usize>,
    /// Scroll the panel to this absolute line next frame.
    scroll_to: Option<usize>,
}

impl Build {
    /// Loads built-in and user build systems from `config_dir`.
    pub fn new(config_dir: Option<PathBuf>) -> Self {
        Self {
            systems: BuildSystems::load(config_dir.as_deref()),
            visible: false,
            config_dir,
            last: None,
            title: String::new(),
            command: String::new(),
            out: Output::new(None, PathBuf::new()),
            running: false,
            started: None,
            finished: None,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            repaint: None,
            current: None,
            scroll_to: None,
        }
    }

    /// Re-read `<config_dir>/build_systems`. Returns load errors for the status bar.
    pub fn reload_systems(&mut self) -> &[String] {
        self.systems = BuildSystems::load(self.config_dir.as_deref());
        &self.systems.errors
    }

    /// Entries for "Build With…" (systems and their variants), best first.
    pub fn choices(&self, ctx: &BuildContext) -> Vec<BuildChoice> {
        self.systems.choices(ctx)
    }

    /// The system ⌘B would run now.
    #[cfg(test)]
    pub fn next_system(&self, ctx: &BuildContext) -> Option<String> {
        if let Some(last) = &self.last
            && self.systems.get(&last.system).is_some_and(|s| s.score(ctx).is_some())
        {
            return Some(last.system.clone());
        }
        self.systems.automatic(ctx).map(|s| s.name.clone())
    }

    /// Forget the last "Build With…" choice (a build system was picked in
    /// Tools ▸ Build System, Automatic included).
    pub fn forget_last(&mut self) {
        self.last = None;
    }

    /// ⌘B: rerun the last "Build With…" choice if it still applies, else Automatic.
    pub fn build(&mut self, ctx: &BuildContext) -> Result<(), String> {
        if let Some(last) = self.last.clone()
            && self.systems.get(&last.system).is_some_and(|s| s.score(ctx).is_some())
        {
            return self.run_choice(&last, ctx);
        }
        let sys = self.systems.automatic(ctx).cloned().ok_or_else(|| "No build system for this file".to_owned())?;
        self.start(&sys, None, ctx)
    }

    /// ⇧⌘B: run `choice` and remember it for ⌘B.
    pub fn build_with(&mut self, choice: &BuildChoice, ctx: &BuildContext) -> Result<(), String> {
        self.last = Some(choice.clone());
        self.run_choice(choice, ctx)
    }

    /// Run `choice` without remembering it (a system pinned in Tools ▸ Build System).
    pub fn run_choice(&mut self, choice: &BuildChoice, ctx: &BuildContext) -> Result<(), String> {
        let sys = self.systems.get(&choice.system).cloned().ok_or_else(|| format!("No build system \"{}\"", choice.system))?;
        self.start(&sys, choice.variant.as_deref(), ctx)
    }

    /// Run `system` (or its `variant`) in the background, replacing any running build.
    /// Errors (missing file, bad working dir, bad regex) are returned and also shown in
    /// the panel.
    pub fn start(&mut self, system: &BuildSystem, variant: Option<&str>, ctx: &BuildContext) -> Result<(), String> {
        self.cancel();
        self.rx = None;
        self.visible = true;
        self.current = None;
        self.scroll_to = None;
        self.finished = None;
        self.running = false;
        let prepared = match system.prepare(variant, ctx) {
            Ok(p) => p,
            Err(e) => return Err(self.fail_early(&system.name, e)),
        };
        let regex = match prepared.file_regex.as_deref().map(Regex::new).transpose() {
            Ok(r) => r,
            Err(e) => return Err(self.fail_early(&prepared.title, format!("bad file_regex: {e}"))),
        };
        self.title = prepared.title.clone();
        self.command = prepared.argv.join(" ");
        self.out = Output::new(regex, prepared.cwd.clone());
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.running = true;
        self.started = Some(Instant::now());
        let (cancel, repaint) = (self.cancel.clone(), self.repaint.clone());
        std::thread::spawn(move || run(prepared, tx, cancel, repaint));
        Ok(())
    }

    fn fail_early(&mut self, title: &str, e: String) -> String {
        log::warn!("build {title:?} didn't start: {e}");
        self.title = title.to_owned();
        self.command.clear();
        self.out = Output::new(None, PathBuf::new());
        self.out.info(&format!("[{e}]"));
        self.finished = Some(format!("[{e}]"));
        e
    }

    /// Cancel Build: kill the process (and its process group on Unix / tree on Windows).
    pub fn cancel(&mut self) {
        if self.running {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Final status ("[Finished in 1.2s]", "[Cancelled]", …) once the build ended.
    #[cfg(test)]
    pub fn status(&self) -> Option<&str> {
        self.finished.as_deref()
    }

    /// The whole panel text.
    #[cfg(test)]
    pub fn output_text(&self) -> String {
        self.out.text()
    }


    /// Drain output from the build thread. Call once per frame (show() does too).
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.repaint.is_none() {
            self.repaint = Some(ctx.clone());
        }
        self.drain();
        if self.running {
            // Keep the spinner and elapsed time moving.
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn drain(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut done = false;
        loop {
            let msg = match rx.try_recv() {
                Ok(msg) => msg,
                // The build thread ended without a word: it panicked (and logged it).
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Msg::Failed("The build stopped because of an internal error; see Help ▸ Show Latest Log".into()),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            };
            match msg {
                Msg::Text(s) => self.out.push_str(&s),
                Msg::Failed(e) => {
                    self.out.info(&format!("[{e}]"));
                    self.finished = Some(format!("[{e}]"));
                    done = true;
                }
                Msg::Exit { code, cancelled, elapsed } => {
                    let secs = format!("{:.1}s", elapsed.as_secs_f64());
                    let line = if cancelled {
                        "[Cancelled]".to_owned()
                    } else {
                        match code {
                            Some(0) => format!("[Finished in {secs}]"),
                            Some(c) => format!("[Finished in {secs} with exit code {c}]"),
                            None => format!("[Finished in {secs}, terminated by a signal]"),
                        }
                    };
                    self.out.info(&line);
                    self.finished = Some(line);
                    done = true;
                }
            }
            if done {
                break;
            }
        }
        if done {
            // Late output from orphaned grandchildren is dropped after the status line.
            self.rx = None;
            self.running = false;
        }
    }

    /// F4: the next result location (wraps around), highlighted in the panel.
    pub fn next_result(&mut self) -> Option<BuildAction> {
        self.step_result(true)
    }

    /// ⇧F4: the previous result location (wraps around).
    pub fn prev_result(&mut self) -> Option<BuildAction> {
        self.step_result(false)
    }

    fn step_result(&mut self, forward: bool) -> Option<BuildAction> {
        let (lo, n) = (self.out.results_dropped, self.out.results.len());
        if n == 0 {
            return None;
        }
        let hi = lo + n - 1;
        let id = match self.current.filter(|c| (lo..=hi).contains(c)) {
            None if forward => lo,
            None => hi,
            Some(c) if forward => if c == hi { lo } else { c + 1 },
            Some(c) => if c == lo { hi } else { c - 1 },
        };
        self.goto(id)
    }

    fn goto(&mut self, id: usize) -> Option<BuildAction> {
        let r = self.out.result(id)?;
        let action = BuildAction::Goto(r.path.clone(), r.line, r.col);
        self.current = Some(id);
        self.scroll_to = Some(r.output_line);
        self.visible = true;
        Some(action)
    }

    /// Draw the panel (header + output). Put it in a resizable bottom panel.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<BuildAction> {
        self.poll(&ui.ctx().clone());
        let mut action = None;
        ui.horizontal(|ui| {
            if self.running {
                ui.add(egui::Spinner::new().size(12.0));
            }
            let title = if self.title.is_empty() { "Build Results" } else { &self.title };
            ui.label(egui::RichText::new(title).strong());
            if self.running {
                let secs = self.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
                ui.label(egui::RichText::new(format!("{secs:.1}s")).weak().small());
            } else if let Some(f) = &self.finished {
                ui.label(egui::RichText::new(f.trim_matches(['[', ']'])).weak().small());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("Hide build results").clicked() {
                    self.visible = false;
                }
                if ui.add_enabled(self.running, egui::Button::new("Cancel").small()).clicked() {
                    self.cancel();
                }
                if ui.small_button("Copy").on_hover_text("Copy the output").clicked() {
                    ui.ctx().copy_text(self.out.text());
                }
                let n = self.out.results.len();
                if n > 0 {
                    if ui.small_button("▼").on_hover_text("Next result (F4)").clicked() {
                        action = self.next_result();
                    }
                    if ui.small_button("▲").on_hover_text("Previous result (Shift+F4)").clicked() {
                        action = self.prev_result();
                    }
                    let label = if n == 1 { "1 result".to_owned() } else { format!("{n} results") };
                    ui.label(egui::RichText::new(label).small());
                }
                if !self.command.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(&self.command).weak().small().monospace()).truncate())
                        .on_hover_text(format!("{}\nin {}", self.command, self.out.base.display()));
                }
            });
        });
        ui.separator();
        if let Some(a) = self.output(ui) {
            action = Some(a);
        }
        action
    }

    fn output(&mut self, ui: &mut egui::Ui) -> Option<BuildAction> {
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
        let visuals = ui.visuals().clone();
        let mut scroll = egui::ScrollArea::both().id_salt("build_output").auto_shrink(false);
        match self.scroll_to.take() {
            Some(abs) => {
                let row = abs.saturating_sub(self.out.first) as f32;
                let spacing = ui.spacing().item_spacing.y;
                let y = (row * (row_h + spacing) - ui.available_height() / 3.0).max(0.0);
                scroll = scroll.vertical_scroll_offset(y);
            }
            // Follow new output unless the user scrolled up.
            None => scroll = scroll.stick_to_bottom(true),
        }
        let mut clicked = None;
        scroll.show_rows(ui, row_h, self.out.lines.len(), |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for i in range {
                let line = &self.out.lines[i];
                let color = match line.level {
                    Level::Error => visuals.error_fg_color,
                    Level::Warning => visuals.warn_fg_color,
                    Level::Info => visuals.weak_text_color(),
                    Level::Normal => visuals.text_color(),
                };
                let mut fmt = egui::TextFormat::simple(font.clone(), color);
                let is_current = line.result.is_some() && line.result == self.current;
                if is_current {
                    fmt.background = visuals.selection.bg_fill;
                }
                let text = if line.text.is_empty() { " " } else { &line.text };
                let job = egui::text::LayoutJob::single_section(text.to_owned(), fmt);
                let Some(id) = line.result else {
                    ui.add(egui::Label::new(job).selectable(false));
                    continue;
                };
                let resp = ui.add(egui::Label::new(job).selectable(false).sense(egui::Sense::click()));
                let resp = match self.out.result(id) {
                    Some(r) if !r.message.is_empty() => resp.on_hover_text(&r.message),
                    _ => resp,
                };
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    ui.painter().hline(resp.rect.x_range(), resp.rect.bottom() - 1.0, egui::Stroke::new(1.0, color));
                }
                if resp.clicked() {
                    clicked = Some(id);
                }
            }
        });
        clicked.and_then(|id| {
            let a = self.goto(id);
            self.scroll_to = None;
            a
        })
    }
}

impl Drop for Build {
    fn drop(&mut self) {
        // Don't leave a build running after the editor quits.
        self.cancel();
    }
}

// ---------------------------------------------------------------------------
// The build thread
// ---------------------------------------------------------------------------

fn command(p: &Prepared) -> Command {
    let mut cmd = if p.shell {
        let line = p.argv.join(" ");
        if cfg!(windows) {
            let mut c = Command::new("cmd");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                // /S strips exactly the outer quotes, leaving the user's line untouched.
                c.raw_arg(format!("/S /C \"{line}\""));
            }
            #[cfg(not(windows))]
            c.arg(line);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(line);
            c
        }
    } else {
        let mut c = Command::new(&p.argv[0]);
        c.args(&p.argv[1..]);
        c
    };
    cmd.current_dir(&p.cwd).stdin(Stdio::null());
    if let Some(path) = augmented_path() {
        cmd.env("PATH", path);
    }
    for (k, v) in &p.env {
        cmd.env(k, v);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so cancel can kill the whole tree.
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// GUI apps on macOS start with a bare PATH; add the usual toolchain dirs that exist.
pub(crate) fn augmented_path() -> Option<std::ffi::OsString> {
    if cfg!(windows) {
        return None;
    }
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&current).collect();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut extra: Vec<PathBuf> = ["/opt/homebrew/bin", "/usr/local/bin"].iter().map(PathBuf::from).collect();
    if let Some(h) = home {
        extra.extend([h.join(".cargo/bin"), h.join(".local/bin"), h.join("go/bin"), h.join(".bun/bin")]);
    }
    let before = dirs.len();
    for d in extra {
        if d.is_dir() && !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    if dirs.len() == before {
        return None;
    }
    std::env::join_paths(dirs).ok()
}

fn repaint(ctx: &Option<egui::Context>) {
    if let Some(c) = ctx {
        c.request_repaint();
    }
}

fn run(p: Prepared, tx: Sender<Msg>, cancel: Arc<AtomicBool>, ctx: Option<egui::Context>) {
    let started = Instant::now();
    let fail = |e: String| {
        log::warn!("build {:?} didn't start: {e}", p.title);
        let _ = tx.send(Msg::Failed(e));
        repaint(&ctx);
    };
    // One pipe for both stdout and stderr keeps them in the order they were written.
    let (reader, writer) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return fail(format!("Could not create a pipe: {e}")),
    };
    let writer2 = match writer.try_clone() {
        Ok(w) => w,
        Err(e) => return fail(format!("Could not create a pipe: {e}")),
    };
    let mut cmd = command(&p);
    cmd.stdout(writer).stderr(writer2);
    let spawned = cmd.spawn();
    // Drop our copies of the write end, or the reader never sees EOF.
    drop(cmd);
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            let what = if p.shell { "the shell" } else { p.argv[0].as_str() };
            return fail(format!("Could not run {what}: {e}"));
        }
    };

    let (done_tx, done_rx) = channel::<()>();
    {
        let (tx, ctx) = (tx.clone(), ctx.clone());
        std::thread::spawn(move || read_output(reader, &tx, &ctx, done_tx));
    }

    let mut cancelled = false;
    let mut term_sent: Option<Instant> = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {}
            Err(_) => break None,
        }
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            match term_sent {
                None => {
                    kill_tree(&mut child, false);
                    term_sent = Some(Instant::now());
                }
                Some(t) if t.elapsed() > Duration::from_millis(500) => {
                    kill_tree(&mut child, true);
                    break child.wait().ok();
                }
                Some(_) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let elapsed = started.elapsed();
    // Let the reader drain what's buffered; a background grandchild holding the pipe
    // open must not keep the build "running" forever.
    let _ = done_rx.recv_timeout(if cancelled { Duration::from_millis(200) } else { Duration::from_secs(1) });
    let _ = tx.send(Msg::Exit { code: status.and_then(|s| s.code()), cancelled, elapsed });
    repaint(&ctx);
}

/// Stream the pipe as text, keeping split UTF-8 sequences intact across reads.
fn read_output(mut reader: impl Read, tx: &Sender<Msg>, ctx: &Option<egui::Context>, done: Sender<()>) {
    let mut buf = [0u8; 8192];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        carry.extend_from_slice(&buf[..n]);
        let valid = match std::str::from_utf8(&carry) {
            Ok(_) => carry.len(),
            // An incomplete sequence at the end waits for the next read; invalid bytes
            // in the middle are replaced.
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => carry.len(),
        };
        let text = String::from_utf8_lossy(&carry[..valid]).into_owned();
        carry.drain(..valid);
        if !text.is_empty() {
            if tx.send(Msg::Text(text)).is_err() {
                break;
            }
            repaint(ctx);
        }
    }
    if !carry.is_empty() {
        let _ = tx.send(Msg::Text(String::from_utf8_lossy(&carry).into_owned()));
    }
    let _ = done.send(());
}

#[cfg(unix)]
fn kill_tree(child: &mut Child, force: bool) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGTERM: i32 = 15;
    const SIGKILL: i32 = 9;
    let Ok(pgid) = i32::try_from(child.id()) else { return };
    // SAFETY: kill(2) with a negative pid signals that process group; no memory involved.
    unsafe {
        kill(-pgid, if force { SIGKILL } else { SIGTERM });
    }
    if force {
        let _ = child.kill();
    }
}

#[cfg(windows)]
fn kill_tree(child: &mut Child, _force: bool) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // taskkill /T takes the children (cargo → rustc, cmd → program) with it.
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &child.id().to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
    let _ = child.kill();
}

#[cfg(not(any(unix, windows)))]
fn kill_tree(child: &mut Child, _force: bool) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-build-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Canonical, so marker walks and path comparisons agree (macOS /var → /private/var).
        dir.canonicalize().unwrap()
    }

    fn ctx(file: Option<&Path>, folder: Option<&Path>, syntax: Option<&str>) -> BuildContext {
        BuildContext { file: file.map(Path::to_owned), folder: folder.map(Path::to_owned), syntax: syntax.map(str::to_owned) }
    }

    #[test]
    fn variable_expansion() {
        let c = ctx(Some(Path::new("/src/my proj/main.rs")), Some(Path::new("/src/my proj")), None);
        let vars = variables(&c, None);
        let e = |s: &str| expand(s, &vars, Quote::None).unwrap();
        assert_eq!(e("$file"), "/src/my proj/main.rs");
        assert_eq!(e("$file_path"), "/src/my proj");
        assert_eq!(e("$file_name"), "main.rs");
        assert_eq!(e("$file_base_name.o"), "main.o");
        assert_eq!(e("${file_base_name}_test"), "main_test");
        assert_eq!(e("$file_extension"), "rs");
        assert_eq!(e("$folder"), "/src/my proj");
        assert_eq!(e("$project_name"), "my proj");
        assert_eq!(e("cost $$5 $not_a_real_var_xyz"), "cost $5 $not_a_real_var_xyz");
        assert_eq!(e("trailing $"), "trailing $");
        if cfg!(unix) {
            assert_eq!(expand("cc $file", &vars, Quote::Shell).unwrap(), "cc '/src/my proj/main.rs'");
            assert_eq!(e("$HOME"), std::env::var("HOME").unwrap());
        }
        // No saved file: file variables are an error, not an empty string.
        let vars = variables(&ctx(None, Some(Path::new("/x")), None), None);
        assert!(expand("python3 $file", &vars, Quote::None).unwrap_err().contains("saved file"));
        assert_eq!(expand("$project_path", &vars, Quote::None).unwrap(), "/x");
    }

    #[test]
    fn selects_builtins_by_syntax_and_project_files() {
        let systems = BuildSystems::load(None);
        let pick = |c: &BuildContext| systems.automatic(c).map(|s| s.name.clone());

        // Cargo project: Cargo beats single-file rustc; variants offered.
        let cargo = temp_dir("cargo");
        fs::create_dir_all(cargo.join("src")).unwrap();
        fs::write(cargo.join("Cargo.toml"), "[package]\n").unwrap();
        let main = cargo.join("src/main.rs");
        fs::write(&main, "fn main() {}\n").unwrap();
        let c = ctx(Some(&main), Some(&cargo), Some("Rust"));
        assert_eq!(pick(&c).as_deref(), Some("Cargo"));
        let labels: Vec<String> = systems.choices(&c).into_iter().map(|c| c.label).collect();
        for want in ["Cargo", "Cargo - Run", "Cargo - Test", "Cargo - Clippy", "Rust", "Rust - Run"] {
            assert!(labels.contains(&want.to_owned()), "{want} in {labels:?}");
        }
        let p = systems.get("Cargo").unwrap().prepare(Some("test"), &c).unwrap();
        assert_eq!(p.argv, ["cargo", "test"]);
        assert_eq!(p.cwd, cargo);
        // A README in the project still builds with Cargo; a loose .rs file uses rustc.
        assert_eq!(pick(&ctx(Some(&cargo.join("README.md")), Some(&cargo), Some("Markdown"))).as_deref(), Some("Cargo"));
        let loose = temp_dir("loose");
        let c = ctx(Some(&loose.join("hello.rs")), None, Some("Rust"));
        assert_eq!(pick(&c).as_deref(), Some("Rust"));
        let p = systems.get("Rust").unwrap().prepare(None, &c).unwrap();
        assert_eq!((p.argv.clone(), p.cwd.clone()), (vec!["rustc".to_owned(), loose.join("hello.rs").to_string_lossy().into_owned()], loose.clone()));

        // Python by syntax, and by extension when the syntax is unknown.
        let py = loose.join("x.py");
        assert_eq!(pick(&ctx(Some(&py), None, Some("Python"))).as_deref(), Some("Python"));
        assert_eq!(pick(&ctx(Some(&py), None, None)).as_deref(), Some("Python"));
        let p = systems.get("Python").unwrap().prepare(None, &ctx(Some(&py), None, None)).unwrap();
        assert_eq!(p.argv[1..], ["-u".to_owned(), py.to_string_lossy().into_owned()]);

        // Makefile folder: Make for C sources and with no file at all.
        let mk = temp_dir("make");
        fs::write(mk.join("Makefile"), "all:\n\techo hi\n").unwrap();
        assert_eq!(pick(&ctx(Some(&mk.join("a.c")), Some(&mk), Some("C"))).as_deref(), Some("Make"));
        assert_eq!(pick(&ctx(None, Some(&mk), None)).as_deref(), Some("Make"));
        assert_eq!(pick(&ctx(None, Some(&loose), None)), None);
        // npm needs package.json.
        assert!(systems.choices(&ctx(None, Some(&mk), None)).iter().all(|c| c.system != "npm"));
        for d in [cargo, loose, mk] {
            fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn user_systems_load_and_override() {
        let cfg = temp_dir("cfg");
        fs::create_dir_all(cfg.join("build_systems")).unwrap();
        fs::write(
            cfg.join("build_systems/lua.json"),
            r#"{"name": "Lua", "cmd": ["lua", "$file"], "selector": {"extensions": ["lua"]},
                "file_regex": "^lua: (.+?):(\\d+):", "priority": 1,
                "variants": [{"name": "Check", "cmd": ["luac", "-p", "$file"]}]}"#,
        )
        .unwrap();
        fs::write(cfg.join("build_systems/py.json"), r#"[{"name": "Python", "cmd": ["pypy", "$file"], "selector": {"syntaxes": ["Python"]}}]"#)
            .unwrap();
        fs::write(cfg.join("build_systems/bad.json"), "{ nope").unwrap();
        let systems = BuildSystems::load(Some(&cfg));
        assert_eq!(systems.errors.len(), 1, "{:?}", systems.errors);
        assert!(systems.errors[0].starts_with("bad.json"));
        let f = cfg.join("x.lua");
        let c = ctx(Some(&f), None, None);
        assert_eq!(systems.automatic(&c).unwrap().name, "Lua");
        let p = systems.get("lua").unwrap().prepare(Some("check"), &c).unwrap();
        assert_eq!(p.title, "Lua - Check");
        assert_eq!(p.argv, ["luac", "-p", &f.to_string_lossy()]);
        assert_eq!(systems.get("Python").unwrap().cmd[0], "pypy");
        assert_eq!(systems.systems.iter().filter(|s| s.name == "Python").count(), 1);
        fs::remove_dir_all(cfg).ok();
    }

    #[test]
    fn broken_user_systems_say_what_and_where() {
        let check = |text: &str, wants: &[&str]| {
            let e = parse_file(text).unwrap_err();
            assert!(!e.contains("untagged"), "{e}");
            for w in wants {
                assert!(e.contains(w), "{e:?} should mention {w:?}");
            }
        };
        // A string where a list belongs, on line 3.
        check("{\n  \"name\": \"Make\",\n  \"cmd\": \"make -j8\"\n}", &["\"cmd\"", "line 3", "list of strings", "[\"make\", \"-j8\"]"]);
        // A misspelled field, in the second system of a list.
        check("[{\"name\": \"A\", \"cmd\": [\"a\"]},\n {\"name\": \"B\", \"cmdd\": [\"b\"]}]", &["unknown field \"cmdd\"", "line 2", "cmd"]);
        // Wrong type inside the selector.
        check("{\"name\": \"X\", \"cmd\": [\"x\"],\n \"selector\": {\"extensions\": \"rs\"}}", &["\"extensions\"", "line 2", "list of strings"]);
        // Neither an object nor a list.
        check("\"make\"", &["a build system object", "or a list"]);
        // Plain JSON syntax errors keep their position.
        check("{ \"name\": }", &["line 1"]);
        assert_eq!(parse_file("[{\"name\": \"A\", \"cmd\": [\"a\"]}]").unwrap().len(), 1);
    }

    #[test]
    fn parses_rustc_gcc_python_results() {
        let base = Path::new("/proj");
        let rustc = Regex::new(RUSTC_REGEX).unwrap();
        assert_eq!(
            parse_result(&rustc, "   --> src/main.rs:12:5", base),
            Some((PathBuf::from("/proj/src/main.rs"), 11, 4, String::new()))
        );
        assert_eq!(parse_result(&rustc, "error[E0425]: cannot find value", base), None);

        let gcc = Regex::new(GCC_REGEX).unwrap();
        assert_eq!(
            parse_result(&gcc, "./a.c:3:10: error: expected ';' after expression", base),
            Some((PathBuf::from("/proj/a.c"), 2, 9, "error: expected ';' after expression".into()))
        );
        assert_eq!(parse_result(&gcc, "/abs/b.c:7: warning: x", base), Some((PathBuf::from("/abs/b.c"), 6, 0, "warning: x".into())));
        assert_eq!(parse_result(&gcc, "make: *** [all] Error 2", base), None);
        assert_eq!(
            parse_result(&gcc, r"C:\src\a.c:4:2: error: boom", base).map(|r| (r.1, r.2, r.3)),
            Some((3, 1, "error: boom".into()))
        );

        let py = Regex::new(PYTHON_REGEX).unwrap();
        assert_eq!(
            parse_result(&py, r#"  File "x.py", line 3, in <module>"#, base),
            Some((PathBuf::from("/proj/x.py"), 2, 0, String::new()))
        );
        assert_eq!(parse_result(&py, r#"  File "<frozen runpy>", line 88, in _run_code"#, base), None);

        // Positional groups for regexes without names; tsc-style alternates.
        let plain = Regex::new(r"^(\S+)\((\d+),(\d+)\): (.*)$").unwrap();
        assert_eq!(parse_result(&plain, "a.cs(9,2): oops", base), Some((PathBuf::from("/proj/a.cs"), 8, 1, "oops".into())));
        let npm = Regex::new(NPM_REGEX).unwrap();
        assert_eq!(
            parse_result(&npm, "src/a.ts(3,5): error TS2322: bad", base),
            Some((PathBuf::from("/proj/src/a.ts"), 2, 4, "error TS2322: bad".into()))
        );
        let node = Regex::new(NODE_REGEX).unwrap();
        assert_eq!(parse_result(&node, "    at f (/x/a.js:3:5)", base).map(|r| (r.0, r.1, r.2)), Some((PathBuf::from("/x/a.js"), 2, 4)));
    }

    /// (file, 1-based line, 1-based col, underlined text) for each location.
    fn located(text: &str) -> Vec<(String, usize, usize, &str)> {
        find_locations(text).into_iter().map(|l| (l.file, l.line + 1, l.col + 1, &text[l.span])).collect()
    }

    fn loc<'a>(file: &str, line: usize, col: usize, text: &'a str) -> (String, usize, usize, &'a str) {
        (file.to_owned(), line, col, text)
    }

    #[test]
    fn finds_unix_locations_anywhere_in_a_line() {
        assert_eq!(located("src/main.rs:42:7"), [loc("src/main.rs", 42, 7, "src/main.rs:42:7")]);
        assert_eq!(located("see ./lib/a.py:10 for details."), [loc("./lib/a.py", 10, 1, "./lib/a.py:10")]);
        assert_eq!(located("/abs/x.go:3: undefined: y"), [loc("/abs/x.go", 3, 1, "/abs/x.go:3")]);
        assert_eq!(
            located("thread 'main' panicked at src/main.rs:3:5:"),
            [loc("src/main.rs", 3, 5, "src/main.rs:3:5")],
            "a Rust panic"
        );
        assert_eq!(located("  --> src/lib.rs:12:9"), [loc("src/lib.rs", 12, 9, "src/lib.rs:12:9")], "rustc");
        assert_eq!(located("(a.rs:3) and 'b.rs:4:2'"), [loc("a.rs", 3, 1, "a.rs:3"), loc("b.rs", 4, 2, "b.rs:4:2")]);
        assert_eq!(located("file:///p/a.mjs:2:3"), [loc("/p/a.mjs", 2, 3, "/p/a.mjs:2:3")]);
        // Nothing that looks like a location without a line number.
        assert_eq!(located("error: could not compile `x` (bin) due to 1 previous error"), []);
        assert_eq!(located("src/main.rs and Cargo.toml"), []);
    }

    #[test]
    fn finds_windows_locations() {
        assert_eq!(located(r"C:\proj\x.cs(12,5): error CS1002: ; expected"), [loc(r"C:\proj\x.cs", 12, 5, r"C:\proj\x.cs(12,5)")]);
        assert_eq!(located(r"main.cpp(7): warning C4101"), [loc("main.cpp", 7, 1, "main.cpp(7)")]);
        assert_eq!(located(r"at D:\a\b.rs:3:1 here"), [loc(r"D:\a\b.rs", 3, 1, r"D:\a\b.rs:3:1")]);
        assert_eq!(located("src/a.ts(3,5): error TS2322"), [loc("src/a.ts", 3, 5, "src/a.ts(3,5)")], "tsc");
        assert_eq!(located(r"--> src\main.rs:2:4"), [loc(r"src\main.rs", 2, 4, r"src\main.rs:2:4")]);
    }

    #[test]
    fn finds_stack_trace_locations() {
        let py = r#"  File "/home/me/my proj/x.py", line 10, in <module>"#;
        assert_eq!(located(py), [loc("/home/me/my proj/x.py", 10, 1, r#"/home/me/my proj/x.py", line 10"#)], "Python quotes paths with spaces");
        assert_eq!(located(r#"  File "<frozen runpy>", line 88, in _run_code"#), []);
        assert_eq!(located("    at main (/abs/x.js:3:7)"), [loc("/abs/x.js", 3, 7, "/abs/x.js:3:7")], "Node");
        assert_eq!(located("    at /abs/y.ts:9:1"), [loc("/abs/y.ts", 9, 1, "/abs/y.ts:9:1")]);
        assert_eq!(located("x.rb:3:in 'foo': boom"), [loc("x.rb", 3, 1, "x.rb:3")], "Ruby");
    }

    #[test]
    fn output_takes_rustc_message_from_heading() {
        let mut out = Output::new(Some(Regex::new(RUSTC_REGEX).unwrap()), PathBuf::from("/p"));
        out.push_str("   Compiling x v0.1.0\nerror[E0425]: cannot find value `y`\n --> src/main.rs:2:5\n");
        assert_eq!(out.results.len(), 1);
        let r = &out.results[0];
        assert_eq!((r.path.clone(), r.line, r.col), (PathBuf::from("/p/src/main.rs"), 1, 4));
        assert_eq!(r.message, "error[E0425]: cannot find value `y`");
        assert_eq!(out.lines[1].level, Level::Error);
        assert_eq!(out.lines[2].result, Some(0));
    }

    #[test]
    fn output_strips_ansi_collapses_cr_and_caps() {
        let mut out = Output::new(None, PathBuf::new());
        // Escapes and CRLF split across chunks.
        out.push_str("\x1b[1;3");
        out.push_str("1merror\x1b[0m: bad\r");
        out.push_str("\nprogress 10%\rprogress 50%");
        assert_eq!(out.text(), "error: bad\nprogress 50%");
        out.push_str("\rprogress 100%\n\x1b]0;title\x07done\n");
        assert_eq!(out.text(), "error: bad\nprogress 100%\ndone");
        assert_eq!(out.lines[0].level, Level::Error);
        let mut big = Output::new(Some(Regex::new(GCC_REGEX).unwrap()), PathBuf::from("/p"));
        big.push_str("a.c:1:1: error: first\n");
        for i in 0..MAX_LINES + 5 {
            big.push_str(&format!("line {i}\n"));
        }
        big.push_str("a.c:2:1: error: last\n");
        assert_eq!(big.lines.len(), MAX_LINES);
        assert_eq!(big.results.len(), 1);
        assert_eq!(big.results_dropped, 1);
        assert_eq!(big.result(1).unwrap().message, "error: last");
    }

    fn wait(b: &mut Build) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while b.is_running() {
            assert!(Instant::now() < deadline, "build did not finish");
            b.drain();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn shell_system(line: &str) -> BuildSystem {
        let mut s = BuildSystem::new("Test", &[line]);
        s.shell = true;
        s.file_regex = Some(GCC_REGEX.into());
        s
    }

    #[test]
    fn runs_a_command_and_reports_exit_code() {
        let dir = temp_dir("run");
        let c = ctx(None, Some(&dir), None);
        let mut b = Build::new(None);
        let line = if cfg!(windows) { "echo hello" } else { "echo out; echo err 1>&2; printf 'x.c:4:2: error: boom\\n'" };
        b.start(&shell_system(line), None, &c).unwrap();
        assert!(b.is_running());
        wait(&mut b);
        let text = b.output_text();
        assert!(b.status().unwrap().starts_with("[Finished in "), "{text}");
        assert!(!b.status().unwrap().contains("exit code"), "{text}");
        if cfg!(windows) {
            assert!(text.contains("hello"), "{text}");
        } else {
            assert!(text.starts_with("out\nerr\nx.c:4:2: error: boom\n[Finished in "), "{text}");
            assert_eq!(b.next_result(), Some(BuildAction::Goto(dir.join("x.c"), 3, 1)));
            // Wraps around with one result.
            assert_eq!(b.prev_result(), Some(BuildAction::Goto(dir.join("x.c"), 3, 1)));
        }

        let fail = if cfg!(windows) { "exit /b 3" } else { "exit 3" };
        b.start(&shell_system(fail), None, &c).unwrap();
        wait(&mut b);
        assert!(b.status().unwrap().ends_with("with exit code 3]"), "{}", b.output_text());

        // Not shell: argv runs directly; a missing program is reported, not a panic.
        let mut s = BuildSystem::new("Nope", &["strata-no-such-program-xyz"]);
        s.working_dir = Some("$folder".into());
        b.start(&s, None, &c).unwrap();
        wait(&mut b);
        assert!(b.status().unwrap().starts_with("[Could not run"), "{}", b.output_text());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn cancel_kills_the_process() {
        let dir = temp_dir("cancel");
        let c = ctx(None, Some(&dir), None);
        let mut b = Build::new(None);
        let line = if cfg!(windows) { "ping -n 30 127.0.0.1 >NUL" } else { "echo started; sleep 30" };
        b.start(&shell_system(line), None, &c).unwrap();
        let t = Instant::now();
        if cfg!(unix) {
            while !b.output_text().contains("started") {
                assert!(t.elapsed() < Duration::from_secs(10));
                b.drain();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        b.cancel();
        wait(&mut b);
        assert!(t.elapsed() < Duration::from_secs(10));
        assert_eq!(b.status(), Some("[Cancelled]"));
        assert!(b.output_text().ends_with("[Cancelled]"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn prepare_errors_show_in_the_panel() {
        let mut b = Build::new(None);
        let py = b.systems.get("Python").unwrap().clone();
        let err = b.start(&py, None, &ctx(None, None, Some("Python"))).unwrap_err();
        assert!(err.contains("saved file"), "{err}");
        assert!(!b.is_running());
        assert!(b.visible);
        assert!(b.output_text().contains("saved file"));
        assert!(b.start(&py, Some("nope"), &ctx(None, None, None)).unwrap_err().contains("no variant"));
    }
}
