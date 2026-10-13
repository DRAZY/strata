//! Incremental syntax highlighting via syntect (reads .sublime-syntax / .tmTheme).
//!
//! Parser state is checkpointed every `CHUNK` lines. Highlighting a visible
//! line resumes from the nearest checkpoint, and an edit only discards state
//! from the edited line down. Work per frame is capped so a huge file never
//! freezes the UI: unfinished lines render plain and fill in on later frames.

use crate::buffer::Buffer;
use egui::Color32;
use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock};
use std::time::Duration;
use syntect::highlighting::{
    FontStyle, HighlightIterator, HighlightState, Highlighter as ThemeHighlighter, Theme, ThemeSet,
};
use syntect::parsing::syntax_definition::Pattern;
use syntect::parsing::{ParseState, Regex, ScopeStack, SyntaxDefinition, SyntaxReference, SyntaxSet};

const CHUNK: usize = 32;
/// Lines parsed per frame before yielding.
const BUDGET: usize = 4_000;
/// Lines longer than this render plain (minified files would stall the parser).
const MAX_LINE: usize = 20_000;

#[derive(Clone)]
pub struct Span {
    /// Byte range within the line text.
    pub range: Range<usize>,
    pub color: Color32,
    pub italic: bool,
    pub underline: bool,
}

pub type LineSpans = Arc<Vec<Span>>;

/// Precompiled by build.rs: syntect's defaults, bat's extra syntaxes (via
/// two-face) and `assets/syntaxes`. Deserializing it is lazy and takes ~1 ms.
static BUNDLED_SYNTAXES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/syntaxes.bin"));
/// License texts the bundled third-party grammars ask to be reproduced.
pub static SYNTAX_LICENSES: &str = include_str!(concat!(env!("OUT_DIR"), "/syntax-licenses.txt"));

/// Config folder whose `Syntaxes/` and `Themes/` add to the bundled ones.
static USER_DIR: OnceLock<PathBuf> = OnceLock::new();
/// Problems loading user syntaxes and themes, for the status bar.
static LOAD_ERRORS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Loaded once and shared by every tab.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(|| {
    let (set, errors) = load_syntaxes(USER_DIR.get().map(|d| d.join("Syntaxes")).as_deref());
    LOAD_ERRORS.lock().unwrap_or_else(|e| e.into_inner()).extend(errors);
    set
});
static THEMES: LazyLock<ThemeSet> = LazyLock::new(|| {
    let (set, errors) = load_themes(USER_DIR.get().map(|d| d.join("Themes")).as_deref());
    LOAD_ERRORS.lock().unwrap_or_else(|e| e.into_inner()).extend(errors);
    set
});

/// Point syntax and theme loading at the config folder (call before any
/// highlighting) and load everything now. Returns problems with user files,
/// which were skipped.
pub fn load_user_assets(config_dir: Option<&Path>) -> Vec<String> {
    if let Some(dir) = config_dir {
        let _ = USER_DIR.set(dir.to_path_buf());
    }
    LazyLock::force(&SYNTAXES);
    LazyLock::force(&THEMES);
    std::mem::take(&mut *LOAD_ERRORS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The bundled syntaxes plus every `.sublime-syntax` in `dir`. A user file
/// that fails to parse, or has a regex the engine rejects, is skipped and
/// reported instead. User syntaxes win over bundled ones with the same name
/// or extension.
pub fn load_syntaxes(dir: Option<&Path>) -> (SyntaxSet, Vec<String>) {
    let bundled: SyntaxSet = syntect::dumps::from_uncompressed_data(BUNDLED_SYNTAXES).expect("bundled syntax dump");
    let files = dir.map(|d| files_with_extension(d, "sublime-syntax")).unwrap_or_default();
    if files.is_empty() {
        return (bundled, Vec::new());
    }
    // Relinking the whole set costs a few hundred ms, paid only when the user has syntaxes.
    let mut builder = bundled.into_builder();
    let mut errors = Vec::new();
    for path in files {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let stem = path.file_stem().and_then(|s| s.to_str());
        let def = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|src| SyntaxDefinition::load_from_str(&src, true, stem).map_err(|e| e.to_string()))
            .and_then(|def| bad_regex(&def).map_or(Ok(def), Err));
        match def {
            Ok(def) => builder.add(def),
            Err(e) => errors.push(format!("Syntax {name}: {e}")),
        }
    }
    (builder.build(), errors)
}

/// The first regex in `def` the regex engine can't compile, as an error.
/// (Patterns with backreferences to a begin match are only complete at parse time.)
fn bad_regex(def: &SyntaxDefinition) -> Option<String> {
    def.contexts.values().flat_map(|c| &c.patterns).find_map(|p| match p {
        Pattern::Match(m) if !m.has_captures => {
            Regex::try_compile(m.regex.regex_str()).map(|e| format!("bad regex `{}`: {e}", m.regex.regex_str()))
        }
        _ => None,
    })
}

/// The bundled color schemes plus every `.tmTheme` in `dir` (named after the file).
pub fn load_themes(dir: Option<&Path>) -> (ThemeSet, Vec<String>) {
    let mut set = ThemeSet::load_defaults();
    for (name, src) in [
        ("Strata Dark", include_str!("../assets/themes/Strata Dark.tmTheme")),
        ("Strata Light", include_str!("../assets/themes/Strata Light.tmTheme")),
    ] {
        let theme = ThemeSet::load_from_reader(&mut std::io::Cursor::new(src)).expect("bundled theme parses");
        set.themes.insert(name.into(), theme);
    }
    let mut errors = Vec::new();
    for path in dir.map(|d| files_with_extension(d, "tmTheme")).unwrap_or_default() {
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        match ThemeSet::get_theme(&path) {
            Ok(theme) => {
                set.themes.insert(name.to_owned(), theme);
            }
            Err(e) => errors.push(format!("Theme {name}: {e}")),
        }
    }
    (set, errors)
}

/// Files directly in `dir` ending in `.ext`, sorted. Empty if `dir` is missing.
fn files_with_extension(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
        .collect();
    files.sort();
    files
}

pub const DEFAULT_THEME: &str = "Strata Dark";

/// Every bundled theme name, sorted.
pub fn theme_names() -> Vec<&'static str> {
    THEMES.themes.keys().map(String::as_str).collect()
}

/// Syntax-highlighted HTML (inline styles) for `text`, for Copy as HTML.
pub fn to_html(text: &str, syntax: &str, theme: &str) -> String {
    let syntax = SYNTAXES.find_syntax_by_name(syntax).unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
    let theme = THEMES.themes.get(theme).unwrap_or(&THEMES.themes[DEFAULT_THEME]);
    syntect::html::highlighted_html_for_string(text, &SYNTAXES, syntax, theme).unwrap_or_else(|_| format!("<pre>{text}</pre>"))
}

/// The theme's foreground for a scope like "markup.heading", if it sets one
/// (Markdown preview colors).
pub fn scope_color(theme: &str, scope: &str) -> Option<Color32> {
    let theme = THEMES.themes.get(theme)?;
    let stack = <ScopeStack as std::str::FromStr>::from_str(scope).ok()?;
    let hl = ThemeHighlighter::new(theme);
    let style = hl.style_for_stack(stack.as_slice());
    let fg = theme.settings.foreground?;
    (style.foreground != fg).then(|| to_color(style.foreground))
}

/// Colored spans for a code snippet whose language is a fence tag like
/// "rust" or "py". None when the language is unknown (render it plain).
/// Byte ranges cover the whole snippet; very long snippets stop early.
pub fn highlight_snippet(code: &str, lang: &str, theme: &str) -> Option<Vec<Span>> {
    let lang = lang.trim();
    if lang.is_empty() {
        return None;
    }
    let syntax = syntax_for_fence(lang)?;
    let theme = THEMES.themes.get(theme).unwrap_or(&THEMES.themes[DEFAULT_THEME]);
    let mut h = syntect::easy::HighlightLines::new(syntax, theme);
    let mut spans = Vec::new();
    let mut offset = 0;
    for (n, line) in syntect::util::LinesWithEndings::from(code).enumerate() {
        if n >= 5_000 || line.len() > MAX_LINE {
            break;
        }
        let Ok(pieces) = h.highlight_line(line, &SYNTAXES) else { break };
        for (style, piece) in pieces {
            spans.push(Span {
                range: offset..offset + piece.len(),
                color: to_color(style.foreground),
                italic: style.font_style.contains(FontStyle::ITALIC),
                underline: false,
            });
            offset += piece.len();
        }
    }
    Some(spans)
}

/// Syntax name for a file by its name or extension, if it isn't plain text.
pub fn syntax_for_path(path: &Path) -> Option<String> {
    syntax_for_file(path).map(|s| s.name.clone()).filter(|n| n != "Plain Text")
}

/// Syntax for a file from its name alone: well-known names first
/// (Dockerfile.dev, .env.ci), then the whole name as listed by a syntax
/// (Makefile, Cargo.lock, .bashrc), then ever shorter compound extensions
/// (`html.erb` before `erb`, `d.ts` falls through to `ts`).
fn syntax_for_file(path: &Path) -> Option<&'static SyntaxReference> {
    let name = path.file_name()?.to_str()?;
    if let Some(s) = special_file_syntax(name).and_then(|n| SYNTAXES.find_syntax_by_name(n)) {
        return Some(s);
    }
    let mut rest = name;
    loop {
        if let Some(s) = SYNTAXES.find_syntax_by_extension(rest) {
            return Some(s);
        }
        rest = rest.split_once('.')?.1;
    }
}

/// Files the syntaxes' own extension lists miss or get ambiguous.
fn special_file_syntax(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e);
    Some(match (lower.as_str(), ext) {
        (n, _) if n.starts_with("dockerfile") || n.starts_with("containerfile") || n.ends_with(".dockerfile") => "Dockerfile",
        (n, _) if n.starts_with(".env.") || n == ".env" => "DotENV",
        (_, Some("env")) => "DotENV",
        (".babelrc" | ".eslintrc" | ".prettierrc" | ".swcrc" | "flake.lock" | "composer.lock", _) => "JSON",
        (_, Some("jsonc" | "json5")) => "JSON",
        (_, Some("mjs" | "cjs")) => "JavaScript",
        (_, Some("h")) => "C++",
        (_, Some("fs" | "fsi" | "fsx")) => "F#",
        (_, Some("sass")) => "Sass",
        (".zshrc" | ".zshenv" | ".zprofile" | ".bashrc" | ".bash_profile" | ".profile" | "pkgbuild", _) => "Bourne Again Shell (bash)",
        _ => return None,
    })
}

/// Syntax for a Markdown fence tag like "rust", "ts" or "shell".
fn syntax_for_fence(tag: &str) -> Option<&'static SyntaxReference> {
    let lower = tag.to_ascii_lowercase();
    let tag = match lower.as_str() {
        "shell" | "console" | "shellsession" | "zsh" | "sh" => "bash",
        "golang" => "go",
        "pwsh" | "powershell" | "ps" => "ps1",
        "terraform" | "hcl" => "tf",
        "dotenv" | "env" => ".env",
        "objc" | "objective-c" => "m",
        "csharp" | "c#" => "cs",
        "fsharp" | "f#" => "fs",
        "proto" | "protobuf" => "proto",
        "docker" => "dockerfile",
        "make" => "makefile",
        "jsonc" | "json5" => "json",
        "javascript" | "node" => "js",
        "typescript" => "ts",
        other => other,
    };
    special_file_syntax(tag)
        .and_then(|n| SYNTAXES.find_syntax_by_name(n))
        .or_else(|| SYNTAXES.find_syntax_by_token(tag))
        .or_else(|| syntax_for_file(Path::new(tag)))
}

/// Every bundled syntax name, sorted.
pub fn syntax_names() -> Vec<&'static str> {
    let mut names: Vec<_> = SYNTAXES.syntaxes().iter().filter(|s| !s.hidden).map(|s| s.name.as_str()).collect();
    names.sort_unstable_by_key(|n| n.to_lowercase());
    names
}

pub struct Highlighter {
    theme: &'static Theme,
    theme_name: String,
    syntax_name: String,
    /// `checkpoints[k]` = state at the start of line `k * CHUNK`.
    checkpoints: Vec<(ParseState, HighlightState)>,
    lines: HashMap<usize, LineSpans>,
    /// Compile a grammar's regexes off the UI thread the first time it is used.
    warm_up: bool,
}

/// One grammar's first-use regex compilation, run on a worker thread.
#[derive(Default)]
struct WarmUp {
    done: Mutex<bool>,
    finished: Condvar,
}

/// Grammars warmed up (or warming up) so far, by syntax name.
static WARM_UPS: LazyLock<Mutex<HashMap<String, Arc<WarmUp>>>> = LazyLock::new(Default::default);
/// How long the UI waits for a warm-up before drawing plain text. Most
/// grammars finish well inside it, so they never show unhighlighted.
const WARM_UP_WAIT: Duration = Duration::from_millis(40);

impl Highlighter {
    /// A highlighter for an editor: a grammar's first use compiles its regexes
    /// on a worker thread (TypeScript's take ~250 ms) while the text shows
    /// plain, instead of freezing the window.
    pub fn for_editor() -> Self {
        Self { warm_up: true, ..Self::new() }
    }

    pub fn new() -> Self {
        Self {
            theme: &THEMES.themes[DEFAULT_THEME],
            theme_name: DEFAULT_THEME.into(),
            syntax_name: "Plain Text".into(),
            checkpoints: Vec::new(),
            lines: HashMap::new(),
            warm_up: false,
        }
    }

    /// True once this grammar's regexes are compiled. The first call per
    /// grammar starts a worker that parses a copy of `lines` (compiled
    /// regexes are cached in the shared syntax set) and waits briefly for it.
    fn warmed_up(&self, buffer: &Buffer, lines: RangeInclusive<usize>) -> bool {
        let (warm, started) = {
            let mut all = WARM_UPS.lock().unwrap_or_else(|e| e.into_inner());
            match all.get(&self.syntax_name) {
                Some(w) => (w.clone(), false),
                None => {
                    let w = Arc::new(WarmUp::default());
                    all.insert(self.syntax_name.clone(), w.clone());
                    (w, true)
                }
            }
        };
        if started {
            let text: Vec<String> = lines.take(400).map(|l| buffer.line_text(l) + "\n").collect();
            let syntax = self.syntax();
            let worker = warm.clone();
            std::thread::spawn(move || {
                let mut parse = ParseState::new(syntax);
                for line in text.iter().filter(|l| l.len() <= MAX_LINE) {
                    if parse.parse_line(line, &SYNTAXES).is_err() {
                        break;
                    }
                }
                *worker.done.lock().unwrap_or_else(|e| e.into_inner()) = true;
                worker.finished.notify_all();
            });
        }
        let done = warm.done.lock().unwrap_or_else(|e| e.into_inner());
        if !started {
            return *done;
        }
        let (done, _) = warm.finished.wait_timeout_while(done, WARM_UP_WAIT, |d| !*d).unwrap_or_else(|e| e.into_inner());
        *done
    }

    pub fn background(&self) -> Color32 {
        self.theme
            .settings
            .background
            .map(to_color)
            .unwrap_or(Color32::from_rgb(0x2b, 0x30, 0x3b))
    }

    pub fn foreground(&self) -> Color32 {
        self.theme
            .settings
            .foreground
            .map(to_color)
            .unwrap_or(Color32::LIGHT_GRAY)
    }

    pub fn selection(&self) -> Color32 {
        self.theme
            .settings
            .selection
            .map(to_color)
            .unwrap_or(Color32::from_rgb(0x4f, 0x5b, 0x66))
    }

    /// Background of the line(s) holding a cursor.
    pub fn line_highlight(&self) -> Color32 {
        self.theme.settings.line_highlight.map(to_color).unwrap_or(Color32::from_white_alpha(8))
    }

    pub fn gutter_foreground(&self) -> Option<Color32> {
        self.theme.settings.gutter_foreground.map(to_color)
    }

    /// Indent guides.
    pub fn guide(&self) -> Color32 {
        self.theme.settings.guide.map(to_color).unwrap_or_else(|| self.foreground().gamma_multiply(0.12))
    }

    pub fn caret(&self) -> Color32 {
        self.theme
            .settings
            .caret
            .map(to_color)
            .unwrap_or(Color32::WHITE)
    }

    pub fn theme_name(&self) -> &str {
        &self.theme_name
    }

    /// Switch color scheme; unknown names are ignored.
    pub fn set_theme(&mut self, name: &str) {
        if let Some(theme) = THEMES.themes.get(name) {
            self.theme = theme;
            self.theme_name = name.into();
            self.invalidate_from(0);
        }
    }

    /// Force a syntax by name (from the syntax picker); unknown names are ignored.
    pub fn set_syntax(&mut self, name: &str) {
        if SYNTAXES.find_syntax_by_name(name).is_some() {
            self.syntax_name = name.into();
            self.invalidate_from(0);
        }
    }

    /// True for themes with a light background (the UI chrome follows).
    pub fn is_light(&self) -> bool {
        let bg = self.background();
        (bg.r() as u32 + bg.g() as u32 + bg.b() as u32) > 3 * 140
    }

    pub fn syntax_name(&self) -> &str {
        &self.syntax_name
    }

    /// Pick a syntax from the file name, falling back to the first line (shebangs).
    pub fn detect(&mut self, path: Option<&Path>, first_line: &str) {
        let by_first_line = || {
            let s = SYNTAXES.find_syntax_by_first_line(first_line)?;
            // Svelte claims `<html` first lines too; plain HTML is the better guess.
            Some(if s.name == "Svelte" { SYNTAXES.find_syntax_by_name("HTML").unwrap_or(s) } else { s })
        };
        let syntax = path
            .and_then(syntax_for_file)
            .or_else(by_first_line)
            .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
        self.syntax_name = syntax.name.clone();
        self.invalidate_from(0);
    }

    fn syntax(&self) -> &'static SyntaxReference {
        SYNTAXES
            .find_syntax_by_name(&self.syntax_name)
            .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text())
    }

    pub fn invalidate_from(&mut self, line: usize) {
        self.checkpoints.truncate(line / CHUNK + 1);
        if line == 0 {
            self.checkpoints.clear();
        }
        self.lines.retain(|&l, _| l < line);
    }

    /// Highlighted spans for a line, if ready. Call `prepare` first each frame.
    pub fn line(&self, line: usize) -> Option<LineSpans> {
        self.lines.get(&line).cloned()
    }

    /// Highlight `visible` lines. Returns false if the budget ran out
    /// (caller should repaint so highlighting continues next frame).
    pub fn prepare(&mut self, buffer: &Buffer, visible: RangeInclusive<usize>) -> bool {
        let last = (*visible.end()).min(buffer.len_lines().saturating_sub(1));
        let first = (*visible.start()).min(last);
        if (first..=last).all(|l| self.lines.contains_key(&l)) {
            return true;
        }
        if self.warm_up && !self.warmed_up(buffer, first.saturating_sub(CHUNK)..=last) {
            return false;
        }
        // Keep memory flat when scrolling through a huge file.
        if self.lines.len() > 8_192 {
            self.lines.retain(|&l, _| l + 2_000 >= first && l <= last + 2_000);
        }
        let hl = ThemeHighlighter::new(self.theme);
        if self.checkpoints.is_empty() {
            let parse = ParseState::new(self.syntax());
            let high = HighlightState::new(&hl, ScopeStack::new());
            self.checkpoints.push((parse, high));
        }

        // Resume from the nearest known checkpoint at or before `first`.
        let k = (first / CHUNK).min(self.checkpoints.len() - 1);
        let (mut parse, mut high) = self.checkpoints[k].clone();
        let mut budget = BUDGET;
        let mut line = k * CHUNK;
        while line <= last {
            if budget == 0 {
                return false;
            }
            budget -= 1;
            if line.is_multiple_of(CHUNK) && line / CHUNK == self.checkpoints.len() {
                self.checkpoints.push((parse.clone(), high.clone()));
            }
            let text = buffer.line_text(line);
            let spans = self.highlight_line(&hl, &mut parse, &mut high, &text);
            if line >= first {
                self.lines.insert(line, spans);
            }
            line += 1;
        }
        true
    }

    fn highlight_line(
        &self,
        hl: &ThemeHighlighter,
        parse: &mut ParseState,
        high: &mut HighlightState,
        text: &str,
    ) -> LineSpans {
        let fg = self.foreground();
        if text.len() > MAX_LINE {
            return Arc::new(vec![plain(0..text.len(), fg)]);
        }
        // The "newlines" syntax set expects each line to end with '\n'.
        let with_nl = format!("{text}\n");
        let Ok(ops) = parse.parse_line(&with_nl, &SYNTAXES) else {
            return Arc::new(vec![plain(0..text.len(), fg)]);
        };
        let mut spans = Vec::new();
        let mut offset = 0;
        for (style, piece) in HighlightIterator::new(high, &ops, &with_nl, hl) {
            let end = (offset + piece.len()).min(text.len());
            if end > offset {
                spans.push(Span {
                    range: offset..end,
                    color: to_color(style.foreground),
                    italic: style.font_style.contains(FontStyle::ITALIC),
                    underline: style.font_style.contains(FontStyle::UNDERLINE),
                });
            }
            offset += piece.len();
        }
        Arc::new(spans)
    }
}

fn plain(range: Range<usize>, color: Color32) -> Span {
    Span { range, color, italic: false, underline: false }
}

fn to_color(c: syntect::highlighting::Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_rehighlight_from_the_changed_line() {
        let mut text = String::new();
        for i in 0..200 {
            text.push_str(&format!("let x{i} = {i};\n"));
        }
        let buf = Buffer::from_str(&text);
        let mut h = Highlighter::new();
        h.detect(Some(Path::new("a.rs")), "");
        assert!(h.prepare(&buf, 150..=170));
        assert!(h.line(160).is_some());
        assert!(h.line(10).is_none(), "only visible lines are kept");
        h.invalidate_from(100);
        assert!(h.line(160).is_none());
        assert_eq!(h.checkpoints.len(), 100 / CHUNK + 1);
        assert!(h.prepare(&buf, 150..=170));
        // Rust keyword `let` should not be the plain foreground color.
        let spans = h.line(160).unwrap();
        assert_ne!(spans[0].color, h.foreground());
    }

    #[test]
    fn bundled_strata_themes_load() {
        let mut h = Highlighter::new();
        assert_eq!(h.theme_name(), "Strata Dark");
        assert!(!h.is_light());
        h.set_theme("Strata Light");
        assert!(h.is_light());
        assert!(theme_names().contains(&"Solarized (dark)"));
    }

    #[test]
    fn file_names_map_to_syntaxes() {
        for (file, syntax) in [
            ("a.ts", "TypeScript"),
            ("types.d.ts", "TypeScript"),
            ("a.mts", "TypeScript"),
            ("a.tsx", "TypeScriptReact"),
            ("a.jsx", "JavaScript (JSX)"),
            ("a.js", "JavaScript"),
            ("a.mjs", "JavaScript"),
            ("Cargo.toml", "TOML"),
            ("Cargo.lock", "TOML"),
            ("a.swift", "Swift"),
            ("a.kt", "Kotlin"),
            ("build.gradle.kts", "Kotlin"),
            ("Dockerfile", "Dockerfile"),
            ("Dockerfile.dev", "Dockerfile"),
            ("app.dockerfile", "Dockerfile"),
            ("Containerfile", "Dockerfile"),
            ("App.vue", "Vue Component"),
            ("App.svelte", "Svelte"),
            ("main.dart", "Dart"),
            ("mix.exs", "Elixir"),
            ("a.ex", "Elixir"),
            ("build.zig", "Zig"),
            ("flake.nix", "Nix"),
            ("main.tf", "Terraform"),
            ("a.hcl", "Terraform"),
            ("schema.graphql", "GraphQL"),
            ("a.proto", "Protocol Buffer"),
            ("a.scss", "SCSS"),
            ("a.sass", "Sass"),
            ("a.less", "Less"),
            ("profile.ps1", "PowerShell"),
            ("a.psm1", "PowerShell"),
            ("setup.ini", "INI"),
            (".editorconfig", "INI"),
            (".env", "DotENV"),
            (".env.local", "DotENV"),
            (".env.ci", "DotENV"),
            ("CMakeLists.txt", "CMake"),
            ("a.cmake", "CMake"),
            ("nginx.conf", "nginx"),
            ("a.fish", "Fish"),
            ("a.csv", "Separated Values"),
            ("a.diff", "Diff"),
            ("a.patch", "Diff"),
            ("Makefile", "Makefile"),
            ("GNUmakefile", "Makefile"),
            ("a.mk", "Makefile"),
            ("a.yaml", "YAML"),
            ("a.yml", "YAML"),
            ("a.json", "JSON"),
            ("tsconfig.json", "JSON"),
            ("a.jsonc", "JSON"),
            ("README.md", "Markdown"),
            (".bashrc", "Bourne Again Shell (bash)"),
            (".zshrc", "Bourne Again Shell (bash)"),
            ("a.zsh", "Bourne Again Shell (bash)"),
            ("Jenkinsfile", "Groovy"),
            ("a.h", "C++"),
            ("a.rs", "Rust"),
            ("a.py", "Python"),
            ("Gemfile", "Ruby"),
            ("go.mod", "Gomod"),
            ("requirements.txt", "Requirements.txt"),
        ] {
            assert_eq!(syntax_for_path(Path::new(file)).as_deref(), Some(syntax), "{file}");
        }
        assert_eq!(syntax_for_path(Path::new("notes.txt")), None);
        assert_eq!(syntax_for_path(Path::new("LICENSE")), None);
    }

    #[test]
    fn first_lines_pick_a_syntax() {
        let mut h = Highlighter::new();
        for (line, syntax) in [
            ("#!/usr/bin/env python3", "Python"),
            ("#!/bin/bash", "Bourne Again Shell (bash)"),
            ("#!/usr/bin/env node", "JavaScript"),
            ("<html>", "HTML"),
            ("<?xml version=\"1.0\"?>", "XML"),
        ] {
            h.detect(Some(Path::new("script")), line);
            assert_eq!(h.syntax_name(), syntax, "{line}");
        }
        h.detect(None, "hello");
        assert_eq!(h.syntax_name(), "Plain Text");
    }

    /// Distinct foreground colors when highlighting `code` as `file`'s syntax.
    fn colors(file: &str, code: &str) -> usize {
        let buf = Buffer::from_str(code);
        let mut h = Highlighter::new();
        h.detect(Some(Path::new(file)), "");
        assert!(h.prepare(&buf, 0..=buf.len_lines() - 1));
        let mut seen = std::collections::HashSet::new();
        for l in 0..buf.len_lines() {
            for s in h.line(l).unwrap().iter() {
                seen.insert(s.color);
            }
        }
        seen.len()
    }

    #[test]
    fn new_languages_actually_highlight() {
        for (file, code) in [
            ("a.ts", "interface P { x: number }\nexport const f = (p: P): string => `${p.x}`; // done\n"),
            ("a.tsx", "export function A() { return <div className=\"a\">{1}</div>; }\n"),
            ("a.jsx", "const el = <App title=\"hi\" />; // jsx\n"),
            ("Cargo.toml", "[package]\nname = \"strata\" # comment\nversion = 1\n"),
            ("a.swift", "import Foundation\nlet x: Int = 42 // answer\nfunc f() -> String { \"s\" }\n"),
            ("a.kt", "fun main() {\n    val s: String = \"hi\" // c\n    println(s)\n}\n"),
            ("Dockerfile", "FROM rust:1.80 AS build\nRUN cargo build --release # build\n"),
            ("a.ps1", "# comment\n$name = \"World\"\nWrite-Host \"Hello $name\" -ForegroundColor Green\n"),
            ("App.vue", "<template><div>{{ x }}</div></template>\n<script>\nexport default { data() { return { x: 1 } } }\n</script>\n"),
            ("main.tf", "resource \"aws_s3_bucket\" \"b\" {\n  bucket = \"x\" # c\n}\n"),
            ("a.proto", "syntax = \"proto3\";\nmessage M { int32 id = 1; }\n"),
            ("a.zig", "const std = @import(\"std\");\npub fn main() void {}\n"),
            ("a.nix", "{ pkgs ? import <nixpkgs> {} }: pkgs.hello # c\n"),
            ("a.graphql", "type Query { user(id: ID!): User }\n"),
            ("a.scss", "$c: #fff;\n.a { color: $c; } // c\n"),
            (".env", "# c\nKEY=\"value\"\n"),
            ("a.ini", "[section]\nkey = value ; c\n"),
            ("CMakeLists.txt", "cmake_minimum_required(VERSION 3.10) # c\n"),
            ("nginx.conf", "server { listen 80; } # c\n"),
            ("a.dart", "void main() { print('hi'); } // c\n"),
            ("a.ex", "defmodule A do\n  def f, do: :ok # c\nend\n"),
        ] {
            assert!(colors(file, code) >= 2, "{file} highlights in more than one color");
        }
    }

    #[test]
    fn fence_tags_find_new_languages() {
        for tag in ["ts", "typescript", "tsx", "jsx", "toml", "swift", "kotlin", "dockerfile", "shell", "powershell", "hcl", "proto", "graphql", "vue", "zig"] {
            assert!(highlight_snippet("x = 1", tag, DEFAULT_THEME).is_some(), "{tag}");
        }
    }

    #[test]
    fn markdown_fences_embed_other_languages() {
        let md = "# T\n\n```typescript\nconst x: number = 1;\n```\n";
        let buf = Buffer::from_str(md);
        let mut h = Highlighter::new();
        h.detect(Some(Path::new("a.md")), "");
        assert!(h.prepare(&buf, 0..=buf.len_lines() - 1));
        let line = h.line(3).unwrap();
        let distinct: std::collections::HashSet<_> = line.iter().map(|s| s.color).collect();
        assert!(distinct.len() >= 2, "code inside a fence is highlighted as TypeScript");
    }

    #[test]
    fn user_syntaxes_and_themes_load_and_broken_ones_are_reported() {
        let dir = std::env::temp_dir().join(format!("strata-user-syntax-{}", std::process::id()));
        let syntaxes = dir.join("Syntaxes");
        let themes = dir.join("Themes");
        std::fs::create_dir_all(&syntaxes).unwrap();
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(
            syntaxes.join("Widget.sublime-syntax"),
            "%YAML 1.2\n---\nname: Widget\nfile_extensions: [widget]\nscope: source.widget\ncontexts:\n  main:\n    - match: '\\bwidget\\b'\n      scope: keyword.control.widget\n    - match: '#.*'\n      scope: comment.line.widget\n",
        )
        .unwrap();
        std::fs::write(syntaxes.join("Broken.sublime-syntax"), "%YAML 1.2\n---\nname: [unclosed\n").unwrap();
        std::fs::write(
            syntaxes.join("BadRegex.sublime-syntax"),
            "%YAML 1.2\n---\nname: BadRegex\nfile_extensions: [badre]\nscope: source.badre\ncontexts:\n  main:\n    - match: '(unclosed'\n      scope: keyword.x\n",
        )
        .unwrap();
        // Including a grammar that doesn't exist must not take the editor down.
        std::fs::write(
            syntaxes.join("Dangling.sublime-syntax"),
            "%YAML 1.2\n---\nname: Dangling\nfile_extensions: [dangle]\nscope: source.dangle\ncontexts:\n  main:\n    - include: scope:source.nope\n    - match: x\n      scope: keyword.x\n",
        )
        .unwrap();
        std::fs::write(themes.join("Mine.tmTheme"), include_str!("../assets/themes/Strata Light.tmTheme")).unwrap();
        std::fs::write(themes.join("Junk.tmTheme"), "not a plist").unwrap();

        let (set, errors) = load_syntaxes(Some(&syntaxes));
        let widget = set.find_syntax_by_extension("widget").expect("user syntax loaded");
        assert_eq!(widget.name, "Widget");
        assert!(set.find_syntax_by_name("Rust").is_some(), "bundled syntaxes kept");
        assert!(set.find_syntax_by_name("BadRegex").is_none());
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors.iter().any(|e| e.contains("Broken.sublime-syntax")));
        assert!(errors.iter().any(|e| e.contains("BadRegex.sublime-syntax") && e.contains("regex")));
        // The user syntax parses and colors text.
        let mut parse = ParseState::new(widget);
        let ops = parse.parse_line("widget # c\n", &set).unwrap();
        assert!(ops.len() >= 2);
        let dangling = set.find_syntax_by_name("Dangling").unwrap();
        let _ = ParseState::new(dangling).parse_line("x y\n", &set);

        let (themes, errors) = load_themes(Some(&themes));
        assert!(themes.themes.contains_key("Mine"));
        assert!(themes.themes.contains_key("Strata Dark"));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("Junk"));

        // No folder: just the bundled set, no errors.
        let (_, errors) = load_syntaxes(Some(&dir.join("missing")));
        assert!(errors.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bundled_set_keeps_every_syntect_default() {
        let defaults = SyntaxSet::load_defaults_newlines();
        let names = syntax_names();
        // bat's Javadoc grammar (embedded in Java comments) is hidden, so it leaves the picker.
        let missing: Vec<_> = defaults
            .syntaxes()
            .iter()
            .filter(|s| !s.hidden && s.name != "JavaDoc" && !names.contains(&s.name.as_str()))
            .map(|s| s.name.as_str())
            .collect();
        assert!(missing.is_empty(), "{missing:?}");
        assert!(SYNTAX_LICENSES.contains("Kotlin"));
    }

    #[test]
    fn editors_warm_up_grammars_off_the_ui_thread() {
        let buf = Buffer::from_str("export const x: number = 1;\n");
        let mut h = Highlighter::for_editor();
        h.detect(Some(Path::new("warm.cts")), "");
        let start = std::time::Instant::now();
        while !h.prepare(&buf, 0..=0) {
            assert!(start.elapsed() < Duration::from_secs(30), "warm-up finishes");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(h.line(0).unwrap().len() >= 2);
        // Later highlighters of the same grammar don't wait.
        let mut again = Highlighter::for_editor();
        again.detect(Some(Path::new("b.ts")), "");
        assert!(again.prepare(&buf, 0..=0));
    }

    #[test]
    fn huge_files_yield_instead_of_blocking() {
        let text = "x\n".repeat(BUDGET * 3);
        let buf = Buffer::from_str(&text);
        let mut h = Highlighter::new();
        h.detect(Some(Path::new("a.py")), "");
        let end = buf.len_lines() - 1;
        assert!(!h.prepare(&buf, end..=end));
        let mut frames = 1;
        while !h.prepare(&buf, end..=end) {
            frames += 1;
        }
        assert!(frames >= 3);
        assert!(h.line(end).is_some());
    }
}
